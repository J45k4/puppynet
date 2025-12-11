import { ensureShell } from "../layout"
import { fetchReleases } from "../api"

const formatDate = (value: string) => {
	const date = new Date(value)
	if (Number.isNaN(date.getTime())) {
		return value
	}
	return date.toLocaleString()
}

const escapeHtml = (value: string) =>
	value
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;")

const snippet = (value: string | null, limit = 280) => {
	if (!value) {
		return ""
	}
	const trimmed = value.trim()
	if (!trimmed) {
		return ""
	}
	const shortened = trimmed.split("\n").map((line) => line.trim()).join(" ")
	return shortened.length <= limit ? shortened : `${shortened.slice(0, limit)}…`
}

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

const renderAssets = (assets: { name: string; browser_download_url: string; size: number }[]) => {
	if (!assets.length) {
		return `<p class="muted">No downloadable assets.</p>`
	}
	return `
		<ul class="updates-assets">
			${assets
				.map(
					(asset) => `
						<li>
							<a href="${asset.browser_download_url}" target="_blank" rel="noreferrer">
								${asset.name}
							</a>
							<span>${formatBytes(asset.size)}</span>
						</li>
					`,
				)
				.join("")}
		</ul>
	`
}

const renderReleaseNotes = (body: string | null) => {
	if (!body || !body.trim().length) {
		return `<p class="muted">No release notes provided.</p>`
	}
	return `
		<div class="release-notes">
			<pre>${escapeHtml(body)}</pre>
		</div>
	`
}

const renderRelease = (release: Awaited<ReturnType<typeof fetchReleases>>[number]) => {
	const title = release.name || release.tag_name
	const badge = release.prerelease ? '<span class="badge">Pre-release</span>' : ""
	const summary = snippet(release.body)
	return `
		<div class="updates-card">
			<div class="updates-card__header">
				<div>
					<h3>${title}</h3>
					<p class="muted">${release.tag_name} • ${formatDate(release.published_at)}</p>
				</div>
				<div class="updates-card__badge">
					${badge}
					<a href="${release.html_url}" target="_blank" rel="noreferrer" class="link-btn">View on GitHub</a>
				</div>
			</div>
			${summary ? `<p>${summary}</p>` : ""}
			${renderReleaseNotes(release.body)}
			${renderAssets(release.assets)}
		</div>
	`
}

export const renderUpdates = async () => {
	const content = ensureShell("/updates")
	content.innerHTML = `
	<section class="hero">
		<h1>Updates</h1>
		<p class="lede">Latest published versions of PuppyNet from GitHub releases.</p>
	</section>
	<div class="card" id="updates-card">
		<div class="card-heading">
			<h2>Release feed</h2>
			<p id="updates-status" class="muted">Fetching releases…</p>
			<button type="button" id="updates-refresh">Refresh</button>
		</div>
		<div id="updates-list" class="updates-list"></div>
	</div>
`
	const statusEl = document.getElementById("updates-status")
	const listEl = document.getElementById("updates-list")
	const refreshButton = document.getElementById("updates-refresh")

	const loadReleases = async () => {
		if (statusEl) statusEl.textContent = "Loading latest releases…"
		if (listEl) listEl.innerHTML = ""
		try {
			const releases = await fetchReleases(5)
			if (!listEl) return
			if (!releases.length) {
				listEl.innerHTML = `<p class="muted">No releases were found.</p>`
				if (statusEl) statusEl.textContent = "No releases available."
				return
			}
			listEl.innerHTML = releases.map((release) => renderRelease(release)).join("")
			if (statusEl) statusEl.textContent = `Showing ${releases.length} release(s)`
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error)
			if (statusEl) statusEl.textContent = `Failed to load releases: ${message}`
			if (listEl) listEl.innerHTML = `<p class="muted">Unable to reach GitHub releases.</p>`
		}
	}

	refreshButton?.addEventListener("click", () => {
		void loadReleases()
	})

	void loadReleases()
}
