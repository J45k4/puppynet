import { routes } from "./router"
import { getServerAddr } from "./api"
import { renderHome } from "./pages/home"
import { renderPeers, renderPeerDetail } from "./pages/peers"
import { renderFiles } from "./pages/files"
import { renderSearch } from "./pages/search"
import { renderStorage } from "./pages/storage"
import { renderUpdates } from "./pages/updates"
import { renderSettings } from "./pages/settings"
import { renderUsers, renderUserDetail } from "./pages/users"
import { renderLogin } from "./pages/login"
import { renderFileByHash } from "./pages/file"

const serverAddr = getServerAddr()

window.onload = () => {
	const body = document.querySelector("body")
	if (!body) {
		throw new Error("No body element found")
	}
	routes({
		"/": () => renderHome(),
		"/login": () => renderLogin(),
		"/peers": () => renderPeers(),
		"/peers/:peerId": ({ peerId }: { peerId: string }) =>
			renderPeerDetail(peerId),
		"/user": () => renderUsers(),
		"/user/:userId": ({ userId }: { userId: string }) =>
			renderUserDetail(userId),
		"/files": () => renderFiles(),
		"/search": () => renderSearch(),
		"/storage": () => renderStorage(),
		"/updates": () => renderUpdates(),
		"/settings": () => renderSettings(),
		"/file/:hash": ({ hash }: { hash: string }) => renderFileByHash(hash),
		"/*": () => renderHome(),
	})

	console.info("Using server address:", serverAddr)
}
