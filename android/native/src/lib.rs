use std::ptr;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jlong};
use rand::RngCore;
use snow::{Builder, HandshakeState, TransportState};

const PROTOCOL_PROLOGUE: &[u8] = b"puppynet-peer-v1";
const MSG_HANDSHAKE: u8 = 0x01;
const MSG_TRANSPORT: u8 = 0x02;
const IDENTITY_V1: u8 = 0x01;
const MAX_FRAGMENT: usize = 60_000;
const SNOW_BUFFER: usize = 65_535;

enum Phase {
	Start,
	AwaitSecond,
	SendThird,
	Complete,
}

struct Session {
	identity: SigningKey,
	noise_public: Vec<u8>,
	handshake: Option<HandshakeState>,
	transport: Option<TransportState>,
	expected_remote: Option<String>,
	remote_id: Option<String>,
	phase: Phase,
	received: Vec<u8>,
}

fn node_id(public: &[u8; 32]) -> String {
	let mut encoded = Vec::with_capacity(38);
	encoded.extend_from_slice(&[0x00, 0x24, 0x08, 0x01, 0x12, 0x20]);
	encoded.extend_from_slice(public);
	bs58::encode(encoded).into_string()
}

fn identity_payload(identity: &SigningKey, noise_static: &[u8]) -> Vec<u8> {
	let mut payload = Vec::with_capacity(97);
	payload.push(IDENTITY_V1);
	payload.extend_from_slice(&identity.verifying_key().to_bytes());
	payload.extend_from_slice(&identity.sign(noise_static).to_bytes());
	payload
}

fn verify_identity(payload: &[u8], noise_static: Option<&[u8]>) -> Result<String, String> {
	if payload.len() != 97 || payload[0] != IDENTITY_V1 {
		return Err("invalid peer identity payload".to_owned());
	}
	let public: [u8; 32] = payload[1..33]
		.try_into()
		.map_err(|_| "invalid peer public key".to_owned())?;
	let signature = Signature::from_slice(&payload[33..97])
		.map_err(|_| "invalid peer identity signature".to_owned())?;
	let verifying =
		VerifyingKey::from_bytes(&public).map_err(|_| "invalid peer identity key".to_owned())?;
	verifying
		.verify(
			noise_static.ok_or_else(|| "missing peer Noise static key".to_owned())?,
			&signature,
		)
		.map_err(|_| "peer identity signature verification failed".to_owned())?;
	Ok(node_id(&public))
}

fn new_session(seed: &[u8], expected_remote: String) -> Result<Session, String> {
	let seed: [u8; 32] = seed
		.try_into()
		.map_err(|_| "identity seed must be 32 bytes".to_owned())?;
	let identity = SigningKey::from_bytes(&seed);
	let params = "Noise_XX_25519_ChaChaPoly_SHA256"
		.parse()
		.map_err(|err| format!("invalid Noise parameters: {err}"))?;
	let builder = Builder::new(params).prologue(PROTOCOL_PROLOGUE);
	let noise = builder
		.generate_keypair()
		.map_err(|err| format!("failed to generate Noise key: {err}"))?;
	let handshake = Builder::new(
		"Noise_XX_25519_ChaChaPoly_SHA256"
			.parse()
			.map_err(|err| format!("invalid Noise parameters: {err}"))?,
	)
	.prologue(PROTOCOL_PROLOGUE)
	.local_private_key(&noise.private)
	.build_initiator()
	.map_err(|err| format!("failed to create Noise initiator: {err}"))?;
	Ok(Session {
		identity,
		noise_public: noise.public,
		handshake: Some(handshake),
		transport: None,
		expected_remote: (!expected_remote.is_empty()).then_some(expected_remote),
		remote_id: None,
		phase: Phase::Start,
		received: Vec::new(),
	})
}

fn next_handshake(session: &mut Session) -> Result<Option<Vec<u8>>, String> {
	match session.phase {
		Phase::Start => {
			let mut buffer = vec![0; SNOW_BUFFER];
			let length = session
				.handshake
				.as_mut()
				.ok_or_else(|| "missing Noise handshake".to_owned())?
				.write_message(&[], &mut buffer)
				.map_err(|err| format!("Noise handshake failed: {err}"))?;
			buffer.truncate(length);
			buffer.insert(0, MSG_HANDSHAKE);
			session.phase = Phase::AwaitSecond;
			Ok(Some(buffer))
		}
		Phase::SendThird => {
			let payload = identity_payload(&session.identity, &session.noise_public);
			let mut buffer = vec![0; SNOW_BUFFER];
			let length = session
				.handshake
				.as_mut()
				.ok_or_else(|| "missing Noise handshake".to_owned())?
				.write_message(&payload, &mut buffer)
				.map_err(|err| format!("Noise handshake failed: {err}"))?;
			buffer.truncate(length);
			buffer.insert(0, MSG_HANDSHAKE);
			let handshake = session
				.handshake
				.take()
				.ok_or_else(|| "missing Noise handshake".to_owned())?;
			session.transport = Some(
				handshake
					.into_transport_mode()
					.map_err(|err| format!("failed to enter transport mode: {err}"))?,
			);
			session.phase = Phase::Complete;
			Ok(Some(buffer))
		}
		Phase::AwaitSecond => Ok(None),
		Phase::Complete => Err("Noise handshake is already complete".to_owned()),
	}
}

