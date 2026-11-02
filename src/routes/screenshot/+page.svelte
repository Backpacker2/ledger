<script lang="ts">
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { page } from '$app/state';
	import { api } from '$lib/api';
	import { attempt, toast } from '$lib/ui.svelte';

	const url = $derived(page.url.searchParams.get('url') ?? '');

	async function copy() {
		try {
			await navigator.clipboard.writeText(url);
			toast('Link copied');
		} catch {
			toast('Could not reach the clipboard', true);
		}
	}
</script>

<svelte:head><title>Screenshot · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Screenshot</h1>
			<div class="sub">Uploaded and ready to share.</div>
		</div>
		<div class="header-actions">
			<button class="btn" onclick={() => attempt(() => api.takeScreenshot())}>Take another</button>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner wide">
		{#if url}
			<div class="shot-view">
				<div class="form-row">
					<input
						class="mono link-box"
						readonly
						value={url}
						aria-label="Link"
						onfocus={(e) => e.currentTarget.select()}
					/>
					<button class="btn primary" onclick={copy}>Copy link</button>
					<button class="btn" onclick={() => openUrl(url).catch(() => {})}>Open in browser</button>
				</div>
				<img src={url} alt="The screenshot you just took" />
			</div>
		{:else}
			<div class="empty-state">
				<h1>No screenshot yet</h1>
				<p>Press your screenshot shortcut, or choose Take another.</p>
			</div>
		{/if}
	</div>
</div>
