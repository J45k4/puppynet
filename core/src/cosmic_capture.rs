use anyhow::{Context, Result, anyhow, bail};
use cosmic_client_toolkit::screencopy::{
	CaptureFrame, CaptureOptions, CaptureSession, CaptureSource, FailureReason, Formats, Frame,
	ScreencopyFrameData, ScreencopyFrameDataExt, ScreencopyHandler, ScreencopySessionData,
	ScreencopySessionDataExt, ScreencopyState,
};
use cosmic_client_toolkit::sctk::output::{OutputHandler, OutputState};
use cosmic_client_toolkit::workspace::{WorkspaceHandler, WorkspaceState};
use cosmic_client_toolkit::{sctk, wayland_client, wayland_protocols};
use image::ImageEncoder;
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::shm::{Shm, ShmHandler, raw::RawPool};
use std::io::Cursor;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_buffer, wl_output, wl_shm};
use wayland_client::{Connection, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::State as WorkspaceStateFlags;

struct SessionData {
	session_data: ScreencopySessionData,
}

impl ScreencopySessionDataExt for SessionData {
	fn screencopy_session_data(&self) -> &ScreencopySessionData {
		&self.session_data
	}
}

struct FrameData {
	frame_data: ScreencopyFrameData,
	pool: Mutex<RawPool>,
	size: (u32, u32),
}

impl ScreencopyFrameDataExt for FrameData {
	fn screencopy_frame_data(&self) -> &ScreencopyFrameData {
		&self.frame_data
	}
}

struct AppData {
	registry_state: RegistryState,
	shm_state: Shm,
	output_state: OutputState,
	screencopy_state: ScreencopyState,
	workspace_state: WorkspaceState,
	workspace_done: bool,
	capture_result: Option<Result<Vec<u8>>>,
}

impl ProvidesRegistryState for AppData {
	fn registry(&mut self) -> &mut RegistryState {
		&mut self.registry_state
	}

	sctk::registry_handlers!();
}

impl OutputHandler for AppData {
	fn output_state(&mut self) -> &mut OutputState {
		&mut self.output_state
	}

	fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

	fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

	fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for AppData {
	fn shm_state(&mut self) -> &mut Shm {
		&mut self.shm_state
	}
}

impl WorkspaceHandler for AppData {
	fn workspace_state(&mut self) -> &mut WorkspaceState {
		&mut self.workspace_state
	}

	fn done(&mut self) {
		self.workspace_done = true;
	}
}

fn encode_png(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
	let mut data = Vec::new();
	let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut data));
	encoder.write_image(bytes, width, height, image::ColorType::Rgba8.into())?;
	Ok(data)
}

fn runtime_dir() -> Option<PathBuf> {
	std::env::var_os("XDG_RUNTIME_DIR")
		.map(PathBuf::from)
		.filter(|path| path.is_absolute())
		.or_else(|| {
			let uid = unsafe { libc::geteuid() };
			Some(PathBuf::from(format!("/run/user/{uid}")))
		})
}

fn wayland_socket_from_env(runtime_dir: Option<&Path>) -> Option<PathBuf> {
	let socket_name = std::env::var_os("WAYLAND_DISPLAY")?;
	let socket_path = PathBuf::from(socket_name);
	if socket_path.is_absolute() {
		Some(socket_path)
	} else {
		runtime_dir.map(|runtime_dir| runtime_dir.join(socket_path))
	}
}

fn available_wayland_sockets(runtime_dir: &Path) -> Vec<PathBuf> {
	let mut sockets = std::fs::read_dir(runtime_dir)
		.ok()
		.into_iter()
		.flat_map(|entries| entries.filter_map(Result::ok))
		.filter_map(|entry| {
			let name = entry.file_name();
			let name = name.to_string_lossy();
			if !name.starts_with("wayland-") || name.ends_with(".lock") {
				return None;
			}
			let file_type = entry.file_type().ok()?;
			if file_type.is_socket() {
				Some(entry.path())
			} else {
				None
			}
		})
		.collect::<Vec<_>>();
	sockets.sort();
	sockets
}

fn wayland_socket_candidates() -> Vec<PathBuf> {
	let runtime_dir = runtime_dir();
	let mut candidates = Vec::new();
	if let Some(socket) = wayland_socket_from_env(runtime_dir.as_deref()) {
		candidates.push(socket);
	}
	if let Some(runtime_dir) = runtime_dir {
		candidates.extend(available_wayland_sockets(&runtime_dir));
	}
	candidates.dedup();
	candidates
}

fn connect_to_wayland() -> Result<Connection> {
	let mut errors = Vec::new();
	for socket in wayland_socket_candidates() {
		match UnixStream::connect(&socket)
			.and_then(|stream| Connection::from_socket(stream).map_err(std::io::Error::other))
		{
			Ok(conn) => return Ok(conn),
			Err(err) => errors.push(format!("{}: {err}", socket.display())),
		}
	}
	if errors.is_empty() {
		bail!("failed to connect to Wayland compositor: no Wayland socket was found");
	}
	bail!(
		"failed to connect to Wayland compositor: {}",
		errors.join("; ")
	);
}

impl ScreencopyHandler for AppData {
	fn screencopy_state(&mut self) -> &mut ScreencopyState {
		&mut self.screencopy_state
	}

	fn init_done(
		&mut self,
		_: &Connection,
		qh: &QueueHandle<Self>,
		session: &CaptureSession,
		formats: &Formats,
	) {
		let (width, height) = formats.buffer_size;
		let size = width as usize * height as usize * 4;
		let Ok(mut pool) = RawPool::new(size, &self.shm_state) else {
			self.capture_result = Some(Err(anyhow!("failed to create Wayland shared memory pool")));
			return;
		};
		let buffer = pool.create_buffer(
			0,
			width as i32,
			height as i32,
			width as i32 * 4,
			wl_shm::Format::Abgr8888,
			(),
			qh,
		);
		session.capture(
			&buffer,
			&[],
			qh,
			FrameData {
				frame_data: ScreencopyFrameData::default(),
				pool: Mutex::new(pool),
				size: formats.buffer_size,
			},
		);
	}

