export type Peer = {
	id: string
	name?: string | null
}

export type SearchArgs = {
	name_query?: string
	mime_types?: string[]
	page?: number
	page_size?: number
}

export type SearchResult = {
	hash: number[] | string
	name: string
	path: string
	size: number
	mime_type?: string | null
	replicas: number
	first_datetime?: string | null
	latest_datetime?: string | null
}

export type DirEntry = {
	name: string
	is_dir: boolean
	extension?: string | null
	mime?: string | null
	size: number
	created_at?: string | null
	modified_at?: string | null
	accessed_at?: string | null
}

export type DiskInfo = {
	name: string
	mount_path: string
	filesystem: string
	total_space: number
	available_space: number
	usage_percent: number
	total_read_bytes: number
	total_written_bytes: number
	read_only: boolean
	removable: boolean
	kind: string
}

export type CpuInfo = {
	name: string
	usage: number
	frequency_hz: number
}

export type InterfaceInfo = {
	name: string
	mac: string
	ips: string[]
	total_received: number
	total_transmitted: number
	packets_received: number
	packets_transmitted: number
	errors_on_received: number
	errors_on_transmitted: number
}

export type StorageUsageFile = {
	node_id: number[]
	node_name: string
	path: string
	size: number
	last_changed?: string | null
}

export type GithubReleaseAsset = {
	name: string
	browser_download_url: string
	size: number
	content_type: string
}

export type GithubRelease = {
	name: string
	tag_name: string
	html_url: string
	published_at: string
	body: string | null
	draft: boolean
	prerelease: boolean
	assets: GithubReleaseAsset[]
}

export type StateResponse = {
	me: string
	peers: Peer[]
	discovered: { peer_id: string; multiaddr: string }[]
	users: { name: string }[]
	shared_folders: { path: string; flags: number }[]
}

const envAddr = process.env.PUBLIC_SERVER_ADDR
const serverAddr = envAddr && envAddr.trim().length > 0
	? envAddr
	: (typeof window !== "undefined" ? window.location.origin : "/")

const apiBase = serverAddr.endsWith("/") ? serverAddr.slice(0, -1) : serverAddr

let peersCache: Peer[] | null = null
let bearerToken: string | null = null
let stateCache: StateResponse | null = null

export const getServerAddr = () => serverAddr

export const setBearerToken = (token: string | null) => {
	bearerToken = token
}

const authHeaders = (): HeadersInit | undefined => {
	if (!bearerToken) return undefined
	return { Authorization: `Bearer ${bearerToken}` }
}

export const apiGet = async <T>(path: string): Promise<T> => {
	const headers = authHeaders()
	const res = await fetch(`${apiBase}${path}`, {
		credentials: "include",
		headers,
	})
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		throw new Error(`Request failed: ${res.status}`)
	}
	return res.json() as Promise<T>
}

export const fetchPeers = async (): Promise<Peer[]> => {
	if (!peersCache) {
		const data = await apiGet<{ peers: Peer[] }>("/api/peers")
		peersCache = data.peers
	}
	return peersCache ?? []
}

export const findPeer = async (peerId: string): Promise<Peer | undefined> => {
	const peers = await fetchPeers()
	const existing = peers.find((p) => p.id === peerId)
	if (existing) {
		return existing
	}
	try {
		const localPeerId = await fetchLocalPeerId()
		if (localPeerId === peerId) {
			return { id: localPeerId, name: "Local node (you)" }
		}
	} catch {
		// ignore errors fetching local peer
	}
	return undefined
}

export const clearPeerCache = () => {
	peersCache = null
	stateCache = null
}

export const fetchMimeTypes = async (): Promise<string[]> => {
	const data = await apiGet<{ mime_types: string[] }>("/api/mime-types")
	return data.mime_types
}

export const fetchUsers = async (): Promise<string[]> => {
	const data = await apiGet<{ users: string[] }>("/users")
	return data.users ?? []
}

