use super::{UiAction, UiContext, UiControllerCore, UiViewState};
use std::sync::Arc;
use wgui::wui::runtime::{Ctx, MountResult};

fn redirect_unauthenticated<C>(ctx: &Arc<Ctx<UiContext, ()>>) -> Option<MountResult<C>> {
	if UiControllerCore::new(ctx).is_authenticated() {
		None
	} else {
		Some(MountResult::Redirect("/login".to_string()))
	}
}

mod files;
mod home;
mod login;
mod not_found;
mod peer;
mod peer_files;
mod peer_webcams;
mod peers;
mod search;
mod settings;
mod storage;
mod updates;
mod users;

pub(super) use files::FilesController;
pub(super) use home::HomeController;
pub(super) use login::LoginController;
pub(super) use not_found::NotFoundController;
pub(super) use peer::PeerController;
pub(super) use peer_files::PeerFilesController;
pub(super) use peer_webcams::PeerWebcamsController;
pub(super) use peers::PeersController;
pub(super) use search::SearchController;
pub(super) use settings::SettingsController;
pub(super) use storage::StorageController;
pub(super) use updates::UpdatesController;
pub(super) use users::UsersController;
