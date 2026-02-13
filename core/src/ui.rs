use crate::db::FileEntry;
use crate::p2p::{CpuInfo, DiskInfo, InterfaceInfo};
use crate::scan::ScanEvent;
use crate::updater::UpdateProgress;
use crate::{PuppyNet, StorageUsageFile};
use anyhow::Result;
use async_trait::async_trait;
use libp2p::PeerId;
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
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
	peer_disks: Vec<DiskInfo>,
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
			peer_disks: Vec::new(),
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
	name: String,
	traffic: String,
	status: String,
	status_color: String,
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
struct UiDisk {
	name: String,
	used_color: String,
	used_width: i32,
	free_width: i32,
	usage_text: String,
}

#[derive(Clone, WuiModel)]
struct UiFileRow {
	hash: String,
	title: String,
	meta: String,
	kind: String,
	tile_color: String,
	first_datetime: String,
	latest_datetime: String,
	thumbnail_url: String,
	is_image: bool,
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
	file_search_query: String,
	file_selected_mimes: Vec<String>,
	file_view_table: bool,
	selected_file_hash: String,
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
	scan_path: String,
	scan_status: String,
	scan_events: Vec<String>,
	scan_in_progress: bool,
	scan_rx: Option<Arc<std::sync::Mutex<mpsc::Receiver<ScanEvent>>>>,
	scan_handle: Option<crate::ScanHandle>,
	scan_folder_modal_open: bool,
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
			file_search_query: String::new(),
			file_selected_mimes: Vec::new(),
			file_view_table: false,
			selected_file_hash: String::new(),
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
			scan_path: String::new(),
			scan_status: String::new(),
			scan_events: Vec::new(),
			scan_in_progress: false,
			scan_rx: None,
			scan_handle: None,
			scan_folder_modal_open: false,
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
	file_search_query: String,
	file_mime_options: Vec<UiMimeOption>,
	has_file_mime_options: bool,
	file_view_table: bool,
	file_view_thumbnails: bool,
	file_nodes: Vec<String>,
	has_file_selected: bool,
	file_selected_name: String,
	file_selected_meta: String,
	file_selected_when: String,
	file_selected_device: String,
	file_selected_is_image: bool,
	file_selected_thumbnail_url: String,
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
	scan_path: String,
	scan_status: String,
	scan_events: Vec<String>,
	has_scan_events: bool,
	scan_in_progress: bool,
	scan_folder_modal_open: bool,
	home_peers: String,
	home_files: String,
	home_storage: String,
	home_users: String,
	current_peer: String,
	has_peers: bool,
	has_cpus: bool,
	has_disks: bool,
	has_interfaces: bool,
	has_files: bool,
	has_storage_rows: bool,
	has_users: bool,
	selected_peer: String,
	peers: Vec<UiPeer>,
	cpus: Vec<UiCpu>,
	disks: Vec<UiDisk>,
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
		let local_peer_id = state.local_peer_id.clone().unwrap_or_default();
		let peers = state
			.peers
			.into_iter()
			.map(|peer| UiPeer {
				id: short_peer_id(&peer.id),
				name: if peer.local {
					format!("{} (you)", peer.name)
				} else {
					peer.name
				},
				traffic: String::from("↑ 0kb/s ↓ 0kb/s"),
				status: String::from("online"),
				status_color: String::from("#1a9b2b"),
			})
			.collect::<Vec<_>>();
		let cpus = state
			.peer_cpus
			.into_iter()
			.map(|cpu| UiCpu {
				line: format!("{} — {:.1}% | {} Hz", cpu.name, cpu.usage, cpu.frequency_hz),
			})
			.collect::<Vec<_>>();
		let disks = state
			.peer_disks
			.into_iter()
			.map(|disk| {
				let total_width = 220i32;
				let usage = disk.usage_percent.clamp(0.0, 100.0) as f64;
				let mut used_width = ((usage / 100.0) * total_width as f64).round() as i32;
				if used_width < 0 {
					used_width = 0;
				}
				if used_width > total_width {
					used_width = total_width;
				}
				let free_width = total_width - used_width;
				let used_color = if usage >= 85.0 {
					"#f03a3a"
				} else if usage >= 65.0 {
					"#e3b628"
				} else {
					"#8fe36e"
				};
				let label = if disk.name.trim().is_empty() {
					disk.mount_path.clone()
				} else {
					disk.name.clone()
				};
				UiDisk {
					name: label,
					used_color: used_color.to_string(),
					used_width,
					free_width,
					usage_text: format!(
						"{} free of {}",
						format_size(disk.available_space),
						format_size(disk.total_space),
					),
				}
			})
			.collect::<Vec<_>>();
		let interfaces = state
			.peer_interfaces
			.into_iter()
			.map(|iface| UiInterface {
				line: format!("{} — {} | {}", iface.name, iface.mac, iface.ips.join(", ")),
			})
			.collect::<Vec<_>>();
		let file_query = session.file_search_query.trim().to_ascii_lowercase();
		let file_selected_mimes = session
			.file_selected_mimes
			.iter()
			.map(|mime| mime.to_ascii_lowercase())
			.collect::<Vec<_>>();
		let files = state
			.files
			.into_iter()
			.filter(|entry| {
				if !file_selected_mimes.is_empty() {
					let mime = entry.mime_type.clone().unwrap_or_default().to_ascii_lowercase();
					if !file_selected_mimes.iter().any(|selected| selected == &mime) {
						return false;
					}
				}
				if file_query.is_empty() {
					return true;
				}
				let hash = format_hash(&entry.hash);
				let mime = entry.mime_type.clone().unwrap_or_default();
				hash.to_ascii_lowercase().contains(&file_query)
					|| mime.to_ascii_lowercase().contains(&file_query)
			})
			.take(48)
			.map(|entry| {
				let hash = format_hash(&entry.hash);
				let short = short_hash(&entry.hash);
				let mime = entry.mime_type.clone().unwrap_or_else(|| String::from("unknown"));
				let is_image = mime.contains("image");
				let thumbnail_url = if is_image && !local_peer_id.is_empty() {
					match self.ctx.state.server.puppy.resolve_local_file_by_hash(&entry.hash) {
						Ok(Some((path, _))) => format!(
							"/api/peers/{}/thumbnail?path={}&max_width=1024&max_height=768",
							local_peer_id,
							url_encode(&path.to_string_lossy()),
						),
						_ => String::new(),
					}
				} else {
					String::new()
				};
				let kind = if mime.contains("image") {
					"IMG"
				} else if mime.contains("video") {
					"VID"
				} else if mime.contains("pdf") {
					"PDF"
				} else {
					"FILE"
				};
				let tile_color = if kind == "IMG" {
					"#d9ecff"
				} else if kind == "VID" {
					"#e7f7df"
				} else if kind == "PDF" {
					"#ffe2e2"
				} else {
					"#ececec"
				};
				UiFileRow {
					hash,
					title: short,
					meta: format!("{} | {}", mime, format_size(entry.size.max(0) as u64)),
					kind: kind.to_string(),
					tile_color: tile_color.to_string(),
					first_datetime: entry.first_datetime,
					latest_datetime: entry.latest_datetime,
					thumbnail_url,
					is_image,
				}
			})
			.collect::<Vec<_>>();
		let selected_file = if session.selected_file_hash.is_empty() {
			files.first().cloned()
		} else {
			files
				.iter()
				.find(|entry| entry.hash == session.selected_file_hash)
				.cloned()
				.or_else(|| files.first().cloned())
		};
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
		let file_mime_options = state
			.search_mime_types
			.iter()
			.map(|mime| UiMimeOption {
				name: mime.clone(),
				selected: session
					.file_selected_mimes
					.iter()
					.any(|selected| selected == mime),
			})
			.collect::<Vec<_>>();
		let has_file_mime_options = !file_mime_options.is_empty();
		let file_nodes = peers.iter().map(|peer| peer.id.clone()).collect::<Vec<_>>();
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
			file_search_query: session.file_search_query,
			file_mime_options,
			has_file_mime_options,
			file_view_table: session.file_view_table,
			file_view_thumbnails: !session.file_view_table,
			file_nodes,
			has_file_selected: selected_file.is_some(),
			file_selected_name: selected_file
				.as_ref()
				.map(|entry| entry.title.clone())
				.unwrap_or_default(),
			file_selected_meta: selected_file
				.as_ref()
				.map(|entry| entry.meta.clone())
				.unwrap_or_default(),
			file_selected_when: selected_file
				.as_ref()
				.map(|entry| format!("{} - {}", entry.first_datetime, entry.latest_datetime))
				.unwrap_or_default(),
			file_selected_device: String::from("Local node"),
			file_selected_is_image: selected_file
				.as_ref()
				.map(|entry| entry.is_image)
				.unwrap_or(false),
			file_selected_thumbnail_url: selected_file
				.as_ref()
				.map(|entry| entry.thumbnail_url.clone())
				.unwrap_or_default(),
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
			scan_path: session.scan_path,
			scan_status: session.scan_status,
			scan_events: session.scan_events.clone(),
			has_scan_events: !session.scan_events.is_empty(),
			scan_in_progress: session.scan_in_progress,
			scan_folder_modal_open: session.scan_folder_modal_open,
			home_peers: format!("Peers: {}", peers.len()),
			home_files: format!("Files captured: {}", files.len()),
			home_storage: format!("Storage entries: {}", storage_rows.len()),
			home_users: format!("Users: {}", users.len()),
			current_peer: match state.local_peer_id.clone() {
				Some(peer_id) => format!("Current peer: {peer_id}"),
				None => String::from("Current peer: unavailable"),
			},
			has_peers: !peers.is_empty(),
			has_cpus: !cpus.is_empty(),
			has_disks: !disks.is_empty(),
			has_interfaces: !interfaces.is_empty(),
			has_files: !files.is_empty(),
			has_storage_rows: !storage_rows.is_empty(),
			has_users: !users.is_empty(),
			selected_peer: state.selected_peer.unwrap_or_default(),
			peers,
			cpus,
			disks,
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
		self.block_on(
			self.ctx
				.state
				.server
				.handle_action(UiAction::RefreshSearchOptions),
		);
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
		self.update_session(|session| {
			session.selected_file_hash = format_hash(&hash);
		});
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

