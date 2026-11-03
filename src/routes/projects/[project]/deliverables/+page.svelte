<script lang="ts">
	import { goto } from '$app/navigation';
	import MmmBadge from '$lib/components/MmmBadge.svelte';
	import { api } from '$lib/api';
	import { deliverableUrl, formatSize, snapshotPath } from '$lib/actions';
	import { ago, plural } from '$lib/format';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const project = $derived(data.project.slug);

	let name = $state('');
	let templateId = $state('');
	let filter = $state('');
	let creating = $state(false);

	const shown = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		return q ? data.deliverables.filter((d) => d.path.toLowerCase().includes(q)) : data.deliverables;
	});

	const KIND = { markdown: 'Markdown', image: 'Image', other: 'File' } as const;

	/** A deliverable whose live file is gone opens at its newest published version. */
	const target = (d: PageData['deliverables'][number]) =>
		d.exists || d.versions.length === 0 ? d.path : snapshotPath(d.path, d.versions.at(-1) as string);

	async function create() {
		creating = true;
		const skeleton = data.templates.find((t) => t.id === templateId)?.body ?? '';
		const made = await attempt(() => api.createDeliverable(project, name, skeleton));
		creating = false;
		if (!made) return;
		name = '';
		await refresh();
		await goto(deliverableUrl(project, made.path));
	}
</script>

<svelte:head><title>Deliverables · {data.project.name} · Ledger</title></svelte:head>

<div class="page">
	<div class="page-inner wide">
		<div class="panel">
			<h2>New deliverable</h2>
			<p class="sub" style="margin:0">
				Deliverables are what prompts produce. They are plain files in <code>deliverables/</code>: markdown you can edit here, or anything
				else — images, spreadsheets, PDFs — that you or an assistant drop into the folder. Publishing freezes a copy as
				<code>name@v1.0.md</code>.
			</p>
			<div class="form-row">
				<label class="field grow">
					File name
					<input bind:value={name} placeholder="e.g. pricing-brief.md" onkeydown={(e) => e.key === 'Enter' && name.trim() && create()} />
				</label>
				<label class="field" style="width:240px">
					Start from
					<select bind:value={templateId}>
						<option value="">Empty file</option>
						{#each data.templates as t (t.scope + '/' + t.id)}<option value={t.id}>{t.name}</option>{/each}
					</select>
				</label>
				<button class="btn primary" onclick={create} disabled={creating || !name.trim()}>Create</button>
			</div>
		</div>

		<div class="panel">
			<div class="header-top">
				<h2>{plural(data.deliverables.length, 'deliverable')}</h2>
				<input
					type="search"
					placeholder="Filter…"
					bind:value={filter}
					style="padding:6px 10px;border-radius:8px;border:1px solid var(--line);background:var(--bg)"
				/>
			</div>
			{#if data.deliverables.length === 0}
				<div class="hint">Nothing yet. Create one from a prompt in a thread, or here.</div>
			{:else}
				<div class="doc-list">
					{#each shown as d (d.path)}
						<a class="doc-row all" href={deliverableUrl(project, target(d))}>
							<span class="doc-title" title={d.path}>{d.path}</span>
							<span class="doc-space">
								{#if d.versions.length}
									<span class="badge published">v{d.versions.at(-1)}</span>
								{:else}
									<span class="badge draft">Draft</span>
								{/if}
								{#if d.mmm}<MmmBadge level={d.mmm} />{/if}
								{#if !d.exists}<span class="badge">live file removed</span>{/if}
								{#if d.openFeedback}<span class="badge draft">{d.openFeedback} open</span>{/if}
							</span>
							<span class="doc-space">{plural(d.promptCount, 'prompt')}</span>
							<span class="badge type">{KIND[d.kind]} · {formatSize(d.size)}</span>
							<span class="doc-date">{d.exists ? ago(d.modified) : ''}</span>
						</a>
					{:else}
						<div class="hint">Nothing matches “{filter}”.</div>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>
