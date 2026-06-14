use super::{UiContext, UiControllerCore, UiViewState};
use async_trait::async_trait;
use std::sync::Arc;
use wgui::wgui_controller;
use wgui::wui::runtime::{Component, Ctx, MountResult, RouteContext};

pub(in super::super) struct SearchController {
	ctx: Arc<Ctx<UiContext, ()>>,
}

impl SearchController {
	fn core(&self) -> UiControllerCore<'_> {
		UiControllerCore::new(&self.ctx)
	}
}

#[wgui_controller]
impl SearchController {
	pub fn state(&self) -> UiViewState {
		self.core().search_state()
	}

	pub fn title(&self) -> String {
		String::from("Search - PuppyNet UI")
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

	pub fn edit_search_name_query(&mut self, value: String) {
		self.core().edit_search_name_query(value);
	}

	pub fn toggle_search_mime(&mut self, idx: u32) {
		self.core().toggle_search_mime(idx);
	}

	pub fn clear_search_mimes(&mut self) {
		self.core().clear_search_mimes();
	}

	pub fn run_search(&mut self) {
		self.core().run_search();
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
		MountResult::Ready(Self { ctx })
	}

	fn render(&self, _ctx: &Ctx<Self::Context, Self::Db>) -> Self::Model {
		self.state()
	}

	fn unmount(self, _ctx: Arc<Ctx<Self::Context, Self::Db>>) {}
}
