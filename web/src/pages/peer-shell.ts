import { ensureShell } from "../layout"
import { startPeerShell, sendPeerShellInput } from "../api"
import { PuppyTerm } from "../PuppyTerm"

export const renderPeerShell = async (peerId: string) => {
	const content = ensureShell("/peers")
	content.innerHTML = `
	<section class="hero">
		<h1>Remote shell</h1>
		<p class="lede">Interactive session for peer ${peerId}</p>
	</section>
	<div class="card">
		<canvas id="peer-shell-canvas" class="peer-shell-canvas"></canvas>
		<p id="peer-shell-status" class="muted" style="margin-top: 8px;">Connecting…</p>
	</div>
`
	const canvas = content.querySelector<HTMLCanvasElement>("#peer-shell-canvas")
	const statusEl = content.querySelector<HTMLElement>("#peer-shell-status")
	if (!canvas) {
		throw new Error("shell canvas missing")
	}

	const fitCanvas = () => {
		const rect = canvas.getBoundingClientRect()
		const dpr = window.devicePixelRatio || 1
		canvas.width = Math.max(300, Math.floor(rect.width * dpr))
		canvas.height = Math.max(240, Math.floor(rect.height * dpr))
	}

	fitCanvas()
	const term = new PuppyTerm()
	term.open(canvas)
	term.write("Connecting to peer…\r\n")

	let sessionId: number | null = null
	try {
		sessionId = await startPeerShell(peerId)
		if (statusEl) statusEl.textContent = `Shell started (session ${sessionId})`
		term.write(`Connected.\r\n`)
	} catch (err) {
		const msg = err instanceof Error ? err.message : String(err)
		if (statusEl) statusEl.textContent = `Failed to start shell: ${msg}`
		term.write(`Failed to start shell: ${msg}\r\n`)
		return
	}

	const encoder = new TextEncoder()
	const decoder = new TextDecoder("utf-8", { fatal: false })

	let inFlight = false
	const send = async (data: string) => {
		if (sessionId === null || inFlight) return
		inFlight = true
		try {
			const bytes = Array.from(encoder.encode(data))
			const outBytes = await sendPeerShellInput(peerId, sessionId, bytes)
			if (outBytes.length) {
				term.write(decoder.decode(new Uint8Array(outBytes)))
			}
		} catch (error) {
			const msg = error instanceof Error ? error.message : String(error)
			if (statusEl) statusEl.textContent = `Shell error: ${msg}`
			term.write(`\r\nShell error: ${msg}\r\n`)
		} finally {
			inFlight = false
		}
	}

	term.onData = (data) => {
		// Local echo to make non-PTY sessions usable.
		term.write(data)
		void send(data)
	}

	window.addEventListener("resize", () => {
		fitCanvas()
		term.resizeToCanvas()
	})
}

