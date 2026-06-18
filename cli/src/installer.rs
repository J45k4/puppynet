use service_manager::*;
use std::{env, path::PathBuf};

const SERVICE_LABEL: &str = "puppynet";

fn current_exe() -> PathBuf {
	env::current_exe().expect("failed to get current exe")
}

fn app_dir() -> PathBuf {
	let path = homedir::my_home()
		.expect("failed to resolve home directory")
		.expect("home directory not found")
		.join(".puppynet");
	std::fs::create_dir_all(&path).expect("failed to create puppynet app directory");
	path
}

fn bin_dir() -> PathBuf {
	let path = app_dir().join("bin");
	std::fs::create_dir_all(&path).expect("failed to create puppynet bin directory");
	path
}

fn managed_binary_path() -> PathBuf {
	bin_dir().join(if cfg!(windows) {
		"puppynet.exe"
	} else {
		"puppynet"
	})
}

fn copy_current_exe_to_managed_path() -> PathBuf {
	let source = current_exe();
	let target = managed_binary_path();

	if source == target {
		return target;
	}

	let temp_target = target.with_extension("new");
	if temp_target.exists() {
		std::fs::remove_file(&temp_target).expect("failed to remove old staged puppynet binary");
	}

	std::fs::copy(&source, &temp_target).expect("failed to stage puppynet service binary");

	#[cfg(windows)]
	if target.exists() {
		std::fs::remove_file(&target).expect("failed to replace puppynet service binary");
	}

	std::fs::rename(&temp_target, &target).expect("failed to install puppynet service binary");
	target
}

pub fn install() {
	let label: ServiceLabel = SERVICE_LABEL.parse().unwrap();
	let manager = <dyn ServiceManager>::native().expect("no supported service manager found");
	let program = copy_current_exe_to_managed_path();
	manager
		.install(ServiceInstallCtx {
			label: label.clone(),
			program,
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
