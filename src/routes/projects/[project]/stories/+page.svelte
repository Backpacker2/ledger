<script lang="ts">
	import StoryModal from '$lib/components/StoryModal.svelte';
	import { plural } from '$lib/format';
	import type { Story } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const project = $derived(data.project.slug);
	const threads = $derived(data.projects.find((p) => p.slug === project)?.threads ?? []);

	let editing = $state<Story | null>(null);
	let creating = $state(false);
</script>

<svelte:head><title>Stories · {data.project.name} · Ledger</title></svelte:head>

<div class="page">
	<div class="page-inner wide">
		<div class="panel">
			<div class="header-top">
				<div>
					<h2>User stories</h2>
					<div class="sub">What the project is for, with the criteria that say when it is done. Tasks on the board can point at a story.</div>
				</div>
				<button class="btn primary small" onclick={() => (creating = true)}>New user story…</button>
			</div>
			{#if data.stories.length === 0}
				<div class="hint">No user stories yet.</div>
			{:else}
				<div class="doc-list">
					{#each data.stories as s (s.id)}
						{@const done = s.criteria.filter((c) => c.done).length}
						<button class="story-row" onclick={() => (editing = s)}>
							<span class="k-key">{s.id}</span>
							<span class="k-title">{s.summary}</span>
							<span class="k-meta">
								{#if s.criteria.length}
									<span class="k-badge" class:done={done === s.criteria.length}>{done}/{s.criteria.length} met</span>
								{/if}
								{#each s.threads.slice(0, 2) as t (t.slug)}<span class="k-badge" title={t.title}>{t.title}</span>{/each}
								{#if s.threads.length > 2}<span class="k-badge">+{s.threads.length - 2}</span>{/if}
							</span>
							<span class="badge">{plural(s.taskCount, 'task')}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>

{#if creating}
	<StoryModal {project} story={null} {threads} onclose={() => (creating = false)} />
{/if}
{#if editing}
	<StoryModal {project} story={editing} {threads} onclose={() => (editing = null)} />
{/if}
