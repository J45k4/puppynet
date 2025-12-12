import { ensureShell } from "../layout"
import { fetchStorageUsage } from "../api"
import type { StorageUsageFile } from "../api"
import { createTreeView, type TreeNode } from "../treeview"

type StorageEntry = {
	path: string
	name: string
	size: number
	itemCount: number
	lastChanged: string | null
	percent: number
	children: StorageEntry[]
}

type StorageNode = {
	name: string
	id: string
	totalSize: number
	entries: StorageEntry[]
}

type StoragePageState = {
	nodes: StorageNode[]
	loading: boolean
	error: string | null
	customStatus: string | null
	viewMode: "tree" | "heatmap"
	heatmapFocus: Map<string, string | null>
}

type EntryStats = {
	size: number
	itemCount: number
	lastChanged: string | null
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

const formatTimestamp = (value?: string | null) => {
	if (!value) {
		return "-"
	}
	const parsed = new Date(value)
	if (Number.isNaN(parsed.getTime())) {
		return value
	}
	return parsed.toLocaleString()
}

const normalizePath = (value: string) =>
	value.replace(/\\/g, "/").replace(/^\/+/, "").replace(/\/+$/, "")

const latestTimestamp = (current: string | null, candidate: string | null) => {
	if (!current) {
		return candidate
	}
	if (!candidate) {
		return current
	}
	return current >= candidate ? current : candidate
}

const formatNodeId = (bytes: number[]) => {
	if (!bytes.length) {
		return "unknown"
	}
	return bytes
		.map((b) => b.toString(16).padStart(2, "0"))
		.join("")
}

const displayName = (path: string) => {
	if (!path) {
		return "Root"
	}
	const segments = path.split("/").filter((segment) => segment.length > 0)
	if (!segments.length) {
		return "Root"
	}
	return segments[segments.length - 1]
}

const buildStorageNodes = (files: StorageUsageFile[]): StorageNode[] => {
	if (!files.length) {
		return []
	}
	const grouped = new Map<
		string,
		{ name: string; id: number[]; records: StorageUsageFile[] }
	>()
	for (const file of files) {
		const key = file.node_id.join(",")
		const existing = grouped.get(key)
		if (existing) {
			existing.records.push(file)
		} else {
			grouped.set(key, {
				name: file.node_name || formatNodeId(file.node_id),
				id: file.node_id,
				records: [file],
			})
		}
	}
	const nodes: StorageNode[] = []
	grouped.forEach(({ name, id, records }) => {
		const { entries, totalSize } = buildStorageTree(records)
		nodes.push({
			name: name || formatNodeId(id),
			id: formatNodeId(id),
			totalSize,
			entries,
		})
	})
	nodes.sort((a, b) => a.name.localeCompare(b.name))
	return nodes
}

const buildStorageTree = (files: StorageUsageFile[]) => {
	const stats = new Map<string, EntryStats>()
	const children = new Map<string, Set<string>>()
	for (const file of files) {
		const normalized = normalizePath(file.path)
		const ancestors = [""]
		if (normalized.length) {
			let current = ""
			for (const segment of normalized.split("/")) {
				if (!segment) {
					continue
				}
				current = current ? `${current}/${segment}` : segment
				ancestors.push(current)
			}
		}
		for (const path of ancestors) {
			const existing = stats.get(path)
			const updated: EntryStats = {
				size: (existing?.size ?? 0) + file.size,
				itemCount: (existing?.itemCount ?? 0) + 1,
				lastChanged: latestTimestamp(existing?.lastChanged ?? null, file.last_changed ?? null),
			}
			stats.set(path, updated)
		}
		for (let i = 0; i < ancestors.length - 1; i += 1) {
			const parent = ancestors[i]
			const child = ancestors[i + 1]
			if (parent === undefined || child === undefined) {
				continue
			}
			const set = children.get(parent) ?? new Set<string>()
			set.add(child)
			children.set(parent, set)
		}
	}
	const rootStats = stats.get("")
	const totalSize = rootStats?.size ?? 0
	const entries = buildStorageEntriesFor("", stats, children, totalSize)
	return { entries, totalSize }
}

const buildStorageEntriesFor = (
	parent: string,
	stats: Map<string, EntryStats>,
	children: Map<string, Set<string>>,
	totalSize: number,
): StorageEntry[] => {
	const childPaths = children.get(parent)
	if (!childPaths) {
		return []
	}
	const sorted = Array.from(childPaths).sort((a, b) => {
		const aSize = stats.get(a)?.size ?? 0
		const bSize = stats.get(b)?.size ?? 0
		return bSize - aSize
	})
	return sorted
		.map((childPath) => {
			const data = stats.get(childPath)
			if (!data) {
				return null
			}
			const percent = totalSize === 0 ? 0 : (data.size / totalSize) * 100
			return {
				path: childPath,
				name: displayName(childPath),
				size: data.size,
				itemCount: data.itemCount,
				lastChanged: data.lastChanged,
				percent,
				children: buildStorageEntriesFor(childPath, stats, children, data.size),
			}
		})
		.filter((entry): entry is StorageEntry => entry !== null)
}

export const renderStorage = async () => {
	const content = ensureShell("/storage")
	content.innerHTML = `
	<section class="hero">
		<h1>Storage</h1>
		<p class="lede">Storage usage summary for shared folders and nodes.</p>
	</section>
	<div class="card" id="storage-card">
		<div class="card-heading">
			<h2>Storage usage overview</h2>
			<p id="storage-status" class="muted">Loading storage usage...</p>
		</div>
		<div class="storage-actions">
			<div class="storage-view-toggle" role="tablist" aria-label="Storage view">
				<button type="button" id="storage-view-tree" role="tab" aria-selected="true">Tree</button>
				<button type="button" id="storage-view-heatmap" role="tab" aria-selected="false">Heatmap</button>
			</div>
			<button type="button" id="storage-refresh">Refresh</button>
		</div>
		<div class="storage-table">
			<div class="storage-row storage-row-header">
				<div class="storage-cell storage-name">Name</div>
				<div class="storage-cell">% of node</div>
				<div class="storage-cell">Size</div>
				<div class="storage-cell">Items</div>
				<div class="storage-cell">Last changed</div>
				<div class="storage-cell">Action</div>
			</div>
		</div>
		<div id="storage-list" class="storage-list"></div>
	</div>
`

	const statusEl = content.querySelector<HTMLElement>("#storage-status")
	const listEl = content.querySelector<HTMLElement>("#storage-list")
	const refreshButton = content.querySelector<HTMLButtonElement>("#storage-refresh")
	const viewTreeButton = content.querySelector<HTMLButtonElement>("#storage-view-tree")
	const viewHeatmapButton =
		content.querySelector<HTMLButtonElement>("#storage-view-heatmap")

	const state: StoragePageState = {
		nodes: [],
		loading: true,
		error: null,
		customStatus: null,
		viewMode: "tree",
		heatmapFocus: new Map(),
	}

	const heatColor = (percent: number) => {
		const clamped = Math.max(0, Math.min(100, percent))
		const hue = 210 - clamped * 2.1
		return `hsl(${hue.toFixed(0)}deg 70% 40%)`
	}

	const renderHeatmap = () => {
		if (!listEl) return
		if (!state.nodes.length) {
			listEl.innerHTML = `<p class="muted">No storage data available.</p>`
			return
		}

		const findEntry = (
			entries: StorageEntry[],
			path: string,
		): StorageEntry | null => {
			for (const entry of entries) {
				if (entry.path === path) return entry
				if (entry.children.length) {
					const found = findEntry(entry.children, path)
					if (found) return found
				}
			}
			return null
		}

		const nodeSections = state.nodes
			.map((node) => {
				const focusPath = state.heatmapFocus.get(node.id) ?? null
				const focusEntry = focusPath
					? findEntry(node.entries, focusPath)
					: null
				const focusEntries = focusEntry ? focusEntry.children : node.entries
				const tiles = focusEntries.slice(0, 120).map((entry) => {
					const bg = heatColor(entry.percent)
					const percentLabel = `${entry.percent.toFixed(1)}%`
					return `
						<button
							type="button"
							class="storage-heatmap-tile"
							style="--heat-bg: ${bg}"
							data-entry-open="${escapeHtml(entry.path)}"
							data-entry-has-children="${entry.children.length ? "1" : "0"}"
							data-node-id="${escapeHtml(node.id)}"
							title="${escapeHtml(entry.path)} • ${formatSize(entry.size)}"
						>
							<strong class="storage-heatmap-name">${escapeHtml(entry.name)}</strong>
							<span class="storage-heatmap-meta">${percentLabel} • ${formatSize(
						entry.size,
					)}</span>
							<span class="storage-heatmap-count">${entry.itemCount} item(s)</span>
						</button>
					`
				})
				const backButton =
					focusEntry || focusPath
						? `<button type="button" class="link-btn storage-heatmap-back" data-heatmap-back="${escapeHtml(
								node.id,
						  )}">Back</button>`
						: ""
				return `
					<section class="storage-heatmap-node">
						<header class="storage-heatmap-header">
							<h3>${escapeHtml(node.name)}</h3>
							<p class="muted">${escapeHtml(node.id)} • ${formatSize(
					node.totalSize,
				)}</p>
							${backButton}
						</header>
						<div class="storage-heatmap-grid">
							${tiles.join("")}
						</div>
					</section>
				`
			})
			.join("")
		listEl.innerHTML = `<div class="storage-heatmap">${nodeSections}</div>`
		const tileButtons = listEl.querySelectorAll<HTMLButtonElement>("[data-entry-open]")
		tileButtons.forEach((btn) => {
			btn.addEventListener("click", () => {
				const path = btn.getAttribute("data-entry-open")
				const nodeId = btn.getAttribute("data-node-id") ?? ""
				const hasChildren = btn.getAttribute("data-entry-has-children") === "1"
				if (!path) return
				if (hasChildren) {
					state.heatmapFocus.set(nodeId, path)
					updateStorageView()
					return
				}
				state.customStatus = `Selected ${path}`
				updateStatus()
			})
		})
		const backButtons = listEl.querySelectorAll<HTMLButtonElement>(
			"[data-heatmap-back]",
		)
		backButtons.forEach((btn) => {
			btn.addEventListener("click", () => {
				const nodeId = btn.getAttribute("data-heatmap-back") ?? ""
				state.heatmapFocus.set(nodeId, null)
				updateStorageView()
			})
		})
	}

	const buildEntryNodes = (
		nodeId: string,
		entries: StorageEntry[],
	): TreeNode<StorageEntry>[] =>
		entries.map((entry) => ({
			id: `${nodeId}:${entry.path}`,
			label: entry.name,
			sublabel: entry.path,
			data: entry,
			children: buildEntryNodes(nodeId, entry.children),
		}))

	const renderRow = (
		node: TreeNode<StorageEntry | StorageNode>,
		depth: number,
		expanded: boolean,
		hasChildren: boolean,
	) => {
		const row = document.createElement("div")
		row.className = "storage-row storage-tree-row"
		row.setAttribute("data-tree-id", node.id)
		row.style.setProperty("--tree-depth", String(depth))
		const data = node.data as StorageEntry | StorageNode
		const isTopNode = (data as StorageNode).entries !== undefined
		if (isTopNode) {
			const storageNode = data as StorageNode
			row.innerHTML = `
				<div class="storage-cell storage-name storage-tree-name">
					${
						hasChildren
							? `<button type="button" class="link-btn" data-tree-toggle="${node.id}">${
									expanded ? "▾" : "▸"
							  }</button>`
							: `<span class="storage-toggle-placeholder"></span>`
					}
					<div class="storage-name-content">
						<strong>${escapeHtml(storageNode.name)}</strong>
						<p class="muted storage-node-id">${escapeHtml(storageNode.id)}</p>
					</div>
				</div>
				<div class="storage-cell">100%</div>
				<div class="storage-cell">${formatSize(storageNode.totalSize)}</div>
				<div class="storage-cell">-</div>
				<div class="storage-cell muted">-</div>
				<div class="storage-cell"></div>
			`
			return row
		}
		const entry = data as StorageEntry
		const openButton = hasChildren
			? ""
			: `<button type="button" class="link-btn" data-entry-open="${escapeHtml(
					entry.path,
			  )}">Open</button>`
		row.innerHTML = `
			<div class="storage-cell storage-name storage-tree-name">
				${
					hasChildren
						? `<button type="button" class="link-btn" data-tree-toggle="${node.id}">${
								expanded ? "▾" : "▸"
						  }</button>`
						: `<span class="storage-toggle-placeholder"></span>`
				}
				<div class="storage-name-content">
					<strong>${escapeHtml(entry.name)}</strong>
					<p class="muted">${escapeHtml(entry.path)}</p>
				</div>
			</div>
			<div class="storage-cell">${entry.percent.toFixed(1)}%</div>
			<div class="storage-cell">${formatSize(entry.size)}</div>
			<div class="storage-cell">${entry.itemCount}</div>
			<div class="storage-cell">${formatTimestamp(entry.lastChanged)}</div>
			<div class="storage-cell">${openButton}</div>
		`
		return row
	}

	const tree = createTreeView<StorageEntry | StorageNode>({
		nodes: [],
		className: "storage-tree",
		renderRow,
		onSelect: (node) => {
			const data = node.data as StorageEntry | StorageNode
			if (node.children?.length) {
				tree.toggle(node.id)
				return
			}
			if ((data as StorageEntry).path !== undefined) {
				const entry = data as StorageEntry
				state.customStatus = `Selected ${entry.path}`
				updateStatus()
			}
		},
	})
	if (listEl) {
		listEl.appendChild(tree.element)
	}

	const updateStatus = () => {
		if (!statusEl) return
		if (state.customStatus) {
			statusEl.textContent = state.customStatus
			return
		}
		if (state.loading) {
			statusEl.textContent = "Loading storage usage..."
		} else if (state.error) {
			statusEl.textContent = `Failed to load storage usage: ${state.error}`
		} else if (!state.nodes.length) {
			statusEl.textContent = "No storage data available."
		} else {
			statusEl.textContent = `Showing ${state.nodes.length} node(s)`
		}
	}

	const updateStorageView = () => {
		if (!listEl) return
		if (state.loading) {
			listEl.innerHTML = `<p class="muted">Loading storage usage...</p>`
			listEl.appendChild(tree.element)
			tree.setNodes([])
		} else if (state.error) {
			const errorMessage = escapeHtml(state.error ?? "Unknown error")
			listEl.innerHTML = `<p class="muted">Error: ${errorMessage}</p>`
			listEl.appendChild(tree.element)
			tree.setNodes([])
		} else if (!state.nodes.length) {
			listEl.innerHTML = `<p class="muted">No storage data available.</p>`
			listEl.appendChild(tree.element)
			tree.setNodes([])
		} else {
			if (state.viewMode === "heatmap") {
				renderHeatmap()
			} else {
				const nodes: TreeNode<StorageEntry | StorageNode>[] = state.nodes.map(
					(storageNode) => {
						const nodeId = `node:${storageNode.id}`
						return {
							id: nodeId,
							label: storageNode.name,
							sublabel: storageNode.id,
							data: storageNode,
							children: buildEntryNodes(nodeId, storageNode.entries),
						}
					},
				)
				listEl.innerHTML = ""
				listEl.appendChild(tree.element)
				tree.setNodes(nodes)
			}
		}
		updateStatus()
		if (state.viewMode === "tree") {
			const entryOpenButtons = listEl.querySelectorAll<HTMLButtonElement>(
				"[data-entry-open]",
			)
			entryOpenButtons.forEach((btn) => {
				btn.addEventListener("click", () => {
					const path = btn.getAttribute("data-entry-open")
					if (!path) return
					state.customStatus = `Selected ${path}`
					updateStatus()
				})
			})
		}
	}

	const loadStorage = async () => {
		state.loading = true
		state.error = null
		state.customStatus = null
		state.nodes = []
		state.heatmapFocus.clear()
		updateStorageView()

		try {
			const files = await fetchStorageUsage()
			state.nodes = buildStorageNodes(files)
		} catch (error) {
			state.error = error instanceof Error ? error.message : String(error)
		} finally {
			state.loading = false
			updateStorageView()
		}
	}

	refreshButton?.addEventListener("click", () => {
		void loadStorage()
	})

	const setViewMode = (mode: StoragePageState["viewMode"]) => {
		state.viewMode = mode
		if (viewTreeButton) {
			viewTreeButton.setAttribute("aria-selected", String(mode === "tree"))
			viewTreeButton.classList.toggle("active", mode === "tree")
		}
		if (viewHeatmapButton) {
			viewHeatmapButton.setAttribute("aria-selected", String(mode === "heatmap"))
			viewHeatmapButton.classList.toggle("active", mode === "heatmap")
		}
		updateStorageView()
	}

	viewTreeButton?.addEventListener("click", () => setViewMode("tree"))
	viewHeatmapButton?.addEventListener("click", () => setViewMode("heatmap"))

	updateStorageView()
	void loadStorage()
}
