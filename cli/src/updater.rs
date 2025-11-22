//! CLI update wrapper that uses the core updater module.

use crate::utility::get_version;
use puppynet_core::updater;

// Re-export the public key and verify_signature for backwards compatibility
pub use puppynet_core::updater::{PUBLIC_KEY, verify_signature};

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
