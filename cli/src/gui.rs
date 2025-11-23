use std::collections::{BTreeSet, HashMap};
use std::mem;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use chrono::{DateTime, Utc};
use iced::alignment::{Horizontal, Vertical};
use iced::executor;
use iced::theme;
use iced::time;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{
	Image, Rule as Divider, button, checkbox, container, pick_list, scrollable, text, text_input,
	tooltip,
};
use iced::{Application, Command, Element, Length, Settings, Subscription, Theme};
use libp2p::PeerId;
use puppynet_core::p2p::{CpuInfo, DirEntry, DiskInfo, InterfaceInfo};
use puppynet_core::scan::ScanEvent;
use puppynet_core::{
	FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FileChunk, FolderRule, Permission, PuppyNet, Rule, State,
	StorageUsageFile, Thumbnail, UpdateProgress,
};
use tokio::task;

const LOCAL_LISTEN_MULTIADDR: &str = "/ip4/0.0.0.0:8336";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const FILE_VIEW_CHUNK_SIZE: u64 = 64 * 1024;
const THUMBNAIL_MAX_SIZE: u32 = 128;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuItem {
	Peers,
	PeersGraph,
	CreateUser,
	FileSearch,
	StorageUsage,
	ScanResults,
	Quit,
}

const MENU_ITEMS: [MenuItem; 7] = [
	MenuItem::Peers,
	MenuItem::PeersGraph,
	MenuItem::CreateUser,
	MenuItem::FileSearch,
	MenuItem::StorageUsage,
	MenuItem::ScanResults,
	MenuItem::Quit,
];

impl MenuItem {
	fn label(self) -> &'static str {
		match self {
			MenuItem::Peers => "Peers",
			MenuItem::PeersGraph => "Peers Graph",
			MenuItem::CreateUser => "Create User",
			MenuItem::FileSearch => "Files",
			MenuItem::StorageUsage => "Storage Usage",
			MenuItem::ScanResults => "Scan Results",
			MenuItem::Quit => "Quit",
		}
	}
}

#[derive(Debug, Clone)]
struct PeerRow {
	id: String,
	address: String,
	status: String,
}

#[derive(Debug, Clone)]
struct PeerCpuState {
	peer_id: String,
	cpus: Vec<CpuInfo>,
}

#[derive(Debug, Clone)]
struct PeerInterfacesState {
	peer_id: String,
	interfaces: Vec<InterfaceInfo>,
	loading: bool,
	error: Option<String>,
}

#[derive(Debug, Clone)]
struct PeerPermissionsState {
	peer_id: String,
	owner: bool,
	folders: Vec<EditableFolderPermission>,
	loading: bool,
	saving: bool,
	error: Option<String>,
}

#[derive(Debug, Clone)]
struct EditableFolderPermission {
	path: String,
	read: bool,
	write: bool,
}

impl PeerPermissionsState {
	fn loading(peer_id: String) -> Self {
		Self {
			peer_id,
			owner: false,
			folders: Vec::new(),
			loading: true,
			saving: false,
			error: None,
		}
	}

	fn from_permissions(peer_id: String, permissions: Vec<Permission>) -> Self {
		let mut state = Self::loading(peer_id);
		state.loading = false;
		for permission in permissions {
			match permission.rule() {
				Rule::Owner => {
					state.owner = true;
				}
				Rule::Folder(rule) => {
					state
						.folders
						.push(EditableFolderPermission::from_rule(rule));
				}
			}
		}
		state
	}

	fn build_permissions(&self) -> Result<Vec<Permission>, String> {
		let mut permissions = Vec::new();
		if self.owner {
			permissions.push(Permission::new(Rule::Owner));
		}
		for folder in &self.folders {
			let permission = folder.to_permission()?;
			permissions.push(permission);
		}
		Ok(permissions)
	}
}

impl EditableFolderPermission {
	fn from_rule(rule: &FolderRule) -> Self {
		Self {
			path: rule.path().to_string_lossy().to_string(),
			read: rule.can_read(),
			write: rule.can_write(),
		}
	}

	fn to_permission(&self) -> Result<Permission, String> {
		if self.path.trim().is_empty() {
			return Err(String::from("Folder path cannot be empty"));
		}
		let normalized = normalize_path(&self.path);
		let mut flags = FLAG_SEARCH;
		if self.read {
			flags |= FLAG_READ;
		}
		if self.write {
			flags |= FLAG_WRITE;
		}
		if flags == FLAG_SEARCH {
			return Err(format!(
				"Permission for {normalized} must allow read or write access"
			));
		}
		let rule = Rule::Folder(FolderRule::new(PathBuf::from(&normalized), flags));
		Ok(Permission::new(rule))
	}
}

#[derive(Debug, Clone)]
struct FileBrowserState {
	peer_id: String,
	path: String,
	entries: Vec<DirEntry>,
	loading: bool,
	error: Option<String>,
	available_roots: Vec<String>,
	disks: Vec<DiskInfo>,
	showing_disks: bool,
	thumbnails: HashMap<String, ThumbnailState>,
}

#[derive(Debug, Clone)]
enum ThumbnailState {
	Loading,
	Loaded(Vec<u8>),
	Failed,
}

impl FileBrowserState {
	fn new(peer_id: String, path: String) -> Self {
		Self {
			peer_id,
			path,
			entries: Vec::new(),
			loading: true,
			error: None,
			available_roots: Vec::new(),
			disks: Vec::new(),
			showing_disks: should_list_disks_first(),
			thumbnails: HashMap::new(),
		}
	}

	fn is_image_entry(entry: &DirEntry) -> bool {
		entry
			.mime
			.as_deref()
			.map(|m| m.starts_with("image/"))
			.unwrap_or(false)
	}

	fn display_path(&self) -> String {
		if self.showing_disks {
			String::from("Disks")
		} else if self.path.trim().is_empty() {
			String::from("/")
		} else {
			self.path.clone()
		}
	}
}

#[derive(Debug, Clone)]
enum FileViewerSource {
	FileBrowser(FileBrowserState),
	StorageUsage(StorageUsageState),
	Files(FileSearchState),
}

#[derive(Debug, Clone)]
struct FileViewerState {
	source: FileViewerSource,
	peer_id: String,
	path: String,
	mime: Option<String>,
	data: Vec<u8>,
	offset: u64,
	eof: bool,
	loading: bool,
	error: Option<String>,
}

impl FileViewerState {
	fn guess_mime(path: &str) -> Option<String> {
		mime_guess::from_path(path)
			.first_raw()
			.map(|s| s.to_string())
	}

	fn from_browser(browser: FileBrowserState, peer_id: String, path: String, mime: Option<String>) -> Self {
		// Use provided mime, or guess from path
		let detected_mime = mime.or_else(|| Self::guess_mime(&path));
		Self {
			peer_id,
			mime: detected_mime,
			source: FileViewerSource::FileBrowser(browser),
			data: Vec::new(),
			offset: 0,
			eof: false,
			loading: true,
			error: None,
			path,
		}
	}

	fn from_storage(storage: StorageUsageState, peer_id: String, path: String) -> Self {
		// Guess mime from path for storage files
		let detected_mime = Self::guess_mime(&path);
		Self {
			peer_id,
			mime: detected_mime,
			source: FileViewerSource::StorageUsage(storage),
			data: Vec::new(),
			offset: 0,
			eof: false,
			loading: true,
			error: None,
			path,
		}
	}

	fn from_files(files_state: FileSearchState, peer_id: String, path: String, mime: Option<String>) -> Self {
		let detected_mime = mime.or_else(|| Self::guess_mime(&path));
		Self {
			peer_id,
			mime: detected_mime,
			source: FileViewerSource::Files(files_state),
			data: Vec::new(),
			offset: 0,
			eof: false,
			loading: true,
			error: None,
			path,
		}
	}

	fn apply_chunk(&mut self, chunk: FileChunk) {
		let offset = chunk.offset;
		let eof = chunk.eof;
		let data = chunk.data;
		if offset != self.offset {
			self.offset = offset;
		}
		if !data.is_empty() {
			self.offset = offset.saturating_add(data.len() as u64);
			self.data.extend_from_slice(&data);
		} else {
			self.offset = offset;
		}
		self.eof = eof;
	}

	fn is_image(&self) -> bool {
		self.mime
			.as_deref()
			.map(|value| value.starts_with("image/"))
			.unwrap_or(false)
	}
}

#[derive(Debug, Clone)]
struct GraphView {
	nodes: Vec<PeerNode>,
	selected: usize,
}

impl GraphView {
	fn new() -> Self {
		Self {
			nodes: Vec::new(),
			selected: 0,
		}
	}

	fn set_peers(&mut self, peers: &[PeerRow]) {
		let count = peers.len().max(1);
		self.nodes = peers
			.iter()
			.enumerate()
			.map(|(idx, peer)| PeerNode {
				id: peer.id.clone(),
				angle: (idx as f32) * (std::f32::consts::TAU / count as f32),
			})
			.collect();
		if self.selected >= self.nodes.len() {
			self.selected = 0;
		}
	}

	fn next(&mut self) {
		if !self.nodes.is_empty() {
			self.selected = (self.selected + 1) % self.nodes.len();
		}
	}

	fn previous(&mut self) {
		if !self.nodes.is_empty() {
			if self.selected == 0 {
				self.selected = self.nodes.len() - 1;
			} else {
				self.selected -= 1;
			}
		}
	}

	fn selected_id(&self) -> Option<&str> {
		self.nodes.get(self.selected).map(|node| node.id.as_str())
	}
}

#[derive(Debug, Clone)]
struct PeerNode {
	id: String,
	angle: f32,
}

#[derive(Debug, Clone)]
struct CreateUserForm {
	username: String,
	password: String,
	status: Option<String>,
}

