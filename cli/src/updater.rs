use crate::utility::get_version;

pub async fn update(version: Option<&str>) -> anyhow::Result<()> {
	let current_version = get_version();
	let message = puppynet_daemon::control::update(version, current_version).await?;
	log::info!("{message}");
	Ok(())
}
