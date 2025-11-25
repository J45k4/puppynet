use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use chrono::DateTime;
use chrono::Utc;
use libp2p::PeerId;
use rusqlite::Connection;
use rusqlite::ToSql;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::scan::FileHash;
use crate::scan::FileLocation;
use crate::state::{FolderRule, Permission, Rule, User};

pub type NodeID = [u8; 16];

struct Migration {
	id: u32,
	name: &'static str,
	sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
	Migration {
		id: 20250208,
		name: "init_database",
		sql: r"
			create table file_entries (
				hash blob not null unique primary key,
				size integer not null,
				mime_type text null,
				first_datetime timestamp null,
				latest_datetime timestamp null
			);
			create table file_locations (
				node_id BLOB not null,
				path text not null,
				hash blob null,
				size integer not null,
				timestamp timestamp not null,
				created_at timestamp null,
				modified_at timestamp null,
				accessed_at timestamp null,
				primary key (node_id, path)
			);
			create table nodes (
				id BLOB primary key,
				name text not null,
				you bool not null,
				total_memory integer not null,
				system_name text not null,
				kernel_version text not null,
				os_version text not null,
				created_at timestamp not null,
				modified_at timestamp not null,
				accessed_at timestamp not null
			);
			create table connections (
				node_id BLOB not null,
				url text not null,
				type integer not null,
				created_at timestamp not null,
				last_used_at timestamp not null
			);
			create table cpus (
				node_id BLOB not null,
				name text not null,
				usage real not null,
				frequency integer not null,
				created_at timestamp not null,
				modified_at timestamp not null,
				primary key(node_id,name)
			);
			create table disks (
				node_id BLOB not null,
				name text not null,
				usage real not null,
				total_size integer not null,
				total_read_bytes integer not null,
				total_written_bytes integer not null,
				mount_path text not null,
				filesystem text not null,
				readonly bool not null,
				removable bool not null,
				kind text not null,
				created_at timestamp not null,
				modified_at timestamp not null,
				primary key(node_id,name)
			);
			create table interfaces (
				node_id BLOB not null,
				name text not null,
				ip text not null,
				mac text not null,
				loopback bool not null,
				linklocal bool not null,
				usage real not null,
				total_received integer,
				created_at timestamp not null,
				modified_at timestamp not null,
				primary key(node_id,name)
			);
			create table temperatures (
				node_id BLOB not null,
				label text not null,
				temperature real null,
				max real null,
				critical real null,
				created_at timestamp not null,
				modified_at timestamp not null,
				primary key(node_id, label)
			);
			CREATE INDEX IF NOT EXISTS idx_file_locations_path ON file_locations(path);
			CREATE INDEX IF NOT EXISTS idx_file_locations_hash ON file_locations(hash);
		",
	},
	Migration {
		id: 20250219,
		name: "peer_permissions",
		sql: r"
			create table peer_permissions (
				id integer primary key autoincrement,
				src_peer blob not null,
				target_peer blob not null,
				rule_type integer not null,
				path text null,
				flags integer null,
				expires_at integer null
			);
			create index if not exists idx_peer_permissions_src_target on peer_permissions(src_peer, target_peer);
		",
	},
	Migration {
		id: 20250226,
		name: "users",
		sql: r"
			create table users (
				username text primary key,
				password text not null
			);
		",
	},
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
	pub id: NodeID,
	pub name: String,
	pub you: bool,
	pub total_memory: u64,
	pub system_name: String,
	pub kernel_version: String,
	pub os_version: String,
	pub created_at: DateTime<Utc>,
	pub modified_at: DateTime<Utc>,
	pub accessed_at: DateTime<Utc>,
}

pub struct ConnectionInfo {
	pub node_id: NodeID,
	pub url: String,
	pub created_at: DateTime<Utc>,
	pub last_used_at: DateTime<Utc>,
}

pub struct Cpu {
	pub node_id: NodeID,
	pub name: String,
	pub usage: f32,
	pub frequency: u32,
	pub created_at: DateTime<Utc>,
	pub modified_at: DateTime<Utc>,
}

