use crate::p2p::{AudioCapability, AudioDevice, AudioDeviceKind};
use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::process::Command as TokioCommand;

#[async_trait]
trait AudioBackend: Send + Sync {
	fn name(&self) -> &'static str;
	async fn available(&self) -> Result<()>;
	async fn list_devices(&self) -> Result<Vec<AudioDevice>>;
	async fn set_default_device(&self, device_id: String) -> Result<Vec<AudioDevice>>;
	async fn set_muted(&self, device_id: Option<String>, muted: bool) -> Result<Vec<AudioDevice>>;
	async fn set_volume(&self, device_id: Option<String>, volume: u8) -> Result<Vec<AudioDevice>>;

	fn capability(&self) -> AudioCapability {
		AudioCapability {
			supported: true,
			backend: Some(self.name().to_string()),
			message: format!("Audio backend: {}", self.name()),
		}
	}
}

struct PactlAudioBackend;

struct UnsupportedAudioBackend {
	message: String,
}

struct WpctlAudioBackend;

fn audio_target_for_tool(device_id: &Option<String>, default_target: &str) -> String {
	device_id
		.as_deref()
		.filter(|value| !value.trim().is_empty())
		.unwrap_or(default_target)
		.to_string()
}

fn parse_percent_token(token: &str) -> Option<u8> {
	if !token.ends_with('%') {
		return None;
	}
	let value = token.trim_end_matches('%').parse::<u16>().ok()?;
	Some(value.min(100) as u8)
}

fn parse_pactl_volume(output: &str) -> Option<u8> {
	output.split_whitespace().find_map(parse_percent_token)
}

fn parse_wpctl_sink_line(line: &str) -> Option<AudioDevice> {
	let trimmed = line.trim();
	let is_default = trimmed.starts_with('*');
	let trimmed = trimmed.strip_prefix('*').unwrap_or(trimmed).trim();
	let (id, rest) = trimmed.split_once('.')?;
	let id = id.trim();
	if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) {
		return None;
	}
	let name = rest.split_once('[').map(|(name, _)| name).unwrap_or(rest);
	let name = name.trim();
	if name.is_empty() {
		return None;
	}
	let volume = line
		.split_whitespace()
		.find_map(|token| token.trim_end_matches(']').parse::<f32>().ok())
		.map(|value| (value * 100.0).round().clamp(0.0, 100.0) as u8)
		.unwrap_or(0);
	Some(AudioDevice {
		id: id.to_string(),
		name: name.to_string(),
		description: String::from("PipeWire output"),
		kind: AudioDeviceKind::Sink,
		volume,
		muted: line.contains("[MUTED]"),
		is_default,
	})
}

fn parse_wpctl_sinks(output: &str) -> Vec<AudioDevice> {
	let mut in_sinks = false;
	let mut devices = Vec::new();
	for line in output.lines() {
		let trimmed = line.trim_start_matches(|ch| matches!(ch, ' ' | '│' | '├' | '└' | '─'));
		if trimmed.starts_with("Sinks:") {
			in_sinks = true;
			continue;
		}
		if in_sinks && trimmed.ends_with(':') {
			break;
		}
		if in_sinks && let Some(device) = parse_wpctl_sink_line(trimmed) {
			devices.push(device);
		}
	}
	devices
}

fn push_pactl_sink(
	devices: &mut Vec<AudioDevice>,
	sink_number: &str,
	name: &str,
	description: &str,
	volume: u8,
	muted: bool,
	default_sink: &str,
) {
	if name.is_empty() {
		return;
	}
	devices.push(AudioDevice {
		id: name.to_string(),
		name: if description.is_empty() {
			name.to_string()
		} else {
			description.to_string()
		},
		description: if description.is_empty() {
			format!("PulseAudio sink {sink_number}")
		} else {
			description.to_string()
		},
		kind: AudioDeviceKind::Sink,
		volume,
		muted,
		is_default: name == default_sink,
	});
}

