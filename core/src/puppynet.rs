use crate::app::{App, Command, ReadFileCmd};
use crate::auth;
use crate::db::{
	StorageUsageFile, delete_session, load_discovered_peers, load_peers, load_user, load_users,
	lookup_session_username, open_db, run_migrations, save_session,
};
use crate::p2p::{
	CpuInfo, DirEntry, DiskInfo, InterfaceInfo, PermissionGrant, Thumbnail, grant_from_permission,
	permission_from_grant,
};
use crate::scan::ScanEvent;
use crate::state::{Peer, FLAG_READ, FLAG_SEARCH, FLAG_WRITE, Permission, State};
use crate::updater::{self, UpdateProgress};
use crate::{FileChunk, FileEntry};
use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use futures::executor::block_on;
use libp2p::PeerId;
use rusqlite::{Connection as SqliteConnection, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanResultRow {
	pub hash: Vec<u8>,
	pub size: u64,
	pub mime_type: Option<String>,
	pub first_datetime: Option<String>,
	pub latest_datetime: Option<String>,
}

#[derive(Clone)]
pub struct ScanHandle {
	receiver: Arc<Mutex<mpsc::Receiver<ScanEvent>>>,
	cancel_flag: Arc<AtomicBool>,
}

impl ScanHandle {
	pub fn receiver(&self) -> Arc<Mutex<mpsc::Receiver<ScanEvent>>> {
		Arc::clone(&self.receiver)
	}

	pub fn cancel(&self) {
		self.cancel_flag.store(true, Ordering::SeqCst);
	}
}

pub struct PuppyNet {
	shutdown_tx: Option<oneshot::Sender<()>>,
	handle: JoinHandle<()>,
	cmd_tx: UnboundedSender<Command>,
	db: Arc<Mutex<SqliteConnection>>,
	remote_scans: Arc<Mutex<HashMap<u64, mpsc::Sender<ScanEvent>>>>,
	remote_scan_counter: AtomicU64,
	remote_updates: Arc<Mutex<HashMap<u64, mpsc::Sender<UpdateProgress>>>>,
	remote_update_counter: AtomicU64,
}

impl PuppyNet {
	pub fn new() -> Self {
		let state = State::default();
		let db = Arc::new(Mutex::new(open_db()));
		{
			let mut conn = db.lock().unwrap();
			if let Err(err) = run_migrations(&mut conn) {
				log::error!("failed to run database migrations: {err}");
			}
		}
		// channel to request shutdown
		let (shutdown_tx, shutdown_rx) = oneshot::channel();
		let remote_scans = Arc::new(Mutex::new(HashMap::new()));
		let remote_updates = Arc::new(Mutex::new(HashMap::new()));
		let (mut app, cmd_tx) = App::new(
			state,
			db.clone(),
			remote_scans.clone(),
			remote_updates.clone(),
		);
		let mut shutdown_rx = shutdown_rx;
		let handle = tokio::spawn(async move {
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
			cmd_tx,
			db,
			remote_scans,
			remote_scan_counter: AtomicU64::new(1),
			remote_updates,
			remote_update_counter: AtomicU64::new(1),
		}
	}

	fn local_peer_id(&self) -> Result<PeerId, String> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::GetLocalPeerId { tx })
			.map_err(|e| format!("failed to send GetLocalPeerId command: {e}"))?;
		block_on(rx).map_err(|e| format!("GetLocalPeerId response channel closed: {e}"))
	}

	pub fn inject_discovered_peer(
		&self,
		peer: PeerId,
		addr: libp2p::Multiaddr,
	) -> Result<(), String> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::InjectDiscoveredPeer { peer, addr, tx })
			.map_err(|e| format!("failed to send InjectDiscoveredPeer command: {e}"))?;
		block_on(rx).map_err(|e| format!("InjectDiscoveredPeer response channel closed: {e}"))
	}

	fn register_shared_folder(&self, path: PathBuf, flags: u8) -> anyhow::Result<()> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::RegisterSharedFolder { path, flags, tx })
			.map_err(|e| anyhow!("failed to send RegisterSharedFolder command: {e}"))?;
		block_on(rx).map_err(|e| anyhow!("RegisterSharedFolder response channel closed: {e}"))?
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

	pub fn create_user(&self, username: String, password: String) -> anyhow::Result<()> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::CreateUser {
				username,
				password,
				tx,
			})
			.map_err(|e| anyhow!("failed to send CreateUser command: {e}"))?;
		block_on(rx).map_err(|e| anyhow!("CreateUser response channel closed: {e}"))?
	}

	pub fn set_peer_permissions(
		&self,
		peer: PeerId,
		permissions: Vec<Permission>,
	) -> anyhow::Result<()> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::SetPeerPermissions {
				peer,
				permissions,
				tx,
			})
			.map_err(|e| anyhow!("failed to send SetPeerPermissions command: {e}"))?;
		block_on(rx).map_err(|e| anyhow!("SetPeerPermissions response channel closed: {e}"))?
	}

	pub async fn request_permissions(
		&self,
		peer: PeerId,
		permissions: Vec<Permission>,
		merge: bool,
	) -> anyhow::Result<Vec<Permission>> {
		let grants: Vec<PermissionGrant> = permissions
			.iter()
			.filter_map(grant_from_permission)
			.collect();
		if grants.is_empty() {
			bail!("no permissions to request");
		}
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::GrantPermissions {
				peer,
				username: String::from("gui"),
				permissions: grants,
				merge,
				tx,
			})
			.map_err(|e| anyhow!("failed to send GrantPermissions command: {e}"))?;
		let ack = rx
			.await
			.map_err(|e| anyhow!("GrantPermissions response channel closed: {e}"))??;
		Ok(ack
			.permissions
			.into_iter()
			.filter_map(|grant| permission_from_grant(&grant))
			.collect())
	}

	pub async fn state_snapshot(&self) -> Option<State> {
		let (tx, rx) = oneshot::channel();
		if self.cmd_tx.send(Command::GetState { tx }).is_err() {
			return None;
		}
		rx.await.ok()
	}

	pub fn list_users_db(&self) -> Result<Vec<String>, String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		load_users(&conn)
			.map(|users| users.into_iter().map(|u| u.name).collect())
			.map_err(|err| format!("failed to load users: {err}"))
	}

	pub fn list_peers_db(&self) -> Result<Vec<Peer>, String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		load_peers(&conn).map_err(|err| format!("failed to load peers: {err}"))
	}

	pub fn list_discovered_peers_db(
		&self,
	) -> Result<Vec<crate::state::DiscoveredPeer>, String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		load_discovered_peers(&conn)
			.map_err(|err| format!("failed to load discovered peers: {err}"))
	}

	pub fn verify_user_credentials(
		&self,
		username: &str,
		password: &str,
	) -> anyhow::Result<bool> {
		let conn = self.db.lock().map_err(|err| anyhow!("db lock poisoned: {err}"))?;
		let Some(user) = load_user(&conn, username)? else {
			return Ok(false);
		};
		auth::verify_password(password, &user.passw)
	}

	pub fn save_session(
		&self,
		token_hash: &[u8],
		username: &str,
		ttl_secs: i64,
	) -> anyhow::Result<()> {
		let now = Utc::now().timestamp();
		let expires_at = now.saturating_add(ttl_secs);
		let mut conn = self.db.lock().map_err(|err| anyhow!("db lock poisoned: {err}"))?;
		save_session(&mut *conn, token_hash, username, now, expires_at)?;
		Ok(())
	}

	pub fn http_me(&self, token_hash: &[u8]) -> anyhow::Result<Option<String>> {
		let conn = self.db.lock().map_err(|err| anyhow!("db lock poisoned: {err}"))?;
		lookup_session_username(&conn, token_hash, Utc::now().timestamp())
	}

	pub fn drop_session(&self, token_hash: &[u8]) -> anyhow::Result<()> {
		let mut conn = self.db.lock().map_err(|err| anyhow!("db lock poisoned: {err}"))?;
		delete_session(&mut *conn, token_hash)?;
		Ok(())
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

	pub async fn list_interfaces(&self, peer_id: PeerId) -> Result<Vec<InterfaceInfo>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListInterfaces { tx, peer_id })
			.map_err(|e| anyhow!("failed to send ListInterfaces command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListInterfaces response channel closed: {e}"))?
	}

	pub fn scan_remote_peer(
		&self,
		peer: PeerId,
		path: impl Into<String>,
	) -> Result<ScanHandle, String> {
		let path = path.into();
		if self.local_peer_id()? == peer {
			return self.scan_folder(path);
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
		Ok(ScanHandle {
			receiver: Arc::new(Mutex::new(rx)),
			cancel_flag: Arc::new(AtomicBool::new(false)),
		})
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

	pub async fn list_storage_files(&self) -> Result<Vec<StorageUsageFile>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListStorageFiles { tx })
			.map_err(|e| anyhow!("failed to send ListStorageFiles command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListStorageFiles response channel closed: {e}"))?
	}

	pub fn list_granted_permissions(&self, peer: PeerId) -> Result<Vec<Permission>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListGrantedPermissions { peer, tx })
			.map_err(|e| anyhow!("failed to send ListGrantedPermissions command: {e}"))?;
		block_on(rx).map_err(|e| anyhow!("ListGrantedPermissions response channel closed: {e}"))?
	}

	pub async fn list_permissions(&self, peer: PeerId) -> Result<Vec<Permission>> {
		let (tx, rx) = oneshot::channel();
		self.cmd_tx
			.send(Command::ListPermissions { peer, tx })
			.map_err(|e| anyhow!("failed to send ListPermissions command: {e}"))?;
		rx.await
			.map_err(|e| anyhow!("ListPermissions response channel closed: {e}"))?
	}

	pub fn scan_folder(&self, path: impl Into<String>) -> Result<ScanHandle, String> {
		let path = path.into();
		let (tx, rx) = mpsc::channel();
		let cancel_flag = Arc::new(AtomicBool::new(false));
		self.cmd_tx
			.send(Command::Scan {
				path,
				tx,
				cancel_flag: Arc::clone(&cancel_flag),
			})
			.map_err(|e| format!("failed to send Scan command: {e}"))?;
		Ok(ScanHandle {
			receiver: Arc::new(Mutex::new(rx)),
			cancel_flag,
		})
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

	/// Search files using file_entries and file_locations tables
	/// Returns (results, mime_types, total_count)
	pub fn search_files(
		&self,
		args: crate::db::SearchFilesArgs,
	) -> Result<(Vec<crate::db::FileSearchResult>, Vec<String>, usize), String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		crate::db::search_files(&conn, args).map_err(|err| format!("search failed: {err}"))
	}

	/// Get all available mime types from file_entries
	pub fn get_mime_types(&self) -> Result<Vec<String>, String> {
		let conn = self
			.db
			.lock()
			.map_err(|err| format!("db lock poisoned: {err}"))?;
		let mut stmt = conn
			.prepare("SELECT DISTINCT mime_type FROM file_entries WHERE mime_type IS NOT NULL")
			.map_err(|err| format!("failed to prepare mime types query: {err}"))?;
		let rows = stmt
			.query_map((), |row| row.get::<_, String>(0))
			.map_err(|err| format!("failed to query mime types: {err}"))?;
		let mut mime_types = Vec::new();
		for mime in rows {
			mime_types.push(mime.map_err(|err| format!("failed to read mime type: {err}"))?);
		}
		Ok(mime_types)
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
		let is_self = self.local_peer_id()? == peer;

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