pub struct Disk {
	pub node_id: NodeID,
	pub name: String,
	pub usage: f32,
	pub total_size: u64,
	pub total_read_bytes: u64,
	pub total_written_bytes: u64,
	pub mount_path: String,
	pub filesystem: String,
	pub readonly: bool,
	pub removable: bool,
	pub kind: String,
	pub created_at: DateTime<Utc>,
	pub modified_at: DateTime<Utc>,
}

pub struct Interface {
	pub node_id: NodeID,
	pub name: String,
	pub ip: String,
	pub mac: String,
	pub loopback: bool,
	pub linklocal: bool,
	pub usage: f32,
	pub total_received: u64,
	pub created_at: DateTime<Utc>,
	pub modified_at: DateTime<Utc>,
}

pub struct Temperature {
	pub node_id: NodeID,
	pub label: String,
	pub temperature: Option<f32>,
	pub max: Option<f32>,
	pub critical: Option<f32>,
	pub created_at: DateTime<Utc>,
	pub modified_at: DateTime<Utc>,
}

pub fn save_temperature(conn: &Connection, temp: &Temperature) -> anyhow::Result<()> {
	conn.execute(
        "INSERT INTO temperatures (node_id, label, temperature, max, critical, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(node_id, label) DO UPDATE SET
             temperature  = excluded.temperature,
             max          = excluded.max,
             critical     = excluded.critical,
             modified_at  = excluded.modified_at",
        params![
            &temp.node_id[..],
            &temp.label,
            temp.temperature,
            temp.max,
            temp.critical,
            &temp.created_at,
            &temp.modified_at
        ],
    )?;
	Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileEntry {
	pub hash: FileHash,
	pub size: i64,
	pub mime_type: Option<String>,
	pub first_datetime: String,
	pub latest_datetime: String,
}

#[derive(Debug, Clone)]
pub struct StorageUsageFile {
	pub node_id: Vec<u8>,
	pub node_name: String,
	pub path: String,
	pub size: u64,
	pub last_changed: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Serialize)]
pub struct ListArgs {
	search_word: Option<String>,
}

pub struct DB {
	conn: Mutex<Connection>,
}

pub fn get_your_node(conn: &Connection) -> anyhow::Result<Option<[u8; 16]>> {
	let mut stmt = conn.prepare("SELECT id FROM nodes WHERE you = 1")?;
	let mut rows = stmt.query_map((), |row| row.get::<_, Vec<u8>>(0))?;

	if let Some(res) = rows.next() {
		Ok(Some(res?.try_into().expect("hash must be 16 bytes")))
	} else {
		Ok(None)
	}
}

/// Saves a fully‑populated `Node` row.
pub fn save_node(conn: &Connection, node: &Node) -> anyhow::Result<()> {
	conn.execute(
        "INSERT INTO nodes (id, name, you, total_memory, system_name, kernel_version, os_version, created_at, modified_at, accessed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             name           = excluded.name,
             you            = excluded.you,
             total_memory   = excluded.total_memory,
             system_name    = excluded.system_name,
             kernel_version = excluded.kernel_version,
             os_version     = excluded.os_version,
             modified_at    = excluded.modified_at,
             accessed_at    = excluded.accessed_at",
        params![
            &node.id[..],
            &node.name,
            &node.you,
            node.total_memory as i64,
            &node.system_name,
            &node.kernel_version,
            &node.os_version,
            &node.created_at,
            &node.modified_at,
            &node.accessed_at
        ],
    )?;
	Ok(())
}

