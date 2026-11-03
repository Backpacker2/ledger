<script lang="ts">
	import { deliverableUrl, threadUrl } from '$lib/actions';
	import { dateTime, highlight, plural } from '$lib/format';
	import { goto } from '$app/navigation';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	// A box of its own on the page: on a phone the sidebar's is tucked away in the menu.
	let query = $state('');
	$effect(() => {
		query = data.q;
	});
	function search(e: SubmitEvent) {
		e.preventDefault();
		const q = query.trim();
		if (q) goto(`/search?q=${encodeURIComponent(q)}`);
	}

	type Hit = PageData['hits'][number];

	const DOM_PREFIX = { post: 'p', prompt: 'pr', feedback: 'f' } as const;
	const LABEL = { post: 'Post in', prompt: 'Prompt in', feedback: 'Feedback in', deliverable: 'Deliverable' } as const;

	const href = (hit: Hit) =>
		hit.kind === 'deliverable' || !hit.threadSlug
			? deliverableUrl(hit.projectSlug, hit.id)
			: `${threadUrl(hit.projectSlug, hit.threadSlug)}?item=${DOM_PREFIX[hit.kind]}-${hit.id}`;
</script>

<svelte:head><title>Search · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Search</h1>
			<div class="sub">
				{#if data.q}
					{plural(data.hits.length, 'result')} for “{data.q}”
				{:else}
					Every post, prompt, feedback and deliverable.
				{/if}
			</div>
		</div>
	</div>
	<form class="search-page-form" onsubmit={search} role="search">
		<!-- svelte-ignore a11y_autofocus -->
		<input type="search" placeholder="Search everything…" autocomplete="off" spellcheck="false" enterkeyhint="search" bind:value={query} autofocus={!data.q} aria-label="Search" />
	</form>
</div>

<div class="page">
	<div class="page-inner">
		{#if data.q && data.hits.length === 0}
			<div class="empty-state"><h1>Nothing found</h1><p>No post, prompt, feedback or deliverable mentions “{data.q}”.</p></div>
		{/if}
		<div class="hit-list">
			{#each data.hits as hit (hit.kind + hit.projectSlug + hit.id)}
				<a class="hit" href={href(hit)}>
					<div class="hit-head">
						<span class="hit-kind">{LABEL[hit.kind]}</span>
						<span class="hit-title">{hit.title}</span>
						<span class="badge accent">{hit.projectName}</span>
						<span class="hint">{dateTime(hit.createdAt)}</span>
					</div>
					<div class="hit-snippet">{@html highlight(hit.snippet, data.q)}</div>
				</a>
			{/each}
		</div>
	</div>
</div>
