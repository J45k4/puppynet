use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crossterm::{
	event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
	execute,
	terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use libp2p::PeerId;
use puppynet_core::{
	PuppyNet, State,
	p2p::{CpuInfo, DirEntry},
};
use ratatui::{
	Frame, Terminal,
	backend::CrosstermBackend,
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	widgets::{
		Block, Borders, List, ListItem, ListState, Paragraph, Wrap,
	},
};
const LOCAL_LISTEN_MULTIADDR: &str = "/ip4/0.0.0.0:8336";

enum Mode {
	Menu,
	Peers(PeersView),
	PeerActions(PeerActionsState),
	PeerCpus(PeerCpuView),
	FileBrowser(FileBrowserView),
	CreateUser(CreateUserForm),
}

struct PeersView {
	peers: Vec<PeerRow>,
	selected: usize,
}

impl PeersView {
	fn new() -> Self {
		Self {
			peers: Vec::new(),
			selected: 0,
		}
	}
	fn next(&mut self) {
		if !self.peers.is_empty() {
			self.selected = (self.selected + 1) % self.peers.len();
		}
	}
	fn previous(&mut self) {
		if !self.peers.is_empty() {
			if self.selected == 0 {
				self.selected = self.peers.len() - 1;
			} else {
				self.selected -= 1;
			}
		}
	}
	fn set_peers(&mut self, peers: Vec<PeerRow>) {
		self.peers = peers;
		if self.selected >= self.peers.len() {
			self.selected = 0;
		}
	}
}

impl Default for PeersView {
	fn default() -> Self {
		Self::new()
	}
}

#[derive(Clone)]
struct PeerRow {
	id: String,
	address: String,
	status: String,
}

// Removed placeholder sample peers; UI now populated from live State.

struct PeerActionsState {
	view: PeersView,
	menu: PeerActionsMenu,
}

impl PeerActionsState {
	fn new(view: PeersView, peer: PeerRow) -> Self {
		Self {
			menu: PeerActionsMenu::new(peer),
			view,
		}
	}

	fn take_view(&mut self) -> PeersView {
		std::mem::take(&mut self.view)
	}

	fn ensure_selected_peer(&mut self) {
		if let Some(peer) = self.view.peers.get(self.view.selected).cloned() {
			self.menu.update_peer(peer);
		}
	}
}

struct PeerActionsMenu {
	peer: PeerRow,
	items: Vec<&'static str>,
	selected: usize,
}

impl PeerActionsMenu {
	fn new(peer: PeerRow) -> Self {
		Self {
			peer,
			items: vec!["cpu info", "file browser", "back"],
			selected: 0,
		}
	}

	fn next(&mut self) {
		if self.items.is_empty() {
			return;
		}
		self.selected = if self.selected + 1 < self.items.len() {
			self.selected + 1
		} else {
			0
		};
	}

	fn previous(&mut self) {
		if self.items.is_empty() {
			return;
		}
		self.selected = if self.selected == 0 {
			self.items.len().saturating_sub(1)
		} else {
			self.selected - 1
		};
	}

	fn selected_item(&self) -> Option<&'static str> {
		self.items.get(self.selected).copied()
	}

	fn update_peer(&mut self, peer: PeerRow) {
		self.peer = peer;
	}
}

struct FileBrowserView {
	peer_id: String,
	path: String,
	entries: Vec<DirEntry>,
	selected: usize,
	scroll: usize,
	viewport: usize,
}

impl FileBrowserView {
	fn new(peer_id: String, path: String, entries: Vec<DirEntry>) -> Self {
		Self {
			peer_id,
			path,
			entries,
			selected: 0,
			scroll: 0,
			viewport: 1,
		}
	}

	fn next(&mut self) {
		if self.entries.is_empty() {
			return;
		}
		self.selected = if self.selected + 1 < self.entries.len() {
			self.selected + 1
		} else {
			self.scroll = 0;
			0
		};
		self.clamp_scroll();
	}

	fn previous(&mut self) {
		if self.entries.is_empty() {
			return;
		}
		self.selected = if self.selected == 0 {
			let last = self.entries.len().saturating_sub(1);
			self.scroll = self.entries.len().saturating_sub(self.viewport);
			last
		} else {
			self.selected - 1
		};
		self.clamp_scroll();
	}

	fn selected_entry(&self) -> Option<&DirEntry> {
		self.entries.get(self.selected)
	}

	fn set_viewport(&mut self, viewport: usize) {
		self.viewport = viewport.max(1);
		self.clamp_scroll();
	}

	fn clamp_scroll(&mut self) {
		if self.entries.is_empty() {
			self.selected = 0;
			self.scroll = 0;
			return;
		}
		if self.selected >= self.entries.len() {
			self.selected = self.entries.len().saturating_sub(1);
		}
		let window = self.viewport.min(self.entries.len());
		if window == 0 {
			self.scroll = 0;
			return;
		}
		let max_scroll = self.entries.len().saturating_sub(window);
		if self.selected < self.scroll {
			self.scroll = self.selected;
		} else if self.selected >= self.scroll + window {
			self.scroll = self.selected + 1 - window;
		}
		if self.scroll > max_scroll {
			self.scroll = max_scroll;
		}
	}

