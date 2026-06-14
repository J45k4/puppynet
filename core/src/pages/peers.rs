use super::{UiAction, UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PeersController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PeersController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui_controller]
impl PeersController {
	pub fn state(&self) -> UiViewState {
		self.core().peers_state()
	}

	pub fn title(&self) -> String {
		String::from("Peers - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_peers(&mut self) {
		self.core().refresh_peers();
	}

	pub fn peer_row(&mut self, idx: u32) {
		self.core().peer_row(idx);
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

	pub fn edit_update_version(&mut self, value: String) {
		self.core().edit_update_version(value);
	}

	pub fn start_peer_update(&mut self) {
		self.core().start_peer_update();
	}

	pub fn poll_peer_update(&mut self) {
		self.core().poll_peer_update();
	}
}

#[async_trait]
impl Component for PeersController {
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
		ctx.state.server.handle_action(UiAction::RefreshPeers).await;
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
