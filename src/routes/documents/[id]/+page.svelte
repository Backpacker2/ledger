<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import Markdown from '$lib/components/Markdown.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { api } from '$lib/api';
	import { nextVersion } from '$lib/actions';
	import { date, dateTime } from '$lib/format';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Bump } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const doc = $derived(data.doc);
	const readonly = $derived(doc.snapshot);

	// ---- the editable copy ---------------------------------------------------------------------
	// The page keeps its own title/body while you type and saves them in the background. It is only reset
	// when a different document (or version) is opened, never by a refresh after one of its own saves.

	let title = $state('');
	let body = $state('');
	let html = $state('');
	let loadedKey = '';
	let loadedId = '';
	let dirty = false;
	let saveState = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | undefined;
	let renderTimer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		const key = `${doc.id}|${doc.snapshot ? doc.version : 'draft'}`;
		if (key === loadedKey) return;
		untrack(() => {
			if (dirty) void flush(); // still the previous document's id and text at this point
			loadedKey = key;
			loadedId = doc.id;
			title = doc.title;
			body = doc.body;
			html = doc.html;
			saveState = 'saved';
		});
	});

	async function flush() {
		clearTimeout(saveTimer);
		if (!dirty) return;
		dirty = false;
		saveState = 'saving';
		const saved = await attempt(() => api.updateDocument(loadedId, { body }));
		saveState = saved ? 'saved' : 'unsaved';
		if (!saved) dirty = true;
	}

	function edited() {
		dirty = true;
		saveState = 'unsaved';
		clearTimeout(saveTimer);
		saveTimer = setTimeout(flush, 800);
		clearTimeout(renderTimer);
		renderTimer = setTimeout(async () => {
			html = (await attempt(() => api.renderMarkdown(body))) ?? html;
		}, 250);
	}

	onDestroy(() => {
		clearTimeout(renderTimer);
		void flush();
	});

	async function saveTitle() {
		const next = title.trim();
		if (!next) {
			title = doc.title;
			return;
		}
		if (next === doc.title) return;
		if (await attempt(() => api.updateDocument(doc.id, { title: next }))) await refresh();
	}

	// ---- view mode -----------------------------------------------------------------------------

	type Mode = 'write' | 'preview' | 'split';
	let mode = $state<Mode>('split');
	try {
		const saved = localStorage.getItem('ledger-doc-mode');
		if (saved === 'write' || saved === 'preview' || saved === 'split') mode = saved;
	} catch {
		// Storage unavailable: the default is fine.
	}
	function setMode(next: Mode) {
		mode = next;
		try {
			localStorage.setItem('ledger-doc-mode', next);
		} catch {
			// Not persisted.
		}
	}

	// ---- actions -------------------------------------------------------------------------------

	let publishing = $state(false);
	let bump = $state<Bump | 'custom'>('minor');
	let custom = $state('');
	let publishBusy = $state(false);
	const preview = $derived({ minor: nextVersion(doc.versions, 'minor'), major: nextVersion(doc.versions, 'major') });

	async function publish() {
		publishBusy = true;
		await flush();
		const published = await attempt(() =>
			api.publishDocument(doc.id, bump === 'custom' ? 'minor' : bump, bump === 'custom' ? custom : null)
		);
		publishBusy = false;
		if (!published) return;
		publishing = false;
		toast(`Published version ${published.version}`);
		await refresh();
	}

	let drafting = $state(false);
	let target = $state<'existing' | 'new'>('existing');
	let existing = $state('');
	let newTitle = $state('');
	let draftBusy = $state(false);

	function openDrafting() {
		target = data.threads.length ? 'existing' : 'new';
		existing = data.threads[0]?.slug ?? '';
		newTitle = doc.title;
		drafting = true;
	}

	async function startDrafting() {
		draftBusy = true;
		const started = await attempt(() =>
			api.startDrafting(doc.id, target === 'existing' ? existing : null, target === 'new' ? newTitle : null)
		);
		draftBusy = false;
		if (!started) return;
		drafting = false;
		toast('Guiding questions added to the thread');
		await refresh();
		await goto(`/threads/${started.threadSlug}`);
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Delete “${doc.title}”?`,
			message: doc.versions.length
				? 'The draft is deleted. Its published versions are kept and stay readable from the thread.'
				: 'The draft is deleted.',
			confirmLabel: 'Delete document',
			danger: true
		});
		if (!ok) return;
		dirty = false;
		if ((await attempt(() => api.deleteDocument(doc.id))) !== undefined) {
			await refresh();
			await goto(doc.threadSlug ? `/threads/${doc.threadSlug}/documents` : '/documents');
		}
	}

	async function copyMarkdown() {
		try {
			await navigator.clipboard.writeText(readonly ? doc.body : body);
			toast('Markdown copied');
		} catch {
			toast('Could not reach the clipboard', true);
		}
	}
</script>

<svelte:head><title>{doc.title} · Ledger</title></svelte:head>

<div class="header-wrap">
	<div class="header-top">
		<div style="flex:1">
			<h1>
				{#if readonly}
					{doc.title}
				{:else}
					<input class="title-input" bind:value={title} onchange={saveTitle} aria-label="Title" />
				{/if}
			</h1>
			<div class="subline">
				<span class="badge type">{doc.documentTypeName}</span>
				{#if readonly}
					<span class="badge published">v{doc.version}</span>
				{:else}
					<span class="badge {doc.version ? 'published' : 'draft'}">{doc.version ? `Draft after v${doc.version}` : 'Draft'}</span>
				{/if}
				{#if doc.threadSlug}
					<a class="chip" href="/threads/{doc.threadSlug}">{doc.threadTitle}</a>
				{:else if !readonly}
					<span class="hint">Not linked to a thread</span>
				{/if}
				{#if !readonly}<span class="save-state">{saveState === 'saved' ? 'Saved' : saveState === 'saving' ? 'Saving…' : 'Unsaved changes'}</span>{/if}
			</div>
		</div>
		<div class="header-actions">
			<button class="btn ghost small" onclick={copyMarkdown}>Copy markdown</button>
			{#if !readonly}
				{#if !doc.threadSlug}<button class="btn small" onclick={openDrafting}>Start drafting…</button>{/if}
				<button class="btn ghost small danger" onclick={remove}>Delete</button>
				<button class="btn primary" onclick={() => (publishing = true)}>Publish version…</button>
			{/if}
		</div>
	</div>

	<div class="subline" style="padding-bottom:12px">
		<span>Versions:</span>
		<div class="version-strip">
			<a class="vchip" class:on={!readonly} href="/documents/{doc.id}" class:hidden={doc.deleted}>Draft</a>
			{#each doc.versions as v (v)}
				<a class="vchip" class:on={readonly && doc.version === v} href="/documents/{doc.id}?v={v}">v{v}</a>
			{:else}
				<span class="hint">none published yet</span>
			{/each}
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner wide">
		{#if readonly}
			<div class="notice info">
				Published version {doc.version}, {dateTime(doc.updatedAt)} — read-only.
				{#if doc.deleted}
					The draft has since been deleted.
				{:else}
					<a href="/documents/{doc.id}">Back to the draft</a>
				{/if}
			</div>
			<div class="doc-view"><Markdown html={doc.html} /></div>
		{:else}
			<div class="form-row" style="justify-content:space-between">
				<div class="seg" role="group" aria-label="Editor view">
					<button class:on={mode === 'write'} onclick={() => setMode('write')}>Write</button>
					<button class:on={mode === 'split'} onclick={() => setMode('split')}>Split</button>
					<button class:on={mode === 'preview'} onclick={() => setMode('preview')}>Preview</button>
				</div>
				<span class="hint">Last changed {date(doc.updatedAt)}. Changes save automatically.</span>
			</div>
			<div class="editor-split" class:both={mode === 'split'}>
				{#if mode !== 'preview'}
					<textarea
						class="doc-editor"
						bind:value={body}
						oninput={edited}
						spellcheck="true"
						placeholder="Write the document in markdown…"
					></textarea>
				{/if}
				{#if mode !== 'write'}
					<div class="doc-view">
						{#if body.trim()}<Markdown {html} />{:else}<span class="hint">Nothing to preview yet.</span>{/if}
					</div>
				{/if}
			</div>
		{/if}
	</div>
</div>

{#if publishing}
	<Modal title="Publish a version" onclose={() => (publishing = false)}>
		<p style="margin:0;color:var(--muted)">
			Publishing freezes the current draft as an immutable version{doc.threadSlug ? ' and leaves a note in the thread' : ''}.
			{doc.versions.length ? `Latest so far: v${doc.versions.at(-1)}.` : 'This is the first version.'}
		</p>
		<label class="check"><input type="radio" bind:group={bump} value="minor" /> Minor — v{preview.minor}</label>
		<label class="check"><input type="radio" bind:group={bump} value="major" /> Major — v{preview.major}</label>
		<label class="check">
			<input type="radio" bind:group={bump} value="custom" /> Custom
			<input
				bind:value={custom}
				placeholder="e.g. 1.4"
				style="width:100px;padding:5px 8px;border-radius:8px;border:1px solid var(--line);background:var(--bg)"
				onfocus={() => (bump = 'custom')}
			/>
		</label>
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (publishing = false)}>Cancel</button>
			<button class="btn primary" onclick={publish} disabled={publishBusy || (bump === 'custom' && !custom.trim())}>Publish</button>
		{/snippet}
	</Modal>
{/if}

{#if drafting}
	<Modal title="Start drafting" onclose={() => (drafting = false)}>
		<p style="margin:0;color:var(--muted)">
			Pick the thread where this document's guiding questions get answered. They're added to it as posts, once.
		</p>
		<label class="check">
			<input type="radio" bind:group={target} value="existing" disabled={data.threads.length === 0} /> Existing thread
		</label>
		{#if target === 'existing'}
			<select bind:value={existing}>
				{#each data.threads as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
			</select>
		{/if}
		<label class="check"><input type="radio" bind:group={target} value="new" /> New thread</label>
		{#if target === 'new'}
			<input bind:value={newTitle} placeholder="Thread title" style="padding:8px 10px;border-radius:8px;border:1px solid var(--line);background:var(--bg)" />
		{/if}
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (drafting = false)}>Cancel</button>
			<button
				class="btn primary"
				onclick={startDrafting}
				disabled={draftBusy || (target === 'existing' ? !existing : !newTitle.trim())}
			>
				Start drafting
			</button>
		{/snippet}
	</Modal>
{/if}

<style>
	.hidden {
		display: none;
	}
</style>
