import { ensureShell } from "../layout"

export const renderHome = () => {
	const content = ensureShell("/")
	content.innerHTML = `
<section class="hero">
	<h1>PuppyNet</h1>
	<p class="lede">Welcome to PuppyNet. Use the navigation to explore peers, files, and settings.</p>
</section>
<div class="card">
	<h2>Getting started</h2>
	<p class="muted">Browse peers to inspect connections or open other sections to mirror the desktop GUI.</p>
</div>
`
}