impl CreateUserForm {
	fn new() -> Self {
		Self {
			username: String::new(),
			password: String::new(),
			status: None,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesViewMode {
	Thumbnails,
	Table,
}

impl FilesViewMode {
	fn label(self) -> &'static str {
		match self {
			FilesViewMode::Thumbnails => "Thumbnails",
			FilesViewMode::Table => "Table",
		}
	}
}

#[derive(Debug, Clone)]
struct FileSearchState {
	view_mode: FilesViewMode,
	name_query: String,
	content_query: String,
	date_from: String,
	date_to: String,
	selected_mime: String,
	mime_filter_input: String,
	available_mime_types: Vec<String>,
	sort_desc: bool,
	results: Vec<FileSearchEntry>,
	loading: bool,
	error: Option<String>,
	// Pagination
	page: usize,
	page_size: usize,
	total_count: usize,
	// Scroll state
	scroll_offset: scrollable::RelativeOffset,
}

#[derive(Debug, Clone)]
pub struct FileSearchEntry {
	hash: String,
	name: String,
	path: String,
	node_id: String,
	size: u64,
	mime_type: Option<String>,
	replicas: u64,
	first: String,
	latest: String,
}

#[derive(Debug, Clone)]
struct ScanState {
	path: String,
	status: Option<String>,
	error: Option<String>,
	scanning: bool,
	total_files: usize,
	processed_files: usize,
}

#[derive(Debug, Clone)]
struct ScanResultsState {
	entries: Vec<FileSearchEntry>,
	loading: bool,
	error: Option<String>,
	page: usize,
	page_size: usize,
	total_entries: usize,
}

#[derive(Debug, Clone)]
struct StorageUsageState {
	nodes: Vec<StorageNodeView>,
	loading: bool,
	error: Option<String>,
}

#[derive(Clone)]
struct ActiveScan {
	id: u64,
	receiver: Arc<Mutex<mpsc::Receiver<ScanEvent>>>,
}

#[derive(Clone)]
struct ActiveUpdate {
	id: u64,
	peer_id: String,
	receiver: Arc<Mutex<mpsc::Receiver<UpdateProgress>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct StorageNodeView {
	name: String,
	id: String,
	total_size: u64,
	entries: Vec<StorageEntryView>,
	expanded: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct StorageEntryView {
	path: String,
	name: String,
	size: u64,
	item_count: u64,
	last_changed: String,
	percent: f32,
	children: Vec<StorageEntryView>,
	expanded: bool,
}

impl FileSearchState {
	fn new() -> Self {
		Self {
			view_mode: FilesViewMode::Table,
			name_query: String::new(),
			content_query: String::new(),
			date_from: String::new(),
			date_to: String::new(),
			selected_mime: String::new(),
			mime_filter_input: String::new(),
			available_mime_types: Vec::new(),
			sort_desc: true,
			results: Vec::new(),
			loading: false,
			error: None,
			page: 0,
			page_size: 50,
			total_count: 0,
			scroll_offset: scrollable::RelativeOffset::START,
		}
	}
}

impl PeerInterfacesState {
	fn loading(peer_id: String) -> Self {
		Self {
			peer_id,
			interfaces: Vec::new(),
			loading: true,
			error: None,
		}
	}
}

impl ScanState {
	fn new() -> Self {
		let default_path = std::env::current_dir()
			.ok()
			.and_then(|path| path.into_os_string().into_string().ok())
			.unwrap_or_else(|| String::from("."));
		Self {
			path: default_path,
			status: None,
			error: None,
			scanning: false,
			total_files: 0,
			processed_files: 0,
		}
	}
}

impl ScanResultsState {
	fn loading(page: usize, page_size: usize) -> Self {
		Self {
			entries: Vec::new(),
			loading: true,
			error: None,
			page,
			page_size,
			total_entries: 0,
		}
	}
}

impl StorageUsageState {
	fn loading() -> Self {
		Self {
			nodes: Vec::new(),
			loading: true,
			error: None,
		}
	}
}

fn map_result<T>(result: anyhow::Result<T, anyhow::Error>) -> Result<T, String> {
	result.map_err(|err| format!("{err}"))
}

async fn list_dir(
	peer: Arc<PuppyNet>,
	peer_id: String,
	path: String,
) -> (String, String, Result<Vec<DirEntry>, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer.list_dir(target, path.clone()).await;
	(peer_id, path, map_result(result))
}

async fn list_permissions(
	peer: Arc<PuppyNet>,
	peer_id: String,
) -> (String, Result<Vec<Permission>, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer.list_permissions(target).await;
	(peer_id, map_result(result))
}

async fn list_granted_permissions(
	peer: Arc<PuppyNet>,
	peer_id: String,
) -> (String, Result<Vec<Permission>, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer.list_granted_permissions(target);
	(peer_id, result.map_err(|err| format!("{err}")))
}

async fn list_disks(
	peer: Arc<PuppyNet>,
	peer_id: String,
) -> (String, Result<Vec<DiskInfo>, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer.list_disks(target).await;
	(peer_id, map_result(result))
}

async fn set_permissions(
	peer: Arc<PuppyNet>,
	peer_id: String,
	permissions: Vec<Permission>,
) -> (String, Result<Vec<Permission>, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer.set_peer_permissions(target, permissions.clone());
	(peer_id, map_result(result.map(|_| permissions)))
}

async fn read_file(
	peer: Arc<PuppyNet>,
	peer_id: String,
	path: String,
	offset: u64,
) -> (String, String, u64, Result<FileChunk, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer
		.read_file(target, path.clone(), offset, Some(FILE_VIEW_CHUNK_SIZE))
		.await;
	(peer_id, path, offset, map_result(result))
}

async fn fetch_thumbnail(
	peer: Arc<PuppyNet>,
	peer_id: String,
	path: String,
) -> (String, Result<Thumbnail, String>) {
	let target = PeerId::from_str(&peer_id).unwrap();
	let result = peer
		.get_thumbnail(target, path.clone(), THUMBNAIL_MAX_SIZE, THUMBNAIL_MAX_SIZE)
		.await;
	(path, map_result(result))
}

pub struct GuiApp {
	peer: Arc<PuppyNet>,
	latest_state: Option<State>,
	local_peer_id: Option<String>,
	menu: MenuItem,
	mode: Mode,
	peers: Vec<PeerRow>,
	selected_peer_id: Option<String>,
	graph: GraphView,
	status: String,
	app_title: String,
	scan_state: ScanState,
	active_scan: Option<ActiveScan>,
	next_scan_id: u64,
	active_update: Option<ActiveUpdate>,
	next_update_id: u64,
}

impl GuiApp {
	fn local_node_id_bytes(&self) -> Vec<u8> {
		if let Ok(state) = self.peer.state().lock() {
			state.me.to_bytes()
		} else {
			self.local_peer_id
				.as_ref()
				.map(|id| id.as_bytes().to_vec())
				.unwrap_or_default()
		}
	}
}

#[derive(Debug, Clone)]
enum Mode {
	Peers,
	PeerActions { peer_id: String },
	PeerPermissions(PeerPermissionsState),
	PeerCpus(PeerCpuState),
	StorageUsage(StorageUsageState),
	PeerInterfaces(PeerInterfacesState),
	FileBrowser(FileBrowserState),
	FileViewer(FileViewerState),
	PeersGraph,
	CreateUser(CreateUserForm),
	FileSearch(FileSearchState),
	ScanResults(ScanResultsState),
}

#[derive(Debug, Clone)]
pub enum GuiMessage {
	Tick,
	MenuSelected(MenuItem),
	BackToPeers,
	PeerActionsRequested(String),
	PeerPermissionsRequested(String),
	PeerPermissionsLoaded {
		peer_id: String,
		permissions: Result<Vec<Permission>, String>,
	},
	PeerPermissionsOwnerToggled(bool),
	PeerPermissionsFolderPathChanged {
		index: usize,
		path: String,
	},
	PeerPermissionsFolderReadToggled {
		index: usize,
		value: bool,
	},
	PeerPermissionsFolderWriteToggled {
		index: usize,
		value: bool,
	},
	PeerPermissionsFolderRemoved(usize),
	PeerPermissionsAddFolder,
	PeerPermissionsSave,
	PeerPermissionsSaved {
		peer_id: String,
		result: Result<Vec<Permission>, String>,
	},
	CpuRequested(String),
	CpuLoaded(String, Result<Vec<CpuInfo>, String>),
	InterfacesRequested(String),
	InterfacesLoaded {
		peer_id: String,
		interfaces: Result<Vec<InterfaceInfo>, String>,
	},
	FileBrowserRequested {
		peer_id: String,
	},
	FileBrowserDisksLoaded {
		peer_id: String,
		disks: Result<Vec<DiskInfo>, String>,
	},
	FileBrowserDiskSelected {
		peer_id: String,
		disk_path: String,
	},
	FileBrowserPermissionsLoaded {
		peer_id: String,
		permissions: Result<Vec<Permission>, String>,
	},
	FileBrowserLoaded {
		peer_id: String,
		path: String,
		entries: Result<Vec<DirEntry>, String>,
	},
	FileEntryActivated(DirEntry),
	FileNavigateUp,
	FileReadLoaded {
		peer_id: String,
		path: String,
		offset: u64,
		result: Result<FileChunk, String>,
	},
	FileReadMore,
	FileViewerBack,
	GraphNext,
	GraphPrev,
	UsernameChanged(String),
	PasswordChanged(String),
	CreateUserSubmit,
	FilesViewModeChanged(FilesViewMode),
	FilesNameQueryChanged(String),
	FilesContentQueryChanged(String),
	FilesDateFromChanged(String),
	FilesDateToChanged(String),
	FileSearchMimeChanged(String),
	FileSearchToggleSort,
	FileSearchExecute,
	FileSearchLoaded(Result<(Vec<FileSearchEntry>, Vec<String>, usize), String>),
	FilesNextPage,
	FilesPrevPage,
	FilesOpenFile {
		node_id: String,
		path: String,
		mime: Option<String>,
	},
	FilesMimeTypesLoaded(Result<Vec<String>, String>),
	FilesScrolled(scrollable::Viewport),
	ScanPathChanged(String),
	ScanRequested,
	ScanEventReceived {
		id: u64,
		event: ScanEvent,
	},
	ScanResultsLoaded {
		page: usize,
		result: Result<(Vec<FileSearchEntry>, usize), String>,
	},
	ScanResultsNextPage,
	ScanResultsPrevPage,
	StorageUsageLoaded(Result<Vec<StorageNodeView>, String>),
	StorageUsageToggleNode(usize),
	StorageUsageToggleEntry {
		node_index: usize,
		path: String,
	},
	StorageUsageOpenFile {
		node_id: String,
		path: String,
	},
	InterfacesFieldEdited,
	ThumbnailLoaded {
		path: String,
		result: Result<Thumbnail, String>,
	},
	/// Request to update a remote peer
	UpdatePeerRequested(String),
	/// Progress event from remote peer update
	UpdatePeerProgress {
		peer_id: String,
		event: UpdateProgress,
	},
}

impl Application for GuiApp {
	type Executor = executor::Default;
	type Message = GuiMessage;
	type Theme = Theme;
	type Flags = String;

	fn new(flags: Self::Flags) -> (Self, Command<Self::Message>) {
		let peer = Arc::new(PuppyNet::new());
		let latest_state = peer.state().lock().ok().map(|state| state.clone());
		let peers = latest_state
			.as_ref()
			.map(aggregate_peers)
			.unwrap_or_default();
		let mut graph = GraphView::new();
		graph.set_peers(&peers);
		let app = GuiApp {
			peer,
			latest_state: latest_state.clone(),
			local_peer_id: latest_state.as_ref().map(|state| state.me.to_string()),
			menu: MenuItem::Peers,
			mode: Mode::Peers,
			peers,
			selected_peer_id: None,
			graph,
			status: String::from("Ready"),
			app_title: flags,
			scan_state: ScanState::new(),
			active_scan: None,
			next_scan_id: 1,
			active_update: None,
			next_update_id: 1,
		};
		(app, Command::none())
	}

	fn title(&self) -> String {
		self.app_title.clone()
	}

	fn theme(&self) -> Theme {
		Theme::Dark
	}

	fn subscription(&self) -> Subscription<Self::Message> {
		time::every(REFRESH_INTERVAL).map(|_| GuiMessage::Tick)
	}

	fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
		match message {
			GuiMessage::Tick => {
				self.refresh_from_state();
				Command::none()
			}
			GuiMessage::MenuSelected(item) => {
				match item {
					MenuItem::Quit => {
						std::process::exit(0);
					}
					MenuItem::Peers => {
						self.menu = item;
						self.refresh_from_state();
						self.mode = Mode::Peers;
						self.status = if self.peers.is_empty() {
							String::from("Showing peers — none discovered")
						} else {
							format!("Showing peers — {} total", self.peers.len())
						};
					}
					MenuItem::PeersGraph => {
						self.menu = item;
						self.mode = Mode::PeersGraph;
						self.refresh_from_state();
						self.selected_peer_id = self.graph.selected_id().map(|id| id.to_string());
						self.status = match self.selected_peer_id.as_deref() {
							Some(id) => format!("Graph overview — focused on {}", id),
							None => String::from("Graph overview — no peers"),
						};
					}
					MenuItem::CreateUser => {
						self.menu = item;
						self.mode = Mode::CreateUser(CreateUserForm::new());
						self.status = String::from("Create user form");
					}
					MenuItem::FileSearch => {
						self.menu = item;
						self.mode = Mode::FileSearch(FileSearchState::new());
						self.status = String::from("Loading mime types...");
						let peer = self.peer.clone();
						return Command::perform(
							load_mime_types(peer),
							GuiMessage::FilesMimeTypesLoaded,
						);
					}
					MenuItem::StorageUsage => {
						self.menu = item;
						self.mode = Mode::StorageUsage(StorageUsageState::loading());
						self.status = String::from("Loading storage usage...");
						let peer = self.peer.clone();
						return Command::perform(
							load_storage_usage(peer),
							GuiMessage::StorageUsageLoaded,
						);
					}
					MenuItem::ScanResults => {
						self.menu = item;
						let state = ScanResultsState::loading(0, 25);
						self.status = String::from("Loading scan results...");
						self.mode = Mode::ScanResults(state);
						let peer = self.peer.clone();
						return Command::perform(
							load_scan_results_page(peer, 0, 25),
							move |result| GuiMessage::ScanResultsLoaded { page: 0, result },
						);
					}
				}
				Command::none()
			}
			GuiMessage::BackToPeers => {
				self.menu = MenuItem::Peers;
				self.mode = Mode::Peers;
				Command::none()
			}
			GuiMessage::PeerActionsRequested(peer_id) => {
				self.mode = Mode::PeerActions {
					peer_id: peer_id.clone(),
				};
				self.selected_peer_id = Some(peer_id.clone());
				self.status = format!("Peer actions for {}", peer_id);
				Command::none()
			}
			GuiMessage::PeerPermissionsRequested(peer_id) => {
				self.status = format!("Loading permissions for {}...", peer_id);
				self.selected_peer_id = Some(peer_id.clone());
				self.mode = Mode::PeerPermissions(PeerPermissionsState::loading(peer_id.clone()));
				let peer = self.peer.clone();
				Command::perform(
					list_granted_permissions(peer, peer_id.clone()),
					|(peer_id, permissions)| GuiMessage::PeerPermissionsLoaded {
						peer_id,
						permissions,
					},
				)
			}
			GuiMessage::PeerPermissionsLoaded {
				peer_id,
				permissions,
			} => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if state.peer_id == peer_id {
						match permissions {
							Ok(perms) => {
								*state =
									PeerPermissionsState::from_permissions(peer_id.clone(), perms);
								self.status = format!("Permissions loaded for {}", peer_id);
							}
							Err(err) => {
								state.loading = false;
								state.error = Some(err.clone());
								self.status =
									format!("Failed to load permissions for {}: {}", peer_id, err);
							}
						}
					}
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsOwnerToggled(value) => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					state.owner = value;
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsFolderPathChanged { index, path } => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if let Some(folder) = state.folders.get_mut(index) {
						folder.path = path;
					}
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsFolderReadToggled { index, value } => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if let Some(folder) = state.folders.get_mut(index) {
						folder.read = value;
					}
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsFolderWriteToggled { index, value } => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if let Some(folder) = state.folders.get_mut(index) {
						folder.write = value;
					}
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsFolderRemoved(index) => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if index < state.folders.len() {
						state.folders.remove(index);
					}
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsAddFolder => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					state.folders.push(EditableFolderPermission {
						path: String::from("/"),
						read: true,
						write: false,
					});
					state.error = None;
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsSave => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					match state.build_permissions() {
						Ok(permissions) => {
							let peer_id = state.peer_id.clone();
							state.saving = true;
							state.error = None;
							self.status = format!("Saving permissions for {}...", peer_id);
							let peer = self.peer.clone();
							return Command::perform(
								set_permissions(peer, peer_id.clone(), permissions),
								|(peer_id, result)| GuiMessage::PeerPermissionsSaved {
									peer_id,
									result,
								},
							);
						}
						Err(err) => {
							state.error = Some(err.clone());
							self.status = format!("Failed to prepare permissions: {}", err);
						}
					}
				}
				Command::none()
			}
			GuiMessage::PeerPermissionsSaved { peer_id, result } => {
				if let Mode::PeerPermissions(state) = &mut self.mode {
					if state.peer_id == peer_id {
						state.saving = false;
						match result {
							Ok(perms) => {
								*state =
									PeerPermissionsState::from_permissions(peer_id.clone(), perms);
								self.status = format!("Permissions updated for {}", peer_id);
							}
							Err(err) => {
								state.error = Some(err.clone());
								self.status =
									format!("Failed to save permissions for {}: {}", peer_id, err);
							}
						}
					}
				}
				Command::none()
			}
			GuiMessage::CpuRequested(peer_id) => {
				self.status = format!("Loading CPU info for {}...", peer_id);
				let peer = self.peer.clone();
				Command::perform(fetch_cpus(peer, peer_id.clone()), move |(id, result)| {
					GuiMessage::CpuLoaded(id, result)
				})
			}
			GuiMessage::CpuLoaded(peer_id, result) => {
				match result {
					Ok(cpus) => {
						self.status = cpu_summary(&cpus);
						self.mode = Mode::PeerCpus(PeerCpuState { peer_id, cpus });
					}
					Err(err) => {
						self.status = format!("Failed to load CPU info: {}", err);
						self.mode = Mode::Peers;
					}
				}
				Command::none()
			}
			GuiMessage::InterfacesRequested(peer_id) => {
				self.status = format!("Loading interfaces for {}...", peer_id);
				self.selected_peer_id = Some(peer_id.clone());
				self.mode = Mode::PeerInterfaces(PeerInterfacesState::loading(peer_id.clone()));
				let peer = self.peer.clone();
				return Command::perform(
					fetch_interfaces(peer, peer_id.clone()),
					|(peer_id, interfaces)| GuiMessage::InterfacesLoaded {
						peer_id,
						interfaces,
					},
				);
			}
			GuiMessage::InterfacesLoaded {
				peer_id,
				interfaces,
			} => {
				if let Mode::PeerInterfaces(state) = &mut self.mode {
					if state.peer_id == peer_id {
						state.loading = false;
						match interfaces {
							Ok(list) => {
								state.interfaces = list;
								state.error = None;
								self.status = format!("Interfaces loaded for {}", peer_id);
							}
							Err(err) => {
								state.error = Some(err.clone());
								self.status = format!("Failed to load interfaces: {}", err);
							}
						}
					}
				}
				Command::none()
			}
			GuiMessage::FileBrowserRequested { peer_id } => {
				let use_disks = should_list_disks_first();
				self.status = if use_disks {
					format!("Fetching disks for {}...", peer_id)
				} else {
					format!("Fetching shared folders for {}...", peer_id)
				};
				self.mode =
					Mode::FileBrowser(FileBrowserState::new(peer_id.clone(), String::new()));
				self.selected_peer_id = Some(peer_id.clone());
				if let Mode::FileBrowser(state) = &mut self.mode {
					state.entries.clear();
					state.loading = true;
					state.error = None;
					state.available_roots.clear();
					state.disks.clear();
					state.showing_disks = use_disks;
					if use_disks {
						state.path.clear();
					}
				}
				if use_disks {
					let peer = self.peer.clone();
					return Command::perform(
						list_disks(peer, peer_id.clone()),
						|(peer_id, disks)| GuiMessage::FileBrowserDisksLoaded { peer_id, disks },
					);
				}
				let peer = self.peer.clone();
				Command::perform(
					list_permissions(peer, peer_id.clone()),
					|(peer_id, permissions)| GuiMessage::FileBrowserPermissionsLoaded {
						peer_id,
						permissions,
					},
				)
			}
			GuiMessage::FileBrowserDisksLoaded { peer_id, disks } => {
				if let Mode::FileBrowser(state) = &mut self.mode {
					if state.peer_id == peer_id {
						state.loading = false;
						match disks {
							Ok(list) => {
								state.disks = list;
								state.entries.clear();
								state.error = None;
								self.status = format!("Select a disk on {}", peer_id);
							}
							Err(err) => {
								state.disks.clear();
								state.error = Some(err.clone());
								self.status = format!("Failed to load disks: {}", err);
							}
						}
					}
				}
				Command::none()
			}
			GuiMessage::FileBrowserDiskSelected { peer_id, disk_path } => {
				if let Mode::FileBrowser(state) = &mut self.mode {
					if state.peer_id == peer_id {
						state.showing_disks = false;
						state.path = disk_path.clone();
						state.available_roots = vec![normalize_path(&state.path)];
						state.entries.clear();
						state.loading = true;
						state.error = None;
						self.status = format!("Listing {} on {}...", disk_path, peer_id);
						let peer = self.peer.clone();
						return Command::perform(
							list_dir(peer, peer_id.clone(), disk_path),
							|(peer_id, path, entries)| GuiMessage::FileBrowserLoaded {
								peer_id,
								path,
								entries,
							},
						);
					}
				}
				Command::none()
			}
			GuiMessage::FileBrowserPermissionsLoaded {
				peer_id,
				permissions,
			} => {
				if should_list_disks_first() {
					return Command::none();
				}
				match permissions {
					Ok(perms) => {
						let default_path =
							default_browser_path(&perms).unwrap_or_else(|| String::from("/"));
						let list_path = default_path.clone();
						let status_path = default_path.clone();
						let roots = permissions_roots(&perms);
						self.status = format!("Listing {} on {}...", status_path, peer_id);
						match &mut self.mode {
							Mode::FileBrowser(state) if state.peer_id == peer_id => {
								state.path = default_path.clone();
								state.available_roots = roots.clone();
								state.entries.clear();
								state.loading = true;
								state.error = None;
							}
							_ => {
								let mut state =
									FileBrowserState::new(peer_id.clone(), default_path.clone());
								state.available_roots = roots.clone();
								self.mode = Mode::FileBrowser(state);
							}
						}
						let peer = self.peer.clone();
						return Command::perform(
							list_dir(peer, peer_id.clone(), list_path),
							|(peer_id, path, entries)| GuiMessage::FileBrowserLoaded {
								peer_id,
								path,
								entries,
							},
						);
					}
					Err(err) => {
						self.status = format!("Failed to fetch permissions: {}", err);
						if let Mode::FileBrowser(state) = &mut self.mode {
							if state.peer_id == peer_id {
								state.loading = false;
								state.error = Some(err.clone());
							}
						}
						return Command::none();
					}
				}
			}
			GuiMessage::FileBrowserLoaded {
				peer_id,
				path,
				entries,
			} => {
				match &mut self.mode {
					Mode::FileBrowser(state) if state.peer_id == peer_id => {
						state.path = path.clone();
						state.loading = false;
						state.thumbnails.clear();
						match entries {
							Ok(entries) => {
								// Collect image entries that need thumbnails
								let mut thumbnail_commands = Vec::new();
								for entry in &entries {
									if !entry.is_dir && FileBrowserState::is_image_entry(entry) {
										let full_path = join_child_path(&path, &entry.name);
										state.thumbnails.insert(full_path.clone(), ThumbnailState::Loading);
										let peer = self.peer.clone();
										let p_id = peer_id.clone();
										thumbnail_commands.push(Command::perform(
											fetch_thumbnail(peer, p_id, full_path.clone()),
											|(path, result)| GuiMessage::ThumbnailLoaded { path, result },
										));
									}
								}
								state.entries = entries;
								state.error = None;
								self.status = format!("Loaded {} entries", state.entries.len());
								if !thumbnail_commands.is_empty() {
									return Command::batch(thumbnail_commands);
								}
							}
							Err(err) => {
								state.entries.clear();
								state.error = Some(err.clone());
								self.status = format!("Failed to load directory: {}", err);
							}
						}
					}
					_ => {}
				}
				Command::none()
			}
			GuiMessage::FileEntryActivated(entry) => {
				if let Mode::FileBrowser(state) = &mut self.mode {
					if state.showing_disks {
						return Command::none();
					}
					if entry.is_dir {
						let target = join_child_path(&state.path, &entry.name);
						let peer_id = state.peer_id.clone();
						state.path = target.clone();
						state.entries.clear();
						state.loading = true;
						state.error = None;
						self.status = format!("Opening {}...", target);
						let peer = self.peer.clone();
						return Command::perform(
							list_dir(peer, peer_id.clone(), target),
							|(peer_id, path, entries)| GuiMessage::FileBrowserLoaded {
								peer_id,
								path,
								entries,
							},
						);
					}
					let target = join_child_path(&state.path, &entry.name);
					let peer_id = state.peer_id.clone();
					let browser_snapshot = state.clone();
					let mime_label = entry.mime.clone().unwrap_or_else(|| String::from("?"));
					self.status = format!(
						"Reading {} ({} | {})",
						target,
						format_size(entry.size),
						mime_label
					);
					let peer = self.peer.clone();
					let command = Command::perform(
						read_file(peer, peer_id.clone(), target.clone(), 0),
						|(peer_id, path, offset, result)| GuiMessage::FileReadLoaded {
							peer_id,
							path,
							offset,
							result,
						},
					);
					self.mode = Mode::FileViewer(FileViewerState::from_browser(
						browser_snapshot,
						peer_id,
						target,
						entry.mime.clone(),
					));
					return command;
				}
				Command::none()
			}
			GuiMessage::FileNavigateUp => {
				if let Mode::FileBrowser(state) = &mut self.mode {
					if state.showing_disks {
						self.status = String::from("Select a disk to browse");
						return Command::none();
					}
					let current = normalize_path(&state.path);
					if state.available_roots.iter().any(|root| root == &current) {
						self.status = String::from("Already at shared root");
						return Command::none();
					}
					let target = parent_path(&state.path);
					if target == state.path {
						self.status = String::from("Already at root");
						return Command::none();
					}
					let peer_id = state.peer_id.clone();
					state.path = target.clone();
					state.entries.clear();
					state.loading = true;
					state.error = None;
					self.status = format!("Opening {}...", target);
					let peer = self.peer.clone();
					return Command::perform(
						list_dir(peer, peer_id.clone(), target),
						|(peer_id, path, entries)| GuiMessage::FileBrowserLoaded {
							peer_id,
							path,
							entries,
						},
					);
				}
				Command::none()
			}
			GuiMessage::FileReadLoaded {
				peer_id,
				path,
				offset: _,
				result,
			} => {
				let mut next_command = Command::none();
				match &mut self.mode {
					Mode::FileViewer(state) if state.peer_id == peer_id && state.path == path => {
						state.loading = false;
						match result {
							Ok(chunk) => {
								let prev_offset = state.offset;
								let chunk_len = chunk.data.len();
								state.error = None;
								state.apply_chunk(chunk);
								let mime_label =
									state.mime.clone().unwrap_or_else(|| String::from("?"));
								let base_status = if state.is_image()
									&& state.eof && !state.data.is_empty()
								{
									format!(
										"Image loaded: {} bytes | {}",
										state.data.len(),
										mime_label
									)
								} else {
									let eof_note = if state.eof { " (end of file)" } else { "" };
									format!(
										"Loaded {} bytes{} | {}",
										state.data.len(),
										eof_note,
										mime_label
									)
								};
								let progressed = state.offset > prev_offset;
								if state.eof {
									self.status = base_status;
								} else if progressed {
									self.status = format!("{}; fetching more...", base_status);
									state.loading = true;
									let peer_id = state.peer_id.clone();
									let path = state.path.clone();
									let offset = state.offset;
									let peer = self.peer.clone();
									next_command = Command::perform(
										read_file(peer, peer_id.clone(), path.clone(), offset),
										|(peer_id, path, offset, result)| {
											GuiMessage::FileReadLoaded {
												peer_id,
												path,
												offset,
												result,
											}
										},
									);
								} else {
									// No progress in this chunk; leave loading stopped for manual retry.
									self.status = format!(
										"{}; waiting for more data at offset {} (received {} bytes)",
										base_status, state.offset, chunk_len,
									);
								}
							}
							Err(err) => {
								state.error = Some(err.clone());
								self.status = format!("Failed to load file chunk: {}", err);
							}
						}
					}
					_ => {}
				}
				next_command
			}
			GuiMessage::FileReadMore => {
				if let Mode::FileViewer(state) = &mut self.mode {
					if state.loading {
						return Command::none();
					}
					if state.eof {
						self.status = String::from("Already at end of file");
						return Command::none();
					}
					state.loading = true;
					let peer_id = state.peer_id.clone();
					let path = state.path.clone();
					let offset = state.offset;
					self.status = format!("Loading bytes starting at {}...", offset);
					let peer = self.peer.clone();
					return Command::perform(
						read_file(peer, peer_id, path.clone(), offset),
						|(peer_id, path, offset, result)| GuiMessage::FileReadLoaded {
							peer_id,
							path,
							offset,
							result,
						},
					);
				}
				Command::none()
			}
			GuiMessage::FileViewerBack => {
				if let Mode::FileViewer(state) = mem::replace(&mut self.mode, Mode::Peers) {
					match state.source {
						FileViewerSource::FileBrowser(browser) => {
							self.status = format!("Browsing {} on {}", browser.path, browser.peer_id);
							self.mode = Mode::FileBrowser(browser);
						}
						FileViewerSource::StorageUsage(storage) => {
							self.status = String::from("Storage usage");
							self.mode = Mode::StorageUsage(storage);
						}
						FileViewerSource::Files(files) => {
							self.status = String::from("Files");
							let scroll_offset = files.scroll_offset;
							self.mode = Mode::FileSearch(files);
							return scrollable::snap_to(
								scrollable::Id::new("files_table"),
								scroll_offset,
							);
						}
					}
				}
				Command::none()
			}
			GuiMessage::GraphNext => {
				self.graph.next();
				if let Some(id) = self.graph.selected_id() {
					self.selected_peer_id = Some(id.to_string());
					self.status = format!("Graph focus: {}", id);
				}
				Command::none()
			}
			GuiMessage::GraphPrev => {
				self.graph.previous();
				if let Some(id) = self.graph.selected_id() {
					self.selected_peer_id = Some(id.to_string());
					self.status = format!("Graph focus: {}", id);
				}
				Command::none()
			}
			GuiMessage::UsernameChanged(value) => {
				if let Mode::CreateUser(form) = &mut self.mode {
					form.username = value;
				}
				Command::none()
			}
			GuiMessage::PasswordChanged(value) => {
				if let Mode::CreateUser(form) = &mut self.mode {
					form.password = value;
				}
				Command::none()
			}
			GuiMessage::CreateUserSubmit => {
				if let Mode::CreateUser(form) = &mut self.mode {
					if form.username.trim().is_empty() || form.password.trim().is_empty() {
						form.status = Some(String::from("Both fields are required"));
					} else {
						self.status = format!("Created user '{}' (placeholder)", form.username);
						form.status = Some(self.status.clone());
						form.password.clear();
					}
				}
				Command::none()
			}
			GuiMessage::FilesViewModeChanged(mode) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.view_mode = mode;
				}
				Command::none()
			}
			GuiMessage::FilesNameQueryChanged(q) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.name_query = q;
				}
				Command::none()
			}
			GuiMessage::FilesContentQueryChanged(q) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.content_query = q;
				}
				Command::none()
			}
			GuiMessage::FilesDateFromChanged(d) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.date_from = d;
				}
				Command::none()
			}
			GuiMessage::FilesDateToChanged(d) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.date_to = d;
				}
				Command::none()
			}
			GuiMessage::FileSearchMimeChanged(m) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.selected_mime = m;
				}
				Command::none()
			}
			GuiMessage::FileSearchToggleSort => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.sort_desc = !state.sort_desc;
				}
				Command::none()
			}
			GuiMessage::FileSearchExecute => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.loading = true;
					state.error = None;
					state.results.clear();
					state.page = 0; // Reset to first page on new search
					let name_query = state.name_query.clone();
					let content_query = state.content_query.clone();
					let date_from = state.date_from.clone();
					let date_to = state.date_to.clone();
					let mime = if state.selected_mime.trim().is_empty() {
						None
					} else {
						Some(state.selected_mime.clone())
					};
					let sort_desc = state.sort_desc;
					let page = state.page;
					let page_size = state.page_size;
					let peer = self.peer.clone();
					return Command::perform(
						search_files(peer, name_query, content_query, date_from, date_to, mime, sort_desc, page, page_size),
						GuiMessage::FileSearchLoaded,
					);
				}
				Command::none()
			}
			GuiMessage::FileSearchLoaded(result) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.loading = false;
					match result {
						Ok((entries, mimes, total)) => {
							state.results = entries;
							state.available_mime_types = mimes;
							state.total_count = total;
							let start = state.page * state.page_size + 1;
							let end = (start + state.results.len()).saturating_sub(1);
							self.status = format!("Showing {}-{} of {} files", start, end, total);
						}
						Err(err) => {
							state.error = Some(err.clone());
							self.status = format!("Search failed: {}", err);
						}
					}
				}
				Command::none()
			}
			GuiMessage::FilesNextPage => {
				if let Mode::FileSearch(state) = &mut self.mode {
					let max_page = state.total_count.saturating_sub(1) / state.page_size;
					if state.page < max_page {
						state.page += 1;
						state.loading = true;
						state.error = None;
						let name_query = state.name_query.clone();
						let content_query = state.content_query.clone();
						let date_from = state.date_from.clone();
						let date_to = state.date_to.clone();
						let mime = if state.selected_mime.trim().is_empty() {
							None
						} else {
							Some(state.selected_mime.clone())
						};
						let sort_desc = state.sort_desc;
						let page = state.page;
						let page_size = state.page_size;
						let peer = self.peer.clone();
						return Command::perform(
							search_files(peer, name_query, content_query, date_from, date_to, mime, sort_desc, page, page_size),
							GuiMessage::FileSearchLoaded,
						);
					}
				}
				Command::none()
			}
			GuiMessage::FilesPrevPage => {
				if let Mode::FileSearch(state) = &mut self.mode {
					if state.page > 0 {
						state.page -= 1;
						state.loading = true;
						state.error = None;
						let name_query = state.name_query.clone();
						let content_query = state.content_query.clone();
						let date_from = state.date_from.clone();
						let date_to = state.date_to.clone();
						let mime = if state.selected_mime.trim().is_empty() {
							None
						} else {
							Some(state.selected_mime.clone())
						};
						let sort_desc = state.sort_desc;
						let page = state.page;
						let page_size = state.page_size;
						let peer = self.peer.clone();
						return Command::perform(
							search_files(peer, name_query, content_query, date_from, date_to, mime, sort_desc, page, page_size),
							GuiMessage::FileSearchLoaded,
						);
					}
				}
				Command::none()
			}
			GuiMessage::FilesOpenFile { node_id: _, path, mime } => {
				if let Mode::FileSearch(state) = &self.mode {
					// Use local peer ID for reading local files
					let peer_id = match &self.local_peer_id {
						Some(id) => id.clone(),
						None => {
							self.status = String::from("Error: Local peer ID not available");
							return Command::none();
						}
					};
					let files_snapshot = state.clone();
					self.status = format!("Reading {}...", path);
					let peer = self.peer.clone();
					let command = Command::perform(
						read_file(peer, peer_id.clone(), path.clone(), 0),
						|(peer_id, path, offset, result)| GuiMessage::FileReadLoaded {
							peer_id,
							path,
							offset,
							result,
						},
					);
					self.mode = Mode::FileViewer(FileViewerState::from_files(
						files_snapshot,
						peer_id,
						path,
						mime,
					));
					return command;
				}
				Command::none()
			}
			GuiMessage::FilesMimeTypesLoaded(result) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					match result {
						Ok(mimes) => {
							state.available_mime_types = mimes;
							self.status = String::from("Files");
						}
						Err(err) => {
							self.status = format!("Failed to load mime types: {}", err);
						}
					}
				}
				Command::none()
			}
			GuiMessage::FilesScrolled(viewport) => {
				if let Mode::FileSearch(state) = &mut self.mode {
					state.scroll_offset = viewport.relative_offset();
				}
				Command::none()
			}
			GuiMessage::ScanPathChanged(path) => {
				self.scan_state.path = path;
				Command::none()
			}
			GuiMessage::ScanRequested => {
				let state = &mut self.scan_state;
				if state.scanning {
					return Command::none();
				}
				let requested = state.path.trim().to_string();
				if requested.is_empty() {
					state.error = Some(String::from("Scan path cannot be empty"));
					return Command::none();
				}
				state.scanning = true;
				state.error = None;
				state.status = Some(format!("Scanning {}...", requested));
				state.processed_files = 0;
				state.total_files = 0;
				self.status = format!("Scanning {}...", requested);
				let scan_id = self.next_scan_id;
				self.next_scan_id += 1;
				let receiver = match self.peer.scan_folder(requested.clone()) {
					Ok(receiver) => receiver,
					Err(err) => {
						state.scanning = false;
						state.error = Some(err);
						return Command::none();
					}
				};
				self.active_scan = Some(ActiveScan {
					id: scan_id,
					receiver: receiver.clone(),
				});
				Command::perform(wait_for_scan_event(receiver), move |event| {
					GuiMessage::ScanEventReceived { id: scan_id, event }
				})
			}
			GuiMessage::ScanEventReceived { id, event } => {
				if self.active_scan.as_ref().map(|scan| scan.id) != Some(id) {
					return Command::none();
				}
				let mut should_poll = false;
				match event {
					ScanEvent::Progress(progress) => {
						let state = &mut self.scan_state;
						state.total_files = progress.total_files;
						state.processed_files = progress.processed_files;
						let status = if progress.total_files > 0 {
							format!(
								"Scanning {}... {}/{} files ({} inserted, {} updated, {} removed)",
								state.path,
								progress.processed_files,
								progress.total_files,
								progress.inserted_count,
								progress.updated_count,
								progress.removed_count
							)
						} else {
							format!("Scanning {}... collecting entries", state.path)
						};
						state.status = Some(status.clone());
						state.error = None;
						self.status = status;
						should_poll = true;
					}
					ScanEvent::Finished(result) => {
						let state = &mut self.scan_state;
						state.scanning = false;
						if state.total_files == 0 {
							state.total_files = state.processed_files;
						}
						match result {
							Ok(stats) => {
								let processed = state.processed_files;
								let summary = format!(
									"Scan finished: {} inserted, {} updated, {} removed after scanning {} files ({:.2}s)",
									stats.inserted_count,
									stats.updated_count,
									stats.removed_count,
									processed,
									stats.duration.as_secs_f32()
								);
								state.status = Some(summary.clone());
								state.error = None;
								self.status = summary;
							}
							Err(err) => {
								state.error = Some(err.clone());
								self.status = format!("Scan failed: {}", err);
							}
						}
						self.active_scan = None;
					}
				}
				if should_poll {
					if let Some(active) = &self.active_scan {
						let receiver = active.receiver.clone();
						return Command::perform(wait_for_scan_event(receiver), move |event| {
							GuiMessage::ScanEventReceived { id, event }
						});
					}
				}
				Command::none()
			}
			GuiMessage::ScanResultsNextPage => {
				if let Mode::ScanResults(state) = &mut self.mode {
					if state.loading {
						return Command::none();
					}
					let next_page = state.page + 1;
					if next_page * state.page_size >= state.total_entries
						&& state.total_entries != 0
					{
						return Command::none();
					}
					state.page = next_page;
					state.loading = true;
					state.error = None;
					let page_size = state.page_size;
					self.status = format!("Loading scan results (page {})...", next_page + 1);
					let peer = self.peer.clone();
					return Command::perform(
						load_scan_results_page(peer, next_page, page_size),
						move |result| GuiMessage::ScanResultsLoaded {
							page: next_page,
							result,
						},
					);
				}
				Command::none()
			}
			GuiMessage::ScanResultsPrevPage => {
				if let Mode::ScanResults(state) = &mut self.mode {
					if state.loading || state.page == 0 {
						return Command::none();
					}
					let prev_page = state.page - 1;
					state.page = prev_page;
					state.loading = true;
					state.error = None;
					let page_size = state.page_size;
					self.status = format!("Loading scan results (page {})...", prev_page + 1);
					let peer = self.peer.clone();
					return Command::perform(
						load_scan_results_page(peer, prev_page, page_size),
						move |result| GuiMessage::ScanResultsLoaded {
							page: prev_page,
							result,
						},
					);
				}
				Command::none()
			}
			GuiMessage::ScanResultsLoaded { page, result } => {
				if let Mode::ScanResults(state) = &mut self.mode {
					state.loading = false;
					match result {
						Ok((entries, total)) => {
							state.entries = entries;
							state.total_entries = total;
							state.page = page;
							state.error = None;
							self.status = format!(
								"Loaded page {} of scan results ({} files total)",
								state.page + 1,
								total
							);
						}
						Err(err) => {
							state.error = Some(err.clone());
							self.status = format!("Failed to load scan results: {}", err);
						}
					}
				}
				Command::none()
			}
			GuiMessage::StorageUsageLoaded(result) => {
				if let Mode::StorageUsage(state) = &mut self.mode {
					state.loading = false;
					match result {
						Ok(nodes) => {
							state.nodes = nodes;
							state.error = None;
							self.status = String::from("Storage usage loaded");
						}
						Err(err) => {
							state.error = Some(err.clone());
							self.status = format!("Failed to load storage usage: {}", err);
						}
					}
				}
				Command::none()
			}
			GuiMessage::StorageUsageToggleNode(index) => {
				self.toggle_storage_node(index);
				Command::none()
			}
			GuiMessage::StorageUsageToggleEntry { node_index, path } => {
				self.toggle_storage_entry(node_index, &path);
				Command::none()
			}
			GuiMessage::StorageUsageOpenFile { node_id, path } => {
				if let Mode::StorageUsage(state) = &self.mode {
					let storage_snapshot = state.clone();
					self.status = format!("Reading {}...", path);
					let peer = self.peer.clone();
					let command = Command::perform(
						read_file(peer, node_id.clone(), path.clone(), 0),
						|(peer_id, path, offset, result)| GuiMessage::FileReadLoaded {
							peer_id,
							path,
							offset,
							result,
						},
					);
					self.mode = Mode::FileViewer(FileViewerState::from_storage(
						storage_snapshot,
						node_id,
						path,
					));
					return command;
				}
				Command::none()
			}
			GuiMessage::InterfacesFieldEdited => Command::none(),
			GuiMessage::ThumbnailLoaded { path, result } => {
				if let Mode::FileBrowser(state) = &mut self.mode {
					match result {
						Ok(thumb) => {
							state.thumbnails.insert(path, ThumbnailState::Loaded(thumb.data));
						}
						Err(_) => {
							state.thumbnails.insert(path, ThumbnailState::Failed);
						}
					}
				}
				Command::none()
			}
			GuiMessage::UpdatePeerRequested(peer_id) => {
				if self.active_update.is_some() {
					self.status = String::from("An update is already in progress");
					return Command::none();
				}
				let target = match PeerId::from_str(&peer_id) {
					Ok(id) => id,
					Err(err) => {
						self.status = format!("Invalid peer id: {}", err);
						return Command::none();
					}
				};
				let receiver = match self.peer.update_remote_peer(target, None) {
					Ok(recv) => recv,
					Err(err) => {
						self.status = format!("Failed to start update: {}", err);
						return Command::none();
					}
				};
				let update_id = self.next_update_id;
				self.next_update_id += 1;
				self.active_update = Some(ActiveUpdate {
					id: update_id,
					peer_id: peer_id.clone(),
					receiver: receiver.clone(),
				});
				self.status = format!("Starting update for peer {}...", peer_id);
				Command::perform(wait_for_update_event(receiver), move |event| {
					GuiMessage::UpdatePeerProgress {
						peer_id: peer_id.clone(),
						event,
					}
				})
			}
			GuiMessage::UpdatePeerProgress { peer_id, event } => {
				if self.active_update.as_ref().map(|u| &u.peer_id) != Some(&peer_id) {
					return Command::none();
				}
				let mut should_poll = false;
				match &event {
					UpdateProgress::FetchingRelease => {
						self.status = format!("Fetching release info for {}...", peer_id);
						should_poll = true;
					}
					UpdateProgress::Downloading { filename } => {
						self.status = format!("Downloading {} for {}...", filename, peer_id);
						should_poll = true;
					}
					UpdateProgress::Unpacking => {
						self.status = format!("Unpacking update for {}...", peer_id);
						should_poll = true;
					}
					UpdateProgress::Verifying => {
						self.status = format!("Verifying update for {}...", peer_id);
						should_poll = true;
					}
					UpdateProgress::Installing => {
						self.status = format!("Installing update for {}...", peer_id);
						should_poll = true;
					}
					UpdateProgress::Completed { version } => {
						self.status = format!("Peer {} updated to version {}", peer_id, version);
						self.active_update = None;
					}
					UpdateProgress::Failed { error } => {
						self.status = format!("Update failed for {}: {}", peer_id, error);
						self.active_update = None;
					}
					UpdateProgress::AlreadyUpToDate { current_version } => {
						self.status = format!("Peer {} is already up to date (version {})", peer_id, current_version);
						self.active_update = None;
					}
				}
				if should_poll {
					if let Some(update) = &self.active_update {
						let receiver = update.receiver.clone();
						let p_id = peer_id.clone();
						return Command::perform(wait_for_update_event(receiver), move |event| {
							GuiMessage::UpdatePeerProgress {
								peer_id: p_id,
								event,
							}
						});
					}
				}
				Command::none()
			}
		}
	}

	fn view(&self) -> Element<'_, Self::Message> {
		let mut menu_column = iced::widget::Column::new().spacing(8);
		for item in MENU_ITEMS.iter() {
			let mut label = item.label().to_string();
			if self.menu == *item {
				label = format!("▶ {}", label);
			}
			let button = button(text(label).size(16))
				// .width(Length::Fill)
				.on_press(GuiMessage::MenuSelected(*item));
			menu_column = menu_column.push(button);
		}
		let sidebar = container(menu_column)
			.width(Length::Shrink)
			.padding(16)
			.style(theme::Container::Box);
		let content: Element<_> = match &self.mode {
			Mode::Peers => self.view_peers(),
			Mode::PeerActions { peer_id } => self.view_peer_actions(peer_id),
			Mode::PeerPermissions(state) => self.view_peer_permissions(state),
			Mode::PeerCpus(state) => self.view_peer_cpus(state),
			Mode::StorageUsage(state) => self.view_storage_usage(state),
			Mode::PeerInterfaces(state) => self.view_peer_interfaces(state),
			Mode::FileBrowser(state) => self.view_file_browser(state),
			Mode::FileViewer(state) => self.view_file_viewer(state),
			Mode::PeersGraph => self.view_graph(),
			Mode::CreateUser(form) => self.view_create_user(form),
			Mode::FileSearch(state) => self.view_file_search(state),
			Mode::ScanResults(state) => self.view_scan_results(state),
		};
		let content_container = container(content)
			.width(Length::Fill)
			.height(Length::Fill)
			.padding(16)
			.style(theme::Container::Box);
		let main = iced::widget::Row::new()
			.spacing(16)
			.push(sidebar)
			.push(content_container)
			.height(Length::Fill);
		let status = container(text(&self.status).size(16))
			.width(Length::Fill)
			.padding(12)
			.style(theme::Container::Box);
		iced::widget::Column::new()
			.spacing(12)
			.padding(12)
			.push(main)
			.push(status)
			.into()
	}
}

