use blake3::Hasher;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::FileEntry;
use crate::scan::{ScanEvent, ScanResult};
use crate::state::{FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FolderRule, Permission, Rule};
use crate::types::FileChunk;
use crate::updater::UpdateProgress;
use crate::wait_group::WaitGroupGuard;

const OWNER_ROLE: &str = "owner";
const VIEWER_ROLE: &str = "viewer";
const DEFAULT_SESSION_TTL: u64 = 60 * 60; // 1 hour sessions for credential auth

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerReq {
	PeerInfo,
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
	AudioCapability,
	ListAudioDevices,
	SetAudioMuted {
		device_id: Option<String>,
		muted: bool,
	},
	SetAudioVolume {
		device_id: Option<String>,
		volume: u8,
	},
	SetDefaultAudioDevice {
		device_id: String,
	},
	MediaCapability,
	ListMediaSources,
	GetMediaFrame {
		source_id: String,
	},
	ListOnvifCameras,
	TestOnvifCamera {
		config: OnvifCameraConfig,
	},
	SaveOnvifCamera {
		config: OnvifCameraConfig,
	},
	DeleteOnvifCamera {
		id: String,
	},
	StartScan {
		id: u64,
		path: String,
	},
	FileEntries {
		offset: u64,
		limit: u64,
	},
	StartSearch {
		id: u64,
		args: LiveSearchArgs,
	},
	SearchEvent {
		id: u64,
		event: SearchEvent,
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
	/// Start a long-lived remote shell session on this peer.
	StartShell {
		id: u64,
	},
	/// Send input bytes to an existing shell session.
	ShellInput {
		id: u64,
		data: Vec<u8>,
	},
	/// Send desktop mouse or keyboard input to this peer.
	DesktopInput {
		input: DesktopInput,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerRes {
	PeerInfo(PeerInfo),
	DirEntries(Vec<DirEntry>),
	FileStat(DirEntry),
	FileChunk(FileChunk),
	WriteAck(FileWriteAck),
	Cpus(Vec<CpuInfo>),
	Disks(Vec<DiskInfo>),
	Interfaces(Vec<InterfaceInfo>),
	AudioCapability(AudioCapability),
	AudioDevices(Vec<AudioDevice>),
	MediaCapability(MediaCapability),
	MediaSources(Vec<MediaSource>),
	MediaFrame(MediaFrame),
	OnvifCameras(Vec<OnvifCamera>),
	OnvifCameraMutation(OnvifCameraMutation),
	FileEntries(Vec<FileEntry>),
	SearchStarted(Result<(), String>),
	SearchEventAck,
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
	/// Acknowledgment that a shell session was created.
	ShellStarted {
		id: u64,
	},
	/// Output bytes from a shell session.
	ShellOutput {
		id: u64,
		data: Vec<u8>,
	},
	/// Shell session exited or was closed.
	ShellExited {
		id: u64,
	},
	/// Acknowledgment for desktop mouse or keyboard input.
	DesktopInputAck(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchSort {
	Latest,
	Name,
	Size,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSearchArgs {
	pub name_query: Option<String>,
	pub mime_types: Vec<String>,
	pub sort: SearchSort,
	pub sort_desc: bool,
	pub page: usize,
	pub page_size: usize,
}

impl Default for LiveSearchArgs {
	fn default() -> Self {
		Self {
			name_query: None,
			mime_types: Vec::new(),
			sort: SearchSort::Latest,
			sort_desc: true,
			page: 0,
			page_size: 50,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSearchRow {
	pub name: String,
	pub path: String,
	pub size: u64,
	pub mime_type: Option<String>,
	pub modified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchEvent {
	Progress { visited: usize, matched: usize },
	Rows { rows: Vec<LiveSearchRow> },
	Finished { total: usize, truncated: bool },
	Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
	pub version: String,
	#[serde(default)]
	pub os: String,
	#[serde(default)]
	pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumbnail {
	pub data: Vec<u8>,
	pub width: u32,
	pub height: u32,
	pub mime_type: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MouseButton {
	Left,
	Middle,
	Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DesktopInput {
	MouseMove { dx: i32, dy: i32 },
	MouseScroll { amount: i32 },
	MouseClick { button: MouseButton },
	MousePress { button: MouseButton },
	MouseRelease { button: MouseButton },
	KeyboardText { text: String },
	KeyboardKey { key: String },
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
pub enum AudioDeviceKind {
	Sink,
	Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
	pub id: String,
	pub name: String,
	pub description: String,
	pub kind: AudioDeviceKind,
	pub volume: u8,
	pub muted: bool,
	pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCapability {
	pub supported: bool,
	pub backend: Option<String>,
	pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCapability {
	pub supported: bool,
	pub backend: Option<String>,
	pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaSourceKind {
	Webcam,
	Screen,
	Microphone,
	SpeakerMonitor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaTransport {
	Snapshot,
	Stream,
	WebRtc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOutput {
	pub transport: MediaTransport,
	pub mime: String,
	pub codec: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSource {
	pub id: String,
	pub name: String,
	pub kind: MediaSourceKind,
	pub live: bool,
	pub outputs: Vec<MediaOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFrame {
	pub mime: String,
	#[serde(with = "base64_bytes")]
	pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifCameraConfig {
	pub id: String,
	pub name: String,
	pub device_service_url: String,
	pub username: String,
	pub password: String,
	pub profile_token: String,
	pub enabled: bool,
	#[serde(default = "default_camera_idle_timeout_seconds")]
	pub idle_timeout_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifCamera {
	pub id: String,
	pub name: String,
	pub device_service_url: String,
	pub username: String,
	pub profile_token: String,
	pub enabled: bool,
	pub idle_timeout_seconds: u32,
}

fn default_camera_idle_timeout_seconds() -> u32 {
	5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifProfile {
	pub token: String,
	pub name: String,
	pub width: u32,
	pub height: u32,
	#[serde(default)]
	pub frame_rate_limit: u32,
}

impl From<&OnvifCameraConfig> for OnvifCamera {
	fn from(config: &OnvifCameraConfig) -> Self {
		Self {
			id: config.id.clone(),
			name: config.name.clone(),
			device_service_url: config.device_service_url.clone(),
			username: config.username.clone(),
			profile_token: config.profile_token.clone(),
			enabled: config.enabled,
			idle_timeout_seconds: config.idle_timeout_seconds,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifCameraMutation {
	pub camera: OnvifCamera,
	pub message: String,
	#[serde(default)]
	pub profiles: Vec<OnvifProfile>,
}

mod base64_bytes {
	use base64::Engine;
	use serde::Deserialize;

	pub fn serialize<S>(data: &Vec<u8>, serializer: S) -> std::result::Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data))
	}

	pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Vec<u8>, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		#[derive(Deserialize)]
		#[serde(untagged)]
		enum EncodedBytes {
			Base64(String),
			Bytes(Vec<u8>),
		}

		match EncodedBytes::deserialize(deserializer)? {
			EncodedBytes::Base64(data) => base64::engine::general_purpose::STANDARD
				.decode(data)
				.map_err(serde::de::Error::custom),
			EncodedBytes::Bytes(data) => Ok(data),
		}
	}
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

pub(crate) fn permission_from_grant(grant: &PermissionGrant) -> Option<Permission> {
	match grant {
		PermissionGrant::Owner => Some(Permission::new(Rule::Owner)),
		PermissionGrant::Viewer => Some(Permission::new(Rule::Folder(FolderRule::new(
			PathBuf::from("/"),
			FLAG_READ | FLAG_SEARCH,
		)))),
		PermissionGrant::Files { path, access } => {
			let normalized = normalize_path(path);
			let mut flags = FLAG_READ | FLAG_SEARCH;
			if matches!(access, FileAccess::ReadWrite) {
				flags |= FLAG_WRITE;
			}
			Some(Permission::new(Rule::Folder(FolderRule::new(
				PathBuf::from(normalized),
				flags,
			))))
		}
		PermissionGrant::SystemInfo | PermissionGrant::DiskInfo | PermissionGrant::NetworkInfo => {
			None
		}
	}
}

pub(crate) fn grant_from_permission(permission: &Permission) -> Option<PermissionGrant> {
	match permission.rule() {
		Rule::Owner => Some(PermissionGrant::Owner),
		Rule::Folder(rule) => {
			let access = if rule.can_write() {
				FileAccess::ReadWrite
			} else if rule.can_read() {
				FileAccess::Read
			} else {
				return None;
			};
			let path = normalize_path(&rule.path().to_string_lossy());
			Some(PermissionGrant::Files { path, access })
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn media_frame_data_serializes_as_base64_string() {
		let frame = MediaFrame {
			mime: String::from("image/jpeg"),
			data: vec![1, 2, 3, 255],
		};

		let json = serde_json::to_value(&frame).unwrap();
		let decoded: MediaFrame = serde_json::from_value(json.clone()).unwrap();

		assert_eq!(json["data"], "AQID/w==");
		assert_eq!(decoded.mime, "image/jpeg");
		assert_eq!(decoded.data, vec![1, 2, 3, 255]);
	}

	#[test]
	fn media_frame_data_deserializes_legacy_byte_array() {
		let json = serde_json::json!({
			"mime": "image/jpeg",
			"data": [1, 2, 3, 255],
		});

		let decoded: MediaFrame = serde_json::from_value(json).unwrap();

		assert_eq!(decoded.mime, "image/jpeg");
		assert_eq!(decoded.data, vec![1, 2, 3, 255]);
	}

	#[test]
	fn legacy_onvif_camera_config_gets_default_idle_timeout() {
		let json = serde_json::json!({
			"id": "front-door",
			"name": "Front door",
			"device_service_url": "http://camera/onvif/device_service",
			"username": "viewer",
			"password": "secret",
			"profile_token": "main",
			"enabled": true,
		});

		let config: OnvifCameraConfig = serde_json::from_value(json).unwrap();

		assert_eq!(config.idle_timeout_seconds, 5);
	}
}
