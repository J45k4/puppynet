const SEMVER_MAJOR_FACTOR: u32 = 1_000_000;
const SEMVER_MINOR_FACTOR: u32 = 1_000;

fn encode_semver(major: u32, minor: u32, patch: u32) -> Option<u32> {
	major
		.checked_mul(SEMVER_MAJOR_FACTOR)?
		.checked_add(minor.checked_mul(SEMVER_MINOR_FACTOR)?)?
		.checked_add(patch)
}

pub(crate) fn version_number_from_label(label: &str) -> Option<u32> {
	let label = label.trim();
	if label.is_empty() {
		return None;
	}

	let label = label
		.strip_prefix('v')
		.or_else(|| label.strip_prefix('V'))
		.unwrap_or(label);
	if label.chars().all(|ch| ch.is_ascii_digit()) {
		return label.parse::<u32>().ok();
	}

	let base = label.split(['-', '+']).next()?;
	let mut parts = base.split('.');
	let major = parts.next()?.parse::<u32>().ok()?;
	let minor = parts.next()?.parse::<u32>().ok()?;
	let patch = parts.next()?.parse::<u32>().ok()?;
	if parts.next().is_some() {
		return None;
	}

	encode_semver(major, minor, patch)
}

pub(crate) fn version_label_str() -> &'static str {
	option_env!("VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub(crate) fn version_label() -> String {
	version_label_str().to_string()
}

pub(crate) fn version_number() -> u32 {
	version_number_from_label(version_label_str()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
	use super::version_number_from_label;

	#[test]
	fn version_number_accepts_legacy_numeric_tags() {
		assert_eq!(version_number_from_label("12"), Some(12));
	}

	#[test]
	fn version_number_accepts_semantic_tags() {
		assert_eq!(version_number_from_label("0.0.1"), Some(1));
		assert_eq!(version_number_from_label("v0.1.2"), Some(1_002));
		assert_eq!(version_number_from_label("1.2.3"), Some(1_002_003));
	}

	#[test]
	fn version_number_accepts_semantic_metadata() {
		assert_eq!(version_number_from_label("0.0.1-beta.1"), Some(1));
		assert_eq!(version_number_from_label("0.0.1+build.2"), Some(1));
	}

	#[test]
	fn version_number_rejects_unknown_labels() {
		assert_eq!(version_number_from_label("dev"), None);
		assert_eq!(version_number_from_label("0.1"), None);
	}
}
