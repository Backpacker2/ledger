<script lang="ts">
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh } from '$lib/ui.svelte';
	import type { Board, Task, ThreadSummary } from '$lib/types';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/** `task` is null when creating one (it lands in the backlog). The modal is mounted fresh each time. */
	let {
		project,
		task,
		board,
		threads,
		onclose
	}: { project: string; task: Task | null; board: Board; threads: ThreadSummary[]; onclose: () => void } = $props();

	function start() {
		return {
			title: task?.title ?? '',
			body: task?.body ?? '',
			columnId: task?.columnId ?? board.backlogColumnId,
			storyId: task?.storyId ?? '',
			threadSlug: task?.threadSlug ?? ''
		};
	}
	const initial = start();

	let title = $state(initial.title);
	let body = $state(initial.body);
	let columnId = $state(initial.columnId);
	let storyId = $state(initial.storyId);
	let threadSlug = $state(initial.threadSlug);
	let saving = $state(false);

	async function save() {
		saving = true;
		const saved = await attempt(async () => {
			if (!task) {
				const made = await api.createTask(project, title, body, storyId || null, threadSlug || null);
				return columnId === made.columnId ? made : api.updateTask(project, made.id, { columnId });
			}
			return api.updateTask(project, task.id, { title, body, columnId, storyId, threadSlug });
		});
		saving = false;
		if (!saved) return;
		await refresh();
		onclose();
	}

	async function remove() {
		if (!task) return;
		const ok = await confirmDialog({ title: `Delete “${task.title}”?`, confirmLabel: 'Delete', danger: true });
		if (!ok) return;
		if ((await attempt(() => api.deleteTask(project, task.id))) !== undefined) {
			await refresh();
			onclose();
		}
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && title.trim()) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title={task ? 'Edit task' : 'New task'} wide {onclose}>
	<label class="field">
		Title
		<input bind:value={title} placeholder="What needs doing?" />
	</label>
	<div class="field">
		Details
		<MarkdownEditor bind:value={body} rows={5} label="Details" placeholder="Context, acceptance, links. Markdown works." />
	</div>
	<div class="form-row">
		<label class="field grow">
			Column
			<select bind:value={columnId}>
				{#each board.columns as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
			</select>
		</label>
		<label class="field grow">
			User story
			<select bind:value={storyId}>
				<option value="">None</option>
				{#each board.stories as s (s.id)}<option value={s.id}>{s.id} — {s.summary}</option>{/each}
			</select>
		</label>
		<label class="field grow">
			Thread
			<select bind:value={threadSlug}>
				<option value="">None</option>
				{#each threads as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
			</select>
		</label>
	</div>

	{#snippet footer()}
		{#if task}<button class="btn ghost danger" onclick={remove}>Delete</button>{/if}
		<span class="spacer-h"></span>
		<span class="hint">Ctrl+Enter saves</span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !title.trim()}>{task ? 'Save' : 'Create'}</button>
	{/snippet}
</Modal>
