const css = `
.media-receiver {
	display: grid;
	gap: 8px;
	width: 100%;
	font: 13px system-ui, sans-serif;
	color: #d6eee9;
}
.media-receiver-controls, .media-receiver-stats {
	display: flex;
	flex-wrap: wrap;
	gap: 8px;
	align-items: center;
}
.media-receiver button, .media-receiver select {
	min-height: 34px;
	border: 1px solid #2d6258;
	background: #020807;
	color: #79f2c0;
	font: inherit;
}
.media-receiver button:disabled {
	color: #78928b;
	border-color: #1f4b44;
}
.media-receiver-video {
	display: grid;
	place-items: center;
	width: 100%;
	aspect-ratio: 16 / 9;
	border: 1px solid #2d6258;
	background: #020807;
	overflow: hidden;
}
.media-receiver-video video, .media-receiver-video img {
	display: block;
	width: 100%;
	height: 100%;
	object-fit: contain;
}
.media-receiver-resolution {
	display: flex;
	align-items: center;
	gap: 6px;
}
.media-receiver-fps {
	color: #9fbdb6;
}
.media-receiver-status {
	min-height: 18px;
	color: #9fbdb6;
}
.media-receiver-error {
	color: #ff8f8f;
}
`;

function joinBytes(parts) {
	const length = parts.reduce((total, part) => total + part.length, 0);
	const joined = new Uint8Array(length);
	let offset = 0;
	for (const part of parts) {
		joined.set(part, offset);
		offset += part.length;
	}
	return joined;
}

function h264Codec(init) {
	for (let index = 0; index + 8 <= init.length; index++) {
		if (String.fromCharCode(...init.subarray(index, index + 4)) !== "avcC") continue;
		const profile = init[index + 5]?.toString(16).padStart(2, "0");
		const compatibility = init[index + 6]?.toString(16).padStart(2, "0");
		const level = init[index + 7]?.toString(16).padStart(2, "0");
		if (profile && compatibility && level) return `avc1.${profile}${compatibility}${level}`;
	}
	throw new Error("ONVIF stream did not provide H.264 codec settings");
}

export default class MediaReceiver {
	constructor(element, context) {
		this.element = element;
		this.context = context;
		this.endpoint = "";
		this.sourceId = "";
		this.kind = "video";
		this.transport = "";
		this.autoStart = false;
		this.revision = "";
		this.running = false;
		this.starting = false;
		this.videoFrameCallback = 0;
	}

	mount(props) {
		this.element.innerHTML = "";
		this.style = document.createElement("style");
		this.style.textContent = css;
		this.root = document.createElement("div");
		this.root.className = "media-receiver";
		this.controls = document.createElement("div");
		this.controls.className = "media-receiver-controls";
		this.startButton = this.button("Start");
		this.stopButton = this.button("Stop");
		this.controls.append(this.startButton, this.stopButton);
		this.stats = document.createElement("div");
		this.stats.className = "media-receiver-stats";
		this.status = document.createElement("div");
		this.status.className = "media-receiver-status";
		this.root.append(this.controls, this.stats, this.status);
		this.element.append(this.style, this.root);
		this.startButton.addEventListener("click", () => this.start());
		this.stopButton.addEventListener("click", () => this.stop("Media stream stopped.", false));
		this.setProps(props);
	}

	setProps(props) {
		const endpoint = String(props?.endpoint ?? "");
		const sourceId = String(props?.source_id ?? "");
		const kind = props?.media_kind === "audio" ? "audio" : "video";
		const transport = String(props?.transport ?? "");
		const revision = String(props?.revision ?? "");
		this.autoStart = Boolean(props?.auto_start);
		this.resolutionOptions = Array.isArray(props?.resolution_options) ? props.resolution_options : [];
		this.resolutionSelectable = Boolean(props?.resolution_selectable);
		const changed = endpoint !== this.endpoint || sourceId !== this.sourceId || kind !== this.kind
			|| transport !== this.transport || Boolean(revision && this.revision && revision !== this.revision);
		this.endpoint = endpoint;
		this.sourceId = sourceId;
		this.kind = kind;
		this.transport = transport;
		if (revision) this.revision = revision;
		if (changed) {
			this.stop("", false);
			this.createMediaElement();
		}
		this.updateResolutionSelect();
		this.setButtons();
		if (!this.endpoint || !this.sourceId) {
			this.setStatus("Media stream is not available.", false);
		} else if (!this.running && !this.starting) {
			this.setStatus("Media stream is ready.", false);
			if (this.autoStart) queueMicrotask(() => this.start());
		}
	}

