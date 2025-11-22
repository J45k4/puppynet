use crate::p2p::{
	AuthMethod, CpuInfo, DirEntry, DiskInfo, FileWriteAck, InterfaceInfo, PeerReq, PeerRes,
	Thumbnail,
};
use crate::types::FileChunk;
use crate::updater::{self, UpdateProgress, UpdateResult};
use crate::{
	db::{
		Cpu as DbCpu, FileEntry, Interface as DbInterface, Node, NodeID, StorageUsageFile,
		fetch_file_entries_paginated, load_peer_permissions, open_db, remove_stale_cpus,
		remove_stale_interfaces, run_migrations, save_cpu, save_interface, save_node,
	},
	p2p::{AgentBehaviour, AgentEvent, build_swarm, load_or_generate_keypair},
	scan::{self, ScanEvent},
	state::{Connection, FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FolderRule, Permission, State},
};
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::executor::block_on;
use libp2p::{PeerId, Swarm, mdns, swarm::SwarmEvent};
use rusqlite::{Connection as SqliteConnection, params};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::{
	env,
	net::IpAddr,
	path::{Path, PathBuf},
	sync::atomic::{AtomicU64, Ordering},
};
use sysinfo::{Disks, Networks, System};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::{
	sync::{
		mpsc::{UnboundedReceiver, UnboundedSender},
		oneshot,
	},
	task::JoinHandle,
};

use libp2p::request_response::OutboundRequestId;

pub struct ReadFileCmd {
	peer_id: libp2p::PeerId,
	path: String,
	offset: u64,
	length: Option<u64>,
	tx: oneshot::Sender<Result<FileChunk>>,
}

pub enum Command {
	Connect {
		peer_id: libp2p::PeerId,
		addr: libp2p::Multiaddr,
	},
	ListDir {
		peer: libp2p::PeerId,
		path: String,
		tx: oneshot::Sender<Result<Vec<DirEntry>>>,
	},
	ListCpus {
		tx: oneshot::Sender<Result<Vec<CpuInfo>>>,
		peer_id: PeerId,
	},
	ListDisks {
		tx: oneshot::Sender<Result<Vec<DiskInfo>>>,
		peer_id: PeerId,
	},
	ListInterfaces {
		tx: oneshot::Sender<Result<Vec<InterfaceInfo>>>,
		peer_id: PeerId,
	},
	ListFileEntries {
		peer: PeerId,
		offset: u64,
		limit: u64,
		tx: oneshot::Sender<Result<Vec<FileEntry>>>,
	},
	ListStorageFiles {
		tx: oneshot::Sender<Result<Vec<StorageUsageFile>>>,
	},
	ListPermissions {
		peer: PeerId,
		tx: oneshot::Sender<Result<Vec<Permission>>>,
	},
	ReadFile(ReadFileCmd),
	Scan {
		path: String,
		tx: mpsc::Sender<ScanEvent>,
	},
	RemoteScan {
		peer: PeerId,
		path: String,
		scan_id: u64,
	},
	GetThumbnail {
		peer: PeerId,
		path: String,
		max_width: u32,
		max_height: u32,
		tx: oneshot::Sender<Result<Thumbnail>>,
	},
	/// Request a remote peer to update itself
	RemoteUpdate {
		peer: PeerId,
		version: Option<String>,
		update_id: u64,
	},
}

async fn read_file(path: &Path, offset: u64, length: Option<u64>) -> Result<FileChunk> {
	let file = fs::File::open(path).await?;
	let metadata = file.metadata().await?;
	if metadata.is_dir() {
		bail!("path is a directory")
	}
	let file_len = metadata.len();
	if offset >= file_len {
		return Ok(FileChunk {
			offset,
			data: Vec::new(),
			eof: true,
		});
	}
	let remaining = file_len - offset;
	let to_read = match length {
		Some(l) => l.min(remaining),
		None => remaining,
	};
	let mut reader = tokio::io::BufReader::new(file);
	reader.seek(std::io::SeekFrom::Start(offset)).await?;
	let mut buffer = vec![0u8; to_read as usize];
	let n = reader.read(&mut buffer).await?;
	buffer.truncate(n);
	let eof = offset + n as u64 >= file_len;
	Ok(FileChunk {
		offset,
		data: buffer,
		eof,
	})
}

async fn write_file(path: &Path, offset: u64, data: &[u8]) -> Result<FileWriteAck> {
	// Open (or create) file with write capability
	let mut file = match fs::OpenOptions::new()
		.create(true)
		.write(true)
		.read(true)
		.open(path)
		.await
	{
		Ok(f) => f,
		Err(e) => return Err(anyhow!("open failed: {}", e)),
	};
	// Ensure we don't overflow length when extending
	let current_len = match file.metadata().await {
		Ok(m) => m.len(),
		Err(e) => return Err(anyhow!("metadata failed: {}", e)),
	};
	let required_len = match offset.checked_add(data.len() as u64) {
		Some(v) => v,
		None => return Err(anyhow!("length overflow")),
	};
	if required_len > current_len {
		if let Err(e) = file.set_len(required_len).await {
			return Err(anyhow!("set_len failed: {}", e));
		}
	}
	if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
		return Err(anyhow!("seek failed: {}", e));
	}
	if let Err(e) = file.write_all(data).await {
		return Err(anyhow!("write failed: {}", e));
	}
	Ok(FileWriteAck {
		bytes_written: data.len() as u64,
	})
}

async fn generate_thumbnail(path: &Path, max_width: u32, max_height: u32) -> Result<Thumbnail> {
	use image::ImageReader;
	use std::io::Cursor;

	// Read the file data
	let data = fs::read(path).await?;

	// Use spawn_blocking for CPU-intensive image processing
	let result = tokio::task::spawn_blocking(move || -> Result<Thumbnail> {
		// Load the image
		let reader = ImageReader::new(Cursor::new(&data))
			.with_guessed_format()
			.map_err(|e| anyhow!("failed to guess image format: {}", e))?;

		let format = reader.format();
		let img = reader
			.decode()
			.map_err(|e| anyhow!("failed to decode image: {}", e))?;

		// Calculate thumbnail dimensions maintaining aspect ratio
		let (orig_width, orig_height) = (img.width(), img.height());
		let (thumb_width, thumb_height) = if orig_width <= max_width && orig_height <= max_height {
			// Image is already smaller than requested thumbnail size
			(orig_width, orig_height)
		} else {
			let width_ratio = max_width as f64 / orig_width as f64;
			let height_ratio = max_height as f64 / orig_height as f64;
			let ratio = width_ratio.min(height_ratio);
			(
				(orig_width as f64 * ratio).round() as u32,
				(orig_height as f64 * ratio).round() as u32,
			)
		};

		// Resize the image
		let thumbnail = img.thumbnail(thumb_width, thumb_height);

		// Encode the thumbnail as JPEG for smaller size
		let mut output = Vec::new();
		let mut cursor = Cursor::new(&mut output);
		thumbnail
			.write_to(&mut cursor, image::ImageFormat::Jpeg)
			.map_err(|e| anyhow!("failed to encode thumbnail: {}", e))?;

		Ok(Thumbnail {
			data: output,
			width: thumbnail.width(),
			height: thumbnail.height(),
			mime_type: String::from("image/jpeg"),
		})
	})
	.await
	.map_err(|e| anyhow!("thumbnail task panicked: {}", e))??;

	Ok(result)
}

pub struct App {
	state: Arc<Mutex<State>>,
	swarm: Swarm<AgentBehaviour>,
	rx: UnboundedReceiver<Command>,
	internal_rx: tokio::sync::mpsc::UnboundedReceiver<InternalCommand>,
	internal_tx: tokio::sync::mpsc::UnboundedSender<InternalCommand>,
	pending_requests: HashMap<OutboundRequestId, PendingRequest>,
	system: System,
	db: Arc<Mutex<SqliteConnection>>,
	remote_scans: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
	remote_updates: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
}

trait ResponseDecoder: Sized + Send + 'static {
	fn decode(response: PeerRes) -> anyhow::Result<Self>;
}

