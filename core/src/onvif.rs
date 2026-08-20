use crate::db::{load_onvif_camera, load_onvif_cameras, open_db};
use crate::p2p::{
	MediaFrame, MediaOutput, MediaSource, MediaSourceKind, MediaTransport, OnvifCamera,
	OnvifCameraConfig,
};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use md5::Md5;
use quick_xml::Reader;
use quick_xml::events::Event;
use rand::RngCore;
use rand::rngs::OsRng;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};
use url::Url;

const SOURCE_PREFIX: &str = "onvif:";
const WORKER_IDLE_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const FRAME_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JPEG_SIZE: usize = 8 * 1024 * 1024;

struct WorkerState {
	frame: Mutex<Option<MediaFrame>>,
	error: Mutex<Option<String>>,
	last_access: Mutex<Instant>,
}

struct CameraWorker {
	state: Arc<WorkerState>,
	task: JoinHandle<()>,
}

static CAMERA_WORKERS: OnceLock<Mutex<HashMap<String, CameraWorker>>> = OnceLock::new();

fn camera_workers() -> &'static Mutex<HashMap<String, CameraWorker>> {
	CAMERA_WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn xml_escape(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&apos;")
}

fn ws_security_header(username: &str, password: &str) -> String {
	if username.is_empty() {
		return String::new();
	}
	let mut nonce = [0u8; 16];
	OsRng.fill_bytes(&mut nonce);
	let created = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
	let mut digest = Sha1::new();
	digest.update(nonce);
	digest.update(created.as_bytes());
	digest.update(password.as_bytes());
	let password_digest = base64::engine::general_purpose::STANDARD.encode(digest.finalize());
	let nonce = base64::engine::general_purpose::STANDARD.encode(nonce);
	format!(
		r#"<s:Header><wsse:Security s:mustUnderstand="1" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"><wsse:UsernameToken><wsse:Username>{}</wsse:Username><wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{password_digest}</wsse:Password><wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{nonce}</wsse:Nonce><wsu:Created>{created}</wsu:Created></wsse:UsernameToken></wsse:Security></s:Header>"#,
		xml_escape(username),
	)
}

fn soap_envelope(username: &str, password: &str, namespaces: &str, body: &str) -> String {
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" {namespaces}>{}<s:Body>{body}</s:Body></s:Envelope>"#,
		ws_security_header(username, password),
	)
}

fn normalize_device_service_url(value: &str) -> Result<String> {
	let value = value.trim();
	if value.is_empty() {
		bail!("ONVIF device URL is required");
	}
	let candidate = if value.contains("://") {
		value.to_string()
	} else {
		format!("http://{value}")
	};
	let mut url = Url::parse(&candidate).context("invalid ONVIF device URL")?;
	if !matches!(url.scheme(), "http" | "https") {
		bail!("ONVIF device URL must use HTTP or HTTPS");
	}
	if !url.username().is_empty() || url.password().is_some() {
		bail!("put ONVIF credentials in the username and password fields");
	}
	if url.host_str().is_none() {
		bail!("ONVIF device URL must include a host");
	}
	if url.path().is_empty() || url.path() == "/" {
		url.set_path("/onvif/device_service");
	}
	Ok(url.to_string())
}

fn xml_text(xml: &str, element_name: &[u8]) -> Result<Option<String>> {
	let mut reader = Reader::from_str(xml);
	reader.config_mut().trim_text(true);
	let mut inside = false;
	loop {
		match reader.read_event()? {
			Event::Start(element) if element.local_name().as_ref() == element_name => {
				inside = true;
			}
			Event::Text(text) if inside => return Ok(Some(text.decode()?.into_owned())),
			Event::CData(text) if inside => {
				return Ok(Some(String::from_utf8_lossy(text.as_ref()).into_owned()));
			}
			Event::End(element) if element.local_name().as_ref() == element_name => {
				return Ok(Some(String::new()));
			}
			Event::Eof => return Ok(None),
			_ => {}
		}
	}
}

