use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PeerControlController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PeerControlController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}

	fn peer_id(&self) -> String {
		self.ctx.param("peer_id").unwrap_or_default()
	}
}

#[wgui::wgui_controller]
impl PeerControlController {
	pub fn state(&self) -> UiViewState {
		self.core().peer_control_state(self.peer_id())
	}

	pub fn title(&self) -> String {
		String::from("Device Control - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn move_peer_mouse(&mut self, payload: wgui::serde_json::Value) {
		self.core().move_peer_mouse(payload);
	}

	pub fn scroll_peer_mouse(&mut self, payload: wgui::serde_json::Value) {
		self.core().scroll_peer_mouse(payload);
	}

	pub fn click_peer_mouse(&mut self, payload: wgui::serde_json::Value) {
		self.core().click_peer_mouse(payload);
	}

	pub fn press_peer_mouse(&mut self, payload: wgui::serde_json::Value) {
		self.core().press_peer_mouse(payload);
	}

	pub fn release_peer_mouse(&mut self, payload: wgui::serde_json::Value) {
		self.core().release_peer_mouse(payload);
	}

	pub fn toggle_monitor_stream(&mut self) {
		self.core().toggle_monitor_stream();
	}

	pub fn edit_control_text(&mut self, value: String) {
		self.core().edit_control_text(value);
	}

	pub fn send_control_text(&mut self) {
		self.core().send_control_text();
	}

	pub fn send_control_key(&mut self, idx: u32) {
		self.core().send_control_key(idx);
	}
}

#[async_trait]
impl Component for PeerControlController {
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
