use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PuppyEyeController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PuppyEyeController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl PuppyEyeController {
	pub fn state(&self) -> UiViewState {
		self.core().puppy_eye_state()
	}

	pub fn title(&self) -> String {
		String::from("Puppy Eye - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_cameras(&mut self) {
		self.core().refresh_puppy_eye();
	}

	pub fn open_camera(&mut self, index: u32) {
		self.core().open_puppy_eye_camera(index);
	}
}

#[async_trait]
impl Component for PuppyEyeController {
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