fn profile_tokens(xml: &str) -> Result<Vec<String>> {
	let mut reader = Reader::from_str(xml);
	reader.config_mut().trim_text(true);
	let mut tokens = Vec::new();
	loop {
		match reader.read_event()? {
			Event::Start(element) | Event::Empty(element)
				if element.local_name().as_ref() == b"Profiles" =>
			{
				for attribute in element.attributes() {
					let attribute = attribute?;
					if attribute.key.local_name().as_ref() == b"token" {
						tokens.push(
							attribute
								.decode_and_unescape_value(reader.decoder())?
								.into_owned(),
						);
					}
				}
			}
			Event::Eof => return Ok(tokens),
			_ => {}
		}
	}
}

fn digest_challenge_fields(header: &str) -> HashMap<String, String> {
	let value = header
		.trim()
		.strip_prefix("Digest ")
		.or_else(|| header.trim().strip_prefix("digest "))
		.unwrap_or(header);
	let mut parts = Vec::new();
	let mut current = String::new();
	let mut quoted = false;
	for character in value.chars() {
		match character {
			'"' => {
				quoted = !quoted;
				current.push(character);
			}
			',' if !quoted => parts.push(std::mem::take(&mut current)),
			_ => current.push(character),
		}
	}
	if !current.is_empty() {
		parts.push(current);
	}
	parts
		.into_iter()
		.filter_map(|part| {
			let (key, value) = part.trim().split_once('=')?;
			Some((
				key.trim().to_ascii_lowercase(),
				value.trim().trim_matches('"').to_string(),
			))
		})
		.collect()
}

fn md5_hex(value: &str) -> String {
	format!("{:x}", Md5::digest(value.as_bytes()))
}

fn digest_header(
	challenge: &str,
	method: &str,
	url: &str,
	username: &str,
	password: &str,
) -> Result<String> {
	let fields = digest_challenge_fields(challenge);
	let realm = fields
		.get("realm")
		.ok_or_else(|| anyhow!("HTTP Digest challenge has no realm"))?;
	let nonce = fields
		.get("nonce")
		.ok_or_else(|| anyhow!("HTTP Digest challenge has no nonce"))?;
	let algorithm = fields.get("algorithm").map(String::as_str).unwrap_or("MD5");
	if !algorithm.eq_ignore_ascii_case("MD5") {
		bail!("unsupported HTTP Digest algorithm {algorithm}");
	}
	let url = Url::parse(url)?;
	let mut request_uri = url.path().to_string();
	if let Some(query) = url.query() {
		request_uri.push('?');
		request_uri.push_str(query);
	}
	let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
	let ha2 = md5_hex(&format!("{method}:{request_uri}"));
	let mut cnonce_bytes = [0u8; 12];
	OsRng.fill_bytes(&mut cnonce_bytes);
	let cnonce = cnonce_bytes
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect::<String>();
	let qop_auth = fields.get("qop").is_some_and(|value| {
		value
			.split(',')
			.map(str::trim)
			.any(|value| value.eq_ignore_ascii_case("auth"))
	});
	let response = if qop_auth {
		md5_hex(&format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}"))
	} else {
		md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
	};
	let mut header = format!(
		"Digest username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{request_uri}\", response=\"{response}\", algorithm=MD5"
	);
	if qop_auth {
		header.push_str(&format!(", qop=auth, nc=00000001, cnonce=\"{cnonce}\""));
	}
	if let Some(opaque) = fields.get("opaque") {
		header.push_str(&format!(", opaque=\"{opaque}\""));
	}
	Ok(header)
}

async fn send_soap_request(
	client: &reqwest::Client,
	url: &str,
	action: &str,
	body: &str,
	authorization: Option<&str>,
) -> Result<reqwest::Response> {
	let mut request = client
		.post(url)
		.header(
			reqwest::header::CONTENT_TYPE,
			format!("application/soap+xml; charset=utf-8; action=\"{action}\""),
		)
		.header("SOAPAction", format!("\"{action}\""))
		.body(body.to_string());
	if let Some(authorization) = authorization {
		request = request.header(reqwest::header::AUTHORIZATION, authorization);
	}
	request
		.send()
		.await
		.with_context(|| format!("failed to contact ONVIF service at {url}"))
}

