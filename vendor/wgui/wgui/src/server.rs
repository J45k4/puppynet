#![cfg(feature = "hyper")]

use futures_util::{Stream, StreamExt};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::ssr;
use crate::types::{ClientMessage, Clients};
use crate::ws::TungsteniteWs;
use crate::wui::runtime::RouteContext;
use crate::{Sessions, SsrResponse, WguiHandle};

const INDEX_HTML_BYTES: &[u8] = include_bytes!("../../dist/index.html");
const INDEX_JS_BYTES: &[u8] = include_bytes!("../../dist/index.js");
const CSS_JS_BYTES: &[u8] = include_bytes!("../../dist/index.css");

pub type HttpHandler = Arc<
	dyn Fn(HttpRequest) -> Pin<Box<dyn Future<Output = Option<HttpResponse>> + Send>> + Send + Sync,
>;
pub(crate) type SharedAppCss = Arc<RwLock<Option<String>>>;
pub(crate) type SharedHttpHandler = Arc<RwLock<Option<HttpHandler>>>;
pub(crate) type SharedStaticMounts = Arc<RwLock<Vec<StaticMount>>>;

#[derive(Clone)]
pub(crate) enum StaticMount {
	File { route: String, file: PathBuf },
	Dir { prefix: String, dir: PathBuf },
}

impl StaticMount {
	pub(crate) fn file(route: String, file: PathBuf) -> Self {
		Self::File {
			route: normalize_mount_route(route),
			file,
		}
	}

	pub(crate) fn dir(prefix: String, dir: PathBuf) -> Self {
		Self::Dir {
			prefix: normalize_mount_route(prefix),
			dir,
		}
	}
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
	pub method: String,
	pub path: String,
	pub query: HashMap<String, String>,
	pub headers: HashMap<String, String>,
	pub body: Vec<u8>,
}

pub struct HttpResponse {
	pub status: u16,
	pub headers: Vec<(String, String)>,
	pub body: Vec<u8>,
	stream: Option<HttpResponseStream>,
}

type HttpBody = UnsyncBoxBody<Bytes, Infallible>;
type HttpResponseStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, Infallible>> + Send>>;

impl HttpResponse {
	pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
		Self {
			status,
			headers: Vec::new(),
			body: body.into(),
			stream: None,
		}
	}

	pub fn stream<S>(status: u16, stream: S) -> Self
	where
		S: Stream<Item = Result<Vec<u8>, Infallible>> + Send + 'static,
	{
		Self {
			status,
			headers: Vec::new(),
			body: Vec::new(),
			stream: Some(Box::pin(stream)),
		}
	}

	pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
		self.headers.push((name.into(), value.into()));
		self
	}
}

fn content_type_for(path: &Path) -> &'static str {
	match path
		.extension()
		.and_then(|ext| ext.to_str())
		.unwrap_or_default()
	{
		"css" => "text/css",
		"js" => "text/javascript",
		"html" => "text/html",
		"ico" => "image/x-icon",
		"stl" => "model/stl",
		"jpg" | "jpeg" => "image/jpeg",
		"png" => "image/png",
		"svg" => "image/svg+xml",
		_ => "application/octet-stream",
	}
}

fn normalize_mount_route(route: String) -> String {
	let trimmed = route.trim();
	let with_slash = if trimmed.starts_with('/') {
		trimmed.to_string()
	} else {
		format!("/{trimmed}")
	};
	if with_slash.len() > 1 {
		with_slash.trim_end_matches('/').to_string()
	} else {
		with_slash
	}
}

fn relative_static_path(base: &Path, relative: &str) -> Option<PathBuf> {
	let mut out = base.to_path_buf();
	if relative.is_empty() {
		return Some(out);
	}
	for part in relative.split('/') {
		if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
			return None;
		}
		out.push(part);
	}
	Some(out)
}

fn sanitize_asset_path(uri_path: &str) -> Option<PathBuf> {
	if !uri_path.starts_with("/assets/") {
		return None;
	}
	let relative = uri_path.trim_start_matches("/assets/");
	if relative.is_empty() {
		return None;
	}
	let mut out = PathBuf::from("assets");
	for part in relative.split('/') {
		if part.is_empty() || part == "." || part == ".." {
			return None;
		}
		out.push(part);
	}
	Some(out)
}

