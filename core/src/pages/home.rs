use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct HomeController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl HomeController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui_controller]
impl HomeController {
	pub fn state(&self) -> UiViewState {
		self.core().home_state()
	}

	pub fn title(&self) -> String {
		String::from("PuppyNet UI")
	}

	pub fn open_login(&mut self) {
		self.core().open_login();
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn nav_home(&mut self) {
		self.core().nav_home();
	}

	pub fn nav_peers(&mut self) {
		self.core().nav_peers();
	}

	pub fn nav_files(&mut self) {
		self.core().nav_files();
	}

	pub fn nav_search(&mut self) {
		self.core().nav_search();
	}

	pub fn nav_storage(&mut self) {
		self.core().nav_storage();
	}

	pub fn nav_users(&mut self) {
		self.core().nav_users();
	}

	pub fn nav_updates(&mut self) {
		self.core().nav_updates();
	}

	pub fn nav_settings(&mut self) {
		self.core().nav_settings();
	}
}

#[async_trait]
impl Component for HomeController {
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