async fn soap_request(
	url: &str,
	action: &str,
	username: &str,
	password: &str,
	body: String,
) -> Result<String> {
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()?;
	let mut response = send_soap_request(&client, url, action, &body, None).await?;
	if response.status() == reqwest::StatusCode::UNAUTHORIZED && !username.is_empty() {
		let challenge = response
			.headers()
			.get(reqwest::header::WWW_AUTHENTICATE)
			.and_then(|value| value.to_str().ok())
			.ok_or_else(|| anyhow!("camera requested authentication without a challenge"))?;
		let authorization = digest_header(challenge, "POST", url, username, password)?;
		response = send_soap_request(&client, url, action, &body, Some(&authorization)).await?;
	}
	let status = response.status();
	let body = response.text().await?;
	if !status.is_success() {
		let detail = xml_text(&body, b"Text")?
			.filter(|value| !value.is_empty())
			.unwrap_or_else(|| status.to_string());
		bail!("ONVIF request failed: {detail}");
	}
	Ok(body)
}

async fn media_service_url(config: &OnvifCameraConfig) -> Result<String> {
	let action = "http://www.onvif.org/ver10/device/wsdl/GetCapabilities";
	let body = soap_envelope(
		&config.username,
		&config.password,
		r#"xmlns:td="http://www.onvif.org/ver10/device/wsdl""#,
		"<td:GetCapabilities><td:Category>Media</td:Category></td:GetCapabilities>",
	);
	let response = soap_request(
		&config.device_service_url,
		action,
		&config.username,
		&config.password,
		body,
	)
	.await?;
	xml_text(&response, b"XAddr")?
		.filter(|value| !value.is_empty())
		.ok_or_else(|| anyhow!("camera did not return an ONVIF media service URL"))
}

async fn select_profile_token(config: &OnvifCameraConfig, media_url: &str) -> Result<String> {
	if !config.profile_token.trim().is_empty() {
		return Ok(config.profile_token.trim().to_string());
	}
	let action = "http://www.onvif.org/ver10/media/wsdl/GetProfiles";
	let body = soap_envelope(
		&config.username,
		&config.password,
		r#"xmlns:trt="http://www.onvif.org/ver10/media/wsdl""#,
		"<trt:GetProfiles/>",
	);
	let response =
		soap_request(media_url, action, &config.username, &config.password, body).await?;
	profile_tokens(&response)?
		.into_iter()
		.next()
		.ok_or_else(|| anyhow!("camera did not return an ONVIF media profile"))
}

async fn stream_uri(config: &OnvifCameraConfig) -> Result<(String, String)> {
	let media_url = media_service_url(config).await?;
	let profile_token = select_profile_token(config, &media_url).await?;
	let action = "http://www.onvif.org/ver10/media/wsdl/GetStreamUri";
	let body = soap_envelope(
		&config.username,
		&config.password,
		r#"xmlns:trt="http://www.onvif.org/ver10/media/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema""#,
		&format!(
			"<trt:GetStreamUri><trt:StreamSetup><tt:Stream>RTP-Unicast</tt:Stream><tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></trt:StreamSetup><trt:ProfileToken>{}</trt:ProfileToken></trt:GetStreamUri>",
			xml_escape(&profile_token),
		),
	);
	let response =
		soap_request(&media_url, action, &config.username, &config.password, body).await?;
	let uri = xml_text(&response, b"Uri")?
		.filter(|value| !value.is_empty())
		.ok_or_else(|| anyhow!("camera did not return an RTSP stream URI"))?;
	let mut uri = Url::parse(&uri).context("camera returned an invalid stream URI")?;
	if uri.scheme() != "rtsp" {
		bail!("camera returned a non-RTSP stream URI");
	}
	if !config.username.is_empty() && uri.username().is_empty() {
		uri.set_username(&config.username)
			.map_err(|_| anyhow!("camera username cannot be added to the stream URI"))?;
		uri.set_password(Some(&config.password))
			.map_err(|_| anyhow!("camera password cannot be added to the stream URI"))?;
	}
	Ok((uri.to_string(), profile_token))
}

