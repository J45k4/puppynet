import { navigate } from "../router"
import { ensureShell } from "../layout"
import {
	fetchPeers,
	findPeer,
	fetchPeerCpus,
	fetchPeerInterfaces,
	fetchLocalPeerId,
} from "../api"
import type { CpuInfo, InterfaceInfo, Peer } from "../api"

const escapeHtml = (value: string) =>
	value
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;")

const formatFrequency = (hz: number) => {
	if (hz >= 1_000_000_000) {
		return `${(hz / 1_000_000_000).toFixed(2)} GHz`
	}
	if (hz >= 1_000_000) {
		return `${(hz / 1_000_000).toFixed(2)} MHz`
	}
	return `${hz} Hz`
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

const joinIps = (ips: string[]) => {
	if (!ips.length) {
		return "No IPs"
	}
	return ips.join(", ")
}

const renderCpuRows = (cpus: CpuInfo[]): string =>
	cpus
		.map(
			(cpu) => `
				<div class="resource-row">
					<div class="resource-name">
						<strong>${escapeHtml(cpu.name)}</strong>
					</div>
					<div class="resource-meta">
						<span>${cpu.usage.toFixed(1)}% usage</span>
						<span>${formatFrequency(cpu.frequency_hz)}</span>
					</div>
				</div>
			`,
		)
		.join("")

const renderInterfaceRows = (interfaces: InterfaceInfo[]): string =>
	interfaces
		.map(
			(iface) => `
				<div class="resource-row interface-row">
					<div class="resource-name">
						<strong>${escapeHtml(iface.name)}</strong>
						<p class="muted">
							${escapeHtml(iface.mac)}
							<span>${escapeHtml(joinIps(iface.ips))}</span>
						</p>
					</div>
					<div class="resource-meta">
						<span>Rx ${formatBytes(iface.total_received)}</span>
						<span>Tx ${formatBytes(iface.total_transmitted)}</span>
						<span>Pkts ${iface.packets_received}/${iface.packets_transmitted}</span>
						<span>Errors ${iface.errors_on_received}/${iface.errors_on_transmitted}</span>
					</div>
				</div>
			`,
		)
		.join("")

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
		let localPeerId: string | null = null
		try {
			localPeerId = await fetchLocalPeerId()
		} catch {
			localPeerId = null
		}
		let peers: Peer[] = []
		let peerError: string | null = null
		try {
			peers = await fetchPeers()
		} catch (error) {
			peerError =
				error instanceof Error ? error.message : String(error)
			peers = []
		}

		const combined: Peer[] = []
		if (localPeerId) {
			combined.push({
				id: localPeerId,
				name: "Local node (you)",
			})
		}
		for (const peer of peers) {
			if (localPeerId && peer.id === localPeerId) {
				continue
			}
			combined.push(peer)
		}

		if (!tableEl) return
		if (!combined.length) {
			const message = peerError ?? "No peers connected."
			tableEl.innerHTML = `<p class="muted">${message}</p>`
			if (statusEl) statusEl.textContent = message
			return
		}

		if (statusEl) {
			const baseMessage = `Showing ${combined.length} peer(s)`
			statusEl.textContent = peerError
				? `${baseMessage}; remote peers failed: ${peerError}`
				: baseMessage
		}

		const rows = combined
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
		<div class="card" id="peer-cpu-card">
			<h2>CPUs</h2>
			<p id="peer-cpu-status" class="muted">CPU metrics will appear here.</p>
			<div id="peer-cpu-list" class="resource-list"></div>
		</div>
		<div class="card" id="peer-interfaces-card">
			<h2>Interfaces</h2>
			<p id="peer-interfaces-status" class="muted">Interface metrics will appear here.</p>
			<div id="peer-interfaces-list" class="resource-list"></div>
		</div>
	`
	const backBtn = document.getElementById("back-to-peers")
	if (backBtn) {
		backBtn.addEventListener("click", () => navigate("/peers"))
	}
	const cpuStatusEl = document.getElementById("peer-cpu-status")
	const cpuListEl = document.getElementById("peer-cpu-list")
	const interfacesStatusEl = document.getElementById("peer-interfaces-status")
	const interfacesListEl = document.getElementById("peer-interfaces-list")

	const loadCpus = async () => {
		if (!cpuStatusEl || !cpuListEl) return
		cpuStatusEl.textContent = "Loading CPU metrics..."
		cpuListEl.innerHTML = ""
		try {
			const cpus = await fetchPeerCpus(peerId)
			if (!cpus.length) {
				cpuListEl.innerHTML = `<p class="muted">No CPU data reported.</p>`
				cpuStatusEl.textContent = "No CPU data available."
				return
			}
			cpuListEl.innerHTML = renderCpuRows(cpus)
			cpuStatusEl.textContent = `Loaded ${cpus.length} CPU core(s)`
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error)
			cpuStatusEl.textContent = `Failed to load CPU info: ${message}`
			cpuListEl.innerHTML = `<p class="muted">Failed to load CPU data.</p>`
		}
	}

	const loadInterfaces = async () => {
		if (!interfacesStatusEl || !interfacesListEl) return
		interfacesStatusEl.textContent = "Loading interface metrics..."
		interfacesListEl.innerHTML = ""
		try {
			const interfaces = await fetchPeerInterfaces(peerId)
			if (!interfaces.length) {
				interfacesListEl.innerHTML = `<p class="muted">No interfaces reported.</p>`
				interfacesStatusEl.textContent = "No interface data available."
				return
			}
			interfacesListEl.innerHTML = renderInterfaceRows(interfaces)
			interfacesStatusEl.textContent = `Loaded ${interfaces.length} interface(s)`
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error)
			interfacesStatusEl.textContent = `Failed to load interfaces: ${message}`
			interfacesListEl.innerHTML = `<p class="muted">Failed to load interface data.</p>`
		}
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
		void loadCpus()
		void loadInterfaces()
	} catch (err) {
		const statusEl = document.getElementById("peer-status")
		if (statusEl) statusEl.textContent = `Failed to load peer: ${err}`
	}
}
