use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use snow::{Builder, HandshakeState, TransportState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;

use crate::node_id::{NodeId, NodeKeypair};
use crate::p2p::{PeerReq, PeerRes};

/// Binds the noise handshake transcript to the PuppyNet peer protocol.
pub const PROTOCOL_PROLOGUE: &[u8] = b"puppynet-peer-v1";
/// WebSocket path used by the peer listener.
pub const PEER_WS_PATH: &str = "/peer/v1";

const MSG_HANDSHAKE: u8 = 0x01;
const MSG_TRANSPORT: u8 = 0x02;
const IDENTITY_V1: u8 = 0x01;
const ED25519_PUBLIC_LEN: usize = 32;
const ED25519_SIGNATURE_LEN: usize = 64;
const IDENTITY_PAYLOAD_LEN: usize = 1 + ED25519_PUBLIC_LEN + ED25519_SIGNATURE_LEN;
/// Largest plaintext fragment handed to snow per wire message. Noise
/// messages are capped at 65535 bytes including the 16 byte tag.
const MAX_FRAGMENT: usize = 60_000;
/// Largest reassembled application message (file chunks arrive as JSON).
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_WS_MESSAGE: usize = MAX_FRAGMENT + 64;
const SNOW_BUFFER: usize = 65_535;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// One envelope per WebSocket/length-prefixed message after the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Wire {
	Req { id: u64, body: PeerReq },
	Res { id: u64, body: PeerRes },
	Ping { id: u64 },
	Pong { id: u64 },
}

/// Role taken during the noise handshake. The dialing peer initiates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
	Initiator,
	Responder,
}

/// Bidirectional message pipe carrying opaque framed messages.
#[async_trait]
pub trait MessageStream: Send {
	async fn send_message(&mut self, data: Vec<u8>) -> Result<()>;
	async fn recv_message(&mut self) -> Result<Vec<u8>>;
}

/// Length-prefixed framing over any byte stream. Each message is
/// `[u32 BE length][payload]`.
pub struct LengthPrefixed<S> {
	io: S,
}

impl<S> LengthPrefixed<S> {
	pub fn new(io: S) -> LengthPrefixed<S> {
		LengthPrefixed { io }
	}
}

#[async_trait]
impl<S> MessageStream for LengthPrefixed<S>
where
	S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
	async fn send_message(&mut self, data: Vec<u8>) -> Result<()> {
		if data.len() > MAX_WS_MESSAGE {
			bail!("frame length {} exceeds limit {MAX_WS_MESSAGE}", data.len());
		}
		self.io
			.write_all(&(data.len() as u32).to_be_bytes())
			.await
			.map_err(|err| anyhow!("failed to write frame length: {err}"))?;
		self.io
			.write_all(&data)
			.await
			.map_err(|err| anyhow!("failed to write frame: {err}"))?;
		self.io
			.flush()
			.await
			.map_err(|err| anyhow!("failed to flush frame: {err}"))?;
		Ok(())
	}

	async fn recv_message(&mut self) -> Result<Vec<u8>> {
		let mut length = [0u8; 4];
		self.io
			.read_exact(&mut length)
			.await
			.map_err(|err| anyhow!("failed to read frame length: {err}"))?;
		let length = u32::from_be_bytes(length) as usize;
		if length > MAX_WS_MESSAGE {
			bail!("frame length {length} exceeds limit {MAX_WS_MESSAGE}");
		}
		let mut data = vec![0u8; length];
		self.io
			.read_exact(&mut data)
			.await
			.map_err(|err| anyhow!("failed to read frame: {err}"))?;
		Ok(data)
	}
}

#[async_trait]
impl<S> MessageStream for WebSocketStream<S>
where
	S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
	async fn send_message(&mut self, data: Vec<u8>) -> Result<()> {
		self.send(tokio_tungstenite::tungstenite::Message::Binary(data))
			.await
			.map_err(|err| anyhow!("websocket send failed: {err}"))?;
		Ok(())
	}

	async fn recv_message(&mut self) -> Result<Vec<u8>> {
		loop {
			match self.next().await {
				None => bail!("websocket closed"),
				Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(data))) => {
					if data.len() > MAX_WS_MESSAGE {
						bail!("websocket message exceeds {MAX_WS_MESSAGE} bytes");
					}
					return Ok(data);
				}
				Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
					bail!("websocket closed");
				}
				Some(Ok(
					tokio_tungstenite::tungstenite::Message::Ping(_)
					| tokio_tungstenite::tungstenite::Message::Pong(_),
				)) => continue,
				Some(Ok(_)) => bail!("unexpected websocket message"),
				Some(Err(err)) => bail!("websocket receive failed: {err}"),
			}
		}
	}
}

