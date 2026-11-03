<script lang="ts">
	// Where the app's log is, and how big it may get. It holds every error (a failed action on any screen, a sync or
	// Trello round that went wrong, a crash) and is kept on this machine in the app's log folder, never in the vault.
	import { onMount } from 'svelte';
	import { revealItemInDir } from '@tauri-apps/plugin-opener';
	import FolderOpen from '@lucide/svelte/icons/folder-open';
	import { logApi } from '$lib/api';
	import { appLog } from '$lib/log.svelte';
	import { attempt, confirmDialog, toast } from '$lib/ui.svelte';
	import type { AboutInfo } from '$lib/types';

	const KB = 1024;
	const MB = 1024 * KB;
	/** The sizes the slider stops at; 1 MB is the default. */
	const sizes = [256 * KB, 512 * KB, 1 * MB, 2 * MB, 5 * MB, 10 * MB, 20 * MB, 50 * MB];
	const label = (bytes: number) => (bytes < MB ? `${Math.round(bytes / KB)} KB` : `${Math.round(bytes / MB)} MB`);
	/** The stop closest to `bytes`. */
	const stopFor = (bytes: number) => sizes.reduce((best, s, i) => (Math.abs(s - bytes) < Math.abs(sizes[best] - bytes) ? i : best), 0);

	let about = $state<AboutInfo | null>(null);
	let stop = $state(stopFor(MB));

	onMount(() => {
		logApi.about().then(
			(a) => {
				about = a;
				stop = stopFor(a.logMaxBytes);
			},
			() => {}
		);
	});

	async function saveSize() {
		const set = await attempt(() => logApi.setMaxBytes(sizes[stop]));
		if (set !== undefined) toast(`The log keeps up to ${label(set)}`);
	}

	function openFolder() {
		if (about) revealItemInDir(about.logFile).catch(() => toast('Could not open the folder', true));
	}

	async function clear() {
		const ok = await confirmDialog({ title: 'Clear the log?', message: 'Every entry is removed from this machine.', confirmLabel: 'Clear', danger: true });
		if (!ok) return;
		if ((await attempt(() => logApi.clear())) !== undefined) {
			appLog.latestError = null;
			toast('Log cleared');
		}
	}
</script>

<div class="panel">
	<h2>Log</h2>
	<p class="sub" style="margin:0">
		Every error lands in the log: a failed action on any screen, a sync or Trello round that went wrong, a crash. It is kept on this machine only and
		never synced.
	</p>

	<div class="field">
		<span>Keep up to <b>{label(sizes[stop])}</b></span>
		<input class="log-size" type="range" min="0" max={sizes.length - 1} step="1" bind:value={stop} onchange={saveSize} aria-label="Most the log may take" disabled={!about} />
		<div class="log-size-scale"><span>{label(sizes[0])}</span><span>{label(sizes[sizes.length - 1])}</span></div>
		<span class="hint">When the log is half full, the older half is set aside and then dropped, so it never takes more than this.</span>
	</div>

	<div class="row-end" style="justify-content:flex-start">
		<button class="btn" onclick={openFolder} disabled={!about}><FolderOpen size={14} />Open the log folder</button>
		<button class="btn ghost danger" onclick={clear}>Clear log</button>
	</div>
	{#if about}<div class="hint mono">{about.logFile}</div>{/if}
</div>

<style>
	.btn :global(.lucide-icon) {
		margin-right: 6px;
		vertical-align: -2px;
	}
	.log-size {
		width: 100%;
		max-width: 420px;
		accent-color: var(--accent);
	}
	.log-size-scale {
		display: flex;
		justify-content: space-between;
		max-width: 420px;
		font-size: 11px;
		color: var(--faint);
	}
</style>
