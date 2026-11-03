<script lang="ts">
	// "Yell at the app": the latest error, ready to report as a GitHub issue (filled in, checked and sent by you in
	// the browser) or to copy as a prompt for an AI assistant.
	import { onMount } from 'svelte';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import Bug from '@lucide/svelte/icons/bug';
	import Bot from '@lucide/svelte/icons/bot';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { logApi } from '$lib/api';
	import { ago, dateTime } from '$lib/format';
	import { appLog } from '$lib/log.svelte';
	import { aiPrompt, issueBody, issueLink, REPO_URL } from '$lib/report';
	import { toast } from '$lib/ui.svelte';
	import type { AboutInfo, LogEntry } from '$lib/types';

	let about = $state<AboutInfo | null>(null);
	let recent = $state<LogEntry[]>([]);

	const latest = $derived(appLog.latestError);

	async function load() {
		recent = await logApi.read(60).catch(() => []);
	}

	onMount(() => {
		logApi.about().then((a) => (about = a), () => {});
	});
	// The log just before the error goes with the report, so it is read again when a new entry comes in.
	$effect(() => {
		void appLog.changes;
		load();
	});

	/** The entries up to and including the error being reported. */
	const before = $derived.by(() => {
		if (!latest) return recent;
		const i = recent.findLastIndex((e) => e.at === latest.at && e.message === latest.message);
		return i >= 0 ? recent.slice(0, i + 1) : recent;
	});

	async function openIssue() {
		if (!latest) return openUrl(`${REPO_URL}/issues/new`).catch(() => toast('Could not open the browser', true));
		const link = issueLink(latest, about, before);
		// Too long for a link: the whole report goes to the clipboard, and the issue opens shortened, asking for it.
		if (!link.complete) {
			try {
				await navigator.clipboard.writeText(link.body);
				toast('The report is too long for a link. It is on your clipboard: paste it into the issue.');
			} catch {
				toast('The report is too long for a link and could not be copied. Use “Copy log” below.', true);
			}
		}
		openUrl(link.url).catch(() => toast('Could not open the browser', true));
	}

	async function copyPrompt() {
		if (!latest) return;
		try {
			await navigator.clipboard.writeText(aiPrompt(latest, about, before));
			toast('Copied. Paste it into Claude or another assistant.');
		} catch {
			toast('Could not copy to the clipboard', true);
		}
	}
</script>

<div class="panel">
	<h2>Yell at the app</h2>
	<p class="sub" style="margin:0">
		Something broke? Send the latest error to Ledger’s GitHub as an issue, or copy it as a prompt to ask an AI assistant what went wrong.
	</p>

	{#if latest}
		<div class="notice bad">
			<div class="hint" style="margin-bottom:4px">
				<b>{latest.source}</b> · <span title={dateTime(latest.at)}>{ago(latest.at)}</span>
			</div>
			<div class="yell-error">{latest.message}</div>
		</div>
	{:else}
		<div class="notice ok">No errors logged. Nothing to yell about.</div>
	{/if}

	<div class="row-end" style="justify-content:flex-start">
		<button class="btn primary" onclick={openIssue}><Bug size={14} />{latest ? 'Report on GitHub' : 'Open an issue'}</button>
		<button class="btn" onclick={copyPrompt} disabled={!latest}><Bot size={14} />Copy as AI prompt</button>
	</div>
	<div class="hint">
		The issue opens in your browser, filled in, and nothing is posted until you submit it there. Ledger’s repository is <b>public</b>, so read it first:
		your home folder and anything shaped like a token or recovery key are taken out, but names of projects and files in the error stay.
	</div>

	{#if latest}
		<details class="more">
			<summary><ChevronRight size={14} class="disclosure" />What the issue will say</summary>
			<pre class="mono tree" style="white-space:pre-wrap;word-break:break-word">{issueBody(latest, about, before)}</pre>
		</details>
	{/if}
</div>

<style>
	.btn :global(.lucide-icon) {
		margin-right: 6px;
		vertical-align: -2px;
	}
</style>
