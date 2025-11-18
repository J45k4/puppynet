use chrono::{DateTime, Utc};
#[cfg(feature = "rayon")]
use rayon::prelude::*;
use rusqlite::{Connection, ToSql};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::canonicalize;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
#[cfg(feature = "rayon")]
use std::sync::mpsc;
use walkdir::WalkDir;

pub type FileHash = [u8; 32];

#[derive(Debug, Default, Serialize)]
pub struct FileLocation {
	pub path: PathBuf,
	pub hash: Option<FileHash>,
	pub size: u64,
	pub mime_type: Option<String>,
	pub timestamp: DateTime<Utc>,
	pub created_at: Option<DateTime<Utc>>,
	pub modified_at: Option<DateTime<Utc>>,
	pub accessed_at: Option<DateTime<Utc>>,
}

impl PartialEq for FileLocation {
	fn eq(&self, other: &Self) -> bool {
		self.path == other.path
			&& self.hash == other.hash
			&& self.size == other.size
			&& self.mime_type == other.mime_type
			&& self.created_at == other.created_at
			&& self.modified_at == other.modified_at
			&& self.accessed_at == other.accessed_at
	}
}

#[cfg(feature = "ring")]
fn sha256_hash<R: Read>(mut reader: R) -> io::Result<[u8; 32]> {
	let mut context = ring::digest::Context::new(&ring::digest::SHA256);
	let mut buffer = [0u8; 4096];
	loop {
		let count = reader.read(&mut buffer)?;
		if count == 0 {
			break;
		}
		context.update(&buffer[..count]);
	}
	// Finalize the hash and copy it into a fixed-size array.
	let digest: ring::digest::Digest = context.finish();
	let mut hash = [0u8; 32];
	hash.copy_from_slice(digest.as_ref());
	Ok(hash)
}

#[cfg(all(not(feature = "ring"), feature = "sha2"))]
fn sha256_hash<R: Read>(mut reader: R) -> io::Result<[u8; 32]> {
	use sha2::Digest;
	let mut hasher = sha2::Sha256::new();
	let mut buffer = [0u8; 4096];
	loop {
		let count = reader.read(&mut buffer)?;
		if count == 0 {
			break;
		}
		hasher.update(&buffer[..count]);
	}
	Ok(hasher.finalize().into())
}

fn to_datetime(m: std::io::Result<std::time::SystemTime>) -> Option<chrono::DateTime<chrono::Utc>> {
	m.ok().map(|t| chrono::DateTime::from(t))
}

fn handle_path<P: AsRef<Path>>(path: P) -> FileLocation {
	let full_path = canonicalize(path.as_ref()).unwrap();
	log::info!("processing {}", full_path.display());
	let mut file = std::fs::File::open(path).unwrap();
	let m = file.metadata().unwrap();
	let created_at = to_datetime(m.created());
	let modified_at = to_datetime(m.modified());
	let accessed_at = to_datetime(m.accessed());
	let mut buffer = [0u8; 1024];
	let mime_type = match file.read(&mut buffer) {
		Ok(count) => match infer::get(&buffer[..count]).map(|mime| mime.to_string()) {
			Some(mime) => Some(mime),
			None => mime_guess::from_path(&full_path)
				.first()
				.map(|m| m.to_string()),
		},
		Err(_) => None,
	};
	file.seek(std::io::SeekFrom::Start(0)).unwrap();
	let hash = sha256_hash(file).unwrap();
	FileLocation {
		path: full_path,
		hash: Some(hash),
		size: m.len(),
		mime_type,
		timestamp: Utc::now(),
		created_at,
		modified_at,
		accessed_at,
	}
}

const INSERT_FILE_LOCATION: &str = "INSERT INTO file_locations (node_id, path, hash, size, timestamp, created_at, modified_at, accessed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
const UPDATE_FILE_LOCATION: &str = "UPDATE file_locations SET hash = ?, size = ?, timestamp = ?, created_at = ?, modified_at = ?, accessed_at = ? WHERE node_id = ? and path = ?";
const DELETE_FILE_LOCATION: &str = "DELETE FROM file_locations WHERE node_id = ? and path = ?";
const UPSERT_FILE_ENTRY: &str = "INSERT INTO file_entries (hash, size, mime_type, first_datetime, latest_datetime) VALUES (?, ?, ?, ?, ?) ON CONFLICT(hash) DO UPDATE SET latest_datetime = excluded.latest_datetime";

