#[cfg(feature = "hyper")]
use server::Server;
use std::any::{Any, TypeId};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
#[cfg(feature = "hyper")]
use std::net::SocketAddr;
#[cfg(feature = "hyper")]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

pub mod db_table;
pub mod diff;
pub mod dist;
pub mod edit_distance;
pub mod gui;
pub mod pubsub;
pub mod schema_diff;
#[cfg(feature = "hyper")]
mod server;
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod ssr;
pub mod table;
pub mod types;
mod ui_client;
pub mod wdb;
pub mod ws;
pub mod wui;

pub use pubsub::PubSub;
pub use serde_json;
pub use wui::runtime::{WdbModel, WdbSchema, WguiModel};
pub use wui_derive::{wgui_controller, Wdb, WguiModel};

use crate::ui_client::UiWsWorker;
use crate::wui::routing::{best_route_index, RoutePattern};
use crate::wui::runtime::{MountResult, RouteContext};

pub use db_table::{Db, DbTable};
pub use dist::*;
pub use gui::*;
#[cfg(feature = "hyper")]
pub use server::{HttpHandler, HttpRequest, HttpResponse};
#[cfg(feature = "sqlite")]
pub use sqlite::{
	apply_sqlite_migrations, configure_sqlite_env_for_project, default_db_path_for_schema,
	schema_diff_sql, schema_diff_sql_from_schema_file, write_schema_migration,
	write_schema_migration_from_schema_file, SQLLiteDB, SQLiteDB, SchemaMigrations, SqliteDb,
	SqliteTable,
};
pub use table::{HasId, Table};
pub use types::*;
#[cfg(feature = "hyper")]
pub use ws::TungsteniteWs;
pub use ws::{next_client_id, WsMessage, WsStream};

pub(crate) type Sessions = Arc<RwLock<HashMap<usize, Option<String>>>>;
type BoxedController = Box<dyn crate::wui::runtime::WuiController + Send>;
type ControllerFuture = Pin<Box<dyn Future<Output = BoxedController> + Send>>;
type ControllerFactory = Arc<dyn Fn() -> ControllerFuture + Send + Sync>;
type PageControllerFuture = Pin<Box<dyn Future<Output = PageMount> + Send>>;
type PageControllerFactory =
	Arc<dyn Fn(RouteContext, Option<usize>, Option<String>) -> PageControllerFuture + Send + Sync>;
type SsrRenderer = Arc<dyn Fn(RouteContext, Option<String>) -> Option<SsrResponse> + Send + Sync>;
type SsrComponentFactories = Arc<std::sync::RwLock<Vec<(String, ControllerFactory)>>>;
type SsrPageFactories = Arc<std::sync::RwLock<Vec<(RoutePattern, PageControllerFactory)>>>;

enum PageMount {
	Ready(BoxedController),
	Redirect(String),
}

pub(crate) enum SsrResponse {
	Render(Item),
	Redirect(String),
}

fn component_route_match_score(route_path: &str, current_path: &str) -> Option<usize> {
	if route_path == "/" {
		return Some(0);
	}
	if current_path == route_path {
		return Some(route_path.trim_end_matches('/').len());
	}
	let prefix = format!("{}/", route_path.trim_end_matches('/'));
	if current_path.starts_with(&prefix) {
		return Some(route_path.trim_end_matches('/').len());
	}
	None
}

fn best_component_route_index<T, F>(
	routes: &[T],
	current_path: &str,
	route_path: F,
) -> Option<usize>
where
	F: Fn(&T) -> &str,
{
	let mut best = None;
	for (index, route) in routes.iter().enumerate() {
		let Some(score) = component_route_match_score(route_path(route), current_path) else {
			continue;
		};
		if best
			.map(|(_, best_score)| score > best_score)
			.unwrap_or(true)
		{
			best = Some((index, score));
		}
	}
	best.map(|(index, _)| index)
}

fn page_route_context(
	pattern: &RoutePattern,
	path: &str,
	query: &HashMap<String, String>,
) -> Option<RouteContext> {
	let matched = pattern.match_path(path)?;
	Some(RouteContext {
		path: path.to_string(),
		params: matched.params,
		query: query.clone(),
	})
}

fn component_route_context(path: &str, query: &HashMap<String, String>) -> RouteContext {
	RouteContext {
		path: path.to_string(),
		params: HashMap::new(),
		query: query.clone(),
	}
}

#[derive(Clone)]
pub struct WguiHandle {
	event_tx: mpsc::UnboundedSender<ClientMessage>,
	clients: Clients,
	sessions: Sessions,
}

impl WguiHandle {
	pub(crate) fn new(
		event_tx: mpsc::UnboundedSender<ClientMessage>,
		clients: Clients,
		sessions: Sessions,
	) -> Self {
		Self {
			event_tx,
			clients,
			sessions,
		}
	}

