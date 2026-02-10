use crate::db::FileEntry;
use crate::p2p::{CpuInfo, InterfaceInfo};
use crate::updater::UpdateProgress;
use crate::{PuppyNet, StorageUsageFile};
use anyhow::Result;
use async_trait::async_trait;
use libp2p::PeerId;
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use std::str::FromStr;
use std::sync::Arc;
use tokio::{signal, sync::Mutex, task};
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx};
use wgui::{Wgui, WuiModel};

#[derive(Clone, PartialEq, Eq)]
enum Page {
	Home,
	Peers,
	PeerDetail(String),
	Files,
	Search,
	Storage,
	Users,
	Updates,
	Settings,
}

#[derive(Clone)]
struct PeerRow {
	id: String,
	name: String,
	local: bool,
}

#[derive(Clone)]
struct UiState {
	page: Page,
	local_peer_id: Option<String>,
	peers: Vec<PeerRow>,
	selected_peer: Option<String>,
	search_mime_types: Vec<String>,
	peer_cpus: Vec<CpuInfo>,
	peer_interfaces: Vec<InterfaceInfo>,
	files: Vec<FileEntry>,
	storage: Vec<StorageUsageFile>,
	users: Vec<String>,
	status: String,
}

impl UiState {
	fn new() -> Self {
		Self {
			page: Page::Home,
			local_peer_id: None,
			peers: Vec::new(),
			selected_peer: None,
			search_mime_types: Vec::new(),
			peer_cpus: Vec::new(),
			peer_interfaces: Vec::new(),
			files: Vec::new(),
			storage: Vec::new(),
			users: Vec::new(),
			status: String::from("Ready"),
		}
	}
}

#[derive(Clone, Copy, Debug)]
enum UiAction {
	NavHome,
	NavPeers,
	NavFiles,
	NavSearch,
	NavStorage,
	NavUsers,
	NavUpdates,
	NavSettings,
	PeerRow(usize),
	PeerBack,
	RefreshPeers,
	RefreshFiles,
	RefreshStorage,
	RefreshUsers,
	RefreshSearchOptions,
}

pub async fn run_ui(puppy: Arc<PuppyNet>, bind: SocketAddr) -> Result<()> {
	log::info!("starting PuppyNet UI on {}", bind);
	let mut wgui = Wgui::new(bind);
	let server_state = Arc::new(UiServer::new(puppy));
	server_state.refresh_all().await;

	let ctx = Arc::new(Ctx::new(UiContext {
		server: Arc::clone(&server_state),
		sessions: std::sync::Mutex::new(HashMap::new()),
	}));
	wgui.set_ctx(ctx);
	wgui.add_component::<UiRootController>("/");

	let shutdown = signal::ctrl_c();
	tokio::pin!(shutdown);
	let mut run_task = tokio::spawn(async move { wgui.run().await });

	tokio::select! {
		_ = &mut run_task => {}
		_ = &mut shutdown => {
			log::info!("shutting down UI");
		}
	}
	if !run_task.is_finished() {
		run_task.abort();
	}
	let _ = run_task.await;
	Ok(())
}

struct UiServer {
	puppy: Arc<PuppyNet>,
	state: Mutex<UiState>,
}

struct UiRootController {
	ctx: Arc<Ctx<UiContext>>,
}

struct UiContext {
	server: Arc<UiServer>,
	sessions: std::sync::Mutex<HashMap<String, UiClientSession>>,
}

#[derive(Clone, WuiModel)]
struct UiPeer {
	id: String,
	label: String,
}

#[derive(Clone, WuiModel)]
struct UiCpu {
	line: String,
}

#[derive(Clone, WuiModel)]
struct UiInterface {
	line: String,
}

#[derive(Clone, WuiModel)]
struct UiFileRow {
	hash: String,
	line: String,
}

#[derive(Clone, WuiModel)]
struct UiStorageRow {
	line: String,
}

#[derive(Clone, WuiModel)]
struct UiMimeOption {
	name: String,
	selected: bool,
}

#[derive(Clone, WuiModel)]
struct UiSearchRow {
	name: String,
	path: String,
	size: String,
	replicas: String,
	peer_id: String,
}