fn parse_pactl_sinks(output: &str, default_sink: &str) -> Vec<AudioDevice> {
	let mut devices = Vec::new();
	let mut sink_number = String::new();
	let mut name = String::new();
	let mut description = String::new();
	let mut volume = 0;
	let mut muted = false;
	let mut seen_sink = false;

	for line in output.lines().chain(std::iter::once("")) {
		if let Some(number) = line.strip_prefix("Sink #") {
			if seen_sink {
				push_pactl_sink(
					&mut devices,
					&sink_number,
					&name,
					&description,
					volume,
					muted,
					default_sink,
				);
			}
			sink_number = number.trim().to_string();
			name.clear();
			description.clear();
			volume = 0;
			muted = false;
			seen_sink = true;
			continue;
		}
		if !seen_sink {
			continue;
		}
		let trimmed = line.trim();
		if let Some(value) = trimmed.strip_prefix("Name:") {
			name = value.trim().to_string();
		} else if let Some(value) = trimmed.strip_prefix("Description:") {
			description = value.trim().to_string();
		} else if let Some(value) = trimmed.strip_prefix("Mute:") {
			muted = value.trim().eq_ignore_ascii_case("yes");
		} else if let Some(value) = trimmed.strip_prefix("Volume:") {
			volume = parse_pactl_volume(value).unwrap_or(0);
		}
	}
	devices
}

async fn command_stdout(program: &str, args: &[&str]) -> Result<String> {
	let output = TokioCommand::new(program).args(args).output().await?;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		if stderr.is_empty() {
			bail!("{program} exited with {}", output.status);
		}
		bail!("{program} failed: {stderr}");
	}
	Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn select_audio_backend() -> Box<dyn AudioBackend> {
	#[cfg(target_os = "linux")]
	{
		let wpctl = WpctlAudioBackend;
		if wpctl.available().await.is_ok() {
			return Box::new(wpctl);
		}

		let pactl = PactlAudioBackend;
		if pactl.available().await.is_ok() {
			return Box::new(pactl);
		}

		return Box::new(UnsupportedAudioBackend {
			message: String::from(
				"Audio control is not available. Install or start PipeWire/WirePlumber (wpctl) or PulseAudio (pactl).",
			),
		});
	}

	#[cfg(not(target_os = "linux"))]
	{
		Box::new(UnsupportedAudioBackend {
			message: format!(
				"Audio control is not supported on {} yet.",
				std::env::consts::OS
			),
		})
	}
}

pub(crate) async fn audio_capability() -> AudioCapability {
	select_audio_backend().await.capability()
}

pub(crate) async fn list_audio_devices() -> Result<Vec<AudioDevice>> {
	select_audio_backend().await.list_devices().await
}

pub(crate) async fn set_default_audio_device(device_id: String) -> Result<Vec<AudioDevice>> {
	select_audio_backend()
		.await
		.set_default_device(device_id)
		.await
}

pub(crate) async fn set_audio_muted(
	device_id: Option<String>,
	muted: bool,
) -> Result<Vec<AudioDevice>> {
	select_audio_backend()
		.await
		.set_muted(device_id, muted)
		.await
}

pub(crate) async fn set_audio_volume(
	device_id: Option<String>,
	volume: u8,
) -> Result<Vec<AudioDevice>> {
	select_audio_backend()
		.await
		.set_volume(device_id, volume)
		.await
}