	pub async fn handle_ws<S>(&self, ws: S) -> usize
	where
		S: WsStream + 'static,
	{
		let id = next_client_id();
		let event_tx = self.event_tx.clone();
		let clients = self.clients.clone();
		log::info!("websocket worker created {}", id);
		tokio::spawn(async move {
			let worker = UiWsWorker::new(id, ws, event_tx, clients).await;
			worker.run().await;
		});

		let mut sessions = self.sessions.write().await;
		sessions.insert(id, None);

		id
	}

	pub async fn handle_ws_with_session<S>(&self, ws: S, session: Option<String>) -> usize
	where
		S: WsStream + 'static,
	{
		let id = self.handle_ws(ws).await;
		let mut sessions = self.sessions.write().await;
		sessions.insert(id, session);
		id
	}

	pub async fn render(&self, client_id: usize, item: Item) {
		log::debug!("render {:?}", item);
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::Render(item)).unwrap();
	}

	pub async fn replace_root(&self, client_id: usize, item: Item) {
		log::debug!("replace root {:?}", item);
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::ReplaceRoot(item)).unwrap();
	}

	pub async fn set_title(&self, client_id: usize, title: &str) {
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::SetTitle(title.to_string())).unwrap();
	}

	pub async fn push_state(&self, client_id: usize, url: &str) {
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::PushState(url.to_string())).unwrap();
	}

	pub async fn navigate(&self, client_id: usize, url: &str) {
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::Navigate(url.to_string())).unwrap();
	}

	pub async fn enable_web_push(
		&self,
		client_id: usize,
		service_worker_path: impl Into<String>,
		vapid_public_key: Option<String>,
	) {
		let service_worker_path = service_worker_path.into();
		if service_worker_path.trim().is_empty() {
			return;
		}
		self.send_actions(
			client_id,
			vec![ClientAction::WebPushEnable {
				service_worker_path,
				vapid_public_key,
			}],
		)
		.await;
	}

	pub async fn disable_web_push(&self, client_id: usize, service_worker_path: impl Into<String>) {
		let service_worker_path = service_worker_path.into();
		if service_worker_path.trim().is_empty() {
			return;
		}
		self.send_actions(
			client_id,
			vec![ClientAction::WebPushDisable {
				service_worker_path,
			}],
		)
		.await;
	}

	pub async fn send_actions(&self, client_id: usize, actions: Vec<ClientAction>) {
		if actions.is_empty() {
			return;
		}
		let clients = self.clients.read().await;
		let sender = match clients.get(&client_id) {
			Some(sender) => sender,
			None => {
				println!("client not found");
				return;
			}
		};
		sender.send(Command::Actions(actions)).unwrap();
	}

	pub async fn session_for_client(&self, client_id: usize) -> Option<String> {
		let sessions = self.sessions.read().await;
		sessions.get(&client_id).cloned().flatten()
	}

	pub async fn clear_session(&self, client_id: usize) {
		let mut sessions = self.sessions.write().await;
		sessions.remove(&client_id);
	}
}

struct ComponentRegistration {
	route_path: String,
	factory: ControllerFactory,
	controllers: HashMap<usize, BoxedController>,
}

impl ComponentRegistration {
	fn new(route_path: String, factory: ControllerFactory) -> Self {
		Self {
			route_path,
			factory,
			controllers: HashMap::new(),
		}
	}
}

struct PageRegistration {
	pattern: RoutePattern,
	factory: PageControllerFactory,
	controllers: HashMap<usize, BoxedController>,
}

impl PageRegistration {
	fn new(pattern: RoutePattern, factory: PageControllerFactory) -> Self {
		Self {
			pattern,
			factory,
			controllers: HashMap::new(),
		}
	}
}

struct ContextAwareController<C, T, DB>
where
	C: crate::wui::runtime::WuiController + Send + 'static,
	T: Send + Sync + 'static,
	DB: Send + Sync + 'static,
{
	inner: C,
	ctx: Arc<crate::wui::runtime::Ctx<T, DB>>,
}

#[crate::wui::runtime::async_trait]
impl<C, T, DB> crate::wui::runtime::WuiController for ContextAwareController<C, T, DB>
where
	C: crate::wui::runtime::WuiController + Send + 'static,
	T: Send + Sync + 'static,
	DB: Send + Sync + 'static,
{
	fn render(&self) -> Item {
		self.inner.render()
	}

	fn render_with_path(&self, path: &str) -> Item {
		self.inner.render_with_path(path)
	}

	fn render_with_route(&self, route: &RouteContext) -> Item {
		self.inner.render_with_route(route)
	}

	fn title(&self) -> Option<String> {
		self.inner.title()
	}

	fn route_title(&self, path: &str) -> Option<String> {
		self.inner.route_title(path)
	}

	fn set_runtime_context(&mut self, client_id: Option<usize>, session: Option<String>) {
		self.ctx.set_current_client(client_id);
		self.ctx.set_current_session(session.clone());
		self.inner.set_runtime_context(client_id, session);
	}

	fn set_route_context(&mut self, route: Option<RouteContext>) {
		self.ctx.set_current_route(route.clone());
		self.inner.set_route_context(route);
	}

	async fn handle(&mut self, event: &crate::types::ClientEvent) -> bool {
		self.inner.handle(event).await
	}
}

