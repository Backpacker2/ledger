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
	let threadSlug = $state('');
	let filter = $state('');
	let creating = $state(false);

	$effect(() => {
		if (!types.some((t) => t.slug === typeSlug)) typeSlug = types[0]?.slug ?? '';
	});

	const shown = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return data.documents;
		return data.documents.filter((d) =>
			[d.title, d.documentTypeName, d.threadTitle ?? ''].some((s) => s.toLowerCase().includes(q))
		);
	});

	async function create() {
		creating = true;
		const doc = await attempt(() => api.createDocument(typeSlug, title, threadSlug || null));
		creating = false;
		if (!doc) return;
		title = '';
		await refresh();
		await goto(`/documents/${doc.id}`);
	}
</script>

<svelte:head><title>Documents · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Documents</h1>
			<div class="sub">Every draft and published version, from all threads.</div>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner wide">
		<div class="panel">
			<h2>New document</h2>
			{#if types.length === 0}
				<div class="notice">
					Documents need a type first. <a href="/document-types">Create a document type</a>
				</div>
			{:else}
				<div class="form-row">
					<label class="field" style="width:200px">
						Type
						<select bind:value={typeSlug}>
							{#each types as t (t.slug)}<option value={t.slug}>{t.name}</option>{/each}
						</select>
					</label>
					<label class="field grow">
						Title
						<input bind:value={title} placeholder="Document title" onkeydown={(e) => e.key === 'Enter' && title.trim() && create()} />
					</label>
					<label class="field" style="width:220px">
						Thread
						<select bind:value={threadSlug}>
							<option value="">Not yet — start drafting later</option>
							{#each data.threads as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
						</select>
					</label>
					<button class="btn primary" onclick={create} disabled={creating || !title.trim()}>Create</button>
				</div>
			{/if}
		</div>

		<div class="panel">
			<div class="form-row" style="justify-content:space-between;align-items:center">
				<h2>All documents <span class="hint">{data.documents.length}</span></h2>
				<input class="inline" type="search" placeholder="Filter…" bind:value={filter} style="width:220px;padding:6px 10px;border-radius:8px;border:1px solid var(--line);background:var(--bg)" />
			</div>
			{#if shown.length === 0}
				<div class="hint">{data.documents.length ? 'No documents match.' : 'No documents yet.'}</div>
			{:else}
				<div class="doc-list">
					{#each shown as d (d.id)}
						<div class="doc-row all">
							<a class="doc-title" href="/documents/{d.id}" style="color:inherit;text-decoration:none">{d.title}</a>
							<span class="badge type" style="justify-self:start">{d.documentTypeName}</span>
							{#if d.threadSlug}
								<a class="doc-space" href="/threads/{d.threadSlug}">{d.threadTitle}</a>
							{:else}
								<span class="doc-space hint">No thread</span>
							{/if}
							<span class="badge {d.version ? 'published' : 'draft'}" style="justify-self:start">{d.version ? `v${d.version}` : 'Draft'}</span>
							<span class="doc-date">{date(d.updatedAt)}</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>
