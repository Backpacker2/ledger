<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import FeedbackCard from '$lib/components/FeedbackCard.svelte';
	import FileImage from '$lib/components/FileImage.svelte';
	import Markdown from '$lib/components/Markdown.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { api } from '$lib/api';
	import { deliverableFile, deliverableUrl, formatSize, nextVersion, snapshotPath, threadUrl } from '$lib/actions';
	import { ago, dateTime } from '$lib/format';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Bump } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const doc = $derived(data.deliverable);
	const project = $derived(data.project.slug);
	const threads = $derived(data.projects.find((p) => p.slug === project)?.threads ?? []);
	const snapshot = $derived(doc.version !== null);
	const isMarkdown = $derived(doc.kind === 'markdown');
	const file = $derived(deliverableFile(doc.path));

	// ---- the editable copy (markdown) ----------------------------------------------------------
	// The page keeps its own text while you type and saves it in the background. It is only reset when a
	// different file is opened — or when the file changed elsewhere while nothing here was waiting to be saved.

	let body = $state('');
	let html = $state('');
	let loadedKey = '';
	let loadedPath = '';
	let dirty = false;
	let lastSaved = 0;
	let saveState = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | undefined;
	let renderTimer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		const key = doc.path;
		const theirs = doc.body ?? '';
		const theirHtml = doc.html ?? '';
		untrack(() => {
			if (key !== loadedKey) {
				if (dirty) void flush(); // still the previous file's path and text at this point
				loadedKey = key;
				loadedPath = key;
				body = theirs;
				html = theirHtml;
				saveState = 'saved';
			} else if (!dirty && saveState === 'saved' && Date.now() - lastSaved > 1500 && theirs !== body) {
				body = theirs;
				html = theirHtml;
			}
		});
	});

	async function flush() {
		clearTimeout(saveTimer);
		if (!dirty) return;
		dirty = false;
		saveState = 'saving';
		const saved = await attempt(() => api.saveDeliverable(project, loadedPath, body));
		lastSaved = Date.now();
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
			api.publishDeliverable(project, doc.path, bump === 'custom' ? 'minor' : bump, bump === 'custom' ? custom : null)
		);
		publishBusy = false;
		if (!published) return;
		publishing = false;
		toast(`Published version ${published.versions.at(-1) ?? ''}`.trim());
		await refresh();
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Delete “${doc.name}”?`,
			message: doc.versions.length
				? 'The live file is deleted. Its published versions are kept and stay readable.'
				: 'The file is deleted from the project.',
			confirmLabel: 'Delete',
			danger: true
		});
		if (!ok) return;
		dirty = false;
		if ((await attempt(() => api.deleteDeliverable(project, doc.path))) === undefined) return;
		await refresh();
		await goto(`/projects/${project}/deliverables`);
	}

	async function copyMarkdown() {
		try {
			await navigator.clipboard.writeText(snapshot ? (doc.body ?? '') : body);
			toast('Markdown copied');
		} catch {
			toast('Could not reach the clipboard', true);
		}
	}

	const open = (reveal: boolean) => attempt(() => api.openProjectFile(project, file, reveal));

	// ---- feedback ------------------------------------------------------------------------------

	let feedbackThread = $state('');
	let feedbackText = $state('');
	let feedbackBusy = $state(false);

	// Feedback is written in a thread: start from the one that asked for this file, else the first.
	$effect(() => {
		const wanted = doc.prompts[0]?.threadSlug;
		const options = threads.map((t) => t.slug);
		if (!options.includes(feedbackThread)) feedbackThread = wanted && options.includes(wanted) ? wanted : (options[0] ?? '');
	});

	async function addFeedback() {
		feedbackBusy = true;
		const added = await attempt(() => api.addFeedback(project, feedbackThread, doc.path, feedbackText));
		feedbackBusy = false;
		if (!added) return;
		feedbackText = '';
		await refresh();
	}

	const openCount = $derived(doc.feedback.filter((f) => f.status === 'open').length);
</script>

<svelte:head><title>{doc.name} · Ledger</title></svelte:head>

<div class="header-wrap">
	<div class="crumbs">
		<a href="/projects/{project}">{data.project.name}</a><span>/</span><a href="/projects/{project}/deliverables">Deliverables</a>
	</div>
	<div class="header-top">
		<div style="flex:1;min-width:0">
			<h1 title={doc.path}>{doc.name}</h1>
			<div class="subline">
				<span class="badge type">{doc.kind === 'markdown' ? 'Markdown' : doc.kind === 'image' ? 'Image' : 'File'}</span>
				{#if snapshot}
					<span class="badge published">v{doc.version}</span>
				{:else}
					<span class="badge {doc.versions.length ? 'published' : 'draft'}">
						{doc.versions.length ? `Draft after v${doc.versions.at(-1)}` : 'Draft'}
					</span>
				{/if}
				<span>{formatSize(doc.size)}</span>
				{#if doc.path.includes('/')}<span class="mono">{doc.path}</span>{/if}
				{#if isMarkdown && !snapshot && doc.exists}
					<span class="save-state">{saveState === 'saved' ? 'Saved' : saveState === 'saving' ? 'Saving…' : 'Unsaved changes'}</span>
				{/if}
			</div>
		</div>
		<div class="header-actions">
			{#if isMarkdown}<button class="btn ghost small" onclick={copyMarkdown}>Copy markdown</button>{/if}
			{#if doc.exists}
				<button class="btn ghost small" onclick={() => open(false)}>Open</button>
				<button class="btn ghost small" onclick={() => open(true)}>Show in folder</button>
			{/if}
			{#if !snapshot && doc.exists}
				<button class="btn ghost small danger" onclick={remove}>Delete</button>
				<button class="btn primary" onclick={() => (publishing = true)}>Publish version…</button>
			{/if}
		</div>
	</div>

	<div class="subline" style="padding-bottom:12px">
		<span>Versions:</span>
		<div class="version-strip">
			{#if doc.basePath}
				{#if doc.exists || !snapshot}
					<a class="vchip" class:on={!snapshot} href={deliverableUrl(project, doc.basePath)}>Live</a>
				{/if}
			{/if}
			{#each doc.versions as v (v)}
				<a class="vchip" class:on={doc.version === v} href={deliverableUrl(project, snapshotPath(doc.basePath, v))}>v{v}</a>
			{:else}
				<span class="hint">none published yet</span>
			{/each}
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner wide">
		{#if snapshot}
			<div class="notice info">
				Published version {doc.version}, {dateTime(doc.modified)} — read-only.
				{#if doc.exists}
					<a href={deliverableUrl(project, doc.basePath)}>Back to the live file</a>
				{/if}
			</div>
		{/if}

		{#if !doc.exists && !snapshot}
			<div class="notice">The file is not in the project’s deliverables folder (any more).</div>
		{:else if isMarkdown && snapshot}
			<div class="doc-view"><Markdown html={doc.html ?? ''} /></div>
		{:else if isMarkdown}
			<div class="form-row" style="justify-content:space-between">
				<div class="seg" role="group" aria-label="Editor view">
					<button class:on={mode === 'write'} onclick={() => setMode('write')}>Write</button>
					<button class:on={mode === 'split'} onclick={() => setMode('split')}>Split</button>
					<button class:on={mode === 'preview'} onclick={() => setMode('preview')}>Preview</button>
				</div>
				<span class="hint">Last changed {ago(doc.modified)}. Changes save automatically.</span>
			</div>
			<div class="editor-split" class:both={mode === 'split'}>
				{#if mode !== 'preview'}
					<textarea
						class="doc-editor"
						bind:value={body}
						oninput={edited}
						spellcheck="true"
						placeholder="Write it in markdown…"
					></textarea>
				{/if}
				{#if mode !== 'write'}
					<div class="doc-view">
						{#if body.trim()}<Markdown {html} />{:else}<span class="hint">Nothing to preview yet.</span>{/if}
					</div>
				{/if}
			</div>
		{:else if doc.kind === 'image'}
			<div class="doc-view" style="display:grid;place-items:center">
				<FileImage {project} path={file} alt={doc.name} cls="shot-preview" />
			</div>
		{:else}
			<div class="panel">
				<h2>{doc.name}</h2>
				<p class="sub" style="margin:0">
					This kind of file can’t be shown here. Open it with the program your system uses for it — it opens the file in the vault, so changes you
					save there are picked up when you come back.
				</p>
				<div class="row-end" style="justify-content:flex-start">
					<button class="btn primary" onclick={() => open(false)}>Open</button>
					<button class="btn" onclick={() => open(true)}>Show in folder</button>
				</div>
			</div>
		{/if}

		<div class="cols-2">
			<section class="panel">
				<div class="header-top">
					<h2>Feedback{doc.feedback.length ? ` · ${openCount} open` : ''}</h2>
				</div>
				<div class="field">
					<span>
						{#if snapshot}On version {doc.version}{:else}On this file{/if}, written in a thread
					</span>
					{#if threads.length === 0}
						<span class="hint">Create a thread first — feedback is written in one.</span>
					{:else}
						<div class="form-row" style="flex-wrap:nowrap">
							<select bind:value={feedbackThread} aria-label="Thread" style="max-width:220px">
								{#each threads as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
							</select>
						</div>
						<textarea
							class="edit-area"
							bind:value={feedbackText}
							rows="3"
							placeholder="What works, what doesn’t, what to change…"
							onkeydown={(e) => {
								if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && feedbackText.trim()) addFeedback();
							}}
						></textarea>
						<div class="row-end">
							<button class="btn small primary" onclick={addFeedback} disabled={feedbackBusy || !feedbackText.trim() || !feedbackThread}>
								Add feedback
							</button>
						</div>
					{/if}
				</div>
				<div style="display:flex;flex-direction:column;gap:10px">
					{#each doc.feedback as f (f.id)}
						<FeedbackCard {project} thread={f.threadSlug} feedback={f} onDeliverable />
					{/each}
				</div>
			</section>

			<aside class="panel">
				<h2>Produced by</h2>
				<div class="mini-list">
					{#each doc.prompts as p (p.threadSlug + p.id)}
						<a href="{threadUrl(project, p.threadSlug)}?item=pr-{p.id}">
							<span class="t">{p.title}</span>
							<span class="s">{p.threadTitle}</span>
						</a>
					{:else}
						<span class="hint">No prompt asked for this file.</span>
					{/each}
				</div>
			</aside>
		</div>
	</div>
</div>

{#if publishing}
	<Modal title="Publish a version" onclose={() => (publishing = false)}>
		<p style="margin:0;color:var(--muted)">
			Publishing freezes the current file as an immutable version (<code>{snapshotPath(doc.path, bump === 'custom' ? custom || 'x.y' : preview[bump])}</code>)
			and leaves a note in the thread of the prompt that asked for it.
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
