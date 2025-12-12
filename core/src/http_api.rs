use crate::auth;
use crate::puppynet::PuppyNet;
use crate::scan::ScanEvent;
use crate::updater::UpdateProgress;
use crate::{Permission, SearchFilesArgs};
use anyhow::Result;
use futures::stream::unfold;
use hyper::body::{Buf, Bytes};
use hyper::header::{
	ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
	ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_LENGTH, CONTENT_RANGE,
	CONTENT_TYPE, HeaderValue, ORIGIN, RANGE, SET_COOKIE,
};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use libp2p::PeerId;
use log::warn;
use mime_guess::from_path;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::fmt::Write;
use std::fs;
use std::io::{ErrorKind, SeekFrom};
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::{signal, task};
use url::form_urlencoded;

const CT_JSON: &str = "application/json";
const SESSION_COOKIE: &str = "sid";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 7;

#[derive(Deserialize)]
struct CreateUserRequest {
	username: String,
	password: String,
}

#[derive(Deserialize)]
struct LoginRequest {
	username: String,
	password: String,
	set_cookie: Option<bool>,
}

#[derive(Deserialize)]
struct PermissionsRequest {
	permissions: Vec<Permission>,
	merge: Option<bool>,
}

#[derive(Deserialize)]
struct SetPermissionsRequest {
	permissions: Vec<Permission>,
}

#[derive(Deserialize)]
struct ScanStartRequest {
	path: String,
}

#[derive(Deserialize)]
struct UpdateStartRequest {
	version: Option<String>,
}

#[derive(Serialize)]
struct StateResponse {
	me: String,
	peers: Vec<PeerSummary>,
	discovered: Vec<DiscoveredSummary>,
	users: Vec<UserSummary>,
	shared_folders: Vec<SharedFolderSummary>,
}

#[derive(Serialize)]
struct PeerSummary {
	id: String,
	name: Option<String>,
	node_id: Option<String>,
}

#[derive(Serialize)]
struct DiscoveredSummary {
	peer_id: String,
	multiaddr: String,
}

#[derive(Serialize)]
struct UserSummary {
	name: String,
}

#[derive(Serialize)]
struct SharedFolderSummary {
	path: String,
	flags: u8,
}

struct ApiState {
	puppy: Arc<PuppyNet>,
	scans: Mutex<HashMap<u64, crate::puppynet::ScanHandle>>,
	next_scan_id: AtomicU64,
	updates: Mutex<HashMap<u64, Arc<Mutex<std::sync::mpsc::Receiver<UpdateProgress>>>>>,
	next_update_id: AtomicU64,
	jwt_secret: String,
}

impl ApiState {
	fn new(puppy: Arc<PuppyNet>, jwt_secret: String) -> Self {
		Self {
			puppy,
			scans: Mutex::new(HashMap::new()),
			next_scan_id: AtomicU64::new(1),
			updates: Mutex::new(HashMap::new()),
			next_update_id: AtomicU64::new(1),
			jwt_secret,
		}
	}

	fn insert_scan(&self, handle: crate::puppynet::ScanHandle) -> u64 {
		let id = self.next_scan_id.fetch_add(1, Ordering::SeqCst);
		self.scans.lock().unwrap().insert(id, handle);
		id
	}

	fn poll_scan(&self, id: u64) -> Option<Vec<ScanEvent>> {
		let mut scans = self.scans.lock().unwrap();
		let handle = scans.get(&id)?;
		let receiver = handle.receiver();
		let mut rx = receiver.lock().unwrap();
		let mut events = Vec::new();
		while let Ok(event) = rx.try_recv() {
			let is_done = matches!(event, ScanEvent::Finished(_));
			events.push(event);
			if is_done {
				scans.remove(&id);
				break;
			}
		}
		Some(events)
	}

	fn cancel_scan(&self, id: u64) -> bool {
		let mut scans = self.scans.lock().unwrap();
		if let Some(handle) = scans.remove(&id) {
			handle.cancel();
			return true;
		}
		false
	}

	fn insert_update(&self, rx: Arc<Mutex<std::sync::mpsc::Receiver<UpdateProgress>>>) -> u64 {
		let id = self.next_update_id.fetch_add(1, Ordering::SeqCst);
		self.updates.lock().unwrap().insert(id, rx);
		id
	}