impl GuiApp {
	fn refresh_from_state(&mut self) {
		if let Ok(state_guard) = self.peer.state().lock() {
			let snapshot = state_guard.clone();
			self.local_peer_id = Some(snapshot.me.to_string());
			self.peers = aggregate_peers(&snapshot);
			if self
				.selected_peer_id
				.clone()
				.filter(|id| !self.peers.iter().any(|p| p.id == *id))
				.is_some()
			{
				self.selected_peer_id = None;
			}
			let missing_peer = match &self.mode {
				Mode::PeerActions { peer_id } => {
					if !self.peers.iter().any(|p| p.id == *peer_id) {
						Some(peer_id.clone())
					} else {
						None
					}
				}
				Mode::PeerPermissions(state) => {
					if !self.peers.iter().any(|p| p.id == state.peer_id) {
						Some(state.peer_id.clone())
					} else {
						None
					}
				}
				_ => None,
			};
			if let Some(peer_id) = missing_peer {
				self.mode = Mode::Peers;
				self.status = format!("Peer {} not available", peer_id);
			}
			self.graph.set_peers(&self.peers);
			if let Some(idx) = self.selected_peer_id.as_ref().and_then(|selected| {
				self.graph
					.nodes
					.iter()
					.position(|node| &node.id == selected)
			}) {
				self.graph.selected = idx;
			}
			self.latest_state = Some(snapshot);
		} else {
			self.status = String::from("Waiting for peer state");
		}
	}

