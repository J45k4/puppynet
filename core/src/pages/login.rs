use super::super::{redirect_response, session_cookie};
use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};
use wgui::{FormData, HttpResponse};

pub(in super::super) struct LoginController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl LoginController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
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

	#[wgui_post("/auth/login")]
	pub fn login_post(&mut self, form: FormData) -> HttpResponse {
		let username = form.get("username").unwrap_or_default().to_string();
		let password = form.get("password").unwrap_or_default().to_string();
		match self.core().login_with_credentials(username, password) {
			Some(token) => redirect_response("/")
				.header("cache-control", "no-store")
				.header("set-cookie", session_cookie(&token)),
			None => redirect_response("/login").header("cache-control", "no-store"),
		}
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
