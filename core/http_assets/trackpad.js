const css = `
.trackpad {
	display: grid;
	grid-template-rows: minmax(180px, 1fr) auto;
	gap: 8px;
	min-height: 260px;
	font: 14px system-ui, sans-serif;
	color: #172033;
}
.trackpad-surface {
	position: relative;
	display: grid;
	place-items: center;
	min-height: 180px;
	border: 1px solid #9aa8ba;
	background: linear-gradient(135deg, #eef3f8, #dfe8ef);
	touch-action: none;
	user-select: none;
	cursor: grab;
}
.trackpad-surface:active {
	cursor: grabbing;
}
.trackpad-crosshair {
	width: 52px;
	height: 52px;
	border: 1px solid #6f7f91;
	border-radius: 50%;
	opacity: .5;
}
.trackpad-buttons {
	display: grid;
	grid-template-columns: 1fr 1fr 1fr;
	gap: 8px;
}
.trackpad-button {
	min-height: 38px;
	border: 1px solid #9aa8ba;
	background: #ffffff;
	color: #172033;
	font: inherit;
}
.trackpad-button:active {
	background: #dfe8ef;
}
`;

export default class Trackpad {
	constructor(element, ctx) {
		this.element = element;
		this.ctx = ctx;
		this.pointerId = null;
		this.lastX = 0;
		this.lastY = 0;
		this.startX = 0;
		this.startY = 0;
		this.dragging = false;
		this.lastTapTime = 0;
		this.lastTapX = 0;
		this.lastTapY = 0;
		this.longPressTimer = null;
		this.longPressReady = false;
		this.dragSelecting = false;
		this.suppressTap = false;
		this.pendingDx = 0;
		this.pendingDy = 0;
		this.frame = 0;
	}

	mount(props) {
		this.element.innerHTML = "";
		this.style = document.createElement("style");
		this.style.textContent = css;
		this.root = document.createElement("div");
		this.root.className = "trackpad";
		this.surface = document.createElement("div");
		this.surface.className = "trackpad-surface";
		this.surface.tabIndex = 0;
		this.surface.title = "Trackpad";
		const crosshair = document.createElement("div");
		crosshair.className = "trackpad-crosshair";
		this.surface.append(crosshair);
		this.buttons = document.createElement("div");
		this.buttons.className = "trackpad-buttons";
		this.buttons.append(
			this.button("Left", "left"),
			this.button("Middle", "middle"),
			this.button("Right", "right"),
		);
		this.root.append(this.surface, this.buttons);
		this.element.append(this.style, this.root);
		this.setProps(props);
		this.onPointerDown = (event) => this.pointerDown(event);
		this.onPointerMove = (event) => this.pointerMove(event);
		this.onPointerUp = (event) => this.pointerUp(event);
		this.onPointerCancel = (event) => this.pointerCancel(event);
		this.onDoubleClick = (event) => this.doubleClick(event);
		this.onWheel = (event) => this.wheel(event);
		this.onContextMenu = (event) => event.preventDefault();
		this.surface.addEventListener("pointerdown", this.onPointerDown);
		this.surface.addEventListener("pointermove", this.onPointerMove);
		this.surface.addEventListener("pointerup", this.onPointerUp);
		this.surface.addEventListener("pointercancel", this.onPointerCancel);
		this.surface.addEventListener("dblclick", this.onDoubleClick);
		this.surface.addEventListener("wheel", this.onWheel, { passive: false });
		this.surface.addEventListener("contextmenu", this.onContextMenu);
	}

	setProps(props) {
		this.props = props ?? {};
		this.sensitivity = Number(this.props.sensitivity ?? 1);
		this.tapMoveThreshold = Number(this.props.tapMoveThreshold ?? 18);
		this.doubleTapDelay = Number(this.props.doubleTapDelay ?? 700);
		this.longPressDelay = Number(this.props.longPressDelay ?? 600);
	}

	dispose() {
		this.clearLongPress();
		this.releaseDragSelection();
		if (this.frame) {
			cancelAnimationFrame(this.frame);
		}
	}

	button(label, button) {
		const element = document.createElement("button");
		element.type = "button";
		element.className = "trackpad-button";
		element.textContent = label;
		element.addEventListener("click", () => {
			this.emitMouseClick(button);
		});
		return element;
	}

	pointerDown(event) {
		this.pointerId = event.pointerId;
		this.lastX = event.clientX;
		this.lastY = event.clientY;
		this.startX = event.clientX;
		this.startY = event.clientY;
		this.dragging = false;
		this.longPressReady = false;
		this.dragSelecting = false;
		this.suppressTap = false;
		if (this.isDoubleTap(event)) {
			this.suppressTap = true;
			this.lastTapTime = 0;
			this.emitLeftClick();
		} else {
			this.scheduleLongPress(event.pointerId);
		}
		this.surface.setPointerCapture(event.pointerId);
		this.surface.focus();
		event.preventDefault();
	}

