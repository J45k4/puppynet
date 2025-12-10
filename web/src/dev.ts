import index from "./index.html"

const serverAddr = Bun.env.SERVER_ADDR ?? "http://localhost:8832"

Bun.serve({
	port: 4222,
	routes: {
		"/config.js": () => {
			const body = `window.__CONFIG__ = ${JSON.stringify({ serverAddr })};`
			return new Response(body, {
				headers: { "Content-Type": "application/javascript" },
			})
		},
		"/*": index
	}
})
