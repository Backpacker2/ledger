<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { DocumentType } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	/** `null` is closed; `'new'` creates; a type edits it. */
	let editing = $state<DocumentType | 'new' | null>(null);
	let name = $state('');
	let trigger = $state('');
	let questions = $state<string[]>([]);
	let saving = $state(false);

	function open(type: DocumentType | 'new') {
		editing = type;
		name = type === 'new' ? '' : type.name;
		trigger = type === 'new' ? '' : type.trigger;
		questions = type === 'new' ? [''] : [...type.questions];
	}

	function move(i: number, by: -1 | 1) {
		const j = i + by;
		if (j < 0 || j >= questions.length) return;
		[questions[i], questions[j]] = [questions[j], questions[i]];
	}

	async function save() {
		if (!editing) return;
		saving = true;
		const saved = await attempt(() =>
			editing === 'new'
				? api.createDocumentType(name, trigger, questions)
				: api.updateDocumentType(editing!.slug, name, trigger, questions)
		);
		saving = false;
		if (!saved) return;
		toast(editing === 'new' ? 'Document type created' : 'Document type saved');
		editing = null;
		await refresh();
	}

	async function remove(type: DocumentType) {
		const ok = await confirmDialog({
			title: `Delete “${type.name}”?`,
			message: 'Documents already started keep the guiding questions they were given.',
			confirmLabel: 'Delete type',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteDocumentType(type.slug))) !== undefined) {
			editing = null;
			await refresh();
		}
	}
</script>

<svelte:head><title>Document types · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Document types</h1>
			<div class="sub">
				A type says when to write a document and which guiding questions to ask. Starting a document seeds them into its thread.
			</div>
		</div>
		<div class="header-actions">
			<button class="btn primary" onclick={() => open('new')}>New document type</button>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		<div class="panel">
			{#if data.documentTypes.length === 0}
				<div class="empty-state" style="margin:0 auto">
					<h1>No document types yet</h1>
					<p>Create one — for example “Meeting notes” with a question or two — then use it to start documents from your threads.</p>
					<button class="btn primary" onclick={() => open('new')}>New document type</button>
				</div>
			{:else}
				<div class="tpl-list">
					{#each data.documentTypes as t (t.slug)}
						<div class="tpl-row">
							<div class="name">
								<b>{t.name}</b>
								<span>
									{t.trigger ? `Use when ${t.trigger} · ` : ''}{t.questions.length} question{t.questions.length === 1 ? '' : 's'} ·
									{t.documentCount} document{t.documentCount === 1 ? '' : 's'}
								</span>
							</div>
							<button class="btn small" onclick={() => open(t)}>Edit</button>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>

{#if editing}
	{@const type = editing}
	<Modal title={type === 'new' ? 'New document type' : `Edit ${type.name}`} wide onclose={() => (editing = null)}>
		<label class="field">
			Name
			<input bind:value={name} placeholder="e.g. Interview summary" />
		</label>
		<label class="field">
			Use when…
			<input bind:value={trigger} placeholder="e.g. you have just spoken to a customer" />
		</label>
		<div class="field">
			<span>Guiding questions</span>
			<div class="q-list">
				{#each questions as _, i (i)}
					<div class="q-row">
						<textarea bind:value={questions[i]} rows="2" placeholder="A question to answer in the thread"></textarea>
						<div class="btns">
							<button class="icon-btn" style="color:var(--muted)" aria-label="Move up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
							<button class="icon-btn" style="color:var(--muted)" aria-label="Move down" disabled={i === questions.length - 1} onclick={() => move(i, 1)}>↓</button>
							<button class="icon-btn" style="color:var(--muted)" aria-label="Remove question" onclick={() => questions.splice(i, 1)}>✕</button>
						</div>
					</div>
				{/each}
			</div>
			<div><button class="btn small ghost" onclick={() => questions.push('')}>+ Add question</button></div>
		</div>
		{#snippet footer()}
			{#if type !== 'new'}
				<button class="btn ghost danger" onclick={() => remove(type)} disabled={type.documentCount > 0} title={type.documentCount > 0 ? 'Documents still use this type' : ''}>
					Delete
				</button>
			{/if}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (editing = null)}>Cancel</button>
			<button class="btn primary" onclick={save} disabled={saving || !name.trim()}>Save</button>
		{/snippet}
	</Modal>
{/if}
