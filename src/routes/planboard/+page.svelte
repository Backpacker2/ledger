<script lang="ts">
	import StoryModal from '$lib/components/StoryModal.svelte';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, promptDialog, refresh } from '$lib/ui.svelte';
	import type { Column, Story } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const columns = $derived(data.board.columns);
	let filter = $state('');
	let editing = $state<Story | null>(null);
	let creating = $state(false);

	// A move shows at once; the override is dropped when the reloaded board arrives.
	let moved = $state<Record<string, string>>({});
	const columnOf = (s: Story) => moved[s.id] ?? s.columnId;

	const visible = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return data.board.userStories;
		return data.board.userStories.filter((s) =>
			[s.id, s.summary, s.role, s.want, s.reason, ...s.threads.map((t) => t.title)].some((v) => v.toLowerCase().includes(q))
		);
	});
	const cardsIn = (column: Column) => visible.filter((s) => columnOf(s) === column.id);
	const total = $derived(data.board.userStories.length);

	// ---- moving cards --------------------------------------------------------------------------

	let dragging = $state<string | null>(null);
	let overColumn = $state<string | null>(null);

	async function move(story: Story, columnId: string) {
		if (columnOf(story) === columnId) return;
		moved[story.id] = columnId;
		const done = await attempt(() => api.updateUserStory(story.id, { columnId }));
		if (done) await refresh();
		delete moved[story.id];
	}

	function step(story: Story, by: -1 | 1) {
		const i = columns.findIndex((c) => c.id === columnOf(story));
		const next = columns[i + by];
		if (next) move(story, next.id);
	}

	function ondragstart(e: DragEvent, story: Story) {
		dragging = story.id;
		e.dataTransfer?.setData('text/plain', story.id);
		if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
	}

	function ondrop(e: DragEvent, column: Column) {
		e.preventDefault();
		const story = data.board.userStories.find((s) => s.id === dragging);
		dragging = overColumn = null;
		if (story) move(story, column.id);
	}

	// ---- columns -------------------------------------------------------------------------------

	async function addColumn() {
		const name = await promptDialog({ title: 'New column', label: 'Name', placeholder: 'e.g. In review', confirmLabel: 'Add' });
		if (name && (await attempt(() => api.addColumn(name)))) await refresh();
	}

	async function renameColumn(column: Column) {
		const name = await promptDialog({ title: 'Rename column', label: 'Name', initial: column.name, confirmLabel: 'Rename' });
		if (name && name !== column.name && (await attempt(() => api.renameColumn(column.id, name)))) await refresh();
	}

	async function deleteColumn(column: Column) {
		const ok = await confirmDialog({
			title: `Delete the “${column.name}” column?`,
			message: 'A column that still holds user stories cannot be deleted.',
			confirmLabel: 'Delete column',
			danger: true
		});
		if (ok && (await attempt(() => api.deleteColumn(column.id)))) await refresh();
	}
</script>

<svelte:head><title>Planboard · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Planboard</h1>
			<div class="sub">{total} user {total === 1 ? 'story' : 'stories'} · drag a card, or use its arrows, to change its column.</div>
		</div>
		<div class="header-actions">
			<button class="btn ghost" onclick={addColumn}>Add column</button>
			<button class="btn primary" onclick={() => (creating = true)}>New user story</button>
		</div>
	</div>
</div>

<div class="board-tools">
	<input type="search" placeholder="Filter stories…" bind:value={filter} aria-label="Filter stories" />
	{#if filter.trim()}<span class="hint">{visible.length} of {total} shown</span>{/if}
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
				<span class="title" title={column.id === data.board.backlogColumnId ? 'New stories land here' : ''}>{column.name}</span>
				<span class="count">{cards.length}</span>
				<button class="icon-btn" title="Rename column" aria-label="Rename {column.name}" onclick={() => renameColumn(column)}>✎</button>
				<button class="icon-btn" title="Delete column" aria-label="Delete {column.name}" onclick={() => deleteColumn(column)}>✕</button>
			</header>
			<div class="col-cards">
				{#each cards as s (s.id)}
					{@const done = s.criteria.filter((c) => c.done).length}
					<div
						class="kcard"
						class:dragging={dragging === s.id}
						role="button"
						tabindex="0"
						draggable="true"
						ondragstart={(e) => ondragstart(e, s)}
						ondragend={() => (dragging = overColumn = null)}
						onclick={() => (editing = s)}
						onkeydown={(e) => e.key === 'Enter' && e.target === e.currentTarget && (editing = s)}
					>
						<div class="k-top">
							<span class="k-key">{s.id}</span>
							<span class="k-arrows">
								<button aria-label="Move left" disabled={ci === 0} onclick={(e) => { e.stopPropagation(); step(s, -1); }}>◀</button>
								<button aria-label="Move right" disabled={ci === columns.length - 1} onclick={(e) => { e.stopPropagation(); step(s, 1); }}>▶</button>
							</span>
						</div>
						<div class="k-title">{s.summary}</div>
						<div class="k-meta">
							{#if s.criteria.length}
								<span class="k-badge" class:done={done === s.criteria.length}>{done}/{s.criteria.length} met</span>
							{/if}
							{#each s.threads.slice(0, 2) as t (t.slug)}<span class="k-badge" title={t.title}>{t.title}</span>{/each}
							{#if s.threads.length > 2}<span class="k-badge">+{s.threads.length - 2}</span>{/if}
						</div>
					</div>
				{:else}
					<div class="col-empty">{filter.trim() ? 'No matches' : 'Drop stories here'}</div>
				{/each}
			</div>
		</section>
	{/each}
</div>

{#if creating}
	<StoryModal story={null} threads={data.threads} onclose={() => (creating = false)} />
{/if}
{#if editing}
	<StoryModal story={editing} threads={data.threads} onclose={() => (editing = null)} />
{/if}
