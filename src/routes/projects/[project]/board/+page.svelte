<script lang="ts">
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import TaskModal from '$lib/components/TaskModal.svelte';
	import { api } from '$lib/api';
	import { threadUrl } from '$lib/actions';
	import { plural } from '$lib/format';
	import { attempt, confirmDialog, promptDialog, refresh } from '$lib/ui.svelte';
	import type { Column, Task } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const project = $derived(data.project.slug);
	const board = $derived(data.board);
	const columns = $derived(board.columns);
	const threads = $derived(data.projects.find((p) => p.slug === project)?.threads ?? []);

	let filter = $state('');
	let editing = $state<Task | null>(null);
	let creating = $state(false);

	// A move shows at once; the override is dropped when the reloaded board arrives.
	let moved = $state<Record<string, string>>({});
	const columnOf = (t: Task) => moved[t.id] ?? t.columnId;

	const visible = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return board.tasks;
		return board.tasks.filter((t) =>
			[t.title, t.body, t.storyId ?? '', t.storySummary ?? '', t.threadTitle ?? ''].some((v) => v.toLowerCase().includes(q))
		);
	});
	const cardsIn = (column: Column) => visible.filter((t) => columnOf(t) === column.id);
	const total = $derived(board.tasks.length);

	// ---- moving cards --------------------------------------------------------------------------

	let dragging = $state<string | null>(null);
	let overColumn = $state<string | null>(null);

	async function move(task: Task, columnId: string) {
		if (columnOf(task) === columnId) return;
		moved[task.id] = columnId;
		const done = await attempt(() => api.updateTask(project, task.id, { columnId }));
		if (done) await refresh();
		delete moved[task.id];
	}

	function step(task: Task, by: -1 | 1) {
		const i = columns.findIndex((c) => c.id === columnOf(task));
		const next = columns[i + by];
		if (next) move(task, next.id);
	}

	function ondragstart(e: DragEvent, task: Task) {
		dragging = task.id;
		e.dataTransfer?.setData('text/plain', task.id);
		if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
	}

	function ondrop(e: DragEvent, column: Column) {
		e.preventDefault();
		const task = board.tasks.find((t) => t.id === dragging);
		dragging = overColumn = null;
		if (task) move(task, column.id);
	}

	// ---- columns -------------------------------------------------------------------------------

	async function addColumn() {
		const name = await promptDialog({ title: 'New column', label: 'Name', placeholder: 'e.g. In review', confirmLabel: 'Add' });
		if (name && (await attempt(() => api.addColumn(project, name)))) await refresh();
	}

	async function renameColumn(column: Column) {
		const name = await promptDialog({ title: 'Rename column', label: 'Name', initial: column.name, confirmLabel: 'Rename' });
		if (name && name !== column.name && (await attempt(() => api.renameColumn(project, column.id, name)))) await refresh();
	}

	async function deleteColumn(column: Column) {
		const ok = await confirmDialog({
			title: `Delete the “${column.name}” column?`,
			message: 'A column that still holds tasks cannot be deleted.',
			confirmLabel: 'Delete column',
			danger: true
		});
		if (ok && (await attempt(() => api.deleteColumn(project, column.id)))) await refresh();
	}
</script>

<svelte:head><title>Board · {data.project.name} · Ledger</title></svelte:head>

<div class="board-tools">
	<input type="search" placeholder="Filter tasks…" bind:value={filter} aria-label="Filter tasks" />
	{#if filter.trim()}<span class="hint">{visible.length} of {total} shown</span>{:else}
		<span class="hint">{plural(total, 'task')} · drag a card, or use its arrows, to change its column.</span>
	{/if}
	<span style="flex:1"></span>
	<button class="btn ghost small" onclick={addColumn}>Add column</button>
	<button class="btn primary small" onclick={() => (creating = true)}>New task</button>
</div>

<div class="board">
	{#each columns as column, ci (column.id)}
		{@const cards = cardsIn(column)}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<section
			class="col"
			class:drop={overColumn === column.id && dragging !== null}
			ondragover={(e) => {
				if (dragging) {
					e.preventDefault();
					overColumn = column.id;
				}
			}}
			ondragleave={(e) => {
				if (!e.currentTarget.contains(e.relatedTarget as Node)) overColumn = null;
			}}
			ondrop={(e) => ondrop(e, column)}
		>
			<header>
				<span class="title" title={column.id === board.backlogColumnId ? 'New tasks land here' : ''}>{column.name}</span>
				<span class="count">{cards.length}</span>
				<button class="icon-btn" title="Rename column" aria-label="Rename {column.name}" onclick={() => renameColumn(column)}><Pencil size={14} /></button>
				<button class="icon-btn" title="Delete column" aria-label="Delete {column.name}" onclick={() => deleteColumn(column)}><Trash2 size={14} /></button>
			</header>
			<div class="col-cards">
				{#each cards as t (t.id)}
					<div
						class="kcard"
						class:dragging={dragging === t.id}
						role="button"
						tabindex="0"
						draggable="true"
						ondragstart={(e) => ondragstart(e, t)}
						ondragend={() => (dragging = overColumn = null)}
						onclick={() => (editing = t)}
						onkeydown={(e) => e.key === 'Enter' && e.target === e.currentTarget && (editing = t)}
					>
						<div class="k-top">
							{#if t.storyId}<span class="k-key" title={t.storySummary ?? ''}>{t.storyId}</span>{:else}<span></span>{/if}
							<span class="k-arrows">
								<button aria-label="Move left" disabled={ci === 0} onclick={(e) => { e.stopPropagation(); step(t, -1); }}><ChevronLeft size={14} /></button>
								<button aria-label="Move right" disabled={ci === columns.length - 1} onclick={(e) => { e.stopPropagation(); step(t, 1); }}><ChevronRight size={14} /></button>
							</span>
						</div>
						<div class="k-title">{t.title}</div>
						{#if t.threadSlug}
							<div class="k-meta">
								<a
									class="k-badge"
									href={threadUrl(project, t.threadSlug)}
									title="Open the thread"
									onclick={(e) => e.stopPropagation()}
									style="text-decoration:none">{t.threadTitle}</a
								>
							</div>
						{/if}
					</div>
				{:else}
					<div class="col-empty">{filter.trim() ? 'No matches' : 'Drop tasks here'}</div>
				{/each}
			</div>
		</section>
	{/each}
</div>

{#if creating}
	<TaskModal {project} task={null} {board} {threads} onclose={() => (creating = false)} />
{/if}
{#if editing}
	<TaskModal {project} task={editing} {board} {threads} onclose={() => (editing = null)} />
{/if}
