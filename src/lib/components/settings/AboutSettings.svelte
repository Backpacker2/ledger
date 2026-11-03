<script lang="ts">
	// About Ledger: what it is, where its code lives, and this build on this machine.
	import { onMount } from 'svelte';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Wordmark from '$lib/components/Wordmark.svelte';
	import { logApi } from '$lib/api';
	import { REPO_URL } from '$lib/report';
	import type { AboutInfo, Settings } from '$lib/types';

	let { settings }: { settings: Settings } = $props();

	let about = $state<AboutInfo | null>(null);
	onMount(() => {
		logApi.about().then((a) => (about = a), () => {});
	});

	const links = [
		{ label: 'Source code', href: REPO_URL },
		{ label: 'Releases and changes', href: `${REPO_URL}/releases` },
		{ label: 'Known problems', href: `${REPO_URL}/issues` }
	];
</script>

<div class="panel">
	<div class="about-head">
		<Wordmark height={30} on="page" />
		<span class="badge accent">v{settings.version}</span>
	</div>
	<p class="sub" style="margin:0">
		Keep your thoughts separate from the artifacts you use to prove what you did. Write in <b>threads</b>, ask for <b>deliverables</b> and give feedback
		on them, and plan the work on a <b>board</b>, all inside <b>projects</b>. Everything is plain markdown in a folder you own, optionally mirrored to a
		private GitHub repository. AI can read your thoughts but never writes them.
	</p>
	<div class="row-end" style="justify-content:flex-start">
		{#each links as l (l.href)}
			<button class="btn small" onclick={() => openUrl(l.href).catch(() => {})}><ExternalLink size={13} />{l.label}</button>
		{/each}
	</div>

	<div class="sync-detail">
		<h3 style="font-size:14px">This machine</h3>
		<div class="kv">
			<dt>Build</dt>
			<dd>Ledger {settings.version}{#if about} · Tauri {about.tauriVersion} · {about.os} {about.arch}{/if}</dd>
			<dt>Vault</dt>
			<dd class="mono">{settings.vaultPath}</dd>
			<dt>Settings</dt>
			<dd class="mono">{settings.settingsFile}</dd>
			{#if about}
				<dt>App config</dt>
				<dd class="mono">{about.configDir}</dd>
				<dt>Log</dt>
				<dd class="mono">{about.logFile}</dd>
			{/if}
		</div>
		<div class="hint">Built with Tauri 2 (Rust) and SvelteKit (Svelte 5).</div>
	</div>
</div>

<style>
	.about-head {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.btn :global(.lucide-icon) {
		margin-right: 4px;
		vertical-align: -2px;
	}
</style>
