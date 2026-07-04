use crate::p2p::{
	MediaCapability, MediaFrame, MediaOutput, MediaSource, MediaSourceKind, MediaTransport,
};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

const DEFAULT_SCREEN_SOURCE_ID: &str = "screen:default";
const SCREEN_STREAM_MAX_HEIGHT: u32 = 540;
const SCREEN_STREAM_MAX_WIDTH: u32 = 960;
const SCREEN_STREAM_QUALITY: u8 = 60;
static SCREEN_CAPTURE_COMMAND: OnceLock<ScreenCaptureCommand> = OnceLock::new();

#[derive(Clone, Copy)]
enum ScreenCaptureCommand {
	Cosmic,
	Grim,
	Import,
}

#[async_trait]
trait WebcamBackend: Send + Sync {
	fn name(&self) -> &'static str;

	async fn available(&self) -> Result<()>;

	async fn list_sources(&self) -> Result<Vec<MediaSource>>;

	async fn capture_frame(&self, source_id: String) -> Result<MediaFrame>;
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

fn screen_source() -> MediaSource {
	MediaSource {
		id: DEFAULT_SCREEN_SOURCE_ID.to_string(),
		name: String::from("Default monitor"),
		kind: MediaSourceKind::Screen,
		live: true,
		outputs: vec![MediaOutput {
			transport: MediaTransport::Stream,
			mime: String::from("image/jpeg"),
			codec: Some(String::from("mjpeg")),
		}],
	}
}

fn validate_screen_id(source_id: &str) -> Result<()> {
	if source_id == DEFAULT_SCREEN_SOURCE_ID {
		Ok(())
	} else {
		bail!("invalid screen source");
	}
}

#[cfg(target_os = "linux")]
fn wayland_runtime_dir() -> PathBuf {
	std::env::var_os("XDG_RUNTIME_DIR")
		.map(PathBuf::from)
		.filter(|path| path.is_absolute())
		.unwrap_or_else(|| {
			let uid = unsafe { libc::geteuid() };
			PathBuf::from(format!("/run/user/{uid}"))
		})
}

#[cfg(target_os = "linux")]
fn detected_wayland_display(runtime_dir: &Path) -> Option<String> {
	use std::os::unix::fs::FileTypeExt;

	let mut displays = std::fs::read_dir(runtime_dir)
		.ok()?
		.filter_map(Result::ok)
		.filter_map(|entry| {
			let name = entry.file_name().to_string_lossy().to_string();
			if !name.starts_with("wayland-") || name.ends_with(".lock") {
				return None;
			}
			let file_type = entry.file_type().ok()?;
			file_type.is_socket().then_some(name)
		})
		.collect::<Vec<_>>();
	displays.sort();
	displays.into_iter().next()
}

#[cfg(target_os = "linux")]
fn configure_wayland_env(process: &mut Command) {
	let runtime_dir = wayland_runtime_dir();
	if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
		process.env("XDG_RUNTIME_DIR", &runtime_dir);
	}
	if std::env::var_os("WAYLAND_DISPLAY").is_none()
		&& let Some(display) = detected_wayland_display(&runtime_dir)
	{
		process.env("WAYLAND_DISPLAY", display);
	}
}

#[cfg(target_os = "linux")]
async fn cosmic_screen_available() -> bool {
	match timeout(
		Duration::from_secs(2),
		task::spawn_blocking(crate::cosmic_capture::available),
	)
	.await
	{
		Ok(Ok(available)) => available,
		Ok(Err(err)) => {
			log::warn!("COSMIC screen capture availability task failed: {err}");
			false
		}
		Err(_) => {
			log::warn!("timed out checking COSMIC screen capture availability");
			false
		}
	}
}

#[cfg(target_os = "linux")]
async fn capture_cosmic_screen_png() -> Result<Vec<u8>> {
	timeout(
		Duration::from_secs(8),
		task::spawn_blocking(crate::cosmic_capture::capture_png),
	)
	.await
	.map_err(|_| anyhow!("timed out capturing COSMIC screen frame"))?
	.map_err(|err| anyhow!("COSMIC screen capture task failed: {err}"))?
}

