use crate::p2p::{
	AuthMethod, CpuInfo, DirEntry, DiskInfo, FileWriteAck, InterfaceInfo, PeerReq, PeerRes,
};
use crate::types::FileChunk;
use crate::{
	db::{load_peer_permissions, open_db, run_migrations},
	p2p::{AgentBehaviour, AgentEvent, build_swarm, load_or_generate_keypair},
	state::{Connection, FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FolderRule, Permission, State},
};
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::executor::block_on;
use libp2p::{PeerId, Swarm, mdns, swarm::SwarmEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::{
	env,
	path::{Path, PathBuf},
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
	ListPermissions {
		peer: PeerId,
		tx: oneshot::Sender<Result<Vec<Permission>>>,
	},
	ReadFile(ReadFileCmd),
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

pub struct App {
	state: Arc<Mutex<State>>,
	swarm: Swarm<AgentBehaviour>,
	rx: UnboundedReceiver<Command>,
	pending_requests: HashMap<OutboundRequestId, PendingRequest>,
	system: System,
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

type PendingRequest = Box<dyn PendingResponseHandler>;

impl App {
	fn can_access(&self, peer: PeerId, path: &Path, access: u8) -> bool {
		self.state
			.lock()
			.map(|state| state.has_fs_access(peer, path, access))
			.unwrap_or(false)
	}

	pub fn new(state: Arc<Mutex<State>>) -> (Self, tokio::sync::mpsc::UnboundedSender<Command>) {
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
			let mut conn = open_db();
			if let Err(err) = run_migrations(&mut conn) {
				log::error!("failed to run database migrations: {err}");
				Vec::new()
			} else {
				match load_peer_permissions(&conn, &peer_id) {
					Ok(perms) => perms,
					Err(err) => {
						log::error!("failed to load peer permissions: {err}");
						Vec::new()
					}
				}
			}
		};
		let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

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
		(
			App {
				state,
				swarm,
				rx,
				pending_requests: HashMap::new(),
				system: System::new(),
			},
			tx,
		)
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
				let networks = Networks::new_with_refreshed_list();
				let infos = networks
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
				PeerRes::Interfaces(infos)
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
		};
		Ok(res)
	}

	fn collect_cpu_info(&mut self) -> Vec<CpuInfo> {
		self.system.refresh_cpu_usage();
		self.system
			.cpus()
			.iter()
			.map(|cpu| CpuInfo {
				name: cpu.name().to_string(),
				usage: cpu.cpu_usage(),
				frequency_hz: cpu.frequency(),
			})
			.collect()
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
			AgentEvent::PuppyPeer(event) => {
				match event {
					libp2p::request_response::Event::Message {
						peer,
						connection_id: _,
						message,
					} => {
						match message {
							libp2p::request_response::Message::Request {
								request_id: _,
								request,
								channel,
							} => {
								if let Ok(res) = self.handle_puppy_peer_req(peer, request).await {
									let _ = self
										.swarm
										.behaviour_mut()
										.puppypeer
										.send_response(channel, res);
								} else {
									let _ = self.swarm.behaviour_mut().puppypeer.send_response(
										channel,
										PeerRes::Error("Internal error".into()),
									);
								}
								// self.swarm.behaviour_mut().puppypeer.send_response(channel, PuppyPeerResponse::)
							}
							libp2p::request_response::Message::Response {
								request_id,
								response,
							} => {
								if let Some(pending) = self.pending_requests.remove(&request_id) {
									pending.complete(response);
								}
							}
						}
					}
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
				}
			}
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
					.puppypeer
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
					.puppypeer
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
					.puppypeer
					.send_request(&peer_id, PeerReq::ListDisks);
				self.pending_requests
					.insert(request_id, Pending::<Vec<DiskInfo>>::new(tx));
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
					.puppypeer
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
				let request_id = self.swarm.behaviour_mut().puppypeer.send_request(
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
		}
	}
}

pub struct PuppyPeer {
	shutdown_tx: Option<oneshot::Sender<()>>,
	handle: JoinHandle<()>,
	state: Arc<Mutex<State>>,
	cmd_tx: UnboundedSender<Command>,
}

impl PuppyPeer {
	pub fn new() -> Self {
		let state = Arc::new(Mutex::new(State::default()));
		// channel to request shutdown
		let (shutdown_tx, shutdown_rx) = oneshot::channel();
		let state_clone = state.clone();
		let (mut app, cmd_tx) = App::new(state_clone);
		let mut shutdown_rx = shutdown_rx;
		let handle: JoinHandle<()> = tokio::spawn(async move {
			loop {
				tokio::select! {
					_ = &mut shutdown_rx => {
						log::info!("PuppyPeer shutting down");
						break;
					}
					_ = app.run() => {}
				}
			}
		});

		PuppyPeer {
			shutdown_tx: Some(shutdown_tx),
			handle,
			state,
			cmd_tx,
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