fn jpeg_end(buffer: &[u8]) -> Option<usize> {
	buffer
		.windows(2)
		.position(|bytes| bytes == [0xff, 0xd9])
		.map(|index| index + 2)
}

fn trim_to_jpeg_start(buffer: &mut Vec<u8>) {
	if let Some(start) = buffer.windows(2).position(|bytes| bytes == [0xff, 0xd8]) {
		if start > 0 {
			buffer.drain(..start);
		}
	} else if buffer.len() > 1 {
		let last = *buffer.last().unwrap();
		buffer.clear();
		if last == 0xff {
			buffer.push(last);
		}
	}
}

fn worker_is_idle(state: &WorkerState) -> bool {
	state
		.last_access
		.lock()
		.map(|last_access| last_access.elapsed() >= WORKER_IDLE_TIMEOUT)
		.unwrap_or(true)
}

fn redact_ffmpeg_error(error: &str, stream_uri: &str, password: &str) -> String {
	let error = error.replace(stream_uri, "[redacted RTSP URL]");
	if password.is_empty() {
		error
	} else {
		error.replace(password, "***")
	}
}

async fn decode_stream_once(config: &OnvifCameraConfig, state: &WorkerState) -> Result<()> {
	let (stream_uri, _) = stream_uri(config).await?;
	let mut child = Command::new("ffmpeg")
		.args([
			"-hide_banner",
			"-loglevel",
			"error",
			"-rtsp_transport",
			"tcp",
			"-timeout",
			"5000000",
			"-i",
			&stream_uri,
			"-an",
			"-vf",
			"fps=5,scale=960:540:force_original_aspect_ratio=decrease",
			"-f",
			"image2pipe",
			"-vcodec",
			"mjpeg",
			"pipe:1",
		])
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()
		.context("failed to start ffmpeg for ONVIF camera")?;
	let mut stdout = child
		.stdout
		.take()
		.ok_or_else(|| anyhow!("ffmpeg did not expose its video output"))?;
	let mut buffer = Vec::new();
	let mut chunk = [0u8; 16 * 1024];
	loop {
		if worker_is_idle(state) {
			let _ = child.kill().await;
			return Ok(());
		}
		let count = match timeout(Duration::from_secs(2), stdout.read(&mut chunk)).await {
			Ok(result) => result?,
			Err(_) => continue,
		};
		if count == 0 {
			let status = child.wait().await?;
			bail!("ffmpeg stream stopped with {status}");
		}
		buffer.extend_from_slice(&chunk[..count]);
		trim_to_jpeg_start(&mut buffer);
		while buffer.starts_with(&[0xff, 0xd8]) {
			let Some(end) = jpeg_end(&buffer[2..]).map(|end| end + 2) else {
				break;
			};
			let data = buffer.drain(..end).collect::<Vec<_>>();
			if let Ok(mut frame) = state.frame.lock() {
				*frame = Some(MediaFrame {
					mime: String::from("image/jpeg"),
					data,
				});
			}
			trim_to_jpeg_start(&mut buffer);
		}
		if buffer.len() > MAX_JPEG_SIZE {
			buffer.clear();
			bail!("ONVIF camera produced an oversized JPEG frame");
		}
	}
}

async fn run_worker(config: OnvifCameraConfig, state: Arc<WorkerState>) {
	loop {
		if worker_is_idle(&state) {
			return;
		}
		if let Err(error) = decode_stream_once(&config, &state).await {
			log::warn!("ONVIF camera {} stopped: {error}", config.id);
			if let Ok(mut worker_error) = state.error.lock() {
				*worker_error = Some(error.to_string());
			}
		}
		if worker_is_idle(&state) {
			return;
		}
		sleep(Duration::from_secs(2)).await;
	}
}

