const css = `
.monitor-stream {
	display: grid;
	gap: 6px;
	width: 100%;
	max-width: 1280px;
	font: 13px system-ui, sans-serif;
	color: #172033;
}
.monitor-stream-frame {
	display: grid;
	place-items: center;
	min-height: 260px;
	border: 1px solid #9aa8ba;
	background: #111827;
	overflow: hidden;
}
.monitor-stream-frame img {
	display: block;
	width: 100%;
	height: auto;
	max-height: 720px;
	object-fit: contain;
}
.monitor-stream-status {
	min-height: 18px;
	color: #475569;
}
.monitor-stream-error {
	color: #b91c1c;
}
`;

export default class MonitorStream {
	constructor(element) {
		this.element = element;
		this.src = "";
	}

	mount(props) {
		this.element.innerHTML = "";
		this.style = document.createElement("style");
		this.style.textContent = css;
		this.root = document.createElement("div");
		this.root.className = "monitor-stream";
		this.frame = document.createElement("div");
		this.frame.className = "monitor-stream-frame";
		this.image = document.createElement("img");
		this.image.alt = "Monitor stream";
		this.image.loading = "eager";
		this.status = document.createElement("div");
		this.status.className = "monitor-stream-status";
		this.frame.append(this.image);
		this.root.append(this.frame, this.status);
		this.element.append(this.style, this.root);
		this.image.addEventListener("load", () => this.setStatus("Monitor stream connected.", false));
		this.image.addEventListener("error", () => {
			if (this.src) {
				this.setStatus("Monitor stream failed to load. Check authentication and daemon logs.", true);
			}
		});
		this.setProps(props);
	}

	setProps(props) {
		const nextSrc = String(props?.src ?? "");
		if (nextSrc === this.src) {
			return;
		}
		this.src = nextSrc;
		if (!this.src) {
			this.closeStream();
			this.setStatus("Monitor stream is not available on this peer.", false);
			return;
		}
		this.setStatus("Connecting to monitor stream...", false);
		this.image.src = this.src;
	}

	dispose() {
		this.closeStream();
	}

	closeStream() {
		if (this.image) {
			this.image.src = "about:blank";
			this.image.removeAttribute("src");
		}
	}

	setStatus(text, error) {
		this.status.textContent = text;
		this.status.classList.toggle("monitor-stream-error", error);
	}
}