pub struct Wgui<DB = ()> {
	events_rx: mpsc::UnboundedReceiver<ClientMessage>,
	handle: WguiHandle,
	components: Vec<ComponentRegistration>,
	pages: Vec<PageRegistration>,
	ssr_components: SsrComponentFactories,
	ssr_pages: SsrPageFactories,
	db: Arc<DB>,
	contexts: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
	#[cfg(feature = "hyper")]
	http_handler: server::SharedHttpHandler,
	#[cfg(feature = "hyper")]
	app_css: server::SharedAppCss,
	#[cfg(feature = "hyper")]
	static_mounts: server::SharedStaticMounts,
}

impl Wgui<()> {
	#[cfg(feature = "hyper")]
	pub fn new(addr: SocketAddr) -> Self {
		let (events_tx, events_rx) = mpsc::unbounded_channel();
		let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
		let sessions: Sessions = Arc::new(RwLock::new(HashMap::new()));
		let ssr_components: SsrComponentFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		let ssr_pages: SsrPageFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		let http_handler = Arc::new(std::sync::RwLock::new(None));
		let app_css = Arc::new(std::sync::RwLock::new(None));
		let static_mounts = Arc::new(std::sync::RwLock::new(Vec::new()));

		{
			let clients = clients.clone();
			let event_tx = events_tx.clone();
			let sessions = sessions.clone();
			let ssr_components = ssr_components.clone();
			let ssr_pages = ssr_pages.clone();
			let http_handler = http_handler.clone();
			let app_css = app_css.clone();
			let static_mounts = static_mounts.clone();
			let ssr: Option<SsrRenderer> = Some(Arc::new(
				move |route: RouteContext, session: Option<String>| {
					if let Some((factory, route)) = {
						let pages = ssr_pages.read().unwrap();
						let index = best_route_index(&pages, &route.path, |(pattern, _)| pattern)?;
						let pattern = &pages[index].0;
						let route = page_route_context(pattern, &route.path, &route.query)?;
						Some((pages[index].1.clone(), route))
					} {
						let mount = tokio::task::block_in_place(|| {
							tokio::runtime::Handle::current().block_on((factory)(
								route.clone(),
								None,
								session.clone(),
							))
						});
						return match mount {
							PageMount::Ready(mut controller) => {
								controller.set_runtime_context(None, session.clone());
								controller.set_route_context(Some(route.clone()));
								Some(SsrResponse::Render(controller.render_with_route(&route)))
							}
							PageMount::Redirect(url) => Some(SsrResponse::Redirect(url)),
						};
					}

					let factory = {
						let factories = ssr_components.read().unwrap();
						let index = best_component_route_index(
							&factories,
							&route.path,
							|(route_path, _)| route_path.as_str(),
						)?;
						factories[index].1.clone()
					};
					let route = component_route_context(&route.path, &route.query);
					let mut controller = tokio::task::block_in_place(|| {
						tokio::runtime::Handle::current().block_on((factory)())
					});
					controller.set_runtime_context(None, session);
					controller.set_route_context(Some(route.clone()));
					Some(SsrResponse::Render(controller.render_with_route(&route)))
				},
			));
			tokio::spawn(async move {
				Server::new(
					addr,
					event_tx,
					clients,
					sessions,
					ssr,
					http_handler,
					app_css,
					static_mounts,
				)
				.await
				.run()
				.await;
			});
		}

		Self {
			events_rx,
			handle: WguiHandle::new(events_tx, clients, sessions),
			components: Vec::new(),
			pages: Vec::new(),
			ssr_components,
			ssr_pages,
			db: Arc::new(()),
			contexts: HashMap::new(),
			http_handler,
			app_css,
			static_mounts,
		}
	}

