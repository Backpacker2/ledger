<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import Dialogs from '$lib/components/Dialogs.svelte';
	import ScreenshotModal from '$lib/components/ScreenshotModal.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import Toasts from '$lib/components/Toasts.svelte';
	import { drawer, initTheme, startBackendEvents } from '$lib/ui.svelte';
	import wordmark from '$lib/assets/brand/svg/ledger-wordmark-on-dark.svg';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	// The screenshot overlay is a window of its own and shows no app chrome.
	const bare = $derived(page.url.pathname.startsWith('/screenshot-overlay'));

	onMount(() => {
		if (bare) return;
		initTheme();
		return startBackendEvents();
	});
</script>

{#if bare}
	{@render children()}
{:else}
	<div class="app">
		<header class="topbar">
			<button class="icon-btn" aria-label="Menu" onclick={() => (drawer.open = true)}>☰</button>
			<img src={wordmark} alt="Ledger" />
		</header>
		<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
		<div class="scrim" class:open={drawer.open} onclick={() => (drawer.open = false)}></div>

		<Sidebar projects={data.projects} />
		<main>{@render children()}</main>
	</div>

	<ScreenshotModal projects={data.projects} />
	<Dialogs />
	<Toasts />
{/if}