	fn replace_entries(&mut self, path: String, entries: Vec<DirEntry>) {
		self.path = path;
		self.entries = entries;
		self.selected = 0;
		self.scroll = 0;
		self.clamp_scroll();
	}
}

struct PeerCpuView {
	peer_id: String,
	cpus: Vec<CpuInfo>,
	selected: usize,
	scroll: usize,
	viewport: usize,
	last_refresh: Instant,
}

impl PeerCpuView {
	fn new(peer_id: String, cpus: Vec<CpuInfo>) -> Self {
		let mut view = Self {
			peer_id,
			cpus: Vec::new(),
			selected: 0,
			scroll: 0,
			viewport: 1,
			last_refresh: Instant::now(),
		};
		view.replace_cpus(cpus);
		view
	}

	fn next(&mut self) {
		if self.cpus.is_empty() {
			return;
		}
		self.selected = if self.selected + 1 < self.cpus.len() {
			self.selected + 1
		} else {
			self.scroll = 0;
			0
		};
		self.clamp_scroll();
	}

	fn previous(&mut self) {
		if self.cpus.is_empty() {
			return;
		}
		self.selected = if self.selected == 0 {
			let last = self.cpus.len().saturating_sub(1);
			self.scroll = self.cpus.len().saturating_sub(self.viewport);
			last
		} else {
			self.selected - 1
		};
		self.clamp_scroll();
	}

	fn selected_cpu(&self) -> Option<&CpuInfo> {
		self.cpus.get(self.selected)
	}

	fn set_viewport(&mut self, viewport: usize) {
		self.viewport = viewport.max(1);
		self.clamp_scroll();
	}

	fn clamp_scroll(&mut self) {
		if self.cpus.is_empty() {
			self.selected = 0;
			self.scroll = 0;
			return;
		}
		if self.selected >= self.cpus.len() {
			self.selected = self.cpus.len().saturating_sub(1);
		}
		let window = self.viewport.min(self.cpus.len());
		if window == 0 {
			self.scroll = 0;
			return;
		}
		let max_scroll = self.cpus.len().saturating_sub(window);
		if self.selected < self.scroll {
			self.scroll = self.selected;
		} else if self.selected >= self.scroll + window {
			self.scroll = self.selected + 1 - window;
		}
		if self.scroll > max_scroll {
			self.scroll = max_scroll;
		}
	}

	fn replace_cpus(&mut self, cpus: Vec<CpuInfo>) {
		self.cpus = cpus;
		if self.cpus.is_empty() {
			self.selected = 0;
			self.scroll = 0;
		}
		self.clamp_scroll();
		self.mark_refreshed();
	}

	fn mark_refreshed(&mut self) {
		self.last_refresh = Instant::now();
	}
}

struct CreateUserForm {
	username: String,
	password: String,
	field: ActiveField,
	submitted: bool,
}

impl CreateUserForm {
	fn new() -> Self {
		Self {
			username: String::new(),
			password: String::new(),
			field: ActiveField::Username,
			submitted: false,
		}
	}
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ActiveField {
	Username,
	Password,
}

struct ShellApp {
	should_quit: bool,
	menu_items: Vec<&'static str>,
	menu_state: ListState,
	status_line: String,
	mode: Mode,
	peer: PuppyNet,
	last_refresh: Instant,
	refresh_interval: Duration,
	refresh_count: u64,
	latest_state: Option<State>,
}

impl ShellApp {
	fn new() -> Self {
		let mut state = ListState::default();
		state.select(Some(0));
		Self {
			should_quit: false,
			menu_items: vec![
				"peers",
				"create token",
				"create user",
				"quit",
			],
			menu_state: state,
			status_line: "Use ↑/↓ to navigate, Enter to select, q to quit".to_string(),
			mode: Mode::Menu,
			peer: PuppyNet::new(),
			last_refresh: Instant::now(),
			refresh_interval: Duration::from_secs(5),
			refresh_count: 0,
			latest_state: None,
		}
	}

	fn next(&mut self) {
		let i = match self.menu_state.selected() {
			Some(i) => {
				if i >= self.menu_items.len() - 1 {
					0
				} else {
					i + 1
				}
			}
			None => 0,
		};
		self.menu_state.select(Some(i));
	}

	fn previous(&mut self) {
		let i = match self.menu_state.selected() {
			Some(i) => {
				if i == 0 {
					self.menu_items.len() - 1
				} else {
					i - 1
				}
			}
			None => 0,
		};
		self.menu_state.select(Some(i));
	}

