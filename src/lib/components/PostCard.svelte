<script lang="ts">
	import type { Snippet } from 'svelte';
	import { api } from '$lib/api';
	import { deliverableUrl, snapshotPath, threadUrl } from '$lib/actions';
	import { dayKey, dayLabel, plainText, time } from '$lib/format';
	import { attempt, confirmDialog, refresh } from '$lib/ui.svelte';
	import type { Post } from '$lib/types';
	import Markdown from './Markdown.svelte';
	import MarkdownEditor from './MarkdownEditor.svelte';

	let {
		project,
		thread,
		post,
		byId,
		variant = 'card',
		quotedElsewhere,
		onreply,
		onjump,
		children
	}: {
		project: string;
		thread: string;
		post: Post;
		byId: Record<string, Post>;
		/** "answer" is a reply shown inside the guiding-question card it belongs to. */
		variant?: 'card' | 'answer';
		/** A parent that is already visible as this card's container; no quote needed. */
		quotedElsewhere?: string;
		onreply: (post: Post) => void;
		onjump: (domId: string) => void;
		children?: Snippet;
	} = $props();

	let editing = $state(false);
	let draft = $state('');
	let saving = $state(false);

	const parent = $derived(post.parentId ? byId[post.parentId] : undefined);
	const showQuote = $derived(!!post.parentId && post.parentId !== quotedElsewhere);

	// Seeded questions read "**Guiding question (Template):** …"; the card's label already says as much, so show
	// the template there and only the question in the body.
	const GUIDING_PREFIX = /^<p><strong>Guiding question \(([^)]*)\):<\/strong>\s*/;
	const guidingType = $derived.by(() => {
		const match = post.guidingQuestion ? post.html.match(GUIDING_PREFIX) : null;
		return match ? (new DOMParser().parseFromString(match[1], 'text/html').documentElement.textContent ?? '') : '';
	});
	const html = $derived(guidingType ? post.html.replace(GUIDING_PREFIX, '<p>') : post.html);

	function startEdit() {
		draft = post.body;
		editing = true;
	}

	async function save() {
		if (!draft.trim()) return;
		saving = true;
		const done = await attempt(() => api.updatePost(project, thread, post.id, draft));
		saving = false;
		if (done) {
			editing = false;
			await refresh();
		}
	}

	async function remove() {
		const ok = await confirmDialog({
			title: 'Delete this post?',
			message: 'Replies to it stay in the thread.',
			confirmLabel: 'Delete',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deletePost(project, thread, post.id))) !== undefined) await refresh();
	}
</script>

<div
	class={variant === 'answer' ? 'answer' : 'card'}
	class:qcard={variant === 'card' && post.guidingQuestion}
	class:note-card={variant === 'card' && post.versionPublished}
	id="p-{post.id}"
>
	<div class="meta">
		{#if variant === 'card' && post.guidingQuestion}
			<span class="qlabel">Guiding question{guidingType ? ` · ${guidingType}` : ''}</span>
		{:else if variant === 'card' && post.versionPublished}
			<span class="qlabel">Version published</span>
		{/if}
		<span class="time">{time(post.createdAt)}</span>
		{#if variant === 'answer'}<span class="file">{dayLabel(dayKey(post.createdAt))}</span>{/if}
		{#if post.updatedAt !== post.createdAt}<span class="file">edited</span>{/if}
	</div>

	{#if showQuote}
		{#if parent}
			<button class="quote" onclick={() => onjump(`p-${parent.id}`)}>
				<span class="qhead">Reply to {time(parent.createdAt)}</span>
				<span class="qtext">{plainText(parent.body)}</span>
			</button>
		{:else}
			<div class="quote gone"><span class="qhead">Reply to a deleted post</span></div>
		{/if}
	{/if}

	{#if editing}
		<MarkdownEditor bind:value={draft} label="Post" autofocus onsubmit={save} oncancel={() => (editing = false)} />
		<div class="row-end">
			<span class="hint" style="align-self:center">Ctrl+Enter saves</span>
			<button class="btn small ghost" onclick={() => (editing = false)}>Cancel</button>
			<button class="btn small primary" onclick={save} disabled={saving || !draft.trim()}>Save</button>
		</div>
	{:else}
		<Markdown {html} />
		{@render children?.()}
		{#if post.versionPublished && post.promptId}
			<div class="note-from">
				{#if post.promptTitle !== null && post.promptThread}
					{#if post.promptThread === thread}
						<button class="link accent" onclick={() => onjump(`pr-${post.promptId}`)}>From prompt: {post.promptTitle}</button>
					{:else}
						<a class="link accent" href="{threadUrl(project, post.promptThread)}?item=pr-{post.promptId}">From prompt: {post.promptTitle}</a>
					{/if}
				{:else}
					<span class="hint">From a prompt that has since been deleted</span>
				{/if}
			</div>
		{/if}
		<div class="actions">
			<button class="link accent" onclick={() => onreply(post)}>{post.guidingQuestion ? 'Answer' : 'Reply'}</button>
			{#if post.versionPublished && post.deliverablePath && post.deliverableVersion}
				<a class="link accent" href={deliverableUrl(project, snapshotPath(post.deliverablePath, post.deliverableVersion))}>
					View version {post.deliverableVersion}
				</a>
			{:else}
				<button class="link" onclick={startEdit}>Edit</button>
			{/if}
			<button class="link danger" onclick={remove}>Delete</button>
		</div>
	{/if}
</div>
