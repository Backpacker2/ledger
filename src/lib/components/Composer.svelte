<script lang="ts">
	import { api } from '$lib/api';
	import { plainText, time } from '$lib/format';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { Post } from '$lib/types';

	let {
		slug,
		replyTo,
		oncancelreply,
		onsent
	}: { slug: string; replyTo: Post | null; oncancelreply: () => void; onsent: () => void } = $props();

	let text = $state('');
	let sending = $state(false);
	let input: HTMLTextAreaElement;

	// Replying focuses the box; so does arriving in a thread.
	$effect(() => {
		slug;
		replyTo;
		input?.focus();
	});

	function grow() {
		input.style.height = 'auto';
		input.style.height = `${input.scrollHeight}px`;
		input.style.overflowY = input.scrollHeight > 220 ? 'auto' : 'hidden';
	}

	async function send() {
		const body = text.trim();
		if (!body || sending) return;
		sending = true;
		const post = await attempt(() => api.addPost(slug, body, replyTo?.id ?? null));
		sending = false;
		if (!post) return;
		text = '';
		queueMicrotask(grow);
		await refresh();
		onsent();
		input.focus();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
			e.preventDefault();
			send();
		}
		if (e.key === 'Escape' && replyTo) oncancelreply();
	}
</script>

<div class="composer">
	{#if replyTo}
		<div class="reply-bar">
			<span><b>Replying to {time(replyTo.createdAt)}:</b> {plainText(replyTo.body, 160)}</span>
			<button class="link" aria-label="Cancel reply" onclick={oncancelreply}>✕</button>
		</div>
	{/if}
	<div class="input-row">
		<textarea
			bind:this={input}
			bind:value={text}
			oninput={grow}
			{onkeydown}
			rows="1"
			placeholder="Write a note… markdown works (Enter saves, Shift+Enter adds a line)"
		></textarea>
		<button class="btn primary" onclick={send} disabled={sending || !text.trim()}>Save</button>
	</div>
</div>