#[derive(Clone)]
struct UiClientSession {
	authenticated: bool,
	username: String,
	login_username: String,
	login_password: String,
	login_error: String,
	search_name_query: String,
	search_selected_mimes: Vec<String>,
	search_results: Vec<UiSearchRow>,
	search_status: String,
	new_user_username: String,
	new_user_password: String,
	new_user_status: String,
	file_preview_peer: String,
	file_preview_path: String,
	file_preview_status: String,
	file_preview_content: String,
	file_preview_modal_open: bool,
	shell_peer: String,
	shell_input: String,
	shell_output: String,
	shell_status: String,
	shell_session_id: Option<u64>,
	update_version: String,
	update_status: String,
	update_events: Vec<String>,
	update_in_progress: bool,
	update_rx: Option<Arc<std::sync::Mutex<mpsc::Receiver<UpdateProgress>>>>,
}

impl Default for UiClientSession {
	fn default() -> Self {
		Self {
			authenticated: false,
			username: String::new(),
			login_username: String::new(),
			login_password: String::new(),
			login_error: String::new(),
			search_name_query: String::new(),
			search_selected_mimes: Vec::new(),
			search_results: Vec::new(),
			search_status: String::new(),
			new_user_username: String::new(),
			new_user_password: String::new(),
			new_user_status: String::new(),
			file_preview_peer: String::new(),
			file_preview_path: String::new(),
			file_preview_status: String::new(),
			file_preview_content: String::new(),
			file_preview_modal_open: false,
			shell_peer: String::new(),
			shell_input: String::new(),
			shell_output: String::new(),
			shell_status: String::new(),
			shell_session_id: None,
			update_version: String::new(),
			update_status: String::new(),
			update_events: Vec::new(),
			update_in_progress: false,
			update_rx: None,
		}
	}
}

#[derive(Clone, WuiModel)]
struct UiViewState {
	page: String,
	status: String,
	authenticated: bool,
	username: String,
	login_username: String,
	login_password: String,
	login_error: String,
	search_name_query: String,
	search_selected_mimes_text: String,
	search_mime_options: Vec<UiMimeOption>,
	has_search_mime_options: bool,
	search_status: String,
	search_results: Vec<UiSearchRow>,
	search_has_results: bool,
	new_user_username: String,
	new_user_password: String,
	new_user_status: String,
	file_preview_peer: String,
	file_preview_path: String,
	file_preview_status: String,
	file_preview_content: String,
	file_preview_modal_open: bool,
	shell_peer: String,
	shell_input: String,
	shell_output: String,
	shell_status: String,
	shell_has_session: bool,
	update_version: String,
	update_status: String,
	update_events: Vec<String>,
	has_update_events: bool,
	update_in_progress: bool,
	home_peers: String,
	home_files: String,
	home_storage: String,
	home_users: String,
	has_peers: bool,
	has_cpus: bool,
	has_interfaces: bool,
	has_files: bool,
	has_storage_rows: bool,
	has_users: bool,
	selected_peer: String,
	peers: Vec<UiPeer>,
	cpus: Vec<UiCpu>,
	interfaces: Vec<UiInterface>,
	files: Vec<UiFileRow>,
	storage_rows: Vec<UiStorageRow>,
	users: Vec<String>,
}

impl UiRootController {
	fn new(ctx: Arc<Ctx<UiContext>>) -> Self {
		Self { ctx }
	}

	fn session_key(&self) -> String {
		self.ctx
			.session_id()
			.unwrap_or_else(|| format!("client-{}", self.ctx.client_id().unwrap_or(0)))
	}

	fn block_on<F>(&self, fut: F) -> F::Output
	where
		F: Future,
	{
		tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
	}

	fn current_session(&self) -> UiClientSession {
		let key = self.session_key();
		let mut sessions = self.ctx.state.sessions.lock().unwrap();
		sessions.entry(key).or_default().clone()
	}

	fn update_session<F>(&self, f: F)
	where
		F: FnOnce(&mut UiClientSession),
	{
		let key = self.session_key();
		let mut sessions = self.ctx.state.sessions.lock().unwrap();
		let entry = sessions.entry(key).or_default();
		f(entry);
	}

	fn is_authenticated(&self) -> bool {
		self.current_session().authenticated
	}
}

