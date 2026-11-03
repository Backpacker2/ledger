<script lang="ts">
	import Menu from '@lucide/svelte/icons/menu';
	import '../app.css';
	import { onMount } from 'svelte';
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { afterNavigate } from '$app/navigation';
	import CaptureBar from '$lib/components/CaptureBar.svelte';
	import Dialogs from '$lib/components/Dialogs.svelte';
	import ScreenshotModal from '$lib/components/ScreenshotModal.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import Toasts from '$lib/components/Toasts.svelte';
	import UpdateNotice from '$lib/components/UpdateNotice.svelte';
	import { drawer, initTheme, startBackendEvents } from '$lib/ui.svelte';
	import { startUpdates } from '$lib/updates.svelte';
	import { captureDebug, startLogEvents } from '$lib/log.svelte';
	import Wordmark from '$lib/components/Wordmark.svelte';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	// The screenshot overlay is a window of its own and shows no app chrome.
	const bare = $derived(page.url.pathname.startsWith('/screenshot-overlay'));

	// A debug capture notes each page opened (its path: project and thread names are in it).
	afterNavigate(({ to }) => {
		if (to && !bare) captureDebug('page', to.url.pathname);
	});

	onMount(() => {
		if (bare) return;
		initTheme();
		const stopLog = startLogEvents();
		const stopEvents = startBackendEvents();
		const stopUpdates = startUpdates();
		return () => {
			stopLog();
			stopEvents();
			stopUpdates();
		};
	});
</script>

{#if bare}
	{@render children()}
{:else}
	<div class="app">
		<header class="topbar">
			<button class="icon-btn" aria-label="Menu" onclick={() => (drawer.open = true)}><Menu size={22} /></button>
			<Wordmark height={22} />
		</header>
		<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
		<div class="scrim" class:open={drawer.open} onclick={() => (drawer.open = false)}></div>

		<Sidebar projects={data.projects} />
		<main>{@render children()}</main>
		<CaptureBar />
	</div>

	<ScreenshotModal projects={data.projects} />
	<UpdateNotice />
	<Dialogs />
	<Toasts />
{/if}
