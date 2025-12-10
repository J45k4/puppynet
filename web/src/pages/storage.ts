import { ensureShell } from "../layout"

export const renderStorage = () => {
	const content = ensureShell("/storage")
	content.innerHTML = `
		<section class="hero">
			<h1>Storage</h1>
			<p class="lede">Storage overview and replication.</p>
		</section>
		<div class="card"><p class="muted">Storage dashboard placeholder.</p></div>
	`
}