#[wgui_controller]
impl UiRootController {
	pub fn state(&self) -> UiViewState {
		let state = self.block_on(self.ctx.state.server.snapshot());
		let session = self.current_session();
		let peers = state
			.peers
			.into_iter()
			.map(|peer| UiPeer {
				id: peer.id.clone(),
				label: if peer.local {
					format!("{} (you)", peer.name)
				} else {
					peer.name
				},
			})
			.collect::<Vec<_>>();
		let cpus = state
			.peer_cpus
			.into_iter()
			.map(|cpu| UiCpu {
				line: format!("{} — {:.1}% | {} Hz", cpu.name, cpu.usage, cpu.frequency_hz),
			})
			.collect::<Vec<_>>();
		let interfaces = state
			.peer_interfaces
			.into_iter()
			.map(|iface| UiInterface {
				line: format!("{} — {} | {}", iface.name, iface.mac, iface.ips.join(", ")),
			})
			.collect::<Vec<_>>();
		let files = state
			.files
			.into_iter()
			.take(20)
			.map(|entry| UiFileRow {
				hash: format_hash(&entry.hash),
				line: format!("{} — {} bytes", format_hash(&entry.hash), entry.size),
			})
			.collect::<Vec<_>>();
		let storage_rows = state
			.storage
			.into_iter()
			.take(10)
			.map(|entry| UiStorageRow {
				line: format!(
					"{} — {} | {}",
					entry.node_name,
					entry.path,
					format_size(entry.size),
				),
			})
			.collect::<Vec<_>>();
		let users = state.users;
		let search_mime_options = state
			.search_mime_types
			.iter()
			.map(|mime| UiMimeOption {
				name: mime.clone(),
				selected: session
					.search_selected_mimes
					.iter()
					.any(|selected| selected == mime),
			})
			.collect::<Vec<_>>();
		UiViewState {
			page: page_label(&state.page).to_string(),
			status: state.status,
			authenticated: session.authenticated,
			username: session.username,
			login_username: session.login_username,
			login_password: session.login_password,
			login_error: session.login_error,
			search_name_query: session.search_name_query,
			search_selected_mimes_text: if session.search_selected_mimes.is_empty() {
				String::from("All mime types")
			} else {
				session.search_selected_mimes.join(", ")
			},
			has_search_mime_options: !search_mime_options.is_empty(),
			search_mime_options,
			search_status: session.search_status,
			search_has_results: !session.search_results.is_empty(),
			search_results: session.search_results,
			new_user_username: session.new_user_username,
			new_user_password: session.new_user_password,
			new_user_status: session.new_user_status,
			file_preview_peer: session.file_preview_peer,
			file_preview_path: session.file_preview_path,
			file_preview_status: session.file_preview_status,
			file_preview_content: session.file_preview_content,
			file_preview_modal_open: session.file_preview_modal_open,
			shell_peer: session.shell_peer,
			shell_input: session.shell_input,
			shell_output: session.shell_output,
			shell_status: session.shell_status,
			shell_has_session: session.shell_session_id.is_some(),
			update_version: session.update_version,
			update_status: session.update_status,
			update_events: session.update_events.clone(),
			has_update_events: !session.update_events.is_empty(),
			update_in_progress: session.update_in_progress,
			home_peers: format!("Peers: {}", peers.len()),
			home_files: format!("Files captured: {}", files.len()),
			home_storage: format!("Storage entries: {}", storage_rows.len()),
			home_users: format!("Users: {}", users.len()),
			has_peers: !peers.is_empty(),
			has_cpus: !cpus.is_empty(),
			has_interfaces: !interfaces.is_empty(),
			has_files: !files.is_empty(),
			has_storage_rows: !storage_rows.is_empty(),
			has_users: !users.is_empty(),
			selected_peer: state.selected_peer.unwrap_or_default(),
			peers,
			cpus,
			interfaces,
			files,
			storage_rows,
			users,
		}
	}

	pub fn open_login(&mut self) {
		self.ctx.push_state("/login");
	}

	pub fn open_app(&mut self) {
		self.ctx.push_state("/");
	}

	pub fn logout(&mut self) {
		self.update_session(|session| {
			session.authenticated = false;
			session.username.clear();
		});
		self.ctx.push_state("/login");
	}

	pub fn edit_login_username(&mut self, value: String) {
		self.update_session(|session| {
			session.login_username = value;
			session.login_error.clear();
		});
	}

	pub fn edit_login_password(&mut self, value: String) {
		self.update_session(|session| {
			session.login_password = value;
			session.login_error.clear();
		});
	}

