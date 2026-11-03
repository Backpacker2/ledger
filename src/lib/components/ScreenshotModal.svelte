<script lang="ts">
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api';
	import { newProjectFlow, threadUrl } from '$lib/actions';
	import { attempt, refresh, screenshot } from '$lib/ui.svelte';
	import type { ProjectSummary } from '$lib/types';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/**
	 * Asks where a freshly taken screenshot belongs: a project and a thread, plus a note about it. With a thread
	 * open it is a short confirmation (that thread, an optional note, Enter); "Change" shows the full choice.
	 */
	let { projects }: { projects: ProjectSummary[] } = $props();

	const NEW = '__new__';

	let src = $state<string | null>(null);
	let project = $state('');
	let thread = $state('');
	let title = $state('');
	let description = $state('');
	let busy = $state(false);
	/** Straight to the open thread: only the preview, where it goes and the description. */
	let compact = $state(false);

	const chosen = $derived(projects.find((p) => p.slug === project));
	const creating = $derived(thread === NEW);

	// A new screenshot: fetch the picture and start from where the person is looking. Only a new screenshot
	// restarts the form — not a project picked in it, nor a refresh of the project list.
	$effect(() => {
		const pending = screenshot.pending;
		if (!pending) return;
		let url: string | null = null;
		let stale = false;
		untrack(() => {
			src = null;
			description = '';
			title = '';
			const here = page.params.project;
			project = projects.find((p) => p.slug === here)?.slug ?? projects[0]?.slug ?? '';
			thread = defaultThread(project);
			compact = !!page.params.thread && thread === page.params.thread;
		});
		api.pendingScreenshotImage()
			.then((bytes) => {
				if (stale) return;
				url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }));
				src = url;
			})
			.catch(() => {});
		return () => {
			stale = true;
			// A moment later, not now: the <img> may still be on screen for the rest of this update.
			const old = url;
			if (old) setTimeout(() => URL.revokeObjectURL(old), 2000);
		};
	});

	function defaultThread(slug: string): string {
		const p = projects.find((q) => q.slug === slug);
		if (!p) return '';
		const here = page.params.thread;
		if (here && p.threads.some((t) => t.slug === here)) return here;
		return p.threads.length ? p.threads[0].slug : NEW;
	}

	function pickProject(slug: string) {
		project = slug;
		thread = defaultThread(slug);
	}

	const threadTitle = $derived(chosen?.threads.find((t) => t.slug === thread)?.title ?? thread);

	async function discard() {
		screenshot.pending = null;
		await attempt(() => api.discardScreenshot());
	}

	async function attach() {
		if (busy || !project) return;
		busy = true;
		let slug = thread;
		if (creating) {
			if (!title.trim()) {
				busy = false;
				return;
			}
			const made = await attempt(() => api.createThread(project, title.trim()));
			if (!made) {
				busy = false;
				return;
			}
			slug = made.slug;
		}
		const upload = await attempt(() => api.attachScreenshot(project, slug, description.trim()));
		busy = false;
		if (!upload) return;
		screenshot.pending = null;
		await refresh();
		await goto(`${threadUrl(project, slug)}?item=u-${upload.id}`);
	}
</script>

{#if screenshot.pending}
	<Modal title="Attach screenshot" onclose={discard} wide>
		{#if src}<img class="shot-preview" {src} alt="The screenshot you just took" />{/if}

		{#if projects.length === 0}
			<p class="notice">A screenshot is attached to a thread in a project, and there are no projects yet.</p>
			<div class="row-end">
				<button
					class="btn"
					onclick={async () => {
						await newProjectFlow();
					}}>New project</button
				>
			</div>
		{:else if compact}
			<div class="shot-target">
				<span>Attach to <strong>{chosen?.name}</strong> › <strong>{threadTitle}</strong></span>
				<button class="btn ghost" onclick={() => (compact = false)}>Change…</button>
			</div>
			<!-- Enter attaches; Shift+Enter is a new line in the description. -->
			<div class="field">
				Description (optional)
				<MarkdownEditor
					bind:value={description}
					rows={2}
					label="Description"
					placeholder="What does this show? Markdown works."
					autofocus
					submitOnEnter
					onsubmit={attach}
				/>
			</div>
		{:else}
			<div class="form-row">
				<label class="field grow">
					Project
					<select value={project} onchange={(e) => pickProject(e.currentTarget.value)}>
						{#each projects as p (p.slug)}<option value={p.slug}>{p.name}</option>{/each}
					</select>
				</label>
				<label class="field grow">
					Thread
					<select bind:value={thread}>
						{#each chosen?.threads ?? [] as t (t.slug)}<option value={t.slug}>{t.title}</option>{/each}
						<option value={NEW}>+ New thread…</option>
					</select>
				</label>
			</div>
			{#if creating}
				<label class="field">
					New thread title
					<input bind:value={title} placeholder="e.g. Checkout flow" onkeydown={(e) => e.key === 'Enter' && attach()} />
				</label>
			{/if}
			<div class="field">
				Description (optional)
				<MarkdownEditor bind:value={description} label="Description" placeholder="What does this show? Markdown works." submitOnEnter onsubmit={attach} />
			</div>
		{/if}

		{#snippet footer()}
			<span class="hint">Enter attaches · Shift+Enter new line · Esc discards</span>
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={discard}>Discard</button>
			<button class="btn primary" onclick={attach} disabled={busy || !project || (creating && !title.trim())}>Attach</button>
		{/snippet}
	</Modal>
{/if}
