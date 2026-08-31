const css = `
.media-receiver {
	display: grid;
	gap: 8px;
	width: 100%;
	font: 13px system-ui, sans-serif;
	color: #d6eee9;
}
.media-receiver-controls {
	display: flex;
	flex-wrap: wrap;
	gap: 8px;
	align-items: center;
}
.media-receiver button {
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
.media-receiver video {
	display: block;
	width: 100%;
	height: 100%;
	object-fit: contain;
}
.media-receiver-audio {
	display: none;
}
.media-receiver-volume {
	display: flex;
	gap: 6px;
	align-items: center;
}
.media-receiver-volume input {
	width: 150px;
}
.media-receiver-level {
	display: flex;
	gap: 6px;
	align-items: center;
	width: 100%;
}
.media-receiver-level-label {
	min-width: 44px;
	text-align: right;
	color: #9fbdb6;
}
.media-receiver-level-track {
	flex: 1;
	height: 10px;
	border: 1px solid #2d6258;
	background: #020807;
	overflow: hidden;
}
.media-receiver-level-fill {
	height: 100%;
	width: 0%;
	background: linear-gradient(90deg, #79f2c0, #f2f079, #ff8f8f);
}
.media-receiver-status {
	min-height: 18px;
	color: #9fbdb6;
}
.media-receiver-stats {
	display: flex;
	flex-wrap: wrap;
	align-items: center;
	gap: 12px;
	color: #9fbdb6;
}
.media-receiver-resolution {
	display: flex;
	align-items: center;
	gap: 6px;
}
.media-receiver-resolution select {
	border: 1px solid #2d6258;
	background: #020807;
	color: #d6eee9;
	font: inherit;
}
.media-receiver-fps {
	display: flex;
	align-items: center;
	gap: 6px;
}
.media-receiver-fps select {
	border: 1px solid #2d6258;
	background: #020807;
	color: #d6eee9;
	font: inherit;
}
.media-receiver-error {
	color: #ff8f8f;
}
`;

