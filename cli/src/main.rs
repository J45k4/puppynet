use anyhow::Context;
use args::Command;
use clap::Parser;
use std::path::Path;

mod args;
mod installer;
mod updater;
mod utility;

fn daemon_config(args: &args::Args) -> puppynet_daemon::Config {
	puppynet_daemon::Config {
		peers: args.peer.clone(),
		read: args.read.clone(),
		write: args.write.clone(),
		ui_bind: args.ui_bind.clone(),
		http: args.http.clone(),
	}
}

async fn run_daemon(args: &args::Args) {
	if let Err(err) = puppynet_daemon::run(daemon_config(args)).await {
		log::error!("daemon error: {err:?}");
		std::process::exit(1);
	}
}

fn set_env_if_missing(name: &str, value: &Path) {
	if std::env::var_os(name).is_none() {
		// SAFETY: This runs before the Tokio runtime or any application threads are created.
		unsafe { std::env::set_var(name, value) };
	}
}

fn configure_local_state() -> anyhow::Result<()> {
	let state_dir = std::env::current_dir()
		.context("failed to find current directory")?
		.join(".puppynet-dev");
	std::fs::create_dir_all(&state_dir).context("failed to create local state directory")?;
	set_env_if_missing("DB", &state_dir.join("puppynet.db"));
	set_env_if_missing("KEYPAIR", &state_dir.join("peer_keypair.bin"));
	set_env_if_missing("PUPPYNET_CONTROL_SOCKET", &state_dir.join("puppynet.sock"));
	Ok(())
}

async fn async_main(args: args::Args) {
	simple_logger::init_with_level(log::Level::Info).unwrap();

	let version_label = utility::get_version_label();
	log::info!("puppynet version {}", version_label);

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
		Some(Command::Install { system }) => {
			if let Err(err) = installer::install(*system) {
				log::error!("failed to install service: {err:?}");
				std::process::exit(1);
			}
			return;
		}
		Some(Command::Start { system }) => {
			if let Err(err) = installer::start(*system) {
				log::error!("failed to start service: {err:?}");
				std::process::exit(1);
			}
			return;
		}
		Some(Command::Stop { system }) => {
			if let Err(err) = installer::stop(*system) {
				log::error!("failed to stop service: {err:?}");
				std::process::exit(1);
			}
			return;
		}
		Some(Command::Restart { system }) => {
			if let Err(err) = installer::restart(*system) {
				log::error!("failed to restart service: {err:?}");
				std::process::exit(1);
			}
			return;
		}
		Some(Command::Status { system }) => {
			match installer::status(*system) {
				Ok(status) => log::info!("service status: {status}"),
				Err(err) => {
					log::error!("failed to get service status: {err:?}");
					std::process::exit(1);
				}
			}
			return;
		}
		Some(Command::Uninstall { system }) => {
			if let Err(err) = installer::uninstall(*system) {
				log::error!("failed to uninstall service: {err:?}");
				std::process::exit(1);
			}
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
			match puppynet_daemon::control::create_user(username, password).await {
				Ok(message) => {
					log::info!("{message}");
				}
				Err(err) => {
					log::error!("failed to create user {}: {err:?}", username);
					std::process::exit(1);
				}
			};
			return;
		}
		Some(Command::Grant {
			peer_id,
			all,
			read,
			write,
		}) => {
			match puppynet_daemon::control::grant(peer_id, *all, read, write).await {
				Ok(message) => {
					log::info!("{message}");
				}
				Err(err) => {
					log::error!("failed to grant peer {}: {err:?}", peer_id);
					std::process::exit(1);
				}
			};
			return;
		}
		Some(Command::Peers) => {
			match puppynet_daemon::control::connected_peers().await {
				Ok(peers) => {
					log::info!("connected peers: {}", peers.len());
					for peer in peers {
						log::info!("{peer}");
					}
				}
				Err(err) => {
					log::error!("failed to get connected peers: {err:?}");
					std::process::exit(1);
				}
			};
			return;
		}
		Some(Command::Daemon) => {
			run_daemon(&args).await;
			return;
		}
		None => {
			run_daemon(&args).await;
			return;
		}
	}
}

fn main() {
	let args = args::Args::parse();
	if args.command.is_none()
		&& let Err(err) = configure_local_state()
	{
		eprintln!("failed to configure local PuppyNet state: {err:?}");
		std::process::exit(1);
	}

	let runtime = tokio::runtime::Builder::new_multi_thread()
		.enable_all()
		.worker_threads(2)
		.build()
		.expect("failed to create Tokio runtime");
	runtime.block_on(async_main(args));
}
