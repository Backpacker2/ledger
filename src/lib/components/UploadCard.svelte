<script lang="ts">
	import { api } from '$lib/api';
	import { formatSize } from '$lib/actions';
	import { time } from '$lib/format';
	import { attempt, confirmDialog, refresh } from '$lib/ui.svelte';
	import type { Upload } from '$lib/types';
	import FileImage from './FileImage.svelte';
	import Markdown from './Markdown.svelte';
	import MarkdownEditor from './MarkdownEditor.svelte';

	let { project, thread, upload }: { project: string; thread: string; upload: Upload } = $props();

	let editing = $state(false);
	let draft = $state('');
	let saving = $state(false);

	function startEdit() {
		draft = upload.description;
		editing = true;
	}

	async function save() {
		saving = true;
		const done = await attempt(() => api.updateUpload(project, thread, upload.id, draft));
		saving = false;
		if (!done) return;
		editing = false;
		await refresh();
	}

	async function remove() {
		const ok = await confirmDialog({
			title: `Remove “${upload.name}”?`,
			message: 'The file is deleted from the project’s attachments unless something else still uses it.',
			confirmLabel: 'Remove',
			danger: true
		});
		if (!ok) return;
		if ((await attempt(() => api.deleteUpload(project, thread, upload.id))) !== undefined) await refresh();
	}

	const open = (reveal: boolean) => attempt(() => api.openProjectFile(project, upload.file, reveal));
</script>

<div class="card upload-card" id="u-{upload.id}">
	<div class="meta">
		<span class="card-kind">Upload</span>
		<span class="time">{time(upload.uploadedAt)}</span>
	</div>

	{#if upload.exists && upload.image}
		<FileImage {project} path={upload.file} alt={upload.name} />
	{/if}
	<div class="file-row">
		<span class="fname" title={upload.file}>{upload.name}</span>
		<span class="fsize">{formatSize(upload.size)}</span>
		{#if upload.exists}
			<button class="link accent" onclick={() => open(false)}>Open</button>
			<button class="link" onclick={() => open(true)}>Show in folder</button>
		{:else}
			<span class="missing">File is missing</span>
		{/if}
	</div>

	{#if editing}
		<MarkdownEditor
			bind:value={draft}
			label="Description"
			placeholder="What is this file?"
			autofocus
			onsubmit={save}
			oncancel={() => (editing = false)}
		/>
		<div class="row-end">
			<button class="btn small ghost" onclick={() => (editing = false)}>Cancel</button>
			<button class="btn small primary" onclick={save} disabled={saving}>Save</button>
		</div>
	{:else}
		{#if upload.description.trim()}<Markdown html={upload.descriptionHtml} />{/if}
		<div class="actions">
			<button class="link" onclick={startEdit}>{upload.description.trim() ? 'Edit description' : 'Add description'}</button>
			<button class="link danger" onclick={remove}>Remove</button>
		</div>
	{/if}
</div>
