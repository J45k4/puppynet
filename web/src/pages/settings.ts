import { ensureShell } from "../layout"

export const renderSettings = () => {
	const content = ensureShell("/settings")
	content.innerHTML = `
		<section class="hero">
			<h1>Settings</h1>
			<p class="lede">Configure PuppyNet.</p>
		</section>
		<div class="card"><p class="muted">Settings UI placeholder.</p></div>
	`
}