	fn view_peers(&self) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text("Discovered Peers").size(24));
		if self.peers.is_empty() {
			layout = layout.push(text("No peers discovered yet.").size(16));
		} else {
			let mut list = iced::widget::Column::new().spacing(4);
			for peer in &self.peers {
				let indicator = if self.selected_peer_id.as_deref() == Some(peer.id.as_str()) {
					"▶"
				} else {
					""
				};
				let id_label = format!("{} {}", indicator, abbreviate_peer_id(&peer.id));
				let id_cell = container(
					tooltip(
						text(id_label).size(16),
						text(peer.id.clone()),
						tooltip::Position::FollowCursor,
					)
					.style(theme::Container::Box),
				)
				.width(Length::FillPortion(2));
				let info = iced::widget::Row::new()
					.spacing(12)
					.push(id_cell)
					.push(
						text(peer.address.clone())
							.size(14)
							.width(Length::FillPortion(3)),
					)
					.push(
						text(peer.status.clone())
							.size(14)
							.width(Length::FillPortion(1)),
					)
					.push(
						button(text("Actions"))
							.on_press(GuiMessage::PeerActionsRequested(peer.id.clone())),
					);
				let card = container(info).padding(8).style(theme::Container::Box);
				list = list.push(card);
			}
			layout = layout.push(scrollable(list).height(Length::Fill));
		}
		layout.into()
	}

	fn view_peer_actions(&self, peer_id: &str) -> Element<'_, GuiMessage> {
		if let Some(peer) = self.peers.iter().find(|row| row.id == peer_id) {
			let mut layout = iced::widget::Column::new().spacing(12);
			layout = layout.push(text(format!("Peer {}", peer.id)).size(24));
			layout = layout.push(text(format!("Status: {}", peer.status)).size(16));
			if !peer.address.is_empty() {
				layout = layout.push(text(format!("Dial address: {}", peer.address)).size(16));
			}
			let addresses = self.gather_known_addresses(peer_id);
			if !addresses.is_empty() {
				let mut addr_box = iced::widget::Column::new().spacing(4);
				for addr in addresses {
					addr_box = addr_box.push(text(addr).size(14));
				}
				layout = layout.push(container(addr_box).padding(8).style(theme::Container::Box));
			}
			let controls = iced::widget::Row::new()
				.spacing(12)
				.push(button(text("CPU info")).on_press(GuiMessage::CpuRequested(peer.id.clone())))
				.push(
					button(text("Interfaces"))
						.on_press(GuiMessage::InterfacesRequested(peer.id.clone())),
				)
				.push(
					button(text("File browser")).on_press(GuiMessage::FileBrowserRequested {
						peer_id: peer.id.clone(),
					}),
				)
				.push(
					button(text("Permissions"))
						.on_press(GuiMessage::PeerPermissionsRequested(peer.id.clone())),
				)
				.push(
					button(text("Update Peer"))
						.on_press(GuiMessage::UpdatePeerRequested(peer.id.clone())),
				)
				.push(button(text("Back")).on_press(GuiMessage::BackToPeers));
			layout = layout.push(controls);
			layout = layout.push(
				container(self.view_scan_controls())
					.padding(8)
					.style(theme::Container::Box),
			);
			layout.into()
		} else {
			container(text("Selected peer not available").size(16))
				.align_x(Horizontal::Center)
				.align_y(Vertical::Center)
				.width(Length::Fill)
				.height(Length::Fill)
				.into()
		}
	}

	fn view_peer_permissions(&self, state: &PeerPermissionsState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text(format!("Permissions for {}", state.peer_id)).size(24));
		if state.loading {
			layout = layout.push(text("Loading permissions...").size(16));
			return layout.into();
		}
		if let Some(err) = &state.error {
			layout = layout.push(text(format!("Error: {}", err)).size(14));
		}
		let owner_toggle = checkbox("Grant owner access (full control)", state.owner)
			.on_toggle(GuiMessage::PeerPermissionsOwnerToggled);
		layout = layout.push(owner_toggle);
		let saving = state.saving;
		let mut folders_column = iced::widget::Column::new().spacing(8);
		if state.folders.is_empty() {
			folders_column = folders_column.push(text("No folder permissions granted.").size(14));
		} else {
			for (idx, folder) in state.folders.iter().enumerate() {
				let path_input = text_input("Shared folder path", &folder.path)
					.padding(8)
					.size(16)
					.on_input({
						let index = idx;
						move |value| GuiMessage::PeerPermissionsFolderPathChanged {
							index,
							path: value,
						}
					});
				let read_toggle = checkbox("Read", folder.read).on_toggle({
					let index = idx;
					move |value| GuiMessage::PeerPermissionsFolderReadToggled { index, value }
				});
				let write_toggle = checkbox("Write", folder.write).on_toggle({
					let index = idx;
					move |value| GuiMessage::PeerPermissionsFolderWriteToggled { index, value }
				});
				let mut remove_button = button(text("Remove"));
				if !saving {
					let index = idx;
					remove_button =
						remove_button.on_press(GuiMessage::PeerPermissionsFolderRemoved(index));
				}
				let toggles = iced::widget::Row::new()
					.spacing(12)
					.push(read_toggle)
					.push(write_toggle)
					.push(remove_button);
				let card = container(
					iced::widget::Column::new()
						.spacing(8)
						.push(path_input)
						.push(toggles),
				)
				.padding(8)
				.style(theme::Container::Box);
				folders_column = folders_column.push(card);
			}
		}
		layout = layout.push(scrollable(folders_column).height(Length::Fill));
		let mut controls = iced::widget::Row::new().spacing(12);
		let mut add_button = button(text("Add folder"));
		if !saving {
			add_button = add_button.on_press(GuiMessage::PeerPermissionsAddFolder);
		}
		controls = controls.push(add_button);
		let mut save_button = button(text(if saving { "Saving..." } else { "Save changes" }));
		if !saving {
			save_button = save_button.on_press(GuiMessage::PeerPermissionsSave);
		}
		controls = controls.push(save_button);
		controls = controls.push(
			button(text("Back to actions"))
				.on_press(GuiMessage::PeerActionsRequested(state.peer_id.clone())),
		);
		layout = layout.push(controls);
		layout.into()
	}

	fn view_peer_cpus(&self, state: &PeerCpuState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text(format!("CPU inventory for {}", state.peer_id)).size(24));
		if state.cpus.is_empty() {
			layout = layout.push(text("No CPU information available.").size(16));
		} else {
			let mut list = iced::widget::Column::new().spacing(4);
			for (idx, cpu) in state.cpus.iter().enumerate() {
				let row = iced::widget::Row::new()
					.spacing(12)
					.push(text(format!("{}", idx)).size(14).width(Length::Shrink))
					.push(
						text(cpu.name.clone())
							.size(14)
							.width(Length::FillPortion(2)),
					)
					.push(
						text(format!("{:.1}%", cpu.usage))
							.size(14)
							.width(Length::FillPortion(1)),
					)
					.push(
						text(format_frequency(cpu.frequency_hz))
							.size(14)
							.width(Length::FillPortion(1)),
					);
				let card = container(row).padding(8).style(theme::Container::Box);
				list = list.push(card);
			}
			layout = layout.push(scrollable(list).height(Length::Fill));
		}
		let controls = iced::widget::Row::new()
			.spacing(12)
			.push(button(text("Refresh")).on_press(GuiMessage::CpuRequested(state.peer_id.clone())))
			.push(
				button(text("Back to actions"))
					.on_press(GuiMessage::PeerActionsRequested(state.peer_id.clone())),
			);
		layout = layout.push(controls);
		layout.into()
	}

	fn view_peer_interfaces(&self, state: &PeerInterfacesState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text(format!("Network interfaces for {}", state.peer_id)).size(24));
		if state.loading {
			return layout
				.push(text("Loading network interfaces...").size(16))
				.into();
		}
		if let Some(err) = &state.error {
			layout = layout.push(text(format!("Error: {}", err)).size(16));
		}
		if state.interfaces.is_empty() {
			layout = layout.push(text("No interface information available.").size(16));
		} else {
			let mut list = iced::widget::Column::new().spacing(4);
			for iface in &state.interfaces {
				let ips = if iface.ips.is_empty() {
					String::from("-")
				} else {
					iface.ips.join(", ")
				};
				let mut fields = iced::widget::Column::new().spacing(6);
				fields = fields
					.push(copyable_interface_field("MAC", iface.mac.clone()))
					.push(copyable_interface_field("IPs", ips))
					.push(copyable_interface_field("MTU", iface.mtu.to_string()))
					.push(copyable_interface_field(
						"Total received",
						format_size(iface.total_received),
					))
					.push(copyable_interface_field(
						"Total transmitted",
						format_size(iface.total_transmitted),
					))
					.push(copyable_interface_field(
						"Packets (rx/tx)",
						format!("{}/{}", iface.packets_received, iface.packets_transmitted),
					))
					.push(copyable_interface_field(
						"Errors (rx/tx)",
						format!(
							"{}/{}",
							iface.errors_on_received, iface.errors_on_transmitted
						),
					));
				let card = iced::widget::Column::new()
					.spacing(6)
					.push(text(format!("Interface {}", iface.name)).size(18))
					.push(Divider::horizontal(0))
					.push(fields);
				list = list.push(container(card).padding(12).style(theme::Container::Box));
			}
			layout = layout.push(scrollable(list).height(Length::Fill));
		}
		let controls = iced::widget::Row::new()
			.spacing(12)
			.push(
				button(text("Refresh"))
					.on_press(GuiMessage::InterfacesRequested(state.peer_id.clone())),
			)
			.push(
				button(text("Back to actions"))
					.on_press(GuiMessage::PeerActionsRequested(state.peer_id.clone())),
			);
		layout = layout.push(controls);
		layout.into()
	}

	fn view_file_browser(&self, state: &FileBrowserState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(
			text(format!(
				"Browsing {} on {}",
				state.display_path(),
				state.peer_id
			))
			.size(24),
		);
		let mut up_button = button(text("Up"));
		if state.showing_disks {
			up_button = up_button.style(theme::Button::Secondary);
		} else {
			up_button = up_button.on_press(GuiMessage::FileNavigateUp);
		}
		let controls = iced::widget::Row::new().spacing(12).push(up_button).push(
			button(text("Back to actions"))
				.on_press(GuiMessage::PeerActionsRequested(state.peer_id.clone())),
		);
		layout = layout.push(controls);
		if state.showing_disks {
			if state.loading {
				layout = layout.push(text("Loading disks...").size(16));
			} else if let Some(err) = &state.error {
				layout = layout.push(text(format!("Error: {}", err)).size(16));
			} else if state.disks.is_empty() {
				layout = layout.push(text("No disks were reported").size(16));
			} else {
				let mut list = iced::widget::Column::new().spacing(4);
				for disk in &state.disks {
					let label = format_disk_label(disk);
					let button = button(text(label)).width(Length::Fill).on_press(
						GuiMessage::FileBrowserDiskSelected {
							peer_id: state.peer_id.clone(),
							disk_path: disk.mount_path.clone(),
						},
					);
					list = list.push(button);
				}
				layout = layout.push(scrollable(list).height(Length::Fill));
			}
		} else {
			if state.loading {
				layout = layout.push(text("Loading directory...").size(16));
			} else if let Some(err) = &state.error {
				layout = layout.push(text(format!("Error: {}", err)).size(16));
			} else if state.entries.is_empty() {
				layout = layout.push(text("Directory is empty").size(16));
			} else {
				let mut list = iced::widget::Column::new().spacing(8);
				for entry in &state.entries {
					let full_path = join_child_path(&state.path, &entry.name);
					let is_image = FileBrowserState::is_image_entry(entry);

					// Create the content row
					let mut row = iced::widget::Row::new().spacing(8).align_items(iced::Alignment::Center);

					// Add thumbnail for images if available
					if is_image {
						match state.thumbnails.get(&full_path) {
							Some(ThumbnailState::Loaded(data)) => {
								let handle = ImageHandle::from_memory(data.clone());
								let thumb_image = Image::new(handle)
									.width(Length::Fixed(64.0))
									.height(Length::Fixed(64.0));
								row = row.push(
									container(thumb_image)
										.width(Length::Fixed(68.0))
										.height(Length::Fixed(68.0))
										.align_x(Horizontal::Center)
										.align_y(Vertical::Center),
								);
							}
							Some(ThumbnailState::Loading) => {
								row = row.push(
									container(text("...").size(12))
										.width(Length::Fixed(68.0))
										.height(Length::Fixed(68.0))
										.align_x(Horizontal::Center)
										.align_y(Vertical::Center)
										.style(theme::Container::Box),
								);
							}
							Some(ThumbnailState::Failed) | None => {
								row = row.push(
									container(text("?").size(12))
										.width(Length::Fixed(68.0))
										.height(Length::Fixed(68.0))
										.align_x(Horizontal::Center)
										.align_y(Vertical::Center)
										.style(theme::Container::Box),
								);
							}
						}
					}

					// Add the file info
					let label = if entry.is_dir {
						format!("[DIR] {}", entry.name)
					} else {
						format!("{} ({})", entry.name, format_size(entry.size))
					};
					row = row.push(text(label).width(Length::Fill));

					let entry_button = button(row)
						.width(Length::Fill)
						.padding(4)
						.on_press(GuiMessage::FileEntryActivated(entry.clone()));
					list = list.push(entry_button);
				}
				layout = layout.push(scrollable(list).height(Length::Fill));
			}
		}
		layout.into()
	}

	fn view_file_viewer(&self, state: &FileViewerState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text(format!("Viewing {} on {}", state.path, state.peer_id)).size(24));
		let mut summary = format!("Loaded {} bytes", state.data.len());
		if let Some(mime) = &state.mime {
			summary.push_str(&format!(" | {}", mime));
		}
		if state.eof {
			summary.push_str(" (end of file)");
		}
		layout = layout.push(text(summary).size(14));
		if let Some(err) = &state.error {
			layout = layout.push(text(format!("Error: {}", err)).size(14));
		}
		if state.is_image() {
			if state.data.is_empty() {
				if state.loading {
					layout = layout.push(text("Loading image data...").size(14));
				} else {
					layout = layout.push(text("Image data not yet loaded").size(14));
				}
			} else if !state.eof {
				layout = layout.push(
					text("Partial image data loaded — load remaining bytes to render").size(14),
				);
			} else {
				let handle = ImageHandle::from_memory(state.data.clone());
				let image_view = Image::new(handle)
					.width(Length::Shrink)
					.height(Length::Shrink);
				layout = layout.push(
					container(image_view)
						.width(Length::Fill)
						.height(Length::Fill)
						.align_x(Horizontal::Center)
						.align_y(Vertical::Center),
				);
			}
		} else if !state.data.is_empty() {
			let (preview, lossy) = file_preview_text(&state.data);
			let mut preview_column = iced::widget::Column::new().spacing(4);
			if lossy {
				preview_column =
					preview_column.push(text("Binary data - non UTF-8 bytes replaced").size(12));
			}
			preview_column = preview_column.push(text(preview).size(14).width(Length::Fill));
			layout = layout.push(
				scrollable(
					container(preview_column)
						.padding(8)
						.style(theme::Container::Box),
				)
				.height(Length::Fill),
			);
		} else if state.loading {
			layout = layout.push(text("Loading file chunk...").size(14));
		} else if state.eof {
			layout = layout.push(text("File is empty").size(14));
		} else {
			layout = layout.push(text("No data loaded yet").size(14));
		}
		let mut controls = iced::widget::Row::new().spacing(12);
		if !state.eof {
			let label = if state.loading {
				"Loading..."
			} else {
				"Load more"
			};
			let mut load_btn = button(text(label));
			if !state.loading {
				load_btn = load_btn.on_press(GuiMessage::FileReadMore);
			}
			controls = controls.push(load_btn);
		}
		controls =
			controls.push(button(text("Back to browser")).on_press(GuiMessage::FileViewerBack));
		layout = layout.push(controls);
		layout.into()
	}

	fn view_graph(&self) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text("Peers Graph Overview").size(24));
		if self.graph.nodes.is_empty() {
			layout = layout.push(text("Graph is empty.").size(16));
		} else {
			if let Some(id) = self.graph.selected_id() {
				layout = layout.push(text(format!("Selected peer: {}", id)).size(16));
			}
			let mut list = iced::widget::Column::new().spacing(4);
			for node in &self.graph.nodes {
				let marker = if Some(node.id.as_str()) == self.graph.selected_id() {
					"▶"
				} else {
					""
				};
				list = list.push(
					text(format!(
						"{} {} (angle {:.2} rad)",
						marker, node.id, node.angle
					))
					.size(14),
				);
			}
			layout = layout.push(scrollable(list).height(Length::Fill));
			let action_message = self
				.graph
				.selected_id()
				.map(|id| GuiMessage::PeerActionsRequested(id.to_string()))
				.unwrap_or(GuiMessage::BackToPeers);
			let controls = iced::widget::Row::new()
				.spacing(12)
				.push(button(text("Previous")).on_press(GuiMessage::GraphPrev))
				.push(button(text("Next")).on_press(GuiMessage::GraphNext))
				.push(button(text("Open actions")).on_press(action_message));
			layout = layout.push(controls);
		}
		layout.into()
	}

	fn view_create_user(&self, form: &CreateUserForm) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text("Create User (placeholder)").size(24));
		layout = layout
			.push(text_input("username", &form.username).on_input(GuiMessage::UsernameChanged));
		layout = layout.push(
			text_input("password", &form.password)
				.secure(true)
				.on_input(GuiMessage::PasswordChanged),
		);
		layout = layout.push(button(text("Submit")).on_press(GuiMessage::CreateUserSubmit));
		if let Some(status) = &form.status {
			layout = layout.push(text(status).size(16));
		}
		layout.into()
	}

	fn view_file_search(&self, state: &FileSearchState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);

		// Title and view mode toggle
		let title_row = iced::widget::Row::new()
			.spacing(12)
			.align_items(iced::Alignment::Center)
			.push(text("Files").size(24))
			.push(iced::widget::Space::with_width(Length::Fill))
			.push(
				button(text("Thumbnails"))
					.style(if state.view_mode == FilesViewMode::Thumbnails {
						theme::Button::Primary
					} else {
						theme::Button::Secondary
					})
					.on_press(GuiMessage::FilesViewModeChanged(FilesViewMode::Thumbnails)),
			)
			.push(
				button(text("Table"))
					.style(if state.view_mode == FilesViewMode::Table {
						theme::Button::Primary
					} else {
						theme::Button::Secondary
					})
					.on_press(GuiMessage::FilesViewModeChanged(FilesViewMode::Table)),
			);
		layout = layout.push(title_row);

		// Search options section
		layout = layout.push(text("Search Options").size(18));

		// Name and content search inputs (side by side)
		let search_row1 = iced::widget::Row::new()
			.spacing(12)
			.push(
				text_input("Name search", &state.name_query)
					.on_input(GuiMessage::FilesNameQueryChanged)
					.width(Length::FillPortion(1)),
			)
			.push(
				text_input("Content search", &state.content_query)
					.on_input(GuiMessage::FilesContentQueryChanged)
					.width(Length::FillPortion(1)),
			);
		layout = layout.push(search_row1);

		// Date range and mime type (side by side)
		let mut mime_options = state.available_mime_types.clone();
		mime_options.sort();
		let search_row2 = iced::widget::Row::new()
			.spacing(12)
			.push(
				text_input("Date from (YYYY-MM-DD)", &state.date_from)
					.on_input(GuiMessage::FilesDateFromChanged)
					.width(Length::FillPortion(1)),
			)
			.push(
				text_input("Date to (YYYY-MM-DD)", &state.date_to)
					.on_input(GuiMessage::FilesDateToChanged)
					.width(Length::FillPortion(1)),
			)
			.push(
				pick_list(
					mime_options,
					if state.selected_mime.is_empty() {
						None
					} else {
						Some(state.selected_mime.clone())
					},
					|v| GuiMessage::FileSearchMimeChanged(v),
				)
				.placeholder("(any mime type)")
				.width(Length::FillPortion(1)),
			);
		layout = layout.push(search_row2);

		// Sort toggle and search button
		let sort_label = if state.sort_desc {
			"Sort: Latest desc"
		} else {
			"Sort: Latest asc"
		};
		let controls_row = iced::widget::Row::new()
			.spacing(12)
			.push(button(text(sort_label)).on_press(GuiMessage::FileSearchToggleSort))
			.push(button(text("Search")).on_press(GuiMessage::FileSearchExecute));
		layout = layout.push(controls_row);

		// Loading/error states
		if state.loading {
			return layout.push(text("Searching...")).into();
		}
		if let Some(err) = &state.error {
			return layout.push(text(format!("Error: {}", err))).into();
		}
		if state.results.is_empty() {
			return layout.push(text("No results (run a search)")).into();
		}

		// Results display based on view mode
		match state.view_mode {
			FilesViewMode::Table => {
				// Table header
				let header = iced::widget::Row::new()
					.spacing(8)
					.push(text("Name").size(14).width(Length::FillPortion(3)))
					.push(text("Size").size(14).width(Length::FillPortion(1)))
					.push(text("Mime Type").size(14).width(Length::FillPortion(2)))
					.push(text("Replicas").size(14).width(Length::FillPortion(1)))
					.push(text("First Date").size(14).width(Length::FillPortion(2)))
					.push(text("Last Date").size(14).width(Length::FillPortion(2)));
				layout = layout.push(container(header).padding(4).style(theme::Container::Box));

				// Table rows
				let mut list = iced::widget::Column::new().spacing(2);
				for entry in &state.results {
					let display_name = if entry.name.is_empty() {
						abbreviate_hash(&entry.hash)
					} else {
						entry.name.clone()
					};
					let row = iced::widget::Row::new()
						.spacing(8)
						.push(text(display_name).size(14).width(Length::FillPortion(3)))
						.push(text(format_size(entry.size)).size(14).width(Length::FillPortion(1)))
						.push(
							text(entry.mime_type.clone().unwrap_or_else(|| "?".into()))
								.size(14)
								.width(Length::FillPortion(2)),
						)
						.push(text(entry.replicas.to_string()).size(14).width(Length::FillPortion(1)))
						.push(text(&entry.first).size(14).width(Length::FillPortion(2)))
						.push(text(&entry.latest).size(14).width(Length::FillPortion(2)));

					// Make row clickable if we have a valid path and node_id
					if !entry.path.is_empty() && !entry.node_id.is_empty() {
						let row_button = button(row)
							.width(Length::Fill)
							.padding(4)
							.style(theme::Button::Text)
							.on_press(GuiMessage::FilesOpenFile {
								node_id: entry.node_id.clone(),
								path: entry.path.clone(),
								mime: entry.mime_type.clone(),
							});
						list = list.push(row_button);
					} else {
						list = list.push(container(row).padding(4));
					}
				}
				layout = layout.push(
					scrollable(list)
						.height(Length::Fill)
						.id(scrollable::Id::new("files_table"))
						.on_scroll(GuiMessage::FilesScrolled),
				);

				// Pagination controls
				let total_pages = if state.total_count == 0 {
					1
				} else {
					(state.total_count + state.page_size - 1) / state.page_size
				};
				let start = state.page * state.page_size + 1;
				let end = (start + state.results.len()).saturating_sub(1);
				let page_info = format!(
					"Page {} of {} ({}-{} of {})",
					state.page + 1,
					total_pages,
					if state.total_count == 0 { 0 } else { start },
					end,
					state.total_count
				);

				let mut prev_btn = button(text("Previous"));
				if state.page > 0 {
					prev_btn = prev_btn.on_press(GuiMessage::FilesPrevPage);
				}

				let mut next_btn = button(text("Next"));
				let max_page = state.total_count.saturating_sub(1) / state.page_size.max(1);
				if state.page < max_page && state.total_count > 0 {
					next_btn = next_btn.on_press(GuiMessage::FilesNextPage);
				}

				let pagination_row = iced::widget::Row::new()
					.spacing(12)
					.align_items(iced::Alignment::Center)
					.push(prev_btn)
					.push(text(page_info).size(14))
					.push(next_btn);
				layout = layout.push(pagination_row);

				layout.into()
			}
			FilesViewMode::Thumbnails => {
				// Placeholder for thumbnails view - will show a simple message for now
				layout = layout.push(text("Thumbnails view coming soon...").size(16));
				layout.into()
			}
		}
	}

	fn view_scan_controls(&self) -> Element<'_, GuiMessage> {
		let state = &self.scan_state;
		let mut layout = iced::widget::Column::new().spacing(8);
		layout = layout.push(text("Local Scan").size(20));
		layout = layout
			.push(text_input("Folder to scan", &state.path).on_input(GuiMessage::ScanPathChanged));
		let mut controls = iced::widget::Row::new().spacing(12);
		let mut scan_btn = button(text(if state.scanning {
			"Scanning..."
		} else {
			"Scan folder"
		}));
		if state.scanning {
			scan_btn = scan_btn.style(theme::Button::Secondary);
		} else {
			scan_btn = scan_btn.on_press(GuiMessage::ScanRequested);
		}
		controls = controls.push(scan_btn).push(
			button(text("View scan results"))
				.on_press(GuiMessage::MenuSelected(MenuItem::ScanResults)),
		);
		layout = layout.push(controls);
		if let Some(status) = &state.status {
			layout = layout.push(text(status).size(14));
		}
		if state.processed_files > 0 || state.total_files > 0 {
			let progress_label = if state.total_files > 0 {
				format!(
					"Processed {} / {} files",
					state.processed_files, state.total_files
				)
			} else {
				format!("Processed {} files", state.processed_files)
			};
			layout = layout.push(text(progress_label).size(14));
		}
		if let Some(err) = &state.error {
			layout = layout.push(text(format!("Scan error: {}", err)).size(14));
		}
		layout.into()
	}

	fn view_scan_results(&self, state: &ScanResultsState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text("Scan Results").size(24));
		if state.loading {
			return layout.push(text("Loading scan results...")).into();
		}
		if let Some(err) = &state.error {
			return layout
				.push(text(format!("Failed to load scan results: {}", err)).size(16))
				.into();
		}
		if state.entries.is_empty() {
			return layout
				.push(text("No scanned files stored yet.").size(16))
				.into();
		}
		let mut list = iced::widget::Column::new().spacing(4);
		for entry in &state.entries {
			let row = iced::widget::Row::new()
				.spacing(8)
				.push(
					text(&abbreviate_hash(&entry.hash))
						.size(14)
						.width(Length::FillPortion(2)),
				)
				.push(
					text(entry.mime_type.clone().unwrap_or_else(|| "?".into()))
						.size(14)
						.width(Length::FillPortion(2)),
				)
				.push(
					text(format_size(entry.size))
						.size(14)
						.width(Length::FillPortion(1)),
				)
				.push(
					text(entry.latest.clone())
						.size(14)
						.width(Length::FillPortion(2)),
				);
			list = list.push(container(row).padding(4).style(theme::Container::Box));
		}
		let total_pages = if state.page_size == 0 {
			1
		} else {
			state.total_entries.div_ceil(state.page_size).max(1)
		};
		let mut controls = iced::widget::Row::new().spacing(12);
		let mut prev_btn = button(text("Previous"));
		if state.page == 0 || state.loading {
			prev_btn = prev_btn.style(theme::Button::Secondary);
		} else {
			prev_btn = prev_btn.on_press(GuiMessage::ScanResultsPrevPage);
		}
		let mut next_btn = button(text("Next"));
		let at_end = state.page_size == 0
			|| (state.page + 1) * state.page_size >= state.total_entries
				&& state.total_entries != 0;
		if at_end || state.loading {
			next_btn = next_btn.style(theme::Button::Secondary);
		} else {
			next_btn = next_btn.on_press(GuiMessage::ScanResultsNextPage);
		}
		controls = controls
			.push(prev_btn)
			.push(
				text(format!(
					"Page {} of {} ({} files)",
					state.page + 1,
					total_pages,
					state.total_entries
				))
				.size(14),
			)
			.push(next_btn);
		layout
			.push(
				scrollable(list)
					.height(Length::FillPortion(9))
					.width(Length::Fill),
			)
			.push(controls)
			.into()
	}

	fn view_storage_usage(&self, state: &StorageUsageState) -> Element<'_, GuiMessage> {
		let mut layout = iced::widget::Column::new().spacing(12);
		layout = layout.push(text("Storage Usage").size(24));
		if state.loading {
			return scrollable(layout.push(text("Loading storage usage...").size(16)))
				.height(Length::Fill)
				.into();
		}
		if let Some(err) = &state.error {
			return scrollable(
				layout.push(text(format!("Failed to load storage usage: {}", err)).size(16)),
			)
			.height(Length::Fill)
			.into();
		}
		if state.nodes.is_empty() {
			return scrollable(layout.push(text("No storage data available.").size(16)))
				.height(Length::Fill)
				.into();
		}
		layout = layout.push(self.storage_header_row());
		for (index, node) in state.nodes.iter().enumerate() {
			layout = layout.push(self.view_storage_node(node, index));
		}
		scrollable(layout).height(Length::Fill).into()
	}

	fn storage_header_row(&self) -> Element<'_, GuiMessage> {
		let row = iced::widget::Row::new()
			.spacing(8)
			.push(text("Name").size(14).width(Length::FillPortion(4)))
			.push(text("% of node").size(14).width(Length::FillPortion(1)))
			.push(text("Size").size(14).width(Length::FillPortion(1)))
			.push(text("Items").size(14).width(Length::FillPortion(1)))
			.push(text("Last changed").size(14).width(Length::FillPortion(2)));
		container(row)
			.padding(8)
			.style(theme::Container::Box)
			.into()
	}

	fn view_storage_node(&self, node: &StorageNodeView, index: usize) -> Element<'_, GuiMessage> {
		let toggle_label = if node.entries.is_empty() {
			String::new()
		} else if node.expanded {
			String::from("▾")
		} else {
			String::from("▸")
		};
		let mut row = iced::widget::Row::new().spacing(8);
		let toggle_element: Element<_> = if node.entries.is_empty() {
			text("").width(Length::Shrink).into()
		} else {
			button(text(toggle_label))
				.on_press(GuiMessage::StorageUsageToggleNode(index))
				.style(theme::Button::Text)
				.into()
		};
		row = row
			.push(toggle_element)
			.push(
				text(format!(
					"{} [{}] (total: {})",
					node.name,
					node.id,
					format_size(node.total_size)
				))
				.size(16)
				.width(Length::FillPortion(4)),
			)
			.push(text("100%").size(14).width(Length::FillPortion(1)))
			.push(
				text(format_size(node.total_size))
					.size(14)
					.width(Length::FillPortion(1)),
			)
			.push(text("-").size(14).width(Length::FillPortion(1)))
			.push(text("-").size(14).width(Length::FillPortion(2)));
		let mut column = iced::widget::Column::new()
			.spacing(4)
			.push(container(row).padding(6).style(theme::Container::Box));
		if node.expanded {
			column = column.push(self.render_storage_entries(&node.entries, &node.id, index, 1));
		}
		column.into()
	}

	fn render_storage_entries(
		&self,
		entries: &[StorageEntryView],
		node_id: &str,
		node_index: usize,
		depth: usize,
	) -> Element<'_, GuiMessage> {
		let mut column = iced::widget::Column::new().spacing(4);
		for entry in entries {
			let indent = " ".repeat(depth * 2);
			let mut row = iced::widget::Row::new().spacing(8);
			let toggle_element: Element<_> = if entry.children.is_empty() {
				text("").width(Length::Shrink).into()
			} else {
				let symbol = if entry.expanded { "▾" } else { "▸" };
				button(text(symbol))
					.on_press(GuiMessage::StorageUsageToggleEntry {
						node_index,
						path: entry.path.clone(),
					})
					.style(theme::Button::Text)
					.into()
			};
			let open_element: Element<_> = if entry.children.is_empty() {
				button(text("Open").size(12))
					.on_press(GuiMessage::StorageUsageOpenFile {
						node_id: node_id.to_string(),
						path: entry.path.clone(),
					})
					.style(theme::Button::Secondary)
					.padding([2, 8])
					.into()
			} else {
				text("").into()
			};
			row = row
				.push(toggle_element)
				.push(
					text(format!("{}{}", indent, entry.name))
						.size(14)
						.width(Length::FillPortion(4)),
				)
				.push(
					text(format!("{:.1}%", entry.percent))
						.size(14)
						.width(Length::FillPortion(1)),
				)
				.push(
					text(format_size(entry.size))
						.size(14)
						.width(Length::FillPortion(1)),
				)
				.push(
					text(entry.item_count.to_string())
						.size(14)
						.width(Length::FillPortion(1)),
				)
				.push(
					text(entry.last_changed.clone())
						.size(14)
						.width(Length::FillPortion(2)),
				)
				.push(open_element);
			column = column.push(container(row).padding(4).style(theme::Container::Box));
			if entry.expanded && !entry.children.is_empty() {
				column = column.push(self.render_storage_entries(
					&entry.children,
					node_id,
					node_index,
					depth + 1,
				));
			}
		}
		column.into()
	}

	fn toggle_storage_node(&mut self, index: usize) {
		if let Mode::StorageUsage(state) = &mut self.mode {
			if let Some(node) = state.nodes.get_mut(index) {
				node.expanded = !node.expanded;
			}
		}
	}

	fn toggle_storage_entry(&mut self, index: usize, path: &str) {
		if let Mode::StorageUsage(state) = &mut self.mode {
			if let Some(node) = state.nodes.get_mut(index) {
				toggle_storage_entry_recursive(&mut node.entries, path);
			}
		}
	}

	fn gather_known_addresses(&self, peer_id: &str) -> Vec<String> {
		if let Some(state) = &self.latest_state {
			if let Ok(target) = PeerId::from_str(peer_id) {
				state
					.discovered_peers
					.iter()
					.filter(|p| p.peer_id == target)
					.map(|p| p.multiaddr.to_string())
					.collect()
			} else {
				Vec::new()
			}
		} else {
			Vec::new()
		}
	}
}

