use crate::db::FileEntry;
use crate::p2p::{CpuInfo, InterfaceInfo};
use crate::{PuppyNet, StorageUsageFile};
use anyhow::Result;
use axum::{Router, serve};
use libp2p::PeerId;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::{net::TcpListener, signal, sync::Mutex, task};
use wgui::types::{ClientEvent, OnClick};
use wgui::{Item, Wgui, WguiHandle, button, hstack, text, vstack};

const ACTION_NAV_HOME: u32 = 100;
const ACTION_NAV_PEERS: u32 = 101;
const ACTION_NAV_FILES: u32 = 102;
const ACTION_NAV_STORAGE: u32 = 103;
const ACTION_NAV_USERS: u32 = 104;
const ACTION_NAV_UPDATES: u32 = 105;
const ACTION_NAV_SETTINGS: u32 = 106;
const ACTION_PEER_ROW: u32 = 200;
const ACTION_PEER_BACK: u32 = 201;
const ACTION_REFRESH_PEERS: u32 = 300;
const ACTION_REFRESH_FILES: u32 = 301;
const ACTION_REFRESH_STORAGE: u32 = 302;
const ACTION_REFRESH_USERS: u32 = 303;

#[derive(Clone, PartialEq, Eq)]
enum Page {
	Home,
	Peers,
	PeerDetail(String),
	Files,
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
}

impl UiAction {
	fn id(&self) -> u32 {
		match self {
			UiAction::NavHome => ACTION_NAV_HOME,
			UiAction::NavPeers => ACTION_NAV_PEERS,
			UiAction::NavFiles => ACTION_NAV_FILES,
			UiAction::NavStorage => ACTION_NAV_STORAGE,
			UiAction::NavUsers => ACTION_NAV_USERS,
			UiAction::NavUpdates => ACTION_NAV_UPDATES,
			UiAction::NavSettings => ACTION_NAV_SETTINGS,
			UiAction::PeerRow(_) => ACTION_PEER_ROW,
			UiAction::PeerBack => ACTION_PEER_BACK,
			UiAction::RefreshPeers => ACTION_REFRESH_PEERS,
			UiAction::RefreshFiles => ACTION_REFRESH_FILES,
			UiAction::RefreshStorage => ACTION_REFRESH_STORAGE,
			UiAction::RefreshUsers => ACTION_REFRESH_USERS,
		}
	}

	fn inx(&self) -> Option<u32> {
		match self {
			UiAction::PeerRow(idx) => Some(*idx as u32),
			_ => None,
		}
	}

	fn from_event(id: u32, inx: Option<u32>) -> Option<Self> {
		match id {
			ACTION_NAV_HOME => Some(UiAction::NavHome),
			ACTION_NAV_PEERS => Some(UiAction::NavPeers),
			ACTION_NAV_FILES => Some(UiAction::NavFiles),
			ACTION_NAV_STORAGE => Some(UiAction::NavStorage),
			ACTION_NAV_USERS => Some(UiAction::NavUsers),
			ACTION_NAV_UPDATES => Some(UiAction::NavUpdates),
			ACTION_NAV_SETTINGS => Some(UiAction::NavSettings),
			ACTION_PEER_ROW => inx.map(|value| UiAction::PeerRow(value as usize)),
			ACTION_PEER_BACK => Some(UiAction::PeerBack),
			ACTION_REFRESH_PEERS => Some(UiAction::RefreshPeers),
			ACTION_REFRESH_FILES => Some(UiAction::RefreshFiles),
			ACTION_REFRESH_STORAGE => Some(UiAction::RefreshStorage),
			ACTION_REFRESH_USERS => Some(UiAction::RefreshUsers),
			_ => None,
		}
	}
}

