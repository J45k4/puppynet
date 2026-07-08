use super::{UiAction, UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct SearchController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl SearchController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui::wgui_controller]
impl SearchController {
	pub fn state(&self) -> UiViewState {
		self.core().search_state()
	}

	pub fn title(&self) -> String {
		String::from("Search - PuppyNet UI")
	}

	pub fn logout(&mut self) {
		self.core().logout();
	}

	pub fn edit_search_name_query(&mut self, value: String) {
		self.core().edit_search_name_query(value);
	}

	pub fn toggle_search_mime(&mut self, idx: u32) {
		self.core().toggle_search_mime(idx);
	}

	pub fn clear_search_mimes(&mut self) {
		self.core().clear_search_mimes();
	}

	pub fn select_search_target(&mut self, value: String) {
		self.core().select_search_target(value);
	}

	pub fn select_search_sort(&mut self, value: String) {
		self.core().select_search_sort(value);
	}

	pub fn select_search_page_size(&mut self, value: String) {
		self.core().select_search_page_size(value);
	}

	pub fn run_search(&mut self) {
		self.core().run_search();
	}

	pub fn search_load_more(&mut self) {
		self.core().search_load_more();
	}

	pub fn search_scroll_load_more(&mut self) {
		self.core().search_load_more();
	}

	pub fn search_preview(&mut self, idx: u32) {
		self.core().search_preview(idx);
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
impl Component for SearchController {
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
		ctx.state
			.server
			.handle_action(UiAction::RefreshSearchOptions)
			.await;
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
