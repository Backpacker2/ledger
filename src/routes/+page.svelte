<script lang="ts">
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import File from '@lucide/svelte/icons/file';
	import FileImage from '@lucide/svelte/icons/file-image';
	import FileText from '@lucide/svelte/icons/file-text';
	import Pin from '@lucide/svelte/icons/pin';
	import MessagesSquare from '@lucide/svelte/icons/messages-square';
	import Plus from '@lucide/svelte/icons/plus';
	import SquareKanban from '@lucide/svelte/icons/square-kanban';
	import { deliverableUrl, newProjectFlow, newThreadFlow, projectUrl, snapshotPath, threadUrl } from '$lib/actions';
	import ContributionGraph from '$lib/components/ContributionGraph.svelte';
	import MmmBadge from '$lib/components/MmmBadge.svelte';
	import { ago, plural } from '$lib/format';
	import { dashboard, isPinned, togglePin } from '$lib/dashboard.svelte';
	import type { DeliverableSummary, ThreadSummary } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const THREADS = 3;
	const DELIVERABLES = 2;
	const DAY = 86_400_000;

	const time = (iso: string) => new Date(iso).getTime() || 0;
	const newest = <T,>(items: readonly T[], at: (item: T) => string) => [...items].sort((a, b) => time(at(b)) - time(at(a)));
	const within = (iso: string, ms: number) => Date.now() - time(iso) < ms;

	const today = new Intl.DateTimeFormat(undefined, { weekday: 'long', day: 'numeric', month: 'long' }).format(new Date());

	/** Every project with what it shows, the most recently active first. */
	const cards = $derived(
		data.projects
			.map((p) => {
				const deliverables = data.details[p.slug]?.deliverables ?? [];
				const threads = newest(p.threads, (t) => t.lastActivity);
				const latest = newest(deliverables, (d) => d.modified);
				const active = [p.updatedAt, threads[0]?.lastActivity, latest[0]?.modified]
					.filter((at): at is string => !!at)
					.reduce((a, b) => (time(b) > time(a) ? b : a));
				return {
					project: p,
					active,
					threads: threads.slice(0, THREADS),
					deliverables: latest.slice(0, DELIVERABLES),
					deliverableCount: deliverables.length,
					openFeedback: deliverables.reduce((n, d) => n + d.openFeedback, 0),
					openTasks: data.details[p.slug]?.openTasks ?? 0
				};
			})
			.sort((a, b) => time(b.active) - time(a.active))
	);

	type Card = (typeof cards)[number];

	/** Pinned projects first, in the order they were pinned; then the rest, the most recently active first. */
	const pinned = $derived(
		dashboard.pins.map((slug) => cards.find((c) => c.project.slug === slug)).filter((c): c is Card => !!c)
	);
	const others = $derived(cards.filter((c) => !dashboard.pins.includes(c.project.slug)));

	/** "4 posts · 2 prompts · 1 feedback": what a thread holds, leaving out what it has none of. */
	function threadMeta(t: ThreadSummary): string {
		const parts = [plural(t.postCount, 'post')];
		if (t.promptCount) parts.push(plural(t.promptCount, 'prompt'));
		if (t.feedbackCount) parts.push(`${t.feedbackCount} feedback`);
		if (t.uploadCount) parts.push(plural(t.uploadCount, 'upload'));
		return parts.join(' · ');
	}

	const ICON = { markdown: FileText, image: FileImage, other: File } as const;

	/** A deliverable whose live file is gone opens at its newest published version. */
	const target = (d: DeliverableSummary) =>
		d.exists || d.versions.length === 0 ? d.path : snapshotPath(d.path, d.versions.at(-1) as string);
</script>

