use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PeerController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PeerController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}

	fn peer_id(&self) -> String {
		self.ctx.param("peer_id").unwrap_or_default()
	}
}

#[wgui::wgui_controller]
impl PeerController {
	pub fn state(&self) -> UiViewState {
		self.core().peer_state(self.peer_id())
	}

	pub fn title(&self) -> String {
		String::from("Device - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn peer_back(&mut self) {
		self.core().peer_back();
	}

	pub fn edit_shell_input(&mut self, value: String) {
		self.core().edit_shell_input(value);
	}

	pub fn start_shell(&mut self) {
		self.core().start_shell();
	}

	pub fn send_shell_input(&mut self) {
		self.core().send_shell_input();
	}

	pub fn refresh_audio(&mut self) {
		self.core().refresh_audio();
	}

	pub fn set_audio_volume(&mut self, value: i32) {
		self.core().set_audio_volume(value);
	}

	pub fn toggle_audio_mute(&mut self) {
		self.core().toggle_audio_mute();
	}

	pub fn select_audio_device(&mut self, value: String) {
		self.core().select_audio_device(value);
	}

	pub fn edit_update_version(&mut self, value: String) {
		self.core().edit_update_version(value);
	}

	pub fn start_peer_update(&mut self) {
		self.core().start_peer_update();
	}
}

impl PeerController {
	pub fn poll_peer_update(&mut self) {
		self.core().poll_peer_update();
	}
}

#[async_trait]
impl Component for PeerController {
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