	pub fn login(&mut self) {
		let (username, password) = {
			let session = self.current_session();
			(
				session.login_username.trim().to_string(),
				session.login_password.clone(),
			)
		};
		if username.is_empty() || password.trim().is_empty() {
			self.update_session(|session| {
				session.login_error = String::from("Username and password are required");
			});
			return;
		}
		match self.ctx.state.server.puppy.verify_user_credentials(&username, &password) {
			Ok(true) => {
				self.update_session(|session| {
					session.authenticated = true;
					session.username = username.clone();
					session.login_password.clear();
					session.login_error.clear();
				});
				self.ctx.push_state("/");
			}
			Ok(false) => {
				self.update_session(|session| {
					session.login_error = String::from("Invalid credentials");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.login_error = format!("Login failed: {err}");
				});
			}
		}
	}

	pub fn nav_home(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavHome));
	}

	pub fn nav_peers(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavPeers));
	}

	pub fn nav_files(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavFiles));
	}

	pub fn nav_search(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavSearch));
		self.block_on(
			self.ctx
				.state
				.server
				.handle_action(UiAction::RefreshSearchOptions),
		);
	}

	pub fn nav_storage(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavStorage));
	}

	pub fn nav_users(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavUsers));
	}

	pub fn nav_updates(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavUpdates));
	}

	pub fn nav_settings(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::NavSettings));
	}

	pub fn peer_row(&mut self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::PeerRow(idx as usize)));
	}

	pub fn peer_back(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::PeerBack));
	}

	pub fn refresh_peers(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshPeers));
	}

	pub fn refresh_files(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshFiles));
	}

	pub fn preview_local_file(&mut self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let hash = {
			let state = self.block_on(self.ctx.state.server.snapshot());
			state.files.get(idx as usize).map(|entry| entry.hash.clone())
		};
		let Some(hash) = hash else {
			return;
		};
		match self.ctx.state.server.puppy.resolve_local_file_by_hash(&hash) {
			Ok(Some((path, _entry))) => {
				self.update_session(|session| {
					session.file_preview_peer.clear();
					session.file_preview_path = path.to_string_lossy().into_owned();
					session.file_preview_modal_open = true;
				});
				self.load_file_preview();
			}
			Ok(None) => {
				self.update_session(|session| {
					session.file_preview_modal_open = true;
					session.file_preview_status =
						String::from("Local file path not found for selected hash");
					session.file_preview_content.clear();
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_modal_open = true;
					session.file_preview_status = format!("Failed to resolve file: {err}");
					session.file_preview_content.clear();
				});
			}
		}
	}

	pub fn refresh_storage(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshStorage));
	}

	pub fn refresh_users(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshUsers));
	}

	pub fn edit_search_name_query(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_name_query = value;
		});
	}

	pub fn toggle_search_mime(&mut self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let mime = {
			let state = self.block_on(self.ctx.state.server.snapshot());
			state.search_mime_types.get(idx as usize).cloned()
		};
		let Some(mime) = mime else {
			return;
		};
		self.update_session(|session| {
			if let Some(pos) = session
				.search_selected_mimes
				.iter()
				.position(|item| item == &mime)
			{
				session.search_selected_mimes.remove(pos);
			} else {
				session.search_selected_mimes.push(mime);
			}
		});
	}

	pub fn clear_search_mimes(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_selected_mimes.clear();
		});
	}

	pub fn run_search(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let session = self.current_session();
		let query = session.search_name_query;
		let args = crate::SearchFilesArgs {
			name_query: if query.trim().is_empty() {
				None
			} else {
				Some(query.clone())
			},
			mime_types: session.search_selected_mimes,
			page: 0,
			page_size: 50,
			sort_desc: true,
			..Default::default()
		};
		match self.ctx.state.server.puppy.search_files(args) {
			Ok((rows, _mimes, total)) => {
				let view_rows = rows
					.into_iter()
					.map(|row| UiSearchRow {
						name: row.name,
						path: row.path,
						size: format_size(row.size),
						replicas: format!("Replicas: {}", row.replicas),
						peer_id: format_hash(&row.node_id),
					})
					.collect::<Vec<_>>();
				self.update_session(|session| {
					session.search_results = view_rows;
					session.search_status = format!("Found {} result(s)", total);
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.search_status = format!("Search failed: {err}");
					session.search_results.clear();
				});
			}
		}
	}

	pub fn search_preview(&mut self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let row = self
			.current_session()
			.search_results
			.get(idx as usize)
			.cloned();
		if let Some(row) = row {
			self.update_session(|session| {
				session.file_preview_path = row.path;
				session.file_preview_peer = row.peer_id;
				session.file_preview_status.clear();
				session.file_preview_modal_open = true;
			});
			self.load_file_preview();
		}
	}

	pub fn close_file_preview_modal(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_modal_open = false;
		});
	}

	pub fn edit_file_preview_path(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_path = value;
		});
	}

	pub fn edit_file_preview_peer(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_peer = value;
		});
	}

	pub fn load_file_preview(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let (peer_text, path) = {
			let session = self.current_session();
			(
				session.file_preview_peer.trim().to_string(),
				session.file_preview_path.trim().to_string(),
			)
		};
		if path.is_empty() {
			self.update_session(|session| {
				session.file_preview_status = String::from("Path is required");
				session.file_preview_content.clear();
			});
			return;
		}
		let peer = if peer_text.is_empty() {
			self.block_on(self.ctx.state.server.local_peer_id())
		} else {
			self.resolve_peer_ref(&peer_text)
		};
		let peer = match peer {
			Some(peer) => peer,
			None => {
				// Some search rows include node ids that are not directly mappable; prefer local read fallback.
				match self.block_on(self.ctx.state.server.local_peer_id()) {
					Some(local) => local,
					None => {
						self.update_session(|session| {
							session.file_preview_status = String::from("Invalid or missing peer id");
							session.file_preview_content.clear();
						});
						return;
					}
				}
			}
		};
		match self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.read_file(peer, path.clone(), 0, Some(8 * 1024)),
		) {
			Ok(chunk) => {
				let preview = format_preview_bytes(&chunk.data);
				self.update_session(|session| {
					session.file_preview_status = format!(
						"Loaded {} byte(s) from {}{}",
						chunk.data.len(),
						path,
						if chunk.eof { "" } else { " (truncated)" }
					);
					session.file_preview_content = preview;
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_status = format!("Failed to read file: {err}");
					session.file_preview_content.clear();
				});
			}
		}
	}

	fn resolve_peer_ref(&self, value: &str) -> Option<PeerId> {
		if let Ok(peer) = PeerId::from_str(value) {
			return Some(peer);
		}
		let target = value.trim().to_ascii_lowercase();
		if target.is_empty() {
			return None;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let local = snapshot.local_peer_id.clone();
		if let Some(local_id) = local {
			if peer_to_node_id_hex(&local_id) == target {
				if let Ok(peer) = PeerId::from_str(&local_id) {
					return Some(peer);
				}
			}
		}
		snapshot
			.peers
			.iter()
			.find_map(|peer| {
				let node_id = peer_to_node_id_hex(&peer.id);
				if node_id == target {
					PeerId::from_str(&peer.id).ok()
				} else {
					None
				}
			})
	}

	pub fn edit_shell_input(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.shell_input = value;
		});
	}

	pub fn start_shell(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let selected_peer = self.block_on(self.ctx.state.server.snapshot()).selected_peer;
		let Some(selected_peer) = selected_peer else {
			self.update_session(|session| {
				session.shell_status = String::from("Select a peer first");
			});
			return;
		};
		let Ok(peer) = PeerId::from_str(&selected_peer) else {
			self.update_session(|session| {
				session.shell_status = String::from("Invalid selected peer");
			});
			return;
		};
		let session_id = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|value| value.as_millis() as u64)
			.unwrap_or(1);
		match self
			.block_on(self.ctx.state.server.puppy.start_shell(peer, session_id))
		{
			Ok(remote_session) => {
				self.update_session(|session| {
					session.shell_peer = selected_peer;
					session.shell_session_id = Some(remote_session);
					session.shell_status = format!("Shell started (session {remote_session})");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.shell_status = format!("Failed to start shell: {err}");
					session.shell_session_id = None;
				});
			}
		}
	}

	pub fn send_shell_input(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let (peer_text, session_id, input) = {
			let session = self.current_session();
			(
				session.shell_peer.clone(),
				session.shell_session_id,
				session.shell_input.clone(),
			)
		};
		if input.is_empty() {
			return;
		}
		let Some(session_id) = session_id else {
			self.update_session(|session| {
				session.shell_status = String::from("Start a shell first");
			});
			return;
		};
		let Ok(peer) = PeerId::from_str(&peer_text) else {
			self.update_session(|session| {
				session.shell_status = String::from("Invalid shell peer");
			});
			return;
		};
		match self.block_on(self.ctx.state.server.puppy.shell_input(
			peer,
			session_id,
			input.clone().into_bytes(),
		)) {
			Ok(out) => {
				let out_text = String::from_utf8_lossy(&out);
				self.update_session(|session| {
					session.shell_output.push_str(&input);
					session.shell_output.push_str(&out_text);
					session.shell_input.clear();
					session.shell_status = String::from("Shell command sent");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.shell_status = format!("Shell command failed: {err}");
				});
			}
		}
	}

	pub fn edit_update_version(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.update_version = value;
		});
	}

	pub fn start_peer_update(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let selected_peer = self.block_on(self.ctx.state.server.snapshot()).selected_peer;
		let Some(selected_peer) = selected_peer else {
			self.update_session(|session| {
				session.update_status = String::from("Select a peer first");
			});
			return;
		};
		let Ok(peer) = PeerId::from_str(&selected_peer) else {
			self.update_session(|session| {
				session.update_status = String::from("Invalid selected peer");
			});
			return;
		};
		let version = {
			let session = self.current_session();
			let trimmed = session.update_version.trim().to_string();
			if trimmed.is_empty() {
				None
			} else {
				Some(trimmed)
			}
		};
		match self.ctx.state.server.puppy.update_remote_peer(peer, version) {
			Ok(rx) => {
				self.update_session(|session| {
					session.update_rx = Some(rx);
					session.update_in_progress = true;
					session.update_events.clear();
					session.update_status = String::from("Update started");
				});
				self.poll_peer_update();
			}
			Err(err) => {
				self.update_session(|session| {
					session.update_status = format!("Failed to start update: {err}");
				});
			}
		}
	}

	pub fn poll_peer_update(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let rx = self.current_session().update_rx;
		let Some(rx) = rx else {
			self.update_session(|session| {
				session.update_status = String::from("No update in progress");
			});
			return;
		};
		let events = {
			let mut events = Vec::new();
			let stream = match rx.lock() {
				Ok(guard) => guard,
				Err(err) => {
					self.update_session(|session| {
						session.update_status = format!("Update stream lock failed: {err}");
						session.update_in_progress = false;
						session.update_rx = None;
					});
					return;
				}
			};
			loop {
				match stream.try_recv() {
					Ok(event) => events.push(event),
					Err(TryRecvError::Empty) => break,
					Err(TryRecvError::Disconnected) => break,
				}
			}
			events
		};
		if events.is_empty() {
			self.update_session(|session| {
				if session.update_in_progress {
					session.update_status = String::from("Waiting for update events...");
				}
			});
			return;
		}
		let mut completed = false;
		let lines = events
			.into_iter()
			.map(|event| {
				if matches!(
					event,
					UpdateProgress::Completed { .. }
						| UpdateProgress::Failed { .. }
						| UpdateProgress::AlreadyUpToDate { .. }
				) {
					completed = true;
				}
				format_update_progress(&event)
			})
			.collect::<Vec<_>>();
		self.update_session(|session| {
			for line in lines {
				session.update_status = line.clone();
				session.update_events.push(line);
			}
			if completed {
				session.update_in_progress = false;
				session.update_rx = None;
			}
		});
	}

	pub fn edit_new_user_username(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_username = value;
		});
	}

	pub fn edit_new_user_password(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_password = value;
		});
	}

	pub fn create_user(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let (username, password) = {
			let session = self.current_session();
			(
				session.new_user_username.trim().to_string(),
				session.new_user_password.clone(),
			)
		};
		if username.is_empty() || password.trim().is_empty() {
			self.update_session(|session| {
				session.new_user_status = String::from("Username and password are required");
			});
			return;
		}
		match self.ctx.state.server.puppy.create_user(username, password) {
			Ok(()) => {
				self.block_on(self.ctx.state.server.refresh_users());
				self.update_session(|session| {
					session.new_user_password.clear();
					session.new_user_status = String::from("User created");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.new_user_status = format!("Create user failed: {err}");
				});
			}
		}
	}
}