/// Fetch all nodes.
pub fn fetch_nodes(conn: &Connection) -> anyhow::Result<Vec<Node>> {
	let mut stmt = conn.prepare(
        "SELECT id, name, you, total_memory, system_name, kernel_version, os_version, created_at, modified_at, accessed_at
         FROM nodes",
    )?;
	let rows = stmt.query_map([], |row| {
		let id_vec: Vec<u8> = row.get(0)?;
		let id: NodeID = id_vec.as_slice().try_into().expect("id must be 16 bytes");
		Ok(Node {
			id,
			name: row.get(1)?,
			you: row.get(2)?,
			total_memory: row.get::<_, i64>(3)? as u64,
			system_name: row.get(4)?,
			kernel_version: row.get(5)?,
			os_version: row.get(6)?,
			created_at: row.get(7)?,
			modified_at: row.get(8)?,
			accessed_at: row.get(9)?,
		})
	})?;

	let mut nodes = Vec::new();
	for n in rows {
		nodes.push(n?);
	}
	Ok(nodes)
}

/// Save a CPU row (upsert on `(node_id,name)`).
pub fn save_cpu(conn: &Connection, cpu: &Cpu) -> anyhow::Result<()> {
	conn.execute(
		"INSERT INTO cpus (node_id, name, usage, frequency, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(node_id, name) DO UPDATE SET
             usage       = excluded.usage,
             frequency   = excluded.frequency,
             modified_at = excluded.modified_at",
		params![
			&cpu.node_id[..],
			&cpu.name,
			cpu.usage,
			cpu.frequency as i64,
			&cpu.created_at,
			&cpu.modified_at
		],
	)?;
	Ok(())
}

/// Fetch all CPUs for the given `node_id`.
pub fn fetch_cpus(conn: &Connection, node_id: &[u8]) -> anyhow::Result<Vec<Cpu>> {
	let mut stmt = conn.prepare(
		"SELECT node_id, name, usage, frequency, created_at, modified_at
         FROM cpus WHERE node_id = ?1",
	)?;
	let rows = stmt.query_map([node_id], |row| {
		let id_vec: Vec<u8> = row.get(0)?;
		let id: NodeID = id_vec.as_slice().try_into().expect("id must be 16 bytes");
		Ok(Cpu {
			node_id: id,
			name: row.get(1)?,
			usage: row.get(2)?,
			frequency: row.get::<_, i64>(3)? as u32,
			created_at: row.get(4)?,
			modified_at: row.get(5)?,
		})
	})?;

	let mut cpus = Vec::new();
	for c in rows {
		cpus.push(c?);
	}
	Ok(cpus)
}

/// Remove CPU rows for `node_id` whose names are not in `current_names`.
pub fn remove_stale_cpus(
	conn: &Connection,
	node_id: &[u8],
	current_names: &[String],
) -> anyhow::Result<()> {
	if current_names.is_empty() {
		conn.execute("DELETE FROM cpus WHERE node_id = ?1", params![node_id])?;
	} else {
		let placeholders = std::iter::repeat("?")
			.take(current_names.len())
			.collect::<Vec<_>>()
			.join(", ");
		let sql = format!(
			"DELETE FROM cpus WHERE node_id = ?1 AND name NOT IN ({})",
			placeholders
		);
		let mut stmt = conn.prepare(&sql)?;
		let mut params: Vec<&dyn ToSql> = Vec::with_capacity(1 + current_names.len());
		params.push(&node_id);
		for name in current_names {
			params.push(name);
		}
		stmt.execute(&params[..])?;
	}
	Ok(())
}

/// Remove interface rows for `node_id` whose names are not in `current_names`.
pub fn remove_stale_interfaces(
	conn: &Connection,
	node_id: &[u8],
	current_names: &[String],
) -> anyhow::Result<()> {
	if current_names.is_empty() {
		conn.execute(
			"DELETE FROM interfaces WHERE node_id = ?1",
			params![node_id],
		)?;
	} else {
		let placeholders = std::iter::repeat("?")
			.take(current_names.len())
			.collect::<Vec<_>>()
			.join(", ");
		let sql = format!(
			"DELETE FROM interfaces WHERE node_id = ?1 AND name NOT IN ({})",
			placeholders
		);
		let mut stmt = conn.prepare(&sql)?;
		let mut params: Vec<&dyn ToSql> = Vec::with_capacity(1 + current_names.len());
		params.push(&node_id);
		for name in current_names {
			params.push(name);
		}
		stmt.execute(&params[..])?;
	}
	Ok(())
}

