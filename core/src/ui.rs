use crate::auth;
use crate::db::FileEntry;
use crate::media_webrtc::{CreateMediaSession, MediaSessionManager};
use crate::node_id::NodeId as PeerId;
use crate::p2p::{
	AudioCapability, AudioDevice, AudioDeviceKind, CpuInfo, DesktopInput, DirEntry, InterfaceInfo,
	LiveSearchArgs, MediaCapability, MediaFrame, MediaSource, MediaSourceKind, MouseButton,
	OnvifCamera, OnvifCameraConfig, PeerInfo, SearchEvent, SearchSort,
};
use crate::updater::UpdateProgress;
use crate::webcam;
use crate::{FLAG_WRITE, LiveSearchPeerEvent, PuppyNet, StorageUsageFile};
use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use rand::rngs::OsRng;
use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use tokio::{signal, sync::Mutex, task};
use wgui::{ClientEvent, HttpRequest, HttpResponse, Item, Wgui, WguiHandle};

const SESSION_COOKIE: &str = "sid";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 7;
const COMMON_SEARCH_MIME_TYPES: &[&str] = &[
	"application/pdf",
	"image/jpeg",
	"image/png",
	"text/plain",
	"video/mp4",
];
const FAVICON_ICO: &[u8] = include_bytes!("../http_assets/favicon.ico");
const MEDIA_RECEIVER_JS: &[u8] = include_bytes!("../http_assets/media_receiver.js");
const TRACKPAD_JS: &[u8] = include_bytes!("../http_assets/trackpad.js");
const SEARCH_ALL_DEVICES: &str = "__all__";

#[derive(Clone, PartialEq, Eq)]
enum Page {
	Home,
	Peers,
	PeerDetail(String),
	PeerControl { peer_id: String },
	PeerFiles { peer_id: String, path: String },
	PeerWebcams { peer_id: String },
	PuppyEye,
	PuppyEyeCamera { peer_id: String, source_id: String },
	OnvifCameras,
	OnvifCamera { peer_id: String, camera_id: String },
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
	version: String,
	os: String,
	uptime: String,
}

#[derive(Clone)]
struct PuppyEyeCamera {
	device_id: String,
	device_name: String,
	source: MediaSource,
}

#[derive(Clone)]
struct PuppyEyeConfiguredCamera {
	device_id: String,
	device_name: String,
	camera: OnvifCamera,
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
	peer_audio_capability: Option<AudioCapability>,
	peer_audio_devices: Vec<AudioDevice>,
	peer_webcam_capability: Option<MediaCapability>,
	peer_webcams: Vec<MediaSource>,
	puppy_eye_cameras: Vec<PuppyEyeCamera>,
	puppy_eye_configured_cameras: Vec<PuppyEyeConfiguredCamera>,
	puppy_eye_status: String,
	peer_microphones: Vec<MediaSource>,
	peer_screens: Vec<MediaSource>,
	peer_screen_status: String,
	peer_files_path: String,
	peer_files: Vec<DirEntry>,
	shared_folders: Vec<UiSharedFolder>,
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
			peer_audio_capability: None,
			peer_audio_devices: Vec::new(),
			peer_webcam_capability: None,
			peer_webcams: Vec::new(),
			puppy_eye_cameras: Vec::new(),
			puppy_eye_configured_cameras: Vec::new(),
			puppy_eye_status: String::from("Camera discovery has not run yet."),
			peer_microphones: Vec::new(),
			peer_screens: Vec::new(),
			peer_screen_status: String::from("Monitor capability not checked yet."),
			peer_files_path: String::from("/"),
			peer_files: Vec::new(),
			shared_folders: Vec::new(),
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
	media: Arc<MediaSessionManager>,
	puppy: Arc<PuppyNet>,
	state: Mutex<UiState>,
}

pub(super) struct UiContext {
	server: Arc<UiServer>,
	sessions: std::sync::Mutex<HashMap<String, UiClientSession>>,
	pending_login_tokens: std::sync::Mutex<HashMap<String, String>>,
}

struct UiClientContext {
	client_id: usize,
	handle: WguiHandle,
	page: std::sync::Mutex<Page>,
	path: std::sync::Mutex<String>,
	session_id: Option<String>,
	state: Arc<UiContext>,
}

async fn set_client_page(client: &UiClientContext, page: Page) {
	*client.page.lock().unwrap() = page.clone();
	client.state.server.set_page(page).await;
}

async fn client_snapshot(client: &UiClientContext) -> UiState {
	let mut snapshot = client.state.server.snapshot().await;
	snapshot.page = client.page.lock().unwrap().clone();
	snapshot
}

impl UiClientContext {
	fn client_id(&self) -> Option<usize> {
		Some(self.client_id)
	}

	fn navigate(&self, url: impl Into<String>) {
		let client_id = self.client_id;
		let handle = self.handle.clone();
		let url = url.into();
		tokio::spawn(async move {
			handle.navigate(client_id, &url).await;
		});
	}

	fn push_state(&self, url: impl Into<String>) {
		let client_id = self.client_id;
		let handle = self.handle.clone();
		let url = url.into();
		tokio::spawn(async move {
			handle.push_state(client_id, &url).await;
		});
	}

	fn push_state_for_client(&self, client_id: usize, url: impl Into<String>) {
		let handle = self.handle.clone();
		let url = url.into();
		tokio::spawn(async move {
			handle.push_state(client_id, &url).await;
		});
	}

	fn session_id(&self) -> Option<String> {
		self.session_id.clone()
	}
}

impl std::ops::Deref for UiClientContext {
	type Target = UiContext;

	fn deref(&self) -> &Self::Target {
		&self.state
	}
}

#[derive(Clone)]
struct UiPeer {
	id: String,
	short_id: String,
	label: String,
	node_kind: String,
	local: bool,
	status: String,
	status_color: String,
	os: String,
	uptime: String,
	version: String,
	last_seen: String,
}

#[derive(Clone)]
struct UiCpu {
	line: String,
}

#[derive(Clone)]
struct UiInterface {
	line: String,
}

#[derive(Clone, serde::Serialize)]
struct UiSelectOption {
	value: String,
	name: String,
}

#[derive(Clone)]
struct UiAudioDevice {
	line: String,
}

#[derive(Clone)]
struct UiWebcamDevice {
	id: String,
	name: String,
	selected: bool,
}

#[derive(Clone)]
struct UiPuppyEyeCamera {
	alt: String,
	camera_name: String,
	device_href: String,
	device_name: String,
	index: u32,
	media_receiver_props: UiMediaReceiverProps,
	profile_options: Vec<UiSelectOption>,
	source_id: String,
	stream_src: String,
	view_href: String,
}

#[derive(Clone)]
struct UiOnvifCamera {
	name: String,
	device_name: String,
	device_service_url: String,
	profile_token: String,
	enabled: bool,
	idle_timeout: String,
	view_href: String,
}

#[derive(Clone)]
struct UiFileRow {
	hash: String,
	line: String,
}

#[derive(Clone)]
struct UiPeerFileRow {
	name: String,
	summary: String,
	href: String,
	is_dir: bool,
}

#[derive(Clone)]
struct UiStorageRow {
	line: String,
}

#[derive(Clone)]
struct UiSharedFolder {
	path: String,
	access: String,
}

#[derive(Clone)]
struct UiMimeOption {
	name: String,
	selected: bool,
}

#[derive(Clone)]
struct UiSearchRow {
	name: String,
	path: String,
	size: String,
	replicas: String,
	peer_id: String,
	device: String,
	mime_type: String,
	modified_at: String,
}

#[derive(Clone)]
struct UiSearchRawRow {
	name: String,
	path: String,
	size: u64,
	mime_type: Option<String>,
	modified_at: Option<String>,
	peer_id: String,
}

#[derive(Clone, Default)]
struct UiTrackpadProps {
	sensitivity: f64,
}

#[derive(Clone, Default)]
struct UiMediaReceiverProps {
	auto_start: bool,
	endpoint: String,
	media_kind: String,
	revision: String,
	source_id: String,
}

#[derive(Clone, Default)]
struct UiClientSession {
	authenticated: bool,
	username: String,
	login_username: String,
	login_password: String,
	login_error: String,
	current_password: String,
	new_password: String,
	confirm_password: String,
	password_change_status: String,
	search_name_query: String,
	search_target: String,
	search_sort: String,
	search_page_size: String,
	search_visible_count: usize,
	search_raw_rows: Vec<UiSearchRawRow>,
	search_selected_mimes: Vec<String>,
	search_results: Vec<UiSearchRow>,
	search_status: String,
	search_in_progress: bool,
	search_total_peers: usize,
	search_done_peers: usize,
	search_truncated: bool,
	search_rx: Option<Arc<std::sync::Mutex<mpsc::Receiver<LiveSearchPeerEvent>>>>,
	shared_folder_path: String,
	shared_folder_access: String,
	shared_folder_status: String,
	new_user_username: String,
	new_user_password: String,
	new_user_status: String,
	new_user_modal_open: bool,
	file_preview_peer: String,
	file_preview_path: String,
	file_preview_status: String,
	file_preview_content: String,
	file_preview_image_src: String,
	file_preview_loaded: bool,
	file_preview_modal_open: bool,
	shell_peer: String,
	shell_input: String,
	shell_output: String,
	shell_status: String,
	shell_session_id: Option<u64>,
	control_text: String,
	control_status: String,
	monitor_stream_enabled: bool,
	audio_status: String,
	webcam_status: String,
	webcam_selected_device: String,
	microphone_selected_device: String,
	update_version: String,
	update_status: String,
	update_events: Vec<String>,
	update_in_progress: bool,
	update_rx: Option<Arc<std::sync::Mutex<mpsc::Receiver<UpdateProgress>>>>,
	onvif_modal_open: bool,
	onvif_camera_id: String,
	onvif_target_device: String,
	onvif_name: String,
	onvif_device_service_url: String,
	onvif_username: String,
	onvif_password: String,
	onvif_profile_token: String,
	onvif_profile_options: Vec<UiSelectOption>,
	onvif_idle_timeout: String,
	onvif_status: String,
	eye_profile_camera: String,
	eye_profile_token: String,
	eye_profile_options: Vec<UiSelectOption>,
	eye_idle_timeout: String,
	eye_profile_status: String,
	eye_grid_width: i32,
	eye_profile_options_by_camera: HashMap<String, Vec<UiSelectOption>>,
}

#[derive(Clone, Default)]
pub(super) struct UiViewState {
	page: String,
	status: String,
	authenticated: bool,
	username: String,
	login_username: String,
	login_password: String,
	login_error: String,
	current_password: String,
	new_password: String,
	confirm_password: String,
	password_change_status: String,
	search_name_query: String,
	search_target: String,
	search_target_options: Vec<UiSelectOption>,
	search_sort: String,
	search_sort_options: Vec<UiSelectOption>,
	search_page_size: String,
	search_page_size_options: Vec<UiSelectOption>,
	search_page_text: String,
	search_can_load_more: bool,
	search_in_progress: bool,
	search_selected_mimes_text: String,
	search_mime_options: Vec<UiMimeOption>,
	has_search_mime_options: bool,
	search_status: String,
	search_results: Vec<UiSearchRow>,
	search_has_results: bool,
	is_current_device: bool,
	shared_folder_path: String,
	shared_folder_access: String,
	shared_folder_access_options: Vec<UiSelectOption>,
	shared_folder_status: String,
	shared_folders: Vec<UiSharedFolder>,
	has_shared_folders: bool,
	new_user_username: String,
	new_user_password: String,
	new_user_status: String,
	new_user_modal_open: bool,
	file_preview_peer: String,
	file_preview_path: String,
	file_preview_status: String,
	file_preview_content: String,
	file_preview_image_src: String,
	file_preview_has_image: bool,
	file_preview_can_load: bool,
	file_preview_modal_open: bool,
	shell_peer: String,
	shell_input: String,
	shell_output: String,
	shell_status: String,
	shell_has_session: bool,
	control_text: String,
	control_status: String,
	trackpad_props: UiTrackpadProps,
	monitor_stream_props: UiMediaReceiverProps,
	monitor_stream_enabled: bool,
	audio_status: String,
	audio_capability_status: String,
	audio_supported: bool,
	audio_volume: i32,
	audio_volume_text: String,
	audio_muted: bool,
	audio_mute_label: String,
	selected_audio_device: String,
	audio_output_options: Vec<UiSelectOption>,
	has_audio_output_options: bool,
	microphone_status: String,
	microphone_listener_props: UiMediaReceiverProps,
	microphone_has_stream: bool,
	selected_microphone: String,
	microphone_options: Vec<UiSelectOption>,
	has_microphone_options: bool,
	webcam_status: String,
	webcam_capability_status: String,
	webcam_supported: bool,
	has_webcam_devices: bool,
	webcam_stream_props: UiMediaReceiverProps,
	webcam_has_stream: bool,
	puppy_eye_cameras: Vec<UiPuppyEyeCamera>,
	puppy_eye_has_cameras: bool,
	puppy_eye_status: String,
	eye_grid_width: i32,
	puppy_eye_viewer_found: bool,
	puppy_eye_viewer_alt: String,
	puppy_eye_viewer_name: String,
	puppy_eye_viewer_device_name: String,
	puppy_eye_viewer_device_href: String,
	puppy_eye_viewer_source_id: String,
	puppy_eye_viewer_stream_src: String,
	puppy_eye_viewer_webrtc: bool,
	puppy_eye_viewer_webrtc_props: UiMediaReceiverProps,
	puppy_eye_viewer_index: u32,
	onvif_cameras: Vec<UiOnvifCamera>,
	onvif_has_cameras: bool,
	onvif_modal_open: bool,
	onvif_editing: bool,
	onvif_target_device: String,
	onvif_target_device_options: Vec<UiSelectOption>,
	onvif_name: String,
	onvif_device_service_url: String,
	onvif_username: String,
	onvif_password: String,
	onvif_profile_token: String,
	onvif_profile_options: Vec<UiSelectOption>,
	onvif_idle_timeout: String,
	onvif_status: String,
	eye_profile_camera: String,
	eye_profile_token: String,
	eye_profile_options: Vec<UiSelectOption>,
	eye_idle_timeout: String,
	eye_profile_status: String,
	onvif_viewer_found: bool,
	onvif_viewer_enabled: bool,
	onvif_viewer_name: String,
	onvif_viewer_device_name: String,
	onvif_viewer_device_href: String,
	onvif_viewer_device_service_url: String,
	onvif_viewer_profile_token: String,
	onvif_viewer_stream_src: String,
	onvif_viewer_webrtc_props: UiMediaReceiverProps,
	onvif_viewer_index: u32,
	monitor_has_stream: bool,
	monitor_status: String,
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
	grant_command: String,
	pairing_qr_src: String,
	has_peers: bool,
	has_cpus: bool,
	has_interfaces: bool,
	has_audio_devices: bool,
	has_files: bool,
	has_peer_files: bool,
	peer_files_path: String,
	selected_peer_details_href: String,
	selected_peer_control_href: String,
	selected_peer_files_href: String,
	selected_peer_webcams_href: String,
	peer_files_parent_href: String,
	peer_files_has_parent: bool,
	has_storage_rows: bool,
	has_users: bool,
	selected_peer: String,
	peers: Vec<UiPeer>,
	cpus: Vec<UiCpu>,
	interfaces: Vec<UiInterface>,
	audio_devices: Vec<UiAudioDevice>,
	webcam_devices: Vec<UiWebcamDevice>,
	files: Vec<UiFileRow>,
	peer_files: Vec<UiPeerFileRow>,
	storage_rows: Vec<UiStorageRow>,
	users: Vec<String>,
}

pub(super) struct UiClientActions<'a> {
	ctx: &'a Arc<UiClientContext>,
}

impl<'a> UiClientActions<'a> {
	fn new(ctx: &'a Arc<UiClientContext>) -> Self {
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

	fn authenticated_username(&self) -> Option<String> {
		if let Some(session_id) = self.ctx.session_id() {
			let hash = auth::token_hash(&session_id);
			if let Ok(Some(username)) = self.ctx.state.server.puppy.http_me(&hash) {
				return Some(username);
			}
		}
		let session = self.current_session();
		session.authenticated.then_some(session.username)
	}

	pub(super) fn is_authenticated(&self) -> bool {
		self.authenticated_username().is_some()
	}
}

fn search_sort_options() -> Vec<UiSelectOption> {
	vec![
		UiSelectOption {
			value: String::from("latest"),
			name: String::from("Latest"),
		},
		UiSelectOption {
			value: String::from("name"),
			name: String::from("Name"),
		},
		UiSelectOption {
			value: String::from("size"),
			name: String::from("Size"),
		},
	]
}

fn search_page_size_options() -> Vec<UiSelectOption> {
	["25", "50", "100"]
		.into_iter()
		.map(|value| UiSelectOption {
			value: value.to_string(),
			name: value.to_string(),
		})
		.collect()
}

fn shared_folder_access_options() -> Vec<UiSelectOption> {
	vec![
		UiSelectOption {
			value: String::from("read"),
			name: String::from("Read"),
		},
		UiSelectOption {
			value: String::from("write"),
			name: String::from("Read/write"),
		},
	]
}

fn shared_folder_access_label(flags: u8) -> String {
	if flags & FLAG_WRITE != 0 {
		String::from("read/write/search")
	} else {
		String::from("read/search")
	}
}

fn search_target_options(peers: &[PeerRow]) -> Vec<UiSelectOption> {
	let mut options = vec![UiSelectOption {
		value: String::from(SEARCH_ALL_DEVICES),
		name: String::from("All devices"),
	}];
	options.extend(peers.iter().map(|peer| UiSelectOption {
		value: peer.id.clone(),
		name: if peer.local {
			format!("{} (current)", peer.name)
		} else {
			peer.name.clone()
		},
	}));
	options
}

fn puppy_eye_node_options(peers: &[PeerRow]) -> Vec<UiSelectOption> {
	peers
		.iter()
		.map(|peer| UiSelectOption {
			value: peer.id.clone(),
			name: if peer.local {
				format!("{} (current)", peer.name)
			} else {
				peer.name.clone()
			},
		})
		.collect()
}

fn search_sort(value: &str) -> SearchSort {
	match value {
		"name" => SearchSort::Name,
		"size" => SearchSort::Size,
		_ => SearchSort::Latest,
	}
}

fn search_page_size(value: &str) -> usize {
	value.parse::<usize>().unwrap_or(50).clamp(1, 250)
}

fn reset_search_visible_count(session: &mut UiClientSession) {
	session.search_visible_count = search_page_size(&session.search_page_size);
}

fn search_row_device(raw: &UiSearchRawRow) -> String {
	short_peer_id(&raw.peer_id)
}

fn search_row_to_ui(raw: UiSearchRawRow) -> UiSearchRow {
	let device = search_row_device(&raw);
	UiSearchRow {
		name: raw.name,
		path: raw.path,
		size: format_size(raw.size),
		replicas: String::from("Live result"),
		peer_id: raw.peer_id.clone(),
		device,
		mime_type: raw.mime_type.unwrap_or_else(|| String::from("unknown")),
		modified_at: raw.modified_at.unwrap_or_else(|| String::from("unknown")),
	}
}

fn rebuild_search_results(session: &mut UiClientSession) {
	let page_size = search_page_size(&session.search_page_size);
	let visible_count = session.search_visible_count.max(page_size);
	let mut rows = session.search_raw_rows.clone();
	match search_sort(&session.search_sort) {
		SearchSort::Name => rows.sort_by(|left, right| {
			left.name
				.to_ascii_lowercase()
				.cmp(&right.name.to_ascii_lowercase())
		}),
		SearchSort::Size => rows.sort_by_key(|row| std::cmp::Reverse(row.size)),
		SearchSort::Latest => rows.sort_by(|left, right| {
			right
				.modified_at
				.cmp(&left.modified_at)
				.then_with(|| left.name.cmp(&right.name))
		}),
	}
	session.search_visible_count = visible_count;
	session.search_results = rows
		.into_iter()
		.take(visible_count)
		.map(search_row_to_ui)
		.collect();
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

fn audio_device_kind_label(kind: &AudioDeviceKind) -> &'static str {
	match kind {
		AudioDeviceKind::Sink => "Output",
		AudioDeviceKind::Source => "Input",
	}
}

fn audio_device_line(device: &AudioDevice) -> String {
	let muted = if device.muted { "muted" } else { "unmuted" };
	let default = if device.is_default { "default " } else { "" };
	format!(
		"{} - {}{} | {}% | {}",
		device.name,
		default,
		audio_device_kind_label(&device.kind),
		device.volume,
		muted
	)
}

fn default_audio_output(devices: &[AudioDevice]) -> Option<&AudioDevice> {
	devices
		.iter()
		.find(|device| device.is_default && matches!(device.kind, AudioDeviceKind::Sink))
		.or_else(|| {
			devices
				.iter()
				.find(|device| matches!(device.kind, AudioDeviceKind::Sink))
		})
}

fn audio_capability_status(capability: Option<&AudioCapability>) -> String {
	match capability {
		Some(capability) => capability.message.clone(),
		None => String::from("Audio capability not checked yet."),
	}
}

fn audio_supported(capability: Option<&AudioCapability>) -> bool {
	capability
		.map(|capability| capability.supported)
		.unwrap_or(false)
}

fn webcam_capability_status(capability: Option<&MediaCapability>) -> String {
	match capability {
		Some(capability) => capability.message.clone(),
		None => String::from("Webcam capability not checked yet."),
	}
}

fn webcam_supported(capability: Option<&MediaCapability>) -> bool {
	capability
		.map(|capability| capability.supported)
		.unwrap_or(false)
}

fn format_uptime(seconds: u64) -> String {
	if seconds == 0 {
		return String::from("unknown");
	}
	let days = seconds / 86_400;
	let hours = (seconds % 86_400) / 3_600;
	let minutes = (seconds % 3_600) / 60;
	if days > 0 {
		format!("{days}d {hours}h {minutes}m")
	} else if hours > 0 {
		format!("{hours}h {minutes}m")
	} else {
		format!("{minutes}m")
	}
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
		return String::from("/devices");
	}
	let query = url::form_urlencoded::Serializer::new(String::new())
		.append_pair("path", path)
		.finish();
	format!("/devices/{peer_id}/files?{query}")
}

