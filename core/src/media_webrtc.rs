use crate::p2p::MediaSourceKind;
use crate::webcam;
use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use openh264::encoder::{
	BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, UsageType,
};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use opus2::{Application, Bitrate, Channels};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, PolynomialDegree, Resampler};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS, MediaEngine};
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample as MediaSample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

const AUDIO_BITRATE: i32 = 32_000;
const AUDIO_CHANNELS: usize = 1;
const AUDIO_FRAME_SAMPLES: usize = 960;
const AUDIO_QUEUE_CAPACITY: usize = 8;
const AUDIO_SAMPLE_RATE: usize = 48_000;
const GATHER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SDP_BYTES: usize = 1024 * 1024;
const SCREEN_BITRATE: u32 = 1_000_000;
const SCREEN_FPS: u32 = 5;
const VIDEO_BITRATE: u32 = 1_200_000;
const VIDEO_FPS: u32 = 15;
const VIDEO_MAX_HEIGHT: u32 = 540;
const VIDEO_MAX_WIDTH: u32 = 960;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct SessionDescription {
	pub(crate) r#type: String,
	pub(crate) sdp: String,
}

#[derive(Deserialize)]
pub(crate) struct CreateMediaSession {
	pub(crate) source_id: String,
	pub(crate) offer: SessionDescription,
}

#[derive(Serialize)]
pub(crate) struct CreatedMediaSession {
	pub(crate) session_id: String,
	pub(crate) answer: SessionDescription,
}

struct MediaSession {
	peer_connection: Arc<RTCPeerConnection>,
	source_id: String,
}

struct MediaProducer {
	force_keyframe: Arc<AtomicBool>,
	stop: Arc<AtomicBool>,
	subscribers: usize,
	track: Arc<TrackLocalStaticSample>,
}

pub(crate) struct MediaSessionManager {
	api: Arc<webrtc::api::API>,
	producers: Mutex<HashMap<String, MediaProducer>>,
	sessions: Mutex<HashMap<String, MediaSession>>,
}

fn microphone_codec() -> RTCRtpCodecCapability {
	RTCRtpCodecCapability {
		mime_type: MIME_TYPE_OPUS.to_owned(),
		clock_rate: AUDIO_SAMPLE_RATE as u32,
		channels: 2,
		sdp_fmtp_line: String::from("minptime=10;useinbandfec=1"),
		rtcp_feedback: Vec::new(),
	}
}

fn video_codec() -> RTCRtpCodecCapability {
	RTCRtpCodecCapability {
		mime_type: MIME_TYPE_H264.to_owned(),
		clock_rate: 90_000,
		..Default::default()
	}
}

fn media_track(source_id: &str, kind: &MediaSourceKind) -> Arc<TrackLocalStaticSample> {
	let codec = if kind == &MediaSourceKind::Microphone {
		microphone_codec()
	} else {
		video_codec()
	};
	Arc::new(TrackLocalStaticSample::new(
		codec,
		source_id.to_string(),
		String::from("puppynet-local-media"),
	))
}

fn downmix_samples<T>(data: &[T], channels: usize) -> Vec<f32>
where
	T: Sample + Copy,
	f32: FromSample<T>,
{
	data.chunks(channels)
		.map(|frame| frame.iter().copied().map(f32::from_sample).sum::<f32>() / frame.len() as f32)
		.collect()
}

fn build_input_stream<T>(
	device: &cpal::Device,
	config: &cpal::StreamConfig,
	samples: std_mpsc::SyncSender<Vec<f32>>,
) -> Result<cpal::Stream>
where
	T: Sample + SizedSample + Copy,
	f32: FromSample<T>,
{
	let channels = usize::from(config.channels);
	let data_callback = move |data: &[T], _: &cpal::InputCallbackInfo| {
		let _ = samples.try_send(downmix_samples(data, channels));
	};
	let error_callback = |error: cpal::Error| {
		log::warn!("microphone WebRTC capture error: {error}");
	};
	Ok(device.build_input_stream(*config, data_callback, error_callback, None)?)
}

