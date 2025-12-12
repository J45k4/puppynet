import { ensureShell } from "../layout"
import { fetchMimeTypes, searchFiles } from "../api"
import { createMultiSelect } from "../multiselect"
import { navigate } from "../router"

const escapeHtml = (value: string) =>
	value
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;")

const defaultPageSize = 25

export const renderSearch = async () => {
	const content = ensureShell("/search")
	content.innerHTML = `
		<section class="hero">
			<h1>Search</h1>
			<p class="lede">Search across indexed files.</p>
		</section>
		<div class="card">
			<h2>Filters</h2>
			<form id="search-form">
				<input id="search-name" name="name_query" placeholder="Name contains..." />
				<div id="search-mime"></div>
				<button type="submit">Search</button>
			</form>
			<p id="search-status" class="muted">Enter a query to search.</p>
		</div>
		<div class="card" id="search-results">
			<h2>Results</h2>
			<div id="search-table"></div>
		</div>
	`

	const statusEl = document.getElementById("search-status")
	const tableEl = document.getElementById("search-table")
	const nameInput = document.getElementById("search-name") as HTMLInputElement | null
	const mimeMount = document.getElementById("search-mime")
	const mimeSelect = createMultiSelect({
		id: "search-mime-select",
		placeholder: "Mime types",
	})
	if (mimeMount?.parentElement) {
		mimeMount.parentElement.replaceChild(mimeSelect.element, mimeMount)
	}

	let currentPage = 0
	let totalResults = 0
	let loading = false
	let hasMore = false
	let observer: IntersectionObserver | null = null

	const loadMimeTypes = async () => {
		try {
			const mimes = await fetchMimeTypes()
			mimeSelect.setOptions(
				mimes.map((m) => ({
					value: m,
					label: m,
				})),
			)
		} catch (err) {
			if (statusEl) statusEl.textContent = `Failed to load mime types: ${err}`
		}
	}

	const resetTable = () => {
		if (!tableEl) return
		tableEl.innerHTML = `
			<div class="table-wrapper">
				<table class="table">
					<thead>
						<tr>
							<th>Name</th>
							<th>Type</th>
							<th>Size</th>
							<th>Replicas</th>
							<th>Updated</th>
							<th>Hash</th>
							<th></th>
						</tr>
					</thead>
					<tbody id="search-body"></tbody>
				</table>
			</div>
			<div id="search-sentinel"></div>
		`
	}

	const formatHashValue = (value: number[] | string | undefined) => {
		if (!value) return ""
		if (typeof value === "string") {
			return value
		}
		if (!Array.isArray(value)) {
			return ""
		}
		return value
			.map((byte) => byte.toString(16).padStart(2, "0"))
			.join("")
	}

	const shortHash = (value: string) => {
		if (!value) return ""
		return value.length > 16 ? `${value.slice(0, 8)}…${value.slice(-8)}` : value
	}

	const appendRows = (rows: any[]) => {
		const body = document.getElementById("search-body")
		if (!body) return
		const html = rows
			.map(
				(r) => {
					const hash = formatHashValue(r.hash)
					const nodeId = formatHashValue(r.node_id)
					const path = r.path ?? ""
					return `
						<tr>
							<td>${escapeHtml(r.name ?? "")}</td>
							<td class="muted">${escapeHtml(r.mime_type ?? "unknown")}</td>
							<td>${((r.size ?? 0) / 1024).toFixed(1)} KB</td>
							<td><span class="badge small">${r.replicas} replicas</span></td>
							<td class="muted">${escapeHtml(r.latest_datetime ?? "")}</td>
							<td class="muted hash-cell">${escapeHtml(shortHash(hash))}</td>
							<td>
								<button
									type="button"
									class="link-btn"
									data-hash-link="${escapeHtml(hash)}"
									data-node-id="${escapeHtml(nodeId)}"
									data-path="${escapeHtml(path)}"
								>
									View
								</button>
							</td>
						</tr>
					`
				},
			)
			.join("")
		body.insertAdjacentHTML("beforeend", html)
	}

	tableEl?.addEventListener("click", (event) => {
		const target = event.target as HTMLElement | null
		const button = target?.closest<HTMLButtonElement>("[data-hash-link]")
		if (!button) return
		const hash = button.getAttribute("data-hash-link")
		if (!hash) return
		const nodeId = button.getAttribute("data-node-id")
		const path = button.getAttribute("data-path")
		const params = new URLSearchParams()
		if (nodeId) params.set("node", nodeId)
		if (path) params.set("path", path)
		const suffix = params.toString() ? `?${params.toString()}` : ""
		navigate(`/file/${encodeURIComponent(hash)}${suffix}`)
	})

	const loadPage = async () => {
		if (loading) return
		loading = true
		if (statusEl) statusEl.textContent = "Searching..."
		try {
			const name_query = nameInput?.value.trim() ?? ""
			const mime_types = mimeSelect.getSelected()
			const data = await searchFiles({
				name_query: name_query || undefined,
				mime_types: mime_types.length ? mime_types : undefined,
				page: currentPage,
				page_size: defaultPageSize,
			})
			totalResults = data.total ?? 0
			if (!tableEl) return
			if (currentPage === 0) {
				resetTable()
			}
			if (!data.results.length && currentPage === 0) {
				tableEl.innerHTML = `<p class="muted">No results.</p>`
				hasMore = false
				return
			}
			appendRows(data.results as any[])
			currentPage += 1
			const loadedCount = Math.min(currentPage * defaultPageSize, totalResults)
			if (statusEl) statusEl.textContent = `Loaded ${loadedCount} of ${totalResults} result(s)`
			hasMore = loadedCount < totalResults
			const sentinel = document.getElementById("search-sentinel")
			if (sentinel) {
				if (!observer) {
					observer = new IntersectionObserver((entries) => {
						if (entries.some((e) => e.isIntersecting) && hasMore) {
							void loadPage()
						}
					})
				}
				if (hasMore) observer.observe(sentinel)
				else observer.unobserve(sentinel)
			}
		} catch (err) {
			if (statusEl) statusEl.textContent = `Search failed: ${err}`
		} finally {
			loading = false
		}
	}

	const form = document.getElementById("search-form")
	form?.addEventListener("submit", (ev) => {
		ev.preventDefault()
		currentPage = 0
		totalResults = 0
		hasMore = false
		if (observer) {
			const sentinel = document.getElementById("search-sentinel")
			if (sentinel) observer.unobserve(sentinel)
		}
		resetTable()
		void loadPage()
	})

	void loadMimeTypes()
}
