use anyhow::{Result, anyhow};
use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
pub struct Claims {
	pub sub: String,
	pub iat: i64,
	pub exp: i64,
	pub scope: Vec<String>,
}

pub fn hash_password(password: &str) -> Result<String> {
	let salt = SaltString::generate(&mut OsRng);
	Ok(Argon2::default()
		.hash_password(password.as_bytes(), &salt)
		.map_err(|err| anyhow!(err))?
		.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
	let parsed = PasswordHash::new(hash).map_err(|err| anyhow!(err))?;
	Ok(Argon2::default()
		.verify_password(password.as_bytes(), &parsed)
		.is_ok())
}

pub fn issue_jwt(username: &str, secret: &[u8]) -> Result<String> {
	let now = Utc::now();
	let claims = Claims {
		sub: username.to_string(),
		iat: now.timestamp(),
		exp: (now + Duration::minutes(15)).timestamp(),
		scope: vec![String::from("api")],
	};
	Ok(encode(
		&Header::default(),
		&claims,
		&EncodingKey::from_secret(secret),
	)?)
}

pub fn verify_jwt(token: &str, secret: &[u8]) -> Result<Claims> {
	Ok(decode::<Claims>(
		token,
		&DecodingKey::from_secret(secret),
		&Validation::default(),
	)?
	.claims)
}

pub fn token_hash(token: &str) -> Vec<u8> {
	let mut hasher = Sha256::new();
	hasher.update(token.as_bytes());
	hasher.finalize().to_vec()
}

pub fn generate_session_token() -> (String, Vec<u8>) {
	let mut bytes = [0u8; 32];
	OsRng.fill_bytes(&mut bytes);
	let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
	let hash = token_hash(&token);
	(token, hash)
}