#[async_trait]
impl Component for UiRootController {
	type Context = UiContext;
	type Model = UiViewState;

	async fn mount(ctx: Arc<Ctx<Self::Context>>) -> Self {
		Self::new(ctx)
	}

	fn render(&self, _ctx: &Ctx<Self::Context>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context>>) {}
}

struct UiControllers<'a> {
	server: &'a UiServer,
}

impl<'a> UiControllers<'a> {
	fn new(server: &'a UiServer) -> Self {
		Self { server }
	}

	async fn nav_home(&self) {
		self.server.set_page(Page::Home).await;
	}

	async fn nav_peers(&self) {
		self.server.set_page(Page::Peers).await;
		self.server.refresh_peers().await;
	}

	async fn nav_files(&self) {
		self.server.set_page(Page::Files).await;
		self.server.refresh_files().await;
	}

	async fn nav_search(&self) {
		self.server.set_page(Page::Search).await;
	}

	async fn refresh_search_options(&self) {
		self.server.refresh_search_mime_types().await;
	}

	async fn nav_storage(&self) {
		self.server.set_page(Page::Storage).await;
		self.server.refresh_storage().await;
	}

	async fn nav_users(&self) {
		self.server.set_page(Page::Users).await;
		self.server.refresh_users().await;
	}