fn write_opus_frames(
	track: &TrackLocalStaticSample,
	handle: &tokio::runtime::Handle,
	encoder: &mut opus2::Encoder,
	resampler: &mut Async<f32>,
	input: &mut VecDeque<f32>,
) -> Result<()> {
	loop {
		let input_frames = resampler.input_frames_next();
		if input.len() < input_frames {
			return Ok(());
		}
		let chunk = input.drain(..input_frames).collect::<Vec<_>>();
		let adapter = InterleavedSlice::new(&chunk, AUDIO_CHANNELS, input_frames)?;
		let output = resampler.process(&adapter, 0, None)?.take_data();
		let encoded = encoder.encode_vec_float(&output, 4000)?;
		handle.block_on(track.write_sample(&MediaSample {
			data: encoded.into(),
			duration: Duration::from_millis(20),
			..Default::default()
		}))?;
	}
}

fn run_microphone_producer(
	source_id: String,
	track: Arc<TrackLocalStaticSample>,
	stop: Arc<AtomicBool>,
	ready: oneshot::Sender<std::result::Result<(), String>>,
) {
	let setup = (|| -> Result<_> {
		let (device, supported) = webcam::microphone_device(&source_id)?;
		let sample_format = supported.sample_format();
		let sample_rate = supported.sample_rate() as usize;
		let config = supported.into();
		let (samples_tx, samples_rx) = std_mpsc::sync_channel(AUDIO_QUEUE_CAPACITY);
		let stream = match sample_format {
			SampleFormat::I8 => build_input_stream::<i8>(&device, &config, samples_tx)?,
			SampleFormat::I16 => build_input_stream::<i16>(&device, &config, samples_tx)?,
			SampleFormat::I32 => build_input_stream::<i32>(&device, &config, samples_tx)?,
			SampleFormat::F32 => build_input_stream::<f32>(&device, &config, samples_tx)?,
			format => bail!("Unsupported microphone sample format: {format}"),
		};
		stream.play()?;
		Ok((sample_rate, samples_rx, stream))
	})();
	let (sample_rate, samples_rx, _stream) = match setup {
		Ok(setup) => setup,
		Err(error) => {
			let _ = ready.send(Err(error.to_string()));
			return;
		}
	};
	let mut encoder =
		match opus2::Encoder::new(AUDIO_SAMPLE_RATE as u32, Channels::Mono, Application::Voip) {
			Ok(encoder) => encoder,
			Err(error) => {
				let _ = ready.send(Err(format!("failed to initialize Opus encoder: {error}")));
				log::warn!("failed to initialize Opus encoder: {error}");
				return;
			}
		};
	if let Err(error) = encoder.set_bitrate(Bitrate::Bits(AUDIO_BITRATE)) {
		let _ = ready.send(Err(format!("failed to configure Opus encoder: {error}")));
		log::warn!("failed to configure Opus encoder: {error}");
		return;
	}
	let mut resampler = match Async::<f32>::new_poly(
		AUDIO_SAMPLE_RATE as f64 / sample_rate as f64,
		1.0,
		PolynomialDegree::Cubic,
		AUDIO_FRAME_SAMPLES,
		AUDIO_CHANNELS,
		FixedAsync::Output,
	) {
		Ok(resampler) => resampler,
		Err(error) => {
			let _ = ready.send(Err(format!(
				"failed to initialize microphone resampler: {error}"
			)));
			log::warn!("failed to initialize microphone resampler: {error}");
			return;
		}
	};
	let _ = ready.send(Ok(()));
	let handle = tokio::runtime::Handle::current();
	let mut input = VecDeque::with_capacity(AUDIO_FRAME_SAMPLES * 2);
	while !stop.load(Ordering::Relaxed) {
		match samples_rx.recv_timeout(Duration::from_millis(100)) {
			Ok(samples) => {
				input.extend(samples);
				if let Err(error) =
					write_opus_frames(&track, &handle, &mut encoder, &mut resampler, &mut input)
				{
					log::warn!("microphone WebRTC producer stopped: {error}");
					return;
				}
			}
			Err(std_mpsc::RecvTimeoutError::Timeout) => {}
			Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
		}
	}
}