/// Save a disk row (upsert on `(node_id,name)`).
pub fn save_disk(conn: &Connection, disk: &Disk) -> anyhow::Result<()> {
	conn.execute(
        "INSERT INTO disks (node_id, name, usage, total_size, total_read_bytes, total_written_bytes, mount_path, filesystem, readonly, removable, kind, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(node_id, name) DO UPDATE SET
             usage               = excluded.usage,
             total_size          = excluded.total_size,
             total_read_bytes    = excluded.total_read_bytes,
             total_written_bytes = excluded.total_written_bytes,
             mount_path          = excluded.mount_path,
             filesystem          = excluded.filesystem,
             readonly            = excluded.readonly,
             removable           = excluded.removable,
             kind                = excluded.kind,
             modified_at         = excluded.modified_at",
        params![
            &disk.node_id[..],
            &disk.name,
            disk.usage,
            disk.total_size as i64,
            disk.total_read_bytes as i64,
            disk.total_written_bytes as i64,
            &disk.mount_path,
            &disk.filesystem,
            disk.readonly,
            disk.removable,
            &disk.kind,
            &disk.created_at,
            &disk.modified_at
        ],
    )?;
	Ok(())
}

/// Fetch all disks for the given `node_id`.
pub fn fetch_disks(conn: &Connection, node_id: &[u8]) -> anyhow::Result<Vec<Disk>> {
	let mut stmt = conn.prepare(
        "SELECT node_id, name, usage, total_size, total_read_bytes, total_written_bytes, mount_path, filesystem, readonly, removable, kind, created_at, modified_at
         FROM disks WHERE node_id = ?1",
    )?;
	let rows = stmt.query_map([node_id], |row| {
		let id_vec: Vec<u8> = row.get(0)?;
		let id: NodeID = id_vec.as_slice().try_into().expect("id must be 16 bytes");
		Ok(Disk {
			node_id: id,
			name: row.get(1)?,
			usage: row.get(2)?,
			total_size: row.get::<_, i64>(3)? as u64,
			total_read_bytes: row.get::<_, i64>(4)? as u64,
			total_written_bytes: row.get::<_, i64>(5)? as u64,
			mount_path: row.get(6)?,
			filesystem: row.get(7)?,
			readonly: row.get(8)?,
			removable: row.get(9)?,
			kind: row.get(10)?,
			created_at: row.get(11)?,
			modified_at: row.get(12)?,
		})
	})?;

	let mut disks = Vec::new();
	for d in rows {
		disks.push(d?);
	}
	Ok(disks)
}

/// Save a network interface row (upsert on `(node_id,name)`).
pub fn save_interface(conn: &Connection, interface: &Interface) -> anyhow::Result<()> {
	conn.execute(
        "INSERT INTO interfaces (node_id, name, ip, mac, loopback, linklocal, usage, total_received, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(node_id, name) DO UPDATE SET
             ip             = excluded.ip,
             mac            = excluded.mac,
             loopback       = excluded.loopback,
             linklocal      = excluded.linklocal,
             usage          = excluded.usage,
             total_received = excluded.total_received,
             modified_at    = excluded.modified_at",
        params![
            &interface.node_id[..],
            &interface.name,
            &interface.ip,
            &interface.mac,
            interface.loopback,
            interface.linklocal,
            interface.usage,
            interface.total_received as i64,
            &interface.created_at,
            &interface.modified_at
        ],
    )?;
	Ok(())
}