export const searchFiles = async (
	args: SearchArgs,
): Promise<{ results: SearchResult[]; total: number; mime_types: string[] }> => {
	const params = new URLSearchParams()
	if (args.name_query) params.set("name_query", args.name_query)
	if (args.mime_types && args.mime_types.length > 0) {
		params.set("mime_types", args.mime_types.join(","))
	}
	if (args.page !== undefined) params.set("page", String(args.page))
	if (args.page_size !== undefined) params.set("page_size", String(args.page_size))
	const headers = authHeaders()
	const res = await fetch(`${apiBase}/api/search?${params.toString()}`, {
		credentials: "include",
		headers,
	})
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		throw new Error(`Search failed: ${res.status}`)
	}
	const data = await res.json()
	return {
		results: data.results ?? [],
		total: data.total ?? 0,
		mime_types: data.mime_types ?? [],
	}
}

export const listPeerDir = async (
	peerId: string,
	path: string,
): Promise<DirEntry[]> => {
	const params = new URLSearchParams()
	params.set("path", path.trim().length > 0 ? path : "/")
	const data = await apiGet<{ entries: DirEntry[] }>(
		`/api/peers/${encodeURIComponent(peerId)}/dir?${params.toString()}`,
	)
	return data.entries ?? []
}

export const listPeerDisks = async (peerId: string): Promise<DiskInfo[]> => {
	const data = await apiGet<{ disks: DiskInfo[] }>(
		`/api/peers/${encodeURIComponent(peerId)}/disks`,
	)
	return data.disks ?? []
}

export const fetchStorageUsage = async (): Promise<StorageUsageFile[]> => {
	const data = await apiGet<{ files: StorageUsageFile[] }>("/api/storage")
	return data.files ?? []
}

export const fetchPeerCpus = async (peerId: string): Promise<CpuInfo[]> => {
	const data = await apiGet<{ cpus: CpuInfo[] }>(
		`/api/peers/${encodeURIComponent(peerId)}/cpus`,
	)
	return data.cpus ?? []
}

export const fetchPeerInterfaces = async (
	peerId: string,
): Promise<InterfaceInfo[]> => {
	const data = await apiGet<{ interfaces: InterfaceInfo[] }>(
		`/api/peers/${encodeURIComponent(peerId)}/interfaces`,
	)
	return data.interfaces ?? []
}

export const fetchState = async (): Promise<StateResponse> => {
	if (!stateCache) {
		stateCache = await apiGet<StateResponse>("/api/state")
	}
	return stateCache
}

export const fetchLocalPeerId = async (): Promise<string> => {
	const state = await fetchState()
	return state.me
}

export const fetchReleases = async (limit = 5): Promise<GithubRelease[]> => {
	const res = await fetch(
		`https://api.github.com/repos/j45k4/puppynet/releases?per_page=${limit}`,
	)
	if (!res.ok) {
		throw new Error(`GitHub releases request failed: ${res.status}`)
	}
	return (await res.json()) as Promise<GithubRelease[]>
}

export const login = async (
	username: string,
	password: string,
	setCookie: boolean,
): Promise<{ access_token: string }> => {
	const res = await fetch(`${apiBase}/auth/login`, {
		method: "POST",
		credentials: "include",
		headers: {
			"content-type": "application/json",
		},
		body: JSON.stringify({
			username,
			password,
			set_cookie: setCookie,
		}),
	})
	if (res.status === 401) {
		const message = await extractErrorMessage(res)
		throw new Error(message ?? "invalid credentials")
	}
	if (!res.ok) {
		const message = await extractErrorMessage(res)
		throw new Error(message ?? `Login failed: ${res.status}`)
	}
	return res.json()
}

async function extractErrorMessage(res: Response): Promise<string | null> {
	try {
		const data = await res.json()
		if (data && typeof data.error === "string" && data.error.length > 0) {
			return data.error
		}
	} catch {
		// ignore
	}
	return res.statusText
}

export const fetchMe = async (): Promise<string | null> => {
	const headers = authHeaders()
	const res = await fetch(`${apiBase}/auth/me`, {
		credentials: "include",
		headers,
	})
	if (res.status === 401) {
		return null
	}
	if (!res.ok) {
		throw new Error(`Failed to load session: ${res.status}`)
	}
	const data = await res.json()
	return data.user ?? null
}

export const logout = async (): Promise<void> => {
	await fetch(`${apiBase}/auth/logout`, {
		method: "POST",
		credentials: "include",
	})
	setBearerToken(null)
	peersCache = null
}
