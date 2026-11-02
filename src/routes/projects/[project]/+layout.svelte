<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { deleteProjectFlow, projectUrl, renameProjectFlow } from '$lib/actions';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const project = $derived(data.project);
	const base = $derived(projectUrl(project.slug));
	const path = $derived(page.url.pathname);

	// A thread and a deliverable have a header of their own (with a way back to the project); the project's tabs
	// belong to the other pages.
	const ownHeader = $derived(!!page.params.thread || !!page.route.id?.endsWith('/deliverables/view'));

	const tabs = $derived([
		{ href: base, label: 'Overview', n: null, exact: true },
		{ href: `${base}/deliverables`, label: 'Deliverables', n: project.deliverableCount, exact: false },
		{ href: `${base}/stories`, label: 'Stories', n: project.storyCount, exact: false },
		{ href: `${base}/board`, label: 'Board', n: project.openTaskCount, exact: false },
		{ href: `${base}/templates`, label: 'Templates', n: null, exact: false }
	]);
	const active = (t: { href: string; exact: boolean }) => (t.exact ? path === t.href : path === t.href || path.startsWith(t.href + '/'));
</script>

{#if !ownHeader}
	<div class="header-wrap">
		<div class="header-top">
			<div>
				<h1>{project.name}</h1>
				{#if project.description}<div class="sub">{project.description}</div>{/if}
			</div>
			<div class="header-actions">
				<button class="btn small ghost" onclick={() => renameProjectFlow(project.slug, project.name)}>Rename</button>
				<button class="btn small ghost danger" onclick={() => deleteProjectFlow(project.slug, project.name)}>Delete</button>
			</div>
		</div>
		<nav class="tabs" aria-label="Project">
			{#each tabs as t (t.href)}
				<a class="tab" class:active={active(t)} href={t.href}>
					{t.label}{#if t.n}<span class="n">{t.n}</span>{/if}
				</a>
			{/each}
		</nav>
	</div>
{/if}

{@render children()}