fn worker_state(config: OnvifCameraConfig) -> Result<Arc<WorkerState>> {
	let mut workers = camera_workers()
		.lock()
		.map_err(|_| anyhow!("ONVIF camera worker registry was poisoned"))?;
	if let Some(worker) = workers.get(&config.id)
		&& !worker.task.is_finished()
	{
		return Ok(Arc::clone(&worker.state));
	}
	let state = Arc::new(WorkerState {
		frame: Mutex::new(None),
		error: Mutex::new(None),
		last_access: Mutex::new(Instant::now()),
	});
	let task = tokio::spawn(run_worker(config.clone(), Arc::clone(&state)));
	workers.insert(
		config.id,
		CameraWorker {
			state: Arc::clone(&state),
			task,
		},
	);
	Ok(state)
}

fn configured_camera(id: &str) -> Result<OnvifCameraConfig> {
	let conn = open_db();
	load_onvif_camera(&conn, id)?.ok_or_else(|| anyhow!("ONVIF camera was not found"))
}

pub(crate) fn configured_cameras() -> Result<Vec<OnvifCameraConfig>> {
	load_onvif_cameras(&open_db())
}

pub(crate) fn configured_sources() -> Result<Vec<MediaSource>> {
	Ok(configured_cameras()?
		.into_iter()
		.filter(|camera| camera.enabled)
		.map(|camera| MediaSource {
			id: format!("{SOURCE_PREFIX}{}", camera.id),
			name: camera.name,
			kind: MediaSourceKind::Webcam,
			live: true,
			outputs: vec![MediaOutput {
				transport: MediaTransport::Stream,
				mime: String::from("image/jpeg"),
				codec: Some(String::from("mjpeg")),
			}],
		})
		.collect())
}

async fn prepared_stream(config: &mut OnvifCameraConfig) -> Result<String> {
	config.device_service_url = normalize_device_service_url(&config.device_service_url)?;
	let (stream_uri, profile_token) = stream_uri(config).await?;
	config.profile_token = profile_token;
	Ok(stream_uri)
}

pub(crate) async fn prepare_camera(config: &mut OnvifCameraConfig) -> Result<String> {
	prepared_stream(config).await?;
	Ok(format!(
		"Connected using ONVIF profile {}",
		config.profile_token
	))
}

pub(crate) async fn test_camera(config: &mut OnvifCameraConfig) -> Result<String> {
	let stream_uri = prepared_stream(config).await?;
	let output = timeout(
		Duration::from_secs(12),
		Command::new("ffmpeg")
			.args([
				"-hide_banner",
				"-loglevel",
				"error",
				"-rtsp_transport",
				"tcp",
				"-timeout",
				"5000000",
				"-i",
				&stream_uri,
				"-frames:v",
				"1",
				"-f",
				"image2pipe",
				"-vcodec",
				"mjpeg",
				"pipe:1",
			])
			.output(),
	)
	.await
	.map_err(|_| anyhow!("timed out waiting for the ONVIF RTSP stream"))??;
	if !output.status.success() || output.stdout.is_empty() {
		let error = String::from_utf8_lossy(&output.stderr);
		let error = redact_ffmpeg_error(error.trim(), &stream_uri, &config.password);
		bail!("failed to read ONVIF RTSP stream: {error}");
	}
	Ok(format!(
		"Connected using ONVIF profile {}; RTSP video frame received",
		config.profile_token
	))
}

pub(crate) async fn capture_frame(source_id: String) -> Result<MediaFrame> {
	let id = source_id
		.strip_prefix(SOURCE_PREFIX)
		.ok_or_else(|| anyhow!("invalid ONVIF source id"))?;
	let config = configured_camera(id)?;
	if !config.enabled {
		bail!("ONVIF camera is disabled");
	}
	let state = worker_state(config)?;
	if let Ok(mut last_access) = state.last_access.lock() {
		*last_access = Instant::now();
	}
	let started = Instant::now();
	loop {
		if let Some(frame) = state.frame.lock().ok().and_then(|frame| frame.clone()) {
			return Ok(frame);
		}
		if let Some(error) = state.error.lock().ok().and_then(|error| error.clone()) {
			return Err(anyhow!(error));
		}
		if started.elapsed() >= FRAME_WAIT_TIMEOUT {
			bail!("timed out waiting for an ONVIF camera frame");
		}
		sleep(Duration::from_millis(100)).await;
	}
}

