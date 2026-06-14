use crate::db::FileEntry;
use crate::p2p::{CpuInfo, DirEntry, InterfaceInfo};
use crate::updater::UpdateProgress;
use crate::{PuppyNet, StorageUsageFile};
use anyhow::Result;
use base64::Engine;
use libp2p::PeerId;
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use tokio::{signal, sync::Mutex, task};
use wgui::wui::runtime::Ctx;
use wgui::{Wgui, WguiModel};

#[path = "pages/mod.rs"]
mod pages;

use pages::{
	FilesController, HomeController, LoginController, NotFoundController, PeerController,
	PeerFilesController, PeersController, SearchController, SettingsController, StorageController,
	UpdatesController, UsersController,
};

#[derive(Clone, PartialEq, Eq)]
enum Page {
	Home,
	Peers,
	PeerDetail(String),
	PeerFiles { peer_id: String, path: String },
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
	peer_files_path: String,
	peer_files: Vec<DirEntry>,
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
			peer_files_path: String::from("/"),
			peer_files: Vec::new(),
			files: Vec::new(),
			storage: Vec::new(),
			users: Vec::new(),
			status: String::from("Ready"),
		}
	}
}

#[derive(Clone, Copy, Debug)]
enum UiAction {
	PeerRow(usize),
	PeerBack,
	RefreshPeers,
	RefreshFiles,
	RefreshStorage,
	RefreshUsers,
	RefreshSearchOptions,
}

pub(super) struct UiServer {
	puppy: Arc<PuppyNet>,
	state: Mutex<UiState>,
}

pub(super) struct UiContext {
	server: Arc<UiServer>,
	sessions: std::sync::Mutex<HashMap<String, UiClientSession>>,
}

#[derive(Clone, WguiModel)]
struct UiPeer {
	id: String,
	short_id: String,
	label: String,
}

#[derive(Clone, WguiModel)]
struct UiCpu {
	line: String,
}

#[derive(Clone, WguiModel)]
struct UiInterface {
	line: String,
}

#[derive(Clone, WguiModel)]
struct UiFileRow {
	hash: String,
	line: String,
}

#[derive(Clone, WguiModel)]
struct UiPeerFileRow {
	name: String,
	summary: String,
	href: String,
	is_dir: bool,
}

#[derive(Clone, WguiModel)]
struct UiStorageRow {
	line: String,
}

#[derive(Clone, WguiModel)]
struct UiMimeOption {
	name: String,
	selected: bool,
}

#[derive(Clone, WguiModel)]
struct UiSearchRow {
	name: String,
	path: String,
	size: String,
	replicas: String,
	peer_id: String,
}

#[derive(Clone, Default)]
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
	file_preview_image_src: String,
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

#[derive(Clone, WguiModel)]
pub(super) struct UiViewState {
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
	file_preview_image_src: String,
	file_preview_has_image: bool,
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
	current_peer: String,
	has_peers: bool,
	has_cpus: bool,
	has_interfaces: bool,
	has_files: bool,
	has_peer_files: bool,
	peer_files_path: String,
	selected_peer_details_href: String,
	selected_peer_files_href: String,
	peer_files_parent_href: String,
	peer_files_has_parent: bool,
	has_storage_rows: bool,
	has_users: bool,
	selected_peer: String,
	peers: Vec<UiPeer>,
	cpus: Vec<UiCpu>,
	interfaces: Vec<UiInterface>,
	files: Vec<UiFileRow>,
	peer_files: Vec<UiPeerFileRow>,
	storage_rows: Vec<UiStorageRow>,
	users: Vec<String>,
}

pub(super) struct UiControllerCore<'a> {
	ctx: &'a Arc<Ctx<UiContext, ()>>,
}

