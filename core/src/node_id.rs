use std::fmt;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const ED25519_KEY_TYPE: u64 = 1;
const MULTIHASH_IDENTITY_CODE: u64 = 0;
const MULTIHASH_SHA256_CODE: u64 = 0x12;
const ED25519_PUBLIC_KEY_LEN: usize = 32;
const ED25519_SIGNATURE_LEN: usize = 64;
const ED25519_KEYPAIR_LEN: usize = 64;

/// Cryptographic node identity derived from an Ed25519 public key.
///
/// The string form is byte-compatible with libp2p Ed25519 peer IDs
/// (`base58btc(identity-multihash(protobuf(pubkey)))`) so existing
/// permission grants keyed by peer ID keep working.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
	/// Derive the node ID from a raw Ed25519 public key.
	pub fn from_public_key(public: &[u8; ED25519_PUBLIC_KEY_LEN]) -> NodeId {
		let key_enc = encode_public_key_protobuf(public);
		let mut multihash = Vec::with_capacity(2 + key_enc.len());
		encode_varint(MULTIHASH_IDENTITY_CODE, &mut multihash);
		encode_varint(key_enc.len() as u64, &mut multihash);
		multihash.extend_from_slice(&key_enc);
		NodeId(bs58::encode(&multihash).into_string())
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	/// Raw multihash bytes used by existing peer permission rows.
	pub fn to_bytes(&self) -> Vec<u8> {
		bs58::decode(&self.0)
			.into_vec()
			.expect("NodeId is validated at construction")
	}

	/// Decode the raw multihash bytes stored by earlier PuppyNet releases.
	pub fn from_bytes(bytes: &[u8]) -> Result<NodeId> {
		parse_multihash(bytes)?;
		let value = bs58::encode(bytes).into_string();
		value.parse()
	}

	pub fn random() -> NodeId {
		NodeKeypair::generate().node_id()
	}
}

impl fmt::Display for NodeId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl FromStr for NodeId {
	type Err = anyhow::Error;

	fn from_str(value: &str) -> Result<Self> {
		let multihash = bs58::decode(value)
			.into_vec()
			.map_err(|err| anyhow!("invalid node id base58: {err}"))?;
		let code = parse_multihash(&multihash)?;
		if code != MULTIHASH_IDENTITY_CODE && code != MULTIHASH_SHA256_CODE {
			return Err(anyhow!("unsupported node id multihash code {code}"));
		}
		Ok(NodeId(value.to_string()))
	}
}

impl Serialize for NodeId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(&self.0)
	}
}

impl<'de> Deserialize<'de> for NodeId {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = String::deserialize(deserializer)?;
		value.parse().map_err(D::Error::custom)
	}
}

/// Ed25519 keypair backing a node identity.
///
/// The on-disk format is byte-compatible with libp2p's keypair protobuf
/// encoding (`0x08 0x01 0x12 0x40 || seed || public_key`).
#[derive(Clone)]
pub struct NodeKeypair {
	signing: SigningKey,
}

impl NodeKeypair {
	pub fn generate() -> NodeKeypair {
		let mut seed = [0u8; ED25519_PUBLIC_KEY_LEN];
		OsRng.fill_bytes(&mut seed);
		NodeKeypair::from_seed(&seed)
	}

	pub fn from_seed(seed: &[u8; ED25519_PUBLIC_KEY_LEN]) -> NodeKeypair {
		NodeKeypair {
			signing: SigningKey::from_bytes(seed),
		}
	}

	pub fn seed(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
		self.signing.to_bytes()
	}

