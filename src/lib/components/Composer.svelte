<script lang="ts">
	import X from '@lucide/svelte/icons/x';
	import { open } from '@tauri-apps/plugin-dialog';
	import { api } from '$lib/api';
	import { plainText, time } from '$lib/format';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { Post } from '$lib/types';

	let {
		project,
		thread,
		replyTo,
		oncancelreply,
		onsent,
		onprompt,
		onfeedback,
		onupload
	}: {
		project: string;
		thread: string;
		replyTo: Post | null;
		oncancelreply: () => void;
		onsent: () => void;
		onprompt: () => void;
		onfeedback: () => void;
		/** Called with the paths of the files the person picked. */
		onupload: (paths: string[]) => void;
	} = $props();

	let text = $state('');
	let sending = $state(false);
	let input: HTMLTextAreaElement;

	// Replying focuses the box; so does arriving in a thread.
	$effect(() => {
		project;
		thread;
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
		const post = await attempt(() => api.addPost(project, thread, body, replyTo?.id ?? null));
		sending = false;
		if (!post) return;
		text = '';
		queueMicrotask(grow);
		await refresh();
		onsent();
		input.focus();
	}

	async function pickFiles() {
		const picked = await attempt(() => open({ multiple: true, title: 'Upload files to this thread' }));
		if (!picked) return;
		const paths = Array.isArray(picked) ? picked : [picked];
		if (paths.length) onupload(paths);
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
			<button class="link" aria-label="Cancel reply" onclick={oncancelreply}><X size={14} /></button>
		</div>
	{/if}
	<div class="toolbar">
		<button class="btn small chip-ai" onclick={onprompt} title="Ask for a deliverable">Prompt…</button>
		<button class="btn small chip-own" onclick={onfeedback} title="Give feedback on a deliverable">Feedback…</button>
		<button class="btn small" onclick={pickFiles} title="Attach files to this thread">Upload files…</button>
	</div>
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