	fn activate(&mut self) {
		if let Mode::Menu = self.mode {
			if let Some(i) = self.menu_state.selected() {
				match self.menu_items[i] {
					"quit" => self.should_quit = true,
					"peers" => {
						self.mode = Mode::Peers(PeersView::new());
						self.status_line =
							"Peers view. Auto-refresh every 5s. ↑/↓ navigate, Esc back".into();
					}
					"create token" => {
						self.status_line = "Token created (placeholder)".into();
					}
					"create user" => {
						self.mode = Mode::CreateUser(CreateUserForm::new());
						self.status_line = "Enter username/password, Tab to switch field, Enter to submit, Esc to cancel".into();
					}
					_ => {}
				}
			}
		}
	}

	fn handle_event(&mut self, event: Event) {
		if let Event::Key(key) = event {
			let mut next_mode: Option<Mode> = None;
			let mut pending_peer_actions: Option<String> = None;
			match &mut self.mode {
				Mode::Menu => match key.code {
					KeyCode::Char('q') => self.should_quit = true,
					KeyCode::Down => self.next(),
					KeyCode::Up => self.previous(),
					KeyCode::Enter => self.activate(),
					_ => {}
				},
				Mode::Peers(view) => match key.code {
					KeyCode::Esc => {
						self.mode = Mode::Menu;
						self.status_line = "Back to menu".into();
					}
					KeyCode::Down => view.next(),
					KeyCode::Up => view.previous(),
					KeyCode::Enter => {
						if let Some(peer) = view.peers.get(view.selected).cloned() {
							let snapshot = std::mem::take(view);
							self.status_line = format!(
								"Peer actions for {}. ↑/↓ navigate, Enter select, Esc back",
								peer.id
							);
							next_mode =
								Some(Mode::PeerActions(PeerActionsState::new(snapshot, peer)));
						}
					}
					KeyCode::Char('r') => {}
					KeyCode::Char('q') => {
						self.should_quit = true;
					}
					_ => {}
				},
				Mode::PeerActions(state) => match key.code {
					KeyCode::Esc => {
						let peer_id = state.menu.peer.id.clone();
						self.status_line = format!("Returning from actions for {}", peer_id);
						next_mode = Some(Mode::Peers(state.take_view()));
					}
					KeyCode::Down => state.menu.next(),
					KeyCode::Up => state.menu.previous(),
					KeyCode::Enter => match state.menu.selected_item() {
						Some("cpu info") => {
							let peer_id = state.menu.peer.id.clone();
							match self.create_cpu_view(peer_id.clone()) {
								Ok(view) => {
									self.status_line = Self::cpu_summary(&view);
									next_mode = Some(Mode::PeerCpus(view));
								}
								Err(err) => {
									self.status_line = format!("Failed to fetch CPUs: {}", err);
								}
							}
						}
						Some("file browser") => {
							let peer_id = state.menu.peer.id.clone();
							match self.create_file_browser_view(peer_id.clone(), "/") {
								Ok(view) => {
									self.status_line = format!("Browsing / on {}", peer_id);
									next_mode = Some(Mode::FileBrowser(view));
								}
								Err(err) => {
									self.status_line =
										format!("Failed to list root directory: {}", err);
								}
							}
						}
						Some("back") => {
							let peer_id = state.menu.peer.id.clone();
							self.status_line = format!("Returning from actions for {}", peer_id);
							next_mode = Some(Mode::Peers(state.take_view()));
						}
						_ => {}
					},
					KeyCode::Char('q') => {
						self.should_quit = true;
					}
					KeyCode::Char('r') => {}
					_ => {}
				},
				Mode::PeerCpus(view) => match key.code {
					KeyCode::Esc => {
						pending_peer_actions = Some(view.peer_id.clone());
					}
					KeyCode::Down => {
						view.next();
						self.status_line = Self::cpu_summary(view);
					}
					KeyCode::Up => {
						view.previous();
						self.status_line = Self::cpu_summary(view);
					}
					KeyCode::Char('q') => {
						self.should_quit = true;
					}
					_ => {}
				},
				Mode::FileBrowser(view) => match key.code {
					KeyCode::Esc => {
						pending_peer_actions = Some(view.peer_id.clone());
					}
					KeyCode::Down => view.next(),
					KeyCode::Up => view.previous(),
					KeyCode::Enter => {
						if let Some(entry) = view.selected_entry().cloned() {
							if entry.is_dir {
								let peer_id = view.peer_id.clone();
								let target = join_child_path(&view.path, &entry.name);
								match self
									.peer
									.list_dir_blocking(peer_id.parse().unwrap(), target.clone())
								{
									Ok(entries) => {
										view.replace_entries(target.clone(), entries);
										self.status_line =
											format!("Browsing {} on {}", target, peer_id);
									}
									Err(err) => {
										self.status_line =
											format!("Failed to open {}: {}", target, err);
									}
								}
							} else {
								self.status_line = format!(
									"Selected file {} ({}). Enter directories to navigate",
									entry.name,
									format_size(entry.size)
								);
							}
						}
					}
					KeyCode::Backspace | KeyCode::Left => {
						let parent = parent_path(&view.path);
						if parent != view.path {
							let peer_id = view.peer_id.clone();
							match self
								.peer
								.list_dir_blocking(peer_id.parse().unwrap(), parent.clone())
							{
								Ok(entries) => {
									view.replace_entries(parent.clone(), entries);
									self.status_line =
										format!("Browsing {} on {}", parent, peer_id);
								}
								Err(err) => {
									self.status_line =
										format!("Failed to open {}: {}", parent, err);
								}
							}
						}
					}
					KeyCode::Char('q') => {
						self.should_quit = true;
					}
					_ => {}
				},
				Mode::CreateUser(form) => match key.code {
					KeyCode::Esc => {
						self.mode = Mode::Menu;
						self.status_line = "Cancelled create user".into();
					}
					KeyCode::Tab | KeyCode::BackTab => {
						form.field = match form.field {
							ActiveField::Username => ActiveField::Password,
							ActiveField::Password => ActiveField::Username,
						};
					}
					KeyCode::Enter => {
						if !form.username.is_empty() && !form.password.is_empty() {
							form.submitted = true;
							self.status_line =
								format!("Created user '{}' (placeholder)", form.username);
							self.mode = Mode::Menu;
						} else {
							self.status_line = "Both fields required".into();
						}
					}
					KeyCode::Char(c) => match form.field {
						ActiveField::Username => form.username.push(c),
						ActiveField::Password => form.password.push(c),
					},
					KeyCode::Backspace => match form.field {
						ActiveField::Username => {
							form.username.pop();
						}
						ActiveField::Password => {
							form.password.pop();
						}
					},
					KeyCode::Left | KeyCode::Right => {}
					_ => {}
				},
			}
			if let Some(mode) = next_mode {
				self.mode = mode;
			}
			if let Some(peer_id) = pending_peer_actions {
				if let Some((state, status)) = self.peer_actions_state_for(&peer_id) {
					self.status_line = status;
					self.mode = Mode::PeerActions(state);
				} else {
					self.status_line = format!("Peer {} not available", peer_id);
					self.mode = Mode::Menu;
				}
			}
		}
	}