async fn capture_external_screen_png(command: ScreenCaptureCommand) -> Result<Vec<u8>> {
	let mut process = match command {
		ScreenCaptureCommand::Cosmic => unreachable!("COSMIC capture is handled above"),
		ScreenCaptureCommand::Grim => {
			let mut process = Command::new("grim");
			#[cfg(target_os = "linux")]
			configure_wayland_env(&mut process);
			process.arg("-");
			process
		}
		ScreenCaptureCommand::Import => {
			let mut process = Command::new("import");
			process.args(["-window", "root", "png:-"]);
			process
		}
	};
	let output = timeout(Duration::from_secs(8), process.output())
		.await
		.map_err(|_| anyhow!("timed out capturing screen frame"))??;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr);
		bail!("failed to capture screen frame: {}", stderr.trim());
	}
	if output.stdout.is_empty() {
		bail!("screen capture returned no image data");
	}
	Ok(output.stdout)
}

async fn probe_external_screen_capture_command(
	command: ScreenCaptureCommand,
) -> Result<ScreenCaptureCommand> {
	let probe = capture_external_screen_png(command).await?;
	screen_stream_frame_blocking(probe).await?;
	Ok(command)
}

async fn detect_screen_capture_command() -> Result<ScreenCaptureCommand> {
	if !cfg!(target_os = "linux") {
		bail!(
			"Screen viewing is not supported on {} yet.",
			std::env::consts::OS
		);
	}
	let mut errors = Vec::new();
	#[cfg(target_os = "linux")]
	if cosmic_screen_available().await {
		match capture_cosmic_screen_png().await {
			Ok(probe) => {
				screen_stream_frame_blocking(probe).await?;
				return Ok(ScreenCaptureCommand::Cosmic);
			}
			Err(err) => errors.push(format!("COSMIC: {err}")),
		}
	}
	if command_available("grim", "-h").await {
		match probe_external_screen_capture_command(ScreenCaptureCommand::Grim).await {
			Ok(command) => return Ok(command),
			Err(err) => errors.push(format!("grim: {err}")),
		}
	}
	if command_available("import", "-version").await {
		match probe_external_screen_capture_command(ScreenCaptureCommand::Import).await {
			Ok(command) => return Ok(command),
			Err(err) => errors.push(format!("ImageMagick import: {err}")),
		}
	}
	if errors.is_empty() {
		bail!(
			"Screen viewing needs a working COSMIC, grim, or ImageMagick import capture backend."
		);
	}
	bail!(
		"No screen capture backend produced a frame: {}",
		errors.join("; ")
	);
}

async fn screen_capture_command() -> Result<ScreenCaptureCommand> {
	if let Some(command) = SCREEN_CAPTURE_COMMAND.get() {
		return Ok(*command);
	}
	let command = detect_screen_capture_command().await?;
	let _ = SCREEN_CAPTURE_COMMAND.set(command);
	Ok(command)
}

async fn screen_available() -> Result<()> {
	screen_capture_command().await.map(|_| ())
}

async fn list_screen_sources() -> Result<Vec<MediaSource>> {
	screen_available().await?;
	Ok(vec![screen_source()])
}

fn screen_stream_frame(data: Vec<u8>) -> Result<MediaFrame> {
	let image = image::load_from_memory(&data)?;
	let image =
		if image.width() > SCREEN_STREAM_MAX_WIDTH || image.height() > SCREEN_STREAM_MAX_HEIGHT {
			image.resize(
				SCREEN_STREAM_MAX_WIDTH,
				SCREEN_STREAM_MAX_HEIGHT,
				image::imageops::FilterType::Triangle,
			)
		} else {
			image
		};
	let mut data = Vec::new();
	let mut encoder =
		image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, SCREEN_STREAM_QUALITY);
	encoder.encode_image(&image)?;
	if data.is_empty() {
		bail!("screen stream encoding returned no image data");
	}
	Ok(MediaFrame {
		mime: String::from("image/jpeg"),
		data,
	})
}

async fn screen_stream_frame_blocking(data: Vec<u8>) -> Result<MediaFrame> {
	timeout(
		Duration::from_secs(8),
		task::spawn_blocking(move || screen_stream_frame(data)),
	)
	.await
	.map_err(|_| anyhow!("timed out encoding screen frame"))?
	.map_err(|err| anyhow!("screen frame encoding task failed: {err}"))?
}

#[cfg(target_os = "linux")]
async fn capture_cosmic_screen_frame() -> Result<MediaFrame> {
	let data = capture_cosmic_screen_png().await?;
	screen_stream_frame_blocking(data).await
}

async fn capture_external_screen_frame(command: ScreenCaptureCommand) -> Result<MediaFrame> {
	let data = capture_external_screen_png(command).await?;
	screen_stream_frame_blocking(data).await
}

