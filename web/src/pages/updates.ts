import { ensureShell } from "../layout"

export const renderUpdates = () => {
	const content = ensureShell("/updates")
	content.innerHTML = `
		<section class="hero">
			<h1>Updates</h1>
			<p class="lede">Manage updates and versions.</p>
		</section>
		<div class="card"><p class="muted">Update management placeholder.</p></div>
	`
}