impl ResponseDecoder for Vec<DirEntry> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::DirEntries(entries) => Ok(entries),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<CpuInfo> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::Cpus(cpus) => Ok(cpus),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<DiskInfo> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::Disks(disks) => Ok(disks),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<FileEntry> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::FileEntries(entries) => Ok(entries),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<InterfaceInfo> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::Interfaces(interfaces) => Ok(interfaces),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<Permission> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::Permissions(perms) => Ok(perms),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for FileChunk {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::FileChunk(chunk) => Ok(chunk),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Thumbnail {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::Thumbnail(thumb) => Ok(thumb),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for UpdateResult {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::UpdateStarted(Ok(())) => Ok(UpdateResult {
				success: true,
				message: "Update started".to_string(),
				new_version: None,
			}),
			PeerRes::UpdateStarted(Err(err)) => Ok(UpdateResult {
				success: false,
				message: err,
				new_version: None,
			}),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

trait PendingResponseHandler: Send {
	fn complete(self: Box<Self>, response: PeerRes);
	fn fail(self: Box<Self>, error: anyhow::Error);
}

struct Pending<T: ResponseDecoder> {
	tx: oneshot::Sender<Result<T>>,
}

impl<T: ResponseDecoder> Pending<T> {
	fn new(tx: oneshot::Sender<Result<T>>) -> PendingRequest {
		Box::new(Self { tx })
	}
}

struct PendingRemoteScanStart {
	scan_id: u64,
	channels: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
}

impl PendingRemoteScanStart {
	fn new(
		scan_id: u64,
		channels: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
	) -> PendingRequest {
		Box::new(Self { scan_id, channels })
	}
}

struct PendingScanEventAck;

impl PendingScanEventAck {
	fn new() -> PendingRequest {
		Box::new(Self)
	}
}

impl PendingResponseHandler for PendingScanEventAck {
	fn complete(self: Box<Self>, _response: PeerRes) {}

	fn fail(self: Box<Self>, error: anyhow::Error) {
		log::warn!("scan event delivery failed: {}", error);
	}
}

struct PendingRemoteUpdateStart {
	update_id: u64,
	channels: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
}

impl PendingRemoteUpdateStart {
	fn new(
		update_id: u64,
		channels: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
	) -> PendingRequest {
		Box::new(Self { update_id, channels })
	}
}

impl PendingResponseHandler for PendingRemoteUpdateStart {
	fn complete(self: Box<Self>, response: PeerRes) {
		match response {
			PeerRes::UpdateStarted(Ok(())) => {}
			PeerRes::UpdateStarted(Err(err)) => {
				if let Some(tx) = self.channels.lock().unwrap().remove(&self.update_id) {
					let _ = tx.send(UpdateProgress::Failed { error: err });
				}
			}
			other => {
				log::warn!("unexpected response for remote update start {:?}", other);
			}
		}
	}

	fn fail(self: Box<Self>, error: anyhow::Error) {
		if let Some(tx) = self.channels.lock().unwrap().remove(&self.update_id) {
			let _ = tx.send(UpdateProgress::Failed {
				error: error.to_string(),
			});
		}
	}
}

struct PendingUpdateEventAck;

impl PendingUpdateEventAck {
	fn new() -> PendingRequest {
		Box::new(Self)
	}
}

impl PendingResponseHandler for PendingUpdateEventAck {
	fn complete(self: Box<Self>, _response: PeerRes) {}

	fn fail(self: Box<Self>, error: anyhow::Error) {
		log::warn!("update event delivery failed: {}", error);
	}
}

impl PendingResponseHandler for PendingRemoteScanStart {
	fn complete(self: Box<Self>, response: PeerRes) {
		match response {
			PeerRes::ScanStarted(Ok(())) => {}
			PeerRes::ScanStarted(Err(err)) => {
				if let Some(tx) = self.channels.lock().unwrap().remove(&self.scan_id) {
					let _ = tx.send(ScanEvent::Finished(Err(err)));
				}
			}
			other => {
				log::warn!("unexpected response for remote scan start {:?}", other);
			}
		}
	}

	fn fail(self: Box<Self>, error: anyhow::Error) {
		if let Some(tx) = self.channels.lock().unwrap().remove(&self.scan_id) {
			let _ = tx.send(ScanEvent::Finished(Err(error.to_string())));
		}
	}
}

impl<T: ResponseDecoder> PendingResponseHandler for Pending<T> {
	fn complete(self: Box<Self>, response: PeerRes) {
		let result = match response {
			PeerRes::Error(err) => Err(anyhow!(err)),
			other => T::decode(other),
		};
		let _ = self.tx.send(result);
	}

	fn fail(self: Box<Self>, error: anyhow::Error) {
		let _ = self.tx.send(Err(error));
	}
}

enum InternalCommand {
	SendScanEvent {
		target: PeerId,
		scan_id: u64,
		event: ScanEvent,
	},
	SendUpdateEvent {
		target: PeerId,
		update_id: u64,
		event: UpdateProgress,
	},
}

type PendingRequest = Box<dyn PendingResponseHandler>;

impl App {
	fn can_access(&self, peer: PeerId, path: &Path, access: u8) -> bool {
		self.state
			.lock()
			.map(|state| state.has_fs_access(peer, path, access))
			.unwrap_or(false)
	}

	pub fn new(
		state: Arc<Mutex<State>>,
		db: Arc<Mutex<SqliteConnection>>,
		remote_scans: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
		remote_updates: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
	) -> (Self, tokio::sync::mpsc::UnboundedSender<Command>) {
		let key_path = env::var("KEYPAIR").unwrap_or_else(|_| String::from("peer_keypair.bin"));
		let key_path = Path::new(&key_path);
		if !key_path.exists() {
			log::warn!(
				"keypair file {} does not exist, generating new keypair",
				key_path.display()
			);
		}
		let id_keys = load_or_generate_keypair(key_path).unwrap_or_else(|err| {
			log::warn!(
				"failed to load persisted keypair at {}: {err}; using ephemeral keypair",
				key_path.display()
			);
			libp2p::identity::Keypair::generate_ed25519()
		});
		let peer_id = PeerId::from(id_keys.public());

		let mut swarm = build_swarm(id_keys, peer_id).unwrap();
		let stored_permissions = {
			let conn = db.lock().unwrap();
			match load_peer_permissions(&conn, &peer_id) {
				Ok(perms) => perms,
				Err(err) => {
					log::error!("failed to load peer permissions: {err}");
					Vec::new()
				}
			}
		};
		let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
		let (internal_tx, internal_rx) = tokio::sync::mpsc::unbounded_channel();

		let listen_addr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
		if let Err(err) = swarm.listen_on(listen_addr) {
			log::warn!("failed to start swarm listener: {err}");
		}
		{
			if let Ok(mut s) = state.lock() {
				s.me = peer_id;
				for (target, permissions) in stored_permissions {
					s.set_peer_permissions_from_storage(target, permissions);
				}
			}
		}
		let mut app = App {
			state,
			swarm,
			rx,
			internal_rx,
			internal_tx,
			pending_requests: HashMap::new(),
			system: System::new(),
			db,
			remote_scans,
			remote_updates,
		};
		app.normalize_file_location_node_ids();
		app.persist_local_node();
		(app, tx)
	}