	#[cfg(feature = "hyper")]
	pub fn new_with_ssr(
		addr: SocketAddr,
		renderer: std::sync::Arc<dyn Fn() -> Item + Send + Sync>,
	) -> Self {
		let (events_tx, events_rx) = mpsc::unbounded_channel();
		let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
		let sessions: Sessions = Arc::new(RwLock::new(HashMap::new()));
		let ssr_components: SsrComponentFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		let ssr_pages: SsrPageFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		let http_handler = Arc::new(std::sync::RwLock::new(None));
		let app_css = Arc::new(std::sync::RwLock::new(None));
		let static_mounts = Arc::new(std::sync::RwLock::new(Vec::new()));

		{
			let clients = clients.clone();
			let event_tx = events_tx.clone();
			let sessions = sessions.clone();
			let http_handler = http_handler.clone();
			let app_css = app_css.clone();
			let static_mounts = static_mounts.clone();
			let ssr: Option<SsrRenderer> = Some(Arc::new(
				move |_route: RouteContext, _session: Option<String>| {
					Some(SsrResponse::Render((renderer)()))
				},
			));
			tokio::spawn(async move {
				Server::new(
					addr,
					event_tx,
					clients,
					sessions,
					ssr,
					http_handler,
					app_css,
					static_mounts,
				)
				.await
				.run()
				.await;
			});
		}

		Self {
			events_rx,
			handle: WguiHandle::new(events_tx, clients, sessions),
			components: Vec::new(),
			pages: Vec::new(),
			ssr_components,
			ssr_pages,
			db: Arc::new(()),
			contexts: HashMap::new(),
			http_handler,
			app_css,
			static_mounts,
		}
	}

	pub fn new_without_server() -> Self {
		let (events_tx, events_rx) = mpsc::unbounded_channel();
		let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
		let sessions: Sessions = Arc::new(RwLock::new(HashMap::new()));
		let ssr_components: SsrComponentFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		let ssr_pages: SsrPageFactories = Arc::new(std::sync::RwLock::new(Vec::new()));
		#[cfg(feature = "hyper")]
		let http_handler = Arc::new(std::sync::RwLock::new(None));
		#[cfg(feature = "hyper")]
		let app_css = Arc::new(std::sync::RwLock::new(None));
		#[cfg(feature = "hyper")]
		let static_mounts = Arc::new(std::sync::RwLock::new(Vec::new()));

		Self {
			events_rx,
			handle: WguiHandle::new(events_tx, clients, sessions),
			components: Vec::new(),
			pages: Vec::new(),
			ssr_components,
			ssr_pages,
			db: Arc::new(()),
			contexts: HashMap::new(),
			#[cfg(feature = "hyper")]
			http_handler,
			#[cfg(feature = "hyper")]
			app_css,
			#[cfg(feature = "hyper")]
			static_mounts,
		}
	}
}

