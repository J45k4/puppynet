use crate::auth;
use crate::p2p::{
	AuthMethod, CpuInfo, DirEntry, DiskInfo, FileWriteAck, InterfaceInfo, PeerReq, PeerRes,
	PermissionGrant, Thumbnail, permission_from_grant,
};
use crate::types::FileChunk;
use crate::updater::{self, UpdateProgress, UpdateResult};
use crate::{
	db::{
		Cpu as DbCpu, FileEntry, Interface as DbInterface, Node, NodeID, StorageUsageFile,
		fetch_file_entries_paginated, load_discovered_peers, load_peer_permissions, load_peers,
		load_users, remove_discovered_peer, remove_stale_cpus, remove_stale_interfaces, save_cpu,
		save_discovered_peer, save_interface, save_node, save_peer, save_user,
	},
	p2p::{AgentBehaviour, AgentEvent, build_swarm, load_or_generate_keypair},
	scan::{self, ScanEvent},
	state::{
		Connection, DiscoveredPeer, FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FolderRule, Peer,
		Permission, State, User,
	},
};
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use libp2p::{
	Multiaddr,
	PeerId,
	Swarm,
	mdns,
	core::connection::ConnectedPoint,
	swarm::SwarmEvent,
};
use rusqlite::{Connection as SqliteConnection, params};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::{
	env,
	net::IpAddr,
	path::{Path, PathBuf},
	sync::atomic::{AtomicBool, Ordering},
};
use sysinfo::{Disks, Networks, System};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::time::{Duration, timeout};
use tokio::{
	sync::{
		mpsc::{UnboundedReceiver, UnboundedSender},
		oneshot,
	},
	task::JoinHandle,
};

use libp2p::request_response::OutboundRequestId;