{#snippet card(c: Card)}
	{@const p = c.project}
	<article class="proj" class:pinned={isPinned(p.slug)}>
		<header class="proj-head">
			<div class="proj-title">
				<a class="proj-name" href={projectUrl(p.slug)}>{p.name}</a>
				<span class="proj-when">Active {ago(c.active)}</span>
				<button
					class="pin-btn"
					class:on={isPinned(p.slug)}
					onclick={() => togglePin(p.slug)}
					aria-pressed={isPinned(p.slug)}
					aria-label={isPinned(p.slug) ? `Unpin ${p.name}` : `Pin ${p.name} to the top`}
					title={isPinned(p.slug) ? 'Unpin' : 'Pin to the top'}
				>
					<Pin size={15} fill={isPinned(p.slug) ? 'currentColor' : 'none'} />
				</button>
			</div>
			<div class="proj-stats">
				<span>{plural(p.threads.length, 'thread')}</span>
				<span>{plural(c.deliverableCount, 'deliverable')}</span>
				<span>{plural(p.storyCount, 'story', 'stories')}</span>
				{#if c.openFeedback}<span class="badge draft">{c.openFeedback} open feedback</span>{/if}
			</div>
		</header>

		<section class="proj-section">
			<div class="proj-section-head">
				<h3>Latest threads</h3>
				{#if p.threads.length > THREADS}
					<a href={projectUrl(p.slug)}>All {p.threads.length}<ArrowRight size={13} /></a>
				{/if}
			</div>
			{#each c.threads as t (t.slug)}
				<a class="dash-row" href={threadUrl(p.slug, t.slug)}>
					<span class="dash-ico"><MessagesSquare size={16} /></span>
					<span class="dash-main">
						<span class="dash-title">{t.title}</span>
						<span class="dash-meta">{threadMeta(t)}</span>
					</span>
					<span class="dash-when" class:fresh={within(t.lastActivity, DAY)}>{ago(t.lastActivity)}</span>
				</a>
			{:else}
				<div class="dash-empty">
					No threads yet.
					<button class="link accent" onclick={() => newThreadFlow(p.slug)}>Start one</button>
				</div>
			{/each}
		</section>

		<section class="proj-section">
			<div class="proj-section-head">
				<h3>Latest deliverables</h3>
				{#if c.deliverableCount > DELIVERABLES}
					<a href="{projectUrl(p.slug)}/deliverables">All {c.deliverableCount}<ArrowRight size={13} /></a>
				{/if}
			</div>
			{#each c.deliverables as d (d.path)}
				{@const Icon = ICON[d.kind]}
				<a class="dash-row" href={deliverableUrl(p.slug, target(d))}>
					<span class="dash-ico"><Icon size={16} /></span>
					<span class="dash-main">
						<span class="dash-title" title={d.path}>{d.name}</span>
						<span class="dash-meta dash-badges">
							{#if d.versions.length}
								<span class="badge published">v{d.versions.at(-1)}</span>
							{:else}
								<span class="badge draft">Draft</span>
							{/if}
							{#if d.mmm}<MmmBadge level={d.mmm} />{/if}
							{#if d.openFeedback}<span class="badge draft">{d.openFeedback} open</span>{/if}
							{#if !d.exists}<span class="badge">live file removed</span>{/if}
						</span>
					</span>
					<span class="dash-when" class:fresh={within(d.modified, DAY)}>{ago(d.modified)}</span>
				</a>
			{:else}
				<div class="dash-empty">
					Nothing delivered yet.
					<a class="link accent" href="{projectUrl(p.slug)}/deliverables">Create one</a>
				</div>
			{/each}
		</section>

		<footer class="proj-foot">
			<a class="btn small ghost" href="{projectUrl(p.slug)}/board">
				<SquareKanban size={14} />Board{#if c.openTasks}<span class="n">{c.openTasks} open</span>{/if}
			</a>
			<button class="btn small ghost" onclick={() => newThreadFlow(p.slug)}><Plus size={14} />New thread</button>
			<a class="btn small proj-open" href={projectUrl(p.slug)}>Open project<ArrowRight size={14} /></a>
		</footer>
	</article>
{/snippet}

<svelte:head><title>Ledger</title></svelte:head>

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
				<h1>Dashboard</h1>
				<div class="sub">{today} · {plural(data.projects.length, 'project')} in your vault</div>
			</div>
			<div class="header-actions">
				<button class="btn primary" onclick={newProjectFlow}><Plus size={16} />New project</button>
			</div>
		</div>
	</div>

	<div class="page">
		<div class="page-inner wide">
			{#if dashboard.showActivity}
				<ContributionGraph events={data.activity} projects={data.projects} />
			{/if}
			{#if pinned.length}
				<section class="proj-group" aria-label="Pinned projects">
					<h2 class="proj-group-head"><Pin size={13} />Pinned</h2>
					<div class="proj-grid">
						{#each pinned as c (c.project.slug)}{@render card(c)}{/each}
					</div>
				</section>
			{/if}
			{#if others.length}
				<section class="proj-group" aria-label="Projects">
					{#if pinned.length}<h2 class="proj-group-head">Other projects</h2>{/if}
					<div class="proj-grid">
						{#each others as c (c.project.slug)}{@render card(c)}{/each}
					</div>
				</section>
			{/if}
		</div>
	</div>
{/if}
