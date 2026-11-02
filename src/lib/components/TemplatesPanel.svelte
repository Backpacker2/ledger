<script lang="ts">
	import { api } from '$lib/api';
	import { attempt, refresh, toast } from '$lib/ui.svelte';
	import type { Template } from '$lib/types';
	import TemplateModal from './TemplateModal.svelte';

	/**
	 * Lists templates and edits them. `scope` is "" for the vault-wide ones, or a project's slug: its own templates
	 * come first, then the vault-wide ones it inherits (each can be copied into the project to change it there).
	 */
	let { scope, templates }: { scope: string; templates: Template[] } = $props();

	/** `null` is closed; `'new'` creates; a template edits it. */
	let editing = $state<Template | 'new' | null>(null);

	const own = $derived(templates.filter((t) => t.scope === scope));
	const inherited = $derived(scope ? templates.filter((t) => t.scope === '') : []);

	async function override(t: Template) {
		const copy = await attempt(() => api.overrideTemplate(scope, t.id));
		if (!copy) return;
		toast(`${copy.name} is now this project's own`);
		await refresh();
		editing = copy;
	}

	const summary = (t: Template) =>
		`${t.trigger ? `Use when ${t.trigger} · ` : ''}${t.questions.length} question${t.questions.length === 1 ? '' : 's'}`;
</script>

<div class="panel">
	<div class="header-top">
		<h2>{scope ? 'This project’s templates' : 'Templates'}</h2>
		<button class="btn primary small" onclick={() => (editing = 'new')}>New template</button>
	</div>
	{#if own.length === 0}
		<div class="hint">
			{scope
				? 'None yet. Prompts in this project can still start from the shared templates below.'
				: 'None yet. A template says when to ask for a deliverable, which guiding questions to answer first, and what the deliverable looks like.'}
		</div>
	{:else}
		<div class="tpl-list">
			{#each own as t (t.id)}
				<div class="tpl-row">
					<div class="name">
						<b>{t.name}</b>
						<span>{summary(t)}</span>
					</div>
					<button class="btn small" onclick={() => (editing = t)}>Edit</button>
				</div>
			{/each}
		</div>
	{/if}
</div>

{#if scope}
	<div class="panel">
		<h2>Shared with every project</h2>
		{#if inherited.length === 0}
			<div class="hint">There are no shared templates. Add some under Templates in the sidebar.</div>
		{:else}
			<div class="tpl-list">
				{#each inherited as t (t.id)}
					<div class="tpl-row">
						<div class="name">
							<b>{t.name}</b>
							<span>{summary(t)}</span>
						</div>
						{#if t.overridden}
							<span class="badge accent">replaced here</span>
						{:else}
							<button class="btn small" onclick={() => override(t)} title="Copy it into this project to change it here">Customise</button>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</div>
{/if}

{#if editing}
	<TemplateModal {scope} template={editing === 'new' ? null : editing} onclose={() => (editing = null)} />
{/if}
