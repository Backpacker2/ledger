<script lang="ts">
	import { newProjectFlow, projectUrl } from '$lib/actions';
	import { ago, plural } from '$lib/format';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();
</script>

{#if data.projects.length === 0}
	<div class="empty-state">
		<h1>Welcome to Ledger</h1>
		<p>
			Everything lives in a folder of markdown files — a vault — that you can open in any editor and sync to a private GitHub
			repository. Start with a project: it holds your threads, deliverables, user stories and board.
		</p>
		<button class="btn primary" onclick={newProjectFlow}>Create your first project</button>
		<p class="hint">Choose where the vault lives, and connect GitHub, in Settings.</p>
	</div>
{:else}
	<div class="header-wrap flat">
		<div class="header-top">
			<div>
				<h1>Projects</h1>
				<div class="sub">{plural(data.projects.length, 'project')} in your vault</div>
			</div>
			<div class="header-actions">
				<button class="btn primary" onclick={newProjectFlow}>New project</button>
			</div>
		</div>
	</div>
	<div class="page">
		<div class="page-inner wide">
			<div class="tiles">
				{#each data.projects as p (p.slug)}
					<a class="tile" href={projectUrl(p.slug)}>
						<b>{p.name}</b>
						<span>
							{plural(p.threads.length, 'thread')} · {plural(p.deliverableCount, 'deliverable')} · {plural(p.taskCount, 'task')}
						</span>
						<span>Updated {ago(p.updatedAt)}</span>
					</a>
				{/each}
			</div>
		</div>
	</div>
{/if}