	async fn handle_puppy_peer_req(
		&mut self,
		peer: PeerId,
		req: PeerReq,
	) -> anyhow::Result<PeerRes> {
		let res = match req {
			PeerReq::ListDir { path } => {
				log::info!("[{}] ListDir {}", peer, path);
				let canonical = match fs::canonicalize(&path).await {
					Ok(p) => p,
					Err(err) => {
						log::warn!("failed to canonicalize directory {}: {err}", path);
						return Ok(PeerRes::Error(format!("Failed to access directory: {err}")));
					}
				};
				if !self.can_access(peer, &canonical, FLAG_READ | FLAG_SEARCH) {
					log::warn!(
						"peer {} denied directory listing for {}",
						peer,
						canonical.display()
					);
					return Ok(PeerRes::Error("Access denied".into()));
				}
				let entries = Self::collect_dir_entries(&canonical).await?;
				PeerRes::DirEntries(entries)
			}
			PeerReq::StatFile { path } => {
				log::info!("[{}] StatFile {}", peer, path);
				let canonical = match fs::canonicalize(&path).await {
					Ok(p) => p,
					Err(err) => {
						log::warn!("failed to canonicalize file {}: {err}", path);
						return Ok(PeerRes::Error(format!("Failed to access file: {err}")));
					}
				};
				if !self.can_access(peer, &canonical, FLAG_READ | FLAG_SEARCH) {
					log::warn!("peer {} denied stat for {}", peer, canonical.display());
					return Ok(PeerRes::Error("Access denied".into()));
				}
				let meta = fs::metadata(&canonical).await?;
				let file_type = meta.file_type();
				let ext = canonical
					.extension()
					.and_then(|s| s.to_str().map(|s| s.to_string()));
				let mime = if file_type.is_dir() {
					None
				} else {
					mime_guess::from_path(&canonical)
						.first_raw()
						.map(|value| value.to_string())
				};
				PeerRes::FileStat(DirEntry {
					name: canonical
						.file_name()
						.and_then(|s| s.to_str().map(|s| s.to_string()))
						.unwrap_or_default(),
					is_dir: file_type.is_dir(),
					extension: ext,
					mime,
					size: meta.len(),
					created_at: meta
						.created()
						.ok()
						.and_then(|t| DateTime::<Utc>::from(t).into()),
					modified_at: meta
						.modified()
						.ok()
						.and_then(|t| DateTime::<Utc>::from(t).into()),
					accessed_at: meta
						.accessed()
						.ok()
						.and_then(|t| DateTime::<Utc>::from(t).into()),
				})
			}
			PeerReq::ReadFile {
				path,
				offset,
				length,
			} => {
				log::info!(
					"[{}] ReadFile {} (offset {}, length {:?})",
					peer,
					path,
					offset,
					length
				);
				let canonical = match fs::canonicalize(&path).await {
					Ok(p) => p,
					Err(err) => {
						log::warn!("failed to canonicalize read path {}: {err}", path);
						return Ok(PeerRes::Error(format!("Failed to access file: {err}")));
					}
				};
				if !self.can_access(peer, &canonical, FLAG_READ | FLAG_SEARCH) {
					log::warn!("peer {} denied read for {}", peer, canonical.display());
					return Ok(PeerRes::Error("Access denied".into()));
				}
				PeerRes::FileChunk(read_file(canonical.as_path(), offset, length).await?)
			}
			PeerReq::WriteFile { path, offset, data } => {
				log::info!(
					"[{}] WriteFile {} (offset {}, {} bytes)",
					peer,
					path,
					offset,
					data.len()
				);
				let requested_path = PathBuf::from(&path);
				let canonical = match fs::metadata(&requested_path).await {
					Ok(_) => match fs::canonicalize(&requested_path).await {
						Ok(p) => p,
						Err(err) => {
							log::warn!("failed to canonicalize write path {}: {err}", path);
							return Ok(PeerRes::Error(format!("Failed to access file: {err}")));
						}
					},
					Err(_) => {
						let parent = match requested_path.parent() {
							Some(p) => p,
							None => {
								log::warn!("peer {} provided invalid write path {}", peer, path);
								return Ok(PeerRes::Error("Invalid path".into()));
							}
						};
						let canonical_parent = match fs::canonicalize(parent).await {
							Ok(p) => p,
							Err(err) => {
								log::warn!(
									"failed to canonicalize parent {} for write: {err}",
									parent.display()
								);
								return Ok(PeerRes::Error(format!(
									"Failed to access parent directory: {err}"
								)));
							}
						};
						match requested_path.file_name() {
							Some(name) => canonical_parent.join(name),
							None => {
								log::warn!(
									"peer {} provided invalid file name in path {}",
									peer,
									path
								);
								return Ok(PeerRes::Error("Invalid file name".into()));
							}
						}
					}
				};
				if !self.can_access(peer, &canonical, FLAG_WRITE | FLAG_READ | FLAG_SEARCH) {
					log::warn!("peer {} denied write for {}", peer, canonical.display());
					return Ok(PeerRes::Error("Access denied".into()));
				}
				PeerRes::WriteAck(write_file(canonical.as_path(), offset, &data).await?)
			}
			PeerReq::ListCpus => {
				let cpus = self.collect_cpu_info();
				PeerRes::Cpus(cpus)
			}
			PeerReq::ListDisks => {
				let disks = self.collect_disk_info();
				PeerRes::Disks(disks)
			}
			PeerReq::ListInterfaces => {
				let interfaces = self.collect_interface_info();
				PeerRes::Interfaces(interfaces)
			}
			PeerReq::FileEntries { offset, limit } => {
				match self.fetch_file_entries(offset, limit) {
					Ok(entries) => PeerRes::FileEntries(entries),
					Err(err) => {
						log::error!("failed to load file entries: {err}");
						PeerRes::Error(format!("failed to load file entries: {err}"))
					}
				}
			}
			PeerReq::StartScan { id, path } => {
				let requested_path = PathBuf::from(&path);
				let canonical = match fs::canonicalize(&requested_path).await {
					Ok(path) => path,
					Err(err) => {
						log::warn!("failed to canonicalize scan path {}: {err}", path);
						return Ok(PeerRes::ScanStarted(Err(format!(
							"failed to access path: {err}"
						))));
					}
				};
				if !self.can_access(peer, &canonical, FLAG_READ | FLAG_SEARCH) {
					return Ok(PeerRes::ScanStarted(Err(String::from("Access denied"))));
				}
				let node_id = match self.local_node_id() {
					Some(id) => id,
					None => {
						return Ok(PeerRes::ScanStarted(Err(String::from(
							"failed to determine node id",
						))));
					}
				};
				let db = Arc::clone(&self.db);
				let internal_tx = self.internal_tx.clone();
				let path_string = canonical.to_string_lossy().to_string();
				let target = peer;
				tokio::spawn(async move {
					let (progress_tx, mut progress_rx) =
						tokio::sync::mpsc::unbounded_channel::<ScanEvent>();
					let forward = tokio::spawn({
						let internal_tx = internal_tx.clone();
						async move {
							while let Some(event) = progress_rx.recv().await {
								let _ = internal_tx.send(InternalCommand::SendScanEvent {
									target,
									scan_id: id,
									event,
								});
							}
						}
					});
					let _ = tokio::task::spawn_blocking(move || {
						let result = db
							.lock()
							.map_err(|err| format!("db lock poisoned: {err}"))
							.and_then(|mut conn| {
								scan::scan_with_progress(
									&node_id,
									&path_string,
									&mut *conn,
									|progress| {
										let _ =
											progress_tx.send(ScanEvent::Progress(progress.clone()));
									},
								)
							});
						let final_event = match result {
							Ok(stats) => ScanEvent::Finished(Ok(stats)),
							Err(err) => ScanEvent::Finished(Err(err)),
						};
						let _ = progress_tx.send(final_event);
					})
					.await;
					let _ = forward.await;
				});
				PeerRes::ScanStarted(Ok(()))
			}
			PeerReq::ScanEvent { id, event } => {
				let mut map = self.remote_scans.lock().unwrap();
				if let Some(tx) = map.get(&id) {
					let _ = tx.send(event.clone());
					if matches!(event, ScanEvent::Finished(_)) {
						map.remove(&id);
					}
				} else {
					log::warn!("received scan event for unknown id {}", id);
				}
				PeerRes::ScanEventAck
			}
			PeerReq::ListPermissions => {
				log::info!("[{}] ListPermissions", peer);
				let permissions = match self.state.lock() {
					Ok(state) => state.permissions_for_peer(&peer),
					Err(err) => {
						log::error!("state lock poisoned while listing permissions: {}", err);
						return Ok(PeerRes::Error("State unavailable".into()));
					}
				};
				PeerRes::Permissions(permissions)
			}
			PeerReq::Authenticate { method } => match method {
				AuthMethod::Token { token } => todo!(),
				AuthMethod::Credentials { username, password } => todo!(),
			},
			PeerReq::CreateUser {
				username,
				password,
				roles,
				permissions,
			} => {
				let mut state = self.state.lock().unwrap();
				state.create_user(username.clone(), password)?;
				PeerRes::UserCreated { username }
			}
			PeerReq::CreateToken {
				username,
				label,
				expires_in,
				permissions,
			} => {
				let mut state = self.state.lock().unwrap();
				if !state.users.iter().any(|u| u.name == username) {
					return Ok(PeerRes::Error("User does not exist".into()));
				}
				PeerRes::TokenIssued {
					token: "".into(),
					token_id: "".into(),
					username: username.clone(),
					permissions: Vec::new(),
					expires_at: None,
				}
			}
			PeerReq::GrantAccess { .. } => PeerRes::Error("GrantAccess not implemented".into()),
			PeerReq::ListUsers => PeerRes::Error("ListUsers not implemented".into()),
			PeerReq::ListTokens { .. } => PeerRes::Error("ListTokens not implemented".into()),
			PeerReq::RevokeToken { .. } => PeerRes::Error("RevokeToken not implemented".into()),
			PeerReq::RevokeUser { .. } => PeerRes::Error("RevokeUser not implemented".into()),
			PeerReq::GetThumbnail {
				path,
				max_width,
				max_height,
			} => {
				log::info!(
					"[{}] GetThumbnail {} ({}x{})",
					peer,
					path,
					max_width,
					max_height
				);
				let canonical = match fs::canonicalize(&path).await {
					Ok(p) => p,
					Err(err) => {
						log::warn!("failed to canonicalize thumbnail path {}: {err}", path);
						return Ok(PeerRes::Error(format!("Failed to access file: {err}")));
					}
				};
				if !self.can_access(peer, &canonical, FLAG_READ | FLAG_SEARCH) {
					log::warn!(
						"peer {} denied thumbnail access for {}",
						peer,
						canonical.display()
					);
					return Ok(PeerRes::Error("Access denied".into()));
				}
				match generate_thumbnail(&canonical, max_width, max_height).await {
					Ok(thumb) => PeerRes::Thumbnail(thumb),
					Err(err) => {
						log::warn!("failed to generate thumbnail for {}: {err}", path);
						PeerRes::Error(format!("Failed to generate thumbnail: {err}"))
					}
				}
			}
			PeerReq::UpdateSelf { id, version } => {
				log::info!("[{}] UpdateSelf (id: {}, version: {:?})", peer, id, version);

				// Get current version (0 if unknown)
				let current_version = 0u32; // TODO: Get actual version from build info

				let internal_tx = self.internal_tx.clone();
				let target = peer;
				let version_clone = version.clone();

				let internal_tx_for_error = internal_tx.clone();
				tokio::spawn(async move {
					let result = updater::update_with_progress(
						version_clone.as_deref(),
						current_version,
						move |progress| {
							let _ = internal_tx.send(InternalCommand::SendUpdateEvent {
								target,
								update_id: id,
								event: progress,
							});
						},
					).await;

					// If update_with_progress failed with an error (not a result),
					// send a failure event
					if let Err(err) = result {
						let _ = internal_tx_for_error.send(InternalCommand::SendUpdateEvent {
							target,
							update_id: id,
							event: UpdateProgress::Failed { error: err.to_string() },
						});
					}
				});

				PeerRes::UpdateStarted(Ok(()))
			}
			PeerReq::UpdateEvent { id, event } => {
				log::debug!("[{}] UpdateEvent (id: {})", peer, id);
				let mut map = self.remote_updates.lock().unwrap();
				if let Some(tx) = map.get(&id) {
					let _ = tx.send(event.clone());
					if matches!(event, UpdateProgress::Completed { .. } | UpdateProgress::Failed { .. } | UpdateProgress::AlreadyUpToDate { .. }) {
						map.remove(&id);
					}
				} else {
					log::warn!("received update event for unknown id {}", id);
				}
				PeerRes::UpdateEventAck
			}
		};
		Ok(res)
	}

