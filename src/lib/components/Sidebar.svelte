<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { newThreadFlow } from '$lib/actions';
	import { ago } from '$lib/format';
	import { drawer, sync } from '$lib/ui.svelte';
	import type { ThreadSummary } from '$lib/types';

	let { threads }: { threads: ThreadSummary[] } = $props();

	let query = $state('');
	const path = $derived(page.url.pathname);
	const on = (prefix: string) => path === prefix || path.startsWith(prefix + '/');

	// Reflect an open search in the box, so refining it is one edit away.
	$effect(() => {
		if (path === '/search') query = page.url.searchParams.get('q') ?? '';
	});

	// What the footer says about the vault and GitHub.
	const footer = $derived.by(() => {
		const s = sync.status;
		if (!s || !s.configured) return { dot: 'off', title: 'Local vault', sub: 'Not synced to GitHub' };
		if (s.state === 'syncing') return { dot: 'warn pulse', title: 'Syncing…', sub: s.repo ?? '' };
		if (s.state === 'error') return { dot: 'bad', title: 'Sync problem', sub: s.lastError ?? 'See Settings', error: true };
		if (s.pending) return { dot: 'warn', title: 'Changes waiting to sync', sub: s.repo ?? '' };
		return { dot: 'ok', title: 'Synced with GitHub', sub: s.lastSyncAt ? `${s.repo} · ${ago(s.lastSyncAt)}` : (s.repo ?? '') };
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
		<div class="sync-line"><span class="dot {footer.dot}"></span><span>{footer.title}</span></div>
		<div class="sync-sub" class:error={footer.error} title={footer.sub}>{footer.sub}</div>
		<a class="btn small" class:on={on('/settings')} href="/settings">Settings</a>
	</div>
</aside>