	fn peer_actions_state_for(&self, peer_id: &str) -> Option<(PeerActionsState, String)> {
		let state = self.latest_state.as_ref()?;
		let aggregated = Self::aggregate_peers(state);
		let mut view = PeersView::new();
		view.set_peers(aggregated.clone());
		if view.peers.is_empty() {
			return None;
		}
		if let Some(idx) = view.peers.iter().position(|p| p.id == peer_id) {
			view.selected = idx;
		}
		let selected = view.peers.get(view.selected)?.clone();
		let mut actions = PeerActionsState::new(view, selected.clone());
		actions.ensure_selected_peer();
		Some((actions, format!("Peer actions for {}", selected.id)))
	}

	fn create_file_browser_view(&self, peer_id: String, path: &str) -> Result<FileBrowserView> {
		let entries = self
			.peer
			.list_dir_blocking(peer_id.parse()?, path.to_string())
			.with_context(|| format!("listing {} locally", path))?;
		Ok(FileBrowserView::new(peer_id, path.to_string(), entries))
	}

	fn create_cpu_view(&self, peer_id: String) -> Result<PeerCpuView> {
		let cpus = self.peer.list_cpus_blocking(peer_id.parse()?)?;
		Ok(PeerCpuView::new(peer_id, cpus))
	}

	fn cpu_summary(view: &PeerCpuView) -> String {
		view.selected_cpu()
			.map(|cpu| {
				format!(
					"{}: {:.1}% @ {}",
					cpu.name,
					cpu.usage,
					format_frequency(cpu.frequency_hz)
				)
			})
			.unwrap_or_else(|| format!("No CPUs reported for {}", view.peer_id))
	}

	fn render(&mut self, f: &mut Frame<'_>) {
		let size = f.size();
		let columns = Layout::default()
			.direction(Direction::Horizontal)
			.margin(1)
			.constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
			.split(size);
		let main_area = columns[0];
		let info_area = columns[1];

		match &mut self.mode {
			Mode::Menu => {
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title / help
						Constraint::Min(5),    // menu list
						Constraint::Length(1), // status line
					])
					.split(main_area);

				let header = Paragraph::new("PuppyNet")
					.style(Style::default().fg(Color::Yellow))
					.block(Block::default().borders(Borders::ALL).title("Header"));
				f.render_widget(header, chunks[0]);

				let items: Vec<ListItem> =
					self.menu_items.iter().map(|m| ListItem::new(*m)).collect();
				let list = List::new(items)
					.block(Block::default().borders(Borders::ALL).title("Menu"))
					.highlight_style(
						Style::default()
							.fg(Color::Cyan)
							.add_modifier(Modifier::BOLD | Modifier::REVERSED),
					)
					.highlight_symbol("▶ ");
				f.render_stateful_widget(list, chunks[1], &mut self.menu_state);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
			Mode::PeerActions(state) => {
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title
						Constraint::Min(5),    // actions list
						Constraint::Length(1), // status
					])
					.split(main_area);

