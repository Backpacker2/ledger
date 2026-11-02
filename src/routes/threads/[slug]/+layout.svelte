<script lang="ts">
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Modal from '$lib/components/Modal.svelte';
	import { api } from '$lib/api';
	import { date, plural } from '$lib/format';
	import { attempt, confirmDialog, promptDialog, refresh } from '$lib/ui.svelte';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const thread = $derived(data.thread);
	const base = $derived(`/threads/${thread.slug}`);
	const path = $derived(page.url.pathname);
	const others = $derived(data.threads.filter((t) => t.slug !== thread.slug));
	const linked = $derived(new Set(thread.linkedThreads.map((t) => t.slug)));
	const lastActivity = $derived(thread.posts.at(-1)?.createdAt ?? thread.createdAt);

	let linking = $state(false);

	async function rename() {
		const title = await promptDialog({ title: 'Rename thread', label: 'Title', initial: thread.title, confirmLabel: 'Rename' });
		if (title && title !== thread.title && (await attempt(() => api.renameThread(thread.slug, title)))) await refresh();
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Delete “${thread.title}”?`,
			message: `Its ${plural(thread.posts.length, 'post')} will be deleted. Its documents are kept, just unlinked, and its user stories lose the link.`,
			confirmLabel: 'Delete thread',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteThread(thread.slug))) !== undefined) {
			await refresh();
			await goto('/');
		}
	}

	async function toggleLink(slug: string, on: boolean) {
		const done = await attempt(() => (on ? api.linkThreads(thread.slug, slug) : api.unlinkThreads(thread.slug, slug)));
		if (done !== undefined) await refresh();
	}
</script>

<svelte:head><title>{thread.title} · Ledger</title></svelte:head>

<div class="header-wrap">
	<div class="header-top">
		<div>
			<h1>{thread.title} <span class="badge accent">Thread</span></h1>
			<div class="sub">
				{plural(thread.posts.length, 'post')} · {plural(thread.documents.length, 'document')} ·
				{plural(thread.stories.length, 'user story', 'user stories')} · last activity {date(lastActivity)}
			</div>
		</div>
		<div class="header-actions">
			<button class="btn ghost small" onclick={rename}>Rename</button>
			<button class="btn ghost small" onclick={() => (linking = true)} disabled={others.length === 0}>Link threads…</button>
			<button class="btn ghost small danger" onclick={remove}>Delete</button>
		</div>
	</div>

	{#if thread.linkedThreads.length}
		<div class="subline">
			<span>Linked:</span>
			<div class="chips">
				{#each thread.linkedThreads as t (t.slug)}
					<a class="chip" href="/threads/{t.slug}">{t.title}</a>
				{/each}
			</div>
		</div>
	{/if}

	<nav class="tabs">
		<a class="tab" class:active={path === base} href={base}>Posts<span class="n">{thread.posts.length}</span></a>
		<a class="tab" class:active={path === `${base}/documents`} href="{base}/documents">Documents<span class="n">{thread.documents.length}</span></a>
		<a class="tab" class:active={path === `${base}/stories`} href="{base}/stories">User stories<span class="n">{thread.stories.length}</span></a>
	</nav>
</div>

<div style="display: contents">{@render children()}</div>

{#if linking}
	<Modal title="Link threads" onclose={() => (linking = false)}>
		<p style="margin:0;color:var(--muted)">Linked threads belong together — for example several threads feeding one document. Links work both ways.</p>
		<div class="link-grid">
			{#each others as t (t.slug)}
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
