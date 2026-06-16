use crate::p2p::{AudioCapability, AudioDevice, AudioDeviceKind};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use tokio::process::Command as TokioCommand;

#[async_trait]
trait AudioBackend: Send + Sync {
	fn name(&self) -> &'static str;
	async fn available(&self) -> Result<()>;
	async fn list_devices(&self) -> Result<Vec<AudioDevice>>;
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
	let value = token.trim_end_matches('%').parse::<u16>().ok()?;
	Some(value.min(100) as u8)
}

fn parse_pactl_volume(output: &str) -> Option<u8> {
	output.split_whitespace().find_map(parse_percent_token)
}

fn parse_wpctl_volume(output: &str) -> Option<(u8, bool)> {
	let volume = output.split_whitespace().find_map(|token| {
		token
			.parse::<f32>()
			.ok()
			.map(|value| (value * 100.0).round().clamp(0.0, 100.0) as u8)
	})?;
	Some((volume, output.contains("[MUTED]")))
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
		let volume_output = command_stdout("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]).await?;
		let mute_output = command_stdout("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]).await?;
		let volume = parse_pactl_volume(&volume_output).unwrap_or(0);
		let muted = mute_output.to_lowercase().contains("yes");
		Ok(vec![AudioDevice {
			id: String::from("@DEFAULT_SINK@"),
			name: String::from("Default output"),
			description: String::from("PulseAudio default output"),
			kind: AudioDeviceKind::Sink,
			volume,
			muted,
			is_default: true,
		}])
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
		let output = command_stdout("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).await?;
		let (volume, muted) = parse_wpctl_volume(&output)
			.ok_or_else(|| anyhow!("failed to parse wpctl volume output"))?;
		Ok(vec![AudioDevice {
			id: String::from("@DEFAULT_AUDIO_SINK@"),
			name: String::from("Default output"),
			description: String::from("PipeWire default output"),
			kind: AudioDeviceKind::Sink,
			volume,
			muted,
			is_default: true,
		}])
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