fn read_handshake(session: &mut Session, message: &[u8]) -> Result<(), String> {
	if !matches!(session.phase, Phase::AwaitSecond) {
		return Err("unexpected Noise handshake message".to_owned());
	}
	let (tag, payload) = message
		.split_first()
		.ok_or_else(|| "empty peer message".to_owned())?;
	if *tag != MSG_HANDSHAKE {
		return Err("expected a Noise handshake message".to_owned());
	}
	let mut plain = vec![0; SNOW_BUFFER];
	let length = session
		.handshake
		.as_mut()
		.ok_or_else(|| "missing Noise handshake".to_owned())?
		.read_message(payload, &mut plain)
		.map_err(|err| format!("Noise handshake failed: {err}"))?;
	plain.truncate(length);
	let remote = verify_identity(
		&plain,
		session
			.handshake
			.as_ref()
			.and_then(HandshakeState::get_remote_static),
	)?;
	if session
		.expected_remote
		.as_ref()
		.is_some_and(|expected| expected != &remote)
	{
		return Err(format!("peer identity mismatch: got {remote}"));
	}
	session.remote_id = Some(remote);
	session.phase = Phase::SendThird;
	Ok(())
}

fn encrypt(session: &mut Session, plain: &[u8]) -> Result<Vec<u8>, String> {
	if plain.len() > MAX_FRAGMENT {
		return Err(format!(
			"message exceeds {MAX_FRAGMENT} byte fragment limit"
		));
	}
	let mut payload = Vec::with_capacity(plain.len() + 1);
	payload.push(0);
	payload.extend_from_slice(plain);
	let mut output = vec![MSG_TRANSPORT; payload.len() + 17];
	let length = session
		.transport
		.as_mut()
		.ok_or_else(|| "Noise handshake is not complete".to_owned())?
		.write_message(&payload, &mut output[1..])
		.map_err(|err| format!("Noise encryption failed: {err}"))?;
	output.truncate(length + 1);
	Ok(output)
}

fn decrypt(session: &mut Session, message: &[u8]) -> Result<Option<Vec<u8>>, String> {
	let (tag, ciphertext) = message
		.split_first()
		.ok_or_else(|| "empty peer message".to_owned())?;
	if *tag != MSG_TRANSPORT {
		return Err("expected encrypted transport message".to_owned());
	}
	let mut plain = vec![0; SNOW_BUFFER];
	let length = session
		.transport
		.as_mut()
		.ok_or_else(|| "Noise handshake is not complete".to_owned())?
		.read_message(ciphertext, &mut plain)
		.map_err(|err| format!("Noise decryption failed: {err}"))?;
	let (more, payload) = plain[..length]
		.split_first()
		.ok_or_else(|| "empty encrypted payload".to_owned())?;
	if *more > 1 || session.received.len() + payload.len() > 16 * 1024 * 1024 {
		return Err("invalid encrypted message fragment".to_owned());
	}
	session.received.extend_from_slice(payload);
	if *more == 1 {
		return Ok(None);
	}
	Ok(Some(std::mem::take(&mut session.received)))
}

fn with_session<T>(
	handle: jlong,
	operation: impl FnOnce(&mut Session) -> Result<T, String>,
) -> Result<T, String> {
	if handle == 0 {
		return Err("native session is closed".to_owned());
	}
	let session = unsafe { (handle as *const Mutex<Session>).as_ref() }
		.ok_or_else(|| "invalid native session".to_owned())?;
	let mut session = session
		.lock()
		.map_err(|_| "native session lock poisoned".to_owned())?;
	operation(&mut session)
}

fn throw(env: &mut JNIEnv, error: String) {
	let _ = env.throw_new("java/lang/IllegalStateException", error);
}