fn aggregate_peers(state: &State) -> Vec<PeerRow> {
	let mut rows: HashMap<String, PeerRow> = HashMap::new();
	for discovered in &state.discovered_peers {
		let id = format!("{}", discovered.peer_id);
		rows.entry(id.clone())
			.and_modify(|row| {
				if row.address.is_empty() {
					row.address = discovered.multiaddr.to_string();
				}
			})
			.or_insert(PeerRow {
				id,
				address: discovered.multiaddr.to_string(),
				status: String::from("discovered"),
			});
	}
	for connection in &state.connections {
		let id = format!("{}", connection.peer_id);
		rows.entry(id.clone())
			.and_modify(|row| {
				row.status = String::from("connected");
			})
			.or_insert(PeerRow {
				id,
				address: String::new(),
				status: String::from("connected"),
			});
	}
	for peer in &state.peers {
		let id = format!("{}", peer.id);
		rows.entry(id.clone()).or_insert(PeerRow {
			id,
			address: String::new(),
			status: String::new(),
		});
	}
	let me_id = format!("{}", state.me);
	rows.entry(me_id.clone())
		.and_modify(|row| {
			row.status = String::from("local");
			if row.address.is_empty() {
				row.address = LOCAL_LISTEN_MULTIADDR.into();
			}
		})
		.or_insert(PeerRow {
			id: me_id,
			address: LOCAL_LISTEN_MULTIADDR.into(),
			status: String::from("local"),
		});
	let mut vec: Vec<PeerRow> = rows.into_iter().map(|(_, row)| row).collect();
	vec.sort_by(|a, b| a.id.cmp(&b.id));
	vec
}