pub async fn run_ui(puppy: Arc<PuppyNet>, bind: SocketAddr) -> Result<()> {
	log::info!("starting PuppyNet UI on {}", bind);
	let mut wgui = Wgui::new_without_server();
	let router = Router::new().merge(wgui.router());
	let handle = wgui.handle();
	let server_state = Arc::new(UiServer::new(puppy, handle));
	server_state.refresh_all().await;
	server_state.broadcast_view().await;

	let listener = TcpListener::bind(bind).await?;
	let server_router = router.clone();
	let server_handle = tokio::spawn(async move {
		if let Err(err) = serve(listener, server_router).await {
			log::error!("wgui server error: {err}");
		}
	});

	let shutdown = signal::ctrl_c();
	tokio::pin!(shutdown);

	loop {
		tokio::select! {
			maybe_event = wgui.next() => {
				match maybe_event {
					Some(event) => server_state.handle_event(event).await,
					None => break,
				}
			}
			_ = &mut shutdown => {
				log::info!("shutting down UI");
				break;
			}
		}
	}

	server_handle.abort();
	let _ = server_handle.await;
	Ok(())
}

struct UiServer {
	puppy: Arc<PuppyNet>,
	handle: WguiHandle,
	state: Mutex<UiState>,
	clients: Mutex<HashSet<usize>>,
}

impl UiServer {
	fn new(puppy: Arc<PuppyNet>, handle: WguiHandle) -> Self {
		Self {
			puppy,
			handle,
			state: Mutex::new(UiState::new()),
			clients: Mutex::new(HashSet::new()),
		}
	}

