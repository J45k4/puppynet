use anyhow::Result;
use blake3::Hasher;
use chrono::{DateTime, Utc};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{
	self, Config as RequestResponseConfig, Event as RequestResponseEvent,
	Message as RequestResponseMessage, ProtocolSupport,
};
use libp2p::{
	Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder, identity, noise,
	swarm::{NetworkBehaviour, SwarmEvent},
	tcp, yamux,
};
use libp2p::{mdns, ping};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Disks, Networks, System};
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};
use uuid::Uuid;

use crate::db::FileEntry;
use crate::scan::{ScanEvent, ScanResult};
use crate::types::FileChunk;
use crate::updater::UpdateProgress;
use crate::wait_group::WaitGroupGuard;

const PUPPYNET_PROTOCOL: &str = "/puppynet/0.0.1";
const MAX_FILE_CHUNK: u64 = 4 * 1024 * 1024; // 4 MiB per transfer chunk
const OWNER_ROLE: &str = "owner";
const VIEWER_ROLE: &str = "viewer";
const DEFAULT_SESSION_TTL: u64 = 60 * 60; // 1 hour sessions for credential auth

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerReq {
	ListDir {
		path: String,
	},
	StatFile {
		path: String,
	},
	ReadFile {
		path: String,
		offset: u64,
		length: Option<u64>,
	},
	WriteFile {
		path: String,
		offset: u64,
		data: Vec<u8>,
	},
	ListCpus,
	ListDisks,
	ListInterfaces,
	StartScan {
		id: u64,
		path: String,
	},
	FileEntries {
		offset: u64,
		limit: u64,
	},
	ScanEvent {
		id: u64,
		event: ScanEvent,
	},
	Authenticate {
		method: AuthMethod,
	},
	CreateUser {
		username: String,
		password: String,
		roles: Vec<String>,
		permissions: Vec<PermissionGrant>,
	},
	CreateToken {
		username: String,
		label: Option<String>,
		expires_in: Option<u64>,
		permissions: Vec<PermissionGrant>,
	},
	GrantAccess {
		username: String,
		permissions: Vec<PermissionGrant>,
		merge: bool,
	},
	ListUsers,
	ListTokens {
		username: Option<String>,
	},
	RevokeToken {
		token_id: String,
	},
	RevokeUser {
		username: String,
	},
	ListPermissions,
	GetThumbnail {
		path: String,
		max_width: u32,
		max_height: u32,
	},
	/// Request the peer to update itself to a specific version (or latest if None)
	UpdateSelf {
		id: u64,
		version: Option<String>,
	},
	/// Progress event from a remote update operation
	UpdateEvent {
		id: u64,
		event: UpdateProgress,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerRes {
	DirEntries(Vec<DirEntry>),
	FileStat(DirEntry),
	FileChunk(FileChunk),
	WriteAck(FileWriteAck),
	Cpus(Vec<CpuInfo>),
	Disks(Vec<DiskInfo>),
	Interfaces(Vec<InterfaceInfo>),
	FileEntries(Vec<FileEntry>),
	ScanStarted(Result<(), String>),
	ScanEventAck,
	AuthSuccess {
		session: SessionInfo,
	},
	AuthFailure {
		reason: String,
	},
	UserCreated {
		username: String,
	},
	UserRemoved {
		username: String,
	},
	TokenIssued {
		token: String,
		token_id: String,
		username: String,
		permissions: Vec<PermissionGrant>,
		expires_at: Option<u64>,
	},
	TokenRevoked {
		token_id: String,
	},
	AccessGranted {
		username: String,
		permissions: Vec<PermissionGrant>,
	},
	Users(Vec<UserSummary>),
	Tokens(Vec<TokenInfo>),
	Error(String),
	Permissions(Vec<crate::state::Permission>),
	Thumbnail(Thumbnail),
	/// Acknowledgment that update has started
	UpdateStarted(Result<(), String>),
	/// Acknowledgment for update progress event
	UpdateEventAck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumbnail {
	pub data: Vec<u8>,
	pub width: u32,
	pub height: u32,
	pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
	pub name: String,
	pub is_dir: bool,
	pub extension: Option<String>,
	pub mime: Option<String>,
	pub size: u64,
	pub created_at: Option<DateTime<Utc>>,
	pub modified_at: Option<DateTime<Utc>>,
	pub accessed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriteAck {
	pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
	pub name: String,
	pub usage: f32,
	pub frequency_hz: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
	pub name: String,
	pub mount_path: String,
	pub filesystem: String,
	pub total_space: u64,
	pub available_space: u64,
	pub usage_percent: f32,
	pub total_read_bytes: u64,
	pub total_written_bytes: u64,
	pub read_only: bool,
	pub removable: bool,
	pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceInfo {
	pub name: String,
	pub mac: String,
	pub ips: Vec<String>,
	pub total_received: u64,
	pub total_transmitted: u64,
	pub packets_received: u64,
	pub packets_transmitted: u64,
	pub errors_on_received: u64,
	pub errors_on_transmitted: u64,
	pub mtu: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
	Token { token: String },
	Credentials { username: String, password: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PermissionGrant {
	Owner,
	Viewer,
	Files { path: String, access: FileAccess },
	SystemInfo,
	DiskInfo,
	NetworkInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FileAccess {
	Read,
	ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
	pub session_id: String,
	pub username: String,
	pub roles: Vec<String>,
	pub permissions: Vec<PermissionGrant>,
	pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSummary {
	pub username: String,
	pub roles: Vec<String>,
	pub permissions: Vec<PermissionGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
	pub id: String,
	pub username: String,
	pub label: Option<String>,
	pub permissions: Vec<PermissionGrant>,
	pub expires_at: Option<u64>,
	pub revoked: bool,
	pub issued_at: u64,
	pub issued_by: String,
}

type PuppyNetBehaviour = request_response::json::Behaviour<PeerReq, PeerRes>;

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "AgentEvent", event_process = false)]
pub struct AgentBehaviour {
	ping: ping::Behaviour,
	pub puppynet: PuppyNetBehaviour,
	pub mdns: mdns::tokio::Behaviour,
}

#[derive(Debug)]
pub enum AgentEvent {
	Ping(ping::Event),
	PuppyNet(RequestResponseEvent<PeerReq, PeerRes>),
	Mdns(mdns::Event),
}

impl From<ping::Event> for AgentEvent {
	fn from(event: ping::Event) -> Self {
		AgentEvent::Ping(event)
	}
}

impl From<RequestResponseEvent<PeerReq, PeerRes>> for AgentEvent {
	fn from(event: RequestResponseEvent<PeerReq, PeerRes>) -> Self {
		AgentEvent::PuppyNet(event)
	}
}

impl From<mdns::Event> for AgentEvent {
	fn from(event: mdns::Event) -> Self {
		AgentEvent::Mdns(event)
	}
}

impl AgentBehaviour {
	fn new(local_peer_id: PeerId) -> Self {
		let puppynet_protocol = std::iter::once((
			StreamProtocol::new(PUPPYNET_PROTOCOL),
			ProtocolSupport::Full,
		));
		let puppynet: request_response::Behaviour<
			request_response::json::codec::Codec<PeerReq, PeerRes>,
		> = request_response::json::Behaviour::new(
			puppynet_protocol,
			RequestResponseConfig::default(),
		);
		let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
			.expect("mDNS init failed");
		Self {
			ping: ping::Behaviour::default(),
			puppynet,
			mdns,
		}
	}
}

fn read_file_chunk(path: &str, offset: u64, length: Option<u64>) -> Result<FileChunk, String> {
	let mut file = File::open(path).map_err(|err| err.to_string())?;
	let metadata = file.metadata().map_err(|err| err.to_string())?;
	if metadata.is_dir() {
		return Err("Cannot read directory".to_string());
	}

	file.seek(SeekFrom::Start(offset))
		.map_err(|err| err.to_string())?;

	let mut buffer = Vec::new();
	let max_len = length.unwrap_or(MAX_FILE_CHUNK).min(MAX_FILE_CHUNK) as usize;
	buffer.resize(max_len, 0);
	let bytes_read = file.read(&mut buffer).map_err(|err| err.to_string())?;
	buffer.truncate(bytes_read);

	let eof = file.stream_position().map_err(|err| err.to_string())? >= metadata.len();

	Ok(FileChunk {
		offset,
		data: buffer,
		eof,
	})
}

fn write_file_range(path: &str, offset: u64, data: &[u8]) -> Result<FileWriteAck, String> {
	let mut file = OpenOptions::new()
		.write(true)
		.create(true)
		.read(true)
		.open(path)
		.map_err(|err| err.to_string())?;

	let current_len = file.metadata().map_err(|err| err.to_string())?.len();
	let required_len = offset
		.checked_add(data.len() as u64)
		.ok_or_else(|| "Write would overflow file length".to_string())?;

	if required_len > current_len {
		file.set_len(required_len).map_err(|err| err.to_string())?;
	}

	file.seek(SeekFrom::Start(offset))
		.map_err(|err| err.to_string())?;
	file.write_all(data).map_err(|err| err.to_string())?;

	Ok(FileWriteAck {
		bytes_written: data.len() as u64,
	})
}

fn collect_disk_info() -> Result<Vec<DiskInfo>, String> {
	let disks = Disks::new_with_refreshed_list();
	let infos = disks
		.list()
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
		.collect();
	Ok(infos)
}

#[derive(Debug, Clone)]
struct UserRecord {
	username: String,
	password_hash: String,
	salt: String,
	roles: HashSet<String>,
	permissions: HashSet<PermissionGrant>,
	tokens: HashSet<String>,
}

#[derive(Debug, Clone)]
enum Capability {
	FileRead(String),
	FileWrite(String),
	System,
	Disks,
	Network,
}

impl Capability {
	fn is_read_only(&self) -> bool {
		matches!(
			self,
			Capability::FileRead(_) | Capability::System | Capability::Disks | Capability::Network
		)
	}
}

impl PermissionGrant {
	fn allows(&self, capability: &Capability) -> bool {
		match self {
			PermissionGrant::Owner => true,
			PermissionGrant::Viewer => capability.is_read_only(),
			PermissionGrant::Files { path, access } => match capability {
				Capability::FileRead(request_path) => path_matches(path, request_path),
				Capability::FileWrite(request_path) => {
					matches!(access, FileAccess::ReadWrite) && path_matches(path, request_path)
				}
				Capability::System | Capability::Disks | Capability::Network => false,
			},
			PermissionGrant::SystemInfo => matches!(capability, Capability::System),
			PermissionGrant::DiskInfo => matches!(capability, Capability::Disks),
			PermissionGrant::NetworkInfo => matches!(capability, Capability::Network),
		}
	}
}

fn default_permissions_for_roles(roles: &HashSet<String>) -> HashSet<PermissionGrant> {
	let mut permissions = HashSet::new();
	if roles.contains(OWNER_ROLE) {
		permissions.insert(PermissionGrant::Owner);
		return permissions;
	}
	if roles.contains(VIEWER_ROLE) {
		permissions.insert(PermissionGrant::Viewer);
		permissions.insert(PermissionGrant::SystemInfo);
		permissions.insert(PermissionGrant::DiskInfo);
		permissions.insert(PermissionGrant::NetworkInfo);
		permissions.insert(PermissionGrant::Files {
			path: String::from("/"),
			access: FileAccess::Read,
		});
	}
	permissions
}

fn normalize_role(role: &str) -> String {
	role.trim().to_lowercase()
}

fn now_timestamp() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_secs())
		.unwrap_or(0)
}

fn hash_password(salt: &str, password: &str) -> String {
	let mut hasher = Hasher::new();
	hasher.update(salt.as_bytes());
	hasher.update(password.as_bytes());
	hasher.finalize().to_hex().to_string()
}

fn verify_password(salt: &str, password: &str, expected_hash: &str) -> bool {
	hash_password(salt, password) == expected_hash
}

fn normalize_path(path: &str) -> String {
	let trimmed = path.trim();
	if trimmed.is_empty() {
		return String::from("/");
	}
	let mut normalized = trimmed.replace('\\', "/");
	while normalized.ends_with('/') && normalized.len() > 1 {
		normalized.pop();
	}
	if normalized.is_empty() {
		normalized.push('/');
	}
	normalized
}

fn path_matches(grant: &str, request: &str) -> bool {
	if grant.is_empty() || grant == "/" || grant == "*" {
		return true;
	}
	let grant_norm = normalize_path(grant);
	let request_norm = normalize_path(request);
	let grant_cmp = grant_norm.trim_start_matches('/');
	let request_cmp = request_norm.trim_start_matches('/');
	if request_norm == grant_norm || request_cmp == grant_cmp {
		return true;
	}
	let prefix = format!("{}/", grant_norm);
	let prefix_cmp = format!("{}/", grant_cmp);
	request_norm.starts_with(&prefix) || request_cmp.starts_with(&prefix_cmp)
}

/// Load or generate an Ed25519 keypair and persist it to disk.
pub fn load_or_generate_keypair(path: &Path) -> Result<identity::Keypair> {
	// Ensure parent directory exists if a directory component was provided.
	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() && !parent.exists() {
			std::fs::create_dir_all(parent)?;
			log::info!("created key directory {}", parent.display());
		}
	}
	if path.exists() {
		let bytes = fs::read(path)?;
		let key = Keypair::from_protobuf_encoding(&bytes)?;
		Ok(key.into())
	} else {
		let key = identity::Keypair::generate_ed25519();
		let bytes = key.to_protobuf_encoding()?;
		fs::write(path, &bytes)?;
		Ok(key.into())
	}
}

fn libp2p_multiaddr(address: &Multiaddr, local_ip: IpAddr, peer_id: &PeerId) -> Multiaddr {
	let mut reachable = Multiaddr::empty();
	for protocol in address.iter() {
		match protocol {
			Protocol::Ip4(ip) if ip.is_unspecified() => match local_ip {
				IpAddr::V4(local) => reachable.push(Protocol::Ip4(local)),
				IpAddr::V6(_) => reachable.push(protocol.clone()),
			},
			Protocol::Ip6(ip) if ip.is_unspecified() => match local_ip {
				IpAddr::V6(local) => reachable.push(Protocol::Ip6(local)),
				IpAddr::V4(_) => reachable.push(protocol.clone()),
			},
			_ => reachable.push(protocol.clone()),
		}
	}
	reachable.push(Protocol::P2p(peer_id.clone().into()));
	reachable
}

pub fn build_swarm(id_keys: identity::Keypair, peer_id: PeerId) -> Result<Swarm<AgentBehaviour>> {
	let swarm = SwarmBuilder::with_existing_identity(id_keys)
		.with_tokio()
		.with_tcp(
			tcp::Config::default(),
			noise::Config::new,
			yamux::Config::default,
		)?
		.with_behaviour(|_| AgentBehaviour::new(peer_id))?
		.with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(u64::MAX)))
		.build();
	Ok(swarm)
}
