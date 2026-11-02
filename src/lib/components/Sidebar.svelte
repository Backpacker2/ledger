<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { newThreadFlow } from '$lib/actions';
	import { drawer } from '$lib/ui.svelte';
	import type { ThreadSummary } from '$lib/types';

	let { threads }: { threads: ThreadSummary[] } = $props();

	let query = $state('');
	const path = $derived(page.url.pathname);
	const on = (prefix: string) => path === prefix || path.startsWith(prefix + '/');

	// Reflect an open search in the box, so refining it is one edit away.
	$effect(() => {
		if (path === '/search') query = page.url.searchParams.get('q') ?? '';
	});

	function search(e: SubmitEvent) {
		e.preventDefault();
		const q = query.trim();
		if (q) goto(`/search?q=${encodeURIComponent(q)}`);
	}

	// On a narrow window the drawer closes after every choice.
	function closeDrawer(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('a, button')) drawer.open = false;
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<aside class="sidebar" class:open={drawer.open} onclick={closeDrawer}>
	<div class="brand"><span class="logo">L</span><span>Ledger</span></div>

	<form class="search-wrap" onsubmit={search}>
		<input type="search" placeholder="Search everything…" autocomplete="off" spellcheck="false" bind:value={query} />
	</form>

	<a class="nav-item" class:active={on('/planboard')} href="/planboard"><span class="nav-ico">▦</span>Planboard</a>
	<a class="nav-item" class:active={on('/documents')} href="/documents"><span class="nav-ico">▤</span>Documents</a>
	<a class="nav-item" class:active={on('/document-types')} href="/document-types"><span class="nav-ico">❖</span>Document types</a>

	<div class="section-head">
		<span>Threads</span>
		<button class="icon-btn" title="New thread" aria-label="New thread" onclick={newThreadFlow}>+</button>
	</div>
	<nav class="space-list">
		{#each threads as t (t.slug)}
			<a class="space-item" class:active={on(`/threads/${t.slug}`)} href="/threads/{t.slug}">
				<span class="dot"></span>
				<span class="name">{t.title}</span>
				<span class="count">{t.postCount || ''}</span>
			</a>
		{:else}
			<div class="space-empty">No threads yet</div>
		{/each}
	</nav>

	<div class="spacer"></div>

	<div class="sync-box">
		<div class="sync-line"><span class="dot ok"></span><span>Stored on this computer</span></div>
		<div class="sync-sub">Everything lives in a local SQLite database.</div>
		<a class="btn small" class:on={on('/settings')} href="/settings">Settings</a>
	</div>
</aside>