	dispose() {
		this.stop("", false);
	}

	button(label) {
		const button = document.createElement("button");
		button.type = "button";
		button.textContent = label;
		return button;
	}

	createMediaElement() {
		this.media?.remove();
		this.frame?.remove();
		this.fullscreenButton?.remove();
		this.volumeControls?.remove();
		this.muteButton?.remove();
		this.media = null;
		this.frame = null;
		this.fullscreenButton = null;
		this.volumeControls = null;
		this.muteButton = null;
		this.configuredFps = "";
		this.stopVideoStats();
		this.stats.replaceChildren();
		if (this.kind === "audio") {
			this.createAudioControls();
			return;
		}
		this.media = document.createElement(this.transport === "mjpeg" ? "img" : "video");
		if (this.media instanceof HTMLVideoElement) {
			this.media.autoplay = true;
			this.media.playsInline = true;
			this.media.muted = true;
			this.media.addEventListener("playing", () => {
				this.setStatus("Media stream playing.", false);
				this.startVideoStats();
			});
		} else {
			this.media.alt = "Live media stream";
			this.media.addEventListener("load", () => this.recordMjpegFrame());
			this.media.addEventListener("error", () => this.stop("Media stream failed.", true));
		}
		this.frame = document.createElement("div");
		this.frame.className = "media-receiver-video";
		this.frame.append(this.media);
		this.root.insertBefore(this.frame, this.status);
		this.fullscreenButton = this.button("Fullscreen");
		this.fullscreenButton.addEventListener("click", () => this.frame.requestFullscreen?.());
		this.controls.append(this.fullscreenButton);
		if (this.resolutionSelectable) {
			this.resolutionStat = document.createElement("label");
			this.resolutionStat.className = "media-receiver-resolution";
			this.resolutionStat.textContent = "Resolution:";
			this.resolutionSelect = document.createElement("select");
			this.resolutionSelect.addEventListener("change", () => {
				this.context?.emit("resolutionChange", this.resolutionSelect.value);
			});
			this.resolutionStat.append(this.resolutionSelect);
			this.stats.append(this.resolutionStat);
		} else {
			this.resolutionStat = document.createElement("span");
			this.resolutionStat.textContent = "Resolution: --";
			this.resolutionSelect = null;
			this.stats.append(this.resolutionStat);
		}
		this.fpsStat = document.createElement("span");
		this.fpsStat.className = "media-receiver-fps";
		this.fpsStat.textContent = "FPS: --";
		this.stats.append(this.fpsStat);
	}

	createAudioControls() {
		this.volumeControls = document.createElement("label");
		this.volumeControls.textContent = "Volume";
		this.volume = document.createElement("input");
		this.volume.type = "range";
		this.volume.min = "0";
		this.volume.max = "100";
		this.volume.value = "100";
		this.volume.addEventListener("input", () => this.setAudioVolume());
		this.volumeControls.append(this.volume);
		this.muteButton = this.button("Mute");
		this.muteButton.addEventListener("click", () => {
			this.muted = !this.muted;
			this.muteButton.textContent = this.muted ? "Unmute" : "Mute";
			this.setAudioVolume();
		});
		this.controls.append(this.volumeControls, this.muteButton);
	}

	updateResolutionSelect() {
		if (!this.resolutionSelect) return;
		const options = this.resolutionOptions.length > 0
			? this.resolutionOptions
			: [{ value: this.revision, name: "Loading resolutions..." }];
		this.resolutionSelect.replaceChildren(...options.map((option) => {
			const element = document.createElement("option");
			element.value = String(option.value ?? "");
			element.textContent = String(option.name ?? option.value ?? "");
			return element;
		}));
		if (this.revision) this.resolutionSelect.value = this.revision;
		this.updateConfiguredFps();
	}

	updateConfiguredFps() {
		if (!this.fpsStat || !this.resolutionSelect) return;
		const match = this.resolutionSelect.selectedOptions[0]?.textContent.match(/@ (\d+) FPS/);
		this.configuredFps = match?.[1] ?? "";
		this.setFpsText();
	}

	setFpsText(actual = "") {
		if (!this.fpsStat) return;
		const values = [];
		if (this.configuredFps) values.push(`configured ${this.configuredFps}`);
		if (actual) values.push(`actual ${actual}`);
		this.fpsStat.textContent = `FPS: ${values.join(" · ") || "--"}`;
	}