impl<DB> Wgui<DB>
where
	DB: Send + Sync + 'static,
{
	pub fn with_db<NewDB>(self, db: NewDB) -> Wgui<NewDB>
	where
		NewDB: Send + Sync + 'static,
	{
		Wgui {
			events_rx: self.events_rx,
			handle: self.handle,
			components: self.components,
			pages: self.pages,
			ssr_components: self.ssr_components,
			ssr_pages: self.ssr_pages,
			db: Arc::new(db),
			contexts: HashMap::new(),
			#[cfg(feature = "hyper")]
			http_handler: self.http_handler,
			#[cfg(feature = "hyper")]
			app_css: self.app_css,
			#[cfg(feature = "hyper")]
			static_mounts: self.static_mounts,
		}
	}

	#[cfg(feature = "hyper")]
	pub fn set_css(&self, css: impl Into<String>) {
		*self.app_css.write().unwrap() = Some(css.into());
	}

	#[cfg(feature = "hyper")]
	pub fn set_http_handler<F, Fut>(&self, handler: F)
	where
		F: Fn(HttpRequest) -> Fut + Send + Sync + 'static,
		Fut: Future<Output = Option<HttpResponse>> + Send + 'static,
	{
		let handler: HttpHandler = Arc::new(move |request| Box::pin(handler(request)));
		*self.http_handler.write().unwrap() = Some(handler);
	}

	#[cfg(feature = "hyper")]
	pub fn mount_static_file(&self, route: impl Into<String>, file: impl Into<PathBuf>) {
		self.static_mounts
			.write()
			.unwrap()
			.push(server::StaticMount::file(route.into(), file.into()));
	}

	#[cfg(feature = "hyper")]
	pub fn mount_static_dir(&self, route_prefix: impl Into<String>, dir: impl Into<PathBuf>) {
		self.static_mounts
			.write()
			.unwrap()
			.push(server::StaticMount::dir(route_prefix.into(), dir.into()));
	}

	pub fn handle(&self) -> WguiHandle {
		self.handle.clone()
	}

	pub async fn next(&mut self) -> Option<ClientMessage> {
		self.events_rx.recv().await
	}

	pub async fn render(&self, client_id: usize, item: Item) {
		self.handle.render(client_id, item).await
	}

	pub async fn set_title(&self, client_id: usize, title: &str) {
		self.handle.set_title(client_id, title).await
	}

	pub async fn session_for_client(&self, client_id: usize) -> Option<String> {
		self.handle.session_for_client(client_id).await
	}

	pub async fn clear_session(&self, client_id: usize) {
		self.handle.clear_session(client_id).await
	}

	pub async fn enable_web_push(
		&self,
		client_id: usize,
		service_worker_path: impl Into<String>,
		vapid_public_key: Option<String>,
	) {
		self.handle
			.enable_web_push(client_id, service_worker_path, vapid_public_key)
			.await;
	}

	pub async fn disable_web_push(&self, client_id: usize, service_worker_path: impl Into<String>) {
		self.handle
			.disable_web_push(client_id, service_worker_path)
			.await;
	}

	pub fn set_ctx<T>(&mut self, ctx: Arc<crate::wui::runtime::Ctx<T, DB>>)
	where
		T: Send + Sync + 'static,
	{
		let mut command_rx = ctx.take_command_rx();
		let handle = self.handle();
		tokio::spawn(async move {
			while let Some(command) = command_rx.recv().await {
				match command {
					crate::wui::runtime::RuntimeCommand::SetTitle { client_id, title } => {
						handle.set_title(client_id, &title).await;
					}
					crate::wui::runtime::RuntimeCommand::PushState { client_id, url } => {
						handle.push_state(client_id, &url).await;
					}
					crate::wui::runtime::RuntimeCommand::Navigate { client_id, url } => {
						handle.navigate(client_id, &url).await;
					}
					crate::wui::runtime::RuntimeCommand::WebPushEnable {
						client_id,
						service_worker_path,
						vapid_public_key,
					} => {
						handle
							.enable_web_push(client_id, service_worker_path, vapid_public_key)
							.await;
					}
					crate::wui::runtime::RuntimeCommand::WebPushDisable {
						client_id,
						service_worker_path,
					} => {
						handle
							.disable_web_push(client_id, service_worker_path)
							.await;
					}
				}
			}
		});

		let erased: Arc<dyn Any + Send + Sync> = ctx;
		self.contexts.insert(TypeId::of::<T>(), erased);
	}

	pub fn set_ctx_state<T>(&mut self, state: T)
	where
		T: Send + Sync + 'static,
	{
		let ctx = Arc::new(crate::wui::runtime::Ctx::new_with_db(
			state,
			self.db.clone(),
		));
		self.set_ctx(ctx);
	}

	pub fn add_component_with<C, F, Fut>(&mut self, path: &str, controller: F)
	where
		C: crate::wui::runtime::WuiController + Send + 'static,
		F: Fn() -> Fut + Send + Sync + 'static,
		Fut: Future<Output = C> + Send + 'static,
	{
		let factory: ControllerFactory = Arc::new(move || {
			let fut = controller();
			Box::pin(async move { Box::new(fut.await) as BoxedController })
		});
		self.ssr_components
			.write()
			.unwrap()
			.push((path.to_string(), factory.clone()));
		self.components
			.push(ComponentRegistration::new(path.to_string(), factory));
	}

	pub fn add_component<C>(&mut self, path: &str)
	where
		C: crate::wui::runtime::Component<Db = DB>
			+ crate::wui::runtime::WuiController
			+ Send
			+ 'static,
		<C as crate::wui::runtime::Component>::Context: Send + Sync + 'static,
	{
		let Some(ctx_any) = self
			.contexts
			.get(&TypeId::of::<<C as crate::wui::runtime::Component>::Context>())
			.cloned()
		else {
			panic!("missing context for component; call wgui.set_ctx(...) first");
		};
		let Ok(ctx) = ctx_any
			.downcast::<crate::wui::runtime::Ctx<<C as crate::wui::runtime::Component>::Context, DB>>(
			)
		else {
			panic!("invalid context type for component");
		};

		self.add_component_with(path, move || {
			let ctx = ctx.clone();
			async move {
				ContextAwareController {
					inner: match C::mount(ctx.clone(), RouteContext::default()).await {
						MountResult::Ready(inner) => inner,
						MountResult::Redirect(_) => {
							panic!(
								"component mount cannot redirect; use add_page for routable pages"
							)
						}
					},
					ctx,
				}
			}
		});
	}

	pub fn add_page_with<C, F, Fut>(&mut self, route: &str, controller: F)
	where
		C: crate::wui::runtime::WuiController + Send + 'static,
		F: Fn() -> Fut + Send + Sync + 'static,
		Fut: Future<Output = C> + Send + 'static,
	{
		let pattern = RoutePattern::parse(route);
		let factory: PageControllerFactory = Arc::new(move |_route, _client_id, _session| {
			let fut = controller();
			Box::pin(async move { PageMount::Ready(Box::new(fut.await) as BoxedController) })
		});
		self.ssr_pages
			.write()
			.unwrap()
			.push((pattern.clone(), factory.clone()));
		self.pages.push(PageRegistration::new(pattern, factory));
	}

	pub fn add_page<C>(&mut self, route: &str)
	where
		C: crate::wui::runtime::Component<Db = DB>
			+ crate::wui::runtime::WuiController
			+ Send
			+ 'static,
		<C as crate::wui::runtime::Component>::Context: Send + Sync + 'static,
	{
		let Some(ctx_any) = self
			.contexts
			.get(&TypeId::of::<<C as crate::wui::runtime::Component>::Context>())
			.cloned()
		else {
			panic!("missing context for page; call wgui.set_ctx(...) first");
		};
		let Ok(ctx) = ctx_any
			.downcast::<crate::wui::runtime::Ctx<<C as crate::wui::runtime::Component>::Context, DB>>(
			)
		else {
			panic!("invalid context type for page");
		};

		let pattern = RoutePattern::parse(route);
		let factory: PageControllerFactory = Arc::new(move |route, client_id, session| {
			let ctx = ctx.clone();
			Box::pin(async move {
				ctx.set_current_client(client_id);
				ctx.set_current_session(session.clone());
				ctx.set_current_route(Some(route.clone()));
				match C::mount(ctx.clone(), route).await {
					MountResult::Ready(inner) => PageMount::Ready(
						Box::new(ContextAwareController { inner, ctx }) as BoxedController,
					),
					MountResult::Redirect(url) => PageMount::Redirect(url),
				}
			})
		});
		self.ssr_pages
			.write()
			.unwrap()
			.push((pattern.clone(), factory.clone()));
		self.pages.push(PageRegistration::new(pattern, factory));
	}

	pub async fn run(&mut self) {
		let handle = self.handle();
		let mut routes: HashMap<usize, RouteContext> = HashMap::new();
		let mut selected_pages: HashMap<usize, Option<usize>> = HashMap::new();
		let mut rtc_rooms: HashMap<String, BTreeSet<usize>> = HashMap::new();
		let mut rtc_client_rooms: HashMap<usize, BTreeSet<String>> = HashMap::new();
		let mut rtc_room_names: HashMap<String, HashMap<usize, String>> = HashMap::new();

		while let Some(message) = self.next().await {
			let client_id = message.client_id;
			match &message.event {
				ClientEvent::Connected { id: _ } => {}
				ClientEvent::Disconnected { id: _ } => {
					selected_pages.remove(&client_id);
					routes.remove(&client_id);
					if let Some(rooms) = rtc_client_rooms.remove(&client_id) {
						for room in rooms {
							let mut room_peers = Vec::new();
							let remove_room = if let Some(participants) = rtc_rooms.get_mut(&room) {
								participants.remove(&client_id);
								room_peers = participants.iter().copied().collect::<Vec<_>>();
								participants.is_empty()
							} else {
								false
							};
							if let Some(names) = rtc_room_names.get_mut(&room) {
								names.remove(&client_id);
								if names.is_empty() {
									rtc_room_names.remove(&room);
								}
							}

							if remove_room {
								rtc_rooms.remove(&room);
								rtc_room_names.remove(&room);
								continue;
							}
							let room_participants = room_peers
								.iter()
								.map(|peer_id| WebRtcParticipant {
									client_id: *peer_id,
									display_name: rtc_room_names
										.get(&room)
										.and_then(|names| names.get(peer_id))
										.cloned()
										.unwrap_or_else(|| format!("user {}", peer_id)),
								})
								.collect::<Vec<_>>();

							for peer_id in &room_peers {
								handle
									.send_actions(
										*peer_id,
										vec![ClientAction::WebRtcRoomState {
											room: room.clone(),
											self_client_id: *peer_id,
											peers: room_peers.clone(),
											participants: room_participants.clone(),
										}],
									)
									.await;
							}
						}
					}

					for component in self.components.iter_mut() {
						component.controllers.remove(&client_id);
					}
					for page in self.pages.iter_mut() {
						page.controllers.remove(&client_id);
					}
					routes.remove(&client_id);
					handle.clear_session(client_id).await;
				}
				ClientEvent::WebRtcJoin(join) => {
					if join.room.is_empty() {
						continue;
					}
					let display_name = join
						.display_name
						.clone()
						.map(|name| name.trim().to_string())
						.filter(|name| !name.is_empty())
						.unwrap_or_else(|| format!("user {}", client_id));

					let peers = {
						let participants = rtc_rooms
							.entry(join.room.clone())
							.or_insert_with(BTreeSet::new);
						participants.insert(client_id);
						participants.iter().copied().collect::<Vec<_>>()
					};
					rtc_room_names
						.entry(join.room.clone())
						.or_insert_with(HashMap::new)
						.insert(client_id, display_name);
					rtc_client_rooms
						.entry(client_id)
						.or_insert_with(BTreeSet::new)
						.insert(join.room.clone());
					let room_participants = peers
						.iter()
						.map(|peer_id| WebRtcParticipant {
							client_id: *peer_id,
							display_name: rtc_room_names
								.get(&join.room)
								.and_then(|names| names.get(peer_id))
								.cloned()
								.unwrap_or_else(|| format!("user {}", peer_id)),
						})
						.collect::<Vec<_>>();

					for peer_id in &peers {
						handle
							.send_actions(
								*peer_id,
								vec![ClientAction::WebRtcRoomState {
									room: join.room.clone(),
									self_client_id: *peer_id,
									peers: peers.clone(),
									participants: room_participants.clone(),
								}],
							)
							.await;
					}
				}
				ClientEvent::WebRtcLeave(leave) => {
					let mut peers = Vec::new();
					let remove_room = if let Some(participants) = rtc_rooms.get_mut(&leave.room) {
						participants.remove(&client_id);
						peers = participants.iter().copied().collect::<Vec<_>>();
						participants.is_empty()
					} else {
						false
					};
					if let Some(names) = rtc_room_names.get_mut(&leave.room) {
						names.remove(&client_id);
						if names.is_empty() {
							rtc_room_names.remove(&leave.room);
						}
					}

					if let Some(rooms) = rtc_client_rooms.get_mut(&client_id) {
						rooms.remove(&leave.room);
						if rooms.is_empty() {
							rtc_client_rooms.remove(&client_id);
						}
					}

					if remove_room {
						rtc_rooms.remove(&leave.room);
						rtc_room_names.remove(&leave.room);
						continue;
					}
					let room_participants = peers
						.iter()
						.map(|peer_id| WebRtcParticipant {
							client_id: *peer_id,
							display_name: rtc_room_names
								.get(&leave.room)
								.and_then(|names| names.get(peer_id))
								.cloned()
								.unwrap_or_else(|| format!("user {}", peer_id)),
						})
						.collect::<Vec<_>>();

					for peer_id in &peers {
						handle
							.send_actions(
								*peer_id,
								vec![ClientAction::WebRtcRoomState {
									room: leave.room.clone(),
									self_client_id: *peer_id,
									peers: peers.clone(),
									participants: room_participants.clone(),
								}],
							)
							.await;
					}
				}
				ClientEvent::WebRtcSignal(signal) => {
					let participants = rtc_rooms
						.get(&signal.room)
						.map(|ids| ids.iter().copied().collect::<Vec<_>>())
						.unwrap_or_default();
					if !participants.contains(&client_id) {
						continue;
					}

					let recipients = if let Some(target_id) = signal.target_client_id {
						if participants.contains(&target_id) {
							vec![target_id]
						} else {
							Vec::new()
						}
					} else {
						participants
							.into_iter()
							.filter(|id| *id != client_id)
							.collect::<Vec<_>>()
					};

					for target_id in recipients {
						handle
							.send_actions(
								target_id,
								vec![ClientAction::WebRtcSignal {
									room: signal.room.clone(),
									from_client_id: client_id,
									payload: signal.payload.clone(),
								}],
							)
							.await;
					}
				}
				ClientEvent::PathChanged(change) => {
					let session = handle.session_for_client(client_id).await;
					let selected_page =
						best_route_index(&self.pages, &change.path, |page| &page.pattern);
					let selected_page_changed =
						selected_pages.get(&client_id).copied().flatten() != selected_page;
					selected_pages.insert(client_id, selected_page);
					let active_route = if let Some(index) = selected_page {
						page_route_context(&self.pages[index].pattern, &change.path, &change.query)
							.unwrap_or_else(|| component_route_context(&change.path, &change.query))
					} else {
						component_route_context(&change.path, &change.query)
					};
					routes.insert(client_id, active_route.clone());

					for (index, page) in self.pages.iter_mut().enumerate() {
						if Some(index) != selected_page {
							page.controllers.remove(&client_id);
							continue;
						}

						match (page.factory)(active_route.clone(), Some(client_id), session.clone())
							.await
						{
							PageMount::Ready(mut controller) => {
								controller.set_runtime_context(Some(client_id), session.clone());
								controller.set_route_context(Some(active_route.clone()));
								let item = controller.render_with_route(&active_route);
								let title = controller
									.title()
									.or_else(|| controller.route_title(&active_route.path));
								if let Some(title) = title {
									handle.set_title(client_id, &title).await;
								}
								if selected_page_changed {
									handle.replace_root(client_id, item).await;
								} else {
									handle.render(client_id, item).await;
								}
								page.controllers.insert(client_id, controller);
							}
							PageMount::Redirect(url) => {
								page.controllers.remove(&client_id);
								handle.push_state(client_id, &url).await;
							}
						}
					}

					if selected_page.is_some() {
						for component in self.components.iter_mut() {
							component.controllers.remove(&client_id);
						}
						continue;
					}

					let selected_component =
						best_component_route_index(&self.components, &change.path, |component| {
							component.route_path.as_str()
						});

					for (index, component) in self.components.iter_mut().enumerate() {
						if Some(index) != selected_component {
							component.controllers.remove(&client_id);
							continue;
						}

						if let Some(controller) = component.controllers.get_mut(&client_id) {
							controller.set_runtime_context(Some(client_id), session.clone());
							controller.set_route_context(Some(active_route.clone()));
							let item = controller.render_with_route(&active_route);
							let title = controller
								.title()
								.or_else(|| controller.route_title(&active_route.path));
							if let Some(title) = title {
								handle.set_title(client_id, &title).await;
							}
							handle.render(client_id, item).await;
						} else {
							let mut controller = (component.factory)().await;
							controller.set_runtime_context(Some(client_id), session.clone());
							controller.set_route_context(Some(active_route.clone()));
							let item = controller.render_with_route(&active_route);
							let title = controller
								.title()
								.or_else(|| controller.route_title(&active_route.path));
							if let Some(title) = title {
								handle.set_title(client_id, &title).await;
							}
							handle.render(client_id, item).await;
							component.controllers.insert(client_id, controller);
						}
					}
				}
				ClientEvent::Input(_) => {}
				_ => {
					let session = handle.session_for_client(client_id).await;
					let mut handled = false;
					for component in self.components.iter_mut() {
						if let Some(controller) = component.controllers.get_mut(&client_id) {
							let route = routes
								.get(&client_id)
								.cloned()
								.unwrap_or_else(|| component_route_context("/", &HashMap::new()));
							controller.set_runtime_context(Some(client_id), session.clone());
							controller.set_route_context(Some(route));
							handled = controller.handle(&message.event).await;
						}
						if handled {
							let mut updates: Vec<(usize, Item, Option<String>)> = Vec::new();
							for (mounted_client_id, mounted_controller) in
								component.controllers.iter_mut()
							{
								let mounted_session =
									handle.session_for_client(*mounted_client_id).await;
								let route =
									routes.get(mounted_client_id).cloned().unwrap_or_else(|| {
										component_route_context("/", &HashMap::new())
									});
								mounted_controller
									.set_runtime_context(Some(*mounted_client_id), mounted_session);
								mounted_controller.set_route_context(Some(route.clone()));
								let item = mounted_controller.render_with_route(&route);
								let title = mounted_controller
									.title()
									.or_else(|| mounted_controller.route_title(&route.path));
								updates.push((*mounted_client_id, item, title));
							}

							for (mounted_client_id, item, title) in updates {
								if let Some(title) = title {
									handle.set_title(mounted_client_id, &title).await;
								}
								handle.render(mounted_client_id, item).await;
							}
							break;
						}
					}
					if handled {
						continue;
					}
					for page in self.pages.iter_mut() {
						if let Some(controller) = page.controllers.get_mut(&client_id) {
							let route = routes
								.get(&client_id)
								.cloned()
								.unwrap_or_else(|| component_route_context("/", &HashMap::new()));
							controller.set_runtime_context(Some(client_id), session.clone());
							controller.set_route_context(Some(route));
							handled = controller.handle(&message.event).await;
						}
						if handled {
							let mut updates: Vec<(usize, Item, Option<String>)> = Vec::new();
							for (mounted_client_id, mounted_controller) in
								page.controllers.iter_mut()
							{
								let mounted_session =
									handle.session_for_client(*mounted_client_id).await;
								let route =
									routes.get(mounted_client_id).cloned().unwrap_or_else(|| {
										component_route_context("/", &HashMap::new())
									});
								mounted_controller
									.set_runtime_context(Some(*mounted_client_id), mounted_session);
								mounted_controller.set_route_context(Some(route.clone()));
								let item = mounted_controller.render_with_route(&route);
								let title = mounted_controller
									.title()
									.or_else(|| mounted_controller.route_title(&route.path));
								updates.push((*mounted_client_id, item, title));
							}

							for (mounted_client_id, item, title) in updates {
								if let Some(title) = title {
									handle.set_title(mounted_client_id, &title).await;
								}
								handle.render(mounted_client_id, item).await;
							}
							break;
						}
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn root_route_is_fallback_match() {
		assert_eq!(component_route_match_score("/", "/"), Some(0));
		assert_eq!(component_route_match_score("/", "/login"), Some(0));
	}

	#[test]
	fn component_route_requires_exact_segment_prefix() {
		assert_eq!(component_route_match_score("/peers", "/peers"), Some(6));
		assert_eq!(component_route_match_score("/peers", "/peers/abc"), Some(6));
		assert_eq!(component_route_match_score("/peers", "/peers-other"), None);
	}

	#[test]
	fn best_matching_route_prefers_specific_route_over_root() {
		let routes = vec!["/".to_string(), "/login".to_string(), "/peers".to_string()];

		assert_eq!(
			best_component_route_index(&routes, "/login", |route| route.as_str()),
			Some(1)
		);
		assert_eq!(
			best_component_route_index(&routes, "/peers/abc", |route| route.as_str()),
			Some(2)
		);
		assert_eq!(
			best_component_route_index(&routes, "/missing", |route| route.as_str()),
			Some(0)
		);
	}
}