	pointerMove(event) {
		if (this.pointerId !== event.pointerId) {
			return;
		}
		if (!this.dragging) {
			const distance = Math.hypot(event.clientX - this.startX, event.clientY - this.startY);
			if (distance <= this.tapMoveThreshold) {
				event.preventDefault();
				return;
			}
			this.clearLongPress();
			this.dragging = true;
			if (this.longPressReady) {
				this.longPressReady = false;
				this.dragSelecting = true;
				this.emitMousePress("left");
				this.lastX = this.startX;
				this.lastY = this.startY;
			} else {
				this.lastX = event.clientX;
				this.lastY = event.clientY;
				event.preventDefault();
				return;
			}
		}
		const dx = (event.clientX - this.lastX) * this.sensitivity;
		const dy = (event.clientY - this.lastY) * this.sensitivity;
		this.lastX = event.clientX;
		this.lastY = event.clientY;
		this.pendingDx += dx;
		this.pendingDy += dy;
		this.scheduleMove();
		event.preventDefault();
	}

	pointerUp(event) {
		if (this.pointerId !== event.pointerId) {
			return;
		}
		this.pointerId = null;
		this.clearLongPress();
		if (this.dragSelecting) {
			this.flushMove();
			this.releaseDragSelection();
			event.preventDefault();
			return;
		}
		if (this.longPressReady && !this.dragging) {
			this.longPressReady = false;
			this.lastTapTime = 0;
			this.emitRightClick();
			event.preventDefault();
			return;
		}
		this.longPressReady = false;
		if (this.suppressTap) {
			this.suppressTap = false;
			event.preventDefault();
			return;
		}
		this.handleTap(event);
		this.flushMove();
		event.preventDefault();
	}

	pointerCancel(event) {
		if (this.pointerId !== event.pointerId) {
			return;
		}
		this.pointerId = null;
		this.longPressReady = false;
		this.suppressTap = false;
		this.clearLongPress();
		this.flushMove();
		this.releaseDragSelection();
		event.preventDefault();
	}

	scheduleLongPress(pointerId) {
		this.clearLongPress();
		this.longPressTimer = setTimeout(() => {
			if (this.pointerId !== pointerId || this.dragging) {
				return;
			}
			this.longPressReady = true;
			this.lastTapTime = 0;
		}, this.longPressDelay);
	}

	clearLongPress() {
		if (this.longPressTimer) {
			clearTimeout(this.longPressTimer);
			this.longPressTimer = null;
		}
	}

	isDoubleTap(event) {
		return this.lastTapTime > 0 && event.timeStamp - this.lastTapTime <= this.doubleTapDelay;
	}

	handleTap(event) {
		if (this.longPressReady) {
			return;
		}
		const distance = Math.hypot(event.clientX - this.startX, event.clientY - this.startY);
		if (this.dragging || distance > this.tapMoveThreshold) {
			this.lastTapTime = 0;
			return;
		}
		const now = event.timeStamp;
		const tapDistance = Math.hypot(event.clientX - this.lastTapX, event.clientY - this.lastTapY);
		if (now - this.lastTapTime <= this.doubleTapDelay && tapDistance <= this.tapMoveThreshold) {
			this.lastTapTime = 0;
			this.emitLeftClick();
			return;
		}
		this.lastTapTime = now;
		this.lastTapX = event.clientX;
		this.lastTapY = event.clientY;
	}

	doubleClick(event) {
		event.preventDefault();
		this.lastTapTime = 0;
		this.suppressTap = true;
		this.clearLongPress();
		this.emitLeftClick();
	}

	emitLeftClick() {
		this.emitMouseClick("left");
	}

	emitRightClick() {
		this.emitMouseClick("right");
	}

	emitMouseClick(button) {
		this.ctx.emit("mouseClicked", { button });
	}

	emitMousePress(button) {
		this.ctx.emit("mousePressed", { button });
	}

	emitMouseRelease(button) {
		this.ctx.emit("mouseReleased", { button });
	}

	releaseDragSelection() {
		if (!this.dragSelecting) {
			return;
		}
		this.dragSelecting = false;
		this.emitMouseRelease("left");
	}

	wheel(event) {
		event.preventDefault();
		const amount = Math.max(-6, Math.min(6, Math.round(event.deltaY / 60)));
		if (amount !== 0) {
			this.ctx.emit("scrolled", { amount });
		}
	}

	scheduleMove() {
		if (!this.frame) {
			this.frame = requestAnimationFrame(() => this.flushMove());
		}
	}

	flushMove() {
		if (this.frame) {
			cancelAnimationFrame(this.frame);
			this.frame = 0;
		}
		const dx = Math.round(this.pendingDx);
		const dy = Math.round(this.pendingDy);
		this.pendingDx = 0;
		this.pendingDy = 0;
		if (dx !== 0 || dy !== 0) {
			this.ctx.emit("mouseMoved", { dx, dy });
		}
	}
}