fn peer_details_href(peer_id: &str) -> String {
	if peer_id.is_empty() {
		String::from("/devices")
	} else {
		format!("/devices/{peer_id}")
	}
}

fn peer_control_href(peer_id: &str) -> String {
	if peer_id.is_empty() {
		String::from("/devices")
	} else {
		format!("/devices/{peer_id}/control")
	}
}

fn peer_webcams_href(peer_id: &str) -> String {
	if peer_id.is_empty() {
		String::from("/devices")
	} else {
		format!("/devices/{peer_id}/webcams")
	}
}

fn puppy_eye_stream_href(peer_id: &str, source_id: &str) -> String {
	let query = url::form_urlencoded::Serializer::new(String::new())
		.append_pair("device", source_id)
		.finish();
	format!("/api/peers/{peer_id}/webcam.mjpeg?{query}")
}

fn puppy_eye_view_href(peer_id: &str, source_id: &str) -> String {
	let source_id = url::form_urlencoded::byte_serialize(source_id.as_bytes()).collect::<String>();
	format!("/device/{peer_id}/camera/{source_id}")
}

fn media_sessions_href(peer_id: &str) -> String {
	if peer_id.is_empty() {
		String::new()
	} else {
		format!("/api/peers/{peer_id}/media/sessions")
	}
}

fn control_key(idx: u32) -> Option<&'static str> {
	match idx {
		0 => Some("Return"),
		1 => Some("Tab"),
		2 => Some("BackSpace"),
		3 => Some("Escape"),
		4 => Some("Up"),
		5 => Some("Down"),
		6 => Some("Left"),
		7 => Some("Right"),
		_ => None,
	}
}

fn json_i32(payload: &wgui::serde_json::Value, key: &str) -> Option<i32> {
	payload
		.get(key)
		.and_then(|value| value.as_i64())
		.and_then(|value| i32::try_from(value).ok())
}

fn json_mouse_button(payload: &wgui::serde_json::Value) -> MouseButton {
	match payload
		.get("button")
		.and_then(|value| value.as_str())
		.unwrap_or("left")
	{
		"middle" => MouseButton::Middle,
		"right" => MouseButton::Right,
		_ => MouseButton::Left,
	}
}

fn cookie_value(headers: &HashMap<String, String>, name: &str) -> Option<String> {
	let header = headers.get("cookie")?;
	for part in header.split(';') {
		let mut kv = part.trim().splitn(2, '=');
		let (Some(key), Some(value)) = (kv.next(), kv.next()) else {
			continue;
		};
		if key == name && !value.is_empty() {
			return Some(value.to_string());
		}
	}
	None
}

fn clear_session_cookie() -> String {
	format!("{SESSION_COOKIE}=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax")
}

