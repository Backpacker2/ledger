<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import type { Snippet } from 'svelte';
	import Dialogs from '$lib/components/Dialogs.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import Toasts from '$lib/components/Toasts.svelte';
	import { drawer, initTheme } from '$lib/ui.svelte';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	onMount(initTheme);
</script>

<div class="app">
	<header class="topbar">
		<button class="icon-btn" aria-label="Menu" onclick={() => (drawer.open = true)}>☰</button>
		<span>Ledger</span>
	</header>
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="scrim" class:open={drawer.open} onclick={() => (drawer.open = false)}></div>

	<Sidebar threads={data.threads} />
	<main>{@render children()}</main>
</div>

<Dialogs />
<Toasts />
