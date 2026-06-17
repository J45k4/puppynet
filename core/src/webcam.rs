use crate::p2p::{
	MediaCapability, MediaFrame, MediaOutput, MediaSource, MediaSourceKind, MediaTransport,
};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::Duration as StdDuration;
use tokio::process::Command;
use tokio::task;
use tokio::time::{Duration, timeout};

#[cfg(target_os = "linux")]
use v4l::buffer::Type;
#[cfg(target_os = "linux")]
use v4l::io::traits::CaptureStream;
#[cfg(target_os = "linux")]
use v4l::prelude::*;
#[cfg(target_os = "linux")]
use v4l::video::Capture;
#[cfg(target_os = "linux")]
use v4l::{FourCC, context};

#[async_trait]
trait WebcamBackend: Send + Sync {
	fn name(&self) -> &'static str;

	async fn available(&self) -> Result<()>;

	async fn list_sources(&self) -> Result<Vec<MediaSource>>;

	async fn capture_frame(&self, source_id: String) -> Result<MediaFrame>;

	fn capability(&self) -> MediaCapability {
		MediaCapability {
			supported: true,
			backend: Some(self.name().to_string()),
			message: format!("Live media sources are available through {}.", self.name()),
		}
	}
}

struct WebcamDevice {
	id: String,
	name: String,
}

fn unsupported(message: impl Into<String>) -> Arc<dyn WebcamBackend> {
	Arc::new(UnsupportedWebcamBackend {
		message: message.into(),
	})
}

async fn command_available(command: &str, arg: &str) -> bool {
	Command::new(command)
		.arg(arg)
		.output()
		.await
		.map(|output| output.status.success())
		.unwrap_or(false)
}

fn fallback_video_devices() -> Vec<WebcamDevice> {
	let mut devices = std::fs::read_dir("/dev")
		.ok()
		.into_iter()
		.flat_map(|entries| entries.filter_map(Result::ok))
		.filter_map(|entry| {
			let name = entry.file_name().to_string_lossy().to_string();
			if !name.starts_with("video") {
				return None;
			}
			let id = format!("/dev/{name}");
			Some(WebcamDevice { id, name })
		})
		.collect::<Vec<_>>();
	devices.sort_by(|a, b| a.id.cmp(&b.id));
	devices
}

fn webcam_source(device: WebcamDevice) -> MediaSource {
	MediaSource {
		id: device.id,
		name: device.name,
		kind: MediaSourceKind::Webcam,
		live: true,
		outputs: vec![MediaOutput {
			transport: MediaTransport::Stream,
			mime: String::from("image/jpeg"),
			codec: Some(String::from("mjpeg")),
		}],
	}
}

fn webcam_sources(devices: Vec<WebcamDevice>) -> Vec<MediaSource> {
	devices.into_iter().map(webcam_source).collect()
}

#[cfg(target_os = "linux")]
fn v4l2_devices() -> Vec<WebcamDevice> {
	let mut devices = context::enum_devices()
		.into_iter()
		.map(|node| {
			let id = node.path().to_string_lossy().to_string();
			let name = node.name().unwrap_or_else(|| id.clone());
			WebcamDevice { id, name }
		})
		.collect::<Vec<_>>();
	devices.sort_by(|a, b| a.id.cmp(&b.id));
	if devices.is_empty() {
		fallback_video_devices()
	} else {
		devices
	}
}

#[cfg(target_os = "linux")]
fn device_supports_mjpeg(device_id: &str) -> bool {
	let Ok(device) = Device::with_path(device_id) else {
		return false;
	};
	device
		.enum_formats()
		.map(|formats| {
			formats
				.iter()
				.any(|format| format.fourcc == FourCC::new(b"MJPG"))
		})
		.unwrap_or(false)
}

#[cfg(target_os = "linux")]
static V4L2_CAPTURE_WORKERS: OnceLock<Mutex<HashMap<String, Arc<V4l2CaptureWorker>>>> =
	OnceLock::new();

#[cfg(target_os = "linux")]
struct V4l2FrameRequest {
	tx: mpsc::Sender<Result<Vec<u8>, String>>,
}

#[cfg(target_os = "linux")]
struct V4l2CaptureWorker {
	tx: mpsc::Sender<V4l2FrameRequest>,
}

#[cfg(target_os = "linux")]
fn open_v4l2_mjpeg_stream(device_id: &str) -> Result<(Device, v4l::Format)> {
	let device = Device::with_path(device_id)?;
	let mut format = device.format()?;
	format.fourcc = FourCC::new(b"MJPG");
	let actual = device.set_format(&format)?;
	if actual.fourcc != FourCC::new(b"MJPG") {
		bail!(
			"webcam does not provide MJPEG frames directly; selected format was {}",
			actual.fourcc
		);
	}
	Ok((device, actual))
}

#[cfg(target_os = "linux")]
fn capture_v4l2_stream_frame(stream: &mut MmapStream<'_>) -> Result<Vec<u8>> {
	let (buffer, _) = stream.next()?;
	if buffer.is_empty() {
		bail!("webcam capture returned no image data");
	}
	Ok(buffer.to_vec())
}