				let title = format!("Actions for {}", state.menu.peer.id);
				let header = Paragraph::new(title)
					.style(Style::default().fg(Color::Green))
					.block(Block::default().borders(Borders::ALL).title("Header"));
				f.render_widget(header, chunks[0]);

				let items: Vec<ListItem> = state
					.menu
					.items
					.iter()
					.enumerate()
					.map(|(idx, item)| {
						let style = if idx == state.menu.selected {
							Style::default()
								.fg(Color::Cyan)
								.add_modifier(Modifier::BOLD)
						} else {
							Style::default()
						};
						let prefix = if idx == state.menu.selected {
							"▶ "
						} else {
							"  "
						};
						ListItem::new(format!("{}{}", prefix, item)).style(style)
					})
					.collect();

				let list = List::new(items).block(
					Block::default()
						.borders(Borders::ALL)
						.title("Peer Actions (Enter select, Esc back)"),
				);
				f.render_widget(list, chunks[1]);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
			Mode::PeerCpus(view) => {
				use ratatui::widgets::{Row, Table};
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title
						Constraint::Min(5),    // table
						Constraint::Length(1), // status
					])
					.split(main_area);

				let header = Paragraph::new("CPU Inventory")
					.style(Style::default().fg(Color::Magenta))
					.block(
						Block::default()
							.borders(Borders::ALL)
							.title(format!("Peer: {}", view.peer_id)),
					);
				f.render_widget(header, chunks[0]);

				let viewport = if chunks[1].height > 1 {
					(chunks[1].height - 1) as usize
				} else {
					1
				};
				view.set_viewport(viewport);

				let header_row = Row::new(vec!["Idx", "CPU", "Usage", "Frequency"])
					.style(Style::default().add_modifier(Modifier::BOLD));
				let rows: Vec<Row> = view
					.cpus
					.iter()
					.enumerate()
					.skip(view.scroll)
					.take(view.viewport)
					.map(|(idx, cpu)| {
						let style = if idx == view.selected {
							Style::default().fg(Color::Cyan)
						} else {
							Style::default()
						};
						Row::new(vec![
							format!("{}", idx),
							cpu.name.clone(),
							format!("{:.1}%", cpu.usage),
							format_frequency(cpu.frequency_hz),
						])
						.style(style)
					})
					.collect();

				let widths = [
					Constraint::Length(4),
					Constraint::Percentage(50),
					Constraint::Length(10),
					Constraint::Length(12),
				];