#[derive(Debug)]
pub struct ScanResult {
	pub updated_count: u64,
	pub inserted_count: u64,
	pub removed_count: u64,
	pub duration: std::time::Duration,
}

const PROGRESS_REPORT_INTERVAL: usize = 25;

#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
	pub total_files: usize,
	pub processed_files: usize,
	pub inserted_count: u64,
	pub updated_count: u64,
	pub removed_count: u64,
}

fn report_progress<F: FnMut(ScanProgress)>(
	cb: &mut F,
	total_files: usize,
	processed_files: usize,
	inserted_count: u64,
	updated_count: u64,
	removed_count: u64,
) {
	cb(ScanProgress {
		total_files,
		processed_files,
		inserted_count,
		updated_count,
		removed_count,
	});
}

fn should_emit_progress(processed: usize, total: usize) -> bool {
	processed == total
		|| processed == 0
		|| processed % PROGRESS_REPORT_INTERVAL == 0
		|| total < PROGRESS_REPORT_INTERVAL
}

pub fn scan<P: AsRef<Path>>(
	node_id: &[u8],
	path: P,
	conn: Connection,
) -> Result<ScanResult, String> {
	scan_with_progress(node_id, path, conn, |_| {})
}

pub fn scan_with_progress<P, F>(
	node_id: &[u8],
	path: P,
	mut conn: Connection,
	mut progress: F,
) -> Result<ScanResult, String>
where
	P: AsRef<Path>,
	F: FnMut(ScanProgress),
{
	let timer = std::time::Instant::now();
	let mut updated_count = 0;
	let mut inserted_count = 0;
	let mut removed_count = 0;
	let path = path.as_ref().to_path_buf();
	let absolute_path = canonicalize(&path).unwrap();
	let tx = conn.transaction().unwrap();
	let mut processed_files = 0usize;

	// load all existing file_locations into a map
	let existing: HashMap<PathBuf, FileLocation> = {
		let mut file_locations_stmt = tx
			.prepare(
				"SELECT path, hash, size, timestamp, created_at, modified_at, accessed_at \
			FROM file_locations \
			WHERE path LIKE ?",
			)
			.map_err(|e| format!("error preparing statement: {:?}", e))?;
		file_locations_stmt
			.query_map(
				[&(absolute_path.to_string_lossy().to_string() + "%")],
				|row| {
					Ok(FileLocation {
						path: PathBuf::from(row.get::<_, String>(0)?),
						hash: row.get(1)?,
						size: row.get(2)?,
						mime_type: None, // not needed for comparison
						timestamp: row.get(3)?,
						created_at: row.get(4)?,
						modified_at: row.get(5)?,
						accessed_at: row.get(6)?,
					})
				},
			)
			.map_err(|e| format!("error querying file locations: {:?}", e))?
			.filter_map(Result::ok)
			.map(|fl| (fl.path.clone(), fl))
			.collect()
	};

	let entries = WalkDir::new(&absolute_path)
		.into_iter()
		.filter_map(|e| e.ok())
		.filter(|e| e.file_type().is_file())
		.collect::<Vec<_>>();
	let total_files = entries.len();
	report_progress(
		&mut progress,
		total_files,
		processed_files,
		inserted_count,
		updated_count,
		removed_count,
	);

	let mut scanned: HashMap<PathBuf, FileLocation>;
	#[cfg(feature = "rayon")]
	{
		let (tx, rx) = mpsc::channel();
		let existing = &existing;
		entries
			.par_iter()
			.for_each_with(tx.clone(), |sender, entry| {
				let pbuf = entry.path().to_path_buf();
				let meta = std::fs::metadata(&pbuf).unwrap();
				let created_at = to_datetime(meta.created());
				let modified_at = to_datetime(meta.modified());
				let accessed_at = to_datetime(meta.accessed());
				let size = meta.len();
				let fl = if let Some(prev) = existing.get(&pbuf) {
					if prev.size == size
						&& prev.created_at == created_at
						&& prev.modified_at == modified_at
						&& prev.accessed_at == accessed_at
					{
						FileLocation {
							path: pbuf.clone(),
							hash: prev.hash,
							size,
							mime_type: prev.mime_type.clone(),
							timestamp: Utc::now(),
							created_at,
							modified_at,
							accessed_at,
						}
					} else {
						handle_path(&pbuf)
					}
				} else {
					handle_path(&pbuf)
				};
				let _ = sender.send((pbuf, fl));
			});
		drop(tx);
		let mut map = HashMap::new();
		for (pbuf, fl) in rx {
			map.insert(pbuf, fl);
			processed_files += 1;
			if should_emit_progress(processed_files, total_files) {
				report_progress(
					&mut progress,
					total_files,
					processed_files,
					inserted_count,
					updated_count,
					removed_count,
				);
			}
		}
		scanned = map;
	}
	#[cfg(not(feature = "rayon"))]
	{
		let mut map = HashMap::new();
		for entry in entries {
			let pbuf = entry.path().to_path_buf();
			let meta = std::fs::metadata(&pbuf).unwrap();
			let created_at = to_datetime(meta.created());
			let modified_at = to_datetime(meta.modified());
			let accessed_at = to_datetime(meta.accessed());
			let size = meta.len();

			let fl = if let Some(prev) = existing.get(&pbuf) {
				if prev.size == size
					&& prev.created_at == created_at
					&& prev.modified_at == modified_at
					&& prev.accessed_at == accessed_at
				{
					FileLocation {
						path: pbuf.clone(),
						hash: prev.hash,
						size,
						mime_type: prev.mime_type.clone(),
						timestamp: Utc::now(),
						created_at,
						modified_at,
						accessed_at,
					}
				} else {
					handle_path(&pbuf)
				}
			} else {
				handle_path(&pbuf)
			};

			map.insert(pbuf, fl);
			processed_files += 1;
			if should_emit_progress(processed_files, total_files) {
				report_progress(
					&mut progress,
					total_files,
					processed_files,
					inserted_count,
					updated_count,
					removed_count,
				);
			}
		}
		scanned = map;
	}

	// remove deleted files
	{
		let mut delete_stmt = tx.prepare(DELETE_FILE_LOCATION).unwrap();
		for old in existing.keys() {
			if !scanned.contains_key(old) {
				delete_stmt
					.execute(&[&node_id as &dyn ToSql, &old.to_string_lossy() as &dyn ToSql])
					.unwrap();
				removed_count += 1;
				report_progress(
					&mut progress,
					total_files,
					processed_files,
					inserted_count,
					updated_count,
					removed_count,
				);
			}
		}
	}

	{
		let mut insert_stmt = tx.prepare(INSERT_FILE_LOCATION).unwrap();
		let mut update_stmt = tx.prepare(UPDATE_FILE_LOCATION).unwrap();
		for (path, fl) in scanned.iter() {
			if let Some(prev) = existing.get(path) {
				if fl == prev {
					continue;
				}
				update_stmt
					.execute(&[
						&fl.hash as &dyn ToSql,
						&fl.size as &dyn ToSql,
						&fl.timestamp as &dyn ToSql,
						&fl.created_at as &dyn ToSql,
						&fl.modified_at as &dyn ToSql,
						&fl.accessed_at as &dyn ToSql,
						&node_id as &dyn ToSql,
						&fl.path.to_string_lossy() as &dyn ToSql,
					])
					.unwrap();
				updated_count += 1;
			} else {
				insert_stmt
					.execute(&[
						&node_id as &dyn ToSql,
						&fl.path.to_string_lossy() as &dyn ToSql,
						&fl.hash as &dyn ToSql,
						&fl.size as &dyn ToSql,
						&fl.timestamp as &dyn ToSql,
						&fl.created_at as &dyn ToSql,
						&fl.modified_at as &dyn ToSql,
						&fl.accessed_at as &dyn ToSql,
					])
					.unwrap();
				inserted_count += 1;
			}
			report_progress(
				&mut progress,
				total_files,
				processed_files,
				inserted_count,
				updated_count,
				removed_count,
			);
		}
	}

	{
		let mut upsert_stmt = tx.prepare(UPSERT_FILE_ENTRY).unwrap();
		for fl in scanned.values() {
			let timestamps: Vec<_> = [fl.created_at, fl.modified_at, fl.accessed_at]
				.iter()
				.copied()
				.flatten()
				.collect();
			let first_dt = timestamps.iter().min().copied();
			let latest_dt = timestamps.iter().max().copied();
			upsert_stmt
				.execute(&[
					&fl.hash as &dyn ToSql,
					&fl.size as &dyn ToSql,
					&fl.mime_type as &dyn ToSql,
					&first_dt as &dyn ToSql,
					&latest_dt as &dyn ToSql,
				])
				.unwrap();
		}
	}

	tx.commit().unwrap();
	report_progress(
		&mut progress,
		total_files,
		processed_files,
		inserted_count,
		updated_count,
		removed_count,
	);
	Ok(ScanResult {
		updated_count,
		inserted_count,
		removed_count,
		duration: timer.elapsed(),
	})
}
