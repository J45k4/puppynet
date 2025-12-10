import { ensureShell } from "../layout"

export const renderFiles = () => {
	const content = ensureShell("/files")
	content.innerHTML = `
		<section class="hero">
			<h1>Files</h1>
			<p class="lede">Coming soon: browse local and shared files.</p>
		</section>
		<div class="card"><p class="muted">The file browser UI from the GUI will be mirrored here.</p></div>
	`
}
