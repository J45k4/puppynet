use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct PuppyEyeCameraController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl PuppyEyeCameraController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}

	fn peer_id(&self) -> String {
		self.ctx.param("peer_id").unwrap_or_default()
	}

	fn decode_source_id(source_id: String) -> String {
		if !source_id.contains('%') && !source_id.contains('+') {
			return source_id;
		}
		url::form_urlencoded::parse(format!("source={source_id}").as_bytes())
			.find_map(|(key, value)| {
				if key == "source" {
					Some(value.into_owned())
				} else {
					None
				}
			})
			.unwrap_or(source_id)
	}

	fn source_id(&self) -> String {
		Self::decode_source_id(self.ctx.query("source").unwrap_or_default())
	}
}

#[wgui::wgui_controller]
impl PuppyEyeCameraController {
	pub fn state(&self) -> UiViewState {
		self.core()
			.puppy_eye_camera_state(self.peer_id(), self.source_id())
	}

	pub fn title(&self) -> String {
		String::from("Camera - Puppy Eye")
	}

	pub fn refresh_camera(&mut self) {
		self.core().refresh_puppy_eye();
	}
}

#[async_trait]
impl Component for PuppyEyeCameraController {
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