	async fn nav_updates(&self) {
		self.server.set_page(Page::Updates).await;
	}

	async fn nav_settings(&self) {
		self.server.set_page(Page::Settings).await;
	}

	async fn open_peer_row(&self, idx: usize) {
		let target = {
			let state = self.server.state.lock().await;
			state.peers.get(idx).map(|peer| peer.id.clone())
		};
		if let Some(peer_id) = target {
			self.server.set_page(Page::PeerDetail(peer_id.clone())).await;
			self.server.refresh_peer_detail(&peer_id).await;
		}
	}

	async fn peer_back(&self) {
		self.server.set_page(Page::Peers).await;
		self.server.refresh_peers().await;
	}

	async fn refresh_peers(&self) {
		self.server.refresh_peers().await;
	}

	async fn refresh_files(&self) {
		self.server.refresh_files().await;
	}

	async fn refresh_storage(&self) {
		self.server.refresh_storage().await;
	}

	async fn refresh_users(&self) {
		self.server.refresh_users().await;
	}
}

impl UiServer {
	fn new(puppy: Arc<PuppyNet>) -> Self {
		Self {
			puppy,
			state: Mutex::new(UiState::new()),
		}
	}

	async fn refresh_all(&self) {
		self.refresh_peers().await;
		self.refresh_files().await;
		self.refresh_storage().await;
		self.refresh_users().await;
		self.refresh_search_mime_types().await;
	}

