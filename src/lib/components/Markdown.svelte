<script lang="ts">
	import { onDestroy } from 'svelte';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { imageLoader, isVaultSrc, type ImageBase } from '$lib/vaultImages';

	/**
	 * `html` is rendered by the backend, which escapes raw HTML and drops unsafe link targets. With `images`, pictures
	 * in the vault are shown too, read from there.
	 */
	let { html, images }: { html: string; images?: ImageBase } = $props();

	let root: HTMLDivElement;
	const pictures = imageLoader(() => images);
	onDestroy(pictures.dispose);

	// The backend writes `<img src="chart.png">`, which the window would look for beside the app: a vault picture waits
	// in data-src until it has been read.
	const shown = $derived(images ? html.replace(/<img src="([^"]*)"/g, (img, src) => (isVaultSrc(src) ? `<img data-src="${src}"` : img)) : html);

	$effect(() => {
		void shown;
		for (const img of root.querySelectorAll<HTMLImageElement>('img[data-src]')) {
			pictures.load(img.dataset.src ?? '').then((url) => {
				if (url) img.src = url;
				else img.replaceWith(Object.assign(document.createElement('span'), { className: 'missing', textContent: `Couldn’t load ${img.alt || img.dataset.src}` }));
			});
		}
	});

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
<div class="md" {onclick} bind:this={root}>{@html shown}</div>