	pub fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
		self.signing.verifying_key().to_bytes()
	}

	pub fn sign(&self, message: &[u8]) -> [u8; ED25519_SIGNATURE_LEN] {
		use ed25519_dalek::Signer;
		self.signing.sign(message).to_bytes()
	}

	pub fn node_id(&self) -> NodeId {
		NodeId::from_public_key(&self.public_key())
	}

	pub fn to_protobuf(&self) -> Vec<u8> {
		let mut bytes = Vec::with_capacity(4 + ED25519_KEYPAIR_LEN);
		bytes.push(0x08);
		bytes.push(ED25519_KEY_TYPE as u8);
		bytes.push(0x12);
		bytes.push(ED25519_KEYPAIR_LEN as u8);
		bytes.extend_from_slice(&self.signing.to_keypair_bytes());
		bytes
	}

	pub fn from_protobuf(bytes: &[u8]) -> Result<NodeKeypair> {
		let (key_type, data) = parse_key_protobuf(bytes)?;
		if key_type != ED25519_KEY_TYPE {
			return Err(anyhow!("unsupported key type {key_type}"));
		}
		let keypair = <[u8; ED25519_KEYPAIR_LEN]>::try_from(data.as_slice())
			.map_err(|_| anyhow!("ed25519 keypair data must be 64 bytes"))?;
		let signing = SigningKey::from_keypair_bytes(&keypair)
			.map_err(|err| anyhow!("invalid ed25519 keypair: {err}"))?;
		Ok(NodeKeypair { signing })
	}

	/// Load the keypair from disk or generate and persist a new one.
	pub fn load_or_generate(path: &Path) -> Result<NodeKeypair> {
		if let Some(parent) = path.parent()
			&& !parent.as_os_str().is_empty()
			&& !parent.exists()
		{
			std::fs::create_dir_all(parent)?;
			log::info!("created key directory {}", parent.display());
		}
		if path.exists() {
			return NodeKeypair::from_protobuf(&std::fs::read(path)?);
		}
		let keypair = NodeKeypair::generate();
		std::fs::write(path, keypair.to_protobuf())?;
		Ok(keypair)
	}
}

/// Encode an Ed25519 public key as a libp2p `PublicKey` protobuf message.
fn encode_public_key_protobuf(public: &[u8; ED25519_PUBLIC_KEY_LEN]) -> [u8; 36] {
	let mut bytes = [0u8; 36];
	bytes[0] = 0x08;
	bytes[1] = ED25519_KEY_TYPE as u8;
	bytes[2] = 0x12;
	bytes[3] = ED25519_PUBLIC_KEY_LEN as u8;
	bytes[4..].copy_from_slice(public);
	bytes
}

/// Parse a libp2p `PrivateKey`/`PublicKey` protobuf message into
/// `(key_type, data)`. Only fields 1 (varint) and 2 (length-delimited)
/// are accepted, matching the libp2p key messages.
fn parse_key_protobuf(bytes: &[u8]) -> Result<(u64, Vec<u8>)> {
	let mut pos = 0;
	let mut key_type = None;
	let mut data = None;
	while pos < bytes.len() {
		let tag = decode_varint(bytes, &mut pos)?;
		let field = tag >> 3;
		let wire_type = tag & 0x07;
		match (field, wire_type) {
			(1, 0) => key_type = Some(decode_varint(bytes, &mut pos)?),
			(2, 2) => {
				let length = decode_varint(bytes, &mut pos)? as usize;
				let end = pos
					.checked_add(length)
					.ok_or_else(|| anyhow!("key data length overflow"))?;
				let value = bytes
					.get(pos..end)
					.ok_or_else(|| anyhow!("key data exceeds buffer"))?;
				data = Some(value.to_vec());
				pos = end;
			}
			_ => return Err(anyhow!("unexpected key protobuf field {field}")),
		}
	}
	let key_type = key_type.ok_or_else(|| anyhow!("missing key type field"))?;
	let data = data.ok_or_else(|| anyhow!("missing key data field"))?;
	Ok((key_type, data))
}

/// Validate multihash framing and return the hash code.
fn parse_multihash(bytes: &[u8]) -> Result<u64> {
	let mut pos = 0;
	let code = decode_varint(bytes, &mut pos)?;
	let length = decode_varint(bytes, &mut pos)? as usize;
	let end = pos
		.checked_add(length)
		.ok_or_else(|| anyhow!("multihash length overflow"))?;
	if end != bytes.len() {
		return Err(anyhow!("multihash length mismatch"));
	}
	Ok(code)
}