/// Tight WebSocket limits for the peer endpoint. Every message is one snow
/// ciphertext, so small bounds are enough.
pub fn peer_ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
	tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
		max_message_size: Some(MAX_WS_MESSAGE),
		max_frame_size: Some(MAX_WS_MESSAGE),
		..Default::default()
	}
}

/// Bind the node identity to the connection: sign the noise static public
/// key with the node's Ed25519 identity key, exactly like libp2p does.
fn identity_payload(keypair: &NodeKeypair, noise_static: &[u8]) -> Vec<u8> {
	let signature = keypair.sign(noise_static);
	let mut payload = Vec::with_capacity(IDENTITY_PAYLOAD_LEN);
	payload.push(IDENTITY_V1);
	payload.extend_from_slice(&keypair.public_key());
	payload.extend_from_slice(&signature);
	payload
}

/// Verify the peer's identity payload and derive its node ID. The noise
/// static key is authenticated by the XX handshake, so a valid signature
/// over it proves possession of the claimed identity key.
fn verify_identity_payload(payload: &[u8], noise_static: Option<&[u8]>) -> Result<NodeId> {
	if payload.len() != IDENTITY_PAYLOAD_LEN {
		bail!(
			"invalid identity payload length {} expected {IDENTITY_PAYLOAD_LEN}",
			payload.len()
		);
	}
	if payload[0] != IDENTITY_V1 {
		bail!("unsupported identity payload version {}", payload[0]);
	}
	let public: [u8; ED25519_PUBLIC_LEN] = payload[1..1 + ED25519_PUBLIC_LEN]
		.try_into()
		.expect("length checked above");
	let signature = Signature::from_slice(&payload[1 + ED25519_PUBLIC_LEN..IDENTITY_PAYLOAD_LEN])
		.map_err(|_| anyhow!("malformed identity signature"))?;
	let noise_static = noise_static.ok_or_else(|| anyhow!("missing remote noise static key"))?;
	let verifying =
		VerifyingKey::from_bytes(&public).map_err(|_| anyhow!("malformed identity key"))?;
	verifying
		.verify(noise_static, &signature)
		.map_err(|_| anyhow!("identity signature verification failed"))?;
	Ok(NodeId::from_public_key(&public))
}

fn check_expected(remote: &NodeId, expected: &Option<NodeId>) -> Result<()> {
	if let Some(expected) = expected
		&& expected != remote
	{
		bail!("peer identity mismatch: expected {expected}, got {remote}");
	}
	Ok(())
}

async fn send_handshake(stream: &mut impl MessageStream, data: &[u8]) -> Result<()> {
	let mut message = Vec::with_capacity(1 + data.len());
	message.push(MSG_HANDSHAKE);
	message.extend_from_slice(data);
	tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.send_message(message))
		.await
		.map_err(|_| anyhow!("handshake send timed out"))??;
	Ok(())
}

async fn recv_handshake(stream: &mut impl MessageStream) -> Result<Vec<u8>> {
	let message = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.recv_message())
		.await
		.map_err(|_| anyhow!("handshake receive timed out"))??;
	let (tag, body) = message
		.split_first()
		.ok_or_else(|| anyhow!("empty handshake message"))?;
	if *tag != MSG_HANDSHAKE {
		bail!("expected handshake message, got tag {tag:#x}");
	}
	Ok(body.to_vec())
}

fn noise_builder() -> Builder<'static> {
	let params: snow::params::NoiseParams = NOISE_PATTERN
		.parse()
		.expect("hardcoded noise pattern is valid");
	Builder::new(params).prologue(PROTOCOL_PROLOGUE)
}

