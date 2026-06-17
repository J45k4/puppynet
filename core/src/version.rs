pub(crate) fn version_label() -> String {
	option_env!("VERSION").unwrap_or("dev").to_string()
}
