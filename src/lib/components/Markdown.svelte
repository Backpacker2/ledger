<script lang="ts">
	import { openUrl } from '@tauri-apps/plugin-opener';

	/** `html` is rendered by the backend, which escapes raw HTML and drops unsafe link targets. */
	let { html }: { html: string } = $props();

	// Links open in the system browser; the app window itself never navigates away.
	function onclick(e: MouseEvent) {
		const anchor = (e.target as HTMLElement).closest('a');
		if (!anchor) return;
		e.preventDefault();
		const href = anchor.getAttribute('href') ?? '';
		if (/^(https?:|mailto:)/i.test(href)) openUrl(href).catch(() => {});
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="md" {onclick}>{@html html}</div>
