<script lang="ts">
	// Settings, in groups: each is a page of its own under /settings, behind these tabs. Typing in the search shows
	// the matching panels from every group in place of the tab, so a setting can be found without knowing its tab.
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import Search from '@lucide/svelte/icons/search';
	import X from '@lucide/svelte/icons/x';
	import { groups, searchPanels } from '$lib/components/settings/panels';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const path = $derived(page.url.pathname);
	let query = $state('');
	const results = $derived(searchPanels(query));
	const searching = $derived(query.trim() !== '');
	/** The results, grouped by tab, in the order of the tabs. */
	const byGroup = $derived(groups.map((g) => ({ ...g, panels: results.filter((p) => p.group === g.id) })).filter((g) => g.panels.length));
</script>

<div class="header-wrap">
	<div class="header-top">
		<div>
			<h1>Settings</h1>
		</div>
		<div class="header-actions">
			<label class="settings-search">
				<Search size={14} />
				<input
					type="search"
					bind:value={query}
					placeholder="Search settings"
					aria-label="Search settings"
					spellcheck="false"
					autocomplete="off"
					onkeydown={(e) => e.key === 'Escape' && (query = '')}
				/>
			</label>
		</div>
	</div>
	<nav class="tabs" aria-label="Settings">
		{#each groups as g (g.id)}
			{@const href = `/settings/${g.id}`}
			<a class="tab settings-tab" class:active={!searching && (path === href || path.startsWith(href + '/'))} {href} onclick={() => (query = '')}>
				<g.icon size={15} />{g.label}
			</a>
		{/each}
	</nav>
</div>

<div class="page">
	<div class="page-inner">
		{#if searching}
			{#each byGroup as g (g.id)}
				<div class="settings-group-label">{g.label}</div>
				{#each g.panels as p (p.id)}
					<p.component settings={data.settings} />
				{/each}
			{:else}
				<div class="empty-state">
					<p>No settings match “{query.trim()}”.</p>
					<button class="btn small" onclick={() => (query = '')}><X size={13} /> Clear the search</button>
				</div>
			{/each}
		{:else}
			{@render children()}
		{/if}
		<div class="hint" style="padding:0 4px 8px">Ledger {data.settings.version}</div>
	</div>
</div>

<style>
	.settings-tab {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
</style>
