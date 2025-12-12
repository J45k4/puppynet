import { ensureShell } from "../layout"
import { fetchFileByHash, fetchPeerFileChunk, fetchState, getServerAddr } from "../api"
import type { FileContentResponse } from "../api"

const formatBytes = (value: number) => {
	if (value < 1024) {
		return `${value} B`
	}
	const units = ["KB", "MB", "GB", "TB"]
	let size = value / 1024
	let index = 0
	while (size >= 1024 && index < units.length - 1) {
		index += 1
		size /= 1024
	}
	return `${size.toFixed(1)} ${units[index]}`
}

const escapeHtml = (value: string) =>
	value
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;")

const isTextMime = (mime: string) =>
	/^(text\/|application\/(json|xml|javascript|svg|x-www-form-urlencoded))/i.test(mime)

const isImageMime = (mime: string) => /^image\//i.test(mime)

const previewLimit = 160_000

const guessMimeFromPath = (value: string) => {
	const normalized = value.trim().toLowerCase()
	if (normalized.endsWith(".txt")) return "text/plain"
	if (normalized.endsWith(".json")) return "application/json"
	if (normalized.endsWith(".md")) return "text/markdown"
	if (normalized.endsWith(".csv")) return "text/csv"
	if (normalized.endsWith(".html") || normalized.endsWith(".htm")) return "text/html"
	if (normalized.endsWith(".xml")) return "application/xml"
	if (normalized.endsWith(".js")) return "application/javascript"
	if (normalized.endsWith(".png")) return "image/png"
	if (normalized.endsWith(".jpg") || normalized.endsWith(".jpeg")) return "image/jpeg"
	if (normalized.endsWith(".gif")) return "image/gif"
	if (normalized.endsWith(".webp")) return "image/webp"
	if (normalized.endsWith(".bmp")) return "image/bmp"
	if (normalized.endsWith(".ico")) return "image/x-icon"
	return "application/octet-stream"
}