	pub fn select_local_file(&mut self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let hash = {
			let state = self.block_on(self.ctx.state.server.snapshot());
			state.files.get(idx as usize).map(|entry| entry.hash)
		};
		let Some(hash) = hash else {
			return;
		};
		self.update_session(|session| {
			session.selected_file_hash = format_hash(&hash);
		});
	}

	pub fn refresh_storage(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshStorage));
	}

	pub fn edit_file_search_query(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_search_query = value;
		});
	}

	pub fn toggle_file_mime(&mut self, idx: u32) {
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
				.file_selected_mimes
				.iter()
				.position(|item| item == &mime)
			{
				session.file_selected_mimes.remove(pos);
			} else {
				session.file_selected_mimes.push(mime);
			}
		});
	}

	pub fn clear_file_mimes(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_selected_mimes.clear();
		});
	}

	pub fn set_files_view_thumbnails(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_view_table = false;
		});
	}

	pub fn set_files_view_table(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_view_table = true;
		});
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

	pub fn edit_scan_path(&mut self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.scan_path = value;
		});
	}

	pub fn open_scan_folder_modal(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.scan_folder_modal_open = true;
		});
	}

	pub fn close_scan_folder_modal(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.scan_folder_modal_open = false;
		});
	}

	pub fn start_peer_scan(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let selected_peer = self.block_on(self.ctx.state.server.snapshot()).selected_peer;
		let Some(selected_peer) = selected_peer else {
			self.update_session(|session| {
				session.scan_status = String::from("Select a peer first");
			});
			return;
		};
		let Ok(peer) = PeerId::from_str(&selected_peer) else {
			self.update_session(|session| {
				session.scan_status = String::from("Invalid selected peer");
			});
			return;
		};
		let path = {
			let session = self.current_session();
			session.scan_path.trim().to_string()
		};
		if path.is_empty() {
			self.update_session(|session| {
				session.scan_status = String::from("Scan path is required");
			});
			return;
		}
		let local_peer = self.block_on(self.ctx.state.server.local_peer_id());
		let is_local_scan = local_peer.as_ref().map(|id| *id == peer).unwrap_or(false);
		if is_local_scan && !Path::new(&path).is_dir() {
			self.update_session(|session| {
				session.scan_status =
					String::from("Local scan path must be an existing directory");
				session.scan_in_progress = false;
				session.scan_rx = None;
				session.scan_handle = None;
			});
			return;
		}
		log::info!(
			"starting {} scan for peer {} path {}",
			if is_local_scan { "local" } else { "remote" },
			selected_peer,
			path
		);
		match self.ctx.state.server.puppy.scan_remote_peer(peer, path.clone()) {
			Ok(handle) => {
				let receiver = handle.receiver();
				self.update_session(|session| {
					session.scan_rx = Some(receiver);
					session.scan_handle = Some(handle);
					session.scan_in_progress = true;
					session.scan_events.clear();
					session.scan_status = if is_local_scan {
						format!("Local scan started for {}", path)
					} else {
						format!("Remote scan requested for {} (waiting for peer)", path)
					};
				});
				self.poll_peer_scan();
			}
			Err(err) => {
				log::warn!("failed to start scan for peer {}: {}", selected_peer, err);
				self.update_session(|session| {
					session.scan_status = format!("Failed to start scan: {err}");
				});
			}
		}
	}

	pub fn poll_peer_scan(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let rx = self.current_session().scan_rx;
		let Some(rx) = rx else {
			self.update_session(|session| {
				session.scan_status = String::from("No scan in progress");
			});
			return;
		};
		let (events, disconnected) = {
			let mut events = Vec::new();
			let mut disconnected = false;
			let stream = match rx.lock() {
				Ok(guard) => guard,
				Err(err) => {
					self.update_session(|session| {
						session.scan_status = format!("Scan stream lock failed: {err}");
						session.scan_in_progress = false;
						session.scan_rx = None;
						session.scan_handle = None;
					});
					return;
				}
			};
			loop {
				match stream.try_recv() {
					Ok(event) => events.push(event),
					Err(TryRecvError::Empty) => break,
					Err(TryRecvError::Disconnected) => {
						disconnected = true;
						break;
					}
				}
			}
			(events, disconnected)
		};
		if events.is_empty() {
			self.update_session(|session| {
				if disconnected {
					session.scan_status = String::from("Scan stream closed");
					session.scan_in_progress = false;
					session.scan_rx = None;
					session.scan_handle = None;
				} else if session.scan_in_progress {
					session.scan_status = String::from("Waiting for scan events...");
				}
			});
			return;
		}
		let mut completed = false;
		let lines = events
			.into_iter()
			.map(|event| {
				if matches!(event, ScanEvent::Finished(_)) {
					completed = true;
				}
				format_scan_event(&event)
			})
			.collect::<Vec<_>>();
		self.update_session(|session| {
			for line in lines {
				session.scan_status = line.clone();
				session.scan_events.push(line);
			}
			if completed {
				session.scan_in_progress = false;
				session.scan_rx = None;
				session.scan_handle = None;
			}
		});
	}

	pub fn cancel_peer_scan(&mut self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let handle = self.current_session().scan_handle;
		let Some(handle) = handle else {
			self.update_session(|session| {
				session.scan_status = String::from("No scan in progress");
			});
			return;
		};
		handle.cancel();
		self.update_session(|session| {
			session.scan_status = String::from("Cancelling scan...");
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
				let mut peers = snapshot
					.peers
					.iter()
					.map(|peer| PeerRow {
						id: peer.id.to_string(),
						name: peer.name.clone().unwrap_or_else(|| "Unnamed".to_string()),
						local: peer.id.to_string() == local_id,
					})
					.collect::<Vec<_>>();
				if !peers.iter().any(|peer| peer.id == local_id) {
					peers.push(PeerRow {
						id: local_id.clone(),
						name: String::from("Current peer"),
						local: true,
					});
				}
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
				if let Ok(disks) = self.puppy.list_disks(peer).await {
					let mut state = self.state.lock().await;
					state.peer_disks = disks;
				} else {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load disks for {peer_id}");
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

fn short_peer_id(peer_id: &str) -> String {
	const LIMIT: usize = 16;
	if peer_id.chars().count() <= LIMIT {
		return peer_id.to_string();
	}
	let mut out = String::new();
	for (idx, ch) in peer_id.chars().enumerate() {
		if idx >= 12 {
			break;
		}
		out.push(ch);
	}
	out.push_str("...");
	out
}

fn short_hash(hash: &[u8]) -> String {
	let text = format_hash(hash);
	let mut out = String::new();
	for (idx, ch) in text.chars().enumerate() {
		if idx >= 12 {
			break;
		}
		out.push(ch);
	}
	out.push_str("...");
	out
}

fn url_encode(input: &str) -> String {
	let mut out = String::with_capacity(input.len());
	for b in input.bytes() {
		let is_unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
		if is_unreserved {
			out.push(b as char);
		} else {
			out.push('%');
			out.push_str(&format!("{b:02X}"));
		}
	}
	out
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

fn format_scan_event(event: &ScanEvent) -> String {
	match event {
		ScanEvent::Progress(progress) => format!(
			"Scanned {}/{} files (inserted {}, updated {}, removed {})",
			progress.processed_files,
			progress.total_files,
			progress.inserted_count,
			progress.updated_count,
			progress.removed_count
		),
		ScanEvent::Finished(Ok(result)) => format!(
			"Scan finished: inserted {}, updated {}, removed {} ({:.2}s)",
			result.inserted_count,
			result.updated_count,
			result.removed_count,
			result.duration.as_secs_f64()
		),
		ScanEvent::Finished(Err(err)) => format!("Scan failed: {err}"),
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
