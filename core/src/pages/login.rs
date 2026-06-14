use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct LoginController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl LoginController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui_controller]
impl LoginController {
	pub fn state(&self) -> UiViewState {
		self.core().state()
	}

	pub fn title(&self) -> String {
		String::from("Login - PuppyNet UI")
	}

	pub fn edit_login_username(&mut self, value: String) {
		self.core().edit_login_username(value);
	}

	pub fn edit_login_password(&mut self, value: String) {
		self.core().edit_login_password(value);
	}

	pub fn login(&mut self) {
		self.core().login();
	}

	pub fn open_app(&mut self) {
		self.core().open_app();
	}
}

#[async_trait]
impl Component for LoginController {
	type Context = UiContext;
	type Db = ();
	type Model = UiViewState;

	async fn mount(
		ctx: Arc<Ctx<Self::Context, Self::Db>>,
		_route: RouteContext,
	) -> MountResult<Self> {
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