export const renderFileByHash = async (hash: string) => {
	const content = ensureShell(`/file/${hash}`)
	content.innerHTML = `
	<section class="hero">
		<h1>File</h1>
		<p class="lede">Viewing file contents for hash ${escapeHtml(hash)}</p>
	</section>
	<div class="card" id="file-card">
		<div class="card-heading">
			<h2>File preview</h2>
			<p id="file-status" class="muted">Loading file data…</p>
		</div>
		<div class="file-meta">
			<p><span class="muted">Hash:</span> ${escapeHtml(hash)}</p>
			<p><span class="muted">Download:</span> <a id="file-download" target="_blank" rel="noreferrer">Raw download</a></p>
			<p><span class="muted">MIME:</span> <span id="file-mime">-</span></p>
			<p><span class="muted">Size:</span> <span id="file-size">-</span></p>
		</div>
		<div class="file-preview">
			<h3>Preview</h3>
			<pre id="file-content" class="resource-meta">Awaiting content…</pre>
			<p id="file-preview-note" class="muted"></p>
		</div>
		<div class="file-actions">
			<button type="button" id="file-refresh">Reload preview</button>
		</div>
	</div>
`
	const statusEl = document.getElementById("file-status")
	const previewEl = document.getElementById("file-content")
	const noteEl = document.getElementById("file-preview-note")
	const downloadLink = document.getElementById("file-download") as HTMLAnchorElement | null
	const mimeEl = document.getElementById("file-mime")
	const sizeEl = document.getElementById("file-size")
	const refreshButton = document.getElementById("file-refresh")

	const params = new URLSearchParams(window.location.search)
	const remoteNodeId = params.get("node")
	const remotePath = params.get("path")

	const downloadUrl = `${getServerAddr()}/api/file/hash?hash=${encodeURIComponent(hash)}`
	const updateDownloadLink = (url: string | null) => {
		if (!downloadLink) return
		if (url) {
			downloadLink.href = url
		} else {
			downloadLink.removeAttribute("href")
		}
	}
	updateDownloadLink(downloadUrl)

	const decodeText = (data: Uint8Array) => {
		const decoder = new TextDecoder("utf-8", { fatal: false })
		return decoder.decode(data)
	}

	let currentObjectUrl: string | null = null

	const renderPreview = (result: Awaited<ReturnType<typeof fetchFileByHash>>) => {
		if (!previewEl || !noteEl) {
			return
		}
		if (currentObjectUrl) {
			URL.revokeObjectURL(currentObjectUrl)
			currentObjectUrl = null
		}
		previewEl.classList.remove("file-content--image")
		if (isImageMime(result.mime)) {
			const blob = new Blob([result.data.buffer as ArrayBuffer], { type: result.mime })
			currentObjectUrl = URL.createObjectURL(blob)
			const img = document.createElement("img")
			img.src = currentObjectUrl
			img.alt = "Image preview"
			img.loading = "lazy"
			img.decoding = "async"
			previewEl.classList.add("file-content--image")
			previewEl.textContent = ""
			previewEl.appendChild(img)
			noteEl.textContent = ""
		} else if (isTextMime(result.mime)) {
			const truncated = result.data.length > previewLimit
			const chunk = result.data.slice(0, previewLimit)
			previewEl.textContent = decodeText(chunk)
			noteEl.textContent = truncated ? "Preview truncated to avoid flooding the UI." : ""
		} else {
				const truncated = result.data.length > previewLimit
				const chunk = result.data.slice(0, previewLimit)
				const snippet = chunk.slice(0, 128)
				const hex = Array.from(snippet)
					.map((byte) => byte.toString(16).padStart(2, "0"))
					.join(" ")
			previewEl.textContent = hex
			noteEl.textContent = result.data.length
				? `Binary data (${result.mime}); showing first ${snippet.length} byte(s).`
				: "No data available."
		}
	}

	const loadFile = async () => {
		if (statusEl) statusEl.textContent = "Loading file data…"
		if (previewEl) previewEl.textContent = ""
		if (noteEl) noteEl.textContent = ""
		try {
			const result = await fetchFileByHash(hash)
			if (mimeEl) mimeEl.textContent = result.mime
			if (sizeEl) sizeEl.textContent = formatBytes(result.length)
			if (statusEl) {
				statusEl.textContent = `Loaded ${formatBytes(result.length)} (${result.mime})`
			}
			renderPreview(result)
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error)
			if (remoteNodeId && remotePath) {
				const handled = await loadRemoteFallback()
				if (handled) {
					return
				}
			}
			if (statusEl) statusEl.textContent = `Failed to load file: ${message}`
			if (previewEl) previewEl.textContent = ""
			if (noteEl) noteEl.textContent = ""
		}
	}

	const loadRemoteFallback = async () => {
		if (!remoteNodeId || !remotePath) {
			return false
		}
		try {
			const state = await fetchState()
			const peer = state.peers.find((entry) => entry.node_id === remoteNodeId)
			if (!peer) {
				return false
			}
			const chunk = await fetchPeerFileChunk(peer.id, remotePath, previewLimit)
			const data = new Uint8Array(chunk.data)
			const remoteResult: FileContentResponse = {
				data,
				mime: guessMimeFromPath(remotePath),
				length: data.length,
				status: chunk.eof ? 200 : 206,
			}
			if (mimeEl) mimeEl.textContent = remoteResult.mime
			if (sizeEl) sizeEl.textContent = formatBytes(remoteResult.length)
			renderPreview(remoteResult)
			if (statusEl) {
				statusEl.textContent = `Loaded ${formatBytes(
					remoteResult.length,
				)} (${peer.name ?? peer.id})`
			}
			const remoteUrl = `${getServerAddr()}/api/peers/${encodeURIComponent(
				peer.id,
			)}/file?path=${encodeURIComponent(remotePath)}`
			updateDownloadLink(remoteUrl)
			if (noteEl) {
				noteEl.textContent = `Remote path: ${escapeHtml(remotePath)}`
			}
			return true
		} catch {
			return false
		}
	}

	refreshButton?.addEventListener("click", () => {
		void loadFile()
	})

	void loadFile()
}
