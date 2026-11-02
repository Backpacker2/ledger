<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { newProjectFlow, newThreadFlow, projectUrl, threadUrl } from '$lib/actions';
	import { ago } from '$lib/format';
	import { orderThreads } from '$lib/order';
	import { listOrder } from '$lib/order.svelte';
	import { drawer, sync } from '$lib/ui.svelte';
	import type { ProjectSummary } from '$lib/types';

	let { projects }: { projects: ProjectSummary[] } = $props();

	const OPEN_KEY = 'ledger-open-projects';

	let query = $state('');
	let open = $state<string[]>(load());
	const path = $derived(page.url.pathname);
	const on = (prefix: string) => path === prefix || path.startsWith(prefix + '/');
	const currentProject = $derived(page.params.project);

	function load(): string[] {
		try {
			const saved = JSON.parse(localStorage.getItem(OPEN_KEY) ?? '[]');
			return Array.isArray(saved) ? saved.filter((s) => typeof s === 'string') : [];
		} catch {
			return [];
		}
	}

	function remember() {
		try {
			localStorage.setItem(OPEN_KEY, JSON.stringify(open));
		} catch {
			// The tree still works for this session.
		}
	}

	function toggle(slug: string) {
		open = open.includes(slug) ? open.filter((s) => s !== slug) : [...open, slug];
		remember();
	}

	// The project being looked at is always unfolded, so its threads are one click away.
	$effect(() => {
		const slug = currentProject;
		if (slug && !open.includes(slug)) {
			open = [...open, slug];
			remember();
		}
	});

	// Reflect an open search in the box, so refining it is one edit away.
	$effect(() => {
		if (path === '/search') query = page.url.searchParams.get('q') ?? '';
	});

	// What the footer says about the vault and GitHub.
	const footer = $derived.by(() => {
		const s = sync.status;
		if (!s || !s.configured) return { dot: 'off', title: 'Local vault', sub: 'Not synced to GitHub' };
		// Next to where the files go, whether they go up sealed.
		const where = `${s.repo ?? ''} · ${s.encrypted ? 'encrypted' : 'not encrypted'}`;
		if (s.state === 'syncing') return { dot: 'warn pulse', title: 'Syncing…', sub: where };
		if (s.state === 'error') return { dot: 'bad', title: 'Sync problem', sub: s.lastError ?? 'See Settings', error: true };
		if (s.pending) return { dot: 'warn', title: 'Changes waiting to sync', sub: where };
		return { dot: 'ok', title: 'Synced with GitHub', sub: s.lastSyncAt ? `${where} · ${ago(s.lastSyncAt)}` : where };
	});

	function search(e: SubmitEvent) {
		e.preventDefault();
		const q = query.trim();
		if (q) goto(`/search?q=${encodeURIComponent(q)}`);
	}

	// On a narrow window the drawer closes after every choice (but not after folding a project).
	function closeDrawer(e: MouseEvent) {
		const target = (e.target as HTMLElement).closest('a, button');
		if (target && !target.classList.contains('folder-toggle')) drawer.open = false;
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<aside class="sidebar" class:open={drawer.open} onclick={closeDrawer}>
	<div class="brand"><span class="logo">L</span><span>Ledger</span></div>

	<form class="search-wrap" onsubmit={search}>
		<input type="search" placeholder="Search everything…" autocomplete="off" spellcheck="false" bind:value={query} />
	</form>

	<a class="nav-item" class:active={path === '/'} href="/"><span class="nav-ico">⌂</span>Home</a>
	<a class="nav-item" class:active={on('/templates')} href="/templates"><span class="nav-ico">❖</span>Templates</a>

	<div class="section-head">
		<span>Projects</span>
		<button class="icon-btn" title="New project" aria-label="New project" onclick={newProjectFlow}>+</button>
	</div>
	<nav class="space-list">
		{#each projects as p (p.slug)}
			{@const unfolded = open.includes(p.slug)}
			<div class="folder-row" class:active={on(projectUrl(p.slug)) && !page.params.thread}>
				<button
					class="folder-toggle"
					aria-label={unfolded ? `Fold ${p.name}` : `Unfold ${p.name}`}
					aria-expanded={unfolded}
					onclick={() => toggle(p.slug)}
				>
					{unfolded ? '▾' : '▸'}
				</button>
				<a class="folder-link" href={projectUrl(p.slug)}>
					<span class="name">{p.name}</span>
					<span class="count">{p.threads.length || ''}</span>
				</a>
			</div>
			{#if unfolded}
				<div class="folder-children">
					{#each orderThreads(p.threads, listOrder.explorer) as t (t.slug)}
						<a class="tree-item" class:active={path === threadUrl(p.slug, t.slug)} href={threadUrl(p.slug, t.slug)}>
							<span class="dot"></span>
							<span class="name">{t.title}</span>
							<span class="count">{t.postCount || ''}</span>
						</a>
					{:else}
						<div class="space-empty">No threads yet</div>
					{/each}
					<button class="tree-item add" onclick={() => newThreadFlow(p.slug)}>＋ New thread</button>
				</div>
			{/if}
		{:else}
			<div class="space-empty">No projects yet</div>
		{/each}
	</nav>

	<div class="spacer"></div>

	<div class="sync-box">
		<div class="sync-line"><span class="dot {footer.dot}"></span><span>{footer.title}</span></div>
		<div class="sync-sub" class:error={footer.error} title={footer.sub}>{footer.sub}</div>
		<a class="btn small" class:on={on('/settings')} href="/settings">Settings</a>
	</div>
</aside>