#[async_trait]
impl AudioBackend for PactlAudioBackend {
	fn name(&self) -> &'static str {
		"pactl"
	}

	async fn available(&self) -> Result<()> {
		command_stdout("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])
			.await
			.map(|_| ())
	}

	async fn list_devices(&self) -> Result<Vec<AudioDevice>> {
		let default_sink = command_stdout("pactl", &["get-default-sink"])
			.await?
			.trim()
			.to_string();
		let output = command_stdout("pactl", &["list", "sinks"]).await?;
		let devices = parse_pactl_sinks(&output, &default_sink);
		if devices.is_empty() {
			bail!("No PulseAudio output devices found");
		}
		Ok(devices)
	}

	async fn set_default_device(&self, device_id: String) -> Result<Vec<AudioDevice>> {
		if device_id.trim().is_empty() {
			bail!("invalid audio device");
		}
		command_stdout("pactl", &["set-default-sink", &device_id]).await?;
		self.list_devices().await
	}

	async fn set_muted(&self, device_id: Option<String>, muted: bool) -> Result<Vec<AudioDevice>> {
		let target = audio_target_for_tool(&device_id, "@DEFAULT_SINK@");
		let muted = if muted { "1" } else { "0" };
		command_stdout("pactl", &["set-sink-mute", &target, muted]).await?;
		self.list_devices().await
	}

	async fn set_volume(&self, device_id: Option<String>, volume: u8) -> Result<Vec<AudioDevice>> {
		let percent = format!("{}%", volume.min(100));
		let target = audio_target_for_tool(&device_id, "@DEFAULT_SINK@");
		command_stdout("pactl", &["set-sink-volume", &target, &percent]).await?;
		self.list_devices().await
	}
}

#[async_trait]
impl AudioBackend for UnsupportedAudioBackend {
	fn name(&self) -> &'static str {
		"unsupported"
	}

	async fn available(&self) -> Result<()> {
		bail!("{}", self.message)
	}

	async fn list_devices(&self) -> Result<Vec<AudioDevice>> {
		bail!("{}", self.message)
	}

	async fn set_default_device(&self, _device_id: String) -> Result<Vec<AudioDevice>> {
		bail!("{}", self.message)
	}

	async fn set_muted(
		&self,
		_device_id: Option<String>,
		_muted: bool,
	) -> Result<Vec<AudioDevice>> {
		bail!("{}", self.message)
	}

	async fn set_volume(
		&self,
		_device_id: Option<String>,
		_volume: u8,
	) -> Result<Vec<AudioDevice>> {
		bail!("{}", self.message)
	}

	fn capability(&self) -> AudioCapability {
		AudioCapability {
			supported: false,
			backend: None,
			message: self.message.clone(),
		}
	}
}

#[async_trait]
impl AudioBackend for WpctlAudioBackend {
	fn name(&self) -> &'static str {
		"wpctl"
	}

	async fn available(&self) -> Result<()> {
		command_stdout("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])
			.await
			.map(|_| ())
	}

	async fn list_devices(&self) -> Result<Vec<AudioDevice>> {
		let output = command_stdout("wpctl", &["status"]).await?;
		let devices = parse_wpctl_sinks(&output);
		if devices.is_empty() {
			bail!("No PipeWire output devices found");
		}
		Ok(devices)
	}

	async fn set_default_device(&self, device_id: String) -> Result<Vec<AudioDevice>> {
		if device_id.trim().is_empty() || !device_id.chars().all(|ch| ch.is_ascii_digit()) {
			bail!("invalid audio device");
		}
		command_stdout("wpctl", &["set-default", &device_id]).await?;
		self.list_devices().await
	}

	async fn set_muted(&self, device_id: Option<String>, muted: bool) -> Result<Vec<AudioDevice>> {
		let target = audio_target_for_tool(&device_id, "@DEFAULT_AUDIO_SINK@");
		let muted = if muted { "1" } else { "0" };
		command_stdout("wpctl", &["set-mute", &target, muted]).await?;
		self.list_devices().await
	}

	async fn set_volume(&self, device_id: Option<String>, volume: u8) -> Result<Vec<AudioDevice>> {
		let percent = format!("{}%", volume.min(100));
		let target = audio_target_for_tool(&device_id, "@DEFAULT_AUDIO_SINK@");
		command_stdout("wpctl", &["set-volume", &target, &percent]).await?;
		self.list_devices().await
	}
}