pub struct ReadFileCmd {
	pub(crate) peer_id: libp2p::PeerId,
	pub(crate) path: String,
	pub(crate) offset: u64,
	pub(crate) length: Option<u64>,
	pub(crate) tx: oneshot::Sender<Result<FileChunk>>,
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
	GrantPermissions {
		peer: PeerId,
		username: String,
		permissions: Vec<PermissionGrant>,
		merge: bool,
		tx: oneshot::Sender<Result<AccessGrantAck>>,
	},
	ReadFile(ReadFileCmd),
	Scan {
		path: String,
		tx: mpsc::Sender<ScanEvent>,
		cancel_flag: Arc<AtomicBool>,
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
	InjectDiscoveredPeer {
		peer: PeerId,
		addr: libp2p::Multiaddr,
		tx: oneshot::Sender<()>,
	},
	GetState {
		tx: oneshot::Sender<State>,
	},
	RegisterSharedFolder {
		path: PathBuf,
		flags: u8,
		tx: oneshot::Sender<anyhow::Result<()>>,
	},
	CreateUser {
		username: String,
		password: String,
		tx: oneshot::Sender<anyhow::Result<()>>,
	},
	SetPeerPermissions {
		peer: PeerId,
		permissions: Vec<Permission>,
		tx: oneshot::Sender<anyhow::Result<()>>,
	},
	ListGrantedPermissions {
		peer: PeerId,
		tx: oneshot::Sender<anyhow::Result<Vec<Permission>>>,
	},
	GetLocalPeerId {
		tx: oneshot::Sender<PeerId>,
	},
	StartShell {
		peer: PeerId,
		session_id: u64,
		tx: oneshot::Sender<Result<u64>>,
	},
	ShellInput {
		peer: PeerId,
		session_id: u64,
		data: Vec<u8>,
		tx: oneshot::Sender<Result<Vec<u8>>>,
	},
}

struct ShellSession {
	child: tokio::process::Child,
	stdin: tokio::process::ChildStdin,
	stdout: tokio::process::ChildStdout,
}

enum ShellInputResult {
	Output(Vec<u8>),
	Exited,
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

trait ResponseDecoder: Sized + Send + 'static {
	fn decode(response: PeerRes) -> anyhow::Result<Self>;
}

#[derive(Debug, Clone)]
pub(crate) struct AccessGrantAck {
	pub(crate) username: String,
	pub(crate) permissions: Vec<PermissionGrant>,
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

impl ResponseDecoder for AccessGrantAck {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::AccessGranted {
				username,
				permissions,
			} => Ok(Self {
				username,
				permissions,
			}),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for u64 {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::ShellStarted { id } => Ok(id),
			other => Err(anyhow!("unexpected response: {:?}", other)),
		}
	}
}

impl ResponseDecoder for Vec<u8> {
	fn decode(response: PeerRes) -> anyhow::Result<Self> {
		match response {
			PeerRes::ShellOutput { data, .. } => Ok(data),
			PeerRes::ShellExited { .. } => Ok(Vec::new()),
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
		Box::new(Self {
			update_id,
			channels,
		})
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

pub struct App {
	state: State,
	swarm: Swarm<AgentBehaviour>,
	rx: UnboundedReceiver<Command>,
	internal_rx: tokio::sync::mpsc::UnboundedReceiver<InternalCommand>,
	internal_tx: tokio::sync::mpsc::UnboundedSender<InternalCommand>,
	pending_requests: HashMap<OutboundRequestId, PendingRequest>,
	system: System,
	db: Arc<Mutex<SqliteConnection>>,
	remote_scans: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
	remote_updates: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
	shell_sessions: HashMap<u64, ShellSession>,
}

impl App {
	fn can_access(&self, peer: PeerId, path: &Path, access: u8) -> bool {
		self.state.has_fs_access(peer, path, access)
	}

	async fn start_shell_session(&mut self, peer: PeerId, session_id: u64) -> anyhow::Result<()> {
		if let Some(mut existing) = self.shell_sessions.remove(&session_id) {
			let _ = existing.child.kill().await;
		}
		let shell_path = env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh"));
		let mut child = TokioCommand::new(shell_path)
			.env("TERM", "xterm-256color")
			.env("PUPPYNET_REMOTE", "1")
			.stdin(std::process::Stdio::piped())
			.stdout(std::process::Stdio::piped())
			.stderr(std::process::Stdio::piped())
			.spawn()
			.map_err(|e| anyhow!("failed to spawn shell: {e}"))?;
		let stdin = child
			.stdin
			.take()
			.ok_or_else(|| anyhow!("failed to take shell stdin"))?;
		let stdout = child
			.stdout
			.take()
			.ok_or_else(|| anyhow!("failed to take shell stdout"))?;
		self.shell_sessions.insert(
			session_id,
			ShellSession {
				child,
				stdin,
				stdout,
			},
		);
		log::info!("[{}] Started remote shell session {}", peer, session_id);
		Ok(())
	}

	fn record_peer_address(&mut self, peer: &PeerId, addr: &Multiaddr) {
		let peer_id = *peer;
		let multiaddr = addr.clone();
		self.state.peer_discovered(peer_id, multiaddr.clone());
		if let Ok(mut conn) = self.db.lock() {
			let _ = save_discovered_peer(
				&mut *conn,
				&DiscoveredPeer {
					peer_id,
					multiaddr,
				},
			);
		}
	}

	fn known_peer_addresses(&self, peer: &PeerId) -> Vec<Multiaddr> {
		self.state
			.discovered_peers
			.iter()
			.filter(|entry| entry.peer_id == *peer)
			.map(|entry| entry.multiaddr.clone())
			.collect()
	}

	async fn process_shell_input(
		&mut self,
		session_id: u64,
		data: &[u8],
		peer: Option<PeerId>,
	) -> anyhow::Result<ShellInputResult> {
		let Some(session) = self.shell_sessions.get_mut(&session_id) else {
			if let Some(peer_id) = peer {
				log::warn!("peer {} requested missing shell session {}", peer_id, session_id);
			}
			return Err(anyhow!("shell session not found"));
		};

		if !data.is_empty() {
			if let Err(err) = session.stdin.write_all(data).await {
				self.shell_sessions.remove(&session_id);
				if let Some(peer_id) = peer {
					log::warn!(
						"[{}] shell stdin failed for session {}: {err}",
						peer_id,
						session_id
					);
				}
				return Err(anyhow!("shell stdin failed: {err}"));
			}
			let _ = session.stdin.flush().await;
		}

		let mut out = Vec::new();
		let mut buf = [0u8; 8192];
		loop {
			match timeout(Duration::from_millis(40), session.stdout.read(&mut buf)).await {
				Ok(Ok(0)) => {
					self.shell_sessions.remove(&session_id);
					return Ok(ShellInputResult::Exited);
				}
				Ok(Ok(n)) => {
					out.extend_from_slice(&buf[..n]);
					if out.len() >= 64 * 1024 {
						break;
					}
				}
				Ok(Err(err)) => {
					self.shell_sessions.remove(&session_id);
					if let Some(peer_id) = peer {
						log::warn!(
							"[{}] shell stdout failed for session {}: {err}",
							peer_id,
							session_id
						);
					}
					return Err(anyhow!("shell stdout failed: {err}"));
				}
				Err(_) => break,
			}
		}

		Ok(ShellInputResult::Output(out))
	}

	pub fn new(
		mut state: State,
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
		let stored_peers = {
			let conn = db.lock().unwrap();
			load_peers(&conn).unwrap_or_else(|err| {
				log::error!("failed to load peers: {err}");
				Vec::new()
			})
		};
		let stored_discovered = {
			let conn = db.lock().unwrap();
			load_discovered_peers(&conn).unwrap_or_else(|err| {
				log::error!("failed to load discovered peers: {err}");
				Vec::new()
			})
		};
		let stored_users = {
			let conn = db.lock().unwrap();
			match load_users(&conn) {
				Ok(users) => users,
				Err(err) => {
					log::error!("failed to load users: {err}");
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
		state.me = peer_id;
		state.users = stored_users;
		state.peers = stored_peers;
		state.discovered_peers = stored_discovered;
		for (target, permissions) in stored_permissions {
			state.set_peer_permissions_from_storage(target, permissions);
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
			shell_sessions: HashMap::new(),
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
				let permissions = self.state.permissions_for_peer(&peer);
				PeerRes::Permissions(permissions)
			}
			PeerReq::Authenticate { method } => match method {
				AuthMethod::Token { token } => todo!(),
				AuthMethod::Credentials { username, password } => todo!(),
			},
			PeerReq::CreateUser {
				username,
				password,
				roles: _,
				permissions: _,
			} => {
				let passw = match auth::hash_password(&password) {
					Ok(hash) => hash,
					Err(err) => {
						log::error!("failed to hash password for {}: {}", username, err);
						return Ok(PeerRes::Error("Failed to hash password".into()));
					}
				};
				let user = User {
					name: username.clone(),
					passw,
				};
				if self.state.users.iter().any(|u| u.name == user.name) {
					return Ok(PeerRes::Error("User already exists".into()));
				}
				match self.db.lock() {
					Ok(mut conn) => {
						if let Err(err) = crate::db::save_user(&mut *conn, &user) {
							log::error!("failed to persist user {}: {}", user.name, err);
							return Ok(PeerRes::Error("Failed to save user".into()));
						}
					}
					Err(err) => {
						log::error!(
							"db lock poisoned while creating user {}: {}",
							user.name,
							err
						);
						return Ok(PeerRes::Error("Database unavailable".into()));
					}
				}
				self.state.users.push(user.clone());
				PeerRes::UserCreated {
					username: user.name,
				}
			}
			PeerReq::CreateToken {
				username,
				label,
				expires_in,
				permissions,
			} => {
				if !self.state.users.iter().any(|u| u.name == username) {
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
			PeerReq::GrantAccess {
				username,
				permissions,
				merge,
			} => {
				let mut mapped: Vec<Permission> = permissions
					.iter()
					.filter_map(permission_from_grant)
					.collect();
				if mapped.is_empty() {
					return Ok(PeerRes::Error(String::from("No permissions to grant")));
				}
				if merge {
					let mut existing = self.state.permissions_granted_to_peer(&peer);
					existing.extend(mapped);
					mapped = existing;
				}
				let me = self.state.me;
				self.state.set_peer_permissions(peer, mapped.clone());
				match self.db.lock() {
					Ok(mut conn) => {
						if let Err(err) =
							crate::db::save_peer_permissions(&mut *conn, &me, &peer, &mapped)
						{
							log::error!("failed to persist granted permissions: {}", err);
							return Ok(PeerRes::Error("Failed to save permissions".into()));
						}
					}
					Err(err) => {
						log::error!(
							"db lock poisoned while granting access to {}: {}",
							peer,
							err
						);
						return Ok(PeerRes::Error("Database unavailable".into()));
					}
				}
				PeerRes::AccessGranted {
					username,
					permissions,
				}
			}
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
					)
					.await;

					// If update_with_progress failed with an error (not a result),
					// send a failure event
					if let Err(err) = result {
								let _ = internal_tx_for_error.send(InternalCommand::SendUpdateEvent {
									target,
									update_id: id,
							event: UpdateProgress::Failed {
								error: err.to_string(),
							},
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
					if matches!(
						event,
						UpdateProgress::Completed { .. }
							| UpdateProgress::Failed { .. }
							| UpdateProgress::AlreadyUpToDate { .. }
					) {
						map.remove(&id);
						}
					} else {
						log::warn!("received update event for unknown id {}", id);
					}
					PeerRes::UpdateEventAck
				}
				PeerReq::StartShell { id } => {
					self.start_shell_session(peer, id).await?;
					PeerRes::ShellStarted { id }
				}
			PeerReq::ShellInput { id, data } => {
				match self
					.process_shell_input(id, &data, Some(peer))
					.await
				{
					Ok(ShellInputResult::Output(out)) => PeerRes::ShellOutput { id, data: out },
					Ok(ShellInputResult::Exited) => PeerRes::ShellExited { id },
					Err(err) => PeerRes::Error(err.to_string()),
				}
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
		match peer_to_node_id(&self.state.me) {
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
						self.state.peer_discovered(peer_id, multiaddr.clone());
						if let Ok(mut conn) = self.db.lock() {
							let _ = save_discovered_peer(
								&mut *conn,
								&DiscoveredPeer {
									peer_id,
									multiaddr: multiaddr.clone(),
								},
							);
						}
						self.swarm.dial(multiaddr).unwrap();
					}
				}
				mdns::Event::Expired(items) => {
					for (peer_id, multiaddr) in items {
						log::info!("mDNS expired peer {} at {}", peer_id, multiaddr);
						self.state.peer_expired(peer_id, multiaddr.clone());
						if let Ok(mut conn) = self.db.lock() {
							let _ = remove_discovered_peer(&mut *conn, &peer_id, &multiaddr);
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
				endpoint,
				num_established: _,
				concurrent_dial_errors: _,
				established_in: _,
			} => {
				log::info!("Connected to peer {}", peer_id);
				self.state.connections.push(Connection {
					peer_id: peer_id.clone(),
					connection_id,
				});
				if let Some(addr) = match endpoint {
					ConnectedPoint::Dialer { address, .. } => Some(address.clone()),
					ConnectedPoint::Listener {
						send_back_addr, ..
					} => Some(send_back_addr.clone()),
				} {
					self.record_peer_address(&peer_id, &addr);
				}
				if let Ok(mut conn) = self.db.lock() {
					let _ = save_peer(
						&mut *conn,
						&Peer {
							id: peer_id,
							name: None,
						},
					);
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
				self.state
					.connections
					.retain(|c| c.connection_id != connection_id);
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
				let is_self = self.state.me == peer;
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
				if self.state.me == peer_id {
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
				if self.state.me == peer_id {
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
				if self.state.me == peer_id {
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
				if self.state.me == peer {
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
				let local_permissions = if self.state.me == peer {
					Some(self.state.permissions_for_peer(&peer))
				} else {
					None
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
			Command::GrantPermissions {
				peer,
				username,
				permissions,
				merge,
				tx,
			} => {
				let request_id = self.swarm.behaviour_mut().puppynet.send_request(
					&peer,
					PeerReq::GrantAccess {
						username,
						permissions,
						merge,
					},
				);
				if let Some(prev) = self
					.pending_requests
					.insert(request_id, Pending::<AccessGrantAck>::new(tx))
				{
					prev.fail(anyhow!("pending GrantPermissions request was replaced"));
				}
			}
			Command::ReadFile(req) => {
				if self.state.me == req.peer_id {
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
			Command::Scan {
				path,
				tx,
				cancel_flag,
			} => {
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
				let cancel_flag = Arc::clone(&cancel_flag);
				tokio::task::spawn_blocking(move || {
					let result = db
						.lock()
						.map_err(|err| format!("db lock poisoned: {}", err))
						.and_then(|mut guard| {
							scan::scan_with_progress_cancelable(
								&node_id,
								&path,
								&mut *guard,
								|progress| {
									let _ = tx.send(ScanEvent::Progress(progress.clone()));
								},
								|| cancel_flag.load(Ordering::SeqCst),
							)
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
				let is_self = self.state.me == peer;
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
				let request_id = self.swarm.behaviour_mut().puppynet.send_request(
					&peer,
					PeerReq::UpdateSelf {
						id: update_id,
						version,
					},
				);
				self.pending_requests.insert(
					request_id,
					PendingRemoteUpdateStart::new(update_id, Arc::clone(&self.remote_updates)),
				);
			}
			Command::InjectDiscoveredPeer { peer, addr, tx } => {
				self.state.peer_discovered(peer, addr);
				let _ = tx.send(());
			}
			Command::GetState { tx } => {
				let _ = tx.send(self.state.clone());
			}
			Command::RegisterSharedFolder { path, flags, tx } => {
				let result = (|| -> anyhow::Result<()> {
					self.state.add_shared_folder(FolderRule::new(path, flags));
					Ok(())
				})();
				let _ = tx.send(result);
			}
			Command::CreateUser {
				username,
				password,
				tx,
			} => {
				let result = (|| -> anyhow::Result<()> {
					if self.state.users.iter().any(|u| u.name == username) {
						bail!("User already exists");
					}
					let passw = auth::hash_password(&password)?;
					let user = User {
						name: username.clone(),
						passw,
					};
					{
						let mut conn = self.db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
						save_user(&mut *conn, &user)?;
					}
					self.state.users.push(user);
					Ok(())
				})();
				let _ = tx.send(result);
			}
			Command::SetPeerPermissions {
				peer,
				permissions,
				tx,
			} => {
				let result = (|| -> anyhow::Result<()> {
					let me = self.state.me;
					self.state.set_peer_permissions(peer, permissions.clone());
					let mut conn = self.db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
					crate::db::save_peer_permissions(&mut *conn, &me, &peer, &permissions)
						.map_err(|err| anyhow!(err))?;
					Ok(())
				})();
				let _ = tx.send(result);
			}
			Command::ListGrantedPermissions { peer, tx } => {
				let result = Ok(self.state.permissions_granted_to_peer(&peer));
				let _ = tx.send(result);
			}
			Command::GetLocalPeerId { tx } => {
				let _ = tx.send(self.state.me);
			}
			Command::StartShell {
				peer,
				session_id,
				tx,
			} => {
				if self.state.me == peer {
					let result = self
						.start_shell_session(peer, session_id)
						.await
						.map(|_| session_id);
					let _ = tx.send(result);
					return;
				}
				let addresses = self.known_peer_addresses(&peer);
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request_with_addresses(
						&peer,
						PeerReq::StartShell { id: session_id },
						addresses,
					);
				self.pending_requests
					.insert(request_id, Pending::<u64>::new(tx));
			}
			Command::ShellInput {
				peer,
				session_id,
				data,
				tx,
			} => {
				if self.state.me == peer {
					let result = match self
						.process_shell_input(session_id, &data, None)
						.await
					{
						Ok(ShellInputResult::Output(out)) => Ok(out),
						Ok(ShellInputResult::Exited) => Ok(Vec::new()),
						Err(err) => Err(err),
					};
					let _ = tx.send(result);
					return;
				}
				let addresses = self.known_peer_addresses(&peer);
				let request_id = self
					.swarm
					.behaviour_mut()
					.puppynet
					.send_request_with_addresses(
						&peer,
						PeerReq::ShellInput {
							id: session_id,
							data,
						},
						addresses,
					);
				self.pending_requests
					.insert(request_id, Pending::<Vec<u8>>::new(tx));
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
				let request_id = self.swarm.behaviour_mut().puppynet.send_request(
					&target,
					PeerReq::UpdateEvent {
						id: update_id,
						event,
					},
				);
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