impl<'a> UiControllerCore<'a> {
	pub(super) fn new(ctx: &'a Arc<Ctx<UiContext, ()>>) -> Self {
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

	pub(super) fn is_authenticated(&self) -> bool {
		self.current_session().authenticated
	}
}

fn short_peer_id(peer_id: &str) -> String {
	const EDGE: usize = 10;
	if peer_id.chars().count() <= EDGE * 2 + 1 {
		return peer_id.to_string();
	}
	let start = peer_id.chars().take(EDGE).collect::<String>();
	let end = peer_id
		.chars()
		.rev()
		.take(EDGE)
		.collect::<String>()
		.chars()
		.rev()
		.collect::<String>();
	format!("{start}...{end}")
}

fn normalize_peer_file_path(path: String) -> String {
	let path = path.trim();
	if path.is_empty() {
		String::from("/")
	} else {
		path.to_string()
	}
}

fn parent_peer_file_path(path: &str) -> Option<String> {
	let normalized = normalize_peer_file_path(path.to_string());
	if normalized == "/" {
		return None;
	}
	let trimmed = normalized.trim_end_matches('/');
	match trimmed.rsplit_once('/') {
		Some(("", _)) => Some(String::from("/")),
		Some((parent, _)) => Some(parent.to_string()),
		None => Some(String::from("/")),
	}
}

fn child_peer_file_path(path: &str, name: &str) -> String {
	let normalized = normalize_peer_file_path(path.to_string());
	if normalized == "/" {
		format!("/{name}")
	} else if normalized.ends_with('/') {
		format!("{normalized}{name}")
	} else {
		format!("{normalized}/{name}")
	}
}

fn peer_files_href(peer_id: &str, path: &str) -> String {
	if peer_id.is_empty() {
		return String::from("/peers");
	}
	let query = url::form_urlencoded::Serializer::new(String::new())
		.append_pair("path", path)
		.finish();
	format!("/peers/{peer_id}/files?{query}")
}

fn peer_details_href(peer_id: &str) -> String {
	if peer_id.is_empty() {
		String::from("/peers")
	} else {
		format!("/peers/{peer_id}")
	}
}

impl UiControllerCore<'_> {
	pub(super) fn state(&self) -> UiViewState {
		let state = self.block_on(self.ctx.state.server.snapshot());
		let session = self.current_session();
		let peers = state
			.peers
			.into_iter()
			.map(|peer| UiPeer {
				id: peer.id.clone(),
				short_id: short_peer_id(&peer.id),
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
		let peer_files = state
			.peer_files
			.iter()
			.map(|entry| UiPeerFileRow {
				name: entry.name.clone(),
				summary: if entry.is_dir {
					String::from("Directory")
				} else {
					let kind = entry
						.mime
						.clone()
						.or_else(|| entry.extension.clone())
						.unwrap_or_else(|| String::from("File"));
					format!("{kind} - {}", format_size(entry.size))
				},
				href: peer_files_href(
					state.selected_peer.as_deref().unwrap_or_default(),
					&child_peer_file_path(&state.peer_files_path, &entry.name),
				),
				is_dir: entry.is_dir,
			})
			.collect::<Vec<_>>();
		let peer_files_parent_href = state
			.selected_peer
			.as_deref()
			.zip(parent_peer_file_path(&state.peer_files_path))
			.map(|(peer_id, parent)| peer_files_href(peer_id, &parent))
			.unwrap_or_default();
		let selected_peer_details_href =
			peer_details_href(state.selected_peer.as_deref().unwrap_or_default());
		let selected_peer_files_href =
			peer_files_href(state.selected_peer.as_deref().unwrap_or_default(), "/");
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
			file_preview_has_image: !session.file_preview_image_src.is_empty(),
			file_preview_image_src: session.file_preview_image_src,
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
			current_peer: match state.local_peer_id.clone() {
				Some(peer_id) => format!("Current peer: {peer_id}"),
				None => String::from("Current peer: unavailable"),
			},
			has_peers: !peers.is_empty(),
			has_cpus: !cpus.is_empty(),
			has_interfaces: !interfaces.is_empty(),
			has_files: !files.is_empty(),
			has_peer_files: !peer_files.is_empty(),
			peer_files_path: state.peer_files_path,
			selected_peer_details_href,
			selected_peer_files_href,
			peer_files_has_parent: !peer_files_parent_href.is_empty(),
			peer_files_parent_href,
			has_storage_rows: !storage_rows.is_empty(),
			has_users: !users.is_empty(),
			selected_peer: state.selected_peer.unwrap_or_default(),
			peers,
			cpus,
			interfaces,
			files,
			peer_files,
			storage_rows,
			users,
		}
	}

	fn state_for_page(&self, page: Page) -> UiViewState {
		self.block_on(self.ctx.state.server.set_page(page));
		self.state()
	}

	pub(super) fn files_state(&self) -> UiViewState {
		self.state_for_page(Page::Files)
	}

	pub(super) fn home_state(&self) -> UiViewState {
		self.state_for_page(Page::Home)
	}

	pub(super) fn peer_state(&self, peer_id: String) -> UiViewState {
		let should_refresh =
			self.block_on(self.ctx.state.server.snapshot())
				.selected_peer
				.as_deref() != Some(peer_id.as_str());
		self.block_on(
			self.ctx
				.state
				.server
				.set_page(Page::PeerDetail(peer_id.clone())),
		);
		if should_refresh {
			self.block_on(self.ctx.state.server.refresh_peer_detail(&peer_id));
		}
		self.state()
	}

	pub(super) fn peer_files_state(&self, peer_id: String, path: String) -> UiViewState {
		let path = normalize_peer_file_path(path);
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let page = Page::PeerFiles {
			peer_id: peer_id.clone(),
			path: path.clone(),
		};
		let should_refresh = snapshot.page != page;
		self.block_on(self.ctx.state.server.set_page(page));
		if should_refresh {
			self.block_on(self.ctx.state.server.refresh_peer_files(&peer_id, &path));
		}
		self.state()
	}

	pub(super) fn peers_state(&self) -> UiViewState {
		self.state_for_page(Page::Peers)
	}

	pub(super) fn search_state(&self) -> UiViewState {
		self.state_for_page(Page::Search)
	}

	pub(super) fn settings_state(&self) -> UiViewState {
		self.state_for_page(Page::Settings)
	}

	pub(super) fn storage_state(&self) -> UiViewState {
		self.state_for_page(Page::Storage)
	}

	pub(super) fn updates_state(&self) -> UiViewState {
		self.state_for_page(Page::Updates)
	}

	pub(super) fn users_state(&self) -> UiViewState {
		self.state_for_page(Page::Users)
	}

	pub fn logout(&self) {
		self.update_session(|session| {
			session.authenticated = false;
			session.username.clear();
		});
		self.ctx.push_state("/login");
	}

	pub fn edit_login_username(&self, value: String) {
		self.update_session(|session| {
			session.login_username = value;
			session.login_error.clear();
		});
	}

	pub fn edit_login_password(&self, value: String) {
		self.update_session(|session| {
			session.login_password = value;
			session.login_error.clear();
		});
	}

	pub fn login(&self) {
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
		match self
			.ctx
			.state
			.server
			.puppy
			.verify_user_credentials(&username, &password)
		{
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

	pub fn peer_row(&self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let peer_id = {
			let state = self.block_on(self.ctx.state.server.snapshot());
			state.peers.get(idx as usize).map(|peer| peer.id.clone())
		};
		self.block_on(
			self.ctx
				.state
				.server
				.handle_action(UiAction::PeerRow(idx as usize)),
		);
		if let Some(peer_id) = peer_id {
			self.ctx.push_state(format!("/peers/{peer_id}"));
		}
	}

	pub fn peer_back(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::PeerBack));
		self.ctx.push_state("/peers");
	}

	pub fn refresh_peer_files(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			return;
		};
		self.block_on(
			self.ctx
				.state
				.server
				.refresh_peer_files(&peer_id, &snapshot.peer_files_path),
		);
	}