				let table = Table::new(rows, &widths)
					.header(header_row)
					.block(
						Block::default()
							.borders(Borders::ALL)
							.title("CPUs (↑/↓ scroll, Esc=back)"),
					)
					.highlight_style(Style::default().add_modifier(Modifier::REVERSED));
				f.render_widget(table, chunks[1]);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
			Mode::FileBrowser(view) => {
				use ratatui::widgets::{Row, Table};
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title
						Constraint::Min(5),    // table
						Constraint::Length(1), // status
					])
					.split(main_area);

				let header = Paragraph::new(format!("File Browser — {}", view.path))
					.style(Style::default().fg(Color::Blue))
					.block(
						Block::default()
							.borders(Borders::ALL)
							.title(format!("Peer: {}", view.peer_id)),
					);
				f.render_widget(header, chunks[0]);

				let viewport = if chunks[1].height > 1 {
					(chunks[1].height - 1) as usize
				} else {
					1
				};
				view.set_viewport(viewport);

				let header_row = Row::new(vec!["Idx", "Name", "Type", "Size"])
					.style(Style::default().add_modifier(Modifier::BOLD));
				let rows: Vec<Row> = view
					.entries
					.iter()
					.enumerate()
					.skip(view.scroll)
					.take(view.viewport)
					.map(|(idx, entry)| {
						let style = if idx == view.selected {
							Style::default().fg(Color::Cyan)
						} else {
							Style::default()
						};
						let display_name = if entry.is_dir {
							format!("{}/", entry.name)
						} else {
							entry.name.clone()
						};
						let entry_type = if entry.is_dir { "dir" } else { "file" };
						Row::new(vec![
							format!("{}", idx),
							display_name,
							entry_type.into(),
							format_size(entry.size),
						])
						.style(style)
					})
					.collect();

				let widths = [
					Constraint::Length(4),
					Constraint::Percentage(60),
					Constraint::Length(6),
					Constraint::Length(12),
				];

				let table = Table::new(rows, &widths)
					.header(header_row)
					.block(
						Block::default()
							.borders(Borders::ALL)
							.title("Files (Enter=open, Backspace=up, Esc=back)"),
					)
					.highlight_style(Style::default().add_modifier(Modifier::REVERSED));
				f.render_widget(table, chunks[1]);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
			Mode::Peers(view) => {
				use ratatui::widgets::{Row, Table};
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title
						Constraint::Min(5),    // table
						Constraint::Length(1), // status
					])
					.split(main_area);

				let header = Paragraph::new("Peers")
					.style(Style::default().fg(Color::Green))
					.block(Block::default().borders(Borders::ALL).title("Header"));
				f.render_widget(header, chunks[0]);

				let header_row = Row::new(vec!["Idx", "Peer ID", "Address", "Status"])
					.style(Style::default().add_modifier(Modifier::BOLD));
				let rows: Vec<Row> = view
					.peers
					.iter()
					.enumerate()
					.map(|(i, p)| {
						let style = if i == view.selected {
							Style::default().fg(Color::Cyan)
						} else {
							Style::default()
						};
						Row::new(vec![
							format!("{}", i),
							p.id.clone(),
							p.address.clone(),
							p.status.clone(),
						])
						.style(style)
					})
					.collect();

				let widths = [
					Constraint::Length(4),
					Constraint::Length(16),
					Constraint::Percentage(50),
					Constraint::Length(12),
				];
				let table = Table::new(rows, &widths)
					.header(header_row)
					.block(
						Block::default()
							.borders(Borders::ALL)
							.title("Peers (r=refresh, Esc=back)"),
					)
					.highlight_style(Style::default().add_modifier(Modifier::REVERSED));
				f.render_widget(table, chunks[1]);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
			Mode::CreateUser(form) => {
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.constraints([
						Constraint::Length(3), // title
						Constraint::Min(5),    // form
						Constraint::Length(1), // status
					])
					.split(main_area);

				let header = Paragraph::new("Create User")
					.style(Style::default().fg(Color::Magenta))
					.block(Block::default().borders(Borders::ALL).title("Header"));
				f.render_widget(header, chunks[0]);

				let form_chunks = Layout::default()
					.direction(Direction::Vertical)
					.margin(1)
					.constraints([
						Constraint::Length(3),
						Constraint::Length(3),
						Constraint::Min(1),
					])
					.split(chunks[1]);

				let username_label = format!("Username: {}", form.username);
				let password_mask: String = "*".repeat(form.password.len());
				let password_label = format!("Password: {}", password_mask);

				let username_title = match form.field {
					ActiveField::Username => "[Username]*",
					ActiveField::Password => "Username",
				};
				let password_title = match form.field {
					ActiveField::Password => "[Password]*",
					ActiveField::Username => "Password",
				};

				let username_style = if form.field == ActiveField::Username {
					Style::default().fg(Color::Cyan)
				} else {
					Style::default()
				};
				let password_style = if form.field == ActiveField::Password {
					Style::default().fg(Color::Cyan)
				} else {
					Style::default()
				};

				let username_para = Paragraph::new(username_label)
					.style(username_style)
					.block(Block::default().borders(Borders::ALL).title(username_title))
					.wrap(Wrap { trim: true });

				let password_para = Paragraph::new(password_label)
					.style(password_style)
					.block(Block::default().borders(Borders::ALL).title(password_title))
					.wrap(Wrap { trim: true });

				let help = Paragraph::new("Tab: switch field | Enter: submit | Esc: cancel")
					.block(Block::default().borders(Borders::ALL).title("Help"));

				f.render_widget(username_para, form_chunks[0]);
				f.render_widget(password_para, form_chunks[1]);
				f.render_widget(help, form_chunks[2]);

				let status = Paragraph::new(self.status_line.as_str())
					.block(Block::default().borders(Borders::ALL).title("Status"));
				f.render_widget(status, chunks[2]);
			}
		}

		render_peer_info(f, info_area, self);
	}

	fn periodic_refresh(&mut self) {
		if self.last_refresh.elapsed() >= self.refresh_interval {
			// Pull latest core state (Arc<Mutex<State>>) via instance and take a snapshot clone
			let state_arc = self.peer.state();
			let snapshot = state_arc.lock().ok().map(|s| s.clone());
			if let Some(state) = snapshot.clone() {
				self.latest_state = Some(state);
			}
			// Update active views from snapshot (if open)
			if let Some(state) = snapshot {
				let aggregated = Self::aggregate_peers(&state);
				match &mut self.mode {
					Mode::Peers(view) => {
						view.set_peers(aggregated.clone());
						self.status_line =
							format!("Auto-refreshed peers ({} entries)", view.peers.len());
					}
					Mode::PeerActions(state) => {
						state.view.set_peers(aggregated.clone());
						if let Some(idx) = state
							.view
							.peers
							.iter()
							.position(|p| p.id == state.menu.peer.id)
						{
							state.view.selected = idx;
						} else if !state.view.peers.is_empty() {
							if state.view.selected >= state.view.peers.len() {
								state.view.selected = state.view.peers.len() - 1;
							}
						}
						state.ensure_selected_peer();
						self.status_line = format!(
							"Auto-refreshed peer actions ({} peers)",
							state.view.peers.len()
						);
					}
					Mode::PeerCpus(view) => {
						if view.last_refresh.elapsed() >= self.refresh_interval {
							match self.peer.list_cpus_blocking(view.peer_id.parse().unwrap()) {
								Ok(cpus) => {
									view.replace_cpus(cpus);
									let headline = Self::cpu_summary(view);
									self.status_line = format!("Refreshed CPUs — {}", headline);
								}
								Err(err) => {
									view.mark_refreshed();
									self.status_line =
										format!("CPU refresh failed for {}: {}", view.peer_id, err);
								}
							}
						}
					}
					_ => {}
				}
			} else {
				self.status_line = "Auto-refresh failed to lock state".into();
			}
			// legacy post-refresh per-mode adjustments removed (state-based updates already applied)
			self.refresh_count += 1;
			self.last_refresh = Instant::now();
		}
	}

	fn aggregate_peers(state: &State) -> Vec<PeerRow> {
		// Map peer_id -> (address (first), status)
		let mut rows: HashMap<String, PeerRow> = HashMap::new();
		// Discovered peers (addresses)
		for d in &state.discovered_peers {
			let id_str = format!("{}", d.peer_id);
			rows.entry(id_str.clone())
				.and_modify(|r| {
					if r.address.is_empty() {
						r.address = d.multiaddr.to_string();
					}
				})
				.or_insert(PeerRow {
					id: id_str,
					address: d.multiaddr.to_string(),
					status: "discovered".into(),
				});
		}
		// Connections override status
		for c in &state.connections {
			let id_str = format!("{}", c.peer_id);
			rows.entry(id_str.clone())
				.and_modify(|r| {
					r.status = "connected".into();
				})
				.or_insert(PeerRow {
					id: id_str,
					address: String::new(),
					status: "connected".into(),
				});
		}
		// Explicit peers list (metadata like names) ensure presence
		for p in &state.peers {
			let id_str = format!("{}", p.id);
			rows.entry(id_str.clone()).or_insert(PeerRow {
				id: id_str,
				address: String::new(),
				status: String::new(),
			});
		}
		let me_id = format!("{}", state.me);
		rows.entry(me_id.clone())
			.and_modify(|r| {
				if r.address.is_empty() {
					r.address = LOCAL_LISTEN_MULTIADDR.into();
				}
				r.status = "local".into();
			})
			.or_insert(PeerRow {
				id: me_id,
				address: LOCAL_LISTEN_MULTIADDR.into(),
				status: "local".into(),
			});
		let mut vec: Vec<PeerRow> = rows.into_iter().map(|(_, v)| v).collect();
		vec.sort_by(|a, b| a.id.cmp(&b.id));
		vec
	}

	fn gather_known_addresses(&self, peer_id: &str) -> Vec<String> {
		if let Some(state) = &self.latest_state {
			if let Ok(target) = PeerId::from_str(peer_id) {
				let mut addresses = Vec::new();
				for discovered in &state.discovered_peers {
					if discovered.peer_id == target {
						addresses.push(discovered.multiaddr.to_string());
					}
				}
				addresses
			} else {
				Vec::new()
			}
		} else {
			Vec::new()
		}
	}

	fn peer_panel_content(&self) -> (String, Vec<String>) {
		match &self.mode {
			Mode::Peers(view) if !view.peers.is_empty() => {
				let peer = &view.peers[view.selected];
				let mut lines = Vec::new();
				lines.push(format!("Peer ID: {}", peer.id));
				let mut addresses = Vec::new();
				if !peer.address.is_empty() {
					addresses.push(peer.address.clone());
				}
				for addr in self.gather_known_addresses(&peer.id) {
					if !addresses.contains(&addr) {
						addresses.push(addr);
					}
				}
				match addresses.len() {
					0 => lines.push("Dial Address: unknown".into()),
					1 => lines.push(format!("Dial Address: {}", addresses[0])),
					_ => {
						lines.push("Dial Addresses:".into());
						for (idx, addr) in addresses.iter().enumerate() {
							lines.push(format!("{}: {}", idx + 1, addr));
						}
					}
				}
				if !peer.status.is_empty() {
					lines.push(format!("Status: {}", peer.status));
				}
				("Selected Peer".into(), lines)
			}
			Mode::PeerActions(state) => {
				let peer = &state.menu.peer;
				let mut lines = Vec::new();
				lines.push(format!("Peer ID: {}", peer.id));
				let mut addresses = Vec::new();
				if !peer.address.is_empty() {
					addresses.push(peer.address.clone());
				}
				for addr in self.gather_known_addresses(&peer.id) {
					if !addresses.contains(&addr) {
						addresses.push(addr);
					}
				}
				match addresses.len() {
					0 => lines.push("Dial Address: unknown".into()),
					1 => lines.push(format!("Dial Address: {}", addresses[0])),
					_ => {
						lines.push("Dial Addresses:".into());
						for (idx, addr) in addresses.iter().enumerate() {
							lines.push(format!("{}: {}", idx + 1, addr));
						}
					}
				}
				if !peer.status.is_empty() {
					lines.push(format!("Status: {}", peer.status));
				}
				("Peer Actions".into(), lines)
			}
			Mode::FileBrowser(view) => {
				let mut lines = Vec::new();
				lines.push(format!("Peer: {}", view.peer_id));
				lines.push(format!("Path: {}", view.path));
				if let Some(entry) = view.selected_entry() {
					lines.push(format!("Name: {}", entry.name));
					lines.push(format!(
						"Type: {}",
						if entry.is_dir { "directory" } else { "file" }
					));
					if let Some(ext) = &entry.extension {
						lines.push(format!("Extension: {}", ext));
					}
					lines.push(format!("Size: {}", format_size(entry.size)));
					if let Some(modified) = entry.modified_at {
						lines.push(format!("Modified: {}", modified.to_rfc3339()));
					}
					if let Some(accessed) = entry.accessed_at {
						lines.push(format!("Accessed: {}", accessed.to_rfc3339()));
					}
				} else {
					lines.push("Directory is empty".into());
				}
				("File Browser".into(), lines)
			}
			Mode::PeerCpus(view) => {
				let mut lines = Vec::new();
				lines.push(format!("Peer: {}", view.peer_id));
				if view.cpus.is_empty() {
					lines.push("No CPU data available".into());
				} else {
					lines.push(format!("Logical CPUs: {}", view.cpus.len()));
					if let Some(cpu) = view.selected_cpu() {
						lines.push(format!(
							"Selected: {} ({:.1}% @ {})",
							cpu.name,
							cpu.usage,
							format_frequency(cpu.frequency_hz)
						));
					}
					for cpu in view.cpus.iter().take(5) {
						lines.push(format!(
							"{} ({:.1}% @ {})",
							cpu.name,
							cpu.usage,
							format_frequency(cpu.frequency_hz)
						));
					}
					if view.cpus.len() > 5 {
						lines.push(format!("(+{} more)", view.cpus.len() - 5));
					}
				}
				("CPU Info".into(), lines)
			}
			_ => {
				if let Some(state) = &self.latest_state {
					let mut lines = Vec::new();
					lines.push(format!("Peer ID: {}", state.me));
					lines.push(format!("Dial Address: {}", LOCAL_LISTEN_MULTIADDR));
					if state.discovered_peers.is_empty() {
						lines.push("Known peers: none".into());
					} else {
						lines.push("Known peers:".into());
						for (idx, peer) in state.discovered_peers.iter().take(5).enumerate() {
							lines.push(format!("{}: {}", idx + 1, peer.multiaddr));
						}
						if state.discovered_peers.len() > 5 {
							lines.push(format!("(+{} more)", state.discovered_peers.len() - 5));
						}
					}
					("Local Peer".into(), lines)
				} else {
					("Peer Info".into(), vec!["Peer state unavailable".into()])
				}
			}
		}
	}
}

