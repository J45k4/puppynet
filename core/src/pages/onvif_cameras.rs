use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct OnvifCamerasController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl OnvifCamerasController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl OnvifCamerasController {
	pub fn state(&self) -> UiViewState {
		self.core().onvif_cameras_state()
	}

	pub fn title(&self) -> String {
		String::from("ONVIF Cameras - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_cameras(&mut self) {
		self.core().refresh_puppy_eye();
	}

	pub fn open_onvif_camera_modal(&mut self) {
		self.core().open_onvif_camera_modal();
	}

	pub fn close_onvif_camera_modal(&mut self) {
		self.core().close_onvif_camera_modal();
	}

	pub fn select_onvif_target_device(&mut self, value: String) {
		self.core().select_onvif_target_device(value);
	}

	pub fn edit_onvif_name(&mut self, value: String) {
		self.core().edit_onvif_name(value);
	}

	pub fn edit_onvif_device_service_url(&mut self, value: String) {
		self.core().edit_onvif_device_service_url(value);
	}

	pub fn edit_onvif_username(&mut self, value: String) {
		self.core().edit_onvif_username(value);
	}

	pub fn edit_onvif_password(&mut self, value: String) {
		self.core().edit_onvif_password(value);
	}

	pub fn edit_onvif_profile_token(&mut self, value: String) {
		self.core().edit_onvif_profile_token(value);
	}

	pub fn test_onvif_camera(&mut self) {
		self.core().test_onvif_camera();
	}

	pub fn save_onvif_camera(&mut self) {
		self.core().save_onvif_camera();
	}

	pub fn delete_onvif_camera(&mut self, index: usize) {
		self.core().delete_onvif_camera(index);
	}
}

#[async_trait]
impl Component for OnvifCamerasController {
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