fn cpu_summary(cpus: &[CpuInfo]) -> String {
	if cpus.is_empty() {
		return String::from("No CPU information available");
	}
	let avg = cpus.iter().map(|cpu| cpu.usage).sum::<f32>() / cpus.len() as f32;
	let max = cpus.iter().map(|cpu| cpu.usage).fold(0.0, f32::max);
	format!("CPUs: {} — avg {:.1}% max {:.1}%", cpus.len(), avg, max)
}

fn format_frequency(freq: u64) -> String {
	if freq >= 1_000_000_000 {
		format!("{:.2} GHz", freq as f64 / 1_000_000_000.0)
	} else if freq >= 1_000_000 {
		format!("{:.2} MHz", freq as f64 / 1_000_000.0)
	} else {
		format!("{} Hz", freq)
	}
}

fn format_size(bytes: u64) -> String {
	const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
	let mut value = bytes as f64;
	let mut unit = 0usize;
	while value >= 1024.0 && unit + 1 < UNITS.len() {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{} {}", bytes, UNITS[unit])
	} else {
		format!("{:.2} {}", value, UNITS[unit])
	}
}

fn copyable_interface_field<'a>(label: &str, value: impl Into<String>) -> Element<'a, GuiMessage> {
	let value_string = value.into();
	let input = text_input("", value_string.as_str())
		.on_input(|_| GuiMessage::InterfacesFieldEdited)
		.padding(6)
		.size(14)
		.width(Length::Fill);
	let row = iced::widget::Row::new()
		.spacing(8)
		.push(text(label).size(14).width(Length::Shrink))
		.push(input);
	container(row).padding(4).into()
}

