<script lang="ts">
	import { tick } from 'svelte';
	import { page } from '$app/state';
	import Composer from '$lib/components/Composer.svelte';
	import PostCard from '$lib/components/PostCard.svelte';
	import { dayKey, dayLabel } from '$lib/format';
	import type { Post } from '$lib/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const posts = $derived(data.thread.posts);
	const slug = $derived(data.thread.slug);
	const byId = $derived(Object.fromEntries(posts.map((p) => [p.id, p])));

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

	const flow = $derived(
		posts
			.filter((p) => !structure.embedded.has(p.id))
			.map((p, i, all) => ({
				post: p,
				day: dayKey(p.createdAt),
				newDay: i === 0 || dayKey(all[i - 1].createdAt) !== dayKey(p.createdAt)
			}))
	);

	let replyTo = $state<Post | null>(null);
	let scroller: HTMLElement;

	// A different thread starts with no reply in progress.
	$effect(() => {
		slug;
		replyTo = null;
	});

	function jump(id: string) {
		const el = document.getElementById(`p-${id}`);
		if (!el) return;
		el.scrollIntoView({ block: 'center', behavior: 'smooth' });
		el.classList.add('flash');
		setTimeout(() => el.classList.remove('flash'), 1400);
	}

	const toBottom = () => scroller && (scroller.scrollTop = scroller.scrollHeight);

	// Arriving in a thread lands on the post a search hit pointed at, or else the newest post; a new post
	// scrolls itself into view.
	let seenSlug = '';
	let seenCount = 0;
	$effect(() => {
		const count = posts.length;
		const target = page.url.searchParams.get('post');
		if (slug !== seenSlug) {
			seenSlug = slug;
			seenCount = count;
			tick().then(() => (target && document.getElementById(`p-${target}`) ? jump(target) : toBottom()));
		} else if (count > seenCount) {
			seenCount = count;
			tick().then(toBottom);
		} else {
			seenCount = count;
		}
	});
</script>

<div class="thread" bind:this={scroller}>
	<div class="thread-inner">
		{#each flow as item (item.post.id)}
			{#if item.newDay}<div class="divider">{dayLabel(item.day)}</div>{/if}
			{@const answers = structure.answers.get(item.post.id)}
			<PostCard {slug} post={item.post} {byId} onreply={(p) => (replyTo = p)} onjump={jump}>
				{#if answers?.length}
					<div class="answers">
						{#each answers as a (a.id)}
							<PostCard {slug} post={a} {byId} variant="answer" quotedElsewhere={item.post.id} onreply={(p) => (replyTo = p)} onjump={jump} />
						{/each}
					</div>
				{/if}
			</PostCard>
		{:else}
			<div class="empty-state">
				<h1>Nothing here yet</h1>
				<p>Write your first note below. Every note is saved on its own with its time, and you can reply to any of them.</p>
			</div>
		{/each}
	</div>
</div>

<Composer {slug} {replyTo} oncancelreply={() => (replyTo = null)} onsent={() => (replyTo = null)} />