	async fn refresh_search_mime_types(&self) {
		match self.puppy.get_mime_types() {
			Ok(mimes) => {
				let mut state = self.state.lock().await;
				state.search_mime_types = mimes;
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.status = format!("Failed to load mime types: {err}");
			}
		}
	}

	async fn refresh_peers(&self) {
		match self.puppy.state_snapshot().await {
			Some(snapshot) => {
				let local_id = snapshot.me.to_string();
				let peers = snapshot
					.peers
					.iter()
					.map(|peer| PeerRow {
						id: peer.id.to_string(),
						name: peer.name.clone().unwrap_or_else(|| "Unnamed".to_string()),
						local: peer.id.to_string() == local_id,
					})
					.collect::<Vec<_>>();
				let mut state = self.state.lock().await;
				state.peers = peers;
				state.local_peer_id = Some(local_id);
				state.status = format!("Loaded {} peer(s)", state.peers.len());
			}
			None => {
				let mut state = self.state.lock().await;
				state.status = String::from("Unable to read peer state");
			}
		}
	}

	async fn refresh_files(&self) {
		if let Some(peer) = self.local_peer_id().await {
			match self.puppy.list_file_entries(peer, 0, 25).await {
				Ok(entries) => {
					let mut state = self.state.lock().await;
					state.files = entries;
					state.status = format!("Loaded {} file entries", state.files.len());
				}
				Err(err) => {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load files: {err}");
				}
			}
		} else {
			let mut state = self.state.lock().await;
			state.status = String::from("Local peer id unavailable");
		}
	}

	async fn refresh_storage(&self) {
		match self.puppy.list_storage_files().await {
			Ok(entries) => {
				let mut state = self.state.lock().await;
				state.storage = entries;
				state.status = format!("Indexed {} storage rows", state.storage.len());
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.status = format!("Failed to load storage data: {err}");
			}
		}
	}

	async fn refresh_users(&self) {
		let puppy = Arc::clone(&self.puppy);
		match task::spawn_blocking(move || puppy.list_users_db()).await {
			Ok(Ok(users)) => {
				let mut state = self.state.lock().await;
				state.users = users;
				state.status = format!("Loaded {} users", state.users.len());
			}
			Ok(Err(err)) => {
				let mut state = self.state.lock().await;
				state.status = format!("Failed to load users: {err}");
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.status = format!("Failed to load users: {err}");
			}
		}
	}

