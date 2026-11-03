<script lang="ts">
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api';
	import { projectUrl, threadUrl } from '$lib/actions';
	import Composer from '$lib/components/Composer.svelte';
	import FeedbackCard from '$lib/components/FeedbackCard.svelte';
	import FeedbackModal from '$lib/components/FeedbackModal.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import PostCard from '$lib/components/PostCard.svelte';
	import PromptCard from '$lib/components/PromptCard.svelte';
	import PromptModal from '$lib/components/PromptModal.svelte';
	import StoryModal from '$lib/components/StoryModal.svelte';
	import UploadCard from '$lib/components/UploadCard.svelte';
	import UploadModal from '$lib/components/UploadModal.svelte';
	import type { Picked } from '$lib/platform';
	import { date, dayKey, dayLabel, plural } from '$lib/format';
	import { orderFlow } from '$lib/order';
	import { listOrder } from '$lib/order.svelte';
	import { attempt, confirmDialog, promptDialog, refresh } from '$lib/ui.svelte';
	import type { Feedback, Post, Prompt, Story, Upload } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const thread = $derived(data.thread);
	const project = $derived(thread.projectSlug);
	const slug = $derived(thread.slug);
	const projectName = $derived(data.project.name);
	const posts = $derived(thread.posts);
	const byId = $derived(Object.fromEntries(posts.map((p) => [p.id, p])));
	const siblings = $derived((data.projects.find((p) => p.slug === project)?.threads ?? []).filter((t) => t.slug !== slug));
	const linked = $derived(new Set(thread.linkedThreads.map((t) => t.slug)));

	/**
	 * Any reply chain rooted at a guiding question is pulled out of the chronological flow and shown inside
	 * the question's card, so question and answers read as one block.
	 */
	const structure = $derived.by(() => {
		const children = new Map<string, Post[]>();
		for (const p of posts) {
			if (p.parentId) children.set(p.parentId, [...(children.get(p.parentId) ?? []), p]);
		}
		const answers = new Map<string, Post[]>();
		const embedded = new Set<string>();
		for (const question of posts.filter((p) => p.guidingQuestion)) {
			const chain: Post[] = [];
			const queue = [...(children.get(question.id) ?? [])];
			for (let next = queue.shift(); next; next = queue.shift()) {
				chain.push(next);
				embedded.add(next.id);
				queue.push(...(children.get(next.id) ?? []));
			}
			chain.sort((a, b) => posts.indexOf(a) - posts.indexOf(b));
			answers.set(question.id, chain);
		}
		return { answers, embedded };
	});

	type Entry =
		| { kind: 'post'; key: string; at: string; post: Post }
		| { kind: 'prompt'; key: string; at: string; prompt: Prompt }
		| { kind: 'feedback'; key: string; at: string; feedback: Feedback }
		| { kind: 'upload'; key: string; at: string; upload: Upload };

	/** Notes, prompts, feedback and uploads in the order they happened, oldest first. */
	const chronological = $derived.by(() => {
		const entries: Entry[] = [
			...posts.filter((p) => !structure.embedded.has(p.id)).map((post): Entry => ({ kind: 'post', key: `p-${post.id}`, at: post.createdAt, post })),
			...thread.prompts.map((prompt): Entry => ({ kind: 'prompt', key: `pr-${prompt.id}`, at: prompt.createdAt, prompt })),
			...thread.feedback.map((feedback): Entry => ({ kind: 'feedback', key: `f-${feedback.id}`, at: feedback.createdAt, feedback })),
			...thread.uploads.map((upload): Entry => ({ kind: 'upload', key: `u-${upload.id}`, at: upload.uploadedAt, upload }))
		];
		entries.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
		return entries;
	});

	/** The same entries read the way Settings says (oldest or newest first), each day under its own heading. */
	const flow = $derived.by(() => {
		const entries = orderFlow(chronological, listOrder.order);
		return entries.map((entry, i) => ({
			entry,
			day: dayKey(entry.at),
			newDay: i === 0 || dayKey(entries[i - 1].at) !== dayKey(entry.at)
		}));
	});

	let replyTo = $state<Post | null>(null);
	let scroller: HTMLElement;

	let promptModal = $state<{ prompt: Prompt | null } | null>(null);
	let feedbackOpen = $state(false);
	let uploadPaths = $state<Picked[] | null>(null);
	let linking = $state(false);
	let storiesOpen = $state(false);
	let storyModal = $state<{ story: Story | null } | null>(null);
	let allStories = $state<Story[]>([]);

	// A different thread starts with no reply in progress.
	$effect(() => {
		slug;
		replyTo = null;
	});

	function jump(domId: string) {
		const el = document.getElementById(domId);
		if (!el) return;
		el.scrollIntoView({ block: 'center', behavior: 'smooth' });
		el.classList.add('flash');
		setTimeout(() => el.classList.remove('flash'), 1400);
	}

	// The newest item is at the bottom when posts read oldest first, and at the top when they read newest first.
	const toNewest = () => scroller && (scroller.scrollTop = listOrder.order === 'asc' ? scroller.scrollHeight : 0);

	// Arriving in a thread lands on the item a search hit (or a fresh screenshot) pointed at, or else the
	// newest one; a new item scrolls itself into view.
	let seenThread = '';
	let seenCount = 0;
	$effect(() => {
		const count = flow.length;
		const key = `${project}/${slug}`;
		const target = page.url.searchParams.get('item');
		if (key !== seenThread) {
			seenThread = key;
			seenCount = count;
			tick().then(() => (target && document.getElementById(target) ? jump(target) : toNewest()));
		} else if (count > seenCount) {
			seenCount = count;
			tick().then(toNewest);
		} else {
			seenCount = count;
		}
	});

	async function rename() {
		const title = await promptDialog({ title: 'Rename thread', label: 'Title', initial: thread.title, confirmLabel: 'Rename' });
		if (!title || title === thread.title) return;
		const renamed = await attempt(() => api.renameThread(project, slug, title));
		if (!renamed) return;
		await refresh();
		if (renamed.slug !== slug) await goto(threadUrl(project, renamed.slug), { replaceState: true });
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Delete “${thread.title}”?`,
			message: `Its ${plural(posts.length, 'post')}, prompts, feedback and uploaded files go with it. Deliverables are kept, and its user stories lose the link.`,
			confirmLabel: 'Delete thread',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteThread(project, slug))) === undefined) return;
		await refresh();
		await goto(projectUrl(project));
	}

	async function toggleLink(other: string, on: boolean) {
		const done = await attempt(() => (on ? api.linkThreads(project, slug, other) : api.unlinkThreads(project, slug, other)));
		if (done !== undefined) await refresh();
	}

	async function openStories() {
		allStories = (await attempt(() => api.listStories(project))) ?? [];
		storiesOpen = true;
	}

	async function toggleStory(story: Story, on: boolean) {
		const done = await attempt(() => (on ? api.linkStoryThread(project, story.id, slug) : api.unlinkStoryThread(project, story.id, slug)));
		if (done) {
			allStories = allStories.map((s) => (s.id === done.id ? done : s));
			await refresh();
		}
	}

	const storyIds = $derived(new Set(thread.stories.map((s) => s.id)));
	// From the oldest-first list: the last item of `flow` is the oldest when posts read newest first.
	const lastActivity = $derived(chronological.at(-1)?.at ?? thread.createdAt);
</script>

<svelte:head><title>{thread.title} · Ledger</title></svelte:head>

<div class="header-wrap">
	<div class="crumbs">
		<a href={projectUrl(project)}>{projectName}</a><span>/</span><span>Threads</span>
	</div>
	<div class="header-top">
		<div>
			<h1>{thread.title} <span class="badge accent">Thread</span></h1>
			<div class="sub">
				{plural(posts.length, 'post')} · {plural(thread.prompts.length, 'prompt')} · {plural(thread.feedback.length, 'feedback item', 'feedback items')} ·
				{plural(thread.uploads.length, 'upload')} · last activity {date(lastActivity)}
			</div>
		</div>
		<div class="header-actions">
			<button class="btn ghost small" onclick={rename}>Rename</button>
			<button class="btn ghost small" onclick={() => (linking = true)} disabled={siblings.length === 0}>Link threads…</button>
			<button class="btn ghost small" onclick={openStories}>Stories{thread.stories.length ? ` (${thread.stories.length})` : ''}…</button>
			<button class="btn ghost small danger" onclick={remove}>Delete</button>
		</div>
	</div>

	{#if thread.linkedThreads.length || thread.stories.length}
		<div class="subline" style="padding-bottom:12px">
			{#if thread.linkedThreads.length}
				<span>Linked:</span>
				<div class="chips">
					{#each thread.linkedThreads as t (t.slug)}
						<a class="chip" href={threadUrl(project, t.slug)}>{t.title}</a>
					{/each}
				</div>
			{/if}
			{#if thread.stories.length}
				<span>Stories:</span>
				<div class="chips">
					{#each thread.stories as s (s.id)}
						<button class="chip plain" title={s.summary} onclick={() => (storyModal = { story: s })}>{s.id}</button>
					{/each}
				</div>
			{/if}
		</div>
	{:else}
		<div style="height:14px"></div>
	{/if}
</div>

<div class="thread" bind:this={scroller}>
	<div class="thread-inner">
		{#each flow as item (item.entry.key)}
			{#if item.newDay}<div class="divider">{dayLabel(item.day)}</div>{/if}
			{@const e = item.entry}
			{#if e.kind === 'post'}
				{@const answers = structure.answers.get(e.post.id)}
				<PostCard {project} thread={slug} post={e.post} {byId} onreply={(p) => (replyTo = p)} onjump={jump}>
					{#if answers?.length}
						<div class="answers">
							{#each answers as a (a.id)}
								<PostCard
									{project}
									thread={slug}
									post={a}
									{byId}
									variant="answer"
									quotedElsewhere={e.post.id}
									onreply={(p) => (replyTo = p)}
									onjump={jump}
								/>
							{/each}
						</div>
					{/if}
				</PostCard>
			{:else if e.kind === 'prompt'}
				<PromptCard {project} thread={slug} prompt={e.prompt} onedit={(prompt) => (promptModal = { prompt })} />
			{:else if e.kind === 'feedback'}
				<FeedbackCard {project} thread={slug} feedback={e.feedback} />
			{:else}
				<UploadCard {project} thread={slug} upload={e.upload} />
			{/if}
		{:else}
			<div class="empty-state">
				<h1>Nothing here yet</h1>
				<p>
					Write your first note below. Every note is saved on its own with its time. Use <b>Prompt</b> to ask for a deliverable,
					<b>Feedback</b> to react to one, and <b>Upload</b> to add files.
				</p>
			</div>
		{/each}
	</div>
</div>

<Composer
	{project}
	thread={slug}
	{replyTo}
	oncancelreply={() => (replyTo = null)}
	onsent={() => (replyTo = null)}
	onprompt={() => (promptModal = { prompt: null })}
	onfeedback={() => (feedbackOpen = true)}
	onupload={(paths) => (uploadPaths = paths)}
/>

{#if promptModal}
	<PromptModal {project} thread={slug} prompt={promptModal.prompt} onclose={() => (promptModal = null)} />
{/if}
{#if feedbackOpen}
	<FeedbackModal {project} thread={slug} onclose={() => (feedbackOpen = false)} />
{/if}
{#if uploadPaths}
	<UploadModal {project} thread={slug} paths={uploadPaths} onclose={() => (uploadPaths = null)} />
{/if}

{#if linking}
	<Modal title="Link threads" onclose={() => (linking = false)}>
		<p style="margin:0;color:var(--muted)">Linked threads belong together — for example several threads feeding one deliverable. Links work both ways.</p>
		<div class="link-grid">
			{#each siblings as t (t.slug)}
				<label class="check">
					<input type="checkbox" checked={linked.has(t.slug)} onchange={(e) => toggleLink(t.slug, e.currentTarget.checked)} />
					{t.title}
				</label>
			{/each}
		</div>
		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn primary" onclick={() => (linking = false)}>Done</button>
		{/snippet}
	</Modal>
{/if}

{#if storiesOpen}
	<Modal title="User stories this thread serves" onclose={() => (storiesOpen = false)}>
		{#if allStories.length === 0}
			<p class="hint" style="margin:0">This project has no user stories yet.</p>
		{:else}
			<div class="link-grid">
				{#each allStories as s (s.id)}
					<label class="check">
						<input type="checkbox" checked={storyIds.has(s.id)} onchange={(e) => toggleStory(s, e.currentTarget.checked)} />
						<span><b>{s.id}</b> {s.summary}</span>
					</label>
				{/each}
			</div>
		{/if}
		{#snippet footer()}
			<button
				class="btn"
				onclick={() => {
					storiesOpen = false;
					storyModal = { story: null };
				}}>New user story…</button
			>
			<span class="spacer-h"></span>
			<button class="btn primary" onclick={() => (storiesOpen = false)}>Done</button>
		{/snippet}
	</Modal>
{/if}

{#if storyModal}
	<StoryModal
		{project}
		story={storyModal.story}
		threads={data.projects.find((p) => p.slug === project)?.threads ?? []}
		initialThreadSlugs={[slug]}
		onclose={() => (storyModal = null)}
	/>
{/if}
