import { patternMatcher } from "./pattern-matcher"

type HandlerResult = void | Promise<void>

let matcher: any
const handleRoute = async (path: string) => {
	if (!matcher) return
	const match = matcher.match(path)
	if (!match) {
		console.error("No route found for", path)
		return
	}
	await Promise.resolve(match.result as HandlerResult)
}
window.addEventListener("popstate", () => {
	void handleRoute(window.location.pathname)
})

export const routes = (routes: any) => {
	matcher = patternMatcher(routes)
	void handleRoute(window.location.pathname)
}

export const navigate = (path: string) => {
	window.history.pushState({}, "", path)
	void handleRoute(path)
}
