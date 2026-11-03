<script lang="ts">
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import Plus from '@lucide/svelte/icons/plus';
	import X from '@lucide/svelte/icons/x';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Template } from '$lib/types';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/** `template` is null when creating one. The modal is mounted fresh each time, so it only reads the initial values. */
	let { scope, template, onclose }: { scope: string; template: Template | null; onclose: () => void } = $props();

	function start() {
		return {
			name: template?.name ?? '',
			trigger: template?.trigger ?? '',
			questions: template ? [...template.questions] : [''],
			body: template?.body ?? ''
		};
	}
	const initial = start();

	let name = $state(initial.name);
	let trigger = $state(initial.trigger);
	let questions = $state<string[]>(initial.questions);
	let body = $state(initial.body);
	let saving = $state(false);

	function move(i: number, by: -1 | 1) {
		const j = i + by;
		if (j < 0 || j >= questions.length) return;
		[questions[i], questions[j]] = [questions[j], questions[i]];
	}

	async function save() {
		saving = true;
		const saved = await attempt(() =>
			template
				? api.updateTemplate(scope, template.id, name, trigger, questions, body)
				: api.createTemplate(scope, name, trigger, questions, body)
		);
		saving = false;
		if (!saved) return;
		toast(template ? 'Template saved' : 'Template created');
		await refresh();
		onclose();
	}

	async function remove() {
		if (!template) return;
		const ok = await confirmDialog({
			title: `Delete “${template.name}”?`,
			message: `Prompts already started from it keep the guiding questions they were given.${scope ? ' If a shared template has the same name, this project uses that one again.' : ''}`,
			confirmLabel: 'Delete template',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteTemplate(scope, template.id))) !== undefined) {
			await refresh();
			onclose();
		}
	}
</script>

<Modal title={template ? `Edit ${template.name}` : scope ? 'New project template' : 'New template'} wide {onclose}>
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
					<MarkdownEditor bind:value={questions[i]} rows={2} label="Guiding question {i + 1}" placeholder="A question to answer in the thread first" />
					<div class="btns">
						<button class="icon-btn" style="color:var(--muted)" aria-label="Move up" disabled={i === 0} onclick={() => move(i, -1)}><ArrowUp size={16} /></button>
						<button class="icon-btn" style="color:var(--muted)" aria-label="Move down" disabled={i === questions.length - 1} onclick={() => move(i, 1)}><ArrowDown size={16} /></button>
						<button class="icon-btn" style="color:var(--muted)" aria-label="Remove question" onclick={() => questions.splice(i, 1)}><X size={16} /></button>
					</div>
				</div>
			{/each}
		</div>
		<div><button class="btn small ghost" onclick={() => questions.push('')}><Plus size={14} />Add question</button></div>
	</div>
	<div class="field">
		Deliverable skeleton
		<MarkdownEditor
			bind:value={body}
			variant="doc"
			minHeight="160px"
			label="Deliverable skeleton"
			placeholder={'# Title\n\n## Section\n…'}
			spellcheck={false}
		/>
		<span class="hint">New deliverables created for a prompt from this template start with this text.</span>
	</div>

	{#snippet footer()}
		{#if template}
			<button class="btn ghost danger" onclick={remove}>Delete</button>
		{/if}
		<span class="spacer-h"></span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !name.trim()}>Save</button>
	{/snippet}
</Modal>
