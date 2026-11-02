<script lang="ts">
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import { date } from '$lib/format';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const types = $derived(data.documentTypes);
	let typeSlug = $state('');
	let title = $state('');
	let creating = $state(false);

	// Default to the first type once the list is there.
	$effect(() => {
		if (!types.some((t) => t.slug === typeSlug)) typeSlug = types[0]?.slug ?? '';
	});
	const chosen = $derived(types.find((t) => t.slug === typeSlug));

	async function create() {
		creating = true;
		const doc = await attempt(() => api.createDocument(typeSlug, title, data.thread.slug));
		creating = false;
		if (!doc) return;
		title = '';
		await refresh();
		await goto(`/documents/${doc.id}`);
	}
</script>

<div class="page">
	<div class="page-inner">
		<div class="panel">
			<h2>New document</h2>
			{#if types.length === 0}
				<div class="notice">
					You need a document type first — it decides which guiding questions land in this thread.
					<a href="/document-types">Create one</a>
				</div>
			{:else}
				<p class="sub" style="margin:0">
					The document is linked to this thread and its type's guiding questions are added to the thread as posts for you to answer.
				</p>
				<div class="form-row">
					<label class="field" style="width:220px">
						Type
						<select bind:value={typeSlug}>
							{#each types as t (t.slug)}<option value={t.slug}>{t.name}</option>{/each}
						</select>
					</label>
					<label class="field grow">
						Title
						<input bind:value={title} placeholder="Document title" onkeydown={(e) => e.key === 'Enter' && title.trim() && create()} />
					</label>
					<button class="btn primary" onclick={create} disabled={creating || !title.trim()}>Create &amp; start drafting</button>
				</div>
				{#if chosen}
					<div class="hint">
						{chosen.trigger ? `Use when ${chosen.trigger}. ` : ''}{chosen.questions.length} guiding question{chosen.questions.length === 1 ? '' : 's'}.
					</div>
				{/if}
			{/if}
		</div>

		<div class="panel">
			<h2>Documents in this thread</h2>
			{#if data.thread.documents.length === 0}
				<div class="hint">None yet.</div>
			{:else}
				<div class="doc-list">
					{#each data.thread.documents as d (d.id)}
						<a class="doc-row" href="/documents/{d.id}">
							<span class="doc-title">{d.title}</span>
							<span class="badge type" style="justify-self:start">{d.documentTypeName}</span>
							<span class="badge {d.version ? 'published' : 'draft'}" style="justify-self:start">{d.version ? `v${d.version}` : 'Draft'}</span>
							<span class="doc-date">{date(d.updatedAt)}</span>
						</a>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>
