<script lang="ts">
	// Updating Ledger from inside the app: whether it looks by itself, and looking now.
	import { onMount } from 'svelte';
	import Switch from '$lib/components/Switch.svelte';
	import { updatesApi } from '$lib/api';
	import { ago, dateTime } from '$lib/format';
	import { attempt } from '$lib/ui.svelte';
	import { showUpdate, updates } from '$lib/updates.svelte';
	import type { UpdateView } from '$lib/types';

	let view = $state<UpdateView | null>(null);
	let checking = $state(false);
	let result = $state<{ kind: 'ok' | 'info' | 'bad'; message: string } | null>(null);

	onMount(async () => {
		view = (await attempt(() => updatesApi.status())) ?? null;
	});

	const available = $derived(updates.available);
	const statusText = $derived.by(() => {
		if (!view) return '';
		if (available) return `Ledger ${available.version} is available${available.simulated ? ' (simulated)' : ''}`;
		if (view.unavailable) return 'This build doesn’t look for updates';
		if (view.checkedAt) return 'Up to date';
		return view.automatic ? 'Looks shortly after starting, then every few hours' : 'Automatic checks are off';
	});

	async function toggle(automatic: boolean) {
		const saved = await attempt(() => updatesApi.saveSettings(automatic));
		if (saved) view = saved;
	}

	async function check() {
		checking = true;
		result = null;
		try {
			const found = await updatesApi.check();
			result = { kind: view?.unavailable ? 'info' : 'ok', message: found.message };
			if (found.available) showUpdate(found.available);
		} catch (e) {
			result = { kind: 'bad', message: e instanceof Error ? e.message : String(e) };
		}
		checking = false;
		view = (await attempt(() => updatesApi.status())) ?? view;
	}

	async function simulate(major: boolean) {
		const info = await attempt(() => updatesApi.simulate(major));
		if (!info) return;
		showUpdate(info);
		result = null;
	}
</script>

<div class="panel">
	<div class="panel-head">
		{#if view}<Switch checked={view.automatic} label="Check for updates automatically" onchange={toggle} />{/if}
		<h2>Updates</h2>
	</div>
	{#if view}
		<div class="kv">
			<dt>Version</dt>
			<dd>Ledger {view.currentVersion}</dd>
			<dt>Updates</dt>
			<dd>
				<span class="dot" class:ok={!available && !!view.checkedAt} class:warn={!!available} class:off={!available && !view.checkedAt}></span>
				{statusText}{#if view.checkedAt}<span class="hint" title={dateTime(view.checkedAt)}> · checked {ago(view.checkedAt)}</span>{/if}
			</dd>
		</div>
		{#if view.unavailable}
			<div class="notice info">{view.unavailable}</div>
		{/if}

		<div class="hint">
			With the switch on, Ledger looks shortly after it starts and every few hours, and tells you when a new version is out. It never installs one until you click
			<b>Install and restart</b>, and then waits for a sync or a Claude Code run to finish first. With this off, Ledger only looks when you click
			<b>Check for updates</b>. The choice is kept for this computer, in <code>{view.configFile}</code>, not in the vault.
		</div>

		{#if result}
			<div class="notice" class:ok={result.kind === 'ok'} class:info={result.kind === 'info'} class:bad={result.kind === 'bad'}>{result.message}</div>
		{/if}
		<div class="row-end" style="justify-content:flex-start">
			<button class="btn" onclick={check} disabled={checking || !!view.unavailable} title={view.unavailable ?? undefined}>
				{checking ? 'Checking…' : 'Check for updates'}
			</button>
			{#if available && updates.hidden}
				<button class="btn ghost" onclick={() => showUpdate(available)}>Show Ledger {available.version}</button>
			{/if}
		</div>

		{#if view.devBuild}
			<div class="sync-detail">
				<h3 style="font-size:14px">Development build</h3>
				<div class="hint">
					Pretend a release was found, to see the notice and go through installing it. Installing waits for syncs and Claude Code runs like a real
					update, then stops short of installing anything.
				</div>
				<div class="row-end" style="justify-content:flex-start">
					<button class="btn small" onclick={() => simulate(false)}>Simulate a minor update</button>
					<button class="btn small" onclick={() => simulate(true)}>Simulate a major update</button>
				</div>
			</div>
		{/if}
	{/if}
</div>
