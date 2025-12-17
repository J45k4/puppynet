use crate::auth;
use anyhow::bail;
use libp2p::{Multiaddr, PeerId, swarm::ConnectionId};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const FLAG_READ: u8 = 0x01;
pub const FLAG_WRITE: u8 = 0x02;
pub const FLAG_EXECUTE: u8 = 0x04;
pub const FLAG_SEARCH: u8 = 0x08;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FolderRule {
	path: PathBuf,
	flags: u8,
}

impl FolderRule {
	pub fn new(path: PathBuf, flags: u8) -> Self {
		Self { path, flags }
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	pub fn flags(&self) -> u8 {
		self.flags
	}

	pub fn can_read(&self) -> bool {
		self.flags & FLAG_READ != 0
	}

	pub fn can_write(&self) -> bool {
		self.flags & FLAG_WRITE != 0
	}

	pub fn can_execute(&self) -> bool {
		self.flags & FLAG_EXECUTE != 0
	}

	pub fn can_search(&self) -> bool {
		self.flags & FLAG_SEARCH != 0
	}

	pub fn allows(&self, access: u8) -> bool {
		if access & FLAG_READ != 0 && !self.can_read() {
			return false;
		}
		if access & FLAG_WRITE != 0 && !self.can_write() {
			return false;
		}
		if access & FLAG_EXECUTE != 0 && !self.can_execute() {
			return false;
		}
		if access & FLAG_SEARCH != 0 && !self.can_search() {
			return false;
		}
		true
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Rule {
	Owner,
	Folder(FolderRule),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Permission {
	rule: Rule,
	expires_at: Option<i64>,
}

impl Permission {
	pub fn new(rule: Rule) -> Self {
		Self {
			rule,
			expires_at: None,
		}
	}

	pub fn with_expiration(rule: Rule, expires_at: Option<i64>) -> Self {
		Self { rule, expires_at }
	}

	pub fn rule(&self) -> &Rule {
		&self.rule
	}

	pub fn expires_at(&self) -> Option<i64> {
		self.expires_at
	}
}

#[derive(Clone, Debug)]
pub struct Relationship {
	src: PeerId,
	target: PeerId,
	rules: Vec<Permission>,
}

pub struct TokenAuth {
	token: String,
}

#[derive(Clone, Debug)]
pub enum AuthMethod {
	Token { token: String },
	Credentials { username: String, password: String },
}

#[derive(Clone, Debug)]
pub struct Auth {
	method: AuthMethod,
	expires_at: Option<i64>,
	rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
pub struct Connection {
	pub peer_id: PeerId,
	pub connection_id: ConnectionId,
}

#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
	pub peer_id: PeerId,
	pub multiaddr: Multiaddr,
}

#[derive(Clone, Debug)]
pub struct Peer {
	pub id: PeerId,
	pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct User {
	pub name: String,
	pub passw: String,
}

#[derive(Clone, Debug)]
pub struct State {
	pub me: PeerId,
	pub relationships: Vec<Relationship>,
	pub auths: Vec<Auth>,
	pub connections: Vec<Connection>,
	pub discovered_peers: Vec<DiscoveredPeer>,
	pub peers: Vec<Peer>,
	pub users: Vec<User>,
	pub shared_folders: Vec<FolderRule>,
	dirty_permission_targets: HashSet<PeerId>,
}

impl Default for State {
	fn default() -> Self {
		Self {
			me: PeerId::random(),
			relationships: Vec::new(),
			auths: Vec::new(),
			connections: Vec::new(),
			discovered_peers: Vec::new(),
			peers: Vec::new(),
			users: Vec::new(),
			shared_folders: Vec::new(),
			dirty_permission_targets: HashSet::new(),
		}
	}
}

impl State {
	pub fn authenticate(&mut self, peer_id: PeerId, method: AuthMethod) {}

	pub fn add_shared_folder(&mut self, rule: FolderRule) {
		self.shared_folders.push(rule);
	}

	pub fn permissions_for_peer(&self, peer_id: &PeerId) -> Vec<Permission> {
		let mut permissions: Vec<Permission> = self
			.shared_folders
			.iter()
			.map(|rule| Permission {
				rule: Rule::Folder(rule.clone()),
				expires_at: None,
			})
			.collect();
		for relationship in &self.relationships {
			if relationship.src == *peer_id || relationship.target == *peer_id {
				permissions.extend(relationship.rules.iter().cloned());
			}
		}
		permissions
	}

	pub fn permissions_granted_to_peer(&self, peer_id: &PeerId) -> Vec<Permission> {
		self.relationships
			.iter()
			.filter(|relationship| relationship.src == self.me && relationship.target == *peer_id)
			.flat_map(|relationship| relationship.rules.iter().cloned())
			.collect()
	}

	pub fn has_fs_access(&self, src: PeerId, path: &Path, access: u8) -> bool {
		if src == self.me {
			return true;
		}

		for rule in &self.shared_folders {
			if path.starts_with(rule.path()) && rule.allows(access) {
				return true;
			}
		}

		for rel in &self.relationships {
			if rel.src == src || rel.target == src {
				for rule in &rel.rules {
					match &rule.rule {
						Rule::Owner => {
							return true;
						}
						Rule::Folder(folder_rule) => {
							if path.starts_with(folder_rule.path()) && folder_rule.allows(access) {
								return true;
							}
						}
					}
				}
			}
		}

		false
	}

	pub fn set_peer_permissions(&mut self, peer_id: PeerId, permissions: Vec<Permission>) {
		let me = self.me;
		self.dirty_permission_targets.insert(peer_id);
		self.relationships.retain(|rel| {
			!(rel.src == me && rel.target == peer_id) && !(rel.src == peer_id && rel.target == me)
		});
		if permissions.is_empty() {
			return;
		}
		self.relationships.push(Relationship {
			src: me,
			target: peer_id,
			rules: permissions,
		});
	}

	pub fn set_peer_permissions_from_storage(
		&mut self,
		peer_id: PeerId,
		permissions: Vec<Permission>,
	) {
		let me = self.me;
		self.relationships.retain(|rel| {
			!(rel.src == me && rel.target == peer_id) && !(rel.src == peer_id && rel.target == me)
		});
		if permissions.is_empty() {
			return;
		}
		self.relationships.push(Relationship {
			src: me,
			target: peer_id,
			rules: permissions,
		});
	}

	pub fn save_changes(&mut self) -> anyhow::Result<()> {
		if self.dirty_permission_targets.is_empty() {
			return Ok(());
		}

		let mut conn = crate::db::open_db();
		let me = self.me;

		for peer_id in self.dirty_permission_targets.drain() {
			let permissions = self
				.relationships
				.iter()
				.find(|rel| rel.src == me && rel.target == peer_id)
				.map(|rel| rel.rules.clone())
				.unwrap_or_default();

			crate::db::save_peer_permissions(&mut conn, &me, &peer_id, &permissions)?;
		}

		Ok(())
	}

	pub fn peer_discovered(&mut self, peer_id: PeerId, multiaddr: Multiaddr) {
		if self
			.discovered_peers
			.iter()
			.any(|p| p.peer_id == peer_id && p.multiaddr == multiaddr)
		{
			return;
		}
		self.discovered_peers
			.push(DiscoveredPeer { peer_id, multiaddr });
	}

	pub fn peer_expired(&mut self, peer_id: PeerId, multiaddr: Multiaddr) {
		self.discovered_peers
			.retain(|p| !(p.peer_id == peer_id && p.multiaddr == multiaddr));
	}

	pub fn create_user(&mut self, username: String, password: String) -> anyhow::Result<()> {
		if self.users.iter().any(|u| u.name == username) {
			bail!("User already exists");
		}
		let passw = auth::hash_password(&password)?;
		self.users.push(User {
			name: username,
			passw,
		});
		Ok(())
	}
}
