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

const envAddr = process.env.PUBLIC_SERVER_ADDR
const serverAddr = envAddr && envAddr.trim().length > 0
	? envAddr
	: (typeof window !== "undefined" ? window.location.origin : "/")

const apiBase = serverAddr.endsWith("/") ? serverAddr.slice(0, -1) : serverAddr

let peersCache: Peer[] | null = null

export const getServerAddr = () => serverAddr

export const apiGet = async <T>(path: string): Promise<T> => {
	const res = await fetch(`${apiBase}${path}`)
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
	return peers.find((p) => p.id === peerId)
}

export const clearPeerCache = () => {
	peersCache = null
}

export const fetchMimeTypes = async (): Promise<string[]> => {
	const data = await apiGet<{ mime_types: string[] }>("/api/mime-types")
	return data.mime_types
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
	const res = await fetch(`${apiBase}/api/search?${params.toString()}`)
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
