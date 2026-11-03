<script lang="ts">
	// Debug capture: record every step for a while, then send the file to the developer.
	import { onMount } from 'svelte';
	import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
	import Bot from '@lucide/svelte/icons/bot';
	import Bug from '@lucide/svelte/icons/bug';
	import CircleDot from '@lucide/svelte/icons/circle-dot';
	import CircleStop from '@lucide/svelte/icons/circle-stop';
	import FolderOpen from '@lucide/svelte/icons/folder-open';
	import { logApi } from '$lib/api';
	import { ago, dateTime } from '$lib/format';
	import { capture, startCapture, stopCapture } from '$lib/log.svelte';
	import { captureAiPrompt, captureIssueUrl } from '$lib/report';
	import { attempt, toast } from '$lib/ui.svelte';
	import type { AboutInfo } from '$lib/types';

	let about = $state<AboutInfo | null>(null);
	onMount(() => {
		logApi.about().then((a) => (about = a), () => {});
	});

	const last = $derived(capture.last);
	const errors = $derived(last ? last.entries.filter((e) => e.level === 'error').length : 0);
	const name = $derived(last ? (last.path.split(/[\\/]/).pop() ?? last.path) : '');

	async function start() {
		if ((await attempt(startCapture)) !== undefined) toast('Recording. Now do what goes wrong, then press Stop.');
	}

	async function stop() {
		await attempt(stopCapture);
	}

	function showFile() {
		if (last) revealItemInDir(last.path).catch(() => toast('Could not open the folder', true));
	}

	function report() {
		if (!last) return;
		showFile();
		openUrl(captureIssueUrl(last.path, last.entries, about)).catch(() => toast('Could not open the browser', true));
		toast(`Drag ${name} from the folder into the issue.`);
	}

	async function copyPrompt() {
		if (!last) return;
		try {
			await navigator.clipboard.writeText(captureAiPrompt(last.text));
			toast('Copied. Paste it into Claude or another assistant.');
		} catch {
			toast('Could not copy to the clipboard', true);
		}
	}
</script>

<div class="panel">
	<h2>Debug capture</h2>
	<p class="sub" style="margin:0">
		Something goes wrong and you can make it happen again? Start a capture, do it, and stop. Ledger records every step in between and saves it as a
		file you can send to the developer.
	</p>

	{#if capture.active}
		<div class="notice bad">
			<strong>Recording</strong>
			{#if capture.startedAt}since <span title={dateTime(capture.startedAt)}>{ago(capture.startedAt)}</span>{/if} · {capture.events} steps so far. Go and
			do what goes wrong; stop here or with the bar at the top.
		</div>
		<div class="row-end" style="justify-content:flex-start">
			<button class="btn primary" onclick={stop} disabled={capture.busy}><CircleStop size={14} />Stop capture</button>
		</div>
	{:else}
		<div class="row-end" style="justify-content:flex-start">
			<button class="btn primary" onclick={start} disabled={capture.busy}><CircleDot size={14} />Start capture</button>
		</div>
	{/if}

	<div class="hint">
		It records each action Ledger’s window asks for (its name, whether it worked and how long it took), each page you open, each message Ledger shows
		you, sync and Trello rounds, and every error. Not what you type, nor what the actions carry. Your home folder and anything shaped like a token are
		taken out of the file; names of projects and threads are in it. A capture stops growing at 5 MB, and one that a crash cut short carries on when
		Ledger starts again.
	</div>

	{#if last && !capture.active}
		<div class="sync-detail">
			<h3 style="font-size:14px">Last capture</h3>
			<div class="kv">
				<dt>File</dt>
				<dd class="mono">{last.path}</dd>
				<dt>Recorded</dt>
				<dd>{last.entries.length} steps, {errors} {errors === 1 ? 'error' : 'errors'}{last.startedAt ? ` · ${dateTime(last.startedAt)}` : ''}</dd>
			</div>
			<div class="row-end" style="justify-content:flex-start">
				<button class="btn primary" onclick={report}><Bug size={14} />Send to the developer</button>
				<button class="btn" onclick={copyPrompt}><Bot size={14} />Copy as AI prompt</button>
				<button class="btn" onclick={showFile}><FolderOpen size={14} />Show the file</button>
			</div>
			<div class="hint">
				<b>Send to the developer</b> opens a new issue on Ledger’s GitHub and the folder with the file in it: drag the file into the issue, check it,
				and submit. The repository is public, so look the file over first.
			</div>
		</div>
	{/if}
</div>

<style>
	.btn :global(.lucide-icon) {
		margin-right: 6px;
		vertical-align: -2px;
	}
</style>