export default class MediaReceiver {
	constructor(element, context) {
		this.element = element;
		this.context = context;
		this.endpoint = "";
		this.sourceId = "";
		this.kind = "video";
		this.autoStart = false;
		this.revision = "";
		this.fps = 30;
		this.resolution = "";
		this.resolutionMode = "";
		this.peerConnection = null;
		this.sessionId = "";
		this.starting = false;
		this.videoStatsTimer = 0;
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
		this.status = document.createElement("div");
		this.status.className = "media-receiver-status";
		this.stats = document.createElement("div");
		this.stats.className = "media-receiver-stats";
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
		const autoStart = Boolean(props?.auto_start);
		const revision = String(props?.revision ?? "");
		this.resolutionOptions = Array.isArray(props?.resolution_options) ? props.resolution_options : [];
		this.resolutionSelectable = Boolean(props?.resolution_selectable);
		this.resolutionMode = String(props?.resolution_mode ?? "");
		const revisionChanged = Boolean(revision && this.revision && revision !== this.revision);
		const changed = endpoint !== this.endpoint || sourceId !== this.sourceId || kind !== this.kind || revisionChanged;
		this.endpoint = endpoint;
		this.sourceId = sourceId;
		this.kind = kind;
		this.autoStart = autoStart;
		if (revision) this.revision = revision;
		if (changed) {
			this.stop("", false);
			this.createMediaElement();
		}
		this.updateResolutionSelect();
		this.setButtons();
		if (!this.endpoint || !this.sourceId) {
			this.setStatus("Media stream is not available.", false);
		} else if (!this.peerConnection && !this.starting) {
			this.setStatus("Media stream is ready.", false);
			if (this.autoStart) {
				queueMicrotask(() => this.start());
			}
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

	updateResolutionSelect() {
		if (!this.resolutionSelect) {
			return;
		}
		const options = this.resolutionOptions.length > 0
			? this.resolutionOptions
			: [{ value: this.revision, name: "Loading resolutions..." }];
		this.resolutionSelect.replaceChildren(...options.map((option) => {
			const element = document.createElement("option");
			element.value = String(option.value ?? "");
			element.textContent = String(option.name ?? option.value ?? "");
			return element;
		}));
		const selected = this.resolutionMode === "profile" ? this.revision : this.resolution;
		if (selected) this.resolutionSelect.value = selected;
		if (!this.resolutionSelect.value && options.length > 0) {
			this.resolutionSelect.value = String(options[0].value ?? "");
		}
		if (this.resolutionMode === "capture") this.resolution = this.resolutionSelect.value;
		if (this.resolutionMode === "profile" && this.fpsSelect) {
			const match = this.resolutionSelect.selectedOptions[0]?.textContent.match(/@ (\d+) FPS/);
			if (match) {
				this.fps = Number(match[1]);
				this.fpsSelect.value = match[1];
			}
		}
	}

	createMediaElement() {
		if (this.media?.isConnected) {
			this.media.remove();
		}
		if (this.fullscreenButton) {
			this.fullscreenButton.remove();
			this.fullscreenButton = null;
		}
		if (this.muteButton) {
			this.muteButton.remove();
			this.muteButton = null;
		}
		if (this.frame) {
			this.frame.remove();
		}
		if (this.volumeControls) {
			this.volumeControls.remove();
			this.volumeControls = null;
		}
		if (this.levelMeter) {
			this.levelMeter.remove();
			this.levelMeter = null;
			this.levelFill = null;
			this.levelLabel = null;
		}
		this.stopLevelMeter();
		this.stopVideoStats();
		this.media = document.createElement(this.kind);
		this.media.autoplay = true;
		this.media.playsInline = true;
		this.media.addEventListener("playing", () => {
			this.setStatus("Media stream playing.", false);
			this.startVideoStats();
		});
		if (this.kind === "video") {
			this.media.muted = true;
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
					if (this.resolutionMode === "profile") {
						this.context?.emit("resolutionChange", this.resolutionSelect.value);
						return;
					}
					this.resolution = this.resolutionSelect.value;
					const restart = Boolean(this.peerConnection || this.starting);
					this.stop("", false);
					if (restart) setTimeout(() => this.start(), 2500);
				});
				this.resolutionStat.append(this.resolutionSelect);
			} else {
				this.resolutionStat = document.createElement("span");
				this.resolutionStat.textContent = "Resolution: --";
				this.resolutionSelect = null;
			}
			this.fpsStat = document.createElement("span");
			this.fpsStat.className = "media-receiver-fps";
			this.fpsStat.textContent = "FPS:";
			this.fpsSelect = document.createElement("select");
			for (const fps of [5, 10, 15, 30]) {
				const option = document.createElement("option");
				option.value = String(fps);
				option.textContent = String(fps);
				this.fpsSelect.append(option);
			}
			this.fpsSelect.value = String(this.fps);
			this.fpsSelect.addEventListener("change", () => {
				this.fps = Number(this.fpsSelect.value);
				const restart = Boolean(this.peerConnection || this.starting);
				this.stop("", false);
				if (restart) setTimeout(() => this.start(), 2500);
			});
			this.fpsActual = document.createElement("span");
			this.fpsActual.textContent = "actual --";
			this.fpsStat.append(this.fpsSelect, this.fpsActual);
			this.stats.replaceChildren(this.resolutionStat, this.fpsStat);
			this.updateResolutionSelect();
		} else {
			this.stats.replaceChildren();
			this.frame = null;
			this.media.className = "media-receiver-audio";
			this.root.insertBefore(this.media, this.status);
			this.levelMeter = document.createElement("div");
			this.levelMeter.className = "media-receiver-level";
			this.levelLabel = document.createElement("span");
			this.levelLabel.className = "media-receiver-level-label";
			this.levelLabel.textContent = "0%";
			this.levelTrack = document.createElement("div");
			this.levelTrack.className = "media-receiver-level-track";
			this.levelFill = document.createElement("div");
			this.levelFill.className = "media-receiver-level-fill";
			this.levelTrack.append(this.levelFill);
			this.levelMeter.append(this.levelLabel, this.levelTrack);
			this.root.insertBefore(this.levelMeter, this.status);
			this.volumeControls = document.createElement("label");
			this.volumeControls.className = "media-receiver-volume";
			this.volumeControls.textContent = "Volume";
			this.volume = document.createElement("input");
			this.volume.type = "range";
			this.volume.min = "0";
			this.volume.max = "100";
			this.volume.value = "100";
			this.volume.addEventListener("input", () => {
				this.media.volume = Number(this.volume.value) / 100;
			});
			this.muteButton = this.button("Mute");
			this.muteButton.addEventListener("click", () => {
				this.media.muted = !this.media.muted;
				this.muteButton.textContent = this.media.muted ? "Unmute" : "Mute";
			});
			this.volumeControls.append(this.volume);
			this.controls.append(this.volumeControls, this.muteButton);
		}
	}

	async waitForIceGathering(peerConnection) {
		if (peerConnection.iceGatheringState === "complete") {
			return;
		}
		await new Promise((resolve, reject) => {
			const timeout = setTimeout(() => {
				peerConnection.removeEventListener("icegatheringstatechange", checkState);
				reject(new Error("timed out gathering WebRTC candidates"));
			}, 10000);
			const checkState = () => {
				if (peerConnection.iceGatheringState === "complete") {
					clearTimeout(timeout);
					peerConnection.removeEventListener("icegatheringstatechange", checkState);
					resolve();
				}
			};
			peerConnection.addEventListener("icegatheringstatechange", checkState);
		});
	}

	async responseError(response) {
		try {
			const body = await response.json();
			return body.error || `request failed: ${response.status}`;
		} catch (_) {
			return `request failed: ${response.status}`;
		}
	}

	async start() {
		if (this.starting || this.peerConnection || !this.endpoint || !this.sourceId) {
			return;
		}
		this.starting = true;
		this.setButtons();
		this.setStatus("Connecting media stream...", false);
		const peerConnection = new RTCPeerConnection({ iceServers: [] });
		this.peerConnection = peerConnection;
		peerConnection.addTransceiver(this.kind, { direction: "recvonly" });
		peerConnection.ontrack = (event) => {
			this.media.srcObject = event.streams[0] || new MediaStream([event.track]);
			this.media.play().catch((error) => this.setStatus(`Playback failed: ${error}`, true));
			if (this.kind === "audio") {
				this.startLevelMeter(this.media.srcObject);
			}
		};
		peerConnection.onconnectionstatechange = () => {
			if (peerConnection.connectionState === "connected") {
				this.setStatus("Media connection established. Waiting for media...", false);
			} else if (["failed", "closed"].includes(peerConnection.connectionState)) {
				this.stop(`Media connection ${peerConnection.connectionState}.`, true);
			}
		};
		try {
			const [width, height] = this.resolutionMode === "capture"
				? this.resolution.split("x").map(Number)
				: [0, 0];
			const offer = await peerConnection.createOffer();
			await peerConnection.setLocalDescription(offer);
			await this.waitForIceGathering(peerConnection);
			const response = await fetch(this.endpoint, {
				method: "POST",
				credentials: "include",
				headers: { "content-type": "application/json" },
				body: JSON.stringify({
					fps: this.fps,
					height,
					source_id: this.sourceId,
					offer: peerConnection.localDescription,
					width,
				}),
			});
			if (!response.ok) {
				throw new Error(await this.responseError(response));
			}
			const session = await response.json();
			this.sessionId = String(session.session_id ?? "");
			await peerConnection.setRemoteDescription(session.answer);
		} catch (error) {
			this.stop(`Media connection failed: ${error}`, true);
		} finally {
			this.starting = false;
			this.setButtons();
		}
	}

	stop(message, error) {
		const sessionId = this.sessionId;
		this.sessionId = "";
		this.starting = false;
		if (this.peerConnection) {
			this.peerConnection.ontrack = null;
			this.peerConnection.onconnectionstatechange = null;
			this.peerConnection.close();
			this.peerConnection = null;
		}
		if (this.media) {
			this.media.pause();
			this.media.srcObject = null;
		}
		this.stopLevelMeter();
		this.stopVideoStats();
		if (sessionId) {
			fetch(`/api/media/sessions/${encodeURIComponent(sessionId)}`, {
				method: "DELETE",
				credentials: "include",
				keepalive: true,
			}).catch(() => {});
		}
		this.setButtons();
		if (message) {
			this.setStatus(message, error);
		}
	}

	startVideoStats() {
		this.stopVideoStats();
		if (this.kind !== "video" || !this.peerConnection) {
			return;
		}
		let previousFrames;
		let previousTimestamp;
		const update = async () => {
			if (!this.resolutionSelect && this.resolutionStat && this.media.videoWidth && this.media.videoHeight) {
				this.resolutionStat.textContent = `Resolution: ${this.media.videoWidth} × ${this.media.videoHeight}`;
			}
			const reports = await this.peerConnection?.getStats().catch(() => null);
			if (!reports) {
				return;
			}
			for (const report of reports.values()) {
				if (report.type !== "inbound-rtp" || report.kind !== "video") {
					continue;
				}
				if (previousFrames !== undefined && report.timestamp > previousTimestamp && this.fpsActual) {
					const fps = (report.framesDecoded - previousFrames) * 1000 / (report.timestamp - previousTimestamp);
					this.fpsActual.textContent = `actual ${fps.toFixed(1)}`;
				}
				previousFrames = report.framesDecoded;
				previousTimestamp = report.timestamp;
				break;
			}
		};
		update();
		this.videoStatsTimer = setInterval(update, 1000);
	}

	stopVideoStats() {
		if (this.videoStatsTimer) {
			clearInterval(this.videoStatsTimer);
		}
		this.videoStatsTimer = 0;
		if (!this.resolutionSelect && this.resolutionStat) {
			this.resolutionStat.textContent = "Resolution: --";
		}
		if (this.fpsActual) {
			this.fpsActual.textContent = "actual --";
		}
	}

	startLevelMeter(stream) {
		this.stopLevelMeter();
		if (!stream) {
			return;
		}
		const AudioContext = window.AudioContext || window.webkitAudioContext;
		if (!AudioContext) {
			return;
		}
		try {
			this.audioContext = new AudioContext();
			if (this.audioContext.state === "suspended") {
				this.audioContext.resume().catch(() => {});
			}
			const analyser = this.audioContext.createAnalyser();
			analyser.fftSize = 1024;
			this.analyser = analyser;
			this.audioSource = this.audioContext.createMediaStreamSource(stream);
			this.audioSource.connect(analyser);
			this.levelData = new Uint8Array(analyser.frequencyBinCount);
			const update = () => {
				if (!this.analyser) {
					return;
				}
				this.analyser.getByteTimeDomainData(this.levelData);
				let sum = 0;
				for (let i = 0; i < this.levelData.length; i++) {
					const sample = (this.levelData[i] - 128) / 128;
					sum += sample * sample;
				}
				const rms = Math.sqrt(sum / this.levelData.length);
				const level = Math.min(1, rms * 1.5);
				const percent = Math.round(level * 100);
				if (this.levelFill) {
					this.levelFill.style.width = `${percent}%`;
				}
				if (this.levelLabel) {
					this.levelLabel.textContent = `${percent}%`;
				}
				this.levelFrame = requestAnimationFrame(update);
			};
			this.levelFrame = requestAnimationFrame(update);
		} catch (_) {
			this.stopLevelMeter();
		}
	}

	stopLevelMeter() {
		if (this.levelFrame) {
			cancelAnimationFrame(this.levelFrame);
			this.levelFrame = 0;
		}
		if (this.audioSource) {
			try {
				this.audioSource.disconnect();
			} catch (_) {}
			this.audioSource = null;
		}
		this.analyser = null;
		if (this.audioContext) {
			this.audioContext.close().catch(() => {});
			this.audioContext = null;
		}
		if (this.levelFill) {
			this.levelFill.style.width = "0%";
		}
		if (this.levelLabel) {
			this.levelLabel.textContent = "0%";
		}
	}

	setButtons() {
		const available = Boolean(this.endpoint && this.sourceId);
		this.startButton.disabled = !available || this.starting || Boolean(this.peerConnection);
		this.stopButton.disabled = !this.starting && !this.peerConnection;
		if (this.fullscreenButton) {
			this.fullscreenButton.disabled = !this.peerConnection;
		}
	}

	setStatus(text, error) {
		this.status.textContent = text;
		this.status.classList.toggle("media-receiver-error", error);
	}
}
