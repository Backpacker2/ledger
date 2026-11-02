<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import StoryModal from '$lib/components/StoryModal.svelte';
	import { api } from '$lib/api';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { Story } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const slug = $derived(data.thread.slug);
	const columnName = (id: string) => data.board.columns.find((c) => c.id === id)?.name ?? '';
	const linkedIds = $derived(new Set(data.thread.stories.map((s) => s.id)));

	let editing = $state<Story | null>(null);
	let creating = $state(false);
	let linking = $state(false);

	async function toggle(story: Story, on: boolean) {
		const done = await attempt(() => (on ? api.linkStoryThread(story.id, slug) : api.unlinkStoryThread(story.id, slug)));
		if (done) await refresh();
	}
</script>

<div class="page">
	<div class="page-inner">
		<div class="panel">
			<div class="form-row" style="justify-content:space-between;align-items:center">
				<h2>User stories this thread serves</h2>
				<div class="header-actions">
					<button class="btn small" onclick={() => (linking = true)} disabled={data.board.userStories.length === 0}>Link existing…</button>
					<button class="btn small primary" onclick={() => (creating = true)}>New user story…</button>
				</div>
			</div>
			{#if data.thread.stories.length === 0}
				<div class="hint">No user stories linked yet. A thread's documents belong to its stories.</div>
			{:else}
				<div class="doc-list">
					{#each data.thread.stories as s (s.id)}
						{@const done = s.criteria.filter((c) => c.done).length}
						<button class="story-row" onclick={() => (editing = s)}>
							<span class="k-key">{s.id}</span>
							<span class="k-title">{s.summary}</span>
							<span class="k-meta">
								{#if s.criteria.length}
									<span class="k-badge" class:done={done === s.criteria.length}>{done}/{s.criteria.length} met</span>
								{/if}
							</span>
							<span class="badge">{columnName(s.columnId)}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>

{#if creating}
	<StoryModal story={null} threads={data.threads} initialThreadSlugs={[slug]} onclose={() => (creating = false)} />
{/if}
{#if editing}
	<StoryModal story={editing} threads={data.threads} onclose={() => (editing = null)} />
{/if}
{#if linking}
	<Modal title="Link user stories" onclose={() => (linking = false)}>
		<p style="margin:0;color:var(--muted)">Tick the stories this thread serves.</p>
		<div class="link-grid">
			{#each data.board.userStories as s (s.id)}
				<label class="check">
					<input type="checkbox" checked={linkedIds.has(s.id)} onchange={(e) => toggle(s, e.currentTarget.checked)} />
					<span><b>{s.id}</b> {s.summary}</span>
				</label>
			{/each}
		</div>
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn primary" onclick={() => (linking = false)}>Done</button>
		{/snippet}
	</Modal>
{/if}
