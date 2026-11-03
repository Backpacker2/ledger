<script lang="ts">
	import { onMount } from 'svelte';
	import { api, claudeCodeApi } from '$lib/api';
	import { autoRunNewPrompt } from '$lib/claudeRuns.svelte';
	import { attempt, refresh, toast } from '$lib/ui.svelte';
	import type { DeliverableSummary, Prompt, PromptDraft, Template } from '$lib/types';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/**
	 * `prompt` is null when writing a new one, which may start from a `draft`; `onsaved` hears about the new prompt.
	 * The modal is mounted fresh each time, so it only reads the initial values.
	 */
	let {
		project,
		thread,
		prompt,
		draft = null,
		onsaved,
		onclose
	}: { project: string; thread: string; prompt: Prompt | null; draft?: PromptDraft | null; onsaved?: (saved: Prompt) => void; onclose: () => void } = $props();

	function start() {
		return { title: prompt?.title ?? draft?.title ?? '', body: prompt?.body ?? draft?.body ?? '' };
	}
	const initial = start();

	let title = $state(initial.title);
	let body = $state(initial.body);
	let templateId = $state('');
	let templates = $state<Template[]>([]);
	let saving = $state(false);
	// Whether saving a new prompt starts it in Claude Code by itself (a setting).
	let autoRun = $state(false);

	// What the prompt asks for: a new deliverable, or changes to one that is already in the project.
	let kind = $state<'new' | 'existing'>('new');
	let existing = $state<DeliverableSummary[]>([]);
	let chosen = $state('');

	const template = $derived(templates.find((t) => t.id === templateId));

	onMount(async () => {
		if (prompt) return;
		try {
			autoRun = (await claudeCodeApi.settings()).autoRun;
		} catch {
			// The line about it is only a courtesy.
		}
		templates = (await attempt(() => api.effectiveTemplates(project))) ?? [];
		existing = ((await attempt(() => api.listDeliverables(project))) ?? []).filter((d) => d.exists);
		// A draft for a document that is still there starts out asking for changes to it.
		const wanted = existing.find((d) => d.path === draft?.deliverable);
		chosen = wanted?.path ?? existing[0]?.path ?? '';
		if (wanted) kind = 'existing';
	});

	function pick(id: string) {
		templateId = id;
		const next = templates.find((t) => t.id === id);
		// A template names the work, so offer its name until the person has typed a title of their own.
		if (next && !title.trim()) title = next.name;
	}

	async function save() {
		saving = true;
		const saved = await attempt(() =>
			prompt
				? api.updatePrompt(project, thread, prompt.id, title, body)
				: api.createPrompt(project, thread, title, body, templateId || null, kind === 'existing' ? chosen : null)
		);
		saving = false;
		if (!saved) return;
		if (!prompt && template?.questions.length) toast(`Added ${template.questions.length} guiding question${template.questions.length === 1 ? '' : 's'} to the thread`);
		await refresh();
		// Not awaited: the prompt is made, and the run reports for itself.
		if (!prompt) void autoRunNewPrompt(project, thread, saved);
		onsaved?.(saved);
		onclose();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title={prompt ? 'Edit prompt' : 'New prompt'} wide {onclose}>
	<p class="hint" style="margin:0">A prompt asks for a deliverable. Whatever produces it — you, an assistant — lands the file in the project’s deliverables.</p>
	{#if !prompt && draft?.note}<div class="notice info">{draft.note}</div>{/if}

	{#if !prompt && templates.length}
		<label class="field">
			Start from a template
			<select value={templateId} onchange={(e) => pick(e.currentTarget.value)}>
				<option value="">No template</option>
				{#each templates as t (t.scope + '/' + t.id)}
					<option value={t.id}>{t.name}{t.scope ? '' : ' (all projects)'}</option>
				{/each}
			</select>
		</label>
		{#if template}
			<div class="notice info">
				{#if template.trigger}<b>Use when:</b> {template.trigger}<br />{/if}
				{#if template.questions.length}
					{template.questions.length} guiding question{template.questions.length === 1 ? '' : 's'} will be added to this thread to answer first.
				{:else}
					This template has no guiding questions.
				{/if}
			</div>
		{/if}
	{/if}

	<label class="field">
		Title
		<input bind:value={title} placeholder="e.g. Write the pricing brief" maxlength="200" />
	</label>
	<div class="field">
		What do you want?
		<MarkdownEditor
			bind:value={body}
			rows={8}
			label="What do you want?"
			placeholder="Describe the deliverable: audience, scope, what good looks like. Markdown works."
		/>
	</div>

	{#if !prompt}
		<div class="field">
			Deliverable
			<label class="check">
				<input type="radio" bind:group={kind} value="new" />
				A new deliverable: the prompt asks for a file that doesn’t exist yet
			</label>
			<label class="check">
				<input type="radio" bind:group={kind} value="existing" disabled={!existing.length} />
				An existing deliverable: the prompt asks for changes to it{existing.length ? '' : ' (there are none in this project yet)'}
			</label>
			{#if kind === 'existing'}
				<select bind:value={chosen} aria-label="The existing deliverable">
					{#each existing as d (d.path)}
						<option value={d.path}>{d.path}{d.versions.length ? ` · latest version v${d.versions.at(-1)}` : ' · not published yet'}</option>
					{/each}
				</select>
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<span class="spacer-h"></span>
		{#if !prompt && autoRun}<span class="hint">Claude Code starts on it as soon as it is created</span>{/if}
		<span class="hint">Ctrl+Enter saves</span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !title.trim() || (kind === 'existing' && !chosen)}>{prompt ? 'Save' : 'Create prompt'}</button>
	{/snippet}
</Modal>
