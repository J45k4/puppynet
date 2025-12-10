use crate::puppynet::PuppyNet;
use crate::scan::ScanEvent;
use crate::updater::UpdateProgress;
use crate::{Permission, SearchFilesArgs};
use anyhow::Result;
use hyper::body::Buf;
use hyper::header::{
	ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use libp2p::PeerId;
use mime_guess::from_path;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::{signal, task};
use tokio::net::TcpListener;
use url::form_urlencoded;

const CT_JSON: &str = "application/json";

#[derive(Deserialize)]
struct CreateUserRequest {
	username: String,
	password: String,
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
}

impl ApiState {
	fn new(puppy: Arc<PuppyNet>) -> Self {
		Self {
			puppy,
			scans: Mutex::new(HashMap::new()),
			next_scan_id: AtomicU64::new(1),
			updates: Mutex::new(HashMap::new()),
			next_update_id: AtomicU64::new(1),
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

	fn insert_update(
		&self,
		rx: Arc<Mutex<std::sync::mpsc::Receiver<UpdateProgress>>>,
	) -> u64 {
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
	let path = if path.is_empty() {
		"index.html"
	} else {
		path
	};
	if let Some(data) = load_dist_asset(path) {
		return Some(asset_response(path, data));
	}
	load_asset(path).map(|data| asset_response(path, data))
}

fn with_cors(mut resp: Response<Body>) -> Response<Body> {
	resp.headers_mut()
		.insert(ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
	resp.headers_mut().insert(
		ACCESS_CONTROL_ALLOW_HEADERS,
		"content-type,authorization".parse().unwrap(),
	);
	resp.headers_mut().insert(
		ACCESS_CONTROL_ALLOW_METHODS,
		"GET,POST,PUT,DELETE,OPTIONS".parse().unwrap(),
	);
	resp
}

async fn handle_request(
	req: Request<Body>,
	state: Arc<ApiState>,
) -> Result<Response<Body>, Infallible> {
	let segments: Vec<&str> = req
		.uri()
		.path()
		.split('/')
		.filter(|s| !s.is_empty())
		.collect();

	let response = match (req.method(), segments.as_slice()) {
		(&Method::OPTIONS, _) => Response::builder()
			.status(StatusCode::NO_CONTENT)
			.body(Body::empty())
			.unwrap(),
		(&Method::GET, ["health"]) => Response::new(Body::from("ok")),
		(&Method::GET, ["users"]) => {
			match state.puppy.list_users_db() {
				Ok(list) => json_response(StatusCode::OK, json!({ "users": list })),
				Err(err) => json_response(
					StatusCode::INTERNAL_SERVER_ERROR,
					json!({ "error": err }),
				),
			}
		}
		(&Method::POST, ["users"]) => {
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(bad_request("failed to read body"));
			};
			let parsed: Result<CreateUserRequest, _> = serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state
					.puppy
					.create_user(payload.username.clone(), payload.password)
				{
					Ok(()) => json_response(
						StatusCode::CREATED,
						json!({ "username": payload.username }),
					),
					Err(err) => bad_request(err.to_string()),
				},
				Err(err) => bad_request(format!("invalid json: {err}")),
			}
		}
		(&Method::GET, ["api", "state"]) => {
			let me = state
				.puppy
				.state_snapshot()
				.map(|s| s.me.to_string())
				.unwrap_or_else(|| String::from("unknown"));
			let shared_folders = state
				.puppy
				.state_snapshot()
				.map(|s| {
					s.shared_folders
						.into_iter()
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
				.map(|p| PeerSummary {
					id: p.id.to_string(),
					name: p.name,
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
				Err(err) => return Ok(bad_request(err)),
			};
			match state.puppy.list_permissions(peer).await {
				Ok(perms) => json_response(StatusCode::OK, json!({ "permissions": perms })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "permissions", "granted"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(bad_request(err)),
			};
			match state.puppy.list_granted_permissions(peer) {
				Ok(perms) => json_response(StatusCode::OK, json!({ "permissions": perms })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::PUT, ["api", "peers", peer_id, "permissions"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(bad_request(err)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(bad_request("failed to read body"));
			};
			let parsed: Result<SetPermissionsRequest, _> =
				serde_json::from_reader(buf.reader());
			match parsed {
				Ok(payload) => match state
					.puppy
					.set_peer_permissions(peer, payload.permissions)
				{
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
				Err(err) => return Ok(bad_request(err)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(bad_request("failed to read body"));
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
				Err(err) => return Ok(bad_request(err)),
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
				Err(err) => return Ok(bad_request(err)),
			};
			match state.puppy.list_disks(peer).await {
				Ok(disks) => json_response(StatusCode::OK, json!({ "disks": disks })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "interfaces"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(bad_request(err)),
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
				Err(err) => return Ok(bad_request(err)),
			};
			match state.puppy.list_cpus(peer).await {
				Ok(cpus) => json_response(StatusCode::OK, json!({ "cpus": cpus })),
				Err(err) => bad_request(err.to_string()),
			}
		}
		(&Method::GET, ["api", "peers", peer_id, "file"]) => {
			let peer = match parse_peer_id(peer_id) {
				Ok(p) => p,
				Err(err) => return Ok(bad_request(err)),
			};
			let query = parse_query(&req);
			let Some(path) = query.get("path") else {
				return Ok(bad_request("missing path"));
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
				Err(err) => return Ok(bad_request(err)),
			};
			let query = parse_query(&req);
			let Some(path) = query.get("path") else {
				return Ok(bad_request("missing path"));
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
				return Ok(bad_request("failed to read body"));
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
				return Ok(bad_request("invalid scan id"));
			};
			match state.poll_scan(id) {
				Some(events) => json_response(StatusCode::OK, json!({ "events": events })),
				None => json_response(StatusCode::NOT_FOUND, json!({ "error": "scan not found" })),
			}
		}
		(&Method::POST, ["api", "scans", scan_id, "cancel"]) => {
			let Ok(id) = scan_id.parse::<u64>() else {
				return Ok(bad_request("invalid scan id"));
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
				replicas_min: q
					.get("replicas_min")
					.and_then(|v| v.parse::<u64>().ok()),
				replicas_max: q
					.get("replicas_max")
					.and_then(|v| v.parse::<u64>().ok()),
				mime_types,
				sort_desc: q
					.get("sort_desc")
					.map(|v| v == "true" || v == "1")
					.unwrap_or(true),
				page: q.get("page").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0),
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
				Err(err) => return Ok(bad_request(err)),
			};
			let body = hyper::body::aggregate(req.into_body()).await;
			let Ok(buf) = body else {
				return Ok(bad_request("failed to read body"));
			};
			let parsed: Result<UpdateStartRequest, _> =
				serde_json::from_reader(buf.reader());
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
				return Ok(bad_request("invalid update id"));
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

	Ok(with_cors(response))
}

/// Start a simple HTTP server exposing a small API surface on top of PuppyNet.
pub async fn serve(puppy: Arc<PuppyNet>, addr: SocketAddr) -> Result<()> {
	let state = Arc::new(ApiState::new(puppy));
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
