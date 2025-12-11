use args::Command;
use clap::Parser;
use puppynet_core::{PuppyNet, http_api};
use std::net::SocketAddr;
use std::sync::Arc;

mod args;
#[cfg(feature = "iced")]
mod gui;
mod installer;
mod updater;
mod utility;

#[tokio::main]
async fn main() {
	let args = args::Args::parse();
	simple_logger::init_with_level(log::Level::Info).unwrap();

	let version_label = utility::get_version_label().unwrap_or("dev");
	log::info!("puppyagent version {}", version_label);

	#[cfg(feature = "rayon")]
	log::info!("rayon enabled");

	match &args.command {
		Some(Command::Copy { src, dest }) => {
			log::info!("copying {} to {}", src, dest);
		}
		Some(Command::Scan { path }) => {
			log::info!("scanning {} (database disabled)", path);
			return;
		}
		Some(Command::Install) => {
			installer::install();
			return;
		}
		Some(Command::Uninstall) => {
			installer::uninstall();
			return;
		}
		Some(Command::Update { version }) => {
			if let Err(err) = updater::update(version.as_deref()).await {
				log::error!("failed to update: {err:?}");
				std::process::exit(1);
			}
			log::info!("update completed successfully");
			return;
		}
		Some(Command::CreateUser { username, password }) => {
			let peer = Arc::new(PuppyNet::new());
			if let Err(err) = peer.create_user(username.clone(), password.clone()) {
				log::error!("failed to create user {}: {err:?}", username);
				std::process::exit(1);
			}
			log::info!("user {} created", username);
			return;
		}
		#[cfg(feature = "iced")]
		Some(Command::Gui) => {
			let app_title = format!("PuppyNet v{}", version_label);
			if let Err(err) = gui::run(app_title) {
				log::error!("gui error: {err:?}");
				std::process::exit(1);
			}
			return;
		}
		Some(Command::Daemon) => {
			log::warn!("Daemon mode: disabled modules");
			return;
		}
		None => {
			let peer = Arc::new(PuppyNet::new());
			for path in &args.read {
				if let Err(err) = peer.share_read_only_folder(path) {
					log::error!("failed to share {} for read: {err:?}", path);
					std::process::exit(1);
				}
			}
			for path in &args.write {
				if let Err(err) = peer.share_read_write_folder(path) {
					log::error!("failed to share {} for read/write: {err:?}", path);
					std::process::exit(1);
				}
			}
			let mut http_task = None;
			if let Some(addr_str) = &args.http {
				match addr_str.parse::<SocketAddr>() {
					Ok(addr) => {
						let puppy = Arc::clone(&peer);
						let handle = tokio::spawn(async move {
							if let Err(err) = http_api::serve(puppy, addr).await {
								log::error!("http server error: {err:?}");
							}
						});
						http_task = Some(handle);
					}
					Err(err) => {
						log::error!("invalid --http address {}: {err}", addr_str);
						std::process::exit(1);
					}
				}
			}

			if http_task.is_some() {
				if let Err(err) = tokio::signal::ctrl_c().await {
					log::error!("failed to listen for ctrl_c: {err}");
				}
				if let Some(task) = http_task {
					task.abort();
					let _ = task.await;
				}
			}

			match Arc::try_unwrap(peer) {
				Ok(p) => p.wait().await,
				Err(_) => {
					log::warn!("PuppyNet still in use; skipping graceful shutdown");
				}
			}
			return;
		}
	}
}
