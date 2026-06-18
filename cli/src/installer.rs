use anyhow::Context;
use service_manager::*;
use std::path::PathBuf;

const SERVICE_LABEL: &str = "puppynet";

fn current_exe() -> anyhow::Result<PathBuf> {
	std::env::current_exe().context("failed to get current exe")
}

fn app_dir() -> anyhow::Result<PathBuf> {
	let path = homedir::my_home()
		.context("failed to resolve home directory")?
		.context("home directory not found")?
		.join(".puppynet");
	std::fs::create_dir_all(&path).context("failed to create puppynet app directory")?;
	Ok(path)
}

fn bin_dir() -> anyhow::Result<PathBuf> {
	let path = app_dir()?.join("bin");
	std::fs::create_dir_all(&path).context("failed to create puppynet bin directory")?;
	Ok(path)
}

fn managed_binary_path() -> anyhow::Result<PathBuf> {
	Ok(bin_dir()?.join(if cfg!(windows) {
		"puppynet.exe"
	} else {
		"puppynet"
	}))
}

fn copy_current_exe_to_managed_path() -> anyhow::Result<PathBuf> {
	let source = current_exe()?;
	let target = managed_binary_path()?;

	if source == target {
		return Ok(target);
	}

	let temp_target = target.with_extension("new");
	if temp_target.exists() {
		std::fs::remove_file(&temp_target)
			.context("failed to remove old staged puppynet binary")?;
	}

	std::fs::copy(&source, &temp_target).context("failed to stage puppynet service binary")?;

	#[cfg(windows)]
	if target.exists() {
		std::fs::remove_file(&target).context("failed to replace puppynet service binary")?;
	}

	std::fs::rename(&temp_target, &target).context("failed to install puppynet service binary")?;
	Ok(target)
}

fn service_manager() -> anyhow::Result<Box<dyn ServiceManager>> {
	let mut manager =
		<dyn ServiceManager>::native().context("no supported service manager found")?;
	if let Err(err) = manager.set_level(ServiceLevel::User) {
		log::info!("user-level service is not supported by this service manager: {err}");
	}
	Ok(manager)
}

pub fn install() -> anyhow::Result<()> {
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager()?;
	let program = copy_current_exe_to_managed_path()?;
	manager.install(ServiceInstallCtx {
		label: label.clone(),
		program,
		args: vec![],
		contents: None,
		username: None,
		working_directory: None,
		autostart: true,
		disable_restart_on_failure: false,
		environment: Some(vec![(String::from("RUST_BACKTRACE"), String::from("1"))]),
	})?;
	log::info!("Service installed: {}", SERVICE_LABEL);
	manager.start(ServiceStartCtx { label })?;
	Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager()?;
	manager.uninstall(ServiceUninstallCtx { label })?;
	log::info!("Service uninstalled: {}", SERVICE_LABEL);
	Ok(())
}
