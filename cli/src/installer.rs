use service_manager::*;
use std::{env, path::PathBuf};

const SERVICE_LABEL: &str = "puppynet";

fn current_exe() -> PathBuf {
	env::current_exe().expect("failed to get current exe")
}

pub fn install() {
	let label: ServiceLabel = SERVICE_LABEL.parse().unwrap();
	let manager = <dyn ServiceManager>::native().expect("no supported service manager found");
	manager
		.install(ServiceInstallCtx {
			label: label.clone(),
			program: current_exe(),
			args: vec![],
			contents: None,
			username: None,
			working_directory: None,
			autostart: true,
			disable_restart_on_failure: false,
			environment: Some(vec![(String::from("RUST_BACKTRACE"), String::from("1"))]),
		})
		.unwrap();
	log::info!("Service installed: {}", SERVICE_LABEL);
	manager.start(ServiceStartCtx { label }).unwrap();
}

pub fn uninstall() {
	let label: ServiceLabel = SERVICE_LABEL.parse().unwrap();
	let manager = <dyn ServiceManager>::native().unwrap();
	manager.uninstall(ServiceUninstallCtx { label }).unwrap();
	log::info!("Service uninstalled: {}", SERVICE_LABEL);
}