fn generate_login_nonce() -> String {
	let mut bytes = [0u8; 16];
	OsRng.fill_bytes(&mut bytes);
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn json_response(status: u16, value: serde_json::Value) -> HttpResponse {
	HttpResponse::new(status, value.to_string()).header("content-type", "application/json")
}

fn redirect_response(location: &str) -> HttpResponse {
	HttpResponse::new(303, Vec::new()).header("location", location)
}

fn favicon_response() -> HttpResponse {
	HttpResponse::new(200, FAVICON_ICO.to_vec())
		.header("content-type", "image/x-icon")
		.header("cache-control", "public, max-age=86400")
}

fn session_cookie(token: &str) -> String {
	format!(
		"{}={}; HttpOnly; Path=/; Max-Age={}; SameSite=Lax",
		SESSION_COOKIE, token, SESSION_TTL_SECS
	)
}

impl UiClientActions<'_> {
	fn state_from_snapshot(&self, state: UiState) -> UiViewState {
		let session = self.current_session();
		let authenticated_username = self.authenticated_username();
		let search_targets = search_target_options(&state.peers);
		let onvif_target_device_options = puppy_eye_node_options(&state.peers);
		let onvif_target_device = if session.onvif_target_device.is_empty() {
			state
				.peers
				.iter()
				.find(|peer| peer.local)
				.or_else(|| state.peers.first())
				.map(|peer| peer.id.clone())
				.unwrap_or_default()
		} else {
			session.onvif_target_device.clone()
		};
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
				node_kind: if peer.local {
					String::from("Control node")
				} else {
					String::from("Mesh device")
				},
				local: peer.local,
				status: if peer.local {
					String::from("LOCAL")
				} else {
					String::from("ONLINE")
				},
				status_color: if peer.local {
					String::from("#7bdcff")
				} else {
					String::from("#4cff91")
				},
				os: peer.os,
				uptime: peer.uptime,
				version: peer.version,
				last_seen: if peer.local {
					String::from("now")
				} else {
					String::from("active")
				},
			})
			.collect::<Vec<_>>();
		let cpus = state
			.peer_cpus
			.into_iter()
			.map(|cpu| UiCpu {
				line: format!("{} - {:.1}% | {} Hz", cpu.name, cpu.usage, cpu.frequency_hz),
			})
			.collect::<Vec<_>>();
		let interfaces = state
			.peer_interfaces
			.into_iter()
			.map(|iface| UiInterface {
				line: format!("{} - {} | {}", iface.name, iface.mac, iface.ips.join(", ")),
			})
			.collect::<Vec<_>>();
		let audio_volume = default_audio_output(&state.peer_audio_devices)
			.map(|device| device.volume as i32)
			.unwrap_or(0);
		let audio_muted = default_audio_output(&state.peer_audio_devices)
			.map(|device| device.muted)
			.unwrap_or(false);
		let selected_audio_device = default_audio_output(&state.peer_audio_devices)
			.map(|device| device.id.clone())
			.unwrap_or_default();
		let audio_output_options = state
			.peer_audio_devices
			.iter()
			.filter(|device| matches!(device.kind, AudioDeviceKind::Sink))
			.map(|device| UiSelectOption {
				value: device.id.clone(),
				name: device.name.clone(),
			})
			.collect::<Vec<_>>();
		let is_current_device = state
			.selected_peer
			.as_deref()
			.zip(state.local_peer_id.as_deref())
			.map(|(selected, local)| selected == local)
			.unwrap_or(false);
		let is_audio_supported = audio_supported(state.peer_audio_capability.as_ref());
		let audio_capability_status = audio_capability_status(state.peer_audio_capability.as_ref());
		let audio_devices = state
			.peer_audio_devices
			.iter()
			.map(|device| UiAudioDevice {
				line: audio_device_line(device),
			})
			.collect::<Vec<_>>();
		let is_webcam_supported =
			is_current_device && webcam_supported(state.peer_webcam_capability.as_ref());
		let webcam_capability_status = if is_current_device {
			webcam_capability_status(state.peer_webcam_capability.as_ref())
		} else {
			String::from("Remote webcam streaming is not available yet.")
		};
		let selected_webcam_device = if session.webcam_selected_device.is_empty() {
			state
				.peer_webcams
				.first()
				.map(|device| device.id.clone())
				.unwrap_or_default()
		} else {
			session.webcam_selected_device.clone()
		};
		let webcam_devices = state
			.peer_webcams
			.iter()
			.map(|device| UiWebcamDevice {
				id: device.id.clone(),
				name: device.name.clone(),
				selected: device.id == selected_webcam_device,
			})
			.collect::<Vec<_>>();
		let puppy_eye_cameras = state
			.puppy_eye_cameras
			.iter()
			.enumerate()
			.map(|(index, camera)| UiPuppyEyeCamera {
				alt: format!(
					"Eye camera: {} on {}",
					camera.source.name, camera.device_name
				),
				camera_name: camera.source.name.clone(),
				device_href: peer_details_href(&camera.device_id),
				device_name: camera.device_name.clone(),
				index: index as u32,
				media_receiver_props: UiMediaReceiverProps {
					auto_start: true,
					endpoint: media_sessions_href(&camera.device_id),
					media_kind: String::from("video"),
					revision: state
						.puppy_eye_configured_cameras
						.iter()
						.find(|configured| {
							configured.device_id == camera.device_id
								&& camera.source.id == format!("onvif:{}", configured.camera.id)
						})
						.map(|configured| configured.camera.profile_token.clone())
						.unwrap_or_default(),
					source_id: camera.source.id.clone(),
				},
				profile_options: session
					.eye_profile_options_by_camera
					.get(&camera.source.id)
					.cloned()
					.unwrap_or_default(),
				source_id: camera.source.id.clone(),
				stream_src: puppy_eye_stream_href(&camera.device_id, &camera.source.id),
				view_href: puppy_eye_view_href(&camera.device_id, &camera.source.id),
			})
			.collect::<Vec<_>>();
		let puppy_eye_viewer = match &state.page {
			Page::PuppyEyeCamera { peer_id, source_id } => {
				state.puppy_eye_cameras.iter().find(|camera| {
					camera.device_id == *peer_id
						&& (source_id.is_empty() || camera.source.id == *source_id)
				})
			}
			_ => None,
		};
		let eye_grid_width = if session.eye_grid_width == 0 {
			560
		} else {
			session.eye_grid_width.clamp(320, 900)
		};
		let puppy_eye_viewer_name = puppy_eye_viewer
			.map(|camera| camera.source.name.clone())
			.unwrap_or_default();
		let puppy_eye_viewer_alt = format!("Eye fullscreen: {puppy_eye_viewer_name}");
		let puppy_eye_viewer_device_name = puppy_eye_viewer
			.map(|camera| camera.device_name.clone())
			.unwrap_or_default();
		let puppy_eye_viewer_device_href = puppy_eye_viewer
			.map(|camera| peer_details_href(&camera.device_id))
			.unwrap_or_else(|| String::from("/devices"));
		let puppy_eye_viewer_source_id = puppy_eye_viewer
			.map(|camera| camera.source.id.clone())
			.unwrap_or_default();
		let puppy_eye_viewer_stream_src = puppy_eye_viewer
			.map(|camera| puppy_eye_stream_href(&camera.device_id, &camera.source.id))
			.unwrap_or_default();
		let puppy_eye_viewer_webrtc = puppy_eye_viewer.is_some();
		let puppy_eye_viewer_webrtc_props = puppy_eye_viewer
			.map(|camera| UiMediaReceiverProps {
				auto_start: true,
				endpoint: media_sessions_href(&camera.device_id),
				media_kind: String::from("video"),
				revision: camera
					.source
					.id
					.strip_prefix("onvif:")
					.and_then(|id| {
						state
							.puppy_eye_configured_cameras
							.iter()
							.find(|configured| configured.camera.id == id)
					})
					.map(|configured| configured.camera.profile_token.clone())
					.unwrap_or_default(),
				source_id: camera.source.id.clone(),
			})
			.unwrap_or_default();
		let puppy_eye_viewer_index = puppy_eye_viewer
			.and_then(|viewer| {
				state.puppy_eye_cameras.iter().position(|camera| {
					camera.device_id == viewer.device_id && camera.source.id == viewer.source.id
				})
			})
			.unwrap_or_default() as u32;
		let onvif_cameras = state
			.puppy_eye_configured_cameras
			.iter()
			.map(|camera| UiOnvifCamera {
				name: camera.camera.name.clone(),
				device_name: camera.device_name.clone(),
				device_service_url: camera.camera.device_service_url.clone(),
				profile_token: camera.camera.profile_token.clone(),
				enabled: camera.camera.enabled,
				idle_timeout: if camera.camera.idle_timeout_seconds == 0 {
					String::from("Never stops when idle")
				} else {
					format!(
						"Stops {}s after last watcher",
						camera.camera.idle_timeout_seconds
					)
				},
				view_href: format!("/onvif-cameras/{}/{}", camera.device_id, camera.camera.id),
			})
			.collect::<Vec<_>>();
		let onvif_viewer = match &state.page {
			Page::OnvifCamera { peer_id, camera_id } => state
				.puppy_eye_configured_cameras
				.iter()
				.find(|camera| camera.device_id == *peer_id && camera.camera.id == *camera_id),
			_ => None,
		};
		let onvif_viewer_enabled = onvif_viewer
			.map(|camera| camera.camera.enabled)
			.unwrap_or(false);
		let onvif_viewer_name = onvif_viewer
			.map(|camera| camera.camera.name.clone())
			.unwrap_or_default();
		let onvif_viewer_device_name = onvif_viewer
			.map(|camera| camera.device_name.clone())
			.unwrap_or_default();
		let onvif_viewer_device_href = onvif_viewer
			.map(|camera| peer_details_href(&camera.device_id))
			.unwrap_or_else(|| String::from("/devices"));
		let onvif_viewer_device_service_url = onvif_viewer
			.map(|camera| camera.camera.device_service_url.clone())
			.unwrap_or_default();
		let onvif_viewer_profile_token = onvif_viewer
			.map(|camera| camera.camera.profile_token.clone())
			.unwrap_or_default();
		let onvif_viewer_stream_src = onvif_viewer
			.map(|camera| {
				puppy_eye_stream_href(&camera.device_id, &format!("onvif:{}", camera.camera.id))
			})
			.unwrap_or_default();
		let onvif_viewer_source_id = onvif_viewer
			.map(|camera| format!("onvif:{}", camera.camera.id))
			.unwrap_or_default();
		let onvif_viewer_webrtc_props = onvif_viewer
			.map(|camera| UiMediaReceiverProps {
				auto_start: true,
				endpoint: media_sessions_href(&camera.device_id),
				media_kind: String::from("video"),
				revision: camera.camera.profile_token.clone(),
				source_id: format!("onvif:{}", camera.camera.id),
			})
			.unwrap_or_default();
		let onvif_viewer_index = state
			.puppy_eye_cameras
			.iter()
			.position(|camera| camera.source.id == onvif_viewer_source_id)
			.unwrap_or_default() as u32;
		let selected_screen = state
			.peer_screens
			.first()
			.map(|source| source.id.clone())
			.unwrap_or_default();
		let selected_microphone = if session.microphone_selected_device.is_empty() {
			state
				.peer_microphones
				.first()
				.map(|source| source.id.clone())
				.unwrap_or_default()
		} else if state
			.peer_microphones
			.iter()
			.any(|source| source.id == session.microphone_selected_device)
		{
			session.microphone_selected_device.clone()
		} else {
			state
				.peer_microphones
				.first()
				.map(|source| source.id.clone())
				.unwrap_or_default()
		};
		let microphone_options = state
			.peer_microphones
			.iter()
			.map(|source| UiSelectOption {
				value: source.id.clone(),
				name: source.name.clone(),
			})
			.collect::<Vec<_>>();
		let monitor_stream_enabled = session.monitor_stream_enabled;
		let monitor_status = if !is_current_device {
			String::from("Remote screen streaming is not available yet.")
		} else if selected_screen.is_empty() {
			state.peer_screen_status.clone()
		} else if !monitor_stream_enabled {
			String::from("Monitor stream is available but disabled.")
		} else if let Some(source) = state.peer_screens.first() {
			format!("Viewing {}", source.name)
		} else {
			state.peer_screen_status.clone()
		};
		let files = state
			.files
			.into_iter()
			.take(20)
			.map(|entry| UiFileRow {
				hash: format_hash(&entry.hash),
				line: format!("{} - {} bytes", format_hash(&entry.hash), entry.size),
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
		let selected_peer_control_href =
			peer_control_href(state.selected_peer.as_deref().unwrap_or_default());
		let selected_peer_files_href =
			peer_files_href(state.selected_peer.as_deref().unwrap_or_default(), "/");
		let selected_peer_webcams_href =
			peer_webcams_href(state.selected_peer.as_deref().unwrap_or_default());
		let media_sessions_endpoint = state
			.selected_peer
			.as_deref()
			.filter(|_| is_current_device)
			.map(media_sessions_href)
			.unwrap_or_default();
		let microphone_status = if !is_current_device {
			String::from("Remote microphone streaming is not available yet.")
		} else {
			state
				.peer_microphones
				.iter()
				.find(|source| source.id == selected_microphone)
				.map(|source| format!("Microphone source: {}", source.name))
				.unwrap_or_else(|| String::from("No microphone listening source available."))
		};
		let storage_rows = state
			.storage
			.into_iter()
			.take(10)
			.map(|entry| UiStorageRow {
				line: format!(
					"{} - {} | {}",
					entry.node_name,
					entry.path,
					format_size(entry.size),
				),
			})
			.collect::<Vec<_>>();
		let shared_folders = state.shared_folders;
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
		let search_target = if session.search_target.is_empty() {
			String::from(SEARCH_ALL_DEVICES)
		} else {
			session.search_target.clone()
		};
		let search_sort = if session.search_sort.is_empty() {
			String::from("latest")
		} else {
			session.search_sort.clone()
		};
		let search_page_size_text = if session.search_page_size.is_empty() {
			String::from("50")
		} else {
			session.search_page_size.clone()
		};
		let search_total_rows = session.search_raw_rows.len();
		let search_visible_rows = session.search_results.len();
		let search_page_text =
			format!("Showing {search_visible_rows} of {search_total_rows} result(s)");
		UiViewState {
			page: page_label(&state.page).to_string(),
			status: state.status,
			authenticated: authenticated_username.is_some(),
			username: authenticated_username.unwrap_or(session.username),
			login_username: session.login_username,
			login_password: session.login_password,
			login_error: session.login_error,
			current_password: session.current_password,
			new_password: session.new_password,
			confirm_password: session.confirm_password,
			password_change_status: session.password_change_status,
			search_name_query: session.search_name_query,
			search_target,
			search_target_options: search_targets,
			search_sort,
			search_sort_options: search_sort_options(),
			search_page_size: search_page_size_text,
			search_page_size_options: search_page_size_options(),
			search_page_text,
			search_can_load_more: search_visible_rows < search_total_rows,
			search_in_progress: session.search_in_progress,
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
			is_current_device,
			shared_folder_path: session.shared_folder_path,
			shared_folder_access: if session.shared_folder_access.is_empty() {
				String::from("read")
			} else {
				session.shared_folder_access
			},
			shared_folder_access_options: shared_folder_access_options(),
			shared_folder_status: session.shared_folder_status,
			has_shared_folders: !shared_folders.is_empty(),
			shared_folders,
			new_user_username: session.new_user_username,
			new_user_password: session.new_user_password,
			new_user_status: session.new_user_status,
			new_user_modal_open: session.new_user_modal_open,
			file_preview_peer: session.file_preview_peer,
			file_preview_path: session.file_preview_path,
			file_preview_status: session.file_preview_status,
			file_preview_content: session.file_preview_content,
			file_preview_has_image: !session.file_preview_image_src.is_empty(),
			file_preview_image_src: session.file_preview_image_src,
			file_preview_can_load: !session.file_preview_loaded,
			file_preview_modal_open: session.file_preview_modal_open,
			shell_peer: session.shell_peer,
			shell_input: session.shell_input,
			shell_output: session.shell_output,
			shell_status: session.shell_status,
			shell_has_session: session.shell_session_id.is_some(),
			control_text: session.control_text,
			control_status: session.control_status,
			trackpad_props: UiTrackpadProps { sensitivity: 1.0 },
			monitor_stream_props: UiMediaReceiverProps {
				auto_start: true,
				endpoint: media_sessions_endpoint.clone(),
				media_kind: String::from("video"),
				revision: String::new(),
				source_id: selected_screen.clone(),
			},
			monitor_stream_enabled,
			audio_status: session.audio_status,
			audio_capability_status,
			audio_supported: is_audio_supported,
			audio_volume,
			audio_volume_text: format!("{audio_volume}%"),
			audio_muted,
			audio_mute_label: if audio_muted {
				String::from("Unmute")
			} else {
				String::from("Mute")
			},
			selected_audio_device,
			has_audio_output_options: !audio_output_options.is_empty(),
			audio_output_options,
			microphone_status,
			microphone_listener_props: UiMediaReceiverProps {
				auto_start: false,
				endpoint: media_sessions_endpoint.clone(),
				media_kind: String::from("audio"),
				revision: String::new(),
				source_id: selected_microphone.clone(),
			},
			microphone_has_stream: is_current_device && !selected_microphone.is_empty(),
			selected_microphone,
			has_microphone_options: !microphone_options.is_empty(),
			microphone_options,
			webcam_status: session.webcam_status,
			webcam_capability_status,
			webcam_supported: is_webcam_supported,
			has_webcam_devices: !webcam_devices.is_empty(),
			webcam_stream_props: UiMediaReceiverProps {
				auto_start: true,
				endpoint: media_sessions_endpoint.clone(),
				media_kind: String::from("video"),
				revision: String::new(),
				source_id: selected_webcam_device.clone(),
			},
			webcam_has_stream: is_current_device && !selected_webcam_device.is_empty(),
			puppy_eye_has_cameras: !puppy_eye_cameras.is_empty(),
			puppy_eye_cameras,
			puppy_eye_status: state.puppy_eye_status,
			eye_grid_width,
			puppy_eye_viewer_found: puppy_eye_viewer.is_some(),
			puppy_eye_viewer_alt,
			puppy_eye_viewer_name,
			puppy_eye_viewer_device_name,
			puppy_eye_viewer_device_href,
			puppy_eye_viewer_source_id,
			puppy_eye_viewer_stream_src,
			puppy_eye_viewer_webrtc,
			puppy_eye_viewer_webrtc_props,
			puppy_eye_viewer_index,
			onvif_has_cameras: !onvif_cameras.is_empty(),
			onvif_cameras,
			onvif_modal_open: session.onvif_modal_open,
			onvif_editing: !session.onvif_camera_id.is_empty(),
			onvif_target_device,
			onvif_target_device_options,
			onvif_name: session.onvif_name,
			onvif_device_service_url: session.onvif_device_service_url,
			onvif_username: session.onvif_username,
			onvif_password: session.onvif_password,
			onvif_profile_token: session.onvif_profile_token,
			onvif_profile_options: session.onvif_profile_options,
			onvif_idle_timeout: if session.onvif_idle_timeout.is_empty() {
				String::from("5")
			} else {
				session.onvif_idle_timeout
			},
			onvif_status: session.onvif_status,
			eye_profile_camera: session.eye_profile_camera,
			eye_profile_token: session.eye_profile_token,
			eye_profile_options: session.eye_profile_options,
			eye_idle_timeout: session.eye_idle_timeout,
			eye_profile_status: session.eye_profile_status,
			onvif_viewer_found: onvif_viewer.is_some(),
			onvif_viewer_enabled,
			onvif_viewer_name,
			onvif_viewer_device_name,
			onvif_viewer_device_href,
			onvif_viewer_device_service_url,
			onvif_viewer_profile_token,
			onvif_viewer_stream_src,
			onvif_viewer_webrtc_props,
			onvif_viewer_index,
			monitor_has_stream: is_current_device
				&& monitor_stream_enabled
				&& !selected_screen.is_empty(),
			monitor_status,
			update_version: session.update_version,
			update_status: session.update_status,
			update_events: session.update_events.clone(),
			has_update_events: !session.update_events.is_empty(),
			update_in_progress: session.update_in_progress,
			home_peers: format!("Devices: {}", peers.len()),
			home_files: format!("Files captured: {}", files.len()),
			home_storage: format!("Storage entries: {}", storage_rows.len()),
			home_users: format!("Users: {}", users.len()),
			current_peer: match state.local_peer_id.clone() {
				Some(peer_id) => format!("Current device: {peer_id}"),
				None => String::from("Current device: unavailable"),
			},
			grant_command: match state.local_peer_id.clone() {
				Some(peer_id) => format!("puppynet grant {peer_id} --all"),
				None => String::from("Grant command unavailable"),
			},
			pairing_qr_src: String::from("/pairing/qr.svg"),
			has_peers: !peers.is_empty(),
			has_cpus: !cpus.is_empty(),
			has_interfaces: !interfaces.is_empty(),
			has_audio_devices: !audio_devices.is_empty(),
			has_files: !files.is_empty(),
			has_peer_files: !peer_files.is_empty(),
			peer_files_path: state.peer_files_path,
			selected_peer_details_href,
			selected_peer_control_href,
			selected_peer_files_href,
			selected_peer_webcams_href,
			peer_files_has_parent: !peer_files_parent_href.is_empty(),
			peer_files_parent_href,
			has_storage_rows: !storage_rows.is_empty(),
			has_users: !users.is_empty(),
			selected_peer: state.selected_peer.unwrap_or_default(),
			peers,
			cpus,
			interfaces,
			audio_devices,
			webcam_devices,
			files,
			peer_files,
			storage_rows,
			users,
		}
	}

	pub fn logout(&self) {
		if let Some(session_id) = self.ctx.session_id() {
			let hash = auth::token_hash(&session_id);
			let _ = self.ctx.state.server.puppy.drop_session(&hash);
		}
		self.update_session(|session| {
			session.authenticated = false;
			session.username.clear();
		});
		self.ctx.navigate("/auth/logout");
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

	pub fn edit_current_password(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.current_password = value;
			session.password_change_status.clear();
		});
	}

	pub fn edit_new_password(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_password = value;
			session.password_change_status.clear();
		});
	}

	pub fn edit_confirm_password(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.confirm_password = value;
			session.password_change_status.clear();
		});
	}

	pub fn change_password(&self) {
		let Some(username) = self.authenticated_username() else {
			self.ctx.push_state("/login");
			return;
		};
		let (current_password, new_password, confirm_password) = {
			let session = self.current_session();
			(
				session.current_password,
				session.new_password,
				session.confirm_password,
			)
		};
		self.change_password_for_user(username, current_password, new_password, confirm_password);
	}

	pub fn change_password_values(
		&self,
		current_password: String,
		new_password: String,
		confirm_password: String,
	) -> bool {
		let Some(username) = self.authenticated_username() else {
			return false;
		};
		self.change_password_for_user(username, current_password, new_password, confirm_password)
	}

	fn change_password_for_user(
		&self,
		username: String,
		current_password: String,
		new_password: String,
		confirm_password: String,
	) -> bool {
		if current_password.trim().is_empty()
			|| new_password.trim().is_empty()
			|| confirm_password.trim().is_empty()
		{
			self.update_session(|session| {
				session.password_change_status = String::from("All password fields are required");
			});
			return false;
		}
		if new_password != confirm_password {
			self.update_session(|session| {
				session.password_change_status =
					String::from("New password and confirmation do not match");
			});
			return false;
		}
		match self
			.ctx
			.state
			.server
			.puppy
			.change_password(username, current_password, new_password)
		{
			Ok(()) => {
				self.update_session(|session| {
					session.current_password.clear();
					session.new_password.clear();
					session.confirm_password.clear();
					session.password_change_status = String::from("Password changed");
				});
				true
			}
			Err(err) => {
				self.update_session(|session| {
					session.password_change_status = format!("Password change failed: {err}");
				});
				false
			}
		}
	}

	pub fn login(&self) {
		let (username, password) = {
			let session = self.current_session();
			(
				session.login_username.trim().to_string(),
				session.login_password.clone(),
			)
		};
		if let Some(token) = self.login_with_credentials(username, password) {
			let nonce = generate_login_nonce();
			self.ctx
				.state
				.pending_login_tokens
				.lock()
				.unwrap()
				.insert(nonce.clone(), token);
			self.ctx.navigate(format!("/auth/finish?nonce={nonce}"));
		}
	}

	pub fn login_with_credentials(&self, username: String, password: String) -> Option<String> {
		let username = username.trim().to_string();
		if username.is_empty() || password.trim().is_empty() {
			self.update_session(|session| {
				session.login_username = username;
				session.login_error = String::from("Username and password are required");
			});
			return None;
		}
		match self
			.ctx
			.state
			.server
			.puppy
			.verify_user_credentials(&username, &password)
		{
			Ok(true) => {
				let (token, hash) = auth::generate_session_token();
				if let Err(err) =
					self.ctx
						.state
						.server
						.puppy
						.save_session(&hash, &username, SESSION_TTL_SECS)
				{
					self.update_session(|session| {
						session.login_error = format!("Login failed: {err}");
					});
					return None;
				}
				self.update_session(|session| {
					session.authenticated = true;
					session.username = username.clone();
					session.login_username = username;
					session.login_password.clear();
					session.login_error.clear();
				});
				Some(token)
			}
			Ok(false) => {
				self.update_session(|session| {
					session.login_username = username;
					session.login_error = String::from("Invalid credentials");
				});
				None
			}
			Err(err) => {
				self.update_session(|session| {
					session.login_username = username;
					session.login_error = format!("Login failed: {err}");
				});
				None
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
			self.ctx.push_state(format!("/devices/{peer_id}"));
		}
	}

	pub fn peer_back(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.handle_action(UiAction::PeerBack));
		self.ctx.push_state("/devices");
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

	pub fn refresh_audio(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			self.update_session(|session| {
				session.audio_status = String::from("Select a peer first");
			});
			return;
		};
		self.block_on(self.ctx.state.server.refresh_peer_audio(&peer_id));
	}

	pub fn refresh_webcams(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			self.update_session(|session| {
				session.webcam_status = String::from("Select a peer first");
			});
			return;
		};
		self.block_on(self.ctx.state.server.refresh_peer_webcams(&peer_id));
	}

	fn onvif_camera_config(&self) -> Result<(PeerId, OnvifCameraConfig)> {
		let session = self.current_session();
		let peer = PeerId::from_str(session.onvif_target_device.trim())
			.context("select a PuppyNet node")?;
		if session.onvif_name.trim().is_empty() {
			anyhow::bail!("camera name is required");
		}
		if session.onvif_device_service_url.trim().is_empty() {
			anyhow::bail!("ONVIF device URL is required");
		}
		let idle_timeout_seconds = if session.onvif_idle_timeout.is_empty() {
			5
		} else {
			session
				.onvif_idle_timeout
				.parse::<u32>()
				.context("invalid camera idle timeout")?
		};
		Ok((
			peer,
			OnvifCameraConfig {
				id: session.onvif_camera_id,
				name: session.onvif_name.trim().to_string(),
				device_service_url: session.onvif_device_service_url.trim().to_string(),
				username: session.onvif_username.trim().to_string(),
				password: session.onvif_password,
				profile_token: session.onvif_profile_token.trim().to_string(),
				enabled: true,
				idle_timeout_seconds,
			},
		))
	}

	pub fn open_onvif_camera_modal(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let default_peer = self
			.block_on(self.ctx.state.server.snapshot())
			.peers
			.into_iter()
			.find(|peer| peer.local)
			.map(|peer| peer.id);
		self.update_session(|session| {
			session.onvif_modal_open = true;
			session.onvif_camera_id.clear();
			session.onvif_status.clear();
			session.onvif_name.clear();
			session.onvif_device_service_url.clear();
			session.onvif_username.clear();
			session.onvif_password.clear();
			session.onvif_profile_token.clear();
			session.onvif_profile_options.clear();
			session.onvif_idle_timeout = String::from("5");
			if session.onvif_target_device.is_empty()
				&& let Some(peer) = default_peer
			{
				session.onvif_target_device = peer;
			}
		});
	}

	pub fn edit_onvif_camera(&self, index: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let camera = self
			.block_on(self.ctx.state.server.snapshot())
			.puppy_eye_configured_cameras
			.get(index as usize)
			.cloned();
		let Some(camera) = camera else { return };
		self.update_session(|session| {
			session.onvif_modal_open = true;
			session.onvif_camera_id = camera.camera.id;
			session.onvif_target_device = camera.device_id;
			session.onvif_name = camera.camera.name;
			session.onvif_device_service_url = camera.camera.device_service_url;
			session.onvif_username = camera.camera.username;
			session.onvif_password.clear();
			session.onvif_profile_token = camera.camera.profile_token;
			session.onvif_profile_options.clear();
			session.onvif_idle_timeout = camera.camera.idle_timeout_seconds.to_string();
			session.onvif_status.clear();
		});
	}

	pub fn close_onvif_camera_modal(&self) {
		self.update_session(|session| {
			session.onvif_modal_open = false;
			session.onvif_camera_id.clear();
			session.onvif_password.clear();
		});
	}

	pub fn select_onvif_target_device(&self, value: String) {
		self.update_session(|session| session.onvif_target_device = value);
	}

	pub fn edit_onvif_name(&self, value: String) {
		self.update_session(|session| session.onvif_name = value);
	}

	pub fn edit_onvif_device_service_url(&self, value: String) {
		self.update_session(|session| session.onvif_device_service_url = value);
	}

	pub fn edit_onvif_username(&self, value: String) {
		self.update_session(|session| session.onvif_username = value);
	}

	pub fn edit_onvif_password(&self, value: String) {
		self.update_session(|session| session.onvif_password = value);
	}

	pub fn edit_onvif_profile_token(&self, value: String) {
		self.update_session(|session| session.onvif_profile_token = value);
	}

	pub fn select_onvif_idle_timeout(&self, value: String) {
		self.update_session(|session| session.onvif_idle_timeout = value);
	}

	pub fn test_onvif_camera(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let result = self.onvif_camera_config().and_then(|(peer, config)| {
			self.block_on(self.ctx.state.server.puppy.test_onvif_camera(peer, config))
		});
		self.update_session(|session| match result {
			Ok(result) => {
				session.onvif_profile_token = result.camera.profile_token;
				session.onvif_profile_options = result
					.profiles
					.into_iter()
					.map(|profile| {
						let resolution = if profile.width > 0 && profile.height > 0 {
							format!("{}x{}", profile.width, profile.height)
						} else {
							String::from("unknown resolution")
						};
						let name = if profile.name.is_empty() {
							format!("{resolution} ({})", profile.token)
						} else {
							format!("{} — {resolution}", profile.name)
						};
						UiSelectOption {
							value: profile.token,
							name,
						}
					})
					.collect();
				session.onvif_status = if session.onvif_profile_options.is_empty() {
					result.message
				} else {
					format!("{}. Select a resolution, then save.", result.message)
				};
			}
			Err(error) => session.onvif_status = format!("Camera test failed: {error}"),
		});
	}

	pub fn save_onvif_camera(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let result = self.onvif_camera_config().and_then(|(peer, config)| {
			self.block_on(self.ctx.state.server.puppy.save_onvif_camera(peer, config))
		});
		let saved = result.is_ok();
		self.update_session(|session| match result {
			Ok(result) => {
				session.onvif_modal_open = false;
				session.onvif_camera_id.clear();
				session.onvif_name.clear();
				session.onvif_device_service_url.clear();
				session.onvif_username.clear();
				session.onvif_password.clear();
				session.onvif_profile_token.clear();
				session.onvif_profile_options.clear();
				session.onvif_idle_timeout.clear();
				session.onvif_status = result.message;
			}
			Err(error) => session.onvif_status = format!("Save camera failed: {error}"),
		});
		if saved {
			self.block_on(self.ctx.state.server.refresh_puppy_eye());
		}
	}

	pub fn delete_onvif_camera(&self, index: usize) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let camera = self
			.block_on(self.ctx.state.server.snapshot())
			.puppy_eye_configured_cameras
			.get(index)
			.cloned();
		let result = camera
			.ok_or_else(|| anyhow::anyhow!("camera was not found"))
			.and_then(|camera| {
				let peer = PeerId::from_str(&camera.device_id)?;
				self.block_on(
					self.ctx
						.state
						.server
						.puppy
						.delete_onvif_camera(peer, camera.camera.id),
				)
			});
		let deleted = result.is_ok();
		self.update_session(|session| {
			session.onvif_status = match result {
				Ok(result) => result.message,
				Err(error) => format!("Delete camera failed: {error}"),
			};
		});
		if deleted {
			self.block_on(self.ctx.state.server.refresh_puppy_eye());
		}
	}

	pub fn refresh_puppy_eye(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.block_on(self.ctx.state.server.refresh_puppy_eye());
		self.load_all_eye_camera_profiles();
	}

	pub fn set_eye_grid_width(&self, value: i32) {
		self.update_session(|session| session.eye_grid_width = value.clamp(320, 900));
	}

	pub fn load_eye_camera_profiles(&self, index: u32) {
		let camera = self
			.block_on(self.ctx.state.server.snapshot())
			.puppy_eye_cameras
			.get(index as usize)
			.cloned();
		let Some(camera) = camera else { return };
		let Some(id) = camera.source.id.strip_prefix("onvif:") else {
			return;
		};
		let config = OnvifCameraConfig {
			id: id.to_string(),
			name: String::new(),
			device_service_url: String::new(),
			username: String::new(),
			password: String::new(),
			profile_token: String::new(),
			enabled: true,
			idle_timeout_seconds: 5,
		};
		let result = self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.test_onvif_camera(camera.device_id.parse().unwrap(), config),
		);
		self.update_session(|session| match result {
			Ok(result) => {
				session.eye_profile_camera = camera.source.id.clone();
				session.eye_profile_token = result.camera.profile_token;
				session.eye_idle_timeout = result.camera.idle_timeout_seconds.to_string();
				let options = result
					.profiles
					.into_iter()
					.map(|profile| UiSelectOption {
						value: profile.token,
						name: format!("{}x{} — {}", profile.width, profile.height, profile.name),
					})
					.collect::<Vec<_>>();
				session.eye_profile_options = options.clone();
				session
					.eye_profile_options_by_camera
					.insert(camera.source.id, options);
				session.eye_profile_status = result.message;
			}
			Err(error) => session.eye_profile_status = format!("Profile load failed: {error}"),
		});
	}

	fn load_all_eye_camera_profiles(&self) {
		let cameras = self
			.block_on(self.ctx.state.server.snapshot())
			.puppy_eye_cameras;
		for (index, camera) in cameras.iter().enumerate() {
			if camera.source.id.starts_with("onvif:") {
				self.load_eye_camera_profiles(index as u32);
			} else if camera.source.id.starts_with("/dev/video") {
				let options = webcam::webcam_resolutions(&camera.source.id)
					.into_iter()
					.map(|(width, height)| UiSelectOption {
						value: format!("{width}x{height}"),
						name: format!("{width}x{height}"),
					})
					.collect();
				self.update_session(|session| {
					session
						.eye_profile_options_by_camera
						.insert(camera.source.id.clone(), options);
				});
			}
		}
	}

	pub fn select_eye_camera_profile(&self, index: u32, value: String) {
		self.update_session(|session| session.eye_profile_token = value);
		self.save_eye_camera_profile(index);
	}

	pub fn select_eye_camera_idle_timeout(&self, value: String) {
		self.update_session(|session| session.eye_idle_timeout = value);
	}

	pub fn save_eye_camera_profile(&self, index: u32) {
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let camera = snapshot.puppy_eye_cameras.get(index as usize).cloned();
		let Some(camera) = camera else { return };
		let Some(id) = camera.source.id.strip_prefix("onvif:") else {
			return;
		};
		let session = self.current_session();
		let profile_token = session.eye_profile_token;
		let idle_timeout_seconds = snapshot
			.puppy_eye_configured_cameras
			.iter()
			.find(|configured| {
				configured.device_id == camera.device_id && configured.camera.id == id
			})
			.map(|configured| configured.camera.idle_timeout_seconds)
			.unwrap_or(5);
		let config = OnvifCameraConfig {
			id: id.to_string(),
			name: String::new(),
			device_service_url: String::new(),
			username: String::new(),
			password: String::new(),
			profile_token,
			enabled: true,
			idle_timeout_seconds,
		};
		let result = self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.save_onvif_camera(camera.device_id.parse().unwrap(), config),
		);
		let saved_camera = result.as_ref().ok().map(|result| result.camera.clone());
		self.update_session(|session| match result {
			Ok(result) => session.eye_profile_status = result.message,
			Err(error) => session.eye_profile_status = format!("Profile save failed: {error}"),
		});
		if let Some(saved_camera) = saved_camera {
			self.block_on(self.ctx.state.server.refresh_puppy_eye());
			self.block_on(async {
				let mut state = self.ctx.state.server.state.lock().await;
				if let Some(configured) =
					state
						.puppy_eye_configured_cameras
						.iter_mut()
						.find(|configured| {
							configured.device_id == camera.device_id
								&& configured.camera.id == saved_camera.id
						}) {
					configured.camera = saved_camera;
				}
			});
			self.load_all_eye_camera_profiles();
		}
	}

	pub fn open_puppy_eye_camera(&self, index: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let camera = self
			.block_on(self.ctx.state.server.snapshot())
			.puppy_eye_cameras
			.get(index as usize)
			.cloned();
		if let Some(camera) = camera {
			self.ctx
				.push_state(puppy_eye_view_href(&camera.device_id, &camera.source.id));
		}
	}

	pub fn view_webcam(&self, idx: u32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(device) = snapshot.peer_webcams.get(idx as usize) else {
			self.update_session(|session| {
				session.webcam_status = String::from("Unknown webcam device");
			});
			return;
		};
		let device_id = device.id.clone();
		let device_name = device.name.clone();
		self.update_session(|session| {
			session.webcam_selected_device = device_id;
			session.webcam_status = format!("Viewing {device_name}");
		});
	}

	pub fn select_microphone(&self, source_id: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(source) = snapshot
			.peer_microphones
			.iter()
			.find(|source| source.id == source_id)
		else {
			self.update_session(|session| {
				session.microphone_selected_device.clear();
			});
			return;
		};
		let id = source.id.clone();
		self.update_session(|session| {
			session.microphone_selected_device = id;
		});
	}

	pub fn set_audio_volume(&self, value: i32) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			self.update_session(|session| {
				session.audio_status = String::from("Select a peer first");
			});
			return;
		};
		if !audio_supported(snapshot.peer_audio_capability.as_ref()) {
			self.update_session(|session| {
				session.audio_status =
					audio_capability_status(snapshot.peer_audio_capability.as_ref());
			});
			return;
		}
		let Ok(peer) = PeerId::from_str(&peer_id) else {
			self.update_session(|session| {
				session.audio_status = String::from("Invalid selected peer");
			});
			return;
		};
		let volume = value.clamp(0, 100) as u8;
		match self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.set_audio_volume(peer, None, volume),
		) {
			Ok(devices) => {
				self.block_on(self.ctx.state.server.set_peer_audio_devices(devices));
				self.update_session(|session| {
					session.audio_status = format!("Set volume to {volume}%");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.audio_status = format!("Failed to set volume: {err}");
				});
			}
		}
	}

	pub fn toggle_audio_mute(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			self.update_session(|session| {
				session.audio_status = String::from("Select a peer first");
			});
			return;
		};
		if !audio_supported(snapshot.peer_audio_capability.as_ref()) {
			self.update_session(|session| {
				session.audio_status =
					audio_capability_status(snapshot.peer_audio_capability.as_ref());
			});
			return;
		}
		let Ok(peer) = PeerId::from_str(&peer_id) else {
			self.update_session(|session| {
				session.audio_status = String::from("Invalid selected peer");
			});
			return;
		};
		let muted = !default_audio_output(&snapshot.peer_audio_devices)
			.map(|device| device.muted)
			.unwrap_or(false);
		match self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.set_audio_muted(peer, None, muted),
		) {
			Ok(devices) => {
				self.block_on(self.ctx.state.server.set_peer_audio_devices(devices));
				self.update_session(|session| {
					session.audio_status = if muted {
						String::from("Muted default output")
					} else {
						String::from("Unmuted default output")
					};
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.audio_status = format!("Failed to change mute: {err}");
				});
			}
		}
	}

	pub fn select_audio_device(&self, device_id: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let Some(peer_id) = snapshot.selected_peer else {
			self.update_session(|session| {
				session.audio_status = String::from("Select a peer first");
			});
			return;
		};
		if !audio_supported(snapshot.peer_audio_capability.as_ref()) {
			self.update_session(|session| {
				session.audio_status =
					audio_capability_status(snapshot.peer_audio_capability.as_ref());
			});
			return;
		}
		let Some(device) = snapshot
			.peer_audio_devices
			.iter()
			.find(|device| device.id == device_id)
		else {
			self.update_session(|session| {
				session.audio_status = String::from("Unknown audio device");
			});
			return;
		};
		if !matches!(device.kind, AudioDeviceKind::Sink) {
			self.update_session(|session| {
				session.audio_status = String::from("Only output devices can be selected");
			});
			return;
		}
		let Ok(peer) = PeerId::from_str(&peer_id) else {
			self.update_session(|session| {
				session.audio_status = String::from("Invalid selected peer");
			});
			return;
		};
		let device_name = device.name.clone();
		match self.block_on(
			self.ctx
				.state
				.server
				.puppy
				.set_default_audio_device(peer, device_id),
		) {
			Ok(devices) => {
				self.block_on(self.ctx.state.server.set_peer_audio_devices(devices));
				self.update_session(|session| {
					session.audio_status = format!("Selected {device_name}");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.audio_status = format!("Failed to select audio output: {err}");
				});
			}
		}
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
					session.file_preview_loaded = false;
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
					session.file_preview_loaded = false;
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_modal_open = true;
					session.file_preview_status = format!("Failed to resolve file: {err}");
					session.file_preview_content.clear();
					session.file_preview_image_src.clear();
					session.file_preview_loaded = false;
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
			session.file_preview_loaded = false;
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

	pub fn edit_shared_folder_path(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.shared_folder_path = value;
			session.shared_folder_status.clear();
		});
	}

	pub fn select_shared_folder_access(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.shared_folder_access = value;
			session.shared_folder_status.clear();
		});
	}

	pub fn add_shared_folder(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		if snapshot.selected_peer != snapshot.local_peer_id {
			self.update_session(|session| {
				session.shared_folder_status =
					String::from("Allowed folders can only be changed on the current device");
			});
			return;
		}
		let (path, access) = {
			let session = self.current_session();
			(
				session.shared_folder_path.trim().to_string(),
				session.shared_folder_access,
			)
		};
		if path.is_empty() {
			self.update_session(|session| {
				session.shared_folder_status = String::from("Path is required");
			});
			return;
		}
		let result = self.block_on(async {
			tokio::time::timeout(std::time::Duration::from_secs(3), async {
				if access == "write" {
					self.ctx
						.state
						.server
						.puppy
						.share_read_write_folder_async(&path)
						.await
				} else {
					self.ctx
						.state
						.server
						.puppy
						.share_read_only_folder_async(&path)
						.await
				}
			})
			.await
			.unwrap_or_else(|_| Err(anyhow::anyhow!("daemon did not respond in time")))
		});
		match result {
			Ok(()) => {
				self.block_on(self.ctx.state.server.refresh_peers());
				self.update_session(|session| {
					session.shared_folder_path.clear();
					session.shared_folder_access = String::from("read");
					session.shared_folder_status = format!("Added allowed folder {path}");
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.shared_folder_status = format!("Failed to add folder: {err}");
				});
			}
		}
	}

	pub fn select_search_target(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_target = value;
			reset_search_visible_count(session);
			rebuild_search_results(session);
		});
	}

	pub fn select_search_sort(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_sort = value;
			reset_search_visible_count(session);
			rebuild_search_results(session);
		});
	}

	pub fn select_search_page_size(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.search_page_size = value;
			reset_search_visible_count(session);
			rebuild_search_results(session);
		});
	}

	pub fn search_load_more(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			let page_size = search_page_size(&session.search_page_size);
			session.search_visible_count = session
				.search_visible_count
				.max(page_size)
				.saturating_add(page_size);
			rebuild_search_results(session);
		});
	}

	fn watch_live_search(&self, rx: Arc<std::sync::Mutex<mpsc::Receiver<LiveSearchPeerEvent>>>) {
		let Some(client_id) = self.ctx.client_id() else {
			return;
		};
		let session_key = self.session_key();
		let route_path = self.ctx.path.lock().unwrap().clone();
		let ctx = Arc::clone(self.ctx);
		std::thread::spawn(move || {
			loop {
				let event = match rx.lock() {
					Ok(stream) => stream.recv(),
					Err(err) => {
						if let Ok(mut sessions) = ctx.sessions.lock()
							&& let Some(session) = sessions.get_mut(&session_key)
						{
							session.search_status = format!("Search stream lock failed: {err}");
							session.search_in_progress = false;
							if session
								.search_rx
								.as_ref()
								.map(|current| Arc::ptr_eq(current, &rx))
								.unwrap_or(false)
							{
								session.search_rx = None;
							}
						}
						ctx.push_state_for_client(client_id, route_path.clone());
						break;
					}
				};
				let event = match event {
					Ok(event) => event,
					Err(_) => {
						if let Ok(mut sessions) = ctx.sessions.lock()
							&& let Some(session) = sessions.get_mut(&session_key)
							&& session
								.search_rx
								.as_ref()
								.map(|current| Arc::ptr_eq(current, &rx))
								.unwrap_or(false)
						{
							session.search_in_progress = false;
							session.search_rx = None;
							rebuild_search_results(session);
							if session.search_status == "Search started" {
								session.search_status =
									format!("Found {} result(s)", session.search_raw_rows.len());
							}
						}
						ctx.push_state_for_client(client_id, route_path.clone());
						break;
					}
				};
				if let Ok(mut sessions) = ctx.sessions.lock()
					&& let Some(session) = sessions.get_mut(&session_key)
				{
					if !session
						.search_rx
						.as_ref()
						.map(|current| Arc::ptr_eq(current, &rx))
						.unwrap_or(false)
					{
						break;
					}
					match event.event {
						SearchEvent::Rows { rows } => {
							session.search_raw_rows.extend(rows.into_iter().map(|row| {
								UiSearchRawRow {
									name: row.name,
									path: row.path,
									size: row.size,
									mime_type: row.mime_type,
									modified_at: row.modified_at,
									peer_id: event.peer.to_string(),
								}
							}));
							rebuild_search_results(session);
							session.search_status =
								format!("Searching... {} result(s)", session.search_raw_rows.len());
						}
						SearchEvent::Progress { visited, matched } => {
							session.search_status =
								format!("Searching... visited {visited}, matched {matched}");
						}
						SearchEvent::Finished { total, truncated } => {
							session.search_done_peers = session.search_done_peers.saturating_add(1);
							session.search_truncated |= truncated;
							rebuild_search_results(session);
							if session.search_done_peers >= session.search_total_peers {
								session.search_in_progress = false;
								session.search_rx = None;
								session.search_status = if session.search_truncated {
									format!(
										"Found {} result(s); result set was truncated",
										session.search_raw_rows.len()
									)
								} else {
									format!("Found {} result(s)", session.search_raw_rows.len())
								};
							} else {
								session.search_status = format!(
									"Device finished with {total} result(s); {}/{} done",
									session.search_done_peers, session.search_total_peers
								);
							}
						}
						SearchEvent::Failed { error } => {
							session.search_done_peers = session.search_done_peers.saturating_add(1);
							rebuild_search_results(session);
							if session.search_done_peers >= session.search_total_peers {
								session.search_in_progress = false;
								session.search_rx = None;
							}
							session.search_status = format!("Search failed on a device: {error}");
						}
					}
				}
				ctx.push_state_for_client(client_id, route_path.clone());
			}
		});
	}

	pub fn run_search(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		let snapshot = self.block_on(self.ctx.state.server.snapshot());
		let session = self.current_session();
		let query = session.search_name_query.clone();
		let target = if session.search_target.is_empty() {
			String::from(SEARCH_ALL_DEVICES)
		} else {
			session.search_target.clone()
		};
		let peer_ids = snapshot
			.peers
			.iter()
			.filter(|peer| target == SEARCH_ALL_DEVICES || peer.id == target)
			.filter_map(|peer| PeerId::from_str(&peer.id).ok())
			.collect::<Vec<_>>();
		if peer_ids.is_empty() {
			self.update_session(|session| {
				session.search_raw_rows.clear();
				session.search_results.clear();
				session.search_status = String::from("No target devices available");
				session.search_in_progress = false;
				session.search_rx = None;
			});
			return;
		}
		let total_peers = peer_ids.len();
		let page_size = search_page_size(&session.search_page_size);
		let args = LiveSearchArgs {
			name_query: if query.trim().is_empty() {
				None
			} else {
				Some(query.clone())
			},
			mime_types: session.search_selected_mimes.clone(),
			page: 0,
			page_size,
			sort: search_sort(&session.search_sort),
			sort_desc: true,
		};
		match self
			.ctx
			.state
			.server
			.puppy
			.live_search_peers(peer_ids, args)
		{
			Ok(rx) => {
				let watch_rx = Arc::clone(&rx);
				self.update_session(|session| {
					session.search_target = target;
					session.search_visible_count = page_size;
					session.search_raw_rows.clear();
					session.search_results.clear();
					session.search_status = String::from("Search started");
					session.search_in_progress = true;
					session.search_total_peers = total_peers;
					session.search_done_peers = 0;
					session.search_truncated = false;
					session.search_rx = Some(rx);
				});
				self.watch_live_search(watch_rx);
			}
			Err(err) => {
				self.update_session(|session| {
					session.search_status = format!("Search failed: {err}");
					session.search_raw_rows.clear();
					session.search_results.clear();
					session.search_in_progress = false;
					session.search_rx = None;
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
				session.file_preview_loaded = false;
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
			session.file_preview_loaded = false;
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
			session.file_preview_loaded = false;
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
				session.file_preview_loaded = false;
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
							session.file_preview_loaded = false;
						});
						return;
					}
				}
			}
		};
		let peer_label = peer.to_string();
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
						session.file_preview_peer = peer_label.clone();
						session.file_preview_status = format!(
							"Loaded image preview ({}x{})",
							thumbnail.width, thumbnail.height
						);
						session.file_preview_image_src =
							format!("data:{};base64,{encoded}", thumbnail.mime_type);
						session.file_preview_content.clear();
						session.file_preview_loaded = true;
					});
				}
				Err(err) => {
					self.update_session(|session| {
						session.file_preview_peer = peer_label.clone();
						session.file_preview_status =
							format!("Failed to load image preview: {err}");
						session.file_preview_content.clear();
						session.file_preview_image_src.clear();
						session.file_preview_loaded = false;
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
					session.file_preview_peer = peer_label.clone();
					session.file_preview_status = format!(
						"Loaded {} byte(s){}",
						chunk.data.len(),
						if chunk.eof { "" } else { " (truncated)" }
					);
					session.file_preview_content = preview;
					session.file_preview_image_src.clear();
					session.file_preview_loaded = true;
				});
			}
			Err(err) => {
				self.update_session(|session| {
					session.file_preview_peer = peer_label;
					session.file_preview_status = format!("Failed to read file: {err}");
					session.file_preview_content.clear();
					session.file_preview_image_src.clear();
					session.file_preview_loaded = false;
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

	fn selected_desktop_peer(&self) -> Option<PeerId> {
		let selected_peer = self
			.block_on(self.ctx.state.server.snapshot())
			.selected_peer;
		let Some(selected_peer) = selected_peer else {
			self.update_session(|session| {
				session.control_status = String::from("Select a peer first");
			});
			return None;
		};
		match PeerId::from_str(&selected_peer) {
			Ok(peer) => Some(peer),
			Err(_) => {
				self.update_session(|session| {
					session.control_status = String::from("Invalid selected peer");
				});
				None
			}
		}
	}

	fn selected_desktop_peer_quiet(&self) -> Option<PeerId> {
		self.block_on(self.ctx.state.server.snapshot())
			.selected_peer
			.and_then(|peer| PeerId::from_str(&peer).ok())
	}

	fn send_desktop_input(&self, input: DesktopInput, success: impl Into<String>) -> bool {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return false;
		}
		let Some(peer) = self.selected_desktop_peer() else {
			return false;
		};
		match self.block_on(self.ctx.state.server.puppy.desktop_input(peer, input)) {
			Ok(()) => {
				let success = success.into();
				self.update_session(|session| {
					session.control_status = success;
				});
				true
			}
			Err(err) => {
				self.update_session(|session| {
					session.control_status = format!("Control input failed: {err}");
				});
				false
			}
		}
	}

	fn send_mouse_move_input(&self, dx: i32, dy: i32) {
		let Some(peer) = self.selected_desktop_peer_quiet() else {
			return;
		};
		let puppy = Arc::clone(&self.ctx.state.server.puppy);
		tokio::spawn(async move {
			if let Err(err) = puppy
				.desktop_input(peer, DesktopInput::MouseMove { dx, dy })
				.await
			{
				log::warn!("mouse move input failed: {err}");
			}
		});
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

	pub fn move_peer_mouse(&self, payload: wgui::serde_json::Value) {
		let dx = json_i32(&payload, "dx").unwrap_or(0).clamp(-500, 500);
		let dy = json_i32(&payload, "dy").unwrap_or(0).clamp(-500, 500);
		if dx == 0 && dy == 0 {
			return;
		}
		self.send_mouse_move_input(dx, dy);
	}

	pub fn toggle_monitor_stream(&self) {
		self.update_session(|session| {
			session.monitor_stream_enabled = !session.monitor_stream_enabled;
		});
	}

	pub fn scroll_peer_mouse(&self, payload: wgui::serde_json::Value) {
		let amount = json_i32(&payload, "amount").unwrap_or(0).clamp(-20, 20);
		if amount == 0 {
			return;
		}
		self.send_desktop_input(
			DesktopInput::MouseScroll { amount },
			format!("Scrolled mouse by {amount}"),
		);
	}

	pub fn click_peer_mouse(&self, payload: wgui::serde_json::Value) {
		let button = json_mouse_button(&payload);
		self.send_desktop_input(DesktopInput::MouseClick { button }, "Mouse click sent");
	}

	pub fn press_peer_mouse(&self, payload: wgui::serde_json::Value) {
		let button = json_mouse_button(&payload);
		self.send_desktop_input(DesktopInput::MousePress { button }, "Mouse button pressed");
	}

	pub fn release_peer_mouse(&self, payload: wgui::serde_json::Value) {
		let button = json_mouse_button(&payload);
		self.send_desktop_input(
			DesktopInput::MouseRelease { button },
			"Mouse button released",
		);
	}

	pub fn edit_control_text(&self, value: String) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.control_text = value;
		});
	}

	pub fn send_control_text(&self) {
		let text = self.current_session().control_text;
		if text.is_empty() {
			return;
		}
		if self.send_desktop_input(DesktopInput::KeyboardText { text }, "Text sent") {
			self.update_session(|session| {
				session.control_text.clear();
			});
		}
	}

	pub fn send_control_key(&self, idx: u32) {
		let Some(key) = control_key(idx) else {
			self.update_session(|session| {
				session.control_status = String::from("Unknown key");
			});
			return;
		};
		self.send_desktop_input(
			DesktopInput::KeyboardKey {
				key: key.to_string(),
			},
			format!("{key} sent"),
		);
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

	fn watch_peer_update(&self, rx: Arc<std::sync::Mutex<mpsc::Receiver<UpdateProgress>>>) {
		let Some(client_id) = self.ctx.client_id() else {
			return;
		};
		let session_key = self.session_key();
		let route_path = self.ctx.path.lock().unwrap().clone();
		let ctx = Arc::clone(self.ctx);
		std::thread::spawn(move || {
			loop {
				let event = match rx.lock() {
					Ok(stream) => stream.recv(),
					Err(err) => {
						if let Ok(mut sessions) = ctx.sessions.lock()
							&& let Some(session) = sessions.get_mut(&session_key)
						{
							session.update_status = format!("Update stream lock failed: {err}");
							session.update_in_progress = false;
							if session
								.update_rx
								.as_ref()
								.map(|current| Arc::ptr_eq(current, &rx))
								.unwrap_or(false)
							{
								session.update_rx = None;
							}
						}
						ctx.push_state_for_client(client_id, route_path.clone());
						break;
					}
				};
				let event = match event {
					Ok(event) => event,
					Err(_) => {
						if let Ok(mut sessions) = ctx.sessions.lock()
							&& let Some(session) = sessions.get_mut(&session_key)
							&& session
								.update_rx
								.as_ref()
								.map(|current| Arc::ptr_eq(current, &rx))
								.unwrap_or(false)
						{
							session.update_in_progress = false;
							session.update_rx = None;
						}
						ctx.push_state_for_client(client_id, route_path.clone());
						break;
					}
				};
				let completed = matches!(
					event,
					UpdateProgress::Completed { .. }
						| UpdateProgress::Failed { .. }
						| UpdateProgress::AlreadyUpToDate { .. }
				);
				let line = format_update_progress(&event);
				if let Ok(mut sessions) = ctx.sessions.lock()
					&& let Some(session) = sessions.get_mut(&session_key)
				{
					if !session
						.update_rx
						.as_ref()
						.map(|current| Arc::ptr_eq(current, &rx))
						.unwrap_or(false)
					{
						break;
					}
					session.update_status = line.clone();
					session.update_events.push(line);
					if completed {
						session.update_in_progress = false;
						session.update_rx = None;
					}
				}
				ctx.push_state_for_client(client_id, route_path.clone());
				if completed {
					break;
				}
			}
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
				let update_rx = Arc::clone(&rx);
				self.update_session(|session| {
					session.update_rx = Some(rx);
					session.update_in_progress = true;
					session.update_events.clear();
					session.update_status = String::from("Update started");
				});
				self.watch_peer_update(update_rx);
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

	pub fn open_new_user_modal(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_modal_open = true;
			session.new_user_status.clear();
		});
	}

	pub fn close_new_user_modal(&self) {
		if !self.is_authenticated() {
			self.ctx.push_state("/login");
			return;
		}
		self.update_session(|session| {
			session.new_user_modal_open = false;
			session.new_user_status.clear();
			session.new_user_password.clear();
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
		if self.create_user_values(username, password) {
			self.block_on(self.ctx.state.server.refresh_users());
		}
	}

	pub fn create_user_values(&self, username: String, password: String) -> bool {
		let username = username.trim().to_string();
		if username.is_empty() || password.trim().is_empty() {
			self.update_session(|session| {
				session.new_user_modal_open = true;
				session.new_user_username = username;
				session.new_user_status = String::from("Username and password are required");
			});
			return false;
		}
		match self.ctx.state.server.puppy.create_user(username, password) {
			Ok(()) => {
				self.update_session(|session| {
					session.new_user_username.clear();
					session.new_user_password.clear();
					session.new_user_modal_open = false;
					session.new_user_status = String::from("User created");
				});
				true
			}
			Err(err) => {
				self.update_session(|session| {
					session.new_user_modal_open = true;
					session.new_user_status = format!("Create user failed: {err}");
				});
				false
			}
		}
	}

	pub async fn create_user_values_async(&self, username: String, password: String) -> bool {
		let username = username.trim().to_string();
		if username.is_empty() || password.trim().is_empty() {
			self.update_session(|session| {
				session.new_user_modal_open = true;
				session.new_user_username = username;
				session.new_user_status = String::from("Username and password are required");
			});
			return false;
		}
		match self
			.ctx
			.state
			.server
			.puppy
			.create_user_async(username, password)
			.await
		{
			Ok(()) => {
				self.update_session(|session| {
					session.new_user_username.clear();
					session.new_user_password.clear();
					session.new_user_modal_open = false;
					session.new_user_status = String::from("User created");
				});
				true
			}
			Err(err) => {
				self.update_session(|session| {
					session.new_user_modal_open = true;
					session.new_user_status = format!("Create user failed: {err}");
				});
				false
			}
		}
	}

	pub async fn delete_user_values_async(&self, username: String) -> bool {
		let username = username.trim().to_string();
		if username.is_empty() {
			self.update_session(|session| {
				session.new_user_status = String::from("User not found");
			});
			return false;
		}
		if self.authenticated_username().as_deref() == Some(username.as_str()) {
			self.update_session(|session| {
				session.new_user_status = String::from("Cannot delete the current user");
			});
			return false;
		}
		match self
			.ctx
			.state
			.server
			.puppy
			.delete_user_async(username.clone())
			.await
		{
			Ok(()) => {
				self.update_session(|session| {
					session.new_user_status = format!("Deleted user {username}");
				});
				true
			}
			Err(err) => {
				self.update_session(|session| {
					session.new_user_status = format!("Delete user failed: {err}");
				});
				false
			}
		}
	}
}

struct UiServerActions<'a> {
	server: &'a UiServer,
}

impl<'a> UiServerActions<'a> {
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
	fn new(puppy: Arc<PuppyNet>) -> Result<Self> {
		Ok(Self {
			media: MediaSessionManager::new()?,
			puppy,
			state: Mutex::new(UiState::new()),
		})
	}

	async fn refresh_all(&self) {
		self.refresh_peers().await;
		self.refresh_files().await;
		self.refresh_storage().await;
		self.refresh_users().await;
		self.refresh_search_mime_types().await;
	}

	async fn refresh_search_mime_types(&self) {
		let mut merged = COMMON_SEARCH_MIME_TYPES
			.iter()
			.map(|mime| mime.to_string())
			.collect::<BTreeSet<_>>();
		match self.puppy.get_mime_types() {
			Ok(mimes) => {
				merged.extend(mimes);
				let mut state = self.state.lock().await;
				state.search_mime_types = merged.into_iter().collect();
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.search_mime_types = merged.into_iter().collect();
				state.status = format!("Failed to load mime types: {err}");
			}
		}
	}

	async fn peer_info(&self, peer_id: &str) -> PeerInfo {
		let Ok(peer) = PeerId::from_str(peer_id) else {
			return PeerInfo {
				version: String::from("unknown"),
				os: String::from("unknown"),
				uptime_seconds: 0,
			};
		};
		tokio::time::timeout(
			std::time::Duration::from_millis(1500),
			self.puppy.peer_info(peer),
		)
		.await
		.ok()
		.and_then(Result::ok)
		.map(|mut info| {
			if info.os.trim().is_empty() {
				info.os = String::from("unknown");
			}
			info
		})
		.unwrap_or_else(|| PeerInfo {
			version: String::from("unknown"),
			os: String::from("unknown"),
			uptime_seconds: 0,
		})
	}

	async fn refresh_peers(&self) {
		match self.puppy.state_snapshot().await {
			Some(snapshot) => {
				let local_id = snapshot.me.to_string();
				let mut peers = Vec::new();
				for peer in &snapshot.peers {
					let id = peer.id.to_string();
					let info = self.peer_info(&id).await;
					peers.push(PeerRow {
						id: peer.id.to_string(),
						name: peer.name.clone().unwrap_or_else(|| "Unnamed".to_string()),
						local: peer.id.to_string() == local_id,
						version: info.version,
						os: info.os,
						uptime: format_uptime(info.uptime_seconds),
					});
				}
				if !peers.iter().any(|peer| peer.id == local_id) {
					let info = self.peer_info(&local_id).await;
					peers.push(PeerRow {
						id: local_id.clone(),
						name: String::from("Current device"),
						local: true,
						version: info.version,
						os: info.os,
						uptime: format_uptime(info.uptime_seconds),
					});
				}
				let mut state = self.state.lock().await;
				state.peers = peers;
				state.local_peer_id = Some(local_id);
				state.shared_folders = snapshot
					.shared_folders
					.into_iter()
					.map(|folder| UiSharedFolder {
						path: folder.path().to_string_lossy().into_owned(),
						access: shared_folder_access_label(folder.flags()),
					})
					.collect();
				state.status = format!("Loaded {} device(s)", state.peers.len());
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

	async fn set_peer_audio_devices(&self, devices: Vec<AudioDevice>) {
		let mut state = self.state.lock().await;
		state.peer_audio_devices = devices;
	}

	async fn is_local_media_peer(&self, peer_id: &str) -> bool {
		self.puppy
			.state_snapshot()
			.await
			.map(|state| state.me.to_string() == peer_id)
			.unwrap_or(false)
	}

	async fn refresh_peer_screens(&self, peer_id: &str) {
		if !self.is_local_media_peer(peer_id).await {
			let mut state = self.state.lock().await;
			state.peer_screens.clear();
			state.peer_screen_status =
				String::from("Remote screen streaming is not available yet.");
			return;
		}
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				let capability = match self.puppy.media_capability(peer.clone()).await {
					Ok(capability) => capability,
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(MediaCapability {
							supported: false,
							backend: None,
							message: format!("Failed to query media capability: {err}"),
						});
						state.peer_screens.clear();
						state.peer_screen_status =
							format!("Failed to query media capability: {err}");
						return;
					}
				};
				if !capability.supported {
					let mut state = self.state.lock().await;
					state.peer_screen_status = capability.message.clone();
					state.peer_webcam_capability = Some(capability);
					state.peer_screens.clear();
					return;
				}

				match self.puppy.list_media_sources(peer.clone()).await {
					Ok(sources) => {
						let screens = sources
							.into_iter()
							.filter(|source| source.kind == MediaSourceKind::Screen)
							.collect::<Vec<_>>();
						let mut state = self.state.lock().await;
						state.peer_screen_status = if screens.is_empty() {
							capability.message.clone()
						} else {
							format!("Loaded {} monitor stream source(s).", screens.len())
						};
						state.peer_webcam_capability = Some(capability);
						state.peer_screens = screens;
					}
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(capability);
						state.peer_screens.clear();
						state.peer_screen_status = format!("Failed to load monitors: {err}");
						state.status = format!("Failed to load monitors for {peer_id}: {err}");
					}
				}
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.peer_webcam_capability = Some(MediaCapability {
					supported: false,
					backend: None,
					message: format!("Invalid peer id: {err}"),
				});
				state.peer_screens.clear();
				state.peer_screen_status = format!("Invalid peer id: {err}");
				state.status = format!("Invalid peer id: {err}");
			}
		}
	}

	async fn refresh_puppy_eye(&self) {
		let devices = {
			let mut state = self.state.lock().await;
			state.puppy_eye_status = String::from("Looking for cameras on PuppyNet devices...");
			state.peers.clone()
		};
		let device_count = devices.len();
		let results = futures::future::join_all(devices.into_iter().map(|device| {
			let puppy = Arc::clone(&self.puppy);
			async move {
				let query = async {
					let peer = PeerId::from_str(&device.id)?;
					let capability = puppy.media_capability(peer.clone()).await?;
					let sources = if capability.supported {
						let sources = puppy
							.list_media_sources(peer.clone())
							.await?
							.into_iter()
							.filter(|source| source.kind == MediaSourceKind::Webcam)
							.collect::<Vec<_>>();
						futures::future::join_all(sources.into_iter().map(|source| {
							let puppy = Arc::clone(&puppy);
							let peer = peer.clone();
							async move {
								tokio::time::timeout(
									std::time::Duration::from_secs(2),
									puppy.get_media_frame(peer, source.id.clone()),
								)
								.await
								.ok()
								.and_then(Result::ok)
								.map(|_| source)
							}
						}))
						.await
						.into_iter()
						.flatten()
						.collect()
					} else {
						Vec::new()
					};
					let configured = puppy.list_onvif_cameras(peer.clone()).await?;
					Ok((sources, configured))
				};
				let result = tokio::time::timeout(std::time::Duration::from_secs(3), query)
					.await
					.unwrap_or_else(|_| Err(anyhow::anyhow!("camera discovery timed out")));
				(device, result)
			}
		}))
		.await;

		let mut cameras = Vec::new();
		let mut configured_cameras = Vec::new();
		let mut failed_devices = 0;
		for (device, result) in results {
			match result {
				Ok((sources, configured)) => {
					cameras.extend(sources.into_iter().map(|source| PuppyEyeCamera {
						device_id: device.id.clone(),
						device_name: device.name.clone(),
						source,
					}));
					configured_cameras.extend(configured.into_iter().map(|camera| {
						PuppyEyeConfiguredCamera {
							device_id: device.id.clone(),
							device_name: device.name.clone(),
							camera,
						}
					}));
				}
				Err(error) => {
					failed_devices += 1;
					log::warn!("failed to discover cameras on {}: {error}", device.id);
				}
			}
		}

		let mut state = self.state.lock().await;
		state.puppy_eye_status = format!(
			"Detected {} camera(s) across {device_count} device(s).",
			cameras.len()
		);
		if failed_devices > 0 {
			state.puppy_eye_status.push_str(&format!(
				" {failed_devices} device(s) could not be checked."
			));
		}
		state.puppy_eye_cameras = cameras;
		state.puppy_eye_configured_cameras = configured_cameras;
	}

	async fn refresh_peer_webcams(&self, peer_id: &str) {
		if !self.is_local_media_peer(peer_id).await {
			let mut state = self.state.lock().await;
			state.peer_webcam_capability = Some(MediaCapability {
				supported: false,
				backend: None,
				message: String::from("Remote webcam streaming is not available yet."),
			});
			state.peer_webcams.clear();
			return;
		}
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				let capability = match self.puppy.media_capability(peer.clone()).await {
					Ok(capability) => capability,
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(MediaCapability {
							supported: false,
							backend: None,
							message: format!("Failed to query media capability: {err}"),
						});
						state.peer_webcams.clear();
						return;
					}
				};
				if !capability.supported {
					let mut state = self.state.lock().await;
					state.peer_webcam_capability = Some(capability);
					state.peer_webcams.clear();
					return;
				}

				match self.puppy.list_media_sources(peer.clone()).await {
					Ok(sources) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(capability);
						state.peer_webcams = sources
							.into_iter()
							.filter(|source| source.kind == MediaSourceKind::Webcam)
							.collect();
					}
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(capability);
						state.peer_webcams.clear();
						state.status = format!("Failed to load webcams for {peer_id}: {err}");
					}
				}
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.peer_webcam_capability = Some(MediaCapability {
					supported: false,
					backend: None,
					message: format!("Invalid peer id: {err}"),
				});
				state.peer_webcams.clear();
				state.status = format!("Invalid peer id: {err}");
			}
		}
	}

	async fn refresh_peer_microphones(&self, peer_id: &str) {
		if !self.is_local_media_peer(peer_id).await {
			let mut state = self.state.lock().await;
			state.peer_microphones.clear();
			return;
		}
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				let capability = match self.puppy.media_capability(peer.clone()).await {
					Ok(capability) => capability,
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(MediaCapability {
							supported: false,
							backend: None,
							message: format!("Failed to query media capability: {err}"),
						});
						state.peer_microphones.clear();
						return;
					}
				};
				if !capability.supported {
					let mut state = self.state.lock().await;
					state.peer_webcam_capability = Some(capability);
					state.peer_microphones.clear();
					return;
				}

				match self.puppy.list_media_sources(peer.clone()).await {
					Ok(sources) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(capability);
						state.peer_microphones = sources
							.into_iter()
							.filter(|source| source.kind == MediaSourceKind::Microphone)
							.collect();
					}
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_webcam_capability = Some(capability);
						state.peer_microphones.clear();
						state.status = format!("Failed to load microphones for {peer_id}: {err}");
					}
				}
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.peer_webcam_capability = Some(MediaCapability {
					supported: false,
					backend: None,
					message: format!("Invalid peer id: {err}"),
				});
				state.peer_microphones.clear();
				state.status = format!("Invalid peer id: {err}");
			}
		}
	}

	async fn refresh_peer_audio(&self, peer_id: &str) {
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				let capability = match self.puppy.audio_capability(peer.clone()).await {
					Ok(capability) => capability,
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_audio_capability = Some(AudioCapability {
							supported: false,
							backend: None,
							message: format!("Failed to query audio capability: {err}"),
						});
						state.peer_audio_devices.clear();
						return;
					}
				};
				if !capability.supported {
					let mut state = self.state.lock().await;
					state.peer_audio_capability = Some(capability);
					state.peer_audio_devices.clear();
					return;
				}

				match self.puppy.list_audio_devices(peer.clone()).await {
					Ok(devices) => {
						let mut state = self.state.lock().await;
						state.peer_audio_capability = Some(capability);
						state.peer_audio_devices = devices;
					}
					Err(err) => {
						let mut state = self.state.lock().await;
						state.peer_audio_capability = Some(capability);
						state.peer_audio_devices.clear();
						state.status = format!("Failed to load audio devices for {peer_id}: {err}");
					}
				}
			}
			Err(err) => {
				let mut state = self.state.lock().await;
				state.peer_audio_capability = Some(AudioCapability {
					supported: false,
					backend: None,
					message: format!("Invalid peer id: {err}"),
				});
				state.peer_audio_devices.clear();
				state.status = format!("Invalid peer id: {err}");
			}
		}
	}

	async fn refresh_peer_detail(&self, peer_id: &str) {
		match PeerId::from_str(peer_id) {
			Ok(peer) => {
				if let Ok(cpus) = self.puppy.list_cpus(peer.clone()).await {
					let mut state = self.state.lock().await;
					state.peer_cpus = cpus;
				} else {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load CPU info for {peer_id}");
				}
				if let Ok(interfaces) = self.puppy.list_interfaces(peer.clone()).await {
					let mut state = self.state.lock().await;
					state.peer_interfaces = interfaces;
					state.status = format!("Loaded detail for {peer_id}");
				} else {
					let mut state = self.state.lock().await;
					state.status = format!("Failed to load interfaces for {peer_id}");
				}
				self.refresh_peer_audio(peer_id).await;
				self.refresh_peer_microphones(peer_id).await;
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
		let controllers = UiServerActions::new(self);
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
		let previous_peer = state.selected_peer.clone();
		state.page = page.clone();
		state.selected_peer = match page {
			Page::PeerDetail(peer_id) => Some(peer_id),
			Page::PeerControl { peer_id } => Some(peer_id),
			Page::PeerFiles { peer_id, path } => {
				state.peer_files_path = path;
				Some(peer_id)
			}
			Page::PeerWebcams { peer_id } => Some(peer_id),
			_ => None,
		};
		if state.selected_peer != previous_peer {
			state.peer_audio_capability = None;
			state.peer_audio_devices.clear();
			state.peer_webcam_capability = None;
			state.peer_webcams.clear();
			state.peer_microphones.clear();
			state.peer_screens.clear();
			state.peer_screen_status = String::from("Monitor capability not checked yet.");
		}
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
		Page::PeerControl { .. } => "peer_control",
		Page::PeerFiles { .. } => "peer_files",
		Page::PeerWebcams { .. } => "peer_webcams",
		Page::PuppyEye => "puppy_eye",
		Page::PuppyEyeCamera { .. } => "puppy_eye_camera",
		Page::OnvifCameras => "onvif_cameras",
		Page::OnvifCamera { .. } => "onvif_camera",
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

fn decode_query_value(value: &str) -> String {
	url::form_urlencoded::parse(format!("value={value}").as_bytes())
		.find_map(|(key, value)| {
			if key == "value" {
				Some(value.into_owned())
			} else {
				None
			}
		})
		.unwrap_or_else(|| value.to_string())
}

fn peer_webcam_stream_peer(path: &str) -> Option<&str> {
	let rest = path.strip_prefix("/api/peers/")?;
	rest.strip_suffix("/webcam.mjpeg")
}

#[cfg(any())]
fn peer_microphone_stream_peer(path: &str) -> Option<&str> {
	let rest = path.strip_prefix("/api/peers/")?;
	rest.strip_suffix("/microphone.wav")
}

fn mjpeg_frame_chunk(mime: &str, data: &[u8]) -> Vec<u8> {
	let data = if mime == "image/jpeg" {
		data.windows(2)
			.position(|window| window == [0xff, 0xd9])
			.map(|end| &data[..end + 2])
			.unwrap_or(data)
	} else {
		data
	};
	let mut chunk = Vec::new();
	chunk.extend_from_slice(b"--puppynetframe\r\n");
	chunk.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
	chunk.extend_from_slice(format!("Content-Length: {}\r\n\r\n", data.len()).as_bytes());
	chunk.extend_from_slice(data);
	chunk.extend_from_slice(b"\r\n");
	chunk
}

fn media_stream_label(path: &str) -> &'static str {
	if path.ends_with("/monitor.mjpeg") {
		"monitor"
	} else {
		"webcam"
	}
}

fn media_stream_frame_delay(stream_label: &str) -> std::time::Duration {
	if stream_label == "monitor" {
		std::time::Duration::from_millis(500)
	} else {
		std::time::Duration::from_millis(150)
	}
}

fn monitor_stream_enabled_for_session(ctx: &Arc<UiContext>, session_id: Option<&str>) -> bool {
	let Some(session_id) = session_id else {
		return false;
	};
	ctx.sessions
		.lock()
		.map(|sessions| {
			sessions
				.get(session_id)
				.map(|session| session.monitor_stream_enabled)
				.unwrap_or(false)
		})
		.unwrap_or(false)
}

async fn handle_peer_webcam_stream(request: HttpRequest, ctx: Arc<UiContext>) -> HttpResponse {
	let stream_label = media_stream_label(&request.path);
	let session_id = cookie_value(&request.headers, SESSION_COOKIE);
	let authenticated = session_id
		.as_deref()
		.and_then(|sid| {
			let hash = auth::token_hash(sid);
			ctx.server.puppy.http_me(&hash).ok().flatten()
		})
		.is_some();
	if !authenticated {
		log::warn!("{stream_label} stream rejected: not authenticated");
		return HttpResponse::new(401, "not authenticated")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	}
	if stream_label == "monitor" && !monitor_stream_enabled_for_session(&ctx, session_id.as_deref())
	{
		log::info!("monitor stream rejected: disabled for session");
		return HttpResponse::new(409, "monitor stream disabled")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	}
	let Some(peer_id) = peer_webcam_stream_peer(&request.path) else {
		return HttpResponse::new(404, "webcam stream not found")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	};
	let Ok(peer) = PeerId::from_str(peer_id) else {
		return HttpResponse::new(400, "invalid peer id")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	};
	let device_id = request
		.query
		.get("device")
		.map(|value| decode_query_value(value))
		.unwrap_or_default();
	if device_id.is_empty() {
		return HttpResponse::new(400, "missing webcam device")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	}

	log::info!("{stream_label} stream started for peer {peer_id} source {device_id}");
	let puppy = Arc::clone(&ctx.server.puppy);
	let first_frame = match puppy.get_media_frame(peer.clone(), device_id.clone()).await {
		Ok(frame) => frame,
		Err(err) => {
			log::warn!("{stream_label} stream failed to start: {err}");
			return HttpResponse::new(503, format!("failed to start {stream_label} stream: {err}"))
				.header("content-type", "text/plain")
				.header("cache-control", "no-store");
		}
	};
	let stream = futures::stream::unfold(
		(
			puppy,
			peer,
			device_id,
			stream_label,
			ctx,
			session_id,
			Some(first_frame),
			false,
		),
		|state| async move {
			let (
				puppy,
				peer,
				device_id,
				stream_label,
				ctx,
				session_id,
				pending_frame,
				logged_first_frame,
			) = state;
			if stream_label == "monitor"
				&& !monitor_stream_enabled_for_session(&ctx, session_id.as_deref())
			{
				log::info!("monitor stream stopped: disabled for session");
				return None;
			}
			let frame = match pending_frame {
				Some(frame) => Ok::<MediaFrame, anyhow::Error>(frame),
				None => puppy.get_media_frame(peer.clone(), device_id.clone()).await,
			};
			match frame {
				Ok(frame) => {
					let next_logged_first_frame = if logged_first_frame {
						true
					} else {
						log::info!(
							"{stream_label} stream produced first frame: {} bytes ({})",
							frame.data.len(),
							frame.mime
						);
						true
					};
					let chunk = mjpeg_frame_chunk(&frame.mime, &frame.data);
					tokio::time::sleep(media_stream_frame_delay(stream_label)).await;
					Some((
						Ok::<_, Infallible>(chunk),
						(
							puppy,
							peer,
							device_id,
							stream_label,
							ctx,
							session_id,
							None,
							next_logged_first_frame,
						),
					))
				}
				Err(err) => {
					log::warn!("{stream_label} stream stopped: {err}");
					None
				}
			}
		},
	);
	HttpResponse::stream(200, stream)
		.header(
			"content-type",
			"multipart/x-mixed-replace; boundary=puppynetframe",
		)
		.header("cache-control", "no-store")
		.header("x-accel-buffering", "no")
}

#[cfg(any())]
async fn handle_peer_microphone_stream(request: HttpRequest, ctx: Arc<UiContext>) -> HttpResponse {
	let session_id = cookie_value(&request.headers, SESSION_COOKIE);
	let authenticated = session_id
		.as_deref()
		.and_then(|sid| {
			let hash = auth::token_hash(sid);
			ctx.server.puppy.http_me(&hash).ok().flatten()
		})
		.is_some();
	if !authenticated {
		log::warn!("microphone stream rejected: not authenticated");
		return HttpResponse::new(401, "not authenticated")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	}
	let Some(peer_id) = peer_microphone_stream_peer(&request.path) else {
		return HttpResponse::new(404, "microphone stream not found")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	};
	let Ok(peer) = PeerId::from_str(peer_id) else {
		return HttpResponse::new(400, "invalid peer id")
			.header("content-type", "text/plain")
			.header("cache-control", "no-store");
	};
	let source_id = request
		.query
		.get("device")
		.map(|value| decode_query_value(value))
		.unwrap_or_else(|| String::from("microphone:default"));

	let puppy = Arc::clone(&ctx.server.puppy);
	match puppy.get_media_frame(peer, source_id).await {
		Ok(frame) => HttpResponse::new(200, frame.data)
			.header("content-type", frame.mime)
			.header("cache-control", "no-store"),
		Err(err) => {
			log::warn!("microphone stream failed: {err}");
			HttpResponse::new(503, format!("failed to capture microphone audio: {err}"))
				.header("content-type", "text/plain")
				.header("cache-control", "no-store")
		}
	}
}

fn request_header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
	headers
		.iter()
		.find(|(key, _)| key.eq_ignore_ascii_case(name))
		.map(|(_, value)| value.as_str())
}

fn authenticated_http_request(request: &HttpRequest, ctx: &Arc<UiContext>) -> bool {
	cookie_value(&request.headers, SESSION_COOKIE)
		.and_then(|sid| {
			let hash = auth::token_hash(&sid);
			ctx.server.puppy.http_me(&hash).ok().flatten()
		})
		.is_some()
}

fn pairing_endpoint(host: &str) -> Result<String> {
	let url = url::Url::parse(&format!("http://{host}"))?;
	let host = url.host_str().context("pairing request has no host")?;
	let host = host.trim_start_matches('[').trim_end_matches(']');
	let host = if host.contains(':') {
		format!("[{host}]")
	} else {
		host.to_string()
	};
	Ok(format!("ws://{host}:8833/peer/v1"))
}

fn pairing_qr_response(request: &HttpRequest, ctx: &Arc<UiContext>) -> HttpResponse {
	if !authenticated_http_request(request, ctx) {
		return HttpResponse::new(401, "not authenticated");
	}
	let result = (|| -> Result<String> {
		let host = request_header(&request.headers, "host").context("missing Host header")?;
		let payload = serde_json::json!({
			"v": 1,
			"node_id": ctx.server.puppy.local_peer_id().map_err(anyhow::Error::msg)?.to_string(),
			"endpoint": pairing_endpoint(host)?,
		});
		let code = qrcode::QrCode::new(payload.to_string().as_bytes())?;
		Ok(code
			.render::<qrcode::render::svg::Color>()
			.min_dimensions(256, 256)
			.build())
	})();
	match result {
		Ok(svg) => HttpResponse::new(200, svg)
			.header("content-type", "image/svg+xml")
			.header("cache-control", "no-store"),
		Err(error) => HttpResponse::new(400, format!("could not create pairing QR code: {error}")),
	}
}

fn same_origin_request(request: &HttpRequest) -> bool {
	let Some(origin) = request_header(&request.headers, "origin") else {
		return true;
	};
	let Some(host) = request_header(&request.headers, "host") else {
		return false;
	};
	origin == format!("http://{host}") || origin == format!("https://{host}")
}

fn peer_media_sessions_peer(path: &str) -> Option<&str> {
	path.strip_prefix("/api/peers/")?
		.strip_suffix("/media/sessions")
		.filter(|peer| !peer.is_empty() && !peer.contains('/'))
}

fn media_session_id(path: &str) -> Option<&str> {
	let session_id = path.strip_prefix("/api/media/sessions/")?;
	(!session_id.is_empty() && !session_id.contains('/')).then_some(session_id)
}

fn media_error(status: u16, message: impl Into<String>) -> HttpResponse {
	json_response(status, serde_json::json!({ "error": message.into() }))
		.header("cache-control", "no-store")
}

async fn handle_create_media_session(request: HttpRequest, ctx: Arc<UiContext>) -> HttpResponse {
	if !authenticated_http_request(&request, &ctx) {
		return media_error(401, "not authenticated");
	}
	if !same_origin_request(&request) {
		return media_error(403, "cross-origin media signaling is not allowed");
	}
	let Some(peer_id) = peer_media_sessions_peer(&request.path) else {
		return media_error(404, "media session endpoint not found");
	};
	let Ok(peer) = PeerId::from_str(peer_id) else {
		return media_error(400, "invalid peer id");
	};
	if ctx.server.local_peer_id().await != Some(peer) {
		return media_error(409, "remote media streaming is not available yet");
	}
	if request.body.len() > 1024 * 1024 {
		return media_error(413, "media session request is too large");
	}
	if !request_header(&request.headers, "content-type")
		.map(|value| value.split(';').next() == Some("application/json"))
		.unwrap_or(false)
	{
		return media_error(415, "media session requests must use application/json");
	}
	let request = match serde_json::from_slice::<CreateMediaSession>(&request.body) {
		Ok(request) => request,
		Err(error) => return media_error(400, format!("invalid media session request: {error}")),
	};
	match ctx.server.media.create_session(request).await {
		Ok(session) => match serde_json::to_value(session) {
			Ok(session) => json_response(201, session).header("cache-control", "no-store"),
			Err(error) => media_error(500, format!("failed to serialize media session: {error}")),
		},
		Err(error) => {
			log::warn!("failed to create local WebRTC media session: {error}");
			media_error(503, format!("failed to create media session: {error}"))
		}
	}
}

async fn handle_delete_media_session(request: HttpRequest, ctx: Arc<UiContext>) -> HttpResponse {
	if !authenticated_http_request(&request, &ctx) {
		return media_error(401, "not authenticated");
	}
	if !same_origin_request(&request) {
		return media_error(403, "cross-origin media signaling is not allowed");
	}
	let Some(session_id) = media_session_id(&request.path) else {
		return media_error(404, "media session not found");
	};
	if ctx.server.media.remove_session(session_id).await {
		HttpResponse::new(204, Vec::new()).header("cache-control", "no-store")
	} else {
		media_error(404, "media session not found")
	}
}

async fn handle_ui_http(request: HttpRequest, ctx: Arc<UiContext>) -> Option<HttpResponse> {
	match (request.method.as_str(), request.path.as_str()) {
		("GET", "/pairing/qr.svg") => Some(pairing_qr_response(&request, &ctx)),
		("GET", path) if peer_webcam_stream_peer(path).is_some() => {
			Some(handle_peer_webcam_stream(request, ctx).await)
		}
		("POST", path) if peer_media_sessions_peer(path).is_some() => {
			Some(handle_create_media_session(request, ctx).await)
		}
		("DELETE", path) if media_session_id(path).is_some() => {
			Some(handle_delete_media_session(request, ctx).await)
		}
		("GET", "/assets/trackpad.js") => Some(
			HttpResponse::new(200, TRACKPAD_JS.to_vec())
				.header("content-type", "text/javascript")
				.header("cache-control", "no-store"),
		),
		("GET", "/assets/media_receiver.js") => Some(
			HttpResponse::new(200, MEDIA_RECEIVER_JS.to_vec())
				.header("content-type", "text/javascript")
				.header("cache-control", "no-store"),
		),
		("GET", "/favicon.ico") => Some(favicon_response()),
		("GET", "/auth/finish") => {
			let token = request
				.query
				.get("nonce")
				.and_then(|nonce| ctx.pending_login_tokens.lock().unwrap().remove(nonce));
			let Some(token) = token else {
				return Some(
					redirect_response("/login")
						.header("cache-control", "no-store")
						.header("set-cookie", clear_session_cookie()),
				);
			};
			Some(
				redirect_response("/")
					.header("cache-control", "no-store")
					.header("set-cookie", session_cookie(&token)),
			)
		}
		("GET", "/auth/logout") | ("POST", "/auth/logout") => {
			if let Some(sid) = cookie_value(&request.headers, SESSION_COOKIE) {
				let hash = auth::token_hash(&sid);
				let _ = ctx.server.puppy.drop_session(&hash);
			}
			Some(
				redirect_response("/login")
					.header("cache-control", "no-store")
					.header("set-cookie", clear_session_cookie()),
			)
		}
		("GET", "/auth/me") => {
			let user = cookie_value(&request.headers, SESSION_COOKIE).and_then(|sid| {
				let hash = auth::token_hash(&sid);
				ctx.server.puppy.http_me(&hash).ok().flatten()
			});
			Some(match user {
				Some(user) => json_response(200, serde_json::json!({ "user": user })),
				None => json_response(401, serde_json::json!({ "error": "not authenticated" })),
			})
		}
		_ => None,
	}
}

const ACTION_LOGIN: u32 = 1;
const ACTION_LOGOUT: u32 = 2;
const ACTION_REFRESH: u32 = 3;
const ACTION_PEER: u32 = 4;
const ACTION_SEARCH: u32 = 5;
const ACTION_SEARCH_MORE: u32 = 6;
const ACTION_SEARCH_MIME: u32 = 7;
const ACTION_SEARCH_CLEAR_MIMES: u32 = 8;
const ACTION_SEARCH_PREVIEW: u32 = 9;
const ACTION_CLOSE_PREVIEW: u32 = 10;
const ACTION_CHANGE_PASSWORD: u32 = 11;
const ACTION_OPEN_USER: u32 = 12;
const ACTION_CLOSE_USER: u32 = 13;
const ACTION_CREATE_USER: u32 = 14;
const ACTION_DELETE_USER: u32 = 15;
const ACTION_OPEN_ONVIF: u32 = 16;
const ACTION_CLOSE_ONVIF: u32 = 17;
const ACTION_TEST_ONVIF: u32 = 18;
const ACTION_SAVE_ONVIF: u32 = 19;
const ACTION_OPEN_CAMERA: u32 = 20;
const ACTION_LOAD_EYE_PROFILE: u32 = 31;
const ACTION_SAVE_EYE_PROFILE: u32 = 32;
const ACTION_PEER_BACK: u32 = 21;
const ACTION_START_SHELL: u32 = 22;
const ACTION_SEND_SHELL: u32 = 23;
const ACTION_REFRESH_AUDIO: u32 = 24;
const ACTION_AUDIO_MUTE: u32 = 25;
const ACTION_ADD_FOLDER: u32 = 26;
const ACTION_START_UPDATE: u32 = 27;
const ACTION_TOGGLE_MONITOR: u32 = 28;
const ACTION_SEND_CONTROL_TEXT: u32 = 29;
const ACTION_EDIT_ONVIF: u32 = 30;
const INPUT_LOGIN_USERNAME: u32 = 100;
const INPUT_LOGIN_PASSWORD: u32 = 101;
const INPUT_SEARCH_NAME: u32 = 102;
const INPUT_SEARCH_TARGET: u32 = 103;
const INPUT_SEARCH_SORT: u32 = 104;
const INPUT_SEARCH_PAGE_SIZE: u32 = 105;
const INPUT_CURRENT_PASSWORD: u32 = 106;
const INPUT_NEW_PASSWORD: u32 = 107;
const INPUT_CONFIRM_PASSWORD: u32 = 108;
const INPUT_NEW_USER_USERNAME: u32 = 109;
const INPUT_NEW_USER_PASSWORD: u32 = 110;
const INPUT_ONVIF_TARGET: u32 = 111;
const INPUT_ONVIF_NAME: u32 = 112;
const INPUT_ONVIF_URL: u32 = 113;
const INPUT_ONVIF_USERNAME: u32 = 114;
const INPUT_ONVIF_PASSWORD: u32 = 115;
const INPUT_ONVIF_PROFILE: u32 = 116;
const INPUT_SHELL: u32 = 117;
const INPUT_UPDATE_VERSION: u32 = 118;
const INPUT_SHARED_FOLDER: u32 = 119;
const INPUT_SHARED_ACCESS: u32 = 120;
const INPUT_AUDIO_DEVICE: u32 = 121;
const INPUT_MICROPHONE: u32 = 122;
const INPUT_AUDIO_VOLUME: u32 = 123;
const INPUT_CONTROL_TEXT: u32 = 124;
const INPUT_EYE_PROFILE: u32 = 125;
const INPUT_ONVIF_IDLE_TIMEOUT: u32 = 126;
const INPUT_EYE_IDLE_TIMEOUT: u32 = 127;
const INPUT_EYE_GRID_WIDTH: u32 = 128;

fn page_title(page: &str) -> String {
	let name = match page {
		"home" => "Home",
		"peers" => "Devices",
		"peer_detail" => "Device",
		"peer_control" => "Device Control",
		"peer_files" => "Device Files",
		"peer_webcams" => "Device Cameras",
		"puppy_eye" => "Eye",
		"puppy_eye_camera" => "Eye Camera",
		"onvif_cameras" => "ONVIF Cameras",
		"onvif_camera" => "ONVIF Camera",
		"files" => "Files",
		"search" => "Search",
		"storage" => "Storage",
		"users" => "Users",
		"updates" => "Updates",
		"settings" => "Settings",
		_ => "PuppyNet",
	};
	format!("{name} - PuppyNet UI")
}

fn nav_link(path: &str, label: &str) -> Item {
	wgui::link(path, label)
		.color("#79f2c0")
		.padding(6)
		.border("1px solid #2d6258")
}

fn render_nav() -> Item {
	wgui::hstack([
		nav_link("/", "Home"),
		nav_link("/devices", "Devices"),
		nav_link("/eye", "Eye"),
		nav_link("/onvif-cameras", "ONVIF"),
		nav_link("/files", "Files"),
		nav_link("/search", "Search"),
		nav_link("/storage", "Storage"),
		nav_link("/users", "Users"),
		nav_link("/updates", "Updates"),
		nav_link("/settings", "Settings"),
		wgui::button("Logout").on_click(ACTION_LOGOUT),
	])
	.spacing(6)
	.wrap(true)
}

fn render_login(state: &UiViewState) -> Item {
	wgui::vstack([
		wgui::text("PuppyNet login").color("#7bdcff"),
		wgui::text_input()
			.id(INPUT_LOGIN_USERNAME)
			.name("username")
			.placeholder("Username")
			.svalue(&state.login_username),
		wgui::text_input()
			.id(INPUT_LOGIN_PASSWORD)
			.name("password")
			.input_type("password")
			.placeholder("Password")
			.svalue(&state.login_password),
		wgui::button("Login").on_click(ACTION_LOGIN),
		wgui::text(&state.login_error).color("#ff8f8f"),
	])
	.spacing(10)
	.padding(20)
	.max_width(420)
}

fn render_devices(state: &UiViewState) -> Item {
	let mut rows = Vec::new();
	rows.push(
		wgui::hstack([
			wgui::text("Devices").color("#7bdcff").grow(1),
			wgui::button("Refresh").on_click(ACTION_REFRESH),
		])
		.spacing(8)
		.wrap(true),
	);
	for (index, peer) in state.peers.iter().enumerate() {
		rows.push(
			wgui::hstack([
				wgui::vstack([
					wgui::text(&peer.label).color("#eafff6"),
					wgui::text(&format!("{} · {}", peer.short_id, peer.os)).color("#9fbdb6"),
				])
				.grow(1),
				wgui::text(&peer.status).color(&peer.status_color),
				wgui::button("Details")
					.on_click(ACTION_PEER)
					.inx(index as u32),
			])
			.spacing(8)
			.padding(10)
			.border("1px solid #2d6258"),
		);
	}
	rows.push(wgui::text(&state.grant_command).break_words(true));
	wgui::vstack(rows).spacing(8)
}

fn select_options(options: &[UiSelectOption]) -> Vec<wgui::SelectOption> {
	options
		.iter()
		.map(|option| wgui::option(&option.value, &option.name))
		.collect()
}

fn camera_idle_timeout_options() -> Vec<wgui::SelectOption> {
	[
		("0", "Never stop"),
		("5", "Stop after 5 seconds"),
		("30", "Stop after 30 seconds"),
		("60", "Stop after 1 minute"),
	]
	.into_iter()
	.map(|(value, name)| wgui::option(value, name))
	.collect()
}

fn render_media_receiver(
	props: &UiMediaReceiverProps,
	profile_options: &[UiSelectOption],
	index: u32,
) -> Item {
	wgui::custom_component(
		"media-receiver",
		"/assets/media_receiver.js?v=4",
		serde_json::json!({
			"auto_start": props.auto_start,
			"endpoint": props.endpoint,
			"media_kind": props.media_kind,
			"revision": props.revision,
			"resolution_options": profile_options,
			"resolution_mode": if props.source_id.starts_with("onvif:") { "profile" } else { "capture" },
			"resolution_selectable": !profile_options.is_empty() || props.source_id.starts_with("onvif:"),
			"source_id": props.source_id,
		}),
	)
	.custom_event("resolutionChange", INPUT_EYE_PROFILE)
	.inx(index)
	.fill(true)
}

fn render_camera_viewer(
	state: &UiViewState,
	name: &str,
	device_name: &str,
	props: &UiMediaReceiverProps,
	index: u32,
	show_camera_page_button: bool,
) -> Item {
	let profile_options = state
		.puppy_eye_cameras
		.iter()
		.find(|camera| camera.source_id == props.source_id)
		.map(|camera| camera.profile_options.as_slice())
		.unwrap_or_default();
	let mut body = vec![
		wgui::text(&format!("{device_name}: {name}")),
		render_media_receiver(props, profile_options, index),
	];
	if show_camera_page_button {
		body.push(
			wgui::button("Open camera page")
				.on_click(ACTION_OPEN_CAMERA)
				.inx(index),
		);
	}
	let viewer = wgui::vstack(body)
		.spacing(6)
		.padding(8)
		.border("1px solid #2d6258");
	if show_camera_page_button {
		viewer
			.width(state.eye_grid_width as u32)
			.min_width(320)
			.grow(1)
	} else {
		viewer.grow(1)
	}
}

fn render_puppy_eye(state: &UiViewState) -> Item {
	let cameras = state
		.puppy_eye_cameras
		.iter()
		.map(|camera| {
			render_camera_viewer(
				state,
				&camera.camera_name,
				&camera.device_name,
				&camera.media_receiver_props,
				camera.index,
				true,
			)
		})
		.collect::<Vec<_>>();
	wgui::vstack([
		wgui::hstack([
			wgui::text("Eye").color("#7bdcff").grow(1),
			wgui::text(&format!("Camera width: {}px", state.eye_grid_width)),
			wgui::slider()
				.id(INPUT_EYE_GRID_WIDTH)
				.min(320)
				.max(900)
				.step(40)
				.ivalue(state.eye_grid_width)
				.width(220),
			wgui::button("Refresh").on_click(ACTION_REFRESH),
		])
		.spacing(8)
		.wrap(true),
		wgui::text(&state.puppy_eye_status),
		wgui::hstack(cameras).spacing(8).wrap(true),
	])
	.spacing(8)
}

fn render_file_preview(state: &UiViewState) -> Item {
	wgui::modal([
		wgui::hstack([
			wgui::text("File preview").color("#7bdcff").grow(1),
			wgui::button("Close").on_click(ACTION_CLOSE_PREVIEW),
		]),
		wgui::text(&state.file_preview_path).break_words(true),
		if state.file_preview_has_image {
			wgui::img(&state.file_preview_image_src, "File preview")
				.max_width(900)
				.max_height(700)
				.object_fit("contain")
		} else {
			wgui::text(&state.file_preview_content).break_words(true)
		},
		wgui::text(&state.file_preview_status),
	])
	.open(state.file_preview_modal_open)
}

fn render_files(state: &UiViewState) -> Item {
	let mut body = vec![
		wgui::hstack([
			wgui::text("Files").color("#7bdcff").grow(1),
			wgui::button("Refresh").on_click(ACTION_REFRESH),
		])
		.spacing(8),
	];
	for (index, file) in state.files.iter().enumerate() {
		body.push(
			wgui::hstack([
				wgui::text(&file.line).break_words(true).grow(1),
				wgui::button("Preview")
					.on_click(ACTION_SEARCH_PREVIEW)
					.inx(index as u32),
			])
			.spacing(8)
			.border("1px solid #2d6258")
			.padding(8),
		);
	}
	body.push(render_file_preview(state));
	wgui::vstack(body).spacing(8)
}

fn render_search(state: &UiViewState) -> Item {
	let mut body = vec![
		wgui::hstack([
			wgui::text_input()
				.id(INPUT_SEARCH_NAME)
				.placeholder("Search by file name")
				.svalue(&state.search_name_query)
				.grow(1),
			wgui::select(select_options(&state.search_target_options))
				.id(INPUT_SEARCH_TARGET)
				.svalue(&state.search_target),
			wgui::button("Search").on_click(ACTION_SEARCH),
		])
		.spacing(6),
		wgui::hstack([
			wgui::select(select_options(&state.search_sort_options))
				.id(INPUT_SEARCH_SORT)
				.svalue(&state.search_sort),
			wgui::select(select_options(&state.search_page_size_options))
				.id(INPUT_SEARCH_PAGE_SIZE)
				.svalue(&state.search_page_size),
			wgui::text(&state.search_page_text),
			wgui::button("Clear filters").on_click(ACTION_SEARCH_CLEAR_MIMES),
		])
		.spacing(6),
	];
	for (index, mime) in state.search_mime_options.iter().enumerate() {
		body.push(
			wgui::hstack([
				wgui::checkbox()
					.checked(mime.selected)
					.id(ACTION_SEARCH_MIME)
					.inx(index as u32),
				wgui::text(&mime.name),
			])
			.spacing(4),
		);
	}
	body.push(wgui::text(&state.search_status));
	for (index, row) in state.search_results.iter().enumerate() {
		body.push(
			wgui::hstack([
				wgui::vstack([
					wgui::text(&row.name),
					wgui::text(&row.path).color("#9fbdb6"),
				])
				.grow(1),
				wgui::text(&row.device),
				wgui::text(&row.size),
				wgui::button("Preview")
					.on_click(ACTION_SEARCH_PREVIEW)
					.inx(index as u32),
			])
			.spacing(8)
			.padding(8)
			.border("1px solid #2d6258"),
		);
	}
	if state.search_can_load_more {
		body.push(wgui::button("Load more").on_click(ACTION_SEARCH_MORE));
	}
	body.push(render_file_preview(state));
	wgui::vstack(body).spacing(8)
}

fn render_settings(state: &UiViewState) -> Item {
	wgui::vstack([
		wgui::text("Change password").color("#7bdcff"),
		wgui::text_input()
			.id(INPUT_CURRENT_PASSWORD)
			.input_type("password")
			.placeholder("Current password")
			.svalue(&state.current_password),
		wgui::text_input()
			.id(INPUT_NEW_PASSWORD)
			.input_type("password")
			.placeholder("New password")
			.svalue(&state.new_password),
		wgui::text_input()
			.id(INPUT_CONFIRM_PASSWORD)
			.input_type("password")
			.placeholder("Confirm password")
			.svalue(&state.confirm_password),
		wgui::button("Change password").on_click(ACTION_CHANGE_PASSWORD),
		wgui::text(&state.password_change_status),
	])
	.spacing(8)
	.max_width(480)
}

fn render_users(state: &UiViewState) -> Item {
	let mut body = vec![
		wgui::hstack([
			wgui::text("Users").color("#7bdcff").grow(1),
			wgui::button("New").on_click(ACTION_OPEN_USER),
			wgui::button("Refresh").on_click(ACTION_REFRESH),
		])
		.spacing(6),
	];
	for (index, user) in state.users.iter().enumerate() {
		body.push(
			wgui::hstack([
				wgui::text(user).grow(1),
				wgui::button("Delete")
					.on_click(ACTION_DELETE_USER)
					.inx(index as u32),
			])
			.padding(8)
			.border("1px solid #2d6258"),
		);
	}
	body.push(
		wgui::modal([
			wgui::hstack([
				wgui::text("Create user").color("#7bdcff").grow(1),
				wgui::button("Close").on_click(ACTION_CLOSE_USER),
			]),
			wgui::text_input()
				.id(INPUT_NEW_USER_USERNAME)
				.placeholder("Username")
				.svalue(&state.new_user_username),
			wgui::text_input()
				.id(INPUT_NEW_USER_PASSWORD)
				.input_type("password")
				.placeholder("Password")
				.svalue(&state.new_user_password),
			wgui::button("Create").on_click(ACTION_CREATE_USER),
			wgui::text(&state.new_user_status),
		])
		.open(state.new_user_modal_open),
	);
	wgui::vstack(body).spacing(8)
}

fn render_onvif(state: &UiViewState) -> Item {
	let mut body = vec![
		wgui::hstack([
			wgui::text("ONVIF cameras").color("#7bdcff").grow(1),
			wgui::button("Add camera").on_click(ACTION_OPEN_ONVIF),
			wgui::button("Refresh").on_click(ACTION_REFRESH),
		])
		.spacing(6),
	];
	for (index, camera) in state.onvif_cameras.iter().enumerate() {
		body.push(
			wgui::hstack([
				wgui::vstack([
					wgui::text(&camera.name),
					wgui::text(&camera.device_name).color("#9fbdb6"),
					wgui::text(&camera.device_service_url).break_words(true),
					wgui::text(&camera.idle_timeout).color("#9fbdb6"),
				])
				.grow(1),
				wgui::vstack([
					wgui::button("Edit")
						.on_click(ACTION_EDIT_ONVIF)
						.inx(index as u32),
					wgui::link(&camera.view_href, "VIEW")
						.padding(5)
						.border("1px solid #2d6258")
						.text_align("center")
						.min_height(30),
				])
				.spacing(6)
				.width(70),
			])
			.spacing(8)
			.padding(8)
			.border("1px solid #2d6258"),
		);
	}
	body.push(
		wgui::modal([wgui::vstack([
			wgui::hstack([
				wgui::text(if state.onvif_editing {
					"Edit ONVIF camera"
				} else {
					"Add ONVIF camera"
				})
				.color("#7bdcff")
				.grow(1),
				wgui::button("Close").on_click(ACTION_CLOSE_ONVIF),
			]),
			if state.onvif_editing {
				wgui::text(&format!("Node: {}", state.onvif_target_device)).break_words(true)
			} else {
				wgui::select(select_options(&state.onvif_target_device_options))
					.id(INPUT_ONVIF_TARGET)
					.svalue(&state.onvif_target_device)
			},
			wgui::text_input()
				.id(INPUT_ONVIF_NAME)
				.placeholder("Camera name")
				.svalue(&state.onvif_name),
			wgui::text_input()
				.id(INPUT_ONVIF_URL)
				.placeholder("Device service URL")
				.svalue(&state.onvif_device_service_url),
			wgui::text_input()
				.id(INPUT_ONVIF_USERNAME)
				.placeholder("Username")
				.svalue(&state.onvif_username),
			wgui::text_input()
				.id(INPUT_ONVIF_PASSWORD)
				.input_type("password")
				.placeholder("Password")
				.svalue(&state.onvif_password),
			if state.onvif_profile_options.is_empty() {
				wgui::text_input()
					.id(INPUT_ONVIF_PROFILE)
					.placeholder("Profile token (test to load resolutions)")
					.svalue(&state.onvif_profile_token)
			} else {
				wgui::select(select_options(&state.onvif_profile_options))
					.id(INPUT_ONVIF_PROFILE)
					.svalue(&state.onvif_profile_token)
			},
			wgui::text("When nobody is watching or recording"),
			wgui::select(camera_idle_timeout_options())
				.id(INPUT_ONVIF_IDLE_TIMEOUT)
				.svalue(&state.onvif_idle_timeout),
			wgui::hstack([
				wgui::button("Test").on_click(ACTION_TEST_ONVIF),
				wgui::button("Save").on_click(ACTION_SAVE_ONVIF),
			]),
			wgui::text(&state.onvif_status),
		])
		.spacing(8)
		.padding(16)
		.width(560)
		.background_color("#00100d")])
		.open(state.onvif_modal_open),
	);
	wgui::vstack(body).spacing(8)
}

fn render_summary(state: &UiViewState) -> Item {
	let mut body = vec![wgui::text(&page_title(&state.page)).color("#7bdcff")];
	match state.page.as_str() {
		"home" => {
			body.push(wgui::text(&state.status));
			body.push(wgui::text(&state.current_peer).break_words(true));
		}
		"files" => {
			for file in &state.files {
				body.push(wgui::text(&file.line).break_words(true));
			}
		}
		"storage" => {
			for row in &state.storage_rows {
				body.push(wgui::text(&row.line).break_words(true));
			}
		}
		"users" => {
			for user in &state.users {
				body.push(wgui::text(user));
			}
		}
		"peer_detail" | "peer_control" | "peer_files" | "peer_webcams" => {
			body.push(wgui::text(&state.selected_peer).break_words(true));
			body.push(wgui::hstack([
				wgui::link(&state.selected_peer_details_href, "Details"),
				wgui::link(&state.selected_peer_control_href, "Control"),
				wgui::link(&state.selected_peer_files_href, "Files"),
				wgui::link(&state.selected_peer_webcams_href, "Cameras"),
			]));
			for cpu in &state.cpus {
				body.push(wgui::text(&cpu.line));
			}
			for interface in &state.interfaces {
				body.push(wgui::text(&interface.line));
			}
			if state.page == "peer_files" {
				body.push(wgui::text(&state.peer_files_path).break_words(true));
				if state.peer_files_has_parent {
					body.push(wgui::link(
						&state.peer_files_parent_href,
						"Parent directory",
					));
				}
				for file in &state.peer_files {
					body.push(wgui::link(
						&file.href,
						&format!("{} — {}", file.name, file.summary),
					));
				}
			}
			if state.page == "peer_webcams" {
				body.push(wgui::text(&state.webcam_capability_status));
				for webcam in &state.webcam_devices {
					body.push(wgui::text(&webcam.name));
				}
			}
			if state.page == "peer_detail" {
				body.push(wgui::button("Refresh audio").on_click(ACTION_REFRESH_AUDIO));
				body.push(wgui::text(&state.audio_capability_status));
				if state.has_audio_output_options {
					body.push(
						wgui::select(select_options(&state.audio_output_options))
							.id(INPUT_AUDIO_DEVICE)
							.svalue(&state.selected_audio_device),
					);
				}
				body.push(
					wgui::slider()
						.id(INPUT_AUDIO_VOLUME)
						.min(0)
						.max(100)
						.ivalue(state.audio_volume),
				);
				body.push(wgui::button(&state.audio_mute_label).on_click(ACTION_AUDIO_MUTE));
				body.push(
					wgui::text_input()
						.id(INPUT_SHELL)
						.placeholder("Shell command")
						.svalue(&state.shell_input),
				);
				body.push(wgui::hstack([
					wgui::button("Start shell").on_click(ACTION_START_SHELL),
					wgui::button("Send").on_click(ACTION_SEND_SHELL),
				]));
				body.push(wgui::text(&state.shell_output).break_words(true));
			}
			if state.page == "peer_control" {
				body.push(wgui::button("Toggle monitor").on_click(ACTION_TOGGLE_MONITOR));
				body.push(wgui::text(&state.monitor_status));
				body.push(
					wgui::text_input()
						.id(INPUT_CONTROL_TEXT)
						.placeholder("Text to type")
						.svalue(&state.control_text),
				);
				body.push(wgui::button("Send text").on_click(ACTION_SEND_CONTROL_TEXT));
			}
			body.push(wgui::button("Back").on_click(ACTION_PEER_BACK));
		}
		"puppy_eye_camera" => {
			if state.puppy_eye_viewer_found {
				body.push(render_camera_viewer(
					state,
					&state.puppy_eye_viewer_name,
					&state.puppy_eye_viewer_device_name,
					&state.puppy_eye_viewer_webrtc_props,
					state.puppy_eye_viewer_index,
					false,
				));
			}
		}
		"onvif_camera" => {
			if state.onvif_viewer_found && state.onvif_viewer_enabled {
				body.push(render_camera_viewer(
					state,
					&state.onvif_viewer_name,
					&state.onvif_viewer_device_name,
					&state.onvif_viewer_webrtc_props,
					state.onvif_viewer_index,
					false,
				));
			}
		}
		_ => body.push(wgui::text(&state.status)),
	}
	wgui::vstack(body).spacing(8)
}

fn render_ui(state: &UiViewState) -> Item {
	if !state.authenticated {
		return render_login(state);
	}
	let page = match state.page.as_str() {
		"peers" => render_devices(state),
		"puppy_eye" => render_puppy_eye(state),
		"onvif_cameras" => render_onvif(state),
		"files" => render_files(state),
		"search" => render_search(state),
		"settings" => render_settings(state),
		"users" => render_users(state),
		_ => render_summary(state),
	};
	wgui::vstack([render_nav(), page, wgui::text(&state.status)])
		.spacing(12)
		.padding(10)
		.fill(true)
		.background_color("#020807")
		.color("#d6eee9")
}

fn spawn_client_refresh<F>(client: &Arc<UiClientContext>, future: F)
where
	F: Future<Output = ()> + Send + 'static,
{
	let client_id = client.client_id;
	let handle = client.handle.clone();
	tokio::spawn(async move {
		future.await;
		handle.refresh(client_id);
	});
}

async fn load_client_path(client: &Arc<UiClientContext>, path: String) {
	*client.path.lock().unwrap() = path.clone();
	let server = &client.state.server;
	let parts = path
		.trim_matches('/')
		.split('/')
		.filter(|part| !part.is_empty())
		.collect::<Vec<_>>();
	match parts.as_slice() {
		[] => set_client_page(client, Page::Home).await,
		["devices"] | ["peers"] => {
			set_client_page(client, Page::Peers).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move { server.refresh_peers().await });
		}
		["devices", peer_id] | ["peers", peer_id] => {
			set_client_page(client, Page::PeerDetail((*peer_id).to_string())).await;
			let peer_id = (*peer_id).to_string();
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move {
				server.refresh_peer_detail(&peer_id).await;
			});
		}
		["devices", peer_id, "control"] | ["peers", peer_id, "control"] => {
			set_client_page(
				client,
				Page::PeerControl {
					peer_id: (*peer_id).to_string(),
				},
			)
			.await;
			let peer_id = (*peer_id).to_string();
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move {
				server.refresh_peer_screens(&peer_id).await;
			});
		}
		["devices", peer_id, "files"] | ["peers", peer_id, "files"] => {
			set_client_page(
				client,
				Page::PeerFiles {
					peer_id: (*peer_id).to_string(),
					path: String::from("/"),
				},
			)
			.await;
			let peer_id = (*peer_id).to_string();
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move {
				server.refresh_peer_files(&peer_id, "/").await;
			});
		}
		["devices", peer_id, "webcams"] | ["peers", peer_id, "webcams"] => {
			set_client_page(
				client,
				Page::PeerWebcams {
					peer_id: (*peer_id).to_string(),
				},
			)
			.await;
			let peer_id = (*peer_id).to_string();
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move {
				server.refresh_peer_webcams(&peer_id).await;
			});
		}
		["eye"] => {
			set_client_page(client, Page::PuppyEye).await;
			let server = Arc::clone(server);
			let refresh_client = Arc::clone(client);
			spawn_client_refresh(client, async move {
				server.refresh_puppy_eye().await;
				UiClientActions::new(&refresh_client).load_all_eye_camera_profiles();
			});
		}
		["device", peer_id, "camera", source_id] => {
			set_client_page(
				client,
				Page::PuppyEyeCamera {
					peer_id: (*peer_id).to_string(),
					source_id: decode_query_value(source_id),
				},
			)
			.await;
			let server = Arc::clone(server);
			let refresh_client = Arc::clone(client);
			spawn_client_refresh(client, async move {
				server.refresh_puppy_eye().await;
				UiClientActions::new(&refresh_client).load_all_eye_camera_profiles();
			});
		}
		["onvif-cameras"] => {
			set_client_page(client, Page::OnvifCameras).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move { server.refresh_puppy_eye().await });
		}
		["onvif-cameras", peer_id, camera_id] => {
			set_client_page(
				client,
				Page::OnvifCamera {
					peer_id: (*peer_id).to_string(),
					camera_id: (*camera_id).to_string(),
				},
			)
			.await;
			let server = Arc::clone(server);
			let refresh_client = Arc::clone(client);
			spawn_client_refresh(client, async move {
				server.refresh_puppy_eye().await;
				UiClientActions::new(&refresh_client).load_all_eye_camera_profiles();
			});
		}
		["files"] => {
			set_client_page(client, Page::Files).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move { server.refresh_files().await });
		}
		["search"] => {
			set_client_page(client, Page::Search).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move {
				server.refresh_search_mime_types().await;
			});
		}
		["storage"] => {
			set_client_page(client, Page::Storage).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move { server.refresh_storage().await });
		}
		["users"] => {
			set_client_page(client, Page::Users).await;
			let server = Arc::clone(server);
			spawn_client_refresh(client, async move { server.refresh_users().await });
		}
		["updates"] => set_client_page(client, Page::Updates).await,
		["settings"] => set_client_page(client, Page::Settings).await,
		["login"] => {}
		_ => {}
	}
}