	streamUrl() {
		return `${this.endpoint}${this.endpoint.includes("?") ? "&" : "?"}_=${Date.now()}`;
	}

	async start() {
		if (this.starting || this.running || !this.endpoint || !this.sourceId) return;
		this.cancelled = false;
		this.starting = true;
		this.setButtons();
		this.setStatus("Connecting media stream...", false);
		try {
			this.running = true;
			this.setButtons();
			if (this.kind === "audio") {
				await this.startPcm();
			} else if (this.transport === "fmp4") {
				await this.startFmp4();
			} else if (this.transport === "mjpeg") {
				await this.startMjpeg();
			} else {
				throw new Error("unsupported media transport");
			}
		} catch (error) {
			if (!this.cancelled) this.stop(`Media stream failed: ${error}`, true);
		} finally {
			this.starting = false;
			this.setButtons();
		}
	}

	async startMjpeg() {
		let retryDelay = 1000;
		while (!this.cancelled) {
			try {
				await this.startMjpegOnce();
				if (this.cancelled) return;
				throw new Error("media stream ended");
			} catch (error) {
				if (this.cancelled) return;
				this.abortController?.abort();
				this.abortController = null;
				this.setStatus(`Media stream interrupted: ${error}. Reconnecting...`, false);
				await this.waitForRetry(retryDelay);
				retryDelay = Math.min(retryDelay * 2, 5000);
			}
		}
	}

	async startMjpegOnce() {
		this.abortController = new AbortController();
		const response = await fetch(this.streamUrl(), {
			credentials: "include",
			signal: this.abortController.signal,
		});
		if (!response.ok) throw new Error(`request failed: ${response.status}`);
		if (!response.body) throw new Error("streaming response is unavailable");
		const reader = response.body.getReader();
		let pending = new Uint8Array();
		while (!this.abortController.signal.aborted) {
			const { done, value } = await reader.read();
			if (done) break;
			pending = joinBytes([pending, value]);
			while (true) {
				const headerEnd = this.mjpegHeaderEnd(pending);
				if (headerEnd < 0) break;
				const header = new TextDecoder().decode(pending.subarray(0, headerEnd));
				const length = Number(header.match(/content-length:\s*(\d+)/i)?.[1]);
				if (!Number.isSafeInteger(length) || length < 1 || length > 16 * 1024 * 1024) {
					throw new Error("invalid MJPEG frame length");
				}
				const dataStart = headerEnd + 4;
				const dataEnd = dataStart + length;
				if (pending.length < dataEnd + 2) break;
				this.showMjpegFrame(pending.slice(dataStart, dataEnd));
				pending = pending.slice(dataEnd + 2);
			}
		}
	}

	mjpegHeaderEnd(data) {
		for (let index = 0; index + 4 <= data.length; index++) {
			if (data[index] === 13 && data[index + 1] === 10
				&& data[index + 2] === 13 && data[index + 3] === 10) return index;
		}
		return -1;
	}

	showMjpegFrame(data) {
		if (!(this.media instanceof HTMLImageElement)) return;
		const previousUrl = this.frameUrl;
		this.frameUrl = URL.createObjectURL(new Blob([data], { type: "image/jpeg" }));
		this.media.src = this.frameUrl;
		if (previousUrl) URL.revokeObjectURL(previousUrl);
	}

	async startFmp4() {
		let retryDelay = 1000;
		while (!this.cancelled) {
			try {
				await this.startFmp4Once();
				if (this.cancelled) return;
				throw new Error("media stream ended");
			} catch (error) {
				if (this.cancelled) return;
				const message = String(error);
				if (message.includes("Media Source Extensions are unavailable")
					|| message.includes("unsupported camera codec")
					|| message.includes("did not provide H.264 codec settings")) {
					throw error;
				}
				this.resetFmp4();
				this.setStatus(`Media stream interrupted: ${error}. Reconnecting...`, false);
				await this.waitForRetry(retryDelay);
				retryDelay = Math.min(retryDelay * 2, 5000);
			}
		}
	}