#[cfg(target_os = "linux")]
fn run_v4l2_capture_worker(
	device_id: String,
	requests: mpsc::Receiver<V4l2FrameRequest>,
	ready: mpsc::Sender<Result<(), String>>,
) {
	let Ok((device, _format)) = open_v4l2_mjpeg_stream(&device_id) else {
		let _ = ready.send(Err(format!("failed to open webcam stream for {device_id}")));
		return;
	};
	let Ok(mut stream) = MmapStream::with_buffers(&device, Type::VideoCapture, 4) else {
		let _ = ready.send(Err(format!(
			"failed to allocate webcam capture buffers for {device_id}"
		)));
		return;
	};
	let _ = capture_v4l2_stream_frame(&mut stream);
	let _ = ready.send(Ok(()));
	for request in requests {
		let result = capture_v4l2_stream_frame(&mut stream).map_err(|err| err.to_string());
		let should_stop = result.is_err();
		let _ = request.tx.send(result);
		if should_stop {
			break;
		}
	}
}

#[cfg(target_os = "linux")]
fn start_v4l2_capture_worker(device_id: String) -> Result<Arc<V4l2CaptureWorker>> {
	let (request_tx, request_rx) = mpsc::channel();
	let (ready_tx, ready_rx) = mpsc::channel();
	thread::Builder::new()
		.name(format!("puppynet-v4l2-{}", device_id.replace('/', "_")))
		.spawn(move || run_v4l2_capture_worker(device_id, request_rx, ready_tx))?;
	match ready_rx.recv_timeout(StdDuration::from_secs(8)) {
		Ok(Ok(())) => Ok(Arc::new(V4l2CaptureWorker { tx: request_tx })),
		Ok(Err(err)) => bail!("{err}"),
		Err(err) => bail!("timed out opening webcam stream: {err}"),
	}
}

#[cfg(target_os = "linux")]
fn v4l2_capture_worker(device_id: &str) -> Result<Arc<V4l2CaptureWorker>> {
	let workers = V4L2_CAPTURE_WORKERS.get_or_init(|| Mutex::new(HashMap::new()));
	let mut workers = workers
		.lock()
		.map_err(|_| anyhow!("webcam capture worker registry was poisoned"))?;
	if let Some(worker) = workers.get(device_id) {
		return Ok(Arc::clone(worker));
	}
	let worker = start_v4l2_capture_worker(device_id.to_string())?;
	workers.insert(device_id.to_string(), Arc::clone(&worker));
	Ok(worker)
}

#[cfg(target_os = "linux")]
fn request_v4l2_frame(worker: &V4l2CaptureWorker) -> Result<Vec<u8>> {
	let (tx, rx) = mpsc::channel();
	worker
		.tx
		.send(V4l2FrameRequest { tx })
		.map_err(|err| anyhow!("webcam capture worker stopped: {err}"))?;
	match rx.recv_timeout(StdDuration::from_secs(8)) {
		Ok(Ok(data)) => Ok(data),
		Ok(Err(err)) => bail!("{err}"),
		Err(err) => bail!("timed out capturing webcam frame: {err}"),
	}
}

#[cfg(target_os = "linux")]
fn capture_v4l2_mjpeg(device_id: &str) -> Result<Vec<u8>> {
	let worker = v4l2_capture_worker(device_id)?;
	match request_v4l2_frame(&worker) {
		Ok(data) => Ok(data),
		Err(err) => {
			if let Some(workers) = V4L2_CAPTURE_WORKERS.get()
				&& let Ok(mut workers) = workers.lock()
			{
				workers.remove(device_id);
			}
			Err(err)
		}
	}
}

fn parse_v4l2_devices(output: &str) -> Vec<WebcamDevice> {
	let mut devices = Vec::new();
	let mut current_name = String::new();
	for line in output.lines() {
		if line.trim().is_empty() {
			continue;
		}
		if line.starts_with('\t') || line.starts_with("    ") {
			let id = line.trim();
			if id.starts_with("/dev/video") {
				devices.push(WebcamDevice {
					id: id.to_string(),
					name: if current_name.is_empty() {
						id.to_string()
					} else {
						current_name.clone()
					},
				});
			}
			continue;
		}
		current_name = line.trim_end_matches(':').to_string();
	}
	devices
}

fn validate_device_id(device_id: &str) -> Result<()> {
	if !device_id.starts_with("/dev/video") {
		bail!("invalid webcam device");
	}
	if device_id.contains("..") || device_id.contains('\0') {
		bail!("invalid webcam device");
	}
	if !Path::new(device_id).exists() {
		bail!("webcam device does not exist");
	}
	Ok(())
}

