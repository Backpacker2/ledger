<script lang="ts">
	import { api } from '$lib/api';
	import { newThreadFlow, threadUrl } from '$lib/actions';
	import Markdown from '$lib/components/Markdown.svelte';
	import MarkdownEditor from '$lib/components/MarkdownEditor.svelte';
	import { ago, plural } from '$lib/format';
	import { orderThreads } from '$lib/order';
	import { listOrder } from '$lib/order.svelte';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const project = $derived(data.project);
	// The guide is the project's README.md: pictures in it are looked for from the project's folder.
	const images = $derived({ project: project.slug, dir: '' });
	const threads = $derived(orderThreads(data.projects.find((p) => p.slug === project.slug)?.threads ?? [], listOrder.order));

	let editing = $state(false);
	let draft = $state('');
	let saving = $state(false);

	function edit() {
		draft = project.readme;
		editing = true;
	}

	async function save() {
		saving = true;
		const done = await attempt(() => api.saveReadme(project.slug, draft));
		saving = false;
		if (!done) return;
		editing = false;
		await refresh();
	}
</script>

<div class="page">
	<div class="page-inner wide">
		<div class="cols-2">
			<section class="panel">
				<div class="header-top">
					<div>
						<h2>Reading guide</h2>
						<div class="sub">README.md — where to start, what matters, how this project works.</div>
					</div>
					{#if !editing}
						<button class="btn small" onclick={edit}>Edit</button>
					{/if}
				</div>
				{#if editing}
					<MarkdownEditor
						bind:value={draft}
						variant="doc"
						minHeight="58vh"
						label="Reading guide"
						autofocus
						spellcheck={false}
						{images}
						onsubmit={save}
						oncancel={() => (editing = false)}
					/>
					<div class="row-end">
						<span class="hint" style="align-self:center">Ctrl+Enter saves</span>
						<button class="btn ghost" onclick={() => (editing = false)}>Cancel</button>
						<button class="btn primary" onclick={save} disabled={saving}>Save</button>
					</div>
				{:else if project.readme.trim()}
					<Markdown html={project.readmeHtml} {images} />
				{:else}
					<p class="hint" style="margin:0">The guide is empty. Write down what a newcomer should read first.</p>
				{/if}
			</section>

			<aside class="panel">
				<div class="header-top">
					<h2>Threads</h2>
					<button class="btn small" onclick={() => newThreadFlow(project.slug)}>New</button>
				</div>
				<div class="mini-list">
					{#each threads as t (t.slug)}
						<a href={threadUrl(project.slug, t.slug)}>
							<span class="t">{t.title}</span>
							<span class="s">{ago(t.lastActivity)}</span>
						</a>
					{:else}
						<span class="hint">No threads yet. A thread is a running conversation about one topic.</span>
					{/each}
				</div>
				<div class="stat-row">
					<span class="badge">{plural(project.deliverableCount, 'deliverable')}</span>
					<span class="badge">{plural(project.storyCount, 'story', 'stories')}</span>
					<span class="badge">{project.openTaskCount} of {plural(project.taskCount, 'task')} open</span>
				</div>
			</aside>
		</div>
	</div>
</div>
