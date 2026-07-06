use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PeerWebcamsController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PeerWebcamsController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}

	fn peer_id(&self) -> String {
		self.ctx.param("peer_id").unwrap_or_default()
	}
}

#[wgui::wgui_controller]
impl PeerWebcamsController {
	pub fn state(&self) -> UiViewState {
		self.core().peer_webcams_state(self.peer_id())
	}

	pub fn title(&self) -> String {
		String::from("Device Webcams - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_webcams(&mut self) {
		self.core().refresh_webcams();
	}

	pub fn view_webcam(&mut self, idx: u32) {
		self.core().view_webcam(idx);
	}
}

#[async_trait]
impl Component for PeerWebcamsController {
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
