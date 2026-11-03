<script lang="ts">
	import { api } from '$lib/api';
	import { attempt, refresh, toast } from '$lib/ui.svelte';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/** Files picked in the file dialog, waiting for a description before they are copied into the project. */
	let { project, thread, paths, onclose }: { project: string; thread: string; paths: string[]; onclose: () => void } = $props();

	let description = $state('');
	let saving = $state(false);

	const nameOf = (path: string) => path.split(/[\\/]/).pop() ?? path;

	async function save() {
		saving = true;
		const added = await attempt(() => api.uploadFiles(project, thread, paths, description.trim()));
		saving = false;
		if (!added) return;
		toast(added.length === 1 ? `Uploaded ${added[0].name}` : `Uploaded ${added.length} files`);
		await refresh();
		onclose();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title={paths.length === 1 ? 'Upload file' : `Upload ${paths.length} files`} {onclose}>
	<ul style="margin:0;padding-left:18px">
		{#each paths as p (p)}<li class="mono" title={p}>{nameOf(p)}</li>{/each}
	</ul>
	<div class="field">
		Description (optional{paths.length > 1 ? ', shared by all of them' : ''})
		<MarkdownEditor bind:value={description} rows={4} label="Description" placeholder="What is this? Markdown works." autofocus />
	</div>
	<p class="hint" style="margin:0">Files are copied into the project’s attachments folder, and noted in this thread.</p>

	{#snippet footer()}
		<span class="spacer-h"></span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving}>Upload</button>
	{/snippet}
</Modal>
