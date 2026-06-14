use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PeerFilesController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PeerFilesController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}

	fn decode_path(path: String) -> String {
		if !path.contains('%') && !path.contains('+') {
			return path;
		}
		url::form_urlencoded::parse(format!("path={path}").as_bytes())
			.find_map(|(key, value)| {
				if key == "path" {
					Some(value.into_owned())
				} else {
					None
				}
			})
			.unwrap_or(path)
	}

	fn path(&self) -> String {
		Self::decode_path(self.ctx.query("path").unwrap_or_else(|| String::from("/")))
	}

	fn peer_id(&self) -> String {
		self.ctx.param("peer_id").unwrap_or_default()
	}
}

#[wgui_controller]
impl PeerFilesController {
	pub fn state(&self) -> UiViewState {
		self.core().peer_files_state(self.peer_id(), self.path())
	}

	pub fn title(&self) -> String {
		String::from("Peer Files - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_peer_files(&mut self) {
		self.core().refresh_peer_files();
	}

	pub fn preview_peer_file(&mut self, idx: u32) {
		self.core().preview_peer_file(idx);
	}

	pub fn close_file_preview_modal(&mut self) {
		self.core().close_file_preview_modal();
	}

	pub fn edit_file_preview_path(&mut self, value: String) {
		self.core().edit_file_preview_path(value);
	}

	pub fn edit_file_preview_peer(&mut self, value: String) {
		self.core().edit_file_preview_peer(value);
	}

	pub fn load_file_preview(&mut self) {
		self.core().load_file_preview();
	}
}

#[async_trait]
impl Component for PeerFilesController {
	type Context = UiContext;
	type Db = ();
	type Model = UiViewState;

	async fn mount(
		ctx: Arc<Ctx<Self::Context, Self::Db>>,
		_route: RouteContext,
	) -> MountResult<Self> {
		if let Some(result) = super::redirect_unauthenticated(&ctx) {
			return result;
		}
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