	fn collect_cpu_info(&mut self) -> Vec<CpuInfo> {
		self.system.refresh_cpu_usage();
		let cpus: Vec<CpuInfo> = self
			.system
			.cpus()
			.iter()
			.map(|cpu| CpuInfo {
				name: cpu.name().to_string(),
				usage: cpu.cpu_usage(),
				frequency_hz: cpu.frequency(),
			})
			.collect();
		self.persist_local_cpus(&cpus);
		cpus
	}

	fn persist_local_node(&mut self) {
		let node_id = match self.local_node_id() {
			Some(id) => id,
			None => return,
		};
		self.system.refresh_memory();
		let now = Utc::now();
		let node = Node {
			id: node_id,
			name: System::host_name().unwrap_or_else(|| String::from("local-node")),
			you: true,
			total_memory: self.system.total_memory(),
			system_name: System::name().unwrap_or_else(|| String::from("unknown")),
			kernel_version: System::kernel_version().unwrap_or_default(),
			os_version: System::os_version().unwrap_or_default(),
			created_at: now,
			modified_at: now,
			accessed_at: now,
		};
		let conn = match self.db.lock() {
			Ok(conn) => conn,
			Err(err) => {
				log::error!("failed to lock database for node persistence: {err}");
				return;
			}
		};
		if let Err(err) = save_node(&conn, &node) {
			log::error!("failed to persist local node: {err}");
		}
	}

	fn normalize_file_location_node_ids(&self) {
		const NODE_ID_LEN: i64 = std::mem::size_of::<NodeID>() as i64;
		let conn = match self.db.lock() {
			Ok(conn) => conn,
			Err(err) => {
				log::error!("failed to lock database to normalize node ids: {err}");
				return;
			}
		};
		if let Err(err) = conn.execute(
			"UPDATE file_locations SET node_id = substr(node_id, 1, ?) WHERE length(node_id) != ?",
			params![NODE_ID_LEN, NODE_ID_LEN],
		) {
			log::error!("failed to normalize legacy node ids: {err}");
		}
	}

	fn persist_local_cpus(&self, cpus: &[CpuInfo]) {
		if cpus.is_empty() {
			return;
		}
		let node_id = match self.local_node_id() {
			Some(id) => id,
			None => return,
		};
		let conn = match self.db.lock() {
			Ok(conn) => conn,
			Err(err) => {
				log::error!("failed to lock database for CPU persistence: {err}");
				return;
			}
		};
		let now = Utc::now();
		let mut current_names = Vec::with_capacity(cpus.len());
		for info in cpus {
			let entry = DbCpu {
				node_id,
				name: info.name.clone(),
				usage: info.usage,
				frequency: info.frequency_hz as u32,
				created_at: now,
				modified_at: now,
			};
			if let Err(err) = save_cpu(&conn, &entry) {
				log::error!("failed to save CPU {}: {err}", entry.name);
			} else {
				current_names.push(entry.name.clone());
			}
		}
		if let Err(err) = remove_stale_cpus(&conn, &node_id, &current_names) {
			log::error!("failed to prune stale CPU entries: {err}");
		}
	}

	fn collect_interface_info(&self) -> Vec<InterfaceInfo> {
		let networks = Networks::new_with_refreshed_list();
		let interfaces: Vec<InterfaceInfo> = networks
			.iter()
			.map(|(name, data)| InterfaceInfo {
				name: name.clone(),
				mac: data.mac_address().to_string(),
				ips: data.ip_networks().iter().map(|ip| ip.to_string()).collect(),
				total_received: data.total_received(),
				total_transmitted: data.total_transmitted(),
				packets_received: data.total_packets_received(),
				packets_transmitted: data.total_packets_transmitted(),
				errors_on_received: data.total_errors_on_received(),
				errors_on_transmitted: data.total_errors_on_transmitted(),
				mtu: data.mtu(),
			})
			.collect();
		self.persist_local_interfaces(&interfaces);
		interfaces
	}