	pub fn refresh_peers(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshPeers));
	}

	pub fn refresh_files(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshFiles));
	}

	pub fn preview_local_file(&self, idx: u32) {
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
		match self
			.ctx
			.state
			.server
			.puppy
			.resolve_local_file_by_hash(&hash)
		{
			Ok(Some((path, _entry))) => {
				self.update_session(|session| {
					session.file_preview_peer.clear();
					session.file_preview_path = path.to_string_lossy().into_owned();
					session.file_preview_status.clear();
					session.file_preview_content.clear();
					session.file_preview_image_src.clear();
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
					session.file_preview_image_src.clear();
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_modal_open = true;
					session.file_preview_status = format!("Failed to resolve file: {err}");
					session.file_preview_content.clear();
					session.file_preview_image_src.clear();
				});
			}
		}
	}

	pub fn preview_peer_file(&self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let target = {
			let state = self.block_on(self.ctx.state.server.snapshot());
			let Some(peer_id) = state.selected_peer else {
				return;
			};
			state.peer_files.get(idx as usize).and_then(|entry| {
				if entry.is_dir {
					None
				} else {
					Some((
						peer_id,
						child_peer_file_path(&state.peer_files_path, &entry.name),
					))
				}
			})
		};
		let Some((peer_id, path)) = target else {
			return;
		};
		self.update_session(|session| {
			session.file_preview_peer = peer_id;
			session.file_preview_path = path;
			session.file_preview_status.clear();
			session.file_preview_content.clear();
			session.file_preview_image_src.clear();
			session.file_preview_modal_open = true;
		});
		self.load_file_preview();
	}

	pub fn refresh_storage(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(
			self.ctx
				.state
				.server
				.handle_action(UiAction::RefreshStorage),
		);
	}

	pub fn refresh_users(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::RefreshUsers));
	}

	pub fn edit_search_name_query(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_name_query = value;
		});
	}

	pub fn toggle_search_mime(&self, idx: u32) {
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

	pub fn clear_search_mimes(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_selected_mimes.clear();
		});
	}

	pub fn run_search(&self) {
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

	pub fn search_preview(&self, idx: u32) {
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
				session.file_preview_content.clear();
				session.file_preview_image_src.clear();
				session.file_preview_modal_open = true;
			});
			self.load_file_preview();
		}
	}

	pub fn close_file_preview_modal(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_modal_open = false;
		});
	}

	pub fn edit_file_preview_path(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_path = value;
			session.file_preview_status.clear();
			session.file_preview_content.clear();
			session.file_preview_image_src.clear();
		});
	}

	pub fn edit_file_preview_peer(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.file_preview_peer = value;
			session.file_preview_status.clear();
			session.file_preview_content.clear();
			session.file_preview_image_src.clear();
		});
	}

	pub fn load_file_preview(&self) {
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
				session.file_preview_image_src.clear();
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
							session.file_preview_status =
								String::from("Invalid or missing peer id");
							session.file_preview_content.clear();
							session.file_preview_image_src.clear();
						});
						return;
					}
				}
			}
		};
		if is_image_path(&path) {
			match self.block_on(self.ctx.state.server.puppy.get_thumbnail(
				peer,
				path.clone(),
				900,
				700,
			)) {
				Ok(thumbnail) => {
					let encoded = base64::engine::general_purpose::STANDARD.encode(thumbnail.data);
					self.update_session(|session| {
						session.file_preview_status = format!(
							"Loaded image preview from {} ({}x{})",
							path, thumbnail.width, thumbnail.height
						);
						session.file_preview_image_src =
							format!("data:{};base64,{encoded}", thumbnail.mime_type);
						session.file_preview_content.clear();
					});
				}
				Err(err) => {
					self.update_session(|session| {
						session.file_preview_status =
							format!("Failed to load image preview: {err}");
						session.file_preview_content.clear();
						session.file_preview_image_src.clear();
					});
				}
			}
			return;
		}
		match self.block_on(self.ctx.state.server.puppy.read_file(
			peer,
			path.clone(),
			0,
			Some(8 * 1024),
		)) {
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
					session.file_preview_image_src.clear();
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_status = format!("Failed to read file: {err}");
					session.file_preview_content.clear();
					session.file_preview_image_src.clear();
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
		if let Some(local_id) = local
			&& peer_to_node_id_hex(&local_id) == target
			&& let Ok(peer) = PeerId::from_str(&local_id)
		{
			return Some(peer);
		}
		snapshot.peers.iter().find_map(|peer| {
			let node_id = peer_to_node_id_hex(&peer.id);
			if node_id == target {
				PeerId::from_str(&peer.id).ok()
			} else {
				None
			}
		})
	}

	pub fn edit_shell_input(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.shell_input = value;
		});
	}

	pub fn start_shell(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let selected_peer = self
			.block_on(self.ctx.state.server.snapshot())
			.selected_peer;
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
		match self.block_on(self.ctx.state.server.puppy.start_shell(peer, session_id)) {
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

	pub fn send_shell_input(&self) {
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

	pub fn edit_update_version(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.update_version = value;
		});
	}

	pub fn start_peer_update(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let selected_peer = self
			.block_on(self.ctx.state.server.snapshot())
			.selected_peer;
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
		match self
			.ctx
			.state
			.server
			.puppy
			.update_remote_peer(peer, version)
		{
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

	pub fn poll_peer_update(&self) {
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

	pub fn edit_new_user_username(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_username = value;
		});
	}

	pub fn edit_new_user_password(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_password = value;
		});
	}

	pub fn create_user(&self) {
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

struct UiControllers<'a> {
	server: &'a UiServer,
}