	fn poll_update(&self, id: u64) -> Option<Vec<UpdateProgress>> {
		let mut updates = self.updates.lock().unwrap();
		let Some(rx) = updates.get(&id) else {
			return None;
		};
		let guard = rx.lock().unwrap();
		let mut events = Vec::new();
		let mut should_remove = false;
		for progress in guard.try_iter() {
			if matches!(
				progress,
				UpdateProgress::Completed { .. }
					| UpdateProgress::Failed { .. }
					| UpdateProgress::AlreadyUpToDate { .. }
			) {
				should_remove = true;
			}
			events.push(progress);
		}
		drop(guard);
		if should_remove {
			updates.remove(&id);
		}
		Some(events)
	}
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response<Body> {
	Response::builder()
		.status(status)
		.header(hyper::header::CONTENT_TYPE, CT_JSON)
		.body(Body::from(value.to_string()))
		.unwrap()
}

fn bad_request(msg: impl Into<String>) -> Response<Body> {
	json_response(StatusCode::BAD_REQUEST, json!({ "error": msg.into() }))
}

fn parse_query(req: &Request<Body>) -> HashMap<String, String> {
	form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
		.into_owned()
		.collect()
}

fn parse_peer_id(id: &str) -> Result<PeerId, String> {
	PeerId::from_str(id).map_err(|e| format!("invalid peer id: {e}"))
}

fn cookie_value(req: &Request<Body>, name: &str) -> Option<String> {
	let raw = req.headers().get(hyper::header::COOKIE)?;
	let header = raw.to_str().ok()?;
	for part in header.split(';') {
		let mut kv = part.trim().splitn(2, '=');
		if let (Some(key), Some(value)) = (kv.next(), kv.next()) {
			if key == name {
				return Some(value.to_string());
			}
		}
	}
	None
}

fn session_cookie(token: &str, ttl_secs: i64) -> Option<HeaderValue> {
	if ttl_secs <= 0 {
		return None;
	}
	let cookie = format!(
		"{}={}; HttpOnly; Path=/; Max-Age={}; SameSite=Lax",
		SESSION_COOKIE, token, ttl_secs
	);
	HeaderValue::from_str(&cookie).ok()
}

fn clear_session_cookie() -> HeaderValue {
	HeaderValue::from_static("sid=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax")
}

fn bearer_token(req: &Request<Body>) -> Option<String> {
	let header = req
		.headers()
		.get(hyper::header::AUTHORIZATION)?
		.to_str()
		.ok()?;
	let (kind, token) = header.split_once(' ')?;
	kind.eq_ignore_ascii_case("bearer")
		.then(|| token.to_string())
}

fn authenticate(req: &Request<Body>, state: &Arc<ApiState>) -> Option<String> {
	if let Some(token) = bearer_token(req) {
		if let Ok(claims) = auth::verify_jwt(&token, state.jwt_secret.as_bytes()) {
			return Some(claims.sub);
		}
	}
	if let Some(sid) = cookie_value(req, SESSION_COOKIE) {
		let hash = auth::token_hash(&sid);
		if let Ok(Some(username)) = state.puppy.http_me(&hash) {
			return Some(username);
		}
	}
	None
}

#[cfg(not(debug_assertions))]
fn load_asset(name: &str) -> Option<Cow<'static, [u8]>> {
	match name {
		"index.html" => Some(Cow::Borrowed(include_bytes!("../http_assets/index.html"))),
		"index.css" => Some(Cow::Borrowed(include_bytes!("../http_assets/index.css"))),
		"index.js" => Some(Cow::Borrowed(include_bytes!("../http_assets/index.js"))),
		_ => None,
	}
}

#[cfg(debug_assertions)]
fn load_asset(name: &str) -> Option<Cow<'static, [u8]>> {
	let path = Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("http_assets")
		.join(name);
	std::fs::read(path).ok().map(Cow::Owned)
}

fn load_dist_asset(name: &str) -> Option<Vec<u8>> {
	if name.contains("..") {
		return None;
	}
	let path = Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("..")
		.join("web")
		.join("dist")
		.join(name);
	if path.is_dir() {
		return None;
	}
	fs::read(path).ok()
}