async fn start_microphone_producer(
	source_id: String,
	track: Arc<TrackLocalStaticSample>,
	stop: Arc<AtomicBool>,
) -> Result<()> {
	let (ready_tx, ready_rx) = oneshot::channel();
	tokio::task::spawn_blocking(move || run_microphone_producer(source_id, track, stop, ready_tx));
	tokio::time::timeout(Duration::from_secs(5), ready_rx)
		.await
		.context("timed out starting microphone capture")?
		.context("microphone capture worker stopped during startup")?
		.map_err(anyhow::Error::msg)
}

fn scaled_dimensions(width: u32, height: u32) -> (u32, u32) {
	let scale = (VIDEO_MAX_WIDTH as f64 / width as f64)
		.min(VIDEO_MAX_HEIGHT as f64 / height as f64)
		.min(1.0);
	let width = ((width as f64 * scale).floor() as u32).max(2) & !1;
	let height = ((height as f64 * scale).floor() as u32).max(2) & !1;
	(width, height)
}

fn encode_video_frame(
	encoder: &mut Encoder,
	force_keyframe: &AtomicBool,
	data: &[u8],
) -> Result<Vec<u8>> {
	let image = image::load_from_memory(data)?;
	let (width, height) = scaled_dimensions(image.width(), image.height());
	let image = image
		.resize_exact(width, height, image::imageops::FilterType::Triangle)
		.to_rgb8();
	let rgb = RgbSliceU8::new(image.as_raw(), (width as usize, height as usize));
	let yuv = YUVBuffer::from_rgb8_source(rgb);
	if force_keyframe.swap(false, Ordering::Relaxed) {
		encoder.force_intra_frame();
	}
	Ok(encoder.encode(&yuv)?.to_vec())
}

fn video_encoder(kind: &MediaSourceKind) -> Result<(Encoder, u32)> {
	let (bitrate, fps, usage) = if kind == &MediaSourceKind::Screen {
		(SCREEN_BITRATE, SCREEN_FPS, UsageType::ScreenContentRealTime)
	} else {
		(VIDEO_BITRATE, VIDEO_FPS, UsageType::CameraVideoRealTime)
	};
	let config = EncoderConfig::new()
		.bitrate(BitRate::from_bps(bitrate))
		.max_frame_rate(FrameRate::from_hz(fps as f32))
		.intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2))
		.profile(Profile::Baseline)
		.usage_type(usage);
	let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)?;
	Ok((encoder, fps))
}

async fn run_video_producer(
	source_id: String,
	kind: MediaSourceKind,
	track: Arc<TrackLocalStaticSample>,
	stop: Arc<AtomicBool>,
	force_keyframe: Arc<AtomicBool>,
) {
	let (encoder, fps) = match video_encoder(&kind) {
		Ok(result) => result,
		Err(error) => {
			log::warn!("failed to initialize H.264 encoder: {error}");
			return;
		}
	};
	let encoder = Arc::new(std::sync::Mutex::new(encoder));
	let frame_duration = Duration::from_secs_f64(1.0 / fps as f64);
	while !stop.load(Ordering::Relaxed) {
		let started = tokio::time::Instant::now();
		match webcam::capture_media_frame(source_id.clone()).await {
			Ok(frame) => {
				let encoder = Arc::clone(&encoder);
				let force_keyframe = Arc::clone(&force_keyframe);
				let encoded = tokio::task::spawn_blocking(move || {
					let mut encoder = encoder
						.lock()
						.map_err(|_| anyhow!("H.264 encoder lock was poisoned"))?;
					encode_video_frame(&mut encoder, &force_keyframe, &frame.data)
				})
				.await;
				match encoded {
					Ok(Ok(data)) if !data.is_empty() => {
						if let Err(error) = track
							.write_sample(&MediaSample {
								data: data.into(),
								duration: frame_duration,
								..Default::default()
							})
							.await
						{
							log::warn!("video WebRTC producer failed: {error}");
						}
					}
					Ok(Ok(_)) => {}
					Ok(Err(error)) => log::warn!("failed to encode video frame: {error}"),
					Err(error) => log::warn!("video encoder task failed: {error}"),
				}
			}
			Err(error) => log::warn!("failed to capture video frame: {error}"),
		}
		if let Some(delay) = frame_duration.checked_sub(started.elapsed()) {
			tokio::time::sleep(delay).await;
		}
	}
}

