<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { attempt, refresh, toast } from '$lib/ui.svelte';
	import type { Prompt, Template } from '$lib/types';
	import Modal from './Modal.svelte';

	/** `prompt` is null when writing a new one. The modal is mounted fresh each time, so it only reads the initial values. */
	let { project, thread, prompt, onclose }: { project: string; thread: string; prompt: Prompt | null; onclose: () => void } = $props();

	function start() {
		return { title: prompt?.title ?? '', body: prompt?.body ?? '' };
	}
	const initial = start();

	let title = $state(initial.title);
	let body = $state(initial.body);
	let templateId = $state('');
	let templates = $state<Template[]>([]);
	let saving = $state(false);

	const template = $derived(templates.find((t) => t.id === templateId));

	onMount(async () => {
		if (!prompt) templates = (await attempt(() => api.effectiveTemplates(project))) ?? [];
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
			prompt ? api.updatePrompt(project, thread, prompt.id, title, body) : api.createPrompt(project, thread, title, body, templateId || null)
		);
		saving = false;
		if (!saved) return;
		if (!prompt && template?.questions.length) toast(`Added ${template.questions.length} guiding question${template.questions.length === 1 ? '' : 's'} to the thread`);
		await refresh();
		onclose();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title={prompt ? 'Edit prompt' : 'New prompt'} wide {onclose}>
	<p class="hint" style="margin:0">A prompt asks for a deliverable. Whatever produces it — you, an assistant — lands the file in the project’s deliverables.</p>

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
	<label class="field">
		What do you want?
		<textarea bind:value={body} rows="8" placeholder="Describe the deliverable: audience, scope, what good looks like. Markdown works."></textarea>
	</label>

	{#snippet footer()}
		<span class="spacer-h"></span>
		<span class="hint">Ctrl+Enter saves</span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !title.trim()}>{prompt ? 'Save' : 'Create prompt'}</button>
	{/snippet}
</Modal>
