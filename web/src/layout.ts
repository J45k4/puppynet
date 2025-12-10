import { navigate } from "./router"

export const ensureShell = (currentPath: string) => {
	let root = document.getElementById("app-root")
	if (!root) {
		document.body.innerHTML = ""
		root = document.createElement("div")
		root.id = "app-root"
		document.body.appendChild(root)
	}
	root.innerHTML = `
<div class="page">
	<nav class="nav">
		<a href="/" data-route="/">Home</a>
		<a href="/peers" data-route="/peers">Peers</a>
		<a href="/files" data-route="/files">Files</a>
		<a href="/search" data-route="/search">Search</a>
		<a href="/storage" data-route="/storage">Storage</a>
		<a href="/updates" data-route="/updates">Updates</a>
		<a href="/settings" data-route="/settings">Settings</a>
	</nav>
	<main id="content"></main>
</div>
`
	const navLinks = root.querySelectorAll<HTMLAnchorElement>(".nav a")
	navLinks.forEach((link) => {
		const route = link.getAttribute("data-route")
		if (
			route === currentPath ||
			(route && currentPath.startsWith(route + "/"))
		) {
			link.classList.add("active")
		}
		link.addEventListener("click", (ev) => {
			ev.preventDefault()
			const href = link.getAttribute("href")
			if (href) navigate(href)
		})
	})
	const content = root.querySelector<HTMLElement>("#content")
	if (!content) throw new Error("content mount missing")
	return content
}