fn start_video_producer(
	source_id: String,
	kind: MediaSourceKind,
	track: Arc<TrackLocalStaticSample>,
	stop: Arc<AtomicBool>,
	force_keyframe: Arc<AtomicBool>,
) {
	tokio::spawn(run_video_producer(
		source_id,
		kind,
		track,
		stop,
		force_keyframe,
	));
}

async fn producer_for_source(source_id: &str, kind: &MediaSourceKind) -> Result<MediaProducer> {
	let track = media_track(source_id, kind);
	let stop = Arc::new(AtomicBool::new(false));
	let force_keyframe = Arc::new(AtomicBool::new(true));
	if kind == &MediaSourceKind::Microphone {
		start_microphone_producer(source_id.to_string(), Arc::clone(&track), Arc::clone(&stop))
			.await?;
	} else if matches!(kind, MediaSourceKind::Webcam | MediaSourceKind::Screen) {
		start_video_producer(
			source_id.to_string(),
			kind.clone(),
			Arc::clone(&track),
			Arc::clone(&stop),
			Arc::clone(&force_keyframe),
		);
	} else {
		bail!("unsupported WebRTC media source");
	}
	Ok(MediaProducer {
		force_keyframe,
		stop,
		subscribers: 1,
		track,
	})
}

impl MediaSessionManager {
	pub(crate) fn new() -> Result<Arc<Self>> {
		let mut media_engine = MediaEngine::default();
		media_engine.register_default_codecs()?;
		let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
		let api = APIBuilder::new()
			.with_media_engine(media_engine)
			.with_interceptor_registry(registry)
			.build();
		Ok(Arc::new(Self {
			api: Arc::new(api),
			producers: Mutex::new(HashMap::new()),
			sessions: Mutex::new(HashMap::new()),
		}))
	}

	async fn acquire_producer(
		&self,
		source_id: &str,
		kind: &MediaSourceKind,
	) -> Result<(Arc<TrackLocalStaticSample>, Arc<AtomicBool>)> {
		let mut producers = self.producers.lock().await;
		if let Some(producer) = producers.get_mut(source_id) {
			producer.subscribers += 1;
			producer.force_keyframe.store(true, Ordering::Relaxed);
			return Ok((
				Arc::clone(&producer.track),
				Arc::clone(&producer.force_keyframe),
			));
		}
		let producer = producer_for_source(source_id, kind).await?;
		let result = (
			Arc::clone(&producer.track),
			Arc::clone(&producer.force_keyframe),
		);
		producers.insert(source_id.to_string(), producer);
		Ok(result)
	}

	async fn release_producer(&self, source_id: &str) {
		let mut producers = self.producers.lock().await;
		let remove = producers
			.get_mut(source_id)
			.map(|producer| {
				producer.subscribers = producer.subscribers.saturating_sub(1);
				producer.subscribers == 0
			})
			.unwrap_or(false);
		if remove && let Some(producer) = producers.remove(source_id) {
			producer.stop.store(true, Ordering::Relaxed);
		}
	}

	async fn source_kind(source_id: &str) -> Result<MediaSourceKind> {
		webcam::list_media_sources()
			.await?
			.into_iter()
			.find(|source| source.id == source_id)
			.map(|source| source.kind)
			.ok_or_else(|| anyhow!("unknown media source"))
	}

	async fn watch_keyframe_requests(
		sender: Arc<webrtc::rtp_transceiver::rtp_sender::RTCRtpSender>,
		force_keyframe: Arc<AtomicBool>,
	) {
		while let Ok((packets, _)) = sender.read_rtcp().await {
			if packets.iter().any(|packet| {
				packet.as_any().is::<PictureLossIndication>()
					|| packet.as_any().is::<FullIntraRequest>()
			}) {
				force_keyframe.store(true, Ordering::Relaxed);
			}
		}
	}

