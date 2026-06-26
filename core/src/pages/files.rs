use super::{UiAction, UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct FilesController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl FilesController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl FilesController {
	pub fn state(&self) -> UiViewState {
		self.core().files_state()
	}

	pub fn title(&self) -> String {
		String::from("Files - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn refresh_files(&mut self) {
		self.core().refresh_files();
	}

	pub fn preview_local_file(&mut self, idx: u32) {
		self.core().preview_local_file(idx);
	}

	pub fn close_file_preview_modal(&mut self) {
		self.core().close_file_preview_modal();
	}

	pub fn edit_file_preview_path(&mut self, value: String) {
		self.core().edit_file_preview_path(value);
	}

	pub fn edit_file_preview_peer(&mut self, value: String) {
		self.core().edit_file_preview_peer(value);
	}

	pub fn load_file_preview(&mut self) {
		self.core().load_file_preview();
	}
}

#[async_trait]
impl Component for FilesController {
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
		ctx.state.server.handle_action(UiAction::RefreshFiles).await;
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
