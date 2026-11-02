<script lang="ts">
	import { dateTime, highlight, plural } from '$lib/format';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const href = (hit: PageData['hits'][number]) =>
		hit.kind === 'post' ? `/threads/${hit.threadSlug}?post=${hit.id}` : `/documents/${hit.id}`;
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
					Type in the search box on the left.
				{/if}
			</div>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		{#if data.q && data.hits.length === 0}
			<div class="empty-state"><h1>Nothing found</h1><p>No post or document mentions “{data.q}”.</p></div>
		{/if}
		<div class="hit-list">
			{#each data.hits as hit (hit.kind + hit.id)}
				<a class="hit" href={href(hit)}>
					<div class="hit-head">
						<span class="hit-kind">{hit.kind === 'post' ? 'Post in' : 'Document'}</span>
						<span class="hit-title">{hit.title}</span>
						{#if hit.kind === 'document' && hit.threadTitle}<span class="badge accent">{hit.threadTitle}</span>{/if}
						<span class="hint">{dateTime(hit.createdAt)}</span>
					</div>
					<div class="hit-snippet">{@html highlight(hit.snippet, data.q)}</div>
				</a>
			{/each}
		</div>
	</div>
</div>