fn asset_response(path: &str, data: impl Into<Body>) -> Response<Body> {
	let content_type = from_path(path)
		.first_or_octet_stream()
		.essence_str()
		.to_string();
	Response::builder()
		.status(StatusCode::OK)
		.header(hyper::header::CONTENT_TYPE, content_type)
		.body(data.into())
		.unwrap()
}

fn serve_static_path(path: &str) -> Option<Response<Body>> {
	let path = if path.is_empty() { "index.html" } else { path };
	if let Some(data) = load_dist_asset(path) {
		return Some(asset_response(path, data));
	}
	load_asset(path).map(|data| asset_response(path, data))
}

fn with_cors(mut resp: Response<Body>, origin: Option<&str>) -> Response<Body> {
	let origin_value: HeaderValue = origin
		.unwrap_or("*")
		.parse()
		.unwrap_or_else(|_| HeaderValue::from_static("*"));
	resp.headers_mut()
		.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin_value);
	resp.headers_mut().insert(
		ACCESS_CONTROL_ALLOW_HEADERS,
		"content-type,authorization".parse().unwrap(),
	);
	resp.headers_mut().insert(
		ACCESS_CONTROL_ALLOW_METHODS,
		"GET,POST,PUT,DELETE,OPTIONS".parse().unwrap(),
	);
	resp.headers_mut().insert(
		ACCESS_CONTROL_ALLOW_CREDENTIALS,
		HeaderValue::from_static("true"),
	);
	resp
}

fn bytes_to_hex(bytes: &[u8]) -> String {
	let mut buf = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		write!(buf, "{:02x}", byte).ok();
	}
	buf
}

fn peer_to_node_id(peer: &PeerId) -> Option<[u8; 16]> {
	let mut node_id = [0u8; 16];
	let bytes = peer.to_bytes();
	let len = node_id.len();
	if bytes.len() < len {
		return None;
	}
	node_id.copy_from_slice(&bytes[..len]);
	Some(node_id)
}

enum RangeParseError {
	Invalid,
	Unsatisfiable,
}

fn range_not_satisfiable_response(total: u64) -> Response<Body> {
	let mut resp = Response::builder()
		.status(StatusCode::RANGE_NOT_SATISFIABLE)
		.header(CONTENT_RANGE, format!("bytes */{}", total))
		.body(Body::empty())
		.unwrap();
	resp.headers_mut()
		.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
	resp
}

const READ_CHUNK_SIZE: usize = 64 * 1024;

fn hex_value(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		b'A'..=b'F' => Some(byte - b'A' + 10),
		_ => None,
	}
}

fn parse_hash_param(value: &str) -> Result<[u8; 32], &'static str> {
	let trimmed = value.trim();
	let stripped = trimmed
		.strip_prefix("0x")
		.or_else(|| trimmed.strip_prefix("0X"))
		.unwrap_or(trimmed);
	if stripped.is_empty() {
		return Err("hash cannot be empty");
	}
	if stripped.len() != 64 {
		return Err("hash must be 32 bytes");
	}
	let bytes = stripped.as_bytes();
	let mut hash = [0u8; 32];
	for i in 0..32 {
		let hi = hex_value(bytes[i * 2]).ok_or("invalid hash")?;
		let lo = hex_value(bytes[i * 2 + 1]).ok_or("invalid hash")?;
		hash[i] = (hi << 4) | lo;
	}
	Ok(hash)
}

fn parse_range_header(value: &str, total: u64) -> Result<(u64, u64), RangeParseError> {
	let trimmed = value.trim();
	if trimmed.len() < 6 {
		return Err(RangeParseError::Invalid);
	}
	if !trimmed[..6].eq_ignore_ascii_case("bytes=") {
		return Err(RangeParseError::Invalid);
	}
	let range_part = trimmed[6..].trim();
	if range_part.is_empty() {
		return Err(RangeParseError::Invalid);
	}
	let first_range = range_part.split(',').next().unwrap_or("").trim();
	let mut parts = first_range.splitn(2, '-');
	let start_str = parts.next().unwrap_or("").trim();
	let end_str = parts.next().unwrap_or("").trim();
	if start_str.is_empty() && end_str.is_empty() {
		return Err(RangeParseError::Invalid);
	}
	if total == 0 {
		return Err(RangeParseError::Unsatisfiable);
	}
	if start_str.is_empty() {
		let suffix = end_str
			.parse::<u64>()
			.map_err(|_| RangeParseError::Invalid)?;
		if suffix == 0 {
			return Err(RangeParseError::Unsatisfiable);
		}
		let start = if suffix >= total { 0 } else { total - suffix };
		let end = total.saturating_sub(1);
		return Ok((start, end));
	}
	let start = start_str
		.parse::<u64>()
		.map_err(|_| RangeParseError::Invalid)?;
	if start >= total {
		return Err(RangeParseError::Unsatisfiable);
	}
	let mut end = if end_str.is_empty() {
		total.saturating_sub(1)
	} else {
		end_str
			.parse::<u64>()
			.map_err(|_| RangeParseError::Invalid)?
	};
	if total > 0 {
		end = end.min(total.saturating_sub(1));
	}
	if start > end {
		return Err(RangeParseError::Unsatisfiable);
	}
	Ok((start, end))
}