	async startFmp4Once() {
		if (!window.MediaSource) throw new Error("Media Source Extensions are unavailable");
		this.abortController = new AbortController();
		this.mediaSource = new MediaSource();
		this.mediaUrl = URL.createObjectURL(this.mediaSource);
		this.media.src = this.mediaUrl;
		await new Promise((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error("timed out opening media source")), 10000);
			this.mediaSource.addEventListener("sourceopen", () => {
				clearTimeout(timeout);
				resolve();
			}, { once: true });
		});
		const response = await fetch(this.streamUrl(), { credentials: "include", signal: this.abortController.signal });
		if (!response.ok) throw new Error(`request failed: ${response.status}`);
		if (!response.body) throw new Error("streaming response is unavailable");
		const reader = response.body.getReader();
		let pending = new Uint8Array();
		let init = [];
		let fragment = [];
		while (!this.abortController.signal.aborted) {
			const { done, value } = await reader.read();
			if (done) break;
			pending = joinBytes([pending, value]);
			while (pending.length >= 8) {
				const size = new DataView(pending.buffer, pending.byteOffset).getUint32(0);
				if (size < 8 || size > pending.length) break;
				const box = pending.slice(0, size);
				pending = pending.slice(size);
				const type = String.fromCharCode(...box.subarray(4, 8));
				if (type === "ftyp" || type === "moov") {
					init.push(box);
					if (type === "moov") {
						const data = joinBytes(init);
						const mime = `video/mp4; codecs="${h264Codec(data)}"`;
						if (!MediaSource.isTypeSupported(mime)) throw new Error(`unsupported camera codec: ${mime}`);
						this.sourceBuffer = this.mediaSource.addSourceBuffer(mime);
						this.appendQueue = [data];
						this.sourceBuffer.addEventListener("updateend", () => this.flushMseQueue());
						this.flushMseQueue();
					}
				} else if (type === "moof") {
					fragment = [box];
				} else if (type === "mdat" && fragment.length > 0) {
					fragment.push(box);
					this.queueMseAppend(joinBytes(fragment));
					fragment = [];
				}
			}
		}
	}

	waitForRetry(delay) {
		return new Promise((resolve) => {
			this.retryResolve = resolve;
			this.retryTimer = setTimeout(() => {
				this.retryTimer = 0;
				this.retryResolve = null;
				resolve();
			}, delay);
		});
	}

	resetFmp4() {
		this.abortController?.abort();
		this.abortController = null;
		this.stopVideoStats();
		if (this.media instanceof HTMLVideoElement) {
			this.media.pause();
			this.media.removeAttribute("src");
			this.media.load();
		}
		if (this.mediaUrl) URL.revokeObjectURL(this.mediaUrl);
		this.mediaUrl = null;
		this.sourceBuffer = null;
		this.appendQueue = [];
		this.mediaSource = null;
	}

	startVideoStats() {
		this.stopVideoStats();
		if (!(this.media instanceof HTMLVideoElement)) return;
		let previousFrames;
		let previousTime;
		const update = (time, metadata) => {
			if (!this.running) return;
			if (this.resolutionStat && this.media.videoWidth && this.media.videoHeight) {
				if (!this.resolutionSelect) {
					this.resolutionStat.textContent = `Resolution: ${this.media.videoWidth} × ${this.media.videoHeight}`;
				}
			}
			if (previousFrames !== undefined && time > previousTime) {
				const fps = (metadata.presentedFrames - previousFrames) * 1000 / (time - previousTime);
				this.setFpsText(fps.toFixed(1));
			}
			previousFrames = metadata.presentedFrames;
			previousTime = time;
			this.videoFrameCallback = this.media.requestVideoFrameCallback(update);
		};
		if (this.media.requestVideoFrameCallback) {
			this.videoFrameCallback = this.media.requestVideoFrameCallback(update);
		} else if (this.media.videoWidth && this.media.videoHeight && !this.resolutionSelect) {
			this.resolutionStat.textContent = `Resolution: ${this.media.videoWidth} × ${this.media.videoHeight}`;
		}
	}

	recordMjpegFrame() {
		if (!(this.media instanceof HTMLImageElement)) return;
		this.setStatus("Media stream playing.", false);
		if (this.resolutionStat && this.media.naturalWidth && this.media.naturalHeight) {
			this.resolutionStat.textContent = `Resolution: ${this.media.naturalWidth} × ${this.media.naturalHeight}`;
		}
		const now = performance.now();
		if (!this.mjpegFpsStarted) {
			this.mjpegFpsStarted = now;
			this.mjpegFrames = 0;
		}
		this.mjpegFrames++;
		const elapsed = now - this.mjpegFpsStarted;
		if (elapsed >= 500) {
			this.setFpsText((this.mjpegFrames * 1000 / elapsed).toFixed(1));
			this.mjpegFpsStarted = now;
			this.mjpegFrames = 0;
		}
	}

	stopVideoStats() {
		if (this.videoFrameCallback && this.media?.cancelVideoFrameCallback) {
			this.media.cancelVideoFrameCallback(this.videoFrameCallback);
		}
		this.videoFrameCallback = 0;
		this.mjpegFpsStarted = 0;
		this.mjpegFrames = 0;
		this.setFpsText();
		if (this.resolutionStat && !this.resolutionSelect) this.resolutionStat.textContent = "Resolution: --";
	}

	queueMseAppend(data) {
		if (!this.sourceBuffer) return;
		this.appendQueue.push(data);
		this.flushMseQueue();
	}

	flushMseQueue() {
		if (!this.sourceBuffer || this.sourceBuffer.updating || this.appendQueue.length === 0) return;
		this.sourceBuffer.appendBuffer(this.appendQueue.shift());
	}

	async startPcm() {
		this.abortController = new AbortController();
		const response = await fetch(this.streamUrl(), { credentials: "include", signal: this.abortController.signal });
		if (!response.ok) throw new Error(`request failed: ${response.status}`);
		if (!response.body) throw new Error("streaming response is unavailable");
		const AudioContext = window.AudioContext || window.webkitAudioContext;
		if (!AudioContext) throw new Error("Web Audio is unavailable");
		this.audioContext = new AudioContext();
		this.audioGain = this.audioContext.createGain();
		this.audioGain.connect(this.audioContext.destination);
		this.setAudioVolume();
		const sampleRate = Number(response.headers.get("x-audio-sample-rate"));
		const channels = Number(response.headers.get("x-audio-channels"));
		if (!Number.isFinite(sampleRate) || !Number.isFinite(channels) || channels < 1) {
			throw new Error("invalid microphone stream format");
		}
		const reader = response.body.getReader();
		let pending = new Uint8Array();
		this.audioNextTime = this.audioContext.currentTime + 0.12;
		while (!this.abortController.signal.aborted) {
			const { done, value } = await reader.read();
			if (done) break;
			pending = joinBytes([pending, value]);
			const bytesPerFrame = channels * 2;
			const length = pending.length - pending.length % bytesPerFrame;
			if (!length) continue;
			this.playPcm(pending.subarray(0, length), sampleRate, channels);
			pending = pending.slice(length);
		}
		if (!this.abortController.signal.aborted) throw new Error("microphone stream ended");
	}

	playPcm(data, sampleRate, channels) {
		const frames = data.length / (channels * 2);
		const buffer = this.audioContext.createBuffer(channels, frames, sampleRate);
		for (let channel = 0; channel < channels; channel++) {
			const output = buffer.getChannelData(channel);
			for (let frame = 0; frame < frames; frame++) {
				const offset = (frame * channels + channel) * 2;
				const value = data[offset] | data[offset + 1] << 8;
				output[frame] = (value & 0x8000 ? value - 0x10000 : value) / 0x8000;
			}
		}
		const source = this.audioContext.createBufferSource();
		source.buffer = buffer;
		source.connect(this.audioGain);
		this.audioNextTime = Math.max(this.audioNextTime, this.audioContext.currentTime + 0.08);
		source.start(this.audioNextTime);
		this.audioNextTime += buffer.duration;
	}

	setAudioVolume() {
		if (this.audioGain) this.audioGain.gain.value = this.muted ? 0 : Number(this.volume?.value ?? 100) / 100;
	}

	stop(message, error) {
		this.cancelled = true;
		if (this.retryTimer) clearTimeout(this.retryTimer);
		this.retryTimer = 0;
		this.retryResolve?.();
		this.retryResolve = null;
		this.stopVideoStats();
		if (this.media instanceof HTMLImageElement) this.media.removeAttribute("src");
		if (this.frameUrl) URL.revokeObjectURL(this.frameUrl);
		this.frameUrl = null;
		this.resetFmp4();
		this.audioContext?.close().catch(() => {});
		this.audioContext = null;
		this.audioGain = null;
		this.running = false;
		this.starting = false;
		this.setButtons();
		if (message) this.setStatus(message, error);
	}

	setButtons() {
		const available = Boolean(this.endpoint && this.sourceId);
		this.startButton.disabled = !available || this.starting || this.running;
		this.stopButton.disabled = !this.starting && !this.running;
		if (this.fullscreenButton) this.fullscreenButton.disabled = !this.running;
	}

	setStatus(text, error) {
		this.status.textContent = text;
		this.status.classList.toggle("media-receiver-error", error);
	}
}
