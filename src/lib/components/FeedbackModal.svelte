<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { snapshotPath } from '$lib/actions';
	import { orderVersions } from '$lib/order';
	import { listOrder } from '$lib/order.svelte';
	import { attempt, refresh } from '$lib/ui.svelte';
	import type { DeliverableSummary } from '$lib/types';
	import MarkdownEditor from './MarkdownEditor.svelte';
	import Modal from './Modal.svelte';

	/** Feedback lands on a deliverable — its current file, or any version that was published. */
	let { project, thread, path = '', onclose }: { project: string; thread: string; path?: string; onclose: () => void } = $props();

	/** Read once: the modal is mounted fresh each time. */
	const start = () => path;

	let all = $state<DeliverableSummary[]>([]);
	let loaded = $state(false);
	let target = $state(start());
	let body = $state('');
	let saving = $state(false);

	const options = $derived(
		all.flatMap((d) => [
			...(d.exists ? [{ value: d.path, label: d.path }] : []),
			...orderVersions(d.versions, listOrder.order).map((v) => ({ value: snapshotPath(d.path, v), label: `${d.path} · version ${v}` }))
		])
	);

	onMount(async () => {
		all = (await attempt(() => api.listDeliverables(project))) ?? [];
		loaded = true;
		if (!options.some((o) => o.value === target)) target = options[0]?.value ?? '';
	});

	async function save() {
		saving = true;
		const saved = await attempt(() => api.addFeedback(project, thread, target, body));
		saving = false;
		if (!saved) return;
		await refresh();
		onclose();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
	}
</script>

<svelte:window {onkeydown} />

<Modal title="Feedback on a deliverable" wide {onclose}>
	{#if loaded && options.length === 0}
		<p class="notice">This project has no deliverables yet. Create one from a prompt first.</p>
	{:else}
		<label class="field">
			Deliverable
			<select bind:value={target}>
				{#each options as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
			</select>
		</label>
		<div class="field">
			Your feedback
			<MarkdownEditor bind:value={body} rows={8} label="Your feedback" placeholder="What works, what doesn’t, what to change. Markdown works." />
		</div>
	{/if}

	{#snippet footer()}
		<span class="spacer-h"></span>
		<span class="hint">Ctrl+Enter saves</span>
		<button class="btn ghost" onclick={onclose}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={saving || !target || !body.trim()}>Add feedback</button>
	{/snippet}
</Modal>