impl<'a> UiControllers<'a> {
	fn new(server: &'a UiServer) -> Self {
		Self { server }
	}

	async fn refresh_search_options(&self) {
		self.server.refresh_search_mime_types().await;
	}

	async fn open_peer_row(&self, idx: usize) {
		let target = {
			let state = self.server.state.lock().await;
			state.peers.get(idx).map(|peer| peer.id.clone())
		};
		if let Some(peer_id) = target {
			self.server
				.set_page(Page::PeerDetail(peer_id.clone()))
				.await;
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

	async fn refresh_peer_files(&self, peer_id: &str, path: &str) {
		match PeerId::from_str(peer_id) {
			Ok(peer) => match self.puppy.list_dir(peer, path.to_string()).await {
				Ok(mut entries) => {
					entries.sort_by(|left, right| {
						right
							.is_dir
							.cmp(&left.is_dir)
							.then_with(|| left.name.cmp(&right.name))
					});
					let mut state = self.state.lock().await;
					state.peer_files = entries;
					state.peer_files_path = path.to_string();
					state.status = format!("Loaded {} item(s) from {path}", state.peer_files.len());
				}
				Err(err) => {
					let mut state = self.state.lock().await;
					state.peer_files.clear();
					state.peer_files_path = path.to_string();
					state.status = format!("Failed to load {path}: {err}");
				}
			},
			Err(err) => {
				let mut state = self.state.lock().await;
				state.peer_files.clear();
				state.peer_files_path = path.to_string();
				state.status = format!("Invalid peer id: {err}");
			}
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
			Page::PeerFiles { peer_id, path } => {
				state.peer_files_path = path;
				Some(peer_id)
			}
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
		Page::PeerFiles { .. } => "peer_files",
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

fn is_image_path(path: &str) -> bool {
	mime_guess::from_path(path)
		.first_raw()
		.map(|mime| mime.starts_with("image/"))
		.unwrap_or(false)
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
	wgui.add_page::<HomeController>("/");
	wgui.add_page::<LoginController>("/login");
	wgui.add_page::<PeersController>("/peers");
	wgui.add_page::<PeerFilesController>("/peers/:peer_id/files");
	wgui.add_page::<PeerController>("/peers/:peer_id");
	wgui.add_page::<FilesController>("/files");
	wgui.add_page::<SearchController>("/search");
	wgui.add_page::<StorageController>("/storage");
	wgui.add_page::<UsersController>("/users");
	wgui.add_page::<UpdatesController>("/updates");
	wgui.add_page::<SettingsController>("/settings");
	wgui.add_page::<NotFoundController>("/*");

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

#[cfg(test)]
mod tests {
	use std::path::Path;

	use wgui::wui::runtime::Template;

	#[test]
	fn wui_templates_parse() {
		let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("wui");
		for module_name in [
			"pages/home",
			"pages/login",
			"pages/peers",
			"pages/peer_files",
			"pages/peer",
			"pages/files",
			"pages/search",
			"pages/storage",
			"pages/users",
			"pages/updates",
			"pages/settings",
			"pages/not_found",
		] {
			let path = base_dir.join(format!("{module_name}.wui"));
			let source = std::fs::read_to_string(&path).unwrap();
			Template::parse_with_dir(&source, module_name, path.parent()).unwrap_or_else(
				|diagnostics| panic!("failed to parse {module_name}: {diagnostics:?}"),
			);
		}
	}
}