	async fn refresh_peer_detail(&self, peer_id: &str) {
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				if let Ok(cpus) = self.puppy.list_cpus(peer).await {
					let mut state = self.state.lock().await;
					state.peer_cpus = cpus;
				} else {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load CPU info for {peer_id}");
				}
				if let Ok(interfaces) = self.puppy.list_interfaces(peer).await {
					let mut state = self.state.lock().await;
					state.peer_interfaces = interfaces;
					state.status = format!("Loaded detail for {peer_id}");
				} else {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load interfaces for {peer_id}");
				}
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.status = format!("Invalid peer id: {err}");
			}
		}
	}

	async fn local_peer_id(&self) -> Option<PeerId> {
		self.puppy.state_snapshot().await.map(|state| state.me)
	}

	async fn handle_action(&self, action: UiAction) {
		let controllers = UiControllers::new(self);
		match action {
			UiAction::NavHome => controllers.nav_home().await,
			UiAction::NavPeers => controllers.nav_peers().await,
			UiAction::NavFiles => controllers.nav_files().await,
			UiAction::NavSearch => controllers.nav_search().await,
			UiAction::NavStorage => controllers.nav_storage().await,
			UiAction::NavUsers => controllers.nav_users().await,
			UiAction::NavUpdates => controllers.nav_updates().await,
			UiAction::NavSettings => controllers.nav_settings().await,
			UiAction::PeerRow(idx) => controllers.open_peer_row(idx).await,
			UiAction::PeerBack => controllers.peer_back().await,
			UiAction::RefreshPeers => controllers.refresh_peers().await,
			UiAction::RefreshFiles => controllers.refresh_files().await,
			UiAction::RefreshStorage => controllers.refresh_storage().await,
			UiAction::RefreshUsers => controllers.refresh_users().await,
			UiAction::RefreshSearchOptions => controllers.refresh_search_options().await,
		}
	}

	async fn set_page(&self, page: Page) {
		let mut state = self.state.lock().await;
		state.page = page.clone();
		state.selected_peer = match page {
			Page::PeerDetail(peer_id) => Some(peer_id),
			_ => None,
		};
	}

	async fn snapshot(&self) -> UiState {
		self.state.lock().await.clone()
	}
}

fn page_label(page: &Page) -> &'static str {
	match page {
		Page::Home => "home",
		Page::Peers => "peers",
		Page::PeerDetail(_) => "peer_detail",
		Page::Files => "files",
		Page::Search => "search",
		Page::Storage => "storage",
		Page::Users => "users",
		Page::Updates => "updates",
		Page::Settings => "settings",
	}
}

fn format_hash(hash: &[u8]) -> String {
	let mut result = String::with_capacity(hash.len() * 2);
	for byte in hash {
		result.push_str(&format!("{:02x}", byte));
	}
	result
}

fn format_size(bytes: u64) -> String {
	const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
	if bytes == 0 {
		return "0 B".to_string();
	}
	let mut size = bytes as f64;
	let mut unit = 0;
	while size >= 1024.0 && unit < UNITS.len() - 1 {
		size /= 1024.0;
		unit += 1;
	}
	format!("{:.2} {}", size, UNITS[unit])
}

fn format_update_progress(progress: &UpdateProgress) -> String {
	match progress {
		UpdateProgress::FetchingRelease => String::from("Fetching release metadata"),
		UpdateProgress::Downloading { filename } => format!("Downloading {filename}"),
		UpdateProgress::Unpacking => String::from("Unpacking update"),
		UpdateProgress::Verifying => String::from("Verifying package"),
		UpdateProgress::Installing => String::from("Installing update"),
		UpdateProgress::Completed { version } => format!("Update completed: {version}"),
		UpdateProgress::Failed { error } => format!("Update failed: {error}"),
		UpdateProgress::AlreadyUpToDate { current_version } => {
			format!("Already up to date ({current_version})")
		}
	}
}

fn format_preview_bytes(data: &[u8]) -> String {
	match std::str::from_utf8(data) {
		Ok(text) => text.to_string(),
		Err(_) => {
			let mut out = String::from("Binary data (first 128 bytes as hex)\n");
			for byte in data.iter().take(128) {
				out.push_str(&format!("{byte:02x} "));
			}
			out.trim_end().to_string()
		}
	}
}

fn peer_to_node_id_hex(peer: &str) -> String {
	let Ok(peer) = PeerId::from_str(peer) else {
		return String::new();
	};
	let bytes = peer.to_bytes();
	let mut node = [0u8; 16];
	let len = node.len().min(bytes.len());
	node[..len].copy_from_slice(&bytes[..len]);
	format_hash(&node)
}