/// Fetch all interfaces for the given `node_id`.
pub fn fetch_interfaces(conn: &Connection, node_id: &[u8]) -> anyhow::Result<Vec<Interface>> {
	let mut stmt = conn.prepare(
        "SELECT node_id, name, ip, mac, loopback, linklocal, usage, total_received, created_at, modified_at
         FROM interfaces WHERE node_id = ?1",
    )?;
	let rows = stmt.query_map([node_id], |row| {
		let id_vec: Vec<u8> = row.get(0)?;
		let id: NodeID = id_vec.as_slice().try_into().expect("id must be 16 bytes");
		Ok(Interface {
			node_id: id,
			name: row.get(1)?,
			ip: row.get(2)?,
			mac: row.get(3)?,
			loopback: row.get(4)?,
			linklocal: row.get(5)?,
			usage: row.get(6)?,
			total_received: row.get::<_, i64>(7)? as u64,
			created_at: row.get(8)?,
			modified_at: row.get(9)?,
		})
	})?;

	let mut interfaces = Vec::new();
	for i in rows {
		interfaces.push(i?);
	}
	Ok(interfaces)
}

pub fn list_files(conn: &Connection, args: ListArgs) -> anyhow::Result<Vec<FileEntry>> {
	// Build SQL and params based on whether we have a search term
	let mut sql = String::from(
		"SELECT hash, size, mime_type, first_datetime, latest_datetime \
		 FROM file_entries",
	);
	let mut params: Vec<&dyn ToSql> = Vec::new();

	// if let Some(ref term) = args.search_word.filter(|s| !s.is_empty()) {
	// 	sql.push_str(" WHERE mime_type LIKE ?1");
	// 	let pattern = format!("%{}%", term);
	// 	params.push(&pattern);
	// }

	let mut stmt = conn.prepare(&sql)?;
	let rows = stmt.query_map(&params[..], |row| {
		Ok(FileEntry {
			hash: row.get(0)?,
			size: row.get(1)?,
			mime_type: row.get(2)?,
			first_datetime: row.get(3)?,
			latest_datetime: row.get(4)?,
		})
	})?;

	let mut files = Vec::new();
	for file in rows {
		files.push(file?);
	}
	Ok(files)
}

pub async fn get_mime_types(conn: &Connection) -> anyhow::Result<Vec<String>> {
	let mut stmt =
		conn.prepare("SELECT DISTINCT mime_type FROM file_entries WHERE mime_type IS NOT NULL")?;
	let rows = stmt.query_map((), |row| row.get::<_, String>(0)).unwrap();

	let mut mime_types = Vec::new();
	for mime_type in rows {
		mime_types.push(mime_type?);
	}

	Ok(mime_types)
}

pub fn get_file_entry(conn: &Connection, hash: &[u8]) -> anyhow::Result<Option<FileEntry>> {
	match conn.query_row(
		"SELECT hash, size, mime_type, first_datetime, latest_datetime FROM file_entries WHERE hash = ?1",
		[hash],
		|row| {
			Ok(FileEntry {
				hash: row.get(0)?,
				size: row.get(1)?,
				mime_type: row.get(2)?,
				first_datetime: row.get(3)?,
				latest_datetime: row.get(4)?,
			})
		},
	) {
		Ok(entry) => Ok(Some(entry)),
		Err(e) => Err(e.into()),
	}
}

pub fn fetch_file_entries_paginated(
	conn: &Connection,
	offset: u64,
	limit: u64,
) -> anyhow::Result<Vec<FileEntry>> {
	let mut stmt = conn.prepare(
		"SELECT hash, size, mime_type, first_datetime, latest_datetime \
		FROM file_entries \
		ORDER BY latest_datetime DESC \
		LIMIT ?1 OFFSET ?2",
	)?;
	let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
		Ok(FileEntry {
			hash: row.get(0)?,
			size: row.get(1)?,
			mime_type: row.get(2)?,
			first_datetime: row.get(3)?,
			latest_datetime: row.get(4)?,
		})
	})?;

	let mut entries = Vec::new();
	for entry in rows {
		entries.push(entry?);
	}
	Ok(entries)
}

