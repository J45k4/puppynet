use std::{
	io::BufReader,
	path::{Path, PathBuf},
};

use anyhow::bail;
use flate2::read::GzDecoder;
use rsa::signature::Verifier;
use rsa::{RsaPublicKey, pkcs1v15, pkcs8::DecodePublicKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tar::Archive;
use tokio::{fs::File, io::AsyncWriteExt};
use zip::ZipArchive;

/// Path resolution: this file is core/src/updater.rs; the key lives at repository root.
pub const PUBLIC_KEY: &str = include_str!("../../public_key.pem");

/// Progress information during an update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateProgress {
	/// Fetching release metadata from GitHub
	FetchingRelease,
	/// Downloading the binary
	Downloading { filename: String },
	/// Unpacking the archive
	Unpacking,
	/// Verifying signature
	Verifying,
	/// Installing the binary
	Installing,
	/// Update completed successfully
	Completed { version: String },
	/// Update failed with error
	Failed { error: String },
	/// Already up to date
	AlreadyUpToDate { current_version: u32 },
}

/// Result of an update operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResult {
	pub success: bool,
	pub message: String,
	pub new_version: Option<String>,
}

pub fn verify_signature(bin: &Path, sig: &Path) -> anyhow::Result<bool> {
	log::info!("verifying {} with {}", bin.display(), sig.display());
	let public_key = RsaPublicKey::from_public_key_pem(PUBLIC_KEY).unwrap();
	let verifying_key = pkcs1v15::VerifyingKey::<Sha256>::new(public_key);
	let signature = std::fs::read(sig)?;
	let signature = rsa::pkcs1v15::Signature::try_from(signature.as_slice())?;
	let data = std::fs::read(bin)?;
	let public_key = RsaPublicKey::from_public_key_pem(PUBLIC_KEY).unwrap();
	let verifying_key = pkcs1v15::VerifyingKey::<Sha256>::new(public_key);
	Ok(verifying_key.verify(&data, &signature).is_ok())
}

fn get_os_name() -> String {
	let os = std::env::consts::OS;
	format!("{}", os)
}

fn app_dir() -> PathBuf {
	let path = homedir::my_home().unwrap().unwrap().join(".puppynet");
	if !path.exists() {
		std::fs::create_dir_all(&path).unwrap();
	}
	path
}

fn bin_dir() -> PathBuf {
	let path = app_dir().join("bin");
	if !path.exists() {
		std::fs::create_dir_all(&path).unwrap();
	}
	path
}

async fn fetch_release(version: Option<&str>) -> anyhow::Result<Value> {
	let client = reqwest::Client::new();
	let url = match version {
		Some(tag) => format!(
			"https://api.github.com/repos/j45k4/puppynet/releases/tags/{}",
			tag
		),
		None => "https://api.github.com/repos/j45k4/puppynet/releases/latest".to_string(),
	};
	let res = client
		.get(url)
		.header("User-Agent", "puppynet")
		.send()
		.await?
		.error_for_status()?;
	let body = res.text().await?;

	Ok(serde_json::from_str::<Value>(&body)?)
}

async fn download_bin(url: &str, filename: &str) -> anyhow::Result<PathBuf> {
	let res = reqwest::get(url).await?;
	if !res.status().is_success() {
		bail!("Failed to download asset. HTTP status: {}", res.status());
	}
	let bytes = res.bytes().await?;
	let path = app_dir().join(&filename);
	let mut file = File::create(&path).await?;
	file.write_all(&bytes).await?;
	Ok(path)
}