	fn persist_local_interfaces(&self, interfaces: &[InterfaceInfo]) {
		if interfaces.is_empty() {
			return;
		}
		let node_id = match self.local_node_id() {
			Some(id) => id,
			None => return,
		};
		let conn = match self.db.lock() {
			Ok(conn) => conn,
			Err(err) => {
				log::error!("failed to lock database for interface persistence: {err}");
				return;
			}
		};
		let now = Utc::now();
		let mut current_names = Vec::with_capacity(interfaces.len());
		for info in interfaces {
			let (ip, loopback, linklocal) = summarize_interface_ips(&info.ips);
			let entry = DbInterface {
				node_id,
				name: info.name.clone(),
				ip,
				mac: info.mac.clone(),
				loopback,
				linklocal,
				usage: (info.total_received + info.total_transmitted) as f32,
				total_received: info.total_received,
				created_at: now,
				modified_at: now,
			};
			if let Err(err) = save_interface(&conn, &entry) {
				log::error!("failed to save interface {}: {err}", entry.name);
			} else {
				current_names.push(entry.name.clone());
			}
		}
		if let Err(err) = remove_stale_interfaces(&conn, &node_id, &current_names) {
			log::error!("failed to prune stale interface entries: {err}");
		}
	}

	fn fetch_storage_files(&self) -> Result<Vec<StorageUsageFile>> {
		let conn = self
			.db
			.lock()
			.map_err(|err| anyhow!("db lock poisoned: {err}"))?;
		let mut stmt = conn
			.prepare("SELECT id, name FROM nodes")
			.map_err(|err| anyhow!("failed to prepare nodes query: {err}"))?;
		let rows = stmt
			.query_map([], |row| {
				let id: Vec<u8> = row.get(0)?;
				let name: String = row.get(1)?;
				Ok((id, name))
			})
			.map_err(|err| anyhow!("failed to query nodes: {err}"))?;
		let mut files = Vec::new();
		for row in rows {
			let (node_id, node_name) =
				row.map_err(|err| anyhow!("failed to read node row: {err}"))?;
			files.extend(Self::load_files_for_node(&conn, &node_id, &node_name)?);
		}
		Ok(files)
	}

	fn load_files_for_node(
		conn: &SqliteConnection,
		node_id: &[u8],
		node_name: &str,
	) -> Result<Vec<StorageUsageFile>> {
		let mut stmt = conn
			.prepare(
				"SELECT path, size, timestamp, modified_at \
				FROM file_locations WHERE node_id = ?1",
			)
			.map_err(|err| anyhow!("failed to prepare file_locations query: {err}"))?;
		let rows = stmt
			.query_map(params![node_id], |row| {
				let path: String = row.get(0)?;
				let size = row.get::<_, i64>(1)?.max(0) as u64;
				let timestamp: Option<DateTime<Utc>> = row.get(2)?;
				let modified: Option<DateTime<Utc>> = row.get(3)?;
				Ok(StorageUsageFile {
					node_id: node_id.to_vec(),
					node_name: node_name.to_string(),
					path,
					size,
					last_changed: modified.or(timestamp),
				})
			})
			.map_err(|err| anyhow!("failed to query file_locations: {err}"))?;
		let mut files = Vec::new();
		for row in rows {
			files.push(row.map_err(|err| anyhow!("failed to read file row: {err}"))?);
		}
		Ok(files)
	}

	fn fetch_file_entries(&self, offset: u64, limit: u64) -> Result<Vec<FileEntry>, String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		fetch_file_entries_paginated(&conn, offset, limit)
			.map_err(|err| format!("failed to fetch file entries: {err}"))
	}

	fn local_node_id(&self) -> Option<NodeID> {
		let state = match self.state.lock() {
			Ok(state) => state,
			Err(err) => {
				log::error!("failed to lock state for hardware persistence: {err}");
				return None;
			}
		};
		match peer_to_node_id(&state.me) {
			Some(id) => Some(id),
			None => {
				log::warn!("local peer id too short to derive node id; skipping persistence");
				None
			}
		}
	}

	fn collect_disk_info(&self) -> Vec<DiskInfo> {
		let disks = Disks::new_with_refreshed_list();
		disks
			.iter()
			.map(|disk| {
				let total_space = disk.total_space();
				let available_space = disk.available_space();
				let usage_percent = if total_space == 0 {
					0.0
				} else {
					let used = total_space.saturating_sub(available_space);
					((used as f64 / total_space as f64) * 100.0) as f32
				};
				let usage = disk.usage();
				DiskInfo {
					name: disk.name().to_string_lossy().to_string(),
					mount_path: disk.mount_point().to_string_lossy().to_string(),
					filesystem: disk.file_system().to_string_lossy().to_string(),
					total_space,
					available_space,
					usage_percent,
					total_read_bytes: usage.total_read_bytes,
					total_written_bytes: usage.total_written_bytes,
					read_only: disk.is_read_only(),
					removable: disk.is_removable(),
					kind: format!("{:?}", disk.kind()),
				}
			})
			.collect()
	}

	async fn collect_dir_entries(path: impl AsRef<Path>) -> Result<Vec<DirEntry>> {
		let path = path.as_ref();
		let mut entries = Vec::new();
		let mut reader = fs::read_dir(path).await?;
		while let Some(entry) = reader.next_entry().await? {
			let file_type = entry.file_type().await?;
			let metadata = match entry.metadata().await {
				Ok(m) => m,
				Err(err) => {
					log::warn!("metadata failed for {:?}: {err}", entry.path());
					continue;
				}
			};
			let extension = entry
				.path()
				.extension()
				.and_then(|s| s.to_str().map(|s| s.to_string()));
			let mime = if file_type.is_dir() {
				None
			} else {
				mime_guess::from_path(entry.path())
					.first_raw()
					.map(|value| value.to_string())
			};
			entries.push(DirEntry {
				name: entry.file_name().to_string_lossy().to_string(),
				is_dir: file_type.is_dir(),
				extension,
				mime,
				size: metadata.len(),
				created_at: metadata
					.created()
					.ok()
					.and_then(|t| DateTime::<Utc>::from(t).into()),
				modified_at: metadata
					.modified()
					.ok()
					.and_then(|t| DateTime::<Utc>::from(t).into()),
				accessed_at: metadata
					.accessed()
					.ok()
					.and_then(|t| DateTime::<Utc>::from(t).into()),
			});
		}
		entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
			(true, false) => std::cmp::Ordering::Less,
			(false, true) => std::cmp::Ordering::Greater,
			_ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
		});
		Ok(entries)
	}

	async fn handle_agent_event(&mut self, event: AgentEvent) {
		match event {
			AgentEvent::Ping(event) => {
				log::info!("Ping event: {:?}", event);
			}
			AgentEvent::PuppyNet(event) => match event {
				libp2p::request_response::Event::Message {
					peer,
					connection_id: _,
					message,
				} => match message {
					libp2p::request_response::Message::Request {
						request_id: _,
						request,
						channel,
					} => {
						if let Ok(res) = self.handle_puppy_peer_req(peer, request).await {
							let _ = self
								.swarm
								.behaviour_mut()
								.puppynet
								.send_response(channel, res);
						} else {
							let _ = self
								.swarm
								.behaviour_mut()
								.puppynet
								.send_response(channel, PeerRes::Error("Internal error".into()));
						}
					}
					libp2p::request_response::Message::Response {
						request_id,
						response,
					} => {
						if let Some(pending) = self.pending_requests.remove(&request_id) {
							pending.complete(response);
						}
					}
				},
				libp2p::request_response::Event::OutboundFailure {
					peer,
					connection_id: _,
					request_id,
					error,
				} => {
					log::warn!("outbound request to {} failed: {error}", peer);
					if let Some(pending) = self.pending_requests.remove(&request_id) {
						pending.fail(anyhow!("request failed: {error}"));
					}
				}
				libp2p::request_response::Event::InboundFailure {
					peer,
					connection_id: _,
					request_id: _,
					error,
				} => {
					log::warn!("inbound failure from {}: {error}", peer);
				}
				libp2p::request_response::Event::ResponseSent {
					peer,
					connection_id: _,
					request_id: _,
				} => {
					log::debug!("response sent to {}", peer);
				}
			},
			AgentEvent::Mdns(event) => match event {
				mdns::Event::Discovered(items) => {
					for (peer_id, multiaddr) in items {
						log::info!("mDNS discovered peer {} at {}", peer_id, multiaddr);
						if let Ok(mut state) = self.state.lock() {
							state.peer_discovered(peer_id, multiaddr.clone());
						}
						self.swarm.dial(multiaddr).unwrap();
					}
				}
				mdns::Event::Expired(items) => {
					for (peer_id, multiaddr) in items {
						log::info!("mDNS expired peer {} at {}", peer_id, multiaddr);
						if let Ok(mut state) = self.state.lock() {
							state.peer_expired(peer_id, multiaddr);
						}
					}
				}
			},
		}
	}