fn sanitize_fs_path(uri_path: &str) -> Option<PathBuf> {
	if !uri_path.starts_with("/fs/") {
		return None;
	}
	let relative = uri_path.trim_start_matches("/fs/");
	if relative.is_empty() {
		return None;
	}
	let mut out = std::env::current_dir().ok()?;
	for part in relative.split('/') {
		if part.is_empty() || part == "." || part == ".." {
			return None;
		}
		out.push(part);
	}
	Some(out)
}

async fn read_static_file(path: &Path) -> Response<HttpBody> {
	match tokio::fs::read(path).await {
		Ok(bytes) => Response::builder()
			.header("content-type", content_type_for(path))
			.header("cache-control", "public, max-age=86400")
			.body(full_body(bytes))
			.unwrap(),
		Err(_) => Response::builder()
			.status(404)
			.body(full_body("file not found"))
			.unwrap(),
	}
}

async fn static_mount_response(
	uri_path: &str,
	mounts: &SharedStaticMounts,
) -> Option<Response<HttpBody>> {
	let mounts = mounts.read().unwrap().clone();
	for mount in mounts {
		match mount {
			StaticMount::File { route, file } => {
				if uri_path == route {
					return Some(read_static_file(&file).await);
				}
			}
			StaticMount::Dir { prefix, dir } => {
				let relative = if prefix == "/" {
					uri_path.trim_start_matches('/')
				} else if uri_path == prefix {
					""
				} else if let Some(relative) = uri_path.strip_prefix(&format!("{prefix}/")) {
					relative
				} else {
					continue;
				};
				let Some(file) = relative_static_path(&dir, relative) else {
					return Some(
						Response::builder()
							.status(400)
							.body(full_body("bad static path"))
							.unwrap(),
					);
				};
				return Some(read_static_file(&file).await);
			}
		}
	}
	None
}

fn index_html_response(app_css: &SharedAppCss) -> Vec<u8> {
	if app_css.read().unwrap().is_none() {
		return INDEX_HTML_BYTES.to_vec();
	}

	let html = String::from_utf8_lossy(INDEX_HTML_BYTES);
	html.replace(
		"<link rel=\"stylesheet\" href=\"/index.css\"></link>",
		"<link rel=\"stylesheet\" href=\"/index.css\"></link><link rel=\"stylesheet\" href=\"/app.css\"></link>",
	)
	.into_bytes()
}

struct Ctx {
	event_tx: mpsc::UnboundedSender<ClientMessage>,
	clients: Clients,
	sessions: Sessions,
	ssr: Option<Arc<dyn Fn(RouteContext, Option<String>) -> Option<SsrResponse> + Send + Sync>>,
	http_handler: SharedHttpHandler,
	app_css: SharedAppCss,
	static_mounts: SharedStaticMounts,
}

fn query_map(req: &Request<hyper::body::Incoming>) -> HashMap<String, String> {
	let mut out = HashMap::new();
	let Some(query) = req.uri().query() else {
		return out;
	};
	for pair in query.split('&') {
		let mut parts = pair.splitn(2, '=');
		let key = parts.next().unwrap_or("");
		if key.is_empty() {
			continue;
		}
		let value = parts.next().unwrap_or("");
		out.insert(key.to_string(), value.to_string());
	}
	out
}

fn header_map(req: &Request<hyper::body::Incoming>) -> HashMap<String, String> {
	let mut out = HashMap::new();
	for (name, value) in req.headers() {
		if let Ok(value) = value.to_str() {
			out.insert(name.as_str().to_ascii_lowercase(), value.to_string());
		}
	}
	out
}

