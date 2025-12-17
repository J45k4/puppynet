export type Peer = {
	id: string
	name?: string | null
	node_id?: string | null
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

export type UpdateProgress =
	| { type: "FetchingRelease" }
	| { type: "Downloading"; filename: string }
	| { type: "Unpacking" }
	| { type: "Verifying" }
	| { type: "Installing" }
	| { type: "Completed"; version: string }
	| { type: "Failed"; error: string }
	| { type: "AlreadyUpToDate"; current_version: number }

const parseUpdateProgress = (raw: Record<string, unknown>): UpdateProgress | null => {
	const entries = Object.entries(raw)
	if (entries.length !== 1) {
		return null
	}
	const [first] = entries
	if (!first) {
		return null
	}
	const [key, value] = first
	switch (key) {
		case "FetchingRelease":
			return { type: "FetchingRelease" }
		case "Downloading": {
			if (value && typeof value === "object" && "filename" in value && typeof (value as any).filename === "string") {
				return { type: "Downloading", filename: (value as any).filename }
			}
			return null
		}
		case "Unpacking":
			return { type: "Unpacking" }
		case "Verifying":
			return { type: "Verifying" }
		case "Installing":
			return { type: "Installing" }
		case "Completed": {
			if (value && typeof value === "object" && "version" in value && typeof (value as any).version === "string") {
				return { type: "Completed", version: (value as any).version }
			}
			return null
		}
		case "Failed": {
			if (value && typeof value === "object" && "error" in value && typeof (value as any).error === "string") {
				return { type: "Failed", error: (value as any).error }
			}
			return null
		}
		case "AlreadyUpToDate": {
			if (
				value &&
				typeof value === "object" &&
				"current_version" in value &&
				typeof (value as any).current_version === "number"
			) {
				return { type: "AlreadyUpToDate", current_version: (value as any).current_version }
			}
			return null
		}
		default:
			return null
	}
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

const authHeaders = (): Record<string, string> | undefined => {
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

export type FileContentResponse = {
	data: Uint8Array
	mime: string
	length: number
	status: number
}

export const fetchFileByHash = async (hash: string): Promise<FileContentResponse> => {
	const headers = authHeaders()
	const res = await fetch(`${apiBase}/api/file/hash?hash=${encodeURIComponent(hash)}`, {
		method: "GET",
		credentials: "include",
		headers,
	})
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		let errorMessage = `Request failed: ${res.status}`
		try {
			const payload = await res.json()
			if (payload && typeof payload.error === "string") {
				errorMessage = payload.error
			}
		} catch {
			// ignore parse failures
		}
		throw new Error(errorMessage)
	}
	const arrayBuffer = await res.arrayBuffer()
	const mime = res.headers.get("content-type") ?? "application/octet-stream"
	const lengthHeader = res.headers.get("content-length")
	return {
		data: new Uint8Array(arrayBuffer),
		mime,
		length: lengthHeader ? Number(lengthHeader) : arrayBuffer.byteLength,
		status: res.status,
	}
}

export type FileChunk = {
	offset: number
	data: number[]
	eof: boolean
}

export const startPeerShell = async (peerId: string): Promise<number> => {
	const headers = authHeaders()
	const res = await fetch(
		`${apiBase}/api/peers/${encodeURIComponent(peerId)}/shell/start`,
		{
			method: "POST",
			credentials: "include",
			headers,
		},
	)
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		throw new Error(`Failed to start shell: ${res.status}`)
	}
	const data = (await res.json()) as { id: number }
	return data.id
}

export const sendPeerShellInput = async (
	peerId: string,
	id: number,
	data: number[],
): Promise<number[]> => {
	const auth = authHeaders()
	const headers: HeadersInit = {
		"content-type": "application/json",
		...(auth ?? {}),
	}
	const res = await fetch(
		`${apiBase}/api/peers/${encodeURIComponent(peerId)}/shell/input`,
		{
			method: "POST",
			credentials: "include",
			headers,
			body: JSON.stringify({ id, data }),
		},
	)
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		throw new Error(`Shell input failed: ${res.status}`)
	}
	const payload = (await res.json()) as { data: number[] }
	return payload.data
}

export const fetchPeerFileChunk = async (
	peerId: string,
	path: string,
	length?: number,
): Promise<FileChunk> => {
	const params = new URLSearchParams()
	params.set("path", path)
	if (length !== undefined) {
		params.set("length", String(length))
	}
	return apiGet<FileChunk>(
		`/api/peers/${encodeURIComponent(peerId)}/file?${params.toString()}`,
	)
}

export const startPeerUpdate = async (
	peerId: string,
	version?: string,
): Promise<number> => {
	const auth = authHeaders()
	const headers: HeadersInit = {
		"content-type": "application/json",
		...(auth ?? {}),
	}
	const res = await fetch(`${apiBase}/api/updates/${encodeURIComponent(peerId)}`, {
		method: "POST",
		credentials: "include",
		headers,
		body: JSON.stringify({ version }),
	})
	if (res.status === 401) {
		throw new Error("not authenticated")
	}
	if (!res.ok) {
		throw new Error(`Failed to start update: ${res.status}`)
	}
	const data = await res.json()
	return data.update_id
}

export const pollPeerUpdate = async (
	updateId: number,
): Promise<UpdateProgress[]> => {
	const data = await apiGet<{ events: Record<string, unknown>[] }>(
		`/api/updates/${updateId}/events`,
	)
	return data.events
		.map((event) => parseUpdateProgress(event))
		.filter((event): event is UpdateProgress => event !== null)
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
