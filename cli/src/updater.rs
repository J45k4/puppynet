//! CLI update wrapper that uses the core updater module.

use crate::utility::get_version;
use puppynet_core::updater;

/// Perform an update to the specified version (or latest if None).
/// This is a thin wrapper around the core updater that provides the current version.
pub async fn update(version: Option<&str>) -> anyhow::Result<()> {
	let current_version = get_version();
	let result = updater::update(version, current_version).await?;

	if !result.success {
		anyhow::bail!("{}", result.message);
	}

	Ok(())
}