pub fn get_file_location(
	conn: &Connection,
	node_id: &[u8],
	hash: &[u8],
) -> anyhow::Result<Option<FileLocation>> {
	let mut stmt = conn.prepare(
		"SELECT path, hash, size, timestamp, created_at, modified_at, accessed_at \
		 FROM file_locations \
		 WHERE node_id = ? AND hash = ?",
	)?;
	let mut rows = stmt.query_map(&[node_id, hash], |row| {
		// get an optional Vec<u8> for the hash
		let hash_opt: Option<Vec<u8>> = row.get(1)?;
		// convert Vec<u8> into [u8; 32] if present
		let hash = hash_opt
			.as_ref()
			.map(|v| v.as_slice().try_into().expect("hash must be 32 bytes"));
		Ok(FileLocation {
			path: PathBuf::from(row.get::<_, String>(0)?),
			hash,
			size: row.get::<_, i64>(2)? as u64,
			// file_locations does not store mime_type, set to None
			mime_type: None,
			timestamp: row.get(3)?,
			created_at: row.get(4)?,
			modified_at: row.get(5)?,
			accessed_at: row.get(6)?,
		})
	})?;

	// return the first matching row if any
	if let Some(res) = rows.next() {
		Ok(Some(res?))
	} else {
		Ok(None)
	}
}

/// Search arguments for filtering files
#[derive(Debug, Default)]
pub struct SearchFilesArgs {
	pub name_query: Option<String>,
	pub content_query: Option<String>,
	pub date_from: Option<String>,
	pub date_to: Option<String>,
	pub mime_type: Option<String>,
	pub sort_desc: bool,
	pub page: usize,
	pub page_size: usize,
}

/// Search result with file info and replica count
#[derive(Debug, Clone)]
pub struct FileSearchResult {
	pub hash: Vec<u8>,
	pub name: String,
	pub path: String,
	pub node_id: Vec<u8>,
	pub size: u64,
	pub mime_type: Option<String>,
	pub replicas: u64,
	pub first_datetime: Option<String>,
	pub latest_datetime: Option<String>,
}

/// Search files using file_entries and file_locations tables
/// Returns (results, mime_types, total_count)
pub fn search_files(
	conn: &Connection,
	args: SearchFilesArgs,
) -> anyhow::Result<(Vec<FileSearchResult>, Vec<String>, usize)> {
	// Build WHERE conditions
	let mut conditions: Vec<String> = Vec::new();
	let mut param_values: Vec<String> = Vec::new();

	// Name search - search in file_locations paths
	if let Some(ref name) = args.name_query {
		if !name.trim().is_empty() {
			conditions.push(format!(
				"EXISTS (SELECT 1 FROM file_locations fl WHERE fl.hash = fe.hash AND fl.path LIKE ?{})",
				param_values.len() + 1
			));
			param_values.push(format!("%{}%", name));
		}
	}

	// Mime type filter
	if let Some(ref mime) = args.mime_type {
		if !mime.trim().is_empty() {
			conditions.push(format!("fe.mime_type = ?{}", param_values.len() + 1));
			param_values.push(mime.clone());
		}
	}

	// Date from filter
	if let Some(ref date_from) = args.date_from {
		if !date_from.trim().is_empty() {
			conditions.push(format!("fe.first_datetime >= ?{}", param_values.len() + 1));
			param_values.push(date_from.clone());
		}
	}

	// Date to filter
	if let Some(ref date_to) = args.date_to {
		if !date_to.trim().is_empty() {
			conditions.push(format!("fe.latest_datetime <= ?{}", param_values.len() + 1));
			param_values.push(date_to.clone());
		}
	}

	// Build WHERE clause
	let where_clause = if conditions.is_empty() {
		String::new()
	} else {
		format!(" WHERE {}", conditions.join(" AND "))
	};

	// First, get total count
	let count_sql = format!("SELECT COUNT(*) FROM file_entries fe{}", where_clause);
	let params: Vec<&dyn ToSql> = param_values.iter().map(|s| s as &dyn ToSql).collect();
	let total_count: i64 = conn.query_row(&count_sql, params.as_slice(), |row| row.get(0))?;

	// Build data query with pagination
	let page_size = if args.page_size == 0 { 50 } else { args.page_size };
	let offset = args.page * page_size;

	let order_clause = if args.sort_desc {
		" ORDER BY fe.latest_datetime DESC"
	} else {
		" ORDER BY fe.latest_datetime ASC"
	};

	let data_sql = format!(
		"SELECT
			fe.hash,
			COALESCE(
				(SELECT fl2.path FROM file_locations fl2 WHERE fl2.hash = fe.hash LIMIT 1),
				''
			) as path,
			COALESCE(
				(SELECT fl2.node_id FROM file_locations fl2 WHERE fl2.hash = fe.hash LIMIT 1),
				X''
			) as node_id,
			fe.size,
			fe.mime_type,
			(SELECT COUNT(*) FROM file_locations fl3 WHERE fl3.hash = fe.hash) as replicas,
			fe.first_datetime,
			fe.latest_datetime
		FROM file_entries fe{}{}
		LIMIT {} OFFSET {}",
		where_clause, order_clause, page_size, offset
	);

	// Execute query
	let mut stmt = conn.prepare(&data_sql)?;
	let params: Vec<&dyn ToSql> = param_values.iter().map(|s| s as &dyn ToSql).collect();

	let rows = stmt.query_map(params.as_slice(), |row| {
		let path: String = row.get(1)?;
		let node_id: Vec<u8> = row.get(2)?;
		// Extract filename from path
		let name = path
			.rsplit(|c| c == '/' || c == '\\')
			.next()
			.unwrap_or(&path)
			.to_string();
		Ok(FileSearchResult {
			hash: row.get(0)?,
			name,
			path,
			node_id,
			size: row.get::<_, i64>(3)? as u64,
			mime_type: row.get(4)?,
			replicas: row.get::<_, i64>(5)? as u64,
			first_datetime: row.get(6)?,
			latest_datetime: row.get(7)?,
		})
	})?;

	let mut results = Vec::new();
	for row in rows {
		results.push(row?);
	}

	// Also fetch available mime types for the filter dropdown
	let mut mime_stmt =
		conn.prepare("SELECT DISTINCT mime_type FROM file_entries WHERE mime_type IS NOT NULL")?;
	let mime_rows = mime_stmt.query_map((), |row| row.get::<_, String>(0))?;
	let mut mime_types = Vec::new();
	for mime in mime_rows {
		mime_types.push(mime?);
	}

	Ok((results, mime_types, total_count.max(0) as usize))
}