async fn handle_client_event(client: &Arc<UiClientContext>, event: &ClientEvent) {
	let core = UiClientActions::new(client);
	match event {
		ClientEvent::PathChanged(change) => load_client_path(client, change.path.clone()).await,
		ClientEvent::OnTextChanged(change) if change.id == INPUT_LOGIN_USERNAME => {
			core.edit_login_username(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_LOGIN_PASSWORD => {
			core.edit_login_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_SEARCH_NAME => {
			core.edit_search_name_query(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_CURRENT_PASSWORD => {
			core.edit_current_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_NEW_PASSWORD => {
			core.edit_new_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_CONFIRM_PASSWORD => {
			core.edit_confirm_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_NEW_USER_USERNAME => {
			core.edit_new_user_username(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_NEW_USER_PASSWORD => {
			core.edit_new_user_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_ONVIF_NAME => {
			core.edit_onvif_name(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_ONVIF_URL => {
			core.edit_onvif_device_service_url(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_ONVIF_USERNAME => {
			core.edit_onvif_username(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_ONVIF_PASSWORD => {
			core.edit_onvif_password(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_ONVIF_PROFILE => {
			core.edit_onvif_profile_token(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_SHELL => {
			core.edit_shell_input(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_UPDATE_VERSION => {
			core.edit_update_version(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_SHARED_FOLDER => {
			core.edit_shared_folder_path(change.value.clone());
		}
		ClientEvent::OnTextChanged(change) if change.id == INPUT_CONTROL_TEXT => {
			core.edit_control_text(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_SEARCH_TARGET => {
			core.select_search_target(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_SEARCH_SORT => {
			core.select_search_sort(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_SEARCH_PAGE_SIZE => {
			core.select_search_page_size(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_ONVIF_TARGET => {
			core.select_onvif_target_device(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_ONVIF_PROFILE => {
			core.edit_onvif_profile_token(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_ONVIF_IDLE_TIMEOUT => {
			core.select_onvif_idle_timeout(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_EYE_PROFILE => {
			core.select_eye_camera_profile(change.inx.unwrap_or_default(), change.value.clone());
		}
		ClientEvent::OnCustom(change)
			if change.id == INPUT_EYE_PROFILE && change.name == "resolutionChange" =>
		{
			if let Some(value) = change.payload.as_str() {
				core.select_eye_camera_profile(change.inx.unwrap_or_default(), value.to_string());
			}
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_EYE_IDLE_TIMEOUT => {
			core.select_eye_camera_idle_timeout(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_SHARED_ACCESS => {
			core.select_shared_folder_access(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_AUDIO_DEVICE => {
			core.select_audio_device(change.value.clone());
		}
		ClientEvent::OnSelect(change) if change.id == INPUT_MICROPHONE => {
			core.select_microphone(change.value.clone());
		}
		ClientEvent::OnSliderChange(change) if change.id == INPUT_AUDIO_VOLUME => {
			core.set_audio_volume(change.value);
		}
		ClientEvent::OnSliderChange(change) if change.id == INPUT_EYE_GRID_WIDTH => {
			core.set_eye_grid_width(change.value);
		}
		ClientEvent::OnClick(click) if click.id == ACTION_LOGIN => core.login(),
		ClientEvent::OnClick(click) if click.id == ACTION_LOGOUT => core.logout(),
		ClientEvent::OnClick(click) if click.id == ACTION_REFRESH => {
			match client.path.lock().unwrap().as_str() {
				"/devices" | "/peers" => core.refresh_peers(),
				"/eye" => core.refresh_puppy_eye(),
				"/files" => core.refresh_files(),
				"/storage" => core.refresh_storage(),
				"/users" => core.refresh_users(),
				"/onvif-cameras" => core.refresh_puppy_eye(),
				_ => {}
			}
		}
		ClientEvent::OnClick(click) if click.id == ACTION_PEER => {
			core.peer_row(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_SEARCH => core.run_search(),
		ClientEvent::OnClick(click) if click.id == ACTION_SEARCH_MORE => core.search_load_more(),
		ClientEvent::OnClick(click) if click.id == ACTION_SEARCH_MIME => {
			core.toggle_search_mime(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_SEARCH_CLEAR_MIMES => {
			core.clear_search_mimes();
		}
		ClientEvent::OnClick(click) if click.id == ACTION_SEARCH_PREVIEW => {
			if client.path.lock().unwrap().as_str() == "/files" {
				core.preview_local_file(click.inx.unwrap_or_default());
			} else {
				core.search_preview(click.inx.unwrap_or_default());
			}
		}
		ClientEvent::OnClick(click) if click.id == ACTION_CLOSE_PREVIEW => {
			core.close_file_preview_modal();
		}
		ClientEvent::OnClick(click) if click.id == ACTION_CHANGE_PASSWORD => core.change_password(),
		ClientEvent::OnClick(click) if click.id == ACTION_OPEN_USER => core.open_new_user_modal(),
		ClientEvent::OnClick(click) if click.id == ACTION_CLOSE_USER => core.close_new_user_modal(),
		ClientEvent::OnClick(click) if click.id == ACTION_CREATE_USER => core.create_user(),
		ClientEvent::OnClick(click) if click.id == ACTION_DELETE_USER => {
			let snapshot = client.state.server.snapshot().await;
			if let Some(username) = snapshot.users.get(click.inx.unwrap_or_default() as usize) {
				core.delete_user_values_async(username.clone()).await;
			}
		}
		ClientEvent::OnClick(click) if click.id == ACTION_OPEN_ONVIF => {
			core.open_onvif_camera_modal();
		}
		ClientEvent::OnClick(click) if click.id == ACTION_EDIT_ONVIF => {
			core.edit_onvif_camera(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_CLOSE_ONVIF => {
			core.close_onvif_camera_modal();
		}
		ClientEvent::OnClick(click) if click.id == ACTION_TEST_ONVIF => core.test_onvif_camera(),
		ClientEvent::OnClick(click) if click.id == ACTION_SAVE_ONVIF => core.save_onvif_camera(),
		ClientEvent::OnClick(click) if click.id == ACTION_OPEN_CAMERA => {
			core.open_puppy_eye_camera(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_LOAD_EYE_PROFILE => {
			core.load_eye_camera_profiles(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_SAVE_EYE_PROFILE => {
			core.save_eye_camera_profile(click.inx.unwrap_or_default());
		}
		ClientEvent::OnClick(click) if click.id == ACTION_PEER_BACK => core.peer_back(),
		ClientEvent::OnClick(click) if click.id == ACTION_START_SHELL => core.start_shell(),
		ClientEvent::OnClick(click) if click.id == ACTION_SEND_SHELL => core.send_shell_input(),
		ClientEvent::OnClick(click) if click.id == ACTION_REFRESH_AUDIO => core.refresh_audio(),
		ClientEvent::OnClick(click) if click.id == ACTION_AUDIO_MUTE => core.toggle_audio_mute(),
		ClientEvent::OnClick(click) if click.id == ACTION_ADD_FOLDER => core.add_shared_folder(),
		ClientEvent::OnClick(click) if click.id == ACTION_START_UPDATE => core.start_peer_update(),
		ClientEvent::OnClick(click) if click.id == ACTION_TOGGLE_MONITOR => {
			core.toggle_monitor_stream();
		}
		ClientEvent::OnClick(click) if click.id == ACTION_SEND_CONTROL_TEXT => {
			core.send_control_text();
		}
		_ => {}
	}
}

async fn available_ui_addr(mut bind: SocketAddr) -> Result<SocketAddr> {
	loop {
		match tokio::net::TcpListener::bind(bind).await {
			Ok(listener) => {
				drop(listener);
				return Ok(bind);
			}
			Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
				let port = bind
					.port()
					.checked_add(1)
					.with_context(|| format!("no available PuppyNet UI port after {bind}"))?;
				bind.set_port(port);
			}
			Err(err) => {
				return Err(err).with_context(|| format!("failed to bind PuppyNet UI on {bind}"));
			}
		}
	}
}

pub async fn run_ui(puppy: Arc<PuppyNet>, bind: SocketAddr) -> Result<()> {
	let bind = available_ui_addr(bind).await?;
	log::info!("starting PuppyNet UI on {}", bind);
	let mut wgui = Wgui::new(bind);
	wgui.set_css(include_str!("ui_style.css"));
	let server_state = Arc::new(UiServer::new(puppy)?);
	server_state.refresh_all().await;

	let ctx = Arc::new(UiContext {
		server: Arc::clone(&server_state),
		sessions: std::sync::Mutex::new(HashMap::new()),
		pending_login_tokens: std::sync::Mutex::new(HashMap::new()),
	});
	let http_ctx = Arc::clone(&ctx);
	wgui.set_http_handler(move |request| {
		let http_ctx = Arc::clone(&http_ctx);
		async move { handle_ui_http(request, http_ctx).await }
	});
	let handle = wgui.handle();
	let mut clients = HashMap::<usize, Arc<UiClientContext>>::new();
	let mut camera_poll = tokio::time::interval(std::time::Duration::from_secs(5));
	camera_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
	loop {
		tokio::select! {
			message = wgui.next() => {
				let Some(message) = message else {
					break;
				};
				let client_id = message.client_id;
				if matches!(message.event, ClientEvent::Disconnected { .. }) {
					clients.remove(&client_id);
					continue;
				}
				if let std::collections::hash_map::Entry::Vacant(entry) = clients.entry(client_id) {
					entry.insert(Arc::new(UiClientContext {
						client_id,
						handle: handle.clone(),
						page: std::sync::Mutex::new(Page::Home),
						path: std::sync::Mutex::new(String::from("/")),
						session_id: wgui.session_for_client(client_id).await,
						state: Arc::clone(&ctx),
					}));
				}
				let client = clients.get(&client_id).expect("client was inserted");
				handle_client_event(client, &message.event).await;
				let snapshot = client_snapshot(client).await;
				let view = UiClientActions::new(client).state_from_snapshot(snapshot);
				let title = if view.authenticated {
					page_title(&view.page)
				} else {
					String::from("Login - PuppyNet UI")
				};
				wgui.set_title(client_id, &title).await;
				wgui.render(client_id, render_ui(&view)).await;
			}
			_ = camera_poll.tick() => {
				let puppy_eye_clients = clients
					.values()
					.filter(|client| {
						let path = client.path.lock().unwrap();
						path.as_str() == "/eye"
							|| path.starts_with("/device/") && path.contains("/camera/")
					})
					.map(|client| client.client_id)
					.collect::<Vec<_>>();
				if !puppy_eye_clients.is_empty() {
					let server = Arc::clone(&server_state);
					let handle = handle.clone();
					tokio::spawn(async move {
						server.refresh_puppy_eye().await;
						for client_id in puppy_eye_clients {
							handle.refresh(client_id);
						}
					});
				}
			}
			_ = signal::ctrl_c() => {
				log::info!("shutting down UI");
				break;
			}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	use wgui::ItemPayload;

	fn collect_text_input_types(item: &wgui::Item, out: &mut Vec<String>) {
		match &item.payload {
			ItemPayload::TextInput { input_type, .. } => out.push(input_type.clone()),
			ItemPayload::Form { body, .. } => {
				for child in body {
					collect_text_input_types(child, out);
				}
			}
			ItemPayload::Layout(layout) => {
				for child in &layout.body {
					collect_text_input_types(child, out);
				}
			}
			_ => {}
		}
	}

	#[tokio::test]
	async fn ui_addr_advances_past_occupied_port() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let occupied = listener.local_addr().unwrap();
		let available = available_ui_addr(occupied).await.unwrap();

		assert!(available.port() > occupied.port());
	}

	#[test]
	fn media_routes_require_exact_paths() {
		assert_eq!(
			peer_webcam_stream_peer("/api/peers/peer-id/webcam.mjpeg"),
			Some("peer-id")
		);
		assert_eq!(
			peer_webcam_stream_peer("/api/peers/peer-id/webcam.mjpeg/extra"),
			None
		);
		assert_eq!(
			peer_media_sessions_peer("/api/peers/peer-id/media/sessions"),
			Some("peer-id")
		);
		assert_eq!(
			peer_media_sessions_peer("/api/peers/peer-id/media/sessions/extra"),
			None
		);
		assert_eq!(
			media_session_id("/api/media/sessions/session-id"),
			Some("session-id")
		);
		assert_eq!(
			media_session_id("/api/media/sessions/session-id/extra"),
			None
		);
	}

	#[test]
	fn mjpeg_chunk_removes_padding_after_jpeg() {
		let chunk = mjpeg_frame_chunk("image/jpeg", &[0xff, 0xd8, 1, 2, 0xff, 0xd9, 0, 0]);

		assert!(chunk.ends_with(&[0xff, 0xd9, b'\r', b'\n']));
		assert!(!chunk.windows(4).any(|window| window == [0xff, 0xd9, 0, 0]));
	}

	#[test]
	fn eye_camera_href_uses_device_route() {
		assert_eq!(
			puppy_eye_view_href("node-id", "/dev/video0"),
			"/device/node-id/camera/%2Fdev%2Fvideo0"
		);
	}

	#[test]
	fn pairing_endpoint_uses_the_web_ui_host() {
		assert_eq!(
			pairing_endpoint("puppy.local:8080").unwrap(),
			"ws://puppy.local:8833/peer/v1"
		);
		assert_eq!(
			pairing_endpoint("[fd00::1]:8080").unwrap(),
			"ws://[fd00::1]:8833/peer/v1"
		);
	}

	#[test]
	fn media_signaling_rejects_cross_origin_requests() {
		let request = HttpRequest {
			method: String::from("POST"),
			path: String::from("/api/peers/peer-id/media/sessions"),
			query: HashMap::new(),
			headers: HashMap::from([
				(String::from("host"), String::from("10.0.0.2:8080")),
				(String::from("origin"), String::from("http://example.com")),
			]),
			body: Vec::new(),
		};

		assert!(!same_origin_request(&request));
	}

	#[test]
	fn login_password_field_renders_as_password_input() {
		let rendered = render_login(&UiViewState::default());
		let mut input_types = Vec::new();
		collect_text_input_types(&rendered, &mut input_types);

		assert_eq!(
			input_types,
			vec!["text".to_string(), "password".to_string()]
		);
	}

	#[test]
	fn settings_password_fields_render_as_password_inputs() {
		let rendered = render_settings(&UiViewState::default());
		let mut input_types = Vec::new();
		collect_text_input_types(&rendered, &mut input_types);

		assert_eq!(
			input_types,
			vec![
				"password".to_string(),
				"password".to_string(),
				"password".to_string(),
			]
		);
	}
}
