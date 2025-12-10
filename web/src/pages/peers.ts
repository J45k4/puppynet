import { navigate } from "../router"
import { ensureShell } from "../layout"
import { fetchPeers, findPeer } from "../api"

export const renderPeers = async () => {
	const content = ensureShell("/peers")
	content.innerHTML = `
		<section class="hero">
			<h1>Peers</h1>
			<p class="lede">Connected peers discovered by PuppyNet.</p>
		</section>
		<div class="card" id="peers-card">
			<h2>Peer list</h2>
			<p id="peers-status" class="muted">Loading peers...</p>
			<div id="peers-table"></div>
		</div>
	`
	const statusEl = document.getElementById("peers-status")
	const tableEl = document.getElementById("peers-table")
	try {
		const peers = await fetchPeers()
		if (statusEl) statusEl.textContent = `${peers.length} peer(s)`
		if (!tableEl) return
		if (peers.length === 0) {
			tableEl.innerHTML = `<p class="muted">No peers connected.</p>`
			return
		}
		const rows = peers
			.map(
				(peer) => `
			<tr data-peer-id="${peer.id}">
				<td><div class="pill"><strong>${peer.name ?? "Unnamed"}</strong><span class="muted">${peer.id}</span></div></td>
				<td><button class="link-btn" data-peer-id="${peer.id}">Open</button></td>
			</tr>
		`,
			)
			.join("")
		tableEl.innerHTML = `
			<table class="table">
				<thead>
					<tr><th>Peer</th><th></th></tr>
				</thead>
				<tbody>${rows}</tbody>
			</table>
		`
		const buttons = tableEl.querySelectorAll<HTMLButtonElement>("[data-peer-id]")
		buttons.forEach((btn) => {
			btn.addEventListener("click", () => {
				const id = btn.getAttribute("data-peer-id")
				if (id) navigate(`/peers/${id}`)
			})
		})
	} catch (err) {
		if (statusEl) statusEl.textContent = `Failed to load peers: ${err}`
	}
}

export const renderPeerDetail = async (peerId: string) => {
	const content = ensureShell("/peers")
	content.innerHTML = `
		<section class="hero">
			<h1>Peer</h1>
			<p class="lede">Details for peer ${peerId}</p>
		</section>
		<div class="card" id="peer-card">
			<h2>Summary</h2>
			<p id="peer-status" class="muted">Loading peer info...</p>
			<div id="peer-details"></div>
			<button class="link-btn" id="back-to-peers">Back to peers</button>
		</div>
	`
	const backBtn = document.getElementById("back-to-peers")
	if (backBtn) {
		backBtn.addEventListener("click", () => navigate("/peers"))
	}
	try {
		const peer = await findPeer(peerId)
		if (!peer) {
			const statusEl = document.getElementById("peer-status")
			if (statusEl) statusEl.textContent = "Peer not found."
			return
		}
		const detailsEl = document.getElementById("peer-details")
		if (!detailsEl) return
		const statusEl = document.getElementById("peer-status")
		if (statusEl) statusEl.textContent = "Connected peer loaded."
		detailsEl.innerHTML = `
			<p><span class="muted">Name:</span> ${peer.name ?? "Unnamed"}</p>
			<p><span class="muted">Peer ID:</span> ${peer.id}</p>
		`
	} catch (err) {
		const statusEl = document.getElementById("peer-status")
		if (statusEl) statusEl.textContent = `Failed to load peer: ${err}`
	}
}
