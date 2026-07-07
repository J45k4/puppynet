pub fn get_version() -> u32 {
	version_number(get_version_label()).unwrap_or(0)
}

pub fn get_version_label() -> &'static str {
	option_env!("VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn encode_semver(major: u32, minor: u32, patch: u32) -> Option<u32> {
	major
		.checked_mul(1_000_000)?
		.checked_add(minor.checked_mul(1_000)?)?
		.checked_add(patch)
}

fn version_number(label: &str) -> Option<u32> {
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
