use anyhow::{Context, Result};
use puppynet_core::{PuppyNet, http_api, ui};
use std::net::SocketAddr;
use std::sync::Arc;

pub mod control;

#[derive(Debug, Clone)]
pub struct Config {
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
		Ok(p) => p.wait().await,
		Err(_) => {
			log::warn!("PuppyNet still in use; skipping graceful shutdown");
		}
	}
}

pub async fn run(config: Config) -> Result<()> {
	let peer = Arc::new(PuppyNet::new());
	register_shared_folders(&peer, &config)?;

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