	fn stopped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &CaptureSession) {
		self.capture_result = Some(Err(anyhow!("COSMIC screen capture session stopped")));
	}

	fn ready(
		&mut self,
		_: &Connection,
		_: &QueueHandle<Self>,
		capture_frame: &CaptureFrame,
		_: Frame,
	) {
		let result = (|| {
			let data = capture_frame
				.data::<FrameData>()
				.ok_or_else(|| anyhow!("missing COSMIC capture frame data"))?;
			let mut pool = data
				.pool
				.lock()
				.map_err(|_| anyhow!("COSMIC capture buffer lock was poisoned"))?;
			let bytes = pool.mmap();
			encode_png(&bytes, data.size.0, data.size.1)
		})();
		self.capture_result = Some(result);
	}

	fn failed(
		&mut self,
		_: &Connection,
		_: &QueueHandle<Self>,
		_: &CaptureFrame,
		reason: WEnum<FailureReason>,
	) {
		self.capture_result = Some(Err(anyhow!("COSMIC screen capture failed: {reason:?}")));
	}
}

fn new_app_data(conn: &Connection) -> Result<(AppData, wayland_client::EventQueue<AppData>)> {
	let (globals, event_queue) = registry_queue_init(conn)?;
	let qh = event_queue.handle();
	let registry_state = RegistryState::new(&globals);
	Ok((
		AppData {
			shm_state: Shm::bind(&globals, &qh).context("COSMIC capture needs wl_shm")?,
			output_state: OutputState::new(&globals, &qh),
			screencopy_state: ScreencopyState::new(&globals, &qh),
			workspace_state: WorkspaceState::new(&registry_state, &qh),
			registry_state,
			workspace_done: false,
			capture_result: None,
		},
		event_queue,
	))
}

fn output_source(data: &AppData) -> Option<CaptureSource> {
	data.output_state
		.outputs()
		.next()
		.map(CaptureSource::Output)
}

fn active_workspace(data: &AppData) -> Result<CaptureSource> {
	let workspace = data
		.workspace_state
		.workspaces()
		.find(|workspace| workspace.state.contains(WorkspaceStateFlags::Active))
		.or_else(|| data.workspace_state.workspaces().next())
		.ok_or_else(|| anyhow!("COSMIC did not report any workspaces"))?;
	Ok(CaptureSource::Workspace(workspace.handle.clone()))
}

fn capture_source(data: &AppData) -> Result<CaptureSource> {
	output_source(data).map_or_else(|| active_workspace(data), Ok)
}

fn wait_for_workspaces(
	event_queue: &mut wayland_client::EventQueue<AppData>,
	data: &mut AppData,
) -> Result<()> {
	while !data.workspace_done {
		event_queue.blocking_dispatch(data)?;
	}
	Ok(())
}

fn wait_for_capture_sources(
	event_queue: &mut wayland_client::EventQueue<AppData>,
	data: &mut AppData,
) -> Result<()> {
	event_queue.roundtrip(data)?;
	if output_source(data).is_some() || data.workspace_done {
		return Ok(());
	}
	wait_for_workspaces(event_queue, data)
}

fn wait_for_capture(
	event_queue: &mut wayland_client::EventQueue<AppData>,
	data: &mut AppData,
) -> Result<Vec<u8>> {
	while data.capture_result.is_none() {
		event_queue.blocking_dispatch(data)?;
	}
	data.capture_result
		.take()
		.ok_or_else(|| anyhow!("COSMIC capture produced no result"))?
}

pub(crate) fn available() -> bool {
	let Ok(conn) = connect_to_wayland() else {
		return false;
	};
	let Ok((mut data, mut event_queue)) = new_app_data(&conn) else {
		return false;
	};
	wait_for_capture_sources(&mut event_queue, &mut data).is_ok()
		&& (output_source(&data).is_some() || data.workspace_state.workspaces().next().is_some())
}

pub(crate) fn capture_png() -> Result<Vec<u8>> {
	let conn = connect_to_wayland()?;
	let (mut data, mut event_queue) = new_app_data(&conn)?;
	wait_for_capture_sources(&mut event_queue, &mut data)?;
	let source = capture_source(&data)?;
	let qh = event_queue.handle();
	let _session = data
		.screencopy_state
		.capturer()
		.create_session(
			&source,
			CaptureOptions::empty(),
			&qh,
			SessionData {
				session_data: ScreencopySessionData::default(),
			},
		)
		.map_err(|err| anyhow!("failed to create COSMIC screen capture session: {err}"))?;
	let data = wait_for_capture(&mut event_queue, &mut data)?;
	if data.is_empty() {
		bail!("COSMIC screen capture returned no image data");
	}
	Ok(data)
}

sctk::delegate_registry!(AppData);
sctk::delegate_output!(AppData);
sctk::delegate_shm!(AppData);
cosmic_client_toolkit::delegate_workspace!(AppData);
cosmic_client_toolkit::delegate_screencopy!(AppData);
delegate_noop!(AppData: ignore wl_buffer::WlBuffer);

#[cfg(test)]
mod tests {
	#[test]
	#[ignore]
	fn capture_png_smoke() {
		let data = super::capture_png().expect("COSMIC capture should produce an image");
		assert!(!data.is_empty());
		eprintln!("captured {} bytes", data.len());
	}
}