	async fn refresh_all(&self) {
		self.refresh_peers().await;
		self.refresh_files().await;
		self.refresh_storage().await;
		self.refresh_users().await;
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

	async fn handle_event(&self, event: ClientEvent) {
		match event {
			ClientEvent::Connected { id } => {
				self.clients.lock().await.insert(id);
				self.broadcast_view().await;
			}
			ClientEvent::Disconnected { id } => {
				self.clients.lock().await.remove(&id);
			}
			ClientEvent::OnClick(OnClick { id, inx }) => {
				if let Some(action) = UiAction::from_event(id, inx) {
					self.handle_action(action).await;
				}
			}
			_ => {}
		}
	}

	async fn handle_action(&self, action: UiAction) {
		match action {
			UiAction::NavHome => {
				self.set_page(Page::Home).await;
			}
			UiAction::NavPeers => {
				self.set_page(Page::Peers).await;
				self.refresh_peers().await;
			}
			UiAction::NavFiles => {
				self.set_page(Page::Files).await;
				self.refresh_files().await;
			}
			UiAction::NavStorage => {
				self.set_page(Page::Storage).await;
				self.refresh_storage().await;
			}
			UiAction::NavUsers => {
				self.set_page(Page::Users).await;
				self.refresh_users().await;
			}
			UiAction::NavUpdates => {
				self.set_page(Page::Updates).await;
			}
			UiAction::NavSettings => {
				self.set_page(Page::Settings).await;
			}
			UiAction::PeerRow(idx) => {
				let target = {
					let state = self.state.lock().await;
					state.peers.get(idx).map(|peer| peer.id.clone())
				};
				if let Some(peer_id) = target {
					self.set_page(Page::PeerDetail(peer_id.clone())).await;
					self.refresh_peer_detail(&peer_id).await;
				}
			}
			UiAction::PeerBack => {
				self.set_page(Page::Peers).await;
				self.refresh_peers().await;
			}
			UiAction::RefreshPeers => {
				self.refresh_peers().await;
			}
			UiAction::RefreshFiles => {
				self.refresh_files().await;
			}
			UiAction::RefreshStorage => {
				self.refresh_storage().await;
			}
			UiAction::RefreshUsers => {
				self.refresh_users().await;
			}
		}
		self.broadcast_view().await;
	}

	async fn set_page(&self, page: Page) {
		let mut state = self.state.lock().await;
		state.page = page.clone();
		state.selected_peer = match page {
			Page::PeerDetail(peer_id) => Some(peer_id),
			_ => None,
		};
	}

	fn build_nav(&self, state: &UiState) -> Item {
		let actions = [
			(UiAction::NavHome, "Home"),
			(UiAction::NavPeers, "Peers"),
			(UiAction::NavFiles, "Files"),
			(UiAction::NavStorage, "Storage"),
			(UiAction::NavUsers, "Users"),
			(UiAction::NavUpdates, "Updates"),
			(UiAction::NavSettings, "Settings"),
		];
		let items = actions
			.iter()
			.map(|(action, label)| {
				let mut btn = button(label).id(action.id());
				if let Some(inx) = action.inx() {
					btn = btn.inx(inx);
				}
				if self.action_active(*action, &state.page) {
					btn = btn.background_color("#276EF1");
				} else {
					btn = btn.background_color("#1F1F1F");
				}
				btn
			})
			.collect::<Vec<_>>();
		hstack(items).spacing(6).padding(4)
	}

	fn action_active(&self, action: UiAction, page: &Page) -> bool {
		match (action, page) {
			(UiAction::NavHome, Page::Home) => true,
			(UiAction::NavPeers, Page::Peers) | (UiAction::NavPeers, Page::PeerDetail(_)) => true,
			(UiAction::NavFiles, Page::Files) => true,
			(UiAction::NavStorage, Page::Storage) => true,
			(UiAction::NavUsers, Page::Users) => true,
			(UiAction::NavUpdates, Page::Updates) => true,
			(UiAction::NavSettings, Page::Settings) => true,
			_ => false,
		}
	}

	fn build_page(&self, state: &UiState) -> Item {
		match &state.page {
			Page::Home => self.render_home(state),
			Page::Peers => self.render_peers(state),
			Page::PeerDetail(_) => self.render_peer_detail(state),
			Page::Files => self.render_files(state),
			Page::Storage => self.render_storage(state),
			Page::Users => self.render_users(state),
			Page::Updates => self.render_updates(state),
			Page::Settings => self.render_settings(state),
		}
	}

	fn render_home(&self, state: &UiState) -> Item {
		let items = vec![
			text(&format!("Peers: {}", state.peers.len())),
			text(&format!("Files captured: {}", state.files.len())),
			text(&format!("Storage entries: {}", state.storage.len())),
			text(&format!("Users: {}", state.users.len())),
		];
		vstack(items)
			.spacing(4)
			.padding(6)
			.background_color("#111")
			.border("#333")
	}

	fn render_peers(&self, state: &UiState) -> Item {
		let rows = if state.peers.is_empty() {
			vec![text("No peers discovered yet.")]
		} else {
			state
				.peers
				.iter()
				.enumerate()
				.map(|(idx, peer)| {
					let label = if peer.local {
						format!("{} (you)", peer.name)
					} else {
						peer.name.clone()
					};
					hstack([
						text(&format!("{} — {}", label, peer.id)).grow(1),
						button("Details")
							.id(ACTION_PEER_ROW)
							.inx(idx as u32)
							.background_color("#2753FF"),
					])
					.padding(4)
				})
				.collect()
		};
		let header = hstack([
			text("Discovered peers").grow(1),
			button("Refresh")
				.id(ACTION_REFRESH_PEERS)
				.background_color("#2753FF"),
		]);
		vstack([header, vstack(rows).spacing(4)])
			.spacing(6)
			.padding(6)
			.background_color("#111")
			.border("#333")
	}

	fn render_peer_detail(&self, state: &UiState) -> Item {
		let title = match &state.selected_peer {
			Some(peer) => text(&format!("Peer detail — {}", peer)),
			None => text("Peer detail"),
		};
		let cpus = if state.peer_cpus.is_empty() {
			vec![text("No CPU data available.")]
		} else {
			state
				.peer_cpus
				.iter()
				.map(|cpu| {
					text(&format!(
						"{} — {:.1}% | {} Hz",
						cpu.name, cpu.usage, cpu.frequency_hz
					))
				})
				.collect()
		};
		let interfaces = if state.peer_interfaces.is_empty() {
			vec![text("No interface data.")]
		} else {
			state
				.peer_interfaces
				.iter()
				.map(|iface| {
					text(&format!(
						"{} — {} | {}",
						iface.name,
						iface.mac,
						iface.ips.join(", "),
					))
				})
				.collect()
		};
		vstack([
			title,
			text("CPUs:"),
			vstack(cpus).spacing(2),
			text("Interfaces:"),
			vstack(interfaces).spacing(2),
			button("Back")
				.id(ACTION_PEER_BACK)
				.background_color("#2753FF"),
		])
		.spacing(6)
		.padding(6)
		.background_color("#111")
		.border("#333")
	}

	fn render_files(&self, state: &UiState) -> Item {
		let rows = if state.files.is_empty() {
			vec![text("No file entries recorded.")]
		} else {
			state
				.files
				.iter()
				.take(20)
				.map(|entry| {
					text(&format!(
						"{} — {} bytes",
						format_hash(&entry.hash),
						entry.size
					))
				})
				.collect()
		};
		let header = hstack([
			text("Local files").grow(1),
			button("Refresh")
				.id(ACTION_REFRESH_FILES)
				.background_color("#2753FF"),
		]);
		vstack([header, vstack(rows).spacing(2)])
			.spacing(6)
			.padding(6)
			.background_color("#111")
			.border("#333")
	}

	fn render_storage(&self, state: &UiState) -> Item {
		let total: u64 = state.storage.iter().map(|entry| entry.size).sum();
		let summary = text(&format!(
			"Storage entries: {} | Total {}",
			state.storage.len(),
			format_size(total)
		));
		let rows = if state.storage.is_empty() {
			vec![text("No storage data captured yet.")]
		} else {
			state
				.storage
				.iter()
				.take(6)
				.map(|entry| {
					text(&format!(
						"{} — {} | {}",
						entry.node_name,
						entry.path,
						format_size(entry.size),
					))
				})
				.collect()
		};
		let header = hstack([
			text("Storage snapshot").grow(1),
			button("Refresh")
				.id(ACTION_REFRESH_STORAGE)
				.background_color("#2753FF"),
		]);
		vstack([header, summary, vstack(rows).spacing(2)])
			.spacing(6)
			.padding(6)
			.background_color("#111")
			.border("#333")
	}

	fn render_users(&self, state: &UiState) -> Item {
		let rows = if state.users.is_empty() {
			vec![text("No local users recorded.")]
		} else {
			state.users.iter().map(|user| text(user)).collect()
		};
		let header = hstack([
			text("Local users").grow(1),
			button("Refresh")
				.id(ACTION_REFRESH_USERS)
				.background_color("#2753FF"),
		]);
		vstack([header, vstack(rows).spacing(2)])
			.spacing(6)
			.padding(6)
			.background_color("#111")
			.border("#333")
	}

	fn render_updates(&self, _state: &UiState) -> Item {
		vstack([
			text("Updates"),
			text("Remote update workflows will appear here in a future revision."),
		])
		.spacing(4)
		.padding(6)
		.background_color("#111")
		.border("#333")
	}

	fn render_settings(&self, _state: &UiState) -> Item {
		vstack([
			text("Settings"),
			text("Configuration panels will land here soon."),
		])
		.spacing(4)
		.padding(6)
		.background_color("#111")
		.border("#333")
	}

	fn build_view(&self, state: &UiState) -> Item {
		vstack([
			text("PuppyNet UI").padding(4),
			self.build_nav(state),
			self.build_page(state),
			text(&state.status).text_align("center").padding(4),
		])
		.spacing(6)
		.padding(8)
		.background_color("#0B0B0B")
	}

	async fn broadcast_view(&self) {
		let state_clone = {
			let state = self.state.lock().await;
			state.clone()
		};
		let clients = {
			let clients = self.clients.lock().await;
			clients.clone()
		};
		let view = self.build_view(&state_clone);
		for client in clients {
			let _ = self.handle.render(client, view.clone()).await;
		}
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
