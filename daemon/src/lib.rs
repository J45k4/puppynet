use anyhow::{Context, Result};
use puppynet_core::{NodeId, PuppyNet, http_api, ui};
use std::net::SocketAddr;
use std::sync::Arc;

pub mod control;

#[derive(Debug, Clone)]
pub struct Config {
	pub peers: Vec<String>,
	pub read: Vec<String>,
	pub write: Vec<String>,
	pub ui_bind: String,
	pub http: Option<String>,
}

fn register_shared_folders(peer: &PuppyNet, config: &Config) -> Result<()> {
	for path in &config.read {
		peer.share_read_only_folder(path)
			.with_context(|| format!("failed to share {path} for read"))?;
	}
	for path in &config.write {
		peer.share_read_write_folder(path)
			.with_context(|| format!("failed to share {path} for read/write"))?;
	}
	Ok(())
}

fn connect_configured_peers(peer: &PuppyNet, config: &Config) -> Result<()> {
	for value in &config.peers {
		let (node_id, endpoint) = value
			.split_once('@')
			.with_context(|| format!("invalid --peer {value}; expected <node-id>@<ws-url>"))?;
		let node_id = node_id
			.parse::<NodeId>()
			.with_context(|| format!("invalid peer node id {node_id}"))?;
		if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
			anyhow::bail!("invalid peer endpoint {endpoint}; expected ws:// or wss:// URL");
		}
		peer.inject_discovered_peer(node_id, endpoint.to_string())
			.map_err(|error| anyhow::anyhow!(error))?;
	}
	Ok(())
}

fn parse_socket_addr(label: &str, value: &str) -> Result<SocketAddr> {
	value
		.parse::<SocketAddr>()
		.with_context(|| format!("invalid {label} address {value}"))
}

fn spawn_ui(peer: Arc<PuppyNet>, bind: SocketAddr) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		if let Err(err) = ui::run_ui(peer, bind).await {
			log::error!("ui server error: {err:?}");
		}
	})
}

fn spawn_http(peer: Arc<PuppyNet>, bind: SocketAddr) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		if let Err(err) = http_api::serve(peer, bind).await {
			log::error!("http server error: {err:?}");
		}
	})
}

fn spawn_control(peer: Arc<PuppyNet>) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		if let Err(err) = control::run(peer).await {
			log::error!("daemon control socket error: {err:?}");
		}
	})
}

async fn wait_for_shutdown() {
	#[cfg(unix)]
	{
		use tokio::signal::unix::{SignalKind, signal};

		let mut terminate = match signal(SignalKind::terminate()) {
			Ok(signal) => signal,
			Err(err) => {
				log::error!("failed to listen for SIGTERM: {err}");
				let _ = tokio::signal::ctrl_c().await;
				return;
			}
		};
		tokio::select! {
			result = tokio::signal::ctrl_c() => {
				if let Err(err) = result {
					log::error!("failed to listen for ctrl_c: {err}");
				}
			}
			_ = terminate.recv() => {}
		}
	}
	#[cfg(not(unix))]
	if let Err(err) = tokio::signal::ctrl_c().await {
		log::error!("failed to listen for ctrl_c: {err}");
	}
}

async fn stop_task(task: tokio::task::JoinHandle<()>) {
	task.abort();
	let _ = task.await;
}

async fn wait_for_peer(peer: Arc<PuppyNet>) {
	match Arc::try_unwrap(peer) {
		Ok(p) => p.shutdown().await,
		Err(_) => {
			log::warn!("PuppyNet still in use; skipping graceful shutdown");
		}
	}
}

pub async fn run(config: Config) -> Result<()> {
	let peer = Arc::new(PuppyNet::new());
	register_shared_folders(&peer, &config)?;
	connect_configured_peers(&peer, &config)?;

	let ui_addr = parse_socket_addr("--ui-bind", &config.ui_bind)?;
	let ui_task = spawn_ui(Arc::clone(&peer), ui_addr);
	let control_task = spawn_control(Arc::clone(&peer));

	let http_task = if let Some(addr_str) = &config.http {
		Some(spawn_http(
			Arc::clone(&peer),
			parse_socket_addr("--http", addr_str)?,
		))
	} else {
		None
	};

	wait_for_shutdown().await;
	stop_task(control_task).await;
	stop_task(ui_task).await;
	if let Some(task) = http_task {
		stop_task(task).await;
	}
	wait_for_peer(peer).await;

	Ok(())
}