/// Perform update with progress callback.
/// The callback receives UpdateProgress events during the update process.
/// The callback must be Send + 'static to work across async boundaries.
pub async fn update_with_progress<F>(
	version: Option<&str>,
	current_version: u32,
	progress_callback: F,
) -> anyhow::Result<UpdateResult>
where
	F: Fn(UpdateProgress) + Send + 'static,
{
	progress_callback(UpdateProgress::FetchingRelease);

	let res = fetch_release(version).await?;
	let tag = match res["tag_name"].as_str() {
		Some(tag) => tag.to_string(),
		None => bail!("release response missing tag_name"),
	};

	if let Some(requested_tag) = version {
		log::info!("requested tag: {}", requested_tag);
	}
	log::info!("current: {}", current_version);
	log::info!("release tag: {}", tag);

	if version.is_none() {
		if let Ok(tag_number) = tag.parse::<u32>() {
			log::info!("latest numeric tag: {}", tag_number);
			if tag_number <= current_version {
				log::info!("Already up to date");
				progress_callback(UpdateProgress::AlreadyUpToDate {
					current_version,
				});
				return Ok(UpdateResult {
					success: true,
					message: "Already up to date".to_string(),
					new_version: None,
				});
			}
		} else {
			log::info!(
				"latest release tag {} is not numeric; skipping automatic version comparison",
				tag
			);
		}
	}

	let assets = match res["assets"] {
		Value::Array(ref assets) => assets,
		_ => bail!("no assets found"),
	};

	let os_name = get_os_name();
	let asset = match assets.iter().find(|asset| {
		if let Some(name) = asset["name"].as_str() {
			name.contains(&os_name)
		} else {
			false
		}
	}) {
		Some(asset) => asset,
		None => bail!("no asset found for os: {}", os_name),
	};

	let download_url = asset["browser_download_url"]
		.as_str()
		.ok_or_else(|| anyhow::anyhow!("no download url found"))?;

	log::info!("download_url: {}", download_url);

	let filename = asset["name"]
		.as_str()
		.map(|s| s.to_string())
		.unwrap_or_else(|| "downloaded_binary".to_string());

	log::info!("Downloading asset: {}", filename);
	progress_callback(UpdateProgress::Downloading {
		filename: filename.clone(),
	});

	let path = download_bin(download_url, &filename).await?;

	log::info!("Downloaded asset to: {:?}", path);

	progress_callback(UpdateProgress::Unpacking);

	// Use spawn_blocking for the synchronous archive extraction
	let path_clone = path.clone();
	let filename_clone = filename.clone();
	tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
		let file = std::fs::File::open(&path_clone)?;

		// Detect archive format based on filename extension
		if filename_clone.ends_with(".zip") {
			// Extract ZIP archive (Windows)
			// Flatten the archive - extract files directly to app_dir using only filename
			log::info!("extracting ZIP archive");
			let mut archive = ZipArchive::new(file)?;
			for i in 0..archive.len() {
				let mut entry = archive.by_index(i)?;
				let full_path = match entry.enclosed_name() {
					Some(name) => name.to_path_buf(),
					None => continue,
				};

				// Skip directories
				if entry.is_dir() {
					continue;
				}

				// Use only the filename, not the full path (flatten the archive)
				let file_name = match full_path.file_name() {
					Some(name) => name,
					None => continue,
				};

				log::info!("unpacking: {:?} (from {:?})", file_name, full_path);
				let dst = app_dir().join(file_name);
				log::info!("unpacking to {:?}", dst);

				let mut outfile = std::fs::File::create(&dst)?;
				std::io::copy(&mut entry, &mut outfile)?;
			}
		} else {
			// Extract tar.gz archive (Linux/macOS)
			log::info!("extracting tar.gz archive");
			let buf_reader = BufReader::new(file);
			let decoder = GzDecoder::new(buf_reader);
			let mut archive = Archive::new(decoder);
			let mut entries = archive.entries()?;
			while let Some(file) = entries.next() {
				let mut file = file?;
				let name = match file.path() {
					Ok(name) => name,
					Err(_) => continue,
				};
				log::info!("unpacking: {:?}", name);
				let dst = app_dir().join(name);
				log::info!("unpacking to {:?}", dst);
				file.unpack(dst)?;
			}
		}
		Ok(())
	}).await??;

	progress_callback(UpdateProgress::Verifying);

	// Use platform-specific binary name
	let bin_name = if cfg!(windows) { "puppynet.exe" } else { "puppynet" };
	let bin_path = app_dir().join(bin_name);

	// List directory contents for debugging
	let entries: Vec<_> = std::fs::read_dir(app_dir())
		.map(|rd| rd.filter_map(|e| e.ok().map(|e| e.file_name())).collect())
		.unwrap_or_default();
	log::info!("app_dir contents after extraction: {:?}", entries);

	// Check that binary exists
	if !bin_path.exists() {
		log::error!("Binary not found at {:?}, directory contains: {:?}", bin_path, entries);
		let error = format!("Binary not found: {:?}. Directory contains: {:?}", bin_path, entries);
		progress_callback(UpdateProgress::Failed { error: error.clone() });
		bail!("{}", error);
	}

	// Find the signature file - try known names first, then search for any .sig file
	let known_sig_names = ["puppynet.sig", "puppynet.exe.sig"];
	let sig_path = known_sig_names
		.iter()
		.map(|name| app_dir().join(name))
		.find(|p| p.exists())
		.or_else(|| {
			// Fallback: search for any .sig file in app_dir
			std::fs::read_dir(app_dir())
				.ok()?
				.filter_map(|e| e.ok())
				.map(|e| e.path())
				.find(|p| p.extension().is_some_and(|ext| ext == "sig"))
		});

	let sig_path = match sig_path {
		Some(p) => {
			log::info!("Found signature file: {:?}", p);
			p
		}
		None => {
			let error = format!(
				"Signature file not found. Tried: {:?}, also searched for any .sig file. Directory contains: {:?}",
				known_sig_names, entries
			);
			progress_callback(UpdateProgress::Failed { error: error.clone() });
			bail!("{}", error);
		}
	};

	// Verify signature in blocking context
	let bin_path_clone = bin_path.clone();
	let sig_path_clone = sig_path.clone();
	let verify_result = tokio::task::spawn_blocking(move || {
		verify_signature(&bin_path_clone, &sig_path_clone)
	}).await??;

	if !verify_result {
		let error = "Signature verification failed".to_string();
		progress_callback(UpdateProgress::Failed {
			error: error.clone(),
		});
		bail!("{}", error);
	}

	progress_callback(UpdateProgress::Installing);

	tokio::fs::copy(&bin_path, bin_dir().join(bin_name)).await?;
	tokio::fs::remove_file(&bin_path).await?;
	tokio::fs::remove_file(&sig_path).await?;

	let tag_clone = tag.clone();
	progress_callback(UpdateProgress::Completed {
		version: tag_clone,
	});

	Ok(UpdateResult {
		success: true,
		message: format!("Updated to version {}", tag),
		new_version: Some(tag),
	})
}

/// Perform update without progress callback (simple version).
pub async fn update(version: Option<&str>, current_version: u32) -> anyhow::Result<UpdateResult> {
	update_with_progress(version, current_version, |_| {}).await
}