const RULE_TYPE_OWNER: i64 = 0;
const RULE_TYPE_FOLDER: i64 = 1;

pub fn save_peer_permissions(
	conn: &mut Connection,
	src_peer: &PeerId,
	target_peer: &PeerId,
	permissions: &[Permission],
) -> anyhow::Result<()> {
	let src_bytes = src_peer.to_bytes();
	let target_bytes = target_peer.to_bytes();
	let tx = conn.transaction()?;
	tx.execute(
		"DELETE FROM peer_permissions WHERE src_peer = ?1 AND target_peer = ?2",
		params![&src_bytes, &target_bytes],
	)?;
	for permission in permissions {
		let (rule_type, path_value, flags_value) = match permission.rule() {
			Rule::Owner => (RULE_TYPE_OWNER, None, None),
			Rule::Folder(folder) => (
				RULE_TYPE_FOLDER,
				Some(folder.path().to_string_lossy().into_owned()),
				Some(folder.flags() as i64),
			),
		};
		tx.execute(
			"INSERT INTO peer_permissions (src_peer, target_peer, rule_type, path, flags, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
			params![
				&src_bytes,
				&target_bytes,
				rule_type,
				path_value.as_deref(),
				flags_value,
				permission.expires_at(),
			],
		)?;
	}
	tx.commit()?;
	Ok(())
}