fn cookie_value(req: &Request<hyper::body::Incoming>, name: &str) -> Option<String> {
	let raw = req.headers().get(hyper::header::COOKIE)?;
	let header = raw.to_str().ok()?;
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

fn session_from_query(req: &Request<hyper::body::Incoming>) -> Option<String> {
	let query = req.uri().query()?;
	for pair in query.split('&') {
		let mut parts = pair.splitn(2, '=');
		let key = parts.next().unwrap_or("");
		let value = parts.next().unwrap_or("");
		if key == "sid" && !value.is_empty() {
			return Some(value.to_string());
		}
	}
	None
}

fn session_from_request(req: &Request<hyper::body::Incoming>) -> Option<String> {
	cookie_value(req, "sid").or_else(|| session_from_query(req))
}

fn full_body(body: impl Into<Bytes>) -> HttpBody {
	Full::new(body.into())
		.map_err(|never| match never {})
		.boxed_unsync()
}

fn response_body(response: HttpResponse) -> HttpBody {
	if let Some(stream) = response.stream {
		return BodyExt::boxed_unsync(StreamBody::new(
			stream.map(|chunk| chunk.map(|bytes| Frame::data(Bytes::from(bytes)))),
		));
	}
	full_body(response.body)
}

fn http_response(response: HttpResponse) -> Response<HttpBody> {
	let mut builder = Response::builder().status(response.status);
	for (name, value) in &response.headers {
		builder = builder.header(name.as_str(), value.as_str());
	}
	builder.body(response_body(response)).unwrap()
}

async fn custom_http_response(
	req: &mut Request<hyper::body::Incoming>,
	handler: &SharedHttpHandler,
) -> Result<Option<Response<HttpBody>>, hyper::Error> {
	let Some(handler) = handler.read().unwrap().clone() else {
		return Ok(None);
	};
	let method = req.method().as_str().to_string();
	let path = req.uri().path().to_string();
	let query = query_map(req);
	let headers = header_map(req);
	let body = req.body_mut().collect().await?.to_bytes().to_vec();
	let request = HttpRequest {
		method,
		path,
		query,
		headers,
		body,
	};
	Ok((handler)(request).await.map(http_response))
}

async fn handle_req(
	mut req: Request<hyper::body::Incoming>,
	ctx: Ctx,
) -> Result<Response<HttpBody>, hyper::Error> {
	log::info!("{} {}", req.method(), req.uri().path());

	if req.uri().path() == "/ws" && hyper_tungstenite::is_upgrade_request(&req) {
		log::info!("upgrade request");
		let (response, websocket) = hyper_tungstenite::upgrade(&mut req, None).unwrap();
		let event_tx = ctx.event_tx.clone();
		let clients = ctx.clients.clone();
		let sessions = ctx.sessions.clone();
		let session = session_from_request(&req);
		tokio::spawn(async move {
			match websocket.await {
				Ok(ws) => {
					log::info!("websocket connected");
					let ws = TungsteniteWs::new(ws);
					let handle = WguiHandle::new(event_tx, clients, sessions);
					handle.handle_ws_with_session(ws, session).await;
				}
				Err(err) => {
					log::error!("websocket error: {:?}", err);
				}
			}
		});
		return Ok(response.map(|body| body.map_err(|never| match never {}).boxed_unsync()));
	}

	if let Some(response) = custom_http_response(&mut req, &ctx.http_handler).await? {
		return Ok(response);
	}

	if let Some(response) = static_mount_response(req.uri().path(), &ctx.static_mounts).await {
		return Ok(response);
	}

	match req.uri().path() {
		"/favicon.ico" => Ok(Response::builder()
			.status(204)
			.header("cache-control", "public, max-age=86400")
			.body(full_body(Bytes::new()))
			.unwrap()),
		"/index.js" => Ok(Response::builder()
			.header("content-type", "text/javascript")
			.header("cache-control", "no-store")
			.body(full_body(INDEX_JS_BYTES))
			.unwrap()),
		"/index.css" => Ok(Response::builder()
			.header("content-type", "text/css")
			.header("cache-control", "no-store")
			.body(full_body(CSS_JS_BYTES))
			.unwrap()),
		"/app.css" => {
			let css = ctx.app_css.read().unwrap().clone();
			match css {
				Some(css) => Ok(Response::builder()
					.header("content-type", "text/css")
					.header("cache-control", "no-store")
					.body(full_body(css))
					.unwrap()),
				None => Ok(Response::builder()
					.status(404)
					.header("cache-control", "no-store")
					.body(full_body("app css not set"))
					.unwrap()),
			}
		}
		path if path.starts_with("/assets/") => {
			let Some(asset_path) = sanitize_asset_path(path) else {
				return Ok(Response::builder()
					.status(400)
					.body(full_body("bad asset path"))
					.unwrap());
			};
			match tokio::fs::read(&asset_path).await {
				Ok(bytes) => Ok(Response::builder()
					.header("content-type", content_type_for(&asset_path))
					.body(full_body(bytes))
					.unwrap()),
				Err(_) => Ok(Response::builder()
					.status(404)
					.body(full_body("asset not found"))
					.unwrap()),
			}
		}
		path if path.starts_with("/fs/") => {
			let Some(file_path) = sanitize_fs_path(path) else {
				return Ok(Response::builder()
					.status(400)
					.body(full_body("bad file path"))
					.unwrap());
			};
			match tokio::fs::read(&file_path).await {
				Ok(bytes) => Ok(Response::builder()
					.header("content-type", content_type_for(&file_path))
					.body(full_body(bytes))
					.unwrap()),
				Err(_) => Ok(Response::builder()
					.status(404)
					.body(full_body("file not found"))
					.unwrap()),
			}
		}
		_ => {
			if let Some(renderer) = ctx.ssr {
				let session = session_from_request(&req);
				let route = RouteContext {
					path: req.uri().path().to_string(),
					params: std::collections::HashMap::new(),
					query: query_map(&req),
				};
				match (renderer)(route, session) {
					Some(SsrResponse::Render(item)) => {
						let html = ssr::render_document_with_app_css(
							&item,
							ctx.app_css.read().unwrap().is_some(),
						);
						Ok(Response::builder()
							.header("content-type", "text/html")
							.header("cache-control", "no-store")
							.body(full_body(html))
							.unwrap())
					}
					Some(SsrResponse::Redirect(url)) => Ok(Response::builder()
						.status(303)
						.header("location", url)
						.header("cache-control", "no-store")
						.body(full_body(Bytes::new()))
						.unwrap()),
					None => Ok(Response::builder()
						.header("content-type", "text/html")
						.header("cache-control", "no-store")
						.body(full_body(index_html_response(&ctx.app_css)))
						.unwrap()),
				}
			} else {
				Ok(Response::builder()
					.header("content-type", "text/html")
					.header("cache-control", "no-store")
					.body(full_body(index_html_response(&ctx.app_css)))
					.unwrap())
			}
		}
	}
}

pub struct Server {
	listener: TcpListener,
	event_tx: mpsc::UnboundedSender<ClientMessage>,
	clients: Clients,
	sessions: Sessions,
	ssr: Option<Arc<dyn Fn(RouteContext, Option<String>) -> Option<SsrResponse> + Send + Sync>>,
	http_handler: SharedHttpHandler,
	app_css: SharedAppCss,
	static_mounts: SharedStaticMounts,
}

impl Server {
	pub async fn new(
		addr: SocketAddr,
		event_tx: mpsc::UnboundedSender<ClientMessage>,
		clients: Clients,
		sessions: Sessions,
		ssr: Option<Arc<dyn Fn(RouteContext, Option<String>) -> Option<SsrResponse> + Send + Sync>>,
		http_handler: SharedHttpHandler,
		app_css: SharedAppCss,
		static_mounts: SharedStaticMounts,
	) -> Self {
		let listener = TcpListener::bind(addr).await.unwrap();
		log::info!("listening on http://localhost:{}", addr.port());

		Self {
			listener,
			event_tx,
			clients,
			sessions,
			ssr,
			http_handler,
			app_css,
			static_mounts,
		}
	}

	pub async fn run(self) {
		loop {
			tokio::select! {
				res = self.listener.accept() => {
					match res {
						Ok((socket, addr)) => {
							log::info!("accepted connection from {}", addr);
							let io = TokioIo::new(socket);
							let event_tx = self.event_tx.clone();
							let clients = self.clients.clone();
								let sessions = self.sessions.clone();
								let ssr = self.ssr.clone();
								let http_handler = self.http_handler.clone();
								let app_css = self.app_css.clone();
								let static_mounts = self.static_mounts.clone();
								tokio::spawn(async move {
									let service = service_fn(move |req| {
										handle_req(req, Ctx {
											event_tx: event_tx.clone(),
											clients: clients.clone(),
											sessions: sessions.clone(),
											ssr: ssr.clone(),
											http_handler: http_handler.clone(),
											app_css: app_css.clone(),
											static_mounts: static_mounts.clone(),
										})
									});

								if let Err(err) = http1::Builder::new()
									.serve_connection(io, service)
									.with_upgrades()
									.await {

									log::error!("server error: {:?}", err);
								}
							});
						},
						Err(err) => {
							log::error!("accept error: {:?}", err);
						}
					}
				}
			}
		}
	}
}