fn format_disk_label(disk: &DiskInfo) -> String {
	let total = format_size(disk.total_space);
	let free = format_size(disk.available_space);
	let mut label = format!(
		"{} ({}) — {} free of {}",
		disk.mount_path, disk.name, free, total
	);
	if disk.read_only {
		label.push_str(" [read-only]");
	}
	if disk.removable {
		label.push_str(" [removable]");
	}
	label
}

fn file_preview_text(data: &[u8]) -> (String, bool) {
	match std::str::from_utf8(data) {
		Ok(text) => (text.to_string(), false),
		Err(_) => (String::from_utf8_lossy(data).to_string(), true),
	}
}

fn abbreviate_peer_id(id: &str) -> String {
	const PREFIX: usize = 8;
	const SUFFIX: usize = 6;
	if id.len() <= PREFIX + SUFFIX + 1 {
		id.to_string()
	} else {
		format!("{}…{}", &id[..PREFIX], &id[id.len() - SUFFIX..])
	}
}

fn abbreviate_hash(hash_hex: &str) -> String {
	const PREFIX: usize = 8;
	const SUFFIX: usize = 8;
	if hash_hex.len() <= PREFIX + SUFFIX + 1 {
		hash_hex.to_string()
	} else {
		format!(
			"{}…{}",
			&hash_hex[..PREFIX],
			&hash_hex[hash_hex.len() - SUFFIX..]
		)
	}
}

fn normalize_path(path: &str) -> String {
	let trimmed = path.trim();
	if trimmed.is_empty() {
		return String::from("/");
	}
	if should_list_disks_first() && is_windows_drive_root(trimmed) {
		return normalize_windows_drive(trimmed);
	}
	let without_sep = trimmed.trim_end_matches(|c| path_separators().contains(&c));
	if without_sep.is_empty() {
		String::from("/")
	} else {
		without_sep.to_string()
	}
}

fn permissions_roots(permissions: &[Permission]) -> Vec<String> {
	let mut roots: BTreeSet<String> = BTreeSet::new();
	for permission in permissions {
		if let Rule::Folder(rule) = permission.rule() {
			let path = rule.path().to_string_lossy().to_string();
			roots.insert(normalize_path(&path));
		}
	}
	roots.into_iter().collect()
}

fn default_browser_path(permissions: &[Permission]) -> Option<String> {
	permissions.iter().find_map(|permission| {
		if let Rule::Folder(rule) = permission.rule() {
			if rule.can_read() || rule.can_search() || rule.can_write() {
				let path = rule.path().to_string_lossy().to_string();
				return Some(normalize_path(&path));
			}
		}
		None
	})
}

fn toggle_storage_entry_recursive(entries: &mut [StorageEntryView], path: &str) -> bool {
	for entry in entries {
		if entry.path == path {
			if !entry.children.is_empty() {
				entry.expanded = !entry.expanded;
			}
			return true;
		}
		if toggle_storage_entry_recursive(&mut entry.children, path) {
			return true;
		}
	}
	false
}

fn join_child_path(base: &str, child: &str) -> String {
	let trimmed_child = child.trim_matches(|c| path_separators().contains(&c));
	if base.is_empty() {
		return if trimmed_child.is_empty() {
			String::from("/")
		} else {
			trimmed_child.to_string()
		};
	}
	let mut path = PathBuf::from(base);
	if !trimmed_child.is_empty() {
		path.push(trimmed_child);
	}
	path.to_string_lossy().to_string()
}

fn parent_path(path: &str) -> String {
	if path.is_empty() {
		return String::from("/");
	}
	if should_list_disks_first() && is_windows_drive_root(path) {
		return normalize_windows_drive(path);
	}
	let trimmed = path.trim_end_matches(|c| path_separators().contains(&c));
	if trimmed.is_empty() || trimmed == "/" {
		return String::from("/");
	}
	let parent = Path::new(trimmed).parent();
	if let Some(parent) = parent {
		let parent_str = parent.to_string_lossy().to_string();
		if parent_str.is_empty() && should_list_disks_first() && is_windows_drive_root(trimmed) {
			return normalize_windows_drive(trimmed);
		}
		if parent_str.is_empty() {
			return String::from("/");
		}
		if should_list_disks_first() && is_windows_drive_root(&parent_str) {
			return normalize_windows_drive(&parent_str);
		}
		return parent_str;
	}
	String::from("/")
}

