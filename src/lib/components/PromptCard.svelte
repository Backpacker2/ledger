<script lang="ts">
	import X from '@lucide/svelte/icons/x';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import { baseName, deliverableUrl } from '$lib/actions';
	import { time } from '$lib/format';
	import { attempt, confirmDialog, promptDialog, refresh } from '$lib/ui.svelte';
	import type { DeliverableSummary, Prompt } from '$lib/types';
	import ClaudeRunPanel from './ClaudeRunPanel.svelte';
	import Markdown from './Markdown.svelte';
	import Modal from './Modal.svelte';

	let {
		project,
		thread,
		prompt,
		onedit
	}: { project: string; thread: string; prompt: Prompt; onedit: (prompt: Prompt) => void } = $props();

	let linking = $state(false);
	let choices = $state<DeliverableSummary[]>([]);

	async function create() {
		const name = await promptDialog({
			title: 'New deliverable',
			label: 'File name',
			initial: prompt.title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''),
			placeholder: 'e.g. pricing-brief.md',
			confirmLabel: 'Create'
		});
		if (!name) return;
		const before = new Set(prompt.deliverables.map((d) => d.path));
		const updated = await attempt(() => api.createPromptDeliverable(project, thread, prompt.id, name));
		if (!updated) return;
		await refresh();
		const added = updated.deliverables.find((d) => !before.has(d.path));
		if (added) await goto(deliverableUrl(project, added.path));
	}

	async function openLinker() {
		const all = await attempt(() => api.listDeliverables(project));
		if (!all) return;
		const linked = new Set(prompt.deliverables.map((d) => d.path));
		choices = all.filter((d) => !linked.has(d.path));
		linking = true;
	}

	async function link(path: string) {
		linking = false;
		if (await attempt(() => api.linkPromptDeliverable(project, thread, prompt.id, path))) await refresh();
	}

	async function unlink(path: string) {
		if (await attempt(() => api.unlinkPromptDeliverable(project, thread, prompt.id, path))) await refresh();
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Delete “${prompt.title}”?`,
			message: 'The deliverables it produced are kept.',
			confirmLabel: 'Delete',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deletePrompt(project, thread, prompt.id))) !== undefined) await refresh();
	}
</script>

<div class="card prompt-card" id="pr-{prompt.id}">
	<div class="meta">
		<span class="card-kind">Prompt</span>
		<span class="time">{time(prompt.createdAt)}</span>
		{#if prompt.templateName}<span class="badge accent">{prompt.templateName}</span>{/if}
		{#if prompt.updatedAt !== prompt.createdAt}<span class="file">edited</span>{/if}
	</div>
	<div class="card-title">{prompt.title}</div>
	{#if prompt.body.trim()}<Markdown html={prompt.html} />{/if}

	<div class="field">
		<span>Deliverables</span>
		<div class="chips">
			{#each prompt.deliverables as d (d.path)}
				<span class="chip" class:plain={!d.exists}>
					{#if d.exists}
						<a href={deliverableUrl(project, d.path)} style="color:inherit;text-decoration:none">{baseName(d.path)}</a>
					{:else}
						<span title="This file is no longer in the project">{baseName(d.path)} (missing)</span>
					{/if}
					<button class="x" aria-label="Unlink {baseName(d.path)}" onclick={() => unlink(d.path)}><X size={12} /></button>
				</span>
			{:else}
				<span class="hint">Nothing produced yet</span>
			{/each}
		</div>
	</div>

	<div class="actions">
		<button class="link accent" onclick={create}>Create deliverable…</button>
		<button class="link accent" onclick={openLinker}>Link existing…</button>
		<button class="link" onclick={() => onedit(prompt)}>Edit</button>
		<button class="link danger" onclick={remove}>Delete</button>
	</div>
	<ClaudeRunPanel {project} {thread} {prompt} />
</div>

{#if linking}
	<Modal title="Link a deliverable" onclose={() => (linking = false)}>
		{#if choices.length === 0}
			<p class="hint" style="margin:0">Every deliverable in this project is already linked, or there are none yet.</p>
		{:else}
			<div class="link-grid">
				{#each choices as d (d.path)}
					<button class="btn ghost" style="justify-content:flex-start" onclick={() => link(d.path)}>{d.path}</button>
				{/each}
			</div>
		{/if}
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (linking = false)}>Close</button>
		{/snippet}
	</Modal>
{/if}