async fn capture_screen_frame(source_id: String) -> Result<MediaFrame> {
	validate_screen_id(&source_id)?;
	let command = screen_capture_command().await?;
	#[cfg(target_os = "linux")]
	if matches!(command, ScreenCaptureCommand::Cosmic) {
		return match capture_cosmic_screen_frame().await {
			Ok(frame) => Ok(frame),
			Err(cosmic_err) => {
				if command_available("grim", "-h").await {
					log::warn!("COSMIC screen capture failed, falling back to grim: {cosmic_err}");
					return capture_external_screen_frame(ScreenCaptureCommand::Grim).await;
				}
				if command_available("import", "-version").await {
					log::warn!(
						"COSMIC screen capture failed, falling back to ImageMagick import: {cosmic_err}"
					);
					return capture_external_screen_frame(ScreenCaptureCommand::Import).await;
				}
				Err(cosmic_err)
			}
		};
	}
	capture_external_screen_frame(command).await
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
const V4L2_CAPTURE_IDLE_TIMEOUT: StdDuration = StdDuration::from_secs(2);

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
	loop {
		let request = match requests.recv_timeout(V4L2_CAPTURE_IDLE_TIMEOUT) {
			Ok(request) => request,
			Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
		};
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
fn remove_v4l2_capture_worker(device_id: &str) {
	if let Some(workers) = V4L2_CAPTURE_WORKERS.get()
		&& let Ok(mut workers) = workers.lock()
	{
		workers.remove(device_id);
	}
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
			remove_v4l2_capture_worker(device_id);
			let retry_worker = v4l2_capture_worker(device_id)
				.map_err(|retry_err| anyhow!("{err}; retry failed: {retry_err}"))?;
			request_v4l2_frame(&retry_worker)
				.map_err(|retry_err| anyhow!("{err}; retry failed: {retry_err}"))
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
	let webcam_available = backend.available().await;
	let screen_available = screen_available().await;
	if webcam_available.is_ok() || screen_available.is_ok() {
		let mut backends = Vec::new();
		let mut details = Vec::new();
		if webcam_available.is_ok() {
			backends.push(backend.name().to_string());
			details.push(String::from("Webcam capture is available."));
		} else if let Err(err) = &webcam_available {
			details.push(format!("Webcam capture unavailable: {err}"));
		}
		if screen_available.is_ok() {
			backends.push(String::from("screen-capture"));
			details.push(String::from("Screen capture is available."));
		} else if let Err(err) = &screen_available {
			details.push(format!("Screen capture unavailable: {err}"));
		}
		return MediaCapability {
			supported: true,
			backend: Some(backends.join(", ")),
			message: details.join(" "),
		};
	}
	let webcam_error = webcam_available
		.err()
		.map(|err| err.to_string())
		.unwrap_or_else(|| String::from("webcam unavailable"));
	let screen_error = screen_available
		.err()
		.map(|err| err.to_string())
		.unwrap_or_else(|| String::from("screen unavailable"));
	MediaCapability {
		supported: false,
		backend: Some(backend.name().to_string()),
		message: format!("{webcam_error} {screen_error}"),
	}
}

pub(crate) async fn list_media_sources() -> Result<Vec<MediaSource>> {
	let backend = select_webcam_backend().await;
	let mut sources = Vec::new();
	if backend.available().await.is_ok() {
		sources.extend(backend.list_sources().await?);
	}
	if screen_available().await.is_ok() {
		sources.extend(list_screen_sources().await?);
	}
	if sources.is_empty() {
		bail!("No live media sources are available.");
	}
	Ok(sources)
}

pub(crate) async fn capture_media_frame(source_id: String) -> Result<MediaFrame> {
	if source_id == DEFAULT_SCREEN_SOURCE_ID {
		return capture_screen_frame(source_id).await;
	}
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
}

#[cfg(test)]
mod tests {
	use super::*;
	use image::{ImageBuffer, ImageEncoder, Rgba};

	#[test]
	fn screen_stream_frame_encodes_resized_jpeg() {
		let image =
			ImageBuffer::<Rgba<u8>, Vec<u8>>::from_pixel(2000, 1000, Rgba([0, 128, 255, 255]));
		let mut png = Vec::new();
		image::codecs::png::PngEncoder::new(&mut png)
			.write_image(image.as_raw(), 2000, 1000, image::ColorType::Rgba8.into())
			.unwrap();

		let frame = screen_stream_frame(png).unwrap();
		let decoded = image::load_from_memory(&frame.data).unwrap();

		assert_eq!(frame.mime, "image/jpeg");
		assert!(decoded.width() <= SCREEN_STREAM_MAX_WIDTH);
		assert!(decoded.height() <= SCREEN_STREAM_MAX_HEIGHT);
	}
}
