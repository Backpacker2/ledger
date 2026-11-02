<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Column, Criterion, Story, ThreadSummary } from '$lib/types';
	import Modal from './Modal.svelte';

	/** `story` is null when creating one. The modal is mounted fresh each time, so it only reads the initial values. */
	let {
		story,
		threads,
		initialThreadSlugs = [],
		onclose
	}: { story: Story | null; threads: ThreadSummary[]; initialThreadSlugs?: string[]; onclose: () => void } = $props();

	/** The form's starting values, read once; edits stay local until Save. */
	function start() {
		return {
			id: story?.id ?? '',
			summary: story?.summary ?? '',
			role: story?.role ?? '',
			want: story?.want ?? '',
			reason: story?.reason ?? '',
			criteria: story ? story.criteria.map((c) => ({ ...c })) : [],
			columnId: story?.columnId ?? '',
			threadSlugs: story ? story.threads.map((t) => t.slug) : [...initialThreadSlugs]
		};
	}
	const initial = start();

	let id = $state(initial.id);
	let summary = $state(initial.summary);
	let role = $state(initial.role);
	let want = $state(initial.want);
	let reason = $state(initial.reason);
	let criteria = $state<Criterion[]>(initial.criteria);
	let columnId = $state(initial.columnId);
	let threadSlugs = $state<string[]>(initial.threadSlugs);
	let newCriterion = $state('');
	let columns = $state<Column[]>([]);
	let saving = $state(false);

	const titleOf = (slug: string) => threads.find((t) => t.slug === slug)?.title ?? slug;
	const unlinked = $derived(threads.filter((t) => !threadSlugs.includes(t.slug)));
	const doneCount = $derived(criteria.filter((c) => c.done).length);

	onMount(async () => {
		if (!story) return;
		columns = (await attempt(() => api.getPlanboard()))?.columns ?? [];
	});

	function addCriterion() {
		const text = newCriterion.trim();
		if (!text) return;
		criteria.push({ text, done: false });
		newCriterion = '';
	}

	async function save() {
		addCriterion();
		saving = true;
		const saved = await attempt(async () => {
			if (!story) {
				return api.createUserStory({ id, summary, role, want, reason, criteria, threadSlugs });
			}
			await api.updateUserStory(story.id, { summary, role, want, reason, criteria, columnId });
			const before = story.threads.map((t) => t.slug);
			for (const slug of threadSlugs.filter((s) => !before.includes(s))) await api.linkStoryThread(story.id, slug);
			for (const slug of before.filter((s) => !threadSlugs.includes(s))) await api.unlinkStoryThread(story.id, slug);
			return story;
		});
		saving = false;
		if (!saved) return;
		toast(story ? `Saved ${saved.id}` : `Created ${saved.id}`);
		await refresh();
		onclose();
	}

	async function remove() {
		if (!story) return;
		const ok = await confirmDialog({
			title: `Delete ${story.id}?`,
			message: 'Its threads and documents are left alone.',
			confirmLabel: 'Delete',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteUserStory(story.id))) !== undefined) {
			await refresh();
			onclose();
		}
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title={story ? `User story ${story.id}` : 'New user story'} wide {onclose}>
	<div class="form-row">
		<label class="field" style="width:150px">
			ID
			<input bind:value={id} placeholder="MM01" disabled={!!story} maxlength="40" spellcheck="false" />
		</label>
		<label class="field grow">
			Summary
			<input bind:value={summary} placeholder="What is this story about?" />
		</label>
	</div>

	<div class="form-row">
		<label class="field grow">As a…<input bind:value={role} placeholder="role" /></label>
		<label class="field grow">I want to…<input bind:value={want} placeholder="goal" /></label>
		<label class="field grow">Such that…<input bind:value={reason} placeholder="reason" /></label>
	</div>

	<div class="field">
		<span>Acceptance criteria{criteria.length ? ` · ${doneCount}/${criteria.length} met` : ''}</span>
		<div class="q-list">
			{#each criteria as c, i (i)}
				<div class="crit-row" class:done={c.done}>
					<input type="checkbox" bind:checked={c.done} aria-label="Met" />
					<input type="text" bind:value={c.text} />
					<button class="icon-btn" style="color:var(--muted)" aria-label="Remove criterion" onclick={() => criteria.splice(i, 1)}>✕</button>
				</div>
			{/each}
			<div class="crit-row">
				<input
					type="text"
					bind:value={newCriterion}
					placeholder="Add a criterion and press Enter"
					onkeydown={(e) => {
						if (e.key === 'Enter' && !e.isComposing) {
							e.preventDefault();
							e.stopPropagation();
							addCriterion();
						}
					}}
				/>
			</div>
		</div>
	</div>

	<div class="field">
		<span>Threads</span>
		<div class="chips">
			{#each threadSlugs as slug (slug)}
				<span class="chip">{titleOf(slug)}<button class="x" aria-label="Unlink {titleOf(slug)}" onclick={() => (threadSlugs = threadSlugs.filter((s) => s !== slug))}>✕</button></span>
			{/each}
			{#if unlinked.length}
				<select
					style="width:auto;min-width:0;padding:4px 8px;font-size:12px"
					aria-label="Link a thread"
					onchange={(e) => {
						const slug = e.currentTarget.value;
						if (slug) threadSlugs.push(slug);
						e.currentTarget.value = '';
					}}
				>
					<option value="">+ Link a thread…</option>
					{#each unlinked as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
				</select>
			{:else if !threadSlugs.length}
				<span class="hint">No threads yet</span>
			{/if}
		</div>
	</div>

	{#if story && columns.length}
		<label class="field" style="width:220px">
			Column
			<select bind:value={columnId}>
				{#each columns as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
			</select>
		</label>
	{/if}

	{#snippet footer()}
		{#if story}<button class="btn danger ghost" onclick={remove}>Delete</button>{/if}
		<span class="spacer-h"></span>
		<span class="hint">Ctrl+Enter saves</span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !summary.trim() || (!story && !id.trim())}>
			{story ? 'Save' : 'Create'}
		</button>
	{/snippet}
</Modal>
