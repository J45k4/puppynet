use anyhow::{Context, Result, anyhow, bail};
use puppynet_core::{
	FLAG_READ, FLAG_SEARCH, FLAG_WRITE, FolderRule, PeerId, Permission, PuppyNet, Rule, updater,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[cfg(unix)]
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(unix)]
const CONNECT_RETRY_COUNT: usize = 30;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum ControlRequest {
	CreateUser {
		username: String,
		password: String,
	},
	Grant {
		peer_id: String,
		all: bool,
		read: Vec<String>,
		write: Vec<String>,
	},
	Peers,
	Update {
		version: Option<String>,
		current_version: u32,
	},
}

#[derive(Debug, Deserialize, Serialize)]
struct ControlResponse {
	ok: bool,
	message: String,
	#[serde(default)]
	peers: Option<Vec<String>>,
}

fn app_dir() -> Result<PathBuf> {
	let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
	let path = PathBuf::from(home).join(".puppynet");
	std::fs::create_dir_all(&path).context("failed to create puppynet app directory")?;
	Ok(path)
}

fn socket_path() -> Result<PathBuf> {
	if let Some(path) = std::env::var_os("PUPPYNET_CONTROL_SOCKET") {
		return Ok(PathBuf::from(path));
	}
	Ok(app_dir()?.join("puppynet.sock"))
}

fn ok(message: impl Into<String>) -> ControlResponse {
	ControlResponse {
		ok: true,
		message: message.into(),
		peers: None,
	}
}

fn error_response(message: impl Into<String>) -> ControlResponse {
	ControlResponse {
		ok: false,
		message: message.into(),
		peers: None,
	}
}

fn peers_response(peer_ids: Vec<String>) -> ControlResponse {
	ControlResponse {
		ok: true,
		message: String::new(),
		peers: Some(peer_ids),
	}
}

#[cfg(unix)]
async fn connect_socket(path: &PathBuf) -> Result<tokio::net::UnixStream> {
	tokio::net::UnixStream::connect(path)
		.await
		.with_context(|| {
			format!(
				"failed to connect to daemon control socket {}",
				path.display()
			)
		})
}

#[cfg(unix)]
async fn write_response(
	stream: &mut tokio::net::UnixStream,
	response: &ControlResponse,
) -> Result<()> {
	let mut line = serde_json::to_vec(response).context("failed to encode control response")?;
	line.push(b'\n');
	stream
		.write_all(&line)
		.await
		.context("failed to write control response")
}

#[cfg(unix)]
async fn read_request(stream: &mut tokio::net::UnixStream) -> Result<ControlRequest> {
	let mut reader = BufReader::new(stream);
	let mut line = String::new();
	reader
		.read_line(&mut line)
		.await
		.context("failed to read control request")?;
	serde_json::from_str(&line).context("failed to decode control request")
}

fn grant_permissions(all: bool, read: Vec<String>, write: Vec<String>) -> Result<Vec<Permission>> {
	if all && (!read.is_empty() || !write.is_empty()) {
		bail!("--all cannot be combined with --read or --write");
	}
	if all {
		return Ok(vec![Permission::new(Rule::Owner)]);
	}
	if read.is_empty() && write.is_empty() {
		bail!("grant needs at least one of --all, --read PATH, or --write PATH");
	}

	let mut permissions = Vec::new();
	for path in read {
		permissions.push(Permission::new(Rule::Folder(FolderRule::new(
			PathBuf::from(path),
			FLAG_READ | FLAG_SEARCH,
		))));
	}
	for path in write {
		permissions.push(Permission::new(Rule::Folder(FolderRule::new(
			PathBuf::from(path),
			FLAG_READ | FLAG_WRITE | FLAG_SEARCH,
		))));
	}
	Ok(permissions)
}

async fn handle_request(peer: &PuppyNet, request: ControlRequest) -> ControlResponse {
	match request {
		ControlRequest::CreateUser { username, password } => {
			match peer.create_user(username.clone(), password) {
				Ok(()) => ok(format!("user {username} created")),
				Err(err) => error_response(format!("failed to create user {username}: {err:?}")),
			}
		}
		ControlRequest::Grant {
			peer_id,
			all,
			read,
			write,
		} => match peer_id.parse::<PeerId>() {
			Ok(peer_id) => {
				let permissions = grant_permissions(all, read, write);
				match permissions {
					Ok(permissions) => match peer.set_peer_permissions(peer_id, permissions) {
						Ok(()) => ok(format!("granted access to peer {peer_id}")),
						Err(err) => error_response(format!(
							"failed to grant access to peer {peer_id}: {err:?}"
						)),
					},
					Err(err) => {
						error_response(format!("invalid grant for peer {peer_id}: {err:?}"))
					}
				}
			}
			Err(err) => error_response(format!("invalid peer id {peer_id}: {err}")),
		},
		ControlRequest::Peers => match peer.state_snapshot().await {
			Some(state) => {
				let peer_ids = state
					.connections
					.into_iter()
					.map(|connection| connection.peer_id.to_string())
					.collect::<BTreeSet<_>>()
					.into_iter()
					.collect();
				peers_response(peer_ids)
			}
			None => error_response("failed to read daemon state"),
		},
		ControlRequest::Update {
			version,
			current_version,
		} => match updater::update(version.as_deref(), current_version).await {
			Ok(result) if result.success => ok(result.message),
			Ok(result) => error_response(result.message),
			Err(err) => error_response(format!("failed to update: {err:?}")),
		},
	}
}