	async fn handle_swarm_event(&mut self, event: SwarmEvent<AgentEvent>) {
		match event {
			SwarmEvent::Behaviour(b) => self.handle_agent_event(b).await,
			SwarmEvent::ConnectionEstablished {
				peer_id,
				connection_id,
				endpoint: _,
				num_established: _,
				concurrent_dial_errors: _,
				established_in: _,
			} => {
				log::info!("Connected to peer {}", peer_id);
				if let Ok(mut state) = self.state.lock() {
					state.connections.push(Connection {
						peer_id,
						connection_id,
					});
				}
			}
			SwarmEvent::ConnectionClosed {
				peer_id,
				connection_id,
				endpoint: _,
				num_established: _,
				cause: _,
			} => {
				log::info!("Disconnected from peer {}", peer_id);
				if let Ok(mut state) = self.state.lock() {
					state
						.connections
						.retain(|c| c.connection_id != connection_id);
				}
			}
			SwarmEvent::IncomingConnection {
				connection_id: _,
				local_addr: _,
				send_back_addr: _,
			} => {}
			SwarmEvent::IncomingConnectionError {
				connection_id: _,
				local_addr: _,
				send_back_addr: _,
				error: _,
				peer_id: _,
			} => {}
			SwarmEvent::OutgoingConnectionError {
				connection_id: _,
				peer_id: _,
				error: _,
			} => {}
			SwarmEvent::Dialing {
				peer_id: _,
				connection_id: _,
			} => {}
			SwarmEvent::NewExternalAddrCandidate { address: _ } => {}
			SwarmEvent::ExternalAddrConfirmed { address: _ } => {}
			SwarmEvent::ExternalAddrExpired { address: _ } => {}
			SwarmEvent::NewExternalAddrOfPeer {
				peer_id: _,
				address: _,
			} => {}
			SwarmEvent::NewListenAddr {
				listener_id: _,
				address,
			} => {
				log::info!("listener address added: {:?}", address);
			}
			SwarmEvent::ExpiredListenAddr {
				listener_id: _,
				address: _,
			} => {}
			SwarmEvent::ListenerClosed {
				listener_id: _,
				addresses: _,
				reason: _,
			} => {}
			SwarmEvent::ListenerError {
				listener_id: _,
				error: _,
			} => {}
			_ => {}
		}
	}

	async fn handle_cmd(&mut self, cmd: Command) {
		match cmd {
			Command::Connect { peer_id: _, addr } => {
				if let Err(err) = self.swarm.dial(addr) {
					log::error!("dial failed: {err}");
				}
			}
			Command::ListDir { peer, path, tx } => {
				let is_self = {
					self.state
						.lock()
						.map(|state| state.me == peer)
						.unwrap_or(false)
				};
				if is_self {
					let result = Self::collect_dir_entries(Path::new(&path)).await;
					let _ = tx.send(result);
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer, PeerReq::ListDir { path: path.clone() });
				if let Some(prev) = self
					.pending_requests
					.insert(request_id, Pending::<Vec<DirEntry>>::new(tx))
				{
					prev.fail(anyhow!("pending ListDir request was replaced"));
				}
			}
			Command::ListCpus { tx, peer_id } => {
				if self.state.lock().unwrap().me == peer_id {
					let cpus = self.collect_cpu_info();
					let _ = tx.send(Ok(cpus));
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer_id, PeerReq::ListCpus);
				self.pending_requests
					.insert(request_id, Pending::<Vec<CpuInfo>>::new(tx));
			}
			Command::ListDisks { tx, peer_id } => {
				if self.state.lock().unwrap().me == peer_id {
					let disks = self.collect_disk_info();
					let _ = tx.send(Ok(disks));
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer_id, PeerReq::ListDisks);
				self.pending_requests
					.insert(request_id, Pending::<Vec<DiskInfo>>::new(tx));
			}
			Command::ListInterfaces { tx, peer_id } => {
				if self.state.lock().unwrap().me == peer_id {
					let interfaces = self.collect_interface_info();
					let _ = tx.send(Ok(interfaces));
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer_id, PeerReq::ListInterfaces);
				self.pending_requests
					.insert(request_id, Pending::<Vec<InterfaceInfo>>::new(tx));
			}
			Command::ListFileEntries {
				peer,
				offset,
				limit,
				tx,
			} => {
				if self.state.lock().unwrap().me == peer {
					let result = self
						.fetch_file_entries(offset, limit)
						.map_err(|err| anyhow!(err));
					let _ = tx.send(result);
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer, PeerReq::FileEntries { offset, limit });
				self.pending_requests
					.insert(request_id, Pending::<Vec<FileEntry>>::new(tx));
			}
			Command::ListPermissions { peer, tx } => {
				let local_permissions = match self.state.lock() {
					Ok(state) => {
						if state.me == peer {
							Some(state.permissions_for_peer(&peer))
						} else {
							None
						}
					}
					Err(err) => {
						let _ = tx.send(Err(anyhow!("state lock poisoned: {}", err)));
						return;
					}
				};
				if let Some(permissions) = local_permissions {
					let _ = tx.send(Ok(permissions));
					return;
				}
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer, PeerReq::ListPermissions);
				if let Some(prev) = self
					.pending_requests
					.insert(request_id, Pending::<Vec<Permission>>::new(tx))
				{
					prev.fail(anyhow!("pending ListPermissions request was replaced"));
				}
			}
			Command::ReadFile(req) => {
				if self.state.lock().unwrap().me == req.peer_id {
					let chunk = read_file(Path::new(&req.path), req.offset, req.length).await;
					let _ = req.tx.send(chunk);
					return;
				}
				let request_id = self.swarm.behaviour_mut().puppynet.send_request(
					&req.peer_id,
					PeerReq::ReadFile {
						path: req.path.clone(),
						offset: req.offset,
						length: req.length,
					},
				);
				self.pending_requests
					.insert(request_id, Pending::<FileChunk>::new(req.tx));
			}
			Command::Scan { path, tx } => {
				let node_id = match self.local_node_id() {
					Some(id) => id,
					None => {
						let _ = tx.send(ScanEvent::Finished(Err(String::from(
							"failed to determine node id",
						))));
						return;
					}
				};
				let db = Arc::clone(&self.db);
				tokio::task::spawn_blocking(move || {
					let result = db
						.lock()
						.map_err(|err| format!("db lock poisoned: {}", err))
						.and_then(|mut guard| {
							scan::scan_with_progress(&node_id, &path, &mut *guard, |progress| {
								let _ = tx.send(ScanEvent::Progress(progress.clone()));
							})
						});
					let final_event = match result {
						Ok(stats) => ScanEvent::Finished(Ok(stats)),
						Err(err) => ScanEvent::Finished(Err(err)),
					};
					let _ = tx.send(final_event);
				});
			}
			Command::RemoteScan {
				peer,
				path,
				scan_id,
			} => {
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer, PeerReq::StartScan { id: scan_id, path });
				self.pending_requests.insert(
					request_id,
					PendingRemoteScanStart::new(scan_id, Arc::clone(&self.remote_scans)),
				);
			}
			Command::ListStorageFiles { tx } => {
				let result = self.fetch_storage_files();
				let _ = tx.send(result);
			}
			Command::GetThumbnail {
				peer,
				path,
				max_width,
				max_height,
				tx,
			} => {
				let is_self = {
					self.state
						.lock()
						.map(|state| state.me == peer)
						.unwrap_or(false)
				};
				if is_self {
					let result = generate_thumbnail(Path::new(&path), max_width, max_height).await;
					let _ = tx.send(result);
					return;
				}
				let request_id = self.swarm.behaviour_mut().puppynet.send_request(
					&peer,
					PeerReq::GetThumbnail {
						path,
						max_width,
						max_height,
					},
				);
				self.pending_requests
					.insert(request_id, Pending::<Thumbnail>::new(tx));
			}
			Command::RemoteUpdate {
				peer,
				version,
				update_id,
			} => {
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&peer, PeerReq::UpdateSelf { id: update_id, version });
				self.pending_requests.insert(
					request_id,
					PendingRemoteUpdateStart::new(update_id, Arc::clone(&self.remote_updates)),
				);
			}
		}
	}

	pub async fn run(&mut self) {
		tokio::select! {
			event = self.swarm.select_next_some() => {
				self.handle_swarm_event(event).await;
			}
			cmd = self.rx.recv() => {
				if let Some(cmd) = cmd {
					self.handle_cmd(cmd).await;
				}
			}
			internal = self.internal_rx.recv() => {
				if let Some(cmd) = internal {
					self.handle_internal_cmd(cmd);
				}
			}
		}
	}

	fn handle_internal_cmd(&mut self, cmd: InternalCommand) {
		match cmd {
			InternalCommand::SendScanEvent {
				target,
				scan_id,
				event,
			} => {
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&target, PeerReq::ScanEvent { id: scan_id, event });
				self.pending_requests
					.insert(request_id, PendingScanEventAck::new());
			}
			InternalCommand::SendUpdateEvent {
				target,
				update_id,
				event,
			} => {
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request(&target, PeerReq::UpdateEvent { id: update_id, event });
				self.pending_requests
					.insert(request_id, PendingUpdateEventAck::new());
			}
		}
	}
}

