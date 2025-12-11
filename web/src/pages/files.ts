import { ensureShell } from "../layout"
import { fetchLocalPeerId, fetchPeers, listPeerDir, listPeerDisks } from "../api"
import type { DiskInfo, DirEntry, Peer } from "../api"

type BrowserState = {
	peerId: string
	path: string
	showingDisks: boolean
	entries: DirEntry[]
	disks: DiskInfo[]
	loading: boolean
	error: string | null
}

const formatSize = (value: number) => {
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

const joinChildPath = (base: string, child: string) => {
	const trimmedChild = child.replace(/^[\\/]+/, "").replace(/[\\/]+$/, "")
	if (!trimmedChild) {
		return base || child
	}
	if (!base) {
		return trimmedChild
	}
	const endsWithSeparator = base.endsWith("/") || base.endsWith("\\")
	if (endsWithSeparator) {
		return `${base}${trimmedChild}`
	}
	const separator = base.includes("\\") && !base.includes("/") ? "\\" : "/"
	return `${base}${separator}${trimmedChild}`
}

const parentPath = (path: string): string | null => {
	const trimmed = path.trim()
	if (!trimmed || trimmed === "/") {
		return null
	}
	const driveRoot = trimmed.match(/^[A-Za-z]:\\?$/)
	if (driveRoot) {
		return ""
	}
	const withoutTrailing = trimmed.replace(/[\\/]+$/, "")
	if (!withoutTrailing) {
		return null
	}
	const lastSep = Math.max(
		withoutTrailing.lastIndexOf("/"),
		withoutTrailing.lastIndexOf("\\"),
	)
	if (lastSep === -1) {
		return ""
	}
	return withoutTrailing.slice(0, lastSep + 1)
}

export const renderFiles = async () => {
	const content = ensureShell("/files")
	content.innerHTML = `
	<section class="hero">
		<h1>Files</h1>
		<p class="lede">Browse shared directories and remote disks.</p>
	</section>
	<div class="card" id="files-card">
		<div class="card-heading">
			<h2>File browser</h2>
			<p id="files-status" class="muted">Loading peers...</p>
		</div>
		<div class="files-controls">
			<label for="files-peer-select">Peer</label>
			<select id="files-peer-select"></select>
			<button type="button" id="files-refresh">Refresh view</button>
		</div>
		<div class="files-toolbar">
			<button type="button" id="files-up">Up</button>
			<div class="files-path-label">
				<span class="muted">Path:</span>
				<strong id="files-path-value">Disks</strong>
			</div>
			<div class="files-path-entry">
				<input id="files-path-input" placeholder="/path/to/folder" />
				<button type="button" id="files-go">Browse</button>
			</div>
		</div>
		<div id="files-browser" class="files-browser"></div>
	</div>
`

	const statusEl = content.querySelector<HTMLElement>("#files-status")
	const peerSelect = content.querySelector<HTMLSelectElement>("#files-peer-select")
	const browserEl = content.querySelector<HTMLElement>("#files-browser")
	const upButton = content.querySelector<HTMLButtonElement>("#files-up")
	const refreshButton = content.querySelector<HTMLButtonElement>("#files-refresh")
	const goButton = content.querySelector<HTMLButtonElement>("#files-go")
	const pathInput = content.querySelector<HTMLInputElement>("#files-path-input")
	const pathValueEl = content.querySelector<HTMLElement>("#files-path-value")

	const state: BrowserState = {
		peerId: "",
		path: "",
		showingDisks: true,
		entries: [],
		disks: [],
		loading: false,
		error: null,
	}

	const updateControls = () => {
		const disabled = state.loading
		if (peerSelect) {
			peerSelect.disabled = disabled
		}
		if (refreshButton) {
			refreshButton.disabled = disabled || !state.peerId
		}
		if (upButton) {
			upButton.disabled =
				disabled || state.showingDisks || !state.path.trim().length
		}
		if (pathInput) {
			pathInput.disabled = disabled || !state.peerId
		}
		if (goButton) {
			goButton.disabled = disabled || !state.peerId
		}
	}

	const renderBrowser = () => {
		if (!browserEl) return
		if (state.loading) {
			browserEl.innerHTML = `<p class="muted">Loading ${
				state.showingDisks ? "disks" : "directory"
			}...</p>`
			return
		}
		if (state.error) {
			browserEl.innerHTML = `<p class="muted">Error: ${escapeHtml(
				state.error,
			)}</p>`
			return
		}

		if (state.showingDisks) {
			if (!state.disks.length) {
				browserEl.innerHTML = `<p class="muted">No disks were reported for this peer.</p>`
				return
			}
			const rows = state.disks
				.map((disk) => {
					const label = disk.name || disk.mount_path
					return `
					<div class="files-row">
						<div>
							<strong>${escapeHtml(label)}</strong>
							<p class="muted">${escapeHtml(disk.mount_path)}</p>
							<p>${formatSize(disk.available_space)} free of ${formatSize(
						disk.total_space,
					)}</p>
						</div>
						<button type="button" class="link-btn" data-disk-path="${escapeHtml(
							disk.mount_path,
						)}">Browse</button>
					</div>
				`
				})
				.join("")
			browserEl.innerHTML = `<div class="files-list">${rows}</div>`
			const diskButtons = browserEl.querySelectorAll<HTMLButtonElement>(
				"[data-disk-path]",
			)
			diskButtons.forEach((btn) => {
				btn.addEventListener("click", () => {
					const diskPath = btn.dataset.diskPath
					if (!diskPath) return
					state.showingDisks = false
					state.path = diskPath
					state.entries = []
					state.error = null
					void loadBrowser()
				})
			})
			return
		}

		if (!state.entries.length) {
			browserEl.innerHTML = `<p class="muted">Directory is empty.</p>`
			return
		}
		const rows = state.entries
			.map((entry) => {
				const label = entry.is_dir
					? `[DIR] ${escapeHtml(entry.name)}`
					: escapeHtml(entry.name)
				const meta = entry.is_dir
					? "Directory"
					: `${entry.mime ?? "File"} • ${formatSize(entry.size)}`
				return `
				<button
					type="button"
					class="files-entry"
					data-entry-name="${escapeHtml(entry.name)}"
					data-entry-dir="${entry.is_dir ? "1" : "0"}"
				>
					<div>
						<strong>${label}</strong>
						<p class="muted">${meta}</p>
					</div>
					<span class="badge small">${entry.is_dir ? "dir" : "file"}</span>
				</button>
			`
			})
			.join("")
		browserEl.innerHTML = `<div class="files-list">${rows}</div>`
		const entryButtons = browserEl.querySelectorAll<HTMLButtonElement>(
			"[data-entry-name]",
		)
		entryButtons.forEach((btn) => {
			btn.addEventListener("click", () => {
				const name = btn.dataset.entryName
				if (!name) return
				const isDir = btn.dataset.entryDir === "1"
				const target = joinChildPath(state.path, name)
				if (isDir) {
					state.showingDisks = false
					state.path = target
					state.entries = []
					state.error = null
					void loadBrowser()
					return
				}
				if (statusEl) {
					statusEl.textContent = `Selected ${target}`
				}
			})
		})
	}

	const updateBrowserView = () => {
		updateControls()
		if (pathValueEl) {
			pathValueEl.textContent = state.showingDisks
				? "Disks"
				: state.path || "/"
		}
		if (statusEl) {
			if (state.error) {
				statusEl.textContent = `Error: ${state.error}`
			} else if (state.loading) {
				statusEl.textContent = `Loading ${
					state.showingDisks ? "disks" : "directory"
				}...`
			} else if (state.showingDisks) {
				statusEl.textContent = "Select a disk to browse."
			} else {
				statusEl.textContent = `Browsing ${state.path || "/"}`
			}
		}
		renderBrowser()
	}

	const loadBrowser = async () => {
		if (!state.peerId) {
			state.error = "Select a peer first."
			updateBrowserView()
			return
		}
		state.loading = true
		state.error = null
		if (state.showingDisks) {
			state.disks = []
		} else {
			state.entries = []
		}
		updateBrowserView()

		try {
			if (state.showingDisks) {
				state.disks = await listPeerDisks(state.peerId)
			} else {
				const targetPath = state.path.trim().length ? state.path : "/"
				state.entries = await listPeerDir(state.peerId, targetPath)
			}
		} catch (err) {
			state.error = err instanceof Error ? err.message : String(err)
		} finally {
			state.loading = false
			updateBrowserView()
		}
	}

	const selectPeer = (peerId: string) => {
		if (!peerId || state.peerId === peerId) {
			return
		}
		state.peerId = peerId
		state.path = ""
		state.entries = []
		state.disks = []
		state.showingDisks = true
		state.error = null
		void loadBrowser()
	}

	const handleGo = () => {
		const target = pathInput?.value.trim() ?? ""
		if (!state.peerId) {
			return
		}
		state.showingDisks = false
		state.path = target || "/"
		state.entries = []
		state.error = null
		void loadBrowser()
	}

	const handleUp = () => {
		if (state.showingDisks || !state.path) {
			return
		}
		const next = parentPath(state.path)
		if (next === null) {
			return
		}
		if (next === "") {
			state.showingDisks = true
			state.path = ""
			state.entries = []
			state.error = null
			void loadBrowser()
			return
		}
		state.showingDisks = false
		state.path = next
		state.entries = []
		state.error = null
		void loadBrowser()
	}

	const formatPeerOption = (peerId: string, label: string) =>
		`<option value="${escapeHtml(peerId)}">${escapeHtml(label)}</option>`

	const describeError = (error: unknown) =>
		error instanceof Error ? error.message : String(error)

	const loadPeers = async () => {
		if (peerSelect) {
			peerSelect.disabled = true
			peerSelect.innerHTML = `<option value="">Loading peers…</option>`
		}
		let localPeerId: string | null = null
		try {
			localPeerId = await fetchLocalPeerId()
		} catch {
			localPeerId = null
		}

		let peerError: string | null = null
		let peers: Peer[] = []
		try {
			peers = await fetchPeers()
		} catch (err) {
			peerError = describeError(err)
			peers = []
		}

		if (!peerSelect) {
			return
		}

		const options: { id: string; label: string }[] = []
		if (localPeerId) {
			options.push({
				id: localPeerId,
				label: `Local node (you) (${localPeerId})`,
			})
		}
		for (const peer of peers) {
			if (peer.id === localPeerId) {
				continue
			}
			options.push({
				id: peer.id,
				label: `${peer.name ?? "Unnamed"} (${peer.id})`,
			})
		}

		if (!options.length) {
			const message = peerError ?? "No peers connected"
			peerSelect.innerHTML = `<option value="">${escapeHtml(message)}</option>`
			if (statusEl) statusEl.textContent = message
			peerSelect.disabled = false
			return
		}

		peerSelect.innerHTML = options
			.map(({ id, label }) => formatPeerOption(id, label))
			.join("")

		const firstPeer = options[0]!
		peerSelect.value = firstPeer.id
		selectPeer(firstPeer.id)

		if (statusEl) {
			if (peerError) {
				statusEl.textContent = `Loaded ${options.length} peer(s); remote peers failed: ${peerError}`
			} else {
				statusEl.textContent = `${options.length} peer(s)`
			}
		}

		if (peerSelect) {
			peerSelect.disabled = false
		}
	}

	peerSelect?.addEventListener("change", () => {
		const peerId = peerSelect.value
		selectPeer(peerId)
	})
	refreshButton?.addEventListener("click", () => {
		void loadBrowser()
	})
	goButton?.addEventListener("click", () => {
		handleGo()
	})
	upButton?.addEventListener("click", () => {
		handleUp()
	})
	pathInput?.addEventListener("keydown", (event) => {
		if (event.key === "Enter") {
			event.preventDefault()
			handleGo()
		}
	})

	updateBrowserView()
	void loadPeers()
}
