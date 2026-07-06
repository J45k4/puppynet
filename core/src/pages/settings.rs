use super::super::redirect_response;
use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};
use wgui::{FormData, HttpResponse};

pub(in super::super) struct SettingsController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl SettingsController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl SettingsController {
	pub fn state(&self) -> UiViewState {
		self.core().settings_state()
	}

	pub fn title(&self) -> String {
		String::from("Settings - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn edit_current_password(&mut self, value: String) {
		self.core().edit_current_password(value);
	}

	pub fn edit_new_password(&mut self, value: String) {
		self.core().edit_new_password(value);
	}

	pub fn edit_confirm_password(&mut self, value: String) {
		self.core().edit_confirm_password(value);
	}

	pub fn change_password(&mut self) {
		self.core().change_password();
	}

	#[wgui_post("/settings/password")]
	pub fn change_password_post(&mut self, form: FormData) -> HttpResponse {
		let current_password = form.get("current_password").unwrap_or_default().to_string();
		let new_password = form.get("new_password").unwrap_or_default().to_string();
		let confirm_password = form.get("confirm_password").unwrap_or_default().to_string();
		let core = self.core();
		let redirect = if core.is_authenticated() {
			core.change_password_values(current_password, new_password, confirm_password);
			"/settings"
		} else {
			"/login"
		};
		redirect_response(redirect).header("cache-control", "no-store")
	}
}

#[async_trait]
impl Component for SettingsController {
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