fn peer_to_node_id(peer: &PeerId) -> Option<NodeID> {
	let mut node_id = [0u8; std::mem::size_of::<NodeID>()];
	let bytes = peer.to_bytes();
	let len = node_id.len();
	if bytes.len() < len {
		return None;
	}
	node_id.copy_from_slice(&bytes[..len]);
	Some(node_id)
}

fn summarize_interface_ips(ips: &[String]) -> (String, bool, bool) {
	let mut first_ip = String::new();
	let mut loopback = false;
	let mut linklocal = false;
	for entry in ips {
		if first_ip.is_empty() {
			first_ip = entry.clone();
		}
		if let Some(addr) = parse_ip_addr(entry) {
			if addr.is_loopback() {
				loopback = true;
			}
			match addr {
				IpAddr::V4(v4) => {
					if v4.is_link_local() {
						linklocal = true;
					}
				}
				IpAddr::V6(v6) => {
					if v6.is_unicast_link_local() {
						linklocal = true;
					}
				}
			}
		}
	}
	if first_ip.is_empty() {
		first_ip = String::from("-");
	}
	(first_ip, loopback, linklocal)
}

fn parse_ip_addr(value: &str) -> Option<IpAddr> {
	let mut parts = value.split('/');
	let addr = parts.next()?.trim();
	addr.parse().ok()
}

#[derive(Debug, Clone)]
pub struct ScanResultRow {
	pub hash: Vec<u8>,
	pub size: u64,
	pub mime_type: Option<String>,
	pub first_datetime: Option<String>,
	pub latest_datetime: Option<String>,
}

pub struct PuppyNet {
	shutdown_tx: Option<oneshot::Sender<()>>,
	handle: JoinHandle<()>,
	state: Arc<Mutex<State>>,
	cmd_tx: UnboundedSender<Command>,
	db: Arc<Mutex<SqliteConnection>>,
	remote_scans: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
	remote_scan_counter: AtomicU64,
	remote_updates: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
	remote_update_counter: AtomicU64,
}

impl PuppyNet {
	pub fn new() -> Self {
		let state = Arc::new(Mutex::new(State::default()));
		let db = Arc::new(Mutex::new(open_db()));
		{
			let mut conn = db.lock().unwrap();
			if let Err(err) = run_migrations(&mut conn) {
				log::error!("failed to run database migrations: {err}");
			}
		}
		// channel to request shutdown
		let (shutdown_tx, shutdown_rx) = oneshot::channel();
		let state_clone = state.clone();
		let remote_scans = Arc::new(Mutex::new(HashMap::new()));
		let remote_updates = Arc::new(Mutex::new(HashMap::new()));
		let (mut app, cmd_tx) = App::new(state_clone, db.clone(), remote_scans.clone(), remote_updates.clone());
		let mut shutdown_rx = shutdown_rx;
		let handle: JoinHandle<()> = tokio::spawn(async move {
			loop {
				tokio::select! {
					_ = &mut shutdown_rx => {
						log::info!("PuppyNet shutting down");
						break;
					}
					_ = app.run() => {}
				}
			}
		});

		PuppyNet {
			shutdown_tx: Some(shutdown_tx),
			handle,
			state,
			cmd_tx,
			db,
			remote_scans,
			remote_scan_counter: AtomicU64::new(1),
			remote_updates,
			remote_update_counter: AtomicU64::new(1),
		}
	}

	fn register_shared_folder(&self, path: PathBuf, flags: u8) -> anyhow::Result<()> {
		let mut state = self
			.state
			.lock()
			.map_err(|_| anyhow!("state lock poisoned"))?;
		state.add_shared_folder(FolderRule::new(path, flags));
		Ok(())
	}

	pub fn share_read_only_folder(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
		let canonical = std::fs::canonicalize(path.as_ref())
			.map_err(|err| anyhow!("failed to canonicalize path: {err}"))?;
		self.register_shared_folder(canonical, FLAG_READ | FLAG_SEARCH)
	}

	pub fn share_read_write_folder(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
		let canonical = std::fs::canonicalize(path.as_ref())
			.map_err(|err| anyhow!("failed to canonicalize path: {err}"))?;
		self.register_shared_folder(canonical, FLAG_READ | FLAG_WRITE | FLAG_SEARCH)
	}

	pub fn set_peer_permissions(
		&self,
		peer: PeerId,
		permissions: Vec<Permission>,
	) -> anyhow::Result<()> {
		let mut state = self
			.state
			.lock()
			.map_err(|_| anyhow!("state lock poisoned"))?;
		state.set_peer_permissions(peer, permissions);
		state.save_changes()
	}

	pub fn state(&self) -> Arc<Mutex<State>> {
		self.state.clone()
	}