/// Run the noise XX handshake over `stream`, binding both node identities.
///
/// Each side generates a fresh X25519 static key for the handshake and
/// signs it with its long-lived Ed25519 identity key; the signature rides
/// in the handshake payload. Returns an encrypted session with the
/// verified remote node ID, or an error if the remote identity does not
/// match `expected_remote`.
pub async fn handshake<S: MessageStream>(
	mut stream: S,
	keypair: &NodeKeypair,
	role: HandshakeRole,
	expected_remote: Option<NodeId>,
) -> Result<PeerSession<S>> {
	let noise = noise_builder().generate_keypair()?;
	let mut state = match role {
		HandshakeRole::Initiator => noise_builder()
			.local_private_key(&noise.private)
			.build_initiator()?,
		HandshakeRole::Responder => noise_builder()
			.local_private_key(&noise.private)
			.build_responder()?,
	};
	let mut buf = vec![0u8; SNOW_BUFFER];
	let remote_id = match role {
		HandshakeRole::Initiator => {
			let len = state.write_message(&[], &mut buf)?;
			send_handshake(&mut stream, &buf[..len]).await?;
			let message = recv_handshake(&mut stream).await?;
			let payload = read_handshake_payload(&mut state, &message)?;
			let remote = verify_identity_payload(&payload, state.get_remote_static())?;
			check_expected(&remote, &expected_remote)?;
			let payload = identity_payload(keypair, &noise.public);
			let len = state.write_message(&payload, &mut buf)?;
			send_handshake(&mut stream, &buf[..len]).await?;
			remote
		}
		HandshakeRole::Responder => {
			let message = recv_handshake(&mut stream).await?;
			read_handshake_payload(&mut state, &message)?;
			let payload = identity_payload(keypair, &noise.public);
			let len = state.write_message(&payload, &mut buf)?;
			send_handshake(&mut stream, &buf[..len]).await?;
			let message = recv_handshake(&mut stream).await?;
			let payload = read_handshake_payload(&mut state, &message)?;
			let remote = verify_identity_payload(&payload, state.get_remote_static())?;
			check_expected(&remote, &expected_remote)?;
			remote
		}
	};
	Ok(PeerSession {
		stream,
		transport: state.into_transport_mode()?,
		remote_id,
	})
}

fn read_handshake_payload(state: &mut HandshakeState, message: &[u8]) -> Result<Vec<u8>> {
	let mut plain = vec![0u8; SNOW_BUFFER];
	let len = state
		.read_message(message, &mut plain)
		.map_err(|err| anyhow!("noise handshake failed: {err}"))?;
	plain.truncate(len);
	Ok(plain)
}

/// Encrypted, mutually authenticated session with a verified peer.
pub struct PeerSession<S> {
	stream: S,
	transport: TransportState,
	remote_id: NodeId,
}

impl<S: MessageStream> PeerSession<S> {
	pub fn remote_id(&self) -> &NodeId {
		&self.remote_id
	}

	/// Encrypt and send an application message, fragmenting large payloads
	/// into snow-sized chunks. Plaintext fragment layout: `[more flag][chunk]`.
	pub async fn send(&mut self, payload: &[u8]) -> Result<()> {
		let mut offset = 0;
		loop {
			let end = (offset + MAX_FRAGMENT).min(payload.len());
			let more = if end == payload.len() { 0u8 } else { 1u8 };
			let mut plain = Vec::with_capacity(1 + end - offset);
			plain.push(more);
			plain.extend_from_slice(&payload[offset..end]);
			let mut message = Vec::with_capacity(1 + plain.len() + 16);
			message.push(MSG_TRANSPORT);
			message.resize(message.len() + plain.len() + 16, 0);
			let len = self
				.transport
				.write_message(&plain, &mut message[1..])
				.map_err(|err| anyhow!("encryption failed: {err}"))?;
			message.truncate(1 + len);
			self.stream.send_message(message).await?;
			offset = end;
			if more == 0 {
				return Ok(());
			}
		}
	}