fn load_jwt_secret() -> String {
	if let Ok(value) = env::var("JWT_SECRET") {
		let trimmed = value.trim();
		if !trimmed.is_empty() {
			return trimmed.to_string();
		}
	}
	let mut bytes = [0u8; 32];
	OsRng.fill_bytes(&mut bytes);
	let fallback: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
	warn!("JWT_SECRET not set; using ephemeral secret");
	fallback
}

async fn handle_request(
	req: Request<Body>,
	state: Arc<ApiState>,
) -> Result<Response<Body>, Infallible> {
	let origin = req
		.headers()
		.get(ORIGIN)
		.and_then(|v| v.to_str().ok())
		.map(|v| v.to_string());
	let origin_ref = origin.as_deref();
	let segments: Vec<&str> = req
		.uri()
		.path()
		.split('/')
		.filter(|s| !s.is_empty())
		.collect();
	let is_protected = matches!(segments.as_slice(), ["api", ..]);
	let auth_user = if is_protected || matches!(segments.as_slice(), ["auth", "me"]) {
		authenticate(&req, &state)
	} else {
		None
	};
	if is_protected && req.method() != Method::OPTIONS && auth_user.is_none() {
		let resp = json_response(
			StatusCode::UNAUTHORIZED,
			json!({ "error": "not authenticated" }),
		);
		return Ok(with_cors(resp, origin_ref));
	}

	let response = match (req.method(), segments.as_slice()) {
		(&Method::OPTIONS, _) => Response::builder()
			.status(StatusCode::NO_CONTENT)
			.body(Body::empty())
			.unwrap(),
		(&Method::GET, ["health"]) => Response::new(Body::from("ok")),
		(&Method::POST, ["auth", "login"]) => {
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<LoginRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => {
					let creds_ok = match state
						.puppy
						.verify_user_credentials(&payload.username, &payload.password)
					{
						Ok(valid) => valid,
						Err(err) => {
							return Ok(with_cors(
								json_response(
									StatusCode::INTERNAL_SERVER_ERROR,
									json!({ "error": err.to_string() }),
								),
								origin_ref,
							));
						}
					};
					if !creds_ok {
						return Ok(with_cors(
							json_response(
								StatusCode::UNAUTHORIZED,
								json!({ "error": "invalid credentials" }),
							),
							origin_ref,
						));
					}
					let access_token =
						match auth::issue_jwt(&payload.username, state.jwt_secret.as_bytes()) {
							Ok(token) => token,
							Err(err) => {
								return Ok(with_cors(
									json_response(
										StatusCode::INTERNAL_SERVER_ERROR,
										json!({ "error": err.to_string() }),
									),
									origin_ref,
								));
							}
						};
					let mut resp =
						json_response(StatusCode::OK, json!({ "access_token": access_token }));
					if payload.set_cookie.unwrap_or(false) {
						let (token, hash) = auth::generate_session_token();
						if let Err(err) =
							state
								.puppy
								.save_session(&hash, &payload.username, SESSION_TTL_SECS)
						{
							return Ok(with_cors(
								json_response(
									StatusCode::INTERNAL_SERVER_ERROR,
									json!({ "error": err.to_string() }),
								),
								origin_ref,
							));
						}
						if let Some(cookie) = session_cookie(&token, SESSION_TTL_SECS) {
							resp.headers_mut().insert(SET_COOKIE, cookie);
						}
					}
					resp
				}
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::POST, ["auth", "logout"]) => {
			if let Some(sid) = cookie_value(&req, SESSION_COOKIE) {
				let hash = auth::token_hash(&sid);
				let _ = state.puppy.drop_session(&hash);
			}
			let mut resp = Response::builder()
				.status(StatusCode::NO_CONTENT)
				.body(Body::empty())
				.unwrap();
			resp.headers_mut()
				.insert(SET_COOKIE, clear_session_cookie());
			resp
		}
		(&Method::GET, ["auth", "me"]) => match auth_user {
			Some(user) => json_response(StatusCode::OK, json!({ "user": user })),
			None => json_response(
				StatusCode::UNAUTHORIZED,
				json!({ "error": "not authenticated" }),
			),
		},
		(&Method::GET, ["users"]) => match state.puppy.list_users_db() {
			Ok(list) => json_response(StatusCode::OK, json!({ "users": list })),
			Err(err) => json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": err })),
		},
		(&Method::POST, ["users"]) => {
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<CreateUserRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state
					.puppy
					.create_user(payload.username.clone(), payload.password)
				{
					Ok(()) => {
						json_response(StatusCode::CREATED, json!({ "username": payload.username }))
					}
					Err(err) => bad_request(err.to_string()),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::GET, ["api", "state"]) => {
			let snapshot = state.puppy.state_snapshot().await;
			let me = snapshot
				.as_ref()
				.map(|s| s.me.to_string())
				.unwrap_or_else(|| String::from("unknown"));
			let shared_folders = snapshot
				.as_ref()
				.map(|s| {
					s.shared_folders
						.iter()
						.map(|f| SharedFolderSummary {
							path: f.path().to_string_lossy().to_string(),
							flags: f.flags(),
						})
						.collect()
				})
				.unwrap_or_default();
			let peers = state
				.puppy
				.list_peers_db()
				.unwrap_or_default()
				.into_iter()
				.map(|p| {
					let node_id = peer_to_node_id(&p.id).map(|id| bytes_to_hex(&id));
					PeerSummary {
						id: p.id.to_string(),
						name: p.name,
						node_id,
					}
				})
				.collect();
			let discovered = state
				.puppy
				.list_discovered_peers_db()
				.unwrap_or_default()
				.into_iter()
				.map(|d| DiscoveredSummary {
					peer_id: d.peer_id.to_string(),
					multiaddr: d.multiaddr.to_string(),
				})
				.collect();
			let users = state
				.puppy
				.list_users_db()
				.unwrap_or_default()
				.into_iter()
				.map(|name| UserSummary { name })
				.collect();
			json_response(
				StatusCode::OK,
				json!(StateResponse {
					me,
					peers,
					discovered,
					users,
					shared_folders
				}),
			)
		}
		(&Method::GET, ["api", "peers"]) => {
			let peers = state
				.puppy
				.list_peers_db()
				.unwrap_or_default()
				.into_iter()
				.map(|p| json!({ "id": p.id.to_string(), "name": p.name }))
				.collect::<Vec<_>>();
			json_response(StatusCode::OK, json!({ "peers": peers }))
		}
		(&Method::GET, ["api", "peers", peer_id, "permissions"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			match state.puppy.list_permissions(peer).await {
				Ok(perms) => json_response(StatusCode::OK, json!({ "permissions": perms })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "permissions", "granted"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			match state.puppy.list_granted_permissions(peer) {
				Ok(perms) => json_response(StatusCode::OK, json!({ "permissions": perms })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::PUT, ["api", "peers", peer_id, "permissions"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<SetPermissionsRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state.puppy.set_peer_permissions(peer, payload.permissions) {
					Ok(()) => Response::builder()
						.status(StatusCode::NO_CONTENT)
						.body(Body::empty())
						.unwrap(),
					Err(err) => bad_request(err.to_string()),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::POST, ["api", "peers", peer_id, "permissions", "request"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<PermissionsRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state
					.puppy
					.request_permissions(peer, payload.permissions, payload.merge.unwrap_or(true))
					.await
				{
					Ok(ack) => json_response(StatusCode::OK, json!({ "permissions": ack })),
					Err(err) => bad_request(err.to_string()),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "dir"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let query = parse_query(&req);
			let path = query.get("path").cloned().unwrap_or_else(|| "/".into());
			match state.puppy.list_dir(peer, path).await {
				Ok(entries) => json_response(StatusCode::OK, json!({ "entries": entries })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "disks"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			match state.puppy.list_disks(peer).await {
				Ok(disks) => json_response(StatusCode::OK, json!({ "disks": disks })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "interfaces"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			match state.puppy.list_interfaces(peer).await {
				Ok(interfaces) => {
					json_response(StatusCode::OK, json!({ "interfaces": interfaces }))
				}
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "cpus"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			match state.puppy.list_cpus(peer).await {
				Ok(cpus) => json_response(StatusCode::OK, json!({ "cpus": cpus })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "file"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let query = parse_query(&req);
			let Some(path) = query.get("path") else {
				return Ok(with_cors(bad_request("missing path"), origin_ref));
			};
			let offset = query
				.get("offset")
				.and_then(|v| v.parse::<u64>().ok())
				.unwrap_or(0);
			let length = query.get("length").and_then(|v| v.parse::<u64>().ok());
			match state
				.puppy
				.read_file(peer, path.clone(), offset, length)
				.await
			{
				Ok(chunk) => json_response(StatusCode::OK, json!(chunk)),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "thumbnail"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let query = parse_query(&req);
			let Some(path) = query.get("path") else {
				return Ok(with_cors(bad_request("missing path"), origin_ref));
			};
			let max_width = query
				.get("max_width")
				.and_then(|v| v.parse::<u32>().ok())
				.unwrap_or(128);
			let max_height = query
				.get("max_height")
				.and_then(|v| v.parse::<u32>().ok())
				.unwrap_or(128);
			match state
				.puppy
				.get_thumbnail(peer, path.clone(), max_width, max_height)
				.await
			{
				Ok(thumb) => Response::builder()
					.status(StatusCode::OK)
					.header(hyper::header::CONTENT_TYPE, thumb.mime_type)
					.body(Body::from(thumb.data))
					.unwrap(),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "storage"]) => match state.puppy.list_storage_files().await {
			Ok(files) => json_response(StatusCode::OK, json!({ "files": files })),
			Err(err) => bad_request(err.to_string()),
		},
		(&Method::GET, ["api", "file", "hash"]) => {
			let query = parse_query(&req);
			let Some(raw_hash) = query.get("hash") else {
				return Ok(with_cors(bad_request("missing hash parameter"), origin_ref));
			};
			let hash_bytes = match parse_hash_param(raw_hash) {
				Ok(value) => value,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let (path, entry) = match state.puppy.resolve_local_file_by_hash(&hash_bytes) {
				Ok(Some(result)) => result,
				Ok(None) => {
					return Ok(with_cors(
						json_response(
							StatusCode::NOT_FOUND,
							json!({ "error": "file hash not found locally" }),
						),
						origin_ref,
					));
				}
				Err(err) => {
					return Ok(with_cors(
						json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": err })),
						origin_ref,
					));
				}
			};
			let mut file = match File::open(&path).await {
				Ok(file) => file,
				Err(err) => {
					let response = if matches!(err.kind(), ErrorKind::NotFound) {
						json_response(
							StatusCode::NOT_FOUND,
							json!({ "error": "file missing on disk" }),
						)
					} else {
						json_response(
							StatusCode::INTERNAL_SERVER_ERROR,
							json!({ "error": err.to_string() }),
						)
					};
					return Ok(with_cors(response, origin_ref));
				}
			};
			let metadata = match file.metadata().await {
				Ok(metadata) => metadata,
				Err(err) => {
					let response = json_response(
						StatusCode::INTERNAL_SERVER_ERROR,
						json!({ "error": err.to_string() }),
					);
					return Ok(with_cors(response, origin_ref));
				}
			};
			let total_len = metadata.len();
			let mime_type = entry.mime_type.clone().unwrap_or_else(|| {
				from_path(&path)
					.first_or_octet_stream()
					.essence_str()
					.to_string()
			});
			let range_header = req.headers().get(RANGE).cloned();
			if total_len == 0 {
				if range_header.is_some() {
					return Ok(with_cors(
						range_not_satisfiable_response(total_len),
						origin_ref,
					));
				}
				let resp = Response::builder()
					.status(StatusCode::OK)
					.header(CONTENT_TYPE, &mime_type)
					.header(CONTENT_LENGTH, "0")
					.header(ACCEPT_RANGES, HeaderValue::from_static("bytes"))
					.body(Body::empty())
					.unwrap();
				return Ok(with_cors(resp, origin_ref));
			}
			let (start, end, status) = if let Some(range_value) = range_header {
				let header_value = match range_value.to_str() {
					Ok(value) => value,
					Err(_) => {
						return Ok(with_cors(bad_request("invalid range header"), origin_ref));
					}
				};
				match parse_range_header(header_value, total_len) {
					Ok((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
					Err(RangeParseError::Invalid) => {
						return Ok(with_cors(bad_request("invalid range header"), origin_ref));
					}
					Err(RangeParseError::Unsatisfiable) => {
						return Ok(with_cors(
							range_not_satisfiable_response(total_len),
							origin_ref,
						));
					}
				}
			} else {
				(0, total_len.saturating_sub(1), StatusCode::OK)
			};
			if start > 0 {
				if let Err(err) = file.seek(SeekFrom::Start(start)).await {
					let response = json_response(
						StatusCode::INTERNAL_SERVER_ERROR,
						json!({ "error": err.to_string() }),
					);
					return Ok(with_cors(response, origin_ref));
				}
			}
			let chunk_len = end - start + 1;
			let stream = unfold((file, chunk_len), |(mut reader, remaining)| async move {
				if remaining == 0 {
					return None;
				}
				let buf_size = remaining.min(READ_CHUNK_SIZE as u64) as usize;
				let mut buf = vec![0u8; buf_size];
				match reader.read(&mut buf).await {
					Ok(0) => None,
					Ok(n) => {
						buf.truncate(n);
						let next_remaining = remaining.saturating_sub(n as u64);
						Some((Ok(Bytes::from(buf)), (reader, next_remaining)))
					}
					Err(err) => Some((Err(err), (reader, 0))),
				}
			});
			let mut builder = Response::builder()
				.status(status)
				.header(CONTENT_TYPE, &mime_type)
				.header(ACCEPT_RANGES, HeaderValue::from_static("bytes"))
				.header(CONTENT_LENGTH, chunk_len.to_string());
			if status == StatusCode::PARTIAL_CONTENT {
				builder = builder.header(
					CONTENT_RANGE,
					format!("bytes {}-{}/{}", start, end, total_len),
				);
			}
			let resp = builder.body(Body::wrap_stream(stream)).unwrap();
			with_cors(resp, origin_ref)
		}
		(&Method::GET, ["api", "scans", "results"]) => {
			let query = parse_query(&req);
			let page = query
				.get("page")
				.and_then(|v| v.parse::<usize>().ok())
				.unwrap_or(0);
			let page_size = query
				.get("page_size")
				.and_then(|v| v.parse::<usize>().ok())
				.unwrap_or(25);
			match state.puppy.fetch_scan_results_page(page, page_size) {
				Ok((rows, total)) => json_response(
					StatusCode::OK,
					json!({ "rows": rows, "total": total, "page": page, "page_size": page_size }),
				),
				Err(err) => bad_request(err),
			}
		}
		(&Method::POST, ["api", "scans"]) => {
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<ScanStartRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state.puppy.scan_folder(payload.path) {
					Ok(handle) => {
						let id = state.insert_scan(handle);
						json_response(StatusCode::CREATED, json!({ "scan_id": id }))
					}
					Err(err) => bad_request(err),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::GET, ["api", "scans", scan_id, "events"]) => {
			let Ok(id) = scan_id.parse::<u64>() else {
				return Ok(with_cors(bad_request("invalid scan id"), origin_ref));
			};
			match state.poll_scan(id) {
				Some(events) => json_response(StatusCode::OK, json!({ "events": events })),
				None => json_response(StatusCode::NOT_FOUND, json!({ "error": "scan not found" })),
			}
		}
		(&Method::POST, ["api", "scans", scan_id, "cancel"]) => {
			let Ok(id) = scan_id.parse::<u64>() else {
				return Ok(with_cors(bad_request("invalid scan id"), origin_ref));
			};
			if state.cancel_scan(id) {
				Response::builder()
					.status(StatusCode::NO_CONTENT)
					.body(Body::empty())
					.unwrap()
			} else {
				json_response(StatusCode::NOT_FOUND, json!({ "error": "scan not found" }))
			}
		}
		(&Method::GET, ["api", "search"]) => {
			let q = parse_query(&req);
			let mut mime_types: Vec<String> = q
				.get("mime_types")
				.map(|raw| {
					raw.split(',')
						.filter(|v| !v.trim().is_empty())
						.map(|v| v.trim().to_string())
						.collect()
				})
				.unwrap_or_default();
			if mime_types.is_empty() {
				if let Some(single) = q.get("mime_type") {
					if !single.trim().is_empty() {
						mime_types.push(single.clone());
					}
				}
			}
			let args = SearchFilesArgs {
				name_query: q.get("name_query").cloned(),
				content_query: q.get("content_query").cloned(),
				date_from: q.get("date_from").cloned(),
				date_to: q.get("date_to").cloned(),
				replicas_min: q.get("replicas_min").and_then(|v| v.parse::<u64>().ok()),
				replicas_max: q.get("replicas_max").and_then(|v| v.parse::<u64>().ok()),
				mime_types,
				sort_desc: q
					.get("sort_desc")
					.map(|v| v == "true" || v == "1")
					.unwrap_or(true),
				page: q
					.get("page")
					.and_then(|v| v.parse::<usize>().ok())
					.unwrap_or(0),
				page_size: q
					.get("page_size")
					.and_then(|v| v.parse::<usize>().ok())
					.unwrap_or(50),
			};
			let puppy = Arc::clone(&state.puppy);
			match task::spawn_blocking(move || puppy.search_files(args)).await {
				Ok(Ok((results, mimes, total))) => json_response(
					StatusCode::OK,
					json!({ "results": results, "mime_types": mimes, "total": total }),
				),
				Ok(Err(err)) => bad_request(err),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "mime-types"]) => {
			let puppy = Arc::clone(&state.puppy);
			match task::spawn_blocking(move || puppy.get_mime_types()).await {
				Ok(Ok(mimes)) => json_response(StatusCode::OK, json!({ "mime_types": mimes })),
				Ok(Err(err)) => bad_request(err),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::POST, ["api", "updates", peer_id]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(with_cors(bad_request(err), origin_ref)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(with_cors(bad_request("failed to read body"), origin_ref));
			};
			let parsed: Result<UpdateStartRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state.puppy.update_remote_peer(peer, payload.version) {
					Ok(rx) => {
						let id = state.insert_update(rx);
						json_response(StatusCode::CREATED, json!({ "update_id": id }))
					}
					Err(err) => bad_request(err),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::GET, ["api", "updates", update_id, "events"]) => {
			let Ok(id) = update_id.parse::<u64>() else {
				return Ok(with_cors(bad_request("invalid update id"), origin_ref));
			};
			match state.poll_update(id) {
				Some(events) => json_response(StatusCode::OK, json!({ "events": events })),
				None => json_response(
					StatusCode::NOT_FOUND,
					json!({ "error": "update not found" }),
				),
			}
		}
		(&Method::GET, segments) => {
			let path = segments.join("/");
			match serve_static_path(&path) {
				Some(resp) => resp,
				None => json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" })),
			}
		}
		_ => json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" })),
	};

	Ok(with_cors(response, origin_ref))
}

/// Start a simple HTTP server exposing a small API surface on top of PuppyNet.
pub async fn serve(puppy: Arc<PuppyNet>, addr: SocketAddr) -> Result<()> {
	let jwt_secret = load_jwt_secret();
	let state = Arc::new(ApiState::new(puppy, jwt_secret));
	let make_svc = make_service_fn(move |_| {
		let state = Arc::clone(&state);
		async move {
			Ok::<_, Infallible>(service_fn(move |req| {
				let state = Arc::clone(&state);
				handle_request(req, state)
			}))
		}
	});

	let listener = TcpListener::bind(addr).await?;
	let std_listener = listener.into_std()?;
	let server = Server::from_tcp(std_listener)?
		.serve(make_svc)
		.with_graceful_shutdown(async {
			let _ = signal::ctrl_c().await;
		});
	log::info!("HTTP API listening on {}", addr);
	server.await?;
	Ok(())
}