pub(crate) fn invalidate_camera(id: &str) {
	if let Ok(mut workers) = camera_workers().lock()
		&& let Some(worker) = workers.remove(id)
	{
		worker.task.abort();
	}
}

pub(crate) async fn shutdown_workers() {
	let tasks = camera_workers()
		.lock()
		.map(|mut workers| {
			workers
				.drain()
				.map(|(_, worker)| {
					worker.task.abort();
					worker.task
				})
				.collect::<Vec<_>>()
		})
		.unwrap_or_default();
	for task in tasks {
		let _ = task.await;
	}
}

pub(crate) fn summaries(configs: &[OnvifCameraConfig]) -> Vec<OnvifCamera> {
	configs.iter().map(OnvifCamera::from).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalizes_camera_host_to_device_service_url() {
		assert_eq!(
			normalize_device_service_url("192.0.2.10").unwrap(),
			"http://192.0.2.10/onvif/device_service"
		);
		assert!(normalize_device_service_url("rtsp://192.0.2.10/live").is_err());
		assert!(normalize_device_service_url("http://user:pass@192.0.2.10").is_err());
	}

	#[test]
	fn reads_namespaced_onvif_values() {
		let xml = r#"<s:Envelope xmlns:s="soap"><s:Body><trt:GetProfilesResponse xmlns:trt="media"><trt:Profiles token="main"/><trt:Profiles token="sub"/></trt:GetProfilesResponse><td:XAddr xmlns:td="device">http://camera/media</td:XAddr></s:Body></s:Envelope>"#;
		assert_eq!(
			xml_text(xml, b"XAddr").unwrap().as_deref(),
			Some("http://camera/media")
		);
		assert_eq!(profile_tokens(xml).unwrap(), vec!["main", "sub"]);
	}

	#[test]
	fn security_header_does_not_include_plaintext_password() {
		let header = ws_security_header("camera-user", "very-secret-password");
		assert!(header.contains("camera-user"));
		assert!(header.contains("PasswordDigest"));
		assert!(!header.contains("very-secret-password"));
	}

	#[test]
	fn creates_http_digest_authorization_for_auth_qop() {
		let header = digest_header(
			r#"Digest realm="camera", qop="auth,auth-int", nonce="abc", opaque="xyz""#,
			"POST",
			"http://camera.local/onvif/device_service?channel=1",
			"camera-user",
			"very-secret-password",
		)
		.unwrap();
		assert!(header.contains(r#"username="camera-user""#));
		assert!(header.contains(r#"uri="/onvif/device_service?channel=1""#));
		assert!(header.contains("qop=auth"));
		assert!(header.contains(r#"opaque="xyz""#));
		assert!(!header.contains("very-secret-password"));
	}

	#[test]
	fn removes_rtsp_credentials_from_ffmpeg_errors() {
		let stream_uri = "rtsp://camera-user:very-secret-password@camera.local/stream";
		let error = format!("Error opening input file {stream_uri}: Connection refused");
		let redacted = redact_ffmpeg_error(&error, stream_uri, "very-secret-password");
		assert!(redacted.contains("[redacted RTSP URL]"));
		assert!(!redacted.contains("camera-user"));
		assert!(!redacted.contains("very-secret-password"));
	}

	#[test]
	fn trims_stream_noise_to_complete_jpeg() {
		let mut buffer = vec![0, 1, 0xff, 0xd8, 3, 4, 0xff, 0xd9, 9];
		trim_to_jpeg_start(&mut buffer);
		assert_eq!(&buffer[..2], &[0xff, 0xd8]);
		assert_eq!(jpeg_end(&buffer), Some(6));
	}
}