pub fn load_peer_permissions(
	conn: &Connection,
	src_peer: &PeerId,
) -> anyhow::Result<Vec<(PeerId, Vec<Permission>)>> {
	let src_bytes = src_peer.to_bytes();
	let mut stmt = conn.prepare(
		"SELECT target_peer, rule_type, path, flags, expires_at FROM peer_permissions WHERE src_peer = ?1 ORDER BY id ASC",
	)?;
	let mut rows = stmt.query(params![&src_bytes])?;
	let mut results: Vec<(PeerId, Vec<Permission>)> = Vec::new();
	while let Some(row) = rows.next()? {
		let target_bytes: Vec<u8> = row.get(0)?;
		let target_peer = PeerId::from_bytes(&target_bytes)
			.map_err(|err| anyhow!("invalid peer id from database: {err}"))?;
		let rule_type: i64 = row.get(1)?;
		let permission = match rule_type {
			RULE_TYPE_OWNER => Permission::with_expiration(Rule::Owner, row.get(4)?),
			RULE_TYPE_FOLDER => {
				let path: Option<String> = row.get(2)?;
				let flags: Option<i64> = row.get(3)?;
				let path = path.ok_or_else(|| anyhow!("missing folder path for permission"))?;
				let flags = flags.ok_or_else(|| anyhow!("missing folder flags for permission"))?;
				let folder = FolderRule::new(PathBuf::from(path), flags as u8);
				Permission::with_expiration(Rule::Folder(folder), row.get(4)?)
			}
			other => bail!("unsupported rule type {other}"),
		};
		if let Some((_, perms)) = results.iter_mut().find(|(peer, _)| *peer == target_peer) {
			perms.push(permission);
		} else {
			results.push((target_peer, vec![permission]));
		}
	}
	Ok(results)
}

pub fn save_user(conn: &Connection, user: &User) -> anyhow::Result<()> {
	conn.execute(
		r#"
		INSERT INTO users (username, password)
		VALUES (?1, ?2)
		ON CONFLICT(username) DO UPDATE SET password = excluded.password
		"#,
		params![&user.name, &user.passw],
	)?;
	Ok(())
}

pub fn load_users(conn: &Connection) -> anyhow::Result<Vec<User>> {
	let mut stmt = conn.prepare("SELECT username, password FROM users ORDER BY username ASC")?;
	let rows = stmt.query_map([], |row| {
		Ok(User {
			name: row.get(0)?,
			passw: row.get(1)?,
		})
	})?;
	let mut users = Vec::new();
	for user in rows {
		users.push(user?);
	}
	Ok(users)
}

/// Runs embedded database migrations.
///
/// # Arguments
///
/// * `conn` - A mutable reference to the rusqlite `Connection`.
///
/// # Errors
///
/// Returns an `anyhow::Error` if any database operation fails.
pub fn run_migrations(conn: &mut Connection) -> anyhow::Result<()> {
	log::info!("running migrations");
	conn.execute(
		"CREATE TABLE IF NOT EXISTS migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
		(),
	)?;

	let applied_migrations: Vec<u32> = {
		let mut stmt = conn.prepare("SELECT id FROM migrations")?;
		let m = stmt.query_map((), |row| row.get(0))?;
		m.filter_map(Result::ok).collect()
	};

	let mut pending_migrations: Vec<&Migration> = MIGRATIONS
		.iter()
		.filter(|migration| !applied_migrations.contains(&migration.id))
		.collect();

	// Sort pending migrations by id to ensure correct order
	pending_migrations.sort_by_key(|migration| migration.id);
	if !pending_migrations.is_empty() {
		for migration in &pending_migrations {
			log::info!("applying migration {}: {}", migration.id, migration.name);

			// Begin a transaction for atomicity
			let tx = conn.transaction()?;

			// Execute the migration SQL
			tx.execute_batch(migration.sql)?;

			// Record the applied migration
			tx.execute(
				"INSERT INTO migrations (id, name) VALUES (?1, ?2)",
				&[&migration.id as &dyn ToSql, &migration.name as &dyn ToSql],
			)?;

			// Commit the transaction
			tx.commit()?;

			log::info!("migration {} applied successfully.", migration.id);
		}
	} else {
		log::info!("No new migrations to apply.");
	}

	Ok(())
}

pub fn open_db() -> Connection {
	let db_name = env::var("DB").unwrap_or_else(|_| String::from("puppynet.db"));
	Connection::open(db_name).unwrap()
}
