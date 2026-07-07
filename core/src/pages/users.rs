use super::super::redirect_response;
use super::{UiAction, UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};
use wgui::{FormData, HttpResponse};

pub(in super::super) struct UsersController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl UsersController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl UsersController {
	pub fn state(&self) -> UiViewState {
		self.core().users_state()
	}

	pub fn title(&self) -> String {
		String::from("Users - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_users(&mut self) {
		self.core().refresh_users();
	}

	pub fn open_new_user_modal(&mut self) {
		self.core().open_new_user_modal();
	}

	pub fn close_new_user_modal(&mut self) {
		self.core().close_new_user_modal();
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

	#[wgui_post("/users/create")]
	pub async fn create_user_post(&mut self, form: FormData) -> HttpResponse {
		let username = form.get("username").unwrap_or_default().to_string();
		let password = form.get("password").unwrap_or_default().to_string();
		let core = self.core();
		let redirect = if core.is_authenticated() {
			if core.create_user_values_async(username, password).await {
				core.ctx.state.server.refresh_users().await;
			}
			"/users"
		} else {
			"/login"
		};
		redirect_response(redirect).header("cache-control", "no-store")
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
		ctx.state.server.handle_action(UiAction::RefreshUsers).await;
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
