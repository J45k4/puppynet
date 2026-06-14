use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct UsersController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl UsersController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui_controller]
impl UsersController {
	pub fn state(&self) -> UiViewState {
		self.core().users_state()
	}

	pub fn title(&self) -> String {
		String::from("Users - PuppyNet UI")
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

	pub fn refresh_users(&mut self) {
		self.core().refresh_users();
	}

	pub fn edit_new_user_username(&mut self, value: String) {
		self.core().edit_new_user_username(value);
	}

	pub fn edit_new_user_password(&mut self, value: String) {
		self.core().edit_new_user_password(value);
	}

	pub fn create_user(&mut self) {
		self.core().create_user();
	}
}

#[async_trait]
impl Component for UsersController {
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
