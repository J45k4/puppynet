import { routes } from "./router"

const serverAddr =
  (typeof process !== "undefined" && process.env.PUBLIC_SERVER_ADDR)
    ? process.env.PUBLIC_SERVER_ADDR
    : "/";

window.onload = () => {
	const body = document.querySelector("body")
	if (!body) {
		throw new Error("No body element found")
	}
	routes({
		"/peers": () => document.body.innerHTML = "<h1>Peers</h1><p>List of peers will be shown here.</p>",
		"/*": () => document.body.innerHTML = "<h1>Home</h1><p>Welcome to PuppyNet!</p>",
	})

	console.info("Using server address:", serverAddr)
}