fn encode_varint(value: u64, out: &mut Vec<u8>) {
	let mut value = value;
	loop {
		let byte = (value & 0x7f) as u8;
		value >>= 7;
		if value == 0 {
			out.push(byte);
			break;
		}
		out.push(byte | 0x80);
	}
}

fn decode_varint(bytes: &[u8], pos: &mut usize) -> Result<u64> {
	let mut value: u64 = 0;
	let mut shift = 0;
	loop {
		let byte = *bytes.get(*pos).ok_or_else(|| anyhow!("truncated varint"))?;
		*pos += 1;
		value |= u64::from(byte & 0x7f) << shift;
		if byte & 0x80 == 0 {
			return Ok(value);
		}
		shift += 7;
		if shift >= 64 {
			return Err(anyhow!("varint too long"));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn test_seed(index: u8) -> [u8; ED25519_PUBLIC_KEY_LEN] {
		let mut seed = [0u8; ED25519_PUBLIC_KEY_LEN];
		seed[0] = index;
		seed[ED25519_PUBLIC_KEY_LEN - 1] = index;
		seed
	}

	/// Fixed golden vectors pinning the libp2p-compatible derivation. The
	/// expected values were cross-checked against libp2p `PeerId` while
	/// libp2p was still a dependency; they must never change.
	#[test]
	fn node_id_known_vectors() {
		let zeros = NodeKeypair::from_seed(&[0u8; ED25519_PUBLIC_KEY_LEN]);
		let expected = [
			"12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN",
			"12D3KooWLgoujyqdk8rSztvZMVsQxxsQdoHdfLTBwz8DMPogGiy2",
			"12D3KooWKgtXurnypDumf1NvcPKfqB1NRC5WGUffMQ5VWjqTiuuJ",
			"12D3KooWQfu42u8iy4KdXayMFaY7P1u6J3pJTLMP5s2neZ6XZ13z",
		];
		assert_eq!(zeros.node_id().as_str(), expected[0]);
		for index in 1..4u8 {
			let keypair = NodeKeypair::from_seed(&test_seed(index));
			assert_eq!(keypair.node_id().as_str(), expected[index as usize]);
		}
	}

	#[test]
	fn keypair_protobuf_roundtrip() {
		let keypair = NodeKeypair::generate();
		let bytes = keypair.to_protobuf();
		assert_eq!(bytes.len(), 68);
		let decoded = NodeKeypair::from_protobuf(&bytes).unwrap();
		assert_eq!(decoded.node_id(), keypair.node_id());
		assert_eq!(decoded.seed(), keypair.seed());
	}

	#[test]
	fn keypair_protobuf_rejects_bad_input() {
		assert!(NodeKeypair::from_protobuf(&[]).is_err());
		assert!(NodeKeypair::from_protobuf(&[0x08, 0x01, 0x12, 0x20, 0x00]).is_err());
		let keypair = NodeKeypair::generate();
		let mut tampered = keypair.to_protobuf();
		let last = tampered.len() - 1;
		tampered[last] ^= 0xff;
		assert!(NodeKeypair::from_protobuf(&tampered).is_err());
	}

	#[test]
	fn node_id_parse_roundtrip() {
		let keypair = NodeKeypair::generate();
		let id = keypair.node_id();
		let parsed: NodeId = id.as_str().parse().unwrap();
		assert_eq!(parsed, id);
		let json = serde_json::to_string(&id).unwrap();
		assert_eq!(json, format!("\"{}\"", id.as_str()));
		let from_json: NodeId = serde_json::from_str(&json).unwrap();
		assert_eq!(from_json, id);
	}

	#[test]
	fn node_id_parse_rejects_garbage() {
		assert!("".parse::<NodeId>().is_err());
		assert!("not a node id".parse::<NodeId>().is_err());
		assert!("12D3KooW".parse::<NodeId>().is_err());
	}
}