fn bytes_or_throw(env: &mut JNIEnv, result: Result<Vec<u8>, String>) -> jbyteArray {
	match result.and_then(|bytes| {
		env.byte_array_from_slice(&bytes)
			.map_err(|err| err.to_string())
	}) {
		Ok(bytes) => bytes.into_raw(),
		Err(error) => {
			throw(env, error);
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_createIdentitySeed(
	mut env: JNIEnv,
	_class: JClass,
) -> jbyteArray {
	let mut seed = [0; 32];
	rand::rngs::OsRng.fill_bytes(&mut seed);
	bytes_or_throw(&mut env, Ok(seed.to_vec()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_nodeId(
	mut env: JNIEnv,
	_class: JClass,
	seed: JByteArray,
) -> jni::sys::jstring {
	let result = env
		.convert_byte_array(&seed)
		.map_err(|err| err.to_string())
		.and_then(|seed| {
			let seed: [u8; 32] = seed
				.try_into()
				.map_err(|_| "identity seed must be 32 bytes".to_owned())?;
			Ok(node_id(
				&SigningKey::from_bytes(&seed).verifying_key().to_bytes(),
			))
		});
	match result.and_then(|id| env.new_string(id).map_err(|err| err.to_string())) {
		Ok(id) => id.into_raw(),
		Err(error) => {
			throw(&mut env, error);
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_createInitiator(
	mut env: JNIEnv,
	_class: JClass,
	seed: JByteArray,
	expected_remote: JString,
) -> jlong {
	let result = (|| {
		let seed = env
			.convert_byte_array(&seed)
			.map_err(|err| err.to_string())?;
		let expected = env
			.get_string(&expected_remote)
			.map_err(|err| err.to_string())?
			.to_string_lossy()
			.into_owned();
		Ok::<_, String>(Box::into_raw(Box::new(Mutex::new(new_session(&seed, expected)?))) as jlong)
	})();
	match result {
		Ok(handle) => handle,
		Err(error) => {
			throw(&mut env, error);
			0
		}
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_nextHandshake(
	mut env: JNIEnv,
	_class: JClass,
	handle: jlong,
) -> jbyteArray {
	match with_session(handle, next_handshake) {
		Ok(Some(message)) => bytes_or_throw(&mut env, Ok(message)),
		Ok(None) => ptr::null_mut(),
		Err(error) => {
			throw(&mut env, error);
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_readHandshake(
	mut env: JNIEnv,
	_class: JClass,
	handle: jlong,
	message: JByteArray,
) {
	let result = env
		.convert_byte_array(&message)
		.map_err(|err| err.to_string())
		.and_then(|message| with_session(handle, |session| read_handshake(session, &message)));
	if let Err(error) = result {
		throw(&mut env, error);
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_isComplete(
	_env: JNIEnv,
	_class: JClass,
	handle: jlong,
) -> jboolean {
	match with_session(handle, |session| {
		Ok(matches!(session.phase, Phase::Complete))
	}) {
		Ok(true) => JNI_TRUE,
		_ => JNI_FALSE,
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_remoteNodeId(
	mut env: JNIEnv,
	_class: JClass,
	handle: jlong,
) -> jni::sys::jstring {
	let result = with_session(handle, |session| {
		session
			.remote_id
			.clone()
			.ok_or_else(|| "Noise handshake is not complete".to_owned())
	});
	match result.and_then(|id| env.new_string(id).map_err(|err| err.to_string())) {
		Ok(id) => id.into_raw(),
		Err(error) => {
			throw(&mut env, error);
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_encrypt(
	mut env: JNIEnv,
	_class: JClass,
	handle: jlong,
	payload: JByteArray,
) -> jbyteArray {
	let result = env
		.convert_byte_array(&payload)
		.map_err(|err| err.to_string())
		.and_then(|payload| with_session(handle, |session| encrypt(session, &payload)));
	bytes_or_throw(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_decrypt(
	mut env: JNIEnv,
	_class: JClass,
	handle: jlong,
	payload: JByteArray,
) -> jbyteArray {
	let result = env
		.convert_byte_array(&payload)
		.map_err(|err| err.to_string())
		.and_then(|payload| with_session(handle, |session| decrypt(session, &payload)))
		.map(|payload| payload.unwrap_or_default());
	bytes_or_throw(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fi_puppycorp_puppynet_NativeSession_close(
	_env: JNIEnv,
	_class: JClass,
	handle: jlong,
) {
	if handle != 0 {
		unsafe {
			drop(Box::from_raw(handle as *mut Mutex<Session>));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn node_id_matches_the_daemon_format() {
		let identity = SigningKey::from_bytes(&[0; 32]);
		assert_eq!(
			node_id(&identity.verifying_key().to_bytes()),
			"12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN"
		);
	}

	#[test]
	fn signed_noise_identity_verifies() {
		let identity = SigningKey::from_bytes(&[7; 32]);
		let payload = identity_payload(&identity, &[3; 32]);
		assert_eq!(
			verify_identity(&payload, Some(&[3; 32])).unwrap(),
			node_id(&identity.verifying_key().to_bytes())
		);
	}
}