	/// Receive and decrypt the next application message, reassembling
	/// fragments. Fails on tampering, replay, or oversized messages.
	pub async fn recv(&mut self) -> Result<Vec<u8>> {
		let mut payload = Vec::new();
		loop {
			let message = self.stream.recv_message().await?;
			let (tag, ciphertext) = message
				.split_first()
				.ok_or_else(|| anyhow!("empty transport message"))?;
			if *tag != MSG_TRANSPORT {
				bail!("expected transport message, got tag {tag:#x}");
			}
			let mut plain = vec![0u8; ciphertext.len()];
			let len = self
				.transport
				.read_message(ciphertext, &mut plain)
				.map_err(|err| anyhow!("decryption failed: {err}"))?;
			let (more, chunk) = plain[..len]
				.split_first()
				.ok_or_else(|| anyhow!("empty plaintext fragment"))?;
			payload.extend_from_slice(chunk);
			if payload.len() > MAX_MESSAGE {
				bail!("reassembled message exceeds {MAX_MESSAGE} bytes");
			}
			match *more {
				0 => return Ok(payload),
				1 => {}
				flag => bail!("invalid fragment flag {flag}"),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn paired_duplex() -> (
		LengthPrefixed<tokio::io::DuplexStream>,
		LengthPrefixed<tokio::io::DuplexStream>,
	) {
		let (a, b) = tokio::io::duplex(1024 * 1024);
		(LengthPrefixed::new(a), LengthPrefixed::new(b))
	}

	/// Corrupts the `corrupt_at`-th send, counting from construction.
	struct TamperStream<S> {
		inner: S,
		sends: usize,
		corrupt_at: usize,
	}

	#[async_trait]
	impl<S: MessageStream> MessageStream for TamperStream<S> {
		async fn send_message(&mut self, data: Vec<u8>) -> Result<()> {
			self.sends += 1;
			if self.sends == self.corrupt_at {
				let mut data = data;
				let last = data.len() - 1;
				data[last] ^= 0xff;
				return self.inner.send_message(data).await;
			}
			self.inner.send_message(data).await
		}

		async fn recv_message(&mut self) -> Result<Vec<u8>> {
			self.inner.recv_message().await
		}
	}

	#[tokio::test]
	async fn handshake_roundtrip_small_messages() {
		let alice = NodeKeypair::generate();
		let bob = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		let expected_alice = alice.node_id();
		let expected_bob = bob.node_id();
		let init = tokio::spawn(async move {
			let mut session = handshake(a, &alice, HandshakeRole::Initiator, Some(expected_bob))
				.await
				.unwrap();
			session.send(b"hello bob").await.unwrap();
			let reply = session.recv().await.unwrap();
			assert_eq!(reply, b"hi alice");
		});
		let mut session = handshake(
			b,
			&bob,
			HandshakeRole::Responder,
			Some(expected_alice.clone()),
		)
		.await
		.unwrap();
		assert_eq!(session.remote_id(), &expected_alice);
		let message = session.recv().await.unwrap();
		assert_eq!(message, b"hello bob");
		session.send(b"hi alice").await.unwrap();
		init.await.unwrap();
	}

	#[tokio::test]
	async fn large_payload_survives_fragmentation() {
		let payload: Vec<u8> = (0..5 * 1024 * 1024 + 123)
			.map(|i| ((i * 7 + i / 251) % 256) as u8)
			.collect();
		let alice = NodeKeypair::generate();
		let bob = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		let expected = payload.clone();
		let init = tokio::spawn(async move {
			let mut session = handshake(a, &alice, HandshakeRole::Initiator, None)
				.await
				.unwrap();
			session.send(&expected).await.unwrap();
		});
		let mut session = handshake(b, &bob, HandshakeRole::Responder, None)
			.await
			.unwrap();
		let received = session.recv().await.unwrap();
		assert_eq!(received.len(), payload.len());
		assert_eq!(received, payload);
		init.await.unwrap();
	}

	#[tokio::test]
	async fn rejects_expected_peer_mismatch() {
		let alice = NodeKeypair::generate();
		let mallory = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		let expected_mallory = mallory.node_id();
		let init = tokio::spawn(async move {
			let result = handshake(
				a,
				&mallory,
				HandshakeRole::Initiator,
				Some(expected_mallory),
			)
			.await;
			assert!(result.is_err());
		});
		let result = handshake(b, &alice, HandshakeRole::Responder, None).await;
		assert!(result.is_err());
		init.await.unwrap();
	}

	#[tokio::test]
	async fn rejects_tampered_ciphertext() {
		let alice = NodeKeypair::generate();
		let bob = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		// Initiator sends two handshake messages, so tamper send #3 (first
		// transport message).
		let a = TamperStream {
			inner: a,
			sends: 0,
			corrupt_at: 3,
		};
		let init = tokio::spawn(async move {
			let mut session = handshake(a, &alice, HandshakeRole::Initiator, None)
				.await
				.unwrap();
			session.send(b"secret").await.unwrap();
			let _ = session.recv().await;
		});
		let mut session = handshake(b, &bob, HandshakeRole::Responder, None)
			.await
			.unwrap();
		let result = session.recv().await;
		assert!(result.is_err());
		drop(session);
		init.await.unwrap();
	}

	#[tokio::test]
	async fn empty_payload_roundtrips() {
		let alice = NodeKeypair::generate();
		let bob = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		let init = tokio::spawn(async move {
			let mut session = handshake(a, &alice, HandshakeRole::Initiator, None)
				.await
				.unwrap();
			session.send(&[]).await.unwrap();
			let reply = session.recv().await.unwrap();
			assert!(reply.is_empty());
		});
		let mut session = handshake(b, &bob, HandshakeRole::Responder, None)
			.await
			.unwrap();
		let message = session.recv().await.unwrap();
		assert!(message.is_empty());
		session.send(&[]).await.unwrap();
		init.await.unwrap();
	}

	#[test]
	fn wire_envelope_roundtrip() {
		let request = Wire::Req {
			id: 42,
			body: PeerReq::ReadFile {
				path: "/tmp/data.bin".into(),
				offset: 1024,
				length: Some(4 * 1024 * 1024),
			},
		};
		let encoded = serde_json::to_vec(&request).unwrap();
		let decoded: Wire = serde_json::from_slice(&encoded).unwrap();
		match decoded {
			Wire::Req { id, body } => {
				assert_eq!(id, 42);
				match body {
					PeerReq::ReadFile {
						path,
						offset,
						length,
					} => {
						assert_eq!(path, "/tmp/data.bin");
						assert_eq!(offset, 1024);
						assert_eq!(length, Some(4 * 1024 * 1024));
					}
					other => panic!("unexpected request body {other:?}"),
				}
			}
			other => panic!("unexpected envelope {other:?}"),
		}

		let response = Wire::Res {
			id: 42,
			body: PeerRes::Error("boom".into()),
		};
		let encoded = serde_json::to_vec(&response).unwrap();
		let decoded: Wire = serde_json::from_slice(&encoded).unwrap();
		match decoded {
			Wire::Res { id, body } => {
				assert_eq!(id, 42);
				match body {
					PeerRes::Error(error) => assert_eq!(error, "boom"),
					other => panic!("unexpected response body {other:?}"),
				}
			}
			other => panic!("unexpected envelope {other:?}"),
		}

		let ping = Wire::Ping { id: 7 };
		let encoded = serde_json::to_vec(&ping).unwrap();
		let decoded: Wire = serde_json::from_slice(&encoded).unwrap();
		match decoded {
			Wire::Ping { id } => assert_eq!(id, 7),
			other => panic!("unexpected envelope {other:?}"),
		}

		let pong = Wire::Pong { id: 8 };
		let encoded = serde_json::to_vec(&pong).unwrap();
		let decoded: Wire = serde_json::from_slice(&encoded).unwrap();
		match decoded {
			Wire::Pong { id } => assert_eq!(id, 8),
			other => panic!("unexpected envelope {other:?}"),
		}
	}

	#[tokio::test]
	async fn websocket_tcp_integration() {
		let alice = NodeKeypair::generate();
		let bob = NodeKeypair::generate();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let server_keypair = bob.clone();
		let expected_client = alice.node_id();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let ws = tokio_tungstenite::accept_async_with_config(stream, Some(peer_ws_config()))
				.await
				.unwrap();
			let mut session = handshake(
				ws,
				&server_keypair,
				HandshakeRole::Responder,
				Some(expected_client),
			)
			.await
			.unwrap();
			let message = session.recv().await.unwrap();
			session.send(&message).await.unwrap();
		});
		let (ws, _response) =
			tokio_tungstenite::connect_async(format!("ws://{addr}{PEER_WS_PATH}"))
				.await
				.unwrap();
		let mut session = handshake(ws, &alice, HandshakeRole::Initiator, Some(bob.node_id()))
			.await
			.unwrap();
		assert_eq!(session.remote_id(), &bob.node_id());
		let payload = serde_json::to_vec(&Wire::Req {
			id: 1,
			body: PeerReq::PeerInfo,
		})
		.unwrap();
		session.send(&payload).await.unwrap();
		let echoed = session.recv().await.unwrap();
		assert_eq!(echoed, payload);
		server.await.unwrap();
	}

	#[tokio::test]
	async fn rejects_garbage_instead_of_handshake() {
		let bob = NodeKeypair::generate();
		let (a, b) = paired_duplex();
		let sender = tokio::spawn(async move {
			use tokio::io::AsyncWriteExt;
			let mut io = a.io;
			io.write_all(&[0x02, 0xde, 0xad, 0xbe, 0xef]).await.unwrap();
			io.shutdown().await.unwrap();
		});
		let result = handshake(b, &bob, HandshakeRole::Responder, None).await;
		assert!(result.is_err());
		sender.await.unwrap();
	}
}