#[cfg(target_os = "windows")]
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];
#[cfg(not(target_os = "windows"))]
const PATH_SEPARATORS: [char; 1] = ['/'];

fn path_separators() -> &'static [char] {
	&PATH_SEPARATORS
}

fn should_list_disks_first() -> bool {
	cfg!(target_os = "windows")
}

fn is_windows_drive_root(path: &str) -> bool {
	if !should_list_disks_first() {
		return false;
	}
	let trimmed = path.trim();
	if trimmed.len() < 2 {
		return false;
	}
	let mut chars = trimmed.chars();
	let drive = chars.next().unwrap();
	if !drive.is_ascii_alphabetic() {
		return false;
	}
	if chars.next() != Some(':') {
		return false;
	}
	let remainder: String = chars.collect();
	remainder.is_empty() || remainder == "\\" || remainder == "/"
}

fn normalize_windows_drive(path: &str) -> String {
	let trimmed = path.trim();
	if trimmed.len() < 2 {
		return trimmed.to_string();
	}
	let mut drive = trimmed[..2].to_string();
	drive.push('\\');
	drive
}

async fn fetch_cpus(
	peer: Arc<PuppyNet>,
	peer_id: String,
) -> (String, Result<Vec<CpuInfo>, String>) {
	let result = match PeerId::from_str(&peer_id) {
		Ok(id) => peer.list_cpus(id).await.map_err(|err| err.to_string()),
		Err(err) => Err(err.to_string()),
	};
	(peer_id, result)
}

async fn fetch_interfaces(
	peer: Arc<PuppyNet>,
	peer_id: String,
) -> (String, Result<Vec<InterfaceInfo>, String>) {
	let result = match PeerId::from_str(&peer_id) {
		Ok(id) => peer
			.list_interfaces(id)
			.await
			.map_err(|err| err.to_string()),
		Err(err) => Err(err.to_string()),
	};
	(peer_id, result)
}

async fn wait_for_scan_event(receiver: Arc<Mutex<mpsc::Receiver<ScanEvent>>>) -> ScanEvent {
	match task::spawn_blocking(move || receiver.lock().unwrap().recv()).await {
		Ok(Ok(event)) => event,
		Ok(Err(_)) => ScanEvent::Finished(Err(String::from("Scan worker stopped"))),
		Err(err) => ScanEvent::Finished(Err(format!("Scan wait failed: {err}"))),
	}
}

async fn wait_for_update_event(receiver: Arc<Mutex<mpsc::Receiver<UpdateProgress>>>) -> UpdateProgress {
	match task::spawn_blocking(move || receiver.lock().unwrap().recv()).await {
		Ok(Ok(event)) => event,
		Ok(Err(_)) => UpdateProgress::Failed { error: String::from("Update worker stopped") },
		Err(err) => UpdateProgress::Failed { error: format!("Update wait failed: {err}") },
	}
}

async fn search_files(
	peer: Arc<PuppyNet>,
	name_query: String,
	_content_query: String,
	date_from: String,
	date_to: String,
	mime: Option<String>,
	sort_desc: bool,
	page: usize,
	page_size: usize,
) -> Result<(Vec<FileSearchEntry>, Vec<String>, usize), String> {
	let args = puppynet_core::SearchFilesArgs {
		name_query: if name_query.trim().is_empty() {
			None
		} else {
			Some(name_query)
		},
		content_query: None, // Content search not yet implemented
		date_from: if date_from.trim().is_empty() {
			None
		} else {
			Some(date_from)
		},
		date_to: if date_to.trim().is_empty() {
			None
		} else {
			Some(date_to)
		},
		mime_type: mime,
		sort_desc,
		page,
		page_size,
	};

	let (results, mimes, total) = task::spawn_blocking(move || peer.search_files(args))
		.await
		.map_err(|err| format!("search task failed: {err}"))??;

	let entries = results
		.into_iter()
		.map(|row| {
			let hash = row.hash.iter().map(|b| format!("{:02x}", b)).collect();
			let node_id = row.node_id.iter().map(|b| format!("{:02x}", b)).collect();
			FileSearchEntry {
				hash,
				name: row.name,
				path: row.path,
				node_id,
				size: row.size,
				mime_type: row.mime_type,
				replicas: row.replicas,
				first: row.first_datetime.unwrap_or_else(|| String::from("-")),
				latest: row.latest_datetime.unwrap_or_else(|| String::from("-")),
			}
		})
		.collect();

	Ok((entries, mimes, total))
}

async fn load_mime_types(peer: Arc<PuppyNet>) -> Result<Vec<String>, String> {
	task::spawn_blocking(move || peer.get_mime_types())
		.await
		.map_err(|err| format!("mime types task failed: {err}"))?
}

async fn load_scan_results_page(
	peer: Arc<PuppyNet>,
	page: usize,
	page_size: usize,
) -> Result<(Vec<FileSearchEntry>, usize), String> {
	let (rows, total) = task::spawn_blocking(move || peer.fetch_scan_results_page(page, page_size))
		.await
		.map_err(|err| format!("scan results task failed: {err}"))??;
	let entries = rows
		.into_iter()
		.map(|row| {
			let hash = row.hash.iter().map(|b| format!("{:02x}", b)).collect();
			FileSearchEntry {
				hash,
				name: String::new(), // TODO: populate from database
				path: String::new(), // TODO: populate from database
				node_id: String::new(), // TODO: populate from database
				size: row.size,
				mime_type: row.mime_type,
				replicas: 0, // TODO: populate from database
				first: row.first_datetime.unwrap_or_else(|| String::from("-")),
				latest: row.latest_datetime.unwrap_or_else(|| String::from("-")),
			}
		})
		.collect();
	Ok((entries, total))
}

async fn load_storage_usage(peer: Arc<PuppyNet>) -> Result<Vec<StorageNodeView>, String> {
	let files = peer
		.list_storage_files()
		.await
		.map_err(|err| err.to_string())?;
	let known_peers: Vec<PeerId> = {
		let state_arc = peer.state();
		let state = state_arc.lock().map_err(|e| e.to_string())?;
		let mut peers = vec![state.me];
		peers.extend(state.connections.iter().map(|c| c.peer_id));
		peers.extend(state.discovered_peers.iter().map(|d| d.peer_id));
		peers
	};
	Ok(build_storage_nodes(files, &known_peers))
}

fn build_storage_nodes(files: Vec<StorageUsageFile>, known_peers: &[PeerId]) -> Vec<StorageNodeView> {
	// Build a map from truncated node_id (first 16 bytes) to full PeerId
	let peer_map: HashMap<Vec<u8>, PeerId> = known_peers
		.iter()
		.filter_map(|peer_id| {
			let bytes = peer_id.to_bytes();
			if bytes.len() >= 16 {
				Some((bytes[..16].to_vec(), *peer_id))
			} else {
				None
			}
		})
		.collect();

	let mut grouped: HashMap<Vec<u8>, (String, Vec<FileRecord>)> = HashMap::new();
	for file in files {
		let entry = grouped
			.entry(file.node_id.clone())
			.or_insert_with(|| (file.node_name.clone(), Vec::new()));
		entry.1.push(FileRecord {
			path: PathBuf::from(file.path),
			size: file.size,
			last_changed: file.last_changed,
		});
	}
	let mut nodes: Vec<StorageNodeView> = grouped
		.into_iter()
		.filter_map(|(node_id, (name, records))| {
			let peer_id = peer_map.get(&node_id)?;
			let (entries, total_size) = build_storage_tree(records);
			Some(StorageNodeView {
				name,
				id: peer_id.to_string(),
				total_size,
				entries,
				expanded: false,
			})
		})
		.collect();
	nodes.sort_by(|a, b| a.name.cmp(&b.name));
	nodes
}

fn build_storage_tree(files: Vec<FileRecord>) -> (Vec<StorageEntryView>, u64) {
	let mut stats: HashMap<PathBuf, EntryStats> = HashMap::new();
	let mut children: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
	for file in files {
		let mut ancestors = Vec::new();
		let mut current = Some(file.path.as_path());
		while let Some(path) = current {
			ancestors.push(path.to_path_buf());
			current = path.parent();
		}
		ancestors.push(PathBuf::new());
		for path in ancestors.iter() {
			let entry = stats.entry(path.clone()).or_insert_with(EntryStats::new);
			entry.size += file.size;
			entry.item_count += 1;
			if let Some(last) = file.last_changed {
				entry.last_changed = match entry.last_changed {
					Some(existing) if existing >= last => Some(existing),
					_ => Some(last),
				};
			}
		}
		for pair in ancestors.windows(2) {
			if let [child, parent] = pair {
				children
					.entry(parent.clone())
					.or_insert_with(BTreeSet::new)
					.insert(child.clone());
			}
		}
	}
	let total_size = stats.get(&PathBuf::new()).map(|s| s.size).unwrap_or(0);
	let entries = build_storage_entries_for(&PathBuf::new(), &stats, &children, total_size);
	(entries, total_size)
}

fn build_storage_entries_for(
	parent: &PathBuf,
	stats: &HashMap<PathBuf, EntryStats>,
	children: &HashMap<PathBuf, BTreeSet<PathBuf>>,
	total_size: u64,
) -> Vec<StorageEntryView> {
	let mut result = Vec::new();
	if let Some(child_paths) = children.get(parent) {
		for child_path in child_paths.iter().rev() {
			if child_path.as_os_str().is_empty() {
				continue;
			}
			if let Some(data) = stats.get(child_path) {
				let percent = if total_size == 0 {
					0.0
				} else {
					(data.size as f32 / total_size as f32) * 100.0
				};
				let mut entry = StorageEntryView {
					path: child_path.to_string_lossy().into_owned(),
					name: display_name(child_path),
					size: data.size,
					item_count: data.item_count,
					last_changed: format_timestamp(data.last_changed),
					percent,
					children: Vec::new(),
					expanded: false,
				};
				entry.children = build_storage_entries_for(child_path, stats, children, data.size);
				result.push(entry);
			}
		}
		result.sort_by(|a, b| b.size.cmp(&a.size));
	}
	result
}

fn display_name(path: &Path) -> String {
	if path.as_os_str().is_empty() {
		String::from("Root")
	} else if let Some(name) = path.file_name() {
		name.to_string_lossy().into_owned()
	} else {
		path.to_string_lossy().into_owned()
	}
}

fn bytes_to_hex(bytes: &[u8]) -> String {
	let mut s = String::with_capacity(bytes.len() * 2);
	for b in bytes {
		use std::fmt::Write;
		let _ = write!(&mut s, "{:02x}", b);
	}
	s
}

fn format_timestamp(value: Option<DateTime<Utc>>) -> String {
	value
		.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
		.unwrap_or_else(|| String::from("-"))
}

#[derive(Debug, Clone)]
struct FileRecord {
	path: PathBuf,
	size: u64,
	last_changed: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct EntryStats {
	size: u64,
	item_count: u64,
	last_changed: Option<DateTime<Utc>>,
}

impl EntryStats {
	fn new() -> Self {
		Self {
			size: 0,
			item_count: 0,
			last_changed: None,
		}
	}
}

pub fn run(app_title: String) -> iced::Result {
	let mut settings = Settings::default();
	settings.window.size = iced::Size::new(1024.0, 720.0);
	settings.flags = app_title;
	GuiApp::run(settings)
}

#[cfg(test)]
mod tests {
	use super::*;

	use libp2p::PeerId;
	use std::fs;
	use std::path::{Path, PathBuf};
	use std::time::{SystemTime, UNIX_EPOCH};

	fn with_runtime<T>(test: impl FnOnce() -> T) -> T {
		let runtime = tokio::runtime::Runtime::new().expect("runtime");
		let guard = runtime.enter();
		let result = test();
		drop(guard);
		runtime.shutdown_background();
		result
	}

	fn temporary_key_path(test: &str) -> PathBuf {
		let mut path = std::env::temp_dir();
		let unique = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos();
		path.push(format!(
			"PuppyNet-test-{}-{}-{}.key",
			test,
			std::process::id(),
			unique
		));
		if path.exists() {
			let _ = fs::remove_file(&path);
		}
		path
	}

	fn set_keypair_var(path: &Path) {
		unsafe {
			std::env::set_var("KEYPAIR", path);
		}
	}

	fn clear_keypair_var() {
		unsafe {
			std::env::remove_var("KEYPAIR");
		}
	}

	#[test]
	fn selecting_peers_refreshes_from_state() {
		with_runtime(|| {
			let key_path = temporary_key_path("refresh");
			set_keypair_var(&key_path);
			let (mut app, _) = GuiApp::new(String::from("Test Title"));
			let new_peer = PeerId::random();
			{
				let state = app.peer.state();
				let mut guard = state.lock().expect("state lock");
				guard.peer_discovered(new_peer, "/ip4/127.0.0.1/tcp/7000".parse().unwrap());
			}
			app.peers.clear();
			let _ = app.update(GuiMessage::MenuSelected(MenuItem::Peers));
			assert!(matches!(app.mode, Mode::Peers));
			assert!(app.peers.iter().any(|row| row.id == new_peer.to_string()));
			assert!(app.status.contains("Showing peers"));
			let _ = fs::remove_file(&key_path);
			clear_keypair_var();
		});
	}

	#[test]
	fn selecting_graph_rebuilds_nodes() {
		with_runtime(|| {
			let key_path = temporary_key_path("graph");
			set_keypair_var(&key_path);
			let (mut app, _) = GuiApp::new(String::from("Test Title"));
			let peer_a = PeerId::random();
			let peer_b = PeerId::random();
			{
				let state = app.peer.state();
				let mut guard = state.lock().expect("state lock");
				guard.peer_discovered(peer_a, "/ip4/127.0.0.1/tcp/7001".parse().unwrap());
				guard.peer_discovered(peer_b, "/ip4/127.0.0.1/tcp/7002".parse().unwrap());
			}
			app.graph.nodes.clear();
			let _ = app.update(GuiMessage::MenuSelected(MenuItem::PeersGraph));
			assert!(matches!(app.mode, Mode::PeersGraph));
			assert_eq!(app.graph.nodes.len(), 3); // includes local peer
			assert!(app.status.contains("Graph overview"));
			let _ = fs::remove_file(&key_path);
			clear_keypair_var();
		});
	}
}