#[cfg(unix)]
async fn handle_connection(peer: Arc<PuppyNet>, mut stream: tokio::net::UnixStream) {
	let response = match read_request(&mut stream).await {
		Ok(request) => handle_request(&peer, request).await,
		Err(err) => error_response(format!("{err:?}")),
	};
	if let Err(err) = write_response(&mut stream, &response).await {
		log::warn!("failed to write control response: {err:?}");
	}
}

#[cfg(unix)]
async fn remove_stale_socket(path: &PathBuf) -> Result<()> {
	if !path.exists() {
		return Ok(());
	}

	if tokio::net::UnixStream::connect(path).await.is_ok() {
		bail!(
			"daemon control socket is already active at {}",
			path.display()
		);
	}

	std::fs::remove_file(path).with_context(|| {
		format!(
			"failed to remove stale daemon control socket {}",
			path.display()
		)
	})
}

#[cfg(unix)]
fn restrict_socket_permissions(path: &PathBuf) -> Result<()> {
	use std::os::unix::fs::PermissionsExt;

	let permissions = std::fs::Permissions::from_mode(0o600);
	std::fs::set_permissions(path, permissions).with_context(|| {
		format!(
			"failed to restrict daemon control socket permissions {}",
			path.display()
		)
	})
}

#[cfg(unix)]
async fn start_user_service() -> Result<()> {
	let status = tokio::process::Command::new("systemctl")
		.arg("--user")
		.arg("start")
		.arg("puppynet")
		.status()
		.await
		.context("failed to run systemctl --user start puppynet")?;
	if status.success() {
		Ok(())
	} else {
		bail!("systemctl --user start puppynet exited with {status}");
	}
}

#[cfg(unix)]
async fn connect_socket_after_service_start(path: &PathBuf) -> Result<tokio::net::UnixStream> {
	for _ in 0..CONNECT_RETRY_COUNT {
		match tokio::net::UnixStream::connect(path).await {
			Ok(stream) => return Ok(stream),
			Err(_) => tokio::time::sleep(CONNECT_RETRY_DELAY).await,
		}
	}
	connect_socket(path).await
}

#[cfg(unix)]
async fn connect_or_start_daemon(path: &PathBuf) -> Result<tokio::net::UnixStream> {
	match connect_socket(path).await {
		Ok(stream) => Ok(stream),
		Err(connect_err) => {
			if let Err(start_err) = start_user_service().await {
				return Err(connect_err)
					.with_context(|| format!("failed to start user service: {start_err:?}"));
			}
			connect_socket_after_service_start(path).await
		}
	}
}

#[cfg(unix)]
async fn send_request(request: ControlRequest) -> Result<ControlResponse> {
	let path = socket_path()?;
	let mut stream = connect_or_start_daemon(&path).await?;
	let mut line = serde_json::to_vec(&request).context("failed to encode control request")?;
	line.push(b'\n');
	stream
		.write_all(&line)
		.await
		.context("failed to write control request")?;

	let mut reader = BufReader::new(stream);
	let mut response_line = String::new();
	reader
		.read_line(&mut response_line)
		.await
		.context("failed to read control response")?;
	let response: ControlResponse =
		serde_json::from_str(&response_line).context("failed to decode control response")?;
	if response.ok {
		Ok(response)
	} else {
		bail!("{}", response.message)
	}
}

#[cfg(not(unix))]
async fn send_request(_request: ControlRequest) -> Result<ControlResponse> {
	bail!("daemon control socket is only supported on Unix platforms")
}

fn validate_grant_options(all: bool, read: &[String], write: &[String]) -> Result<()> {
	if all && (!read.is_empty() || !write.is_empty()) {
		bail!("--all cannot be combined with --read or --write");
	}
	if !all && read.is_empty() && write.is_empty() {
		bail!("grant needs at least one of --all, --read PATH, or --write PATH");
	}
	Ok(())
}

pub async fn create_user(username: &str, password: &str) -> Result<String> {
	let request = ControlRequest::CreateUser {
		username: username.to_string(),
		password: password.to_string(),
	};
	Ok(send_request(request).await?.message)
}

pub async fn grant(peer_id: &str, all: bool, read: &[String], write: &[String]) -> Result<String> {
	validate_grant_options(all, read, write)?;
	let request = ControlRequest::Grant {
		peer_id: peer_id.to_string(),
		all,
		read: read.to_vec(),
		write: write.to_vec(),
	};
	Ok(send_request(request).await?.message)
}

pub async fn connected_peers() -> Result<Vec<String>> {
	let response = send_request(ControlRequest::Peers).await?;
	response
		.peers
		.ok_or_else(|| anyhow!("daemon returned no connected peer list"))
}

pub async fn update(version: Option<&str>, current_version: u32) -> Result<String> {
	let request = ControlRequest::Update {
		version: version.map(str::to_string),
		current_version,
	};
	Ok(send_request(request).await?.message)
}

#[cfg(unix)]
pub async fn run(peer: Arc<PuppyNet>) -> Result<()> {
	let path = socket_path()?;
	remove_stale_socket(&path).await?;
	let listener = tokio::net::UnixListener::bind(&path)
		.with_context(|| format!("failed to bind daemon control socket {}", path.display()))?;
	restrict_socket_permissions(&path)?;
	log::info!("daemon control socket listening on {}", path.display());

	loop {
		match listener.accept().await {
			Ok((stream, _addr)) => {
				tokio::spawn(handle_connection(Arc::clone(&peer), stream));
			}
			Err(err) => {
				log::warn!("failed to accept daemon control connection: {err:?}");
			}
		}
	}
}

#[cfg(not(unix))]
pub async fn run(_peer: Arc<PuppyNet>) -> Result<()> {
	bail!("daemon control socket is only supported on Unix platforms")
}