	async fn negotiate(
		&self,
		track: Arc<TrackLocalStaticSample>,
		force_keyframe: Arc<AtomicBool>,
		kind: &MediaSourceKind,
		offer: &SessionDescription,
	) -> Result<(Arc<RTCPeerConnection>, SessionDescription)> {
		if offer.r#type != "offer" {
			bail!("session description must be an offer");
		}
		if offer.sdp.is_empty() || offer.sdp.len() > MAX_SDP_BYTES {
			bail!("invalid SDP size");
		}
		let peer_connection = Arc::new(
			self.api
				.new_peer_connection(RTCConfiguration::default())
				.await?,
		);
		let sender = peer_connection
			.add_track(track as Arc<dyn TrackLocal + Send + Sync>)
			.await?;
		if kind != &MediaSourceKind::Microphone {
			tokio::spawn(Self::watch_keyframe_requests(sender, force_keyframe));
		}
		peer_connection
			.set_remote_description(RTCSessionDescription::offer(offer.sdp.clone())?)
			.await?;
		let answer = peer_connection.create_answer(None).await?;
		let mut gathering_complete = peer_connection.gathering_complete_promise().await;
		peer_connection.set_local_description(answer).await?;
		tokio::time::timeout(GATHER_TIMEOUT, gathering_complete.recv())
			.await
			.context("timed out gathering WebRTC candidates")?;
		let answer = peer_connection
			.local_description()
			.await
			.ok_or_else(|| anyhow!("WebRTC answer was not generated"))?;
		Ok((
			peer_connection,
			SessionDescription {
				r#type: String::from("answer"),
				sdp: answer.sdp,
			},
		))
	}

	pub(crate) async fn remove_session(&self, session_id: &str) -> bool {
		let Some(session) = self.sessions.lock().await.remove(session_id) else {
			return false;
		};
		if let Err(error) = session.peer_connection.close().await {
			log::warn!("failed to close WebRTC media session {session_id}: {error}");
		}
		self.release_producer(&session.source_id).await;
		true
	}

	async fn remove_if_disconnected(&self, session_id: &str) {
		let disconnected = self
			.sessions
			.lock()
			.await
			.get(session_id)
			.map(|session| {
				session.peer_connection.connection_state() == RTCPeerConnectionState::Disconnected
			})
			.unwrap_or(false);
		if disconnected {
			self.remove_session(session_id).await;
		}
	}

	async fn set_connection_cleanup(
		self: &Arc<Self>,
		peer_connection: &RTCPeerConnection,
		session_id: String,
	) {
		let manager = Arc::downgrade(self);
		peer_connection.on_peer_connection_state_change(Box::new(move |state| {
			let manager = manager.clone();
			let session_id = session_id.clone();
			Box::pin(async move {
				match state {
					RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
						if let Some(manager) = manager.upgrade() {
							manager.remove_session(&session_id).await;
						}
					}
					RTCPeerConnectionState::Disconnected => {
						tokio::time::sleep(Duration::from_secs(10)).await;
						if let Some(manager) = manager.upgrade() {
							manager.remove_if_disconnected(&session_id).await;
						}
					}
					_ => {}
				}
			})
		}));
	}

	pub(crate) async fn create_session(
		self: &Arc<Self>,
		request: CreateMediaSession,
	) -> Result<CreatedMediaSession> {
		let kind = Self::source_kind(&request.source_id).await?;
		let (track, force_keyframe) = self.acquire_producer(&request.source_id, &kind).await?;
		let negotiated = self
			.negotiate(track, force_keyframe, &kind, &request.offer)
			.await;
		let (peer_connection, answer) = match negotiated {
			Ok(negotiated) => negotiated,
			Err(error) => {
				self.release_producer(&request.source_id).await;
				return Err(error);
			}
		};
		let session_id = uuid::Uuid::new_v4().to_string();
		self.sessions.lock().await.insert(
			session_id.clone(),
			MediaSession {
				peer_connection: Arc::clone(&peer_connection),
				source_id: request.source_id,
			},
		);
		self.set_connection_cleanup(&peer_connection, session_id.clone())
			.await;
		Ok(CreatedMediaSession { session_id, answer })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
	use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
	use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;

	#[test]
	fn downmixes_interleaved_audio() {
		let output = downmix_samples(&[1.0_f32, -1.0, 0.5, 0.5], 2);
		assert_eq!(output, vec![0.0, 0.5]);
	}

	#[test]
	fn scales_video_to_even_bounded_dimensions() {
		assert_eq!(scaled_dimensions(1920, 1080), (960, 540));
		assert_eq!(scaled_dimensions(641, 481), (640, 480));
		assert_eq!(scaled_dimensions(320, 240), (320, 240));
	}

	#[test]
	fn resamples_and_encodes_twenty_millisecond_opus_frame() {
		let mut resampler = Async::<f32>::new_poly(
			AUDIO_SAMPLE_RATE as f64 / 44_100.0,
			1.0,
			PolynomialDegree::Cubic,
			AUDIO_FRAME_SAMPLES,
			AUDIO_CHANNELS,
			FixedAsync::Output,
		)
		.unwrap();
		let input_frames = resampler.input_frames_next();
		let input = vec![0.0_f32; input_frames];
		let adapter = InterleavedSlice::new(&input, AUDIO_CHANNELS, input_frames).unwrap();
		let output = resampler.process(&adapter, 0, None).unwrap().take_data();
		let mut encoder =
			opus2::Encoder::new(AUDIO_SAMPLE_RATE as u32, Channels::Mono, Application::Voip)
				.unwrap();
		let encoded = encoder.encode_vec_float(&output, 4000).unwrap();

		assert_eq!(output.len(), AUDIO_FRAME_SAMPLES);
		assert!(!encoded.is_empty());
	}

	#[test]
	fn encodes_browser_compatible_h264_sample() {
		let image = image::DynamicImage::new_rgb8(16, 16);
		let mut encoded_image = std::io::Cursor::new(Vec::new());
		image
			.write_to(&mut encoded_image, image::ImageFormat::Png)
			.unwrap();
		let (mut encoder, _) = video_encoder(&MediaSourceKind::Webcam).unwrap();
		let force_keyframe = AtomicBool::new(true);
		let encoded =
			encode_video_frame(&mut encoder, &force_keyframe, &encoded_image.into_inner()).unwrap();

		assert!(encoded.starts_with(&[0, 0, 0, 1]));
		assert!(!force_keyframe.load(Ordering::Relaxed));
	}

	#[tokio::test]
	async fn negotiates_receiver_only_audio_session() {
		let manager = MediaSessionManager::new().unwrap();
		let mut media_engine = MediaEngine::default();
		media_engine.register_default_codecs().unwrap();
		let browser_api = APIBuilder::new().with_media_engine(media_engine).build();
		let browser = browser_api
			.new_peer_connection(RTCConfiguration::default())
			.await
			.unwrap();
		browser
			.add_transceiver_from_kind(
				RTPCodecType::Audio,
				Some(RTCRtpTransceiverInit {
					direction: RTCRtpTransceiverDirection::Recvonly,
					send_encodings: Vec::new(),
				}),
			)
			.await
			.unwrap();
		let offer = browser.create_offer(None).await.unwrap();
		let mut gathering_complete = browser.gathering_complete_promise().await;
		browser.set_local_description(offer).await.unwrap();
		gathering_complete.recv().await;
		let offer = browser.local_description().await.unwrap();
		let track = media_track("microphone:test", &MediaSourceKind::Microphone);
		let (server, answer) = manager
			.negotiate(
				track,
				Arc::new(AtomicBool::new(false)),
				&MediaSourceKind::Microphone,
				&SessionDescription {
					r#type: String::from("offer"),
					sdp: offer.sdp,
				},
			)
			.await
			.unwrap();
		browser
			.set_remote_description(RTCSessionDescription::answer(answer.sdp).unwrap())
			.await
			.unwrap();

		assert!(browser.remote_description().await.is_some());
		server.close().await.unwrap();
		browser.close().await.unwrap();
	}
}