pub fn run() -> io::Result<()> {
	enable_raw_mode()?;
	let mut stdout = io::stdout();
	execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	let result = run_app(&mut terminal);

	restore_terminal(&mut terminal)?;

	result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
	let mut app = ShellApp::new();

	while !app.should_quit {
		app.periodic_refresh();
		terminal.draw(|f| app.render(f))?;

		if event::poll(Duration::from_millis(200))? {
			let event = event::read()?;
			app.handle_event(event);
		}
	}

	Ok(())
}

fn render_peer_info(f: &mut Frame<'_>, area: Rect, app: &ShellApp) {
	if area.width == 0 || area.height == 0 {
		return;
	}
	let (title, lines) = app.peer_panel_content();
	let body = if lines.is_empty() {
		String::from("No peer information available")
	} else {
		lines.join("\n")
	};
	let panel = Paragraph::new(body)
		.block(Block::default().borders(Borders::ALL).title(title))
		.wrap(Wrap { trim: true });
	f.render_widget(panel, area);
}

fn join_child_path(base: &str, child: &str) -> String {
	if base == "/" {
		format!("/{}", child)
	} else {
		Path::new(base).join(child).to_string_lossy().to_string()
	}
}

fn parent_path(path: &str) -> String {
	if path == "/" {
		return "/".into();
	}
	let mut buf = PathBuf::from(path);
	if buf.pop() {
		let parent = buf.to_string_lossy().to_string();
		if parent.is_empty() {
			"/".into()
		} else {
			parent
		}
	} else {
		"/".into()
	}
}

fn format_size(size: u64) -> String {
	const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
	let mut value = size as f64;
	let mut unit_index = 0;
	while value >= 1024.0 && unit_index < UNITS.len() - 1 {
		value /= 1024.0;
		unit_index += 1;
	}
	if unit_index == 0 {
		format!("{} {}", size, UNITS[unit_index])
	} else {
		format!("{:.1} {}", value, UNITS[unit_index])
	}
}

fn format_frequency(freq_mhz: u64) -> String {
	if freq_mhz == 0 {
		return "0 MHz".into();
	}
	if freq_mhz >= 1000 {
		format!("{:.2} GHz", freq_mhz as f64 / 1000.0)
	} else {
		format!("{} MHz", freq_mhz)
	}
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
	disable_raw_mode()?;
	execute!(
		terminal.backend_mut(),
		LeaveAlternateScreen,
		DisableMouseCapture
	)?;
	terminal.show_cursor()?;
	Ok(())
}
