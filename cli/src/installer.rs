use anyhow::Context;
use service_manager::*;
use std::path::PathBuf;

const SERVICE_LABEL: &str = "puppynet";
const SYSTEM_BINARY_PATH: &str = "/usr/local/bin/puppynet";

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

fn user_binary_path() -> anyhow::Result<PathBuf> {
	Ok(bin_dir()?.join(if cfg!(windows) {
		"puppynet.exe"
	} else {
		"puppynet"
	}))
}

fn system_binary_path() -> PathBuf {
	PathBuf::from(if cfg!(windows) {
		"puppynet.exe"
	} else {
		SYSTEM_BINARY_PATH
	})
}

fn managed_binary_path(level: ServiceLevel) -> anyhow::Result<PathBuf> {
	match level {
		ServiceLevel::User => user_binary_path(),
		ServiceLevel::System => Ok(system_binary_path()),
	}
}

fn copy_current_exe_to_managed_path(level: ServiceLevel) -> anyhow::Result<PathBuf> {
	let source = current_exe()?;
	let target = managed_binary_path(level)?;

	if source == target {
		return Ok(target);
	}

	if let Some(parent) = target.parent() {
		std::fs::create_dir_all(parent).context("failed to create puppynet bin directory")?;
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

fn install_level(system: bool) -> ServiceLevel {
	if system {
		ServiceLevel::System
	} else {
		ServiceLevel::User
	}
}

fn service_manager(level: ServiceLevel) -> anyhow::Result<Box<dyn ServiceManager>> {
	let mut manager =
		<dyn ServiceManager>::native().context("no supported service manager found")?;
	manager
		.set_level(level)
		.with_context(|| format!("service manager does not support {level:?} services"))?;
	Ok(manager)
}

fn service_status_label(status: ServiceStatus) -> String {
	match status {
		ServiceStatus::NotInstalled => String::from("not installed"),
		ServiceStatus::Running => String::from("running"),
		ServiceStatus::Stopped(Some(reason)) => format!("stopped ({reason})"),
		ServiceStatus::Stopped(None) => String::from("stopped"),
	}
}

fn stop_service(manager: &dyn ServiceManager, label: ServiceLabel) -> anyhow::Result<()> {
	manager.stop(ServiceStopCtx { label })?;
	Ok(())
}

pub fn install(system: bool) -> anyhow::Result<()> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	let program = copy_current_exe_to_managed_path(level)?;
	manager.install(ServiceInstallCtx {
		label: label.clone(),
		program,
		args: vec![String::from("daemon").into()],
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

pub fn start(system: bool) -> anyhow::Result<()> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	manager.start(ServiceStartCtx { label })?;
	log::info!("Service started: {}", SERVICE_LABEL);
	Ok(())
}

pub fn stop(system: bool) -> anyhow::Result<()> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	stop_service(manager.as_ref(), label)?;
	log::info!("Service stopped: {}", SERVICE_LABEL);
	Ok(())
}

pub fn restart(system: bool) -> anyhow::Result<()> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	if let Err(err) = stop_service(manager.as_ref(), label.clone()) {
		log::warn!("failed to stop service before restart: {err}");
	}
	manager.start(ServiceStartCtx { label })?;
	log::info!("Service restarted: {}", SERVICE_LABEL);
	Ok(())
}

pub fn status(system: bool) -> anyhow::Result<String> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	let status = manager.status(ServiceStatusCtx { label })?;
	Ok(service_status_label(status))
}

pub fn uninstall(system: bool) -> anyhow::Result<()> {
	let level = install_level(system);
	let label: ServiceLabel = SERVICE_LABEL.parse()?;
	let manager = service_manager(level)?;
	if let Err(err) = stop_service(manager.as_ref(), label.clone()) {
		log::warn!("failed to stop service before uninstall: {err}");
	}
	manager.uninstall(ServiceUninstallCtx { label })?;
	log::info!("Service uninstalled: {}", SERVICE_LABEL);
	Ok(())
}