	pub async fn list_dir(&self, peer: PeerId, path: impl Into<String>) -> Result<Vec<DirEntry>> {
		let path = path.into();
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListDir { peer, path, tx })
			.map_err(|e| anyhow!("failed to send ListDir command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListDir response channel closed: {e}"))?
	}

	pub fn list_dir_blocking(
		&self,
		peer: PeerId,
		path: impl Into<String>,
	) -> Result<Vec<DirEntry>> {
		block_on(self.list_dir(peer, path))
	}

	pub async fn list_cpus(&self, peer_id: PeerId) -> Result<Vec<CpuInfo>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListCpus { tx, peer_id })
			.map_err(|e| anyhow!("failed to send ListCpus command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListCpus response channel closed: {e}"))?
	}

	pub fn list_cpus_blocking(&self, peer_id: PeerId) -> Result<Vec<CpuInfo>> {
		block_on(self.list_cpus(peer_id))
	}

	pub async fn list_disks(&self, peer_id: PeerId) -> Result<Vec<DiskInfo>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListDisks { tx, peer_id })
			.map_err(|e| anyhow!("failed to send ListDisks command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListDisks response channel closed: {e}"))?
	}

	pub fn list_disks_blocking(&self, peer_id: PeerId) -> Result<Vec<DiskInfo>> {
		block_on(self.list_disks(peer_id))
	}

	pub async fn list_interfaces(&self, peer_id: PeerId) -> Result<Vec<InterfaceInfo>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListInterfaces { tx, peer_id })
			.map_err(|e| anyhow!("failed to send ListInterfaces command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListInterfaces response channel closed: {e}"))?
	}

	pub fn list_interfaces_blocking(&self, peer_id: PeerId) -> Result<Vec<InterfaceInfo>> {
		block_on(self.list_interfaces(peer_id))
	}

	pub fn scan_remote_peer(
		&self,
		peer: PeerId,
		path: impl Into<String>,
	) -> Result<Arc<Mutex<mpsc::Receiver<ScanEvent>>>, String> {
		let path = path.into();
		{
			let state = self
				.state
				.lock()
				.map_err(|_| String::from("state lock poisoned"))?;
			if state.me == peer {
				return self.scan_folder(path);
			}
		}
		let (tx, rx) = mpsc::channel();
		let scan_id = self.remote_scan_counter.fetch_add(1, Ordering::SeqCst);
		self.remote_scans
			.lock()
			.unwrap()
			.insert(scan_id, tx.clone());
		self.cmd_tx
			.send(Command::RemoteScan {
				peer,
				path,
				scan_id,
			})
			.map_err(|e| {
				self.remote_scans.lock().unwrap().remove(&scan_id);
				format!("failed to send RemoteScan command: {e}")
			})?;
		Ok(Arc::new(Mutex::new(rx)))
	}

	pub fn scan_remote_peer_blocking(
		&self,
		peer: PeerId,
		path: impl Into<String>,
	) -> Result<Arc<Mutex<mpsc::Receiver<ScanEvent>>>, String> {
		self.scan_remote_peer(peer, path)
	}

	pub async fn list_file_entries(
		&self,
		peer: PeerId,
		offset: u64,
		limit: u64,
	) -> Result<Vec<FileEntry>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListFileEntries {
				peer,
				offset,
				limit,
				tx,
			})
			.map_err(|e| anyhow!("failed to send ListFileEntries command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListFileEntries response channel closed: {e}"))?
	}

	pub fn list_file_entries_blocking(
		&self,
		peer: PeerId,
		offset: u64,
		limit: u64,
	) -> Result<Vec<FileEntry>> {
		block_on(self.list_file_entries(peer, offset, limit))
	}

	pub async fn list_storage_files(&self) -> Result<Vec<StorageUsageFile>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListStorageFiles { tx })
			.map_err(|e| anyhow!("failed to send ListStorageFiles command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListStorageFiles response channel closed: {e}"))?
	}

	pub fn list_storage_files_blocking(&self) -> Result<Vec<StorageUsageFile>> {
		block_on(self.list_storage_files())
	}

	pub fn list_granted_permissions(&self, peer: PeerId) -> Result<Vec<Permission>> {
		let state = self
			.state
			.lock()
			.map_err(|_| anyhow!("state lock poisoned"))?;
		Ok(state.permissions_granted_to_peer(&peer))
	}

	pub async fn list_permissions(&self, peer: PeerId) -> Result<Vec<Permission>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListPermissions { peer, tx })
			.map_err(|e| anyhow!("failed to send ListPermissions command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListPermissions response channel closed: {e}"))?
	}

	pub fn list_permissions_blocking(&self, peer: PeerId) -> Result<Vec<Permission>> {
		block_on(self.list_permissions(peer))
	}

	pub fn scan_folder(
		&self,
		path: impl Into<String>,
	) -> Result<Arc<Mutex<mpsc::Receiver<ScanEvent>>>, String> {
		let path = path.into();
		let (tx, rx) = mpsc::channel();
		self.cmd_tx
			.send(Command::Scan { path, tx })
			.map_err(|e| format!("failed to send Scan command: {e}"))?;
		Ok(Arc::new(Mutex::new(rx)))
	}

	pub fn fetch_scan_results_page(
		&self,
		page: usize,
		page_size: usize,
	) -> Result<(Vec<ScanResultRow>, usize), String> {
		let offset = page.saturating_mul(page_size);
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {}", err))?;
		let total_entries: i64 = conn
			.query_row("SELECT COUNT(*) FROM file_entries", [], |row| row.get(0))
			.map_err(|err| format!("failed to count scan results: {err}"))?;
		let mut stmt = conn
			.prepare(
				"SELECT hash, size, mime_type, first_datetime, latest_datetime \
				FROM file_entries \
				ORDER BY latest_datetime DESC \
				LIMIT ? OFFSET ?",
			)
			.map_err(|err| format!("failed to prepare scan results query: {err}"))?;
		let rows = stmt
			.query_map(params![page_size as i64, offset as i64], |row| {
				let hash: Vec<u8> = row.get(0)?;
				let size = row.get::<_, i64>(1)?.max(0) as u64;
				let mime_type = row.get(2)?;
				let first = row.get(3)?;
				let latest = row.get(4)?;
				Ok(ScanResultRow {
					hash,
					size,
					mime_type,
					first_datetime: first,
					latest_datetime: latest,
				})
			})
			.map_err(|err| format!("failed to query scan results: {err}"))?;
		let mut entries = Vec::new();
		for entry in rows {
			entries.push(entry.map_err(|err| format!("error reading scan row: {err}"))?);
		}
		Ok((entries, total_entries.max(0) as usize))
	}

	pub async fn read_file(
		&self,
		peer: libp2p::PeerId,
		path: impl Into<String>,
		offset: u64,
		length: Option<u64>,
	) -> Result<FileChunk> {
		let path = path.into();
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ReadFile(ReadFileCmd {
				peer_id: peer,
				path,
				offset,
				length,
				tx,
			}))
			.map_err(|e| anyhow!("failed to send ReadFile command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ReadFile response channel closed: {e}"))?
	}

	pub fn read_file_blocking(
		&self,
		peer: libp2p::PeerId,
		path: impl Into<String>,
		offset: u64,
		length: Option<u64>,
	) -> Result<FileChunk> {
		block_on(self.read_file(peer, path, offset, length))
	}

	pub async fn get_thumbnail(
		&self,
		peer: libp2p::PeerId,
		path: impl Into<String>,
		max_width: u32,
		max_height: u32,
	) -> Result<Thumbnail> {
		let path = path.into();
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::GetThumbnail {
				peer,
				path,
				max_width,
				max_height,
				tx,
			})
			.map_err(|e| anyhow!("failed to send GetThumbnail command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("GetThumbnail response channel closed: {e}"))?
	}

	pub fn get_thumbnail_blocking(
		&self,
		peer: libp2p::PeerId,
		path: impl Into<String>,
		max_width: u32,
		max_height: u32,
	) -> Result<Thumbnail> {
		block_on(self.get_thumbnail(peer, path, max_width, max_height))
	}

	/// Request a remote peer to update itself.
	/// Returns a receiver that will receive UpdateProgress events as the update proceeds.
	/// If the target peer is the local peer, performs a local self-update instead.
	pub fn update_remote_peer(
		&self,
		peer: PeerId,
		version: Option<String>,
	) -> Result<Arc<Mutex<mpsc::Receiver<UpdateProgress>>>, String> {
		let (tx, rx) = mpsc::channel();
		let update_id = self.remote_update_counter.fetch_add(1, Ordering::SeqCst);

		// Check if the target peer is self - if so, perform a local update
		let is_self = {
			let state = self.state.lock().unwrap();
			peer == state.me
		};

		if is_self {
			// Perform local self-update
			let tx_clone = tx.clone();
			let version_clone = version.clone();
			// Use current_version = 0 for core library (actual version comes from CLI build)
			let current_version = 0u32;

			std::thread::spawn(move || {
				let rt = tokio::runtime::Runtime::new().unwrap();
				rt.block_on(async move {
					let result = updater::update_with_progress(
						version_clone.as_deref(),
						current_version,
						move |progress| {
							let _ = tx_clone.send(progress);
						},
					)
					.await;

					if let Err(e) = result {
						let _ = tx.send(UpdateProgress::Failed {
							error: e.to_string(),
						});
					}
				});
			});
		} else {
			// Remote update - send command to dial peer
			self.remote_updates
				.lock()
				.unwrap()
				.insert(update_id, tx.clone());
			self.cmd_tx
				.send(Command::RemoteUpdate {
					peer,
					version,
					update_id,
				})
				.map_err(|e| {
					self.remote_updates.lock().unwrap().remove(&update_id);
					format!("failed to send RemoteUpdate command: {e}")
				})?;
		}

		Ok(Arc::new(Mutex::new(rx)))
	}

	/// Wait for the peer until Ctrl+C (SIGINT) then perform a graceful shutdown.
	pub async fn wait(mut self) {
		// Wait for Ctrl+C
		if let Err(e) = tokio::signal::ctrl_c().await {
			log::error!("failed to listen for ctrl_c: {e}");
		}
		log::info!("interrupt received, shutting down");
		if let Some(tx) = self.shutdown_tx.take() {
			let _ = tx.send(());
		}
		// Await the background task
		if let Err(e) = self.handle.await {
			log::error!("task join error: {e}");
		}
	}
}
