<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import CodeXml from '@lucide/svelte/icons/code-xml';
	import PencilLine from '@lucide/svelte/icons/pencil-line';
	import { goto } from '$app/navigation';
	import FeedbackCard from '$lib/components/FeedbackCard.svelte';
	import FileImage from '$lib/components/FileImage.svelte';
	import Markdown from '$lib/components/Markdown.svelte';
	import MarkdownEditor from '$lib/components/MarkdownEditor.svelte';
	import MmmBadge from '$lib/components/MmmBadge.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import ProvenancePanel from '$lib/components/ProvenancePanel.svelte';
	import { api } from '$lib/api';
	import { deliverableFile, deliverableUrl, formatSize, nextVersion, snapshotPath } from '$lib/actions';
	import { ago, dateTime } from '$lib/format';
	import { MMM } from '$lib/mmm';
	import { orderFeedback, orderVersions } from '$lib/order';
	import { listOrder } from '$lib/order.svelte';
	import { dirOf } from '$lib/vaultImages';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import { beforeRestart } from '$lib/updates.svelte';
	import type { Bump, MmmLevel } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const doc = $derived(data.deliverable);
	const provenance = $derived(data.provenance);
	const project = $derived(data.project.slug);
	const threads = $derived(data.projects.find((p) => p.slug === project)?.threads ?? []);
	const snapshot = $derived(doc.version !== null);
	const isMarkdown = $derived(doc.kind === 'markdown');
	const file = $derived(deliverableFile(doc.path));
	// Pictures in it are looked for beside the file.
	const images = $derived({ project, dir: dirOf(file) });

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

	// Installing an update restarts Ledger: what is typed and still waiting for the autosave is saved first.
	$effect(() => beforeRestart(flush));

	// ---- view mode -----------------------------------------------------------------------------

	// As in Obsidian: the raw markdown, the live preview (marks hidden away from the cursor), or the rendered page.
	type Mode = 'source' | 'live' | 'reading';
	const modes: { id: Mode; label: string; icon: typeof BookOpen }[] = [
		{ id: 'source', label: 'Source', icon: CodeXml },
		{ id: 'live', label: 'Live preview', icon: PencilLine },
		{ id: 'reading', label: 'Reading', icon: BookOpen }
	];
	let mode = $state<Mode>('live');
	try {
		// The modes before these: Write and Split were editors, Preview was reading.
		const saved = localStorage.getItem('ledger-doc-mode');
		if (saved === 'source' || saved === 'live' || saved === 'reading') mode = saved;
		else if (saved === 'preview') mode = 'reading';
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

	// ---- the "Produced by" side window -----------------------------------------------------------
	// Open by default where the window is wide enough to hold it next to the page (below that it is a drawer over it).

	let sideOpen = $state(true);
	try {
		const saved = localStorage.getItem('ledger-doc-provenance');
		sideOpen = saved === 'open' || saved === 'closed' ? saved === 'open' : window.innerWidth >= 1100;
	} catch {
		// Storage unavailable: open.
	}
	function setSide(open: boolean) {
		sideOpen = open;
		try {
			localStorage.setItem('ledger-doc-provenance', open ? 'open' : 'closed');
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

	// What the person says about the change. Claude Code runs started here are recorded as they happen; the box is for AI
	// used anywhere else, and starts ticked when a run is known to have changed this file without being published.
	let thread = $state('');
	let aiMade = $state(false);
	let level = $state<MmmLevel | ''>('');
	let promptId = $state('');
	let changeNote = $state('');
	const stopped = $derived(provenance.stoppedRun);
	const manMadeConflict = $derived(level === 'man-made' && (aiMade || provenance.historyHasAi));
	const suggestManMade = $derived(!aiMade && !provenance.historyHasAi && !level);

	function startPublishing() {
		bump = 'minor';
		custom = '';
		thread = provenance.defaultThread ?? '';
		aiMade = stopped !== null;
		level = provenance.suggestedLabel ?? '';
		promptId = stopped?.promptId ?? '';
		changeNote = '';
		publishing = true;
	}

	async function publish() {
		publishBusy = true;
		await flush();
		const published = await attempt(() =>
			api.publishDeliverable(project, doc.path, bump === 'custom' ? 'minor' : bump, bump === 'custom' ? custom : null, {
				thread,
				ai: aiMade,
				mmm: level || null,
				prompt: aiMade ? promptId || null : null,
				// The stopped run only goes on the record when the prompt is the one it ran.
				run: aiMade && stopped && (!promptId || promptId === stopped.promptId) ? stopped.id : null,
				note: changeNote.trim() || null
			})
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
	// Only how the list is read changes (see Settings); `doc.feedback` stays oldest first.
	// "Live" is the newest of all: first when newest reads first, last when oldest does.
	const showLive = $derived(!!doc.basePath && (doc.exists || !snapshot));
	const orderedVersions = $derived(orderVersions(doc.versions, listOrder.order));
	const feedbackList = $derived(orderFeedback(doc.feedback, listOrder.order));
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
				{#if provenance.summary.label}<MmmBadge level={provenance.summary.label.level} />{/if}
				{#if doc.path.includes('/')}<span class="mono">{doc.path}</span>{/if}
				{#if isMarkdown && !snapshot && doc.exists}
					<span class="save-state">{saveState === 'saved' ? 'Saved' : saveState === 'saving' ? 'Saving…' : 'Unsaved changes'}</span>
				{/if}
			</div>
		</div>
		<div class="header-actions">
			<button class="btn ghost small" aria-pressed={sideOpen} onclick={() => setSide(!sideOpen)}>Produced by</button>
			{#if isMarkdown}<button class="btn ghost small" onclick={copyMarkdown}>Copy markdown</button>{/if}
			{#if doc.exists}
				<button class="btn ghost small" onclick={() => open(false)}>Open</button>
				<button class="btn ghost small" onclick={() => open(true)}>Show in folder</button>
			{/if}
			{#if !snapshot && doc.exists}
				<button class="btn ghost small danger" onclick={remove}>Delete</button>
				<button class="btn primary" onclick={startPublishing}>Publish version…</button>
			{/if}
		</div>
	</div>

	<div class="subline" style="padding-bottom:12px">
		<span>Versions:</span>
		<div class="version-strip">
			{#snippet liveChip()}
				<a class="vchip" class:on={!snapshot} href={deliverableUrl(project, doc.basePath)}>Live</a>
			{/snippet}
			{#if showLive && listOrder.order === 'desc'}{@render liveChip()}{/if}
			<!-- `doc.versions` itself stays oldest first, which "latest" relies on; only the chips follow the setting. -->
			{#each orderedVersions as v (v)}
				<a class="vchip" class:on={doc.version === v} href={deliverableUrl(project, snapshotPath(doc.basePath, v))}>v{v}</a>
			{:else}
				<span class="hint">none published yet</span>
			{/each}
			{#if showLive && listOrder.order === 'asc'}{@render liveChip()}{/if}
		</div>
	</div>
</div>

<div class="doc-body">
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
			<div class="doc-view"><Markdown html={doc.html ?? ''} {images} /></div>
		{:else if isMarkdown}
			<div class="form-row" style="justify-content:space-between">
				<div class="seg" role="group" aria-label="View">
					{#each modes as m (m.id)}
						<button class:on={mode === m.id} aria-pressed={mode === m.id} onclick={() => setMode(m.id)}><m.icon size={14} />{m.label}</button>
					{/each}
				</div>
				<span class="hint">Last changed {ago(doc.modified)}. Changes save automatically.</span>
			</div>
			{#if mode === 'reading'}
				<div class="doc-view">
					{#if body.trim()}<Markdown {html} {images} />{:else}<span class="hint">Nothing here yet.</span>{/if}
				</div>
			{:else}
				<MarkdownEditor
					bind:value={body}
					variant="doc"
					minHeight="58vh"
					label={doc.name}
					placeholder="Write it in markdown…"
					source={mode === 'source'}
					{images}
					oninput={edited}
				/>
			{/if}
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

		<div>
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
						<MarkdownEditor
							bind:value={feedbackText}
							label="Feedback"
							placeholder="What works, what doesn’t, what to change…"
							onsubmit={() => feedbackText.trim() && addFeedback()}
						/>
						<div class="row-end">
							<button class="btn small primary" onclick={addFeedback} disabled={feedbackBusy || !feedbackText.trim() || !feedbackThread}>
								Add feedback
							</button>
						</div>
					{/if}
				</div>
				<div style="display:flex;flex-direction:column;gap:10px">
					{#each feedbackList as f (f.id)}
						<FeedbackCard {project} thread={f.threadSlug} feedback={f} onDeliverable />
					{/each}
				</div>
			</section>
		</div>
	</div>
</div>

{#if sideOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="side-scrim open" onclick={() => setSide(false)}></div>
	<aside class="side-window" aria-label="Produced by">
		<ProvenancePanel {project} {doc} {provenance} onclose={() => setSide(false)} />
	</aside>
{/if}
</div>

{#if publishing}
	<Modal title="Publish a version" onclose={() => (publishing = false)}>
		<p style="margin:0;color:var(--muted)">
			Publishing freezes the current file as an immutable version (<code>{snapshotPath(doc.path, bump === 'custom' ? custom || 'x.y' : preview[bump])}</code>),
			records who made the change, and leaves a note in the thread you pick.
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

		<hr class="rule" />
		<label class="field">
			<span>Note in thread</span>
			<select bind:value={thread}>
				<option value="">No thread — just record it</option>
				{#each threads as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
			</select>
		</label>
		<div class="field">
			<label class="check">
				<input type="checkbox" bind:checked={aiMade} /> This change was made with generative AI
			</label>
			<span class="hint">
				{#if stopped}
					Claude Code’s run of “{stopped.promptTitle}” changed this file{stopped.status === 'finished' ? '' : ` and was ${stopped.status}`}, and no version holds
					its changes, so this starts ticked. Untick it only if you rewrote them yourself.
				{:else}
					Leave it empty for what you wrote yourself. Runs started from Ledger are recorded automatically; tick this for AI used elsewhere (a chat, an editor assistant, a paste).
				{/if}
			</span>
		</div>
		{#if aiMade && doc.prompts.length}
			<label class="field">
				<span>Which prompt asked for it? (optional)</span>
				<select bind:value={promptId}>
					<option value="">None of them</option>
					{#each doc.prompts as p (p.id)}<option value={p.id}>{p.threadTitle} · {p.title || 'Untitled prompt'}</option>{/each}
				</select>
			</label>
		{/if}
		<label class="field">
			<span>AI-use label for the result (optional)</span>
			<select bind:value={level}>
				<option value="">Not labelled</option>
				{#each MMM as m (m.level)}<option value={m.level}>{m.name} — {m.tagline}</option>{/each}
			</select>
			{#if level}
				<span class="hint">{MMM.find((m) => m.level === level)?.meaning}</span>
			{/if}
			{#if manMadeConflict}
				<span class="pv-warn">Man-Made means no generative AI, but AI is recorded {aiMade ? 'for this change' : 'in this file’s history'}.</span>
			{:else if suggestManMade}
				<span class="hint">
					Nothing here is recorded as AI. <button class="link accent" onclick={() => (level = 'man-made')}>Label it Man-Made</button> if that is true.
				</span>
			{/if}
		</label>
		<div class="field">
			<span>Changelog (optional)</span>
			<MarkdownEditor bind:value={changeNote} rows={2} label="Changelog" placeholder="What changed, in your words" />
			<span class="hint">Quoted in the note in the thread, and shown with the version in the Produced by window.</span>
		</div>
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (publishing = false)}>Cancel</button>
			<button class="btn primary" onclick={publish} disabled={publishBusy || (bump === 'custom' && !custom.trim())}>Publish</button>
		{/snippet}
	</Modal>
{/if}