async fn select_webcam_backend() -> Arc<dyn WebcamBackend> {
	if !cfg!(target_os = "linux") {
		return unsupported(format!(
			"Webcam viewing is not supported on {} yet.",
			std::env::consts::OS
		));
	}
	#[cfg(target_os = "linux")]
	{
		let direct = LinuxV4l2Backend;
		if direct.available().await.is_ok() {
			return Arc::new(direct);
		}
	}
	if command_available("ffmpeg", "-version").await {
		return Arc::new(FfmpegWebcamBackend);
	}
	unsupported(
		"Webcam viewing needs a camera that supports MJPEG through V4L2, or ffmpeg as fallback.",
	)
}

pub(crate) async fn media_capability() -> MediaCapability {
	let backend = select_webcam_backend().await;
	match backend.available().await {
		Ok(()) => backend.capability(),
		Err(err) => MediaCapability {
			supported: false,
			backend: Some(backend.name().to_string()),
			message: err.to_string(),
		},
	}
}

pub(crate) async fn list_media_sources() -> Result<Vec<MediaSource>> {
	let backend = select_webcam_backend().await;
	backend.available().await?;
	backend.list_sources().await
}

pub(crate) async fn capture_media_frame(source_id: String) -> Result<MediaFrame> {
	let backend = select_webcam_backend().await;
	backend.available().await?;
	backend.capture_frame(source_id).await
}

#[cfg(target_os = "linux")]
struct LinuxV4l2Backend;

#[cfg(target_os = "linux")]
#[async_trait]
impl WebcamBackend for LinuxV4l2Backend {
	fn name(&self) -> &'static str {
		"linux-v4l2"
	}

	async fn available(&self) -> Result<()> {
		let devices = v4l2_devices();
		if devices.is_empty() {
			bail!("No webcam devices were found on this Linux peer.");
		}
		if devices
			.iter()
			.any(|device| device_supports_mjpeg(&device.id))
		{
			return Ok(());
		}
		bail!("No webcam devices support direct MJPEG capture through V4L2.");
	}

	async fn list_sources(&self) -> Result<Vec<MediaSource>> {
		Ok(webcam_sources(v4l2_devices()))
	}

	async fn capture_frame(&self, source_id: String) -> Result<MediaFrame> {
		validate_device_id(&source_id)?;
		let data = timeout(
			Duration::from_secs(8),
			task::spawn_blocking(move || capture_v4l2_mjpeg(&source_id)),
		)
		.await
		.map_err(|_| anyhow!("timed out capturing webcam frame"))???;
		Ok(MediaFrame {
			mime: String::from("image/jpeg"),
			data,
		})
	}
}

struct FfmpegWebcamBackend;

#[async_trait]
impl WebcamBackend for FfmpegWebcamBackend {
	fn name(&self) -> &'static str {
		"ffmpeg-v4l2"
	}

	async fn available(&self) -> Result<()> {
		if fallback_video_devices().is_empty() {
			bail!("No webcam devices were found on this Linux peer.");
		}
		if !command_available("ffmpeg", "-version").await {
			bail!("Webcam viewing needs ffmpeg on this Linux peer.");
		}
		Ok(())
	}

	async fn list_sources(&self) -> Result<Vec<MediaSource>> {
		let devices = match Command::new("v4l2-ctl")
			.arg("--list-devices")
			.output()
			.await
		{
			Ok(output) if output.status.success() => {
				parse_v4l2_devices(&String::from_utf8_lossy(&output.stdout))
			}
			_ => fallback_video_devices(),
		};
		Ok(webcam_sources(devices))
	}

	async fn capture_frame(&self, source_id: String) -> Result<MediaFrame> {
		validate_device_id(&source_id)?;
		let output = timeout(
			Duration::from_secs(8),
			Command::new("ffmpeg")
				.args([
					"-hide_banner",
					"-loglevel",
					"error",
					"-f",
					"v4l2",
					"-i",
					&source_id,
					"-frames:v",
					"1",
					"-f",
					"image2pipe",
					"-vcodec",
					"mjpeg",
					"pipe:1",
				])
				.output(),
		)
		.await
		.map_err(|_| anyhow!("timed out capturing webcam frame"))??;
		if !output.status.success() {
			let stderr = String::from_utf8_lossy(&output.stderr);
			bail!("failed to capture webcam frame: {}", stderr.trim());
		}
		if output.stdout.is_empty() {
			bail!("webcam capture returned no image data");
		}
		Ok(MediaFrame {
			mime: String::from("image/jpeg"),
			data: output.stdout,
		})
	}
}

struct UnsupportedWebcamBackend {
	message: String,
}

#[async_trait]
impl WebcamBackend for UnsupportedWebcamBackend {
	fn name(&self) -> &'static str {
		"unsupported"
	}

	async fn available(&self) -> Result<()> {
		bail!("{}", self.message)
	}

	async fn list_sources(&self) -> Result<Vec<MediaSource>> {
		bail!("{}", self.message)
	}

	async fn capture_frame(&self, _source_id: String) -> Result<MediaFrame> {
		bail!("{}", self.message)
	}

	fn capability(&self) -> MediaCapability {
		MediaCapability {
			supported: false,
			backend: None,
			message: self.message.clone(),
		}
	}
}
