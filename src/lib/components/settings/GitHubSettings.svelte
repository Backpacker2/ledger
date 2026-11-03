<script lang="ts">
	// Syncing the vault with a private GitHub repository: where to, the token, and how the last sync went.
	import { openUrl } from '@tauri-apps/plugin-opener';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Switch from '$lib/components/Switch.svelte';
	import { api } from '$lib/api';
	import { ago, dateTime } from '$lib/format';
	import { attempt, confirmDialog, refresh, sync, toast } from '$lib/ui.svelte';
	import type { Settings } from '$lib/types';

	let { settings }: { settings: Settings } = $props();

	const status = $derived(sync.status ?? settings.status);

	/** The form's starting values, read once; edits stay local until saved. */
	function start() {
		return { repo: settings.sync.repo, branch: settings.sync.branch || 'main' };
	}
	const initial = start();

	let repo = $state(initial.repo);
	let branch = $state(initial.branch);
	let token = $state('');
	let busy = $state<'test' | 'save' | null>(null);
	let result = $state<{ ok: boolean; message: string } | null>(null);
	let repoInput = $state<HTMLInputElement>();

	const hasToken = $derived(!!token.trim() || settings.sync.hasToken);

	async function test() {
		busy = 'test';
		result = null;
		try {
			result = { ok: true, message: await api.testSync(repo, branch, token.trim() || null) };
		} catch (e) {
			result = { ok: false, message: e instanceof Error ? e.message : String(e) };
		}
		busy = null;
	}

	async function save() {
		busy = 'save';
		const saved = await attempt(() => api.saveSyncSettings(repo, branch, token.trim() || null));
		busy = null;
		if (!saved) return;
		// Show what was actually saved (a pasted URL becomes owner/name).
		repo = saved.sync.repo;
		branch = saved.sync.branch;
		token = '';
		result = null;
		toast('Saved. Syncing…');
		await refresh();
	}

	async function syncNow() {
		await attempt(() => api.syncNow());
	}

	async function disconnect() {
		const ok = await confirmDialog({
			title: 'Turn off GitHub sync?',
			message: 'The access token is removed from this machine. Your files stay where they are, here and on GitHub.',
			confirmLabel: 'Turn off',
			danger: true
		});
		if (!ok) return;
		if (await attempt(() => api.disconnectSync())) {
			result = null;
			await refresh();
		}
	}

	/** Off removes the token (after asking); on saves the form if it is filled in, or points at what is missing. */
	function toggle(on: boolean) {
		if (!on) return void disconnect();
		if (repo.trim() && branch.trim() && hasToken) return void save();
		toast('Fill in the repository and an access token below, then save.');
		repoInput?.focus();
	}

	const stateText = $derived.by(() => {
		if (!status.configured) return 'Not syncing';
		if (status.state === 'syncing') return 'Syncing…';
		if (status.state === 'error') return 'Last sync failed';
		return status.pending ? 'Changes waiting to be pushed' : 'Up to date';
	});
</script>

<div class="panel">
	<div class="panel-head">
		<Switch checked={settings.sync.configured} label="Sync the vault to GitHub" disabled={busy !== null} onchange={toggle} />
		<h2>Sync to GitHub</h2>
	</div>
	<p class="sub" style="margin:0">
		Keep the vault in a <strong>private</strong> GitHub repository, so it survives this machine and follows you to others. Ledger pulls when it starts,
		pushes shortly after you make changes, and checks for new changes every few minutes. It refuses to sync to a public repository.
	</p>

	<div class="kv">
		<dt>Status</dt>
		<dd>
			<span class="dot" class:ok={status.state === 'idle' && !status.pending} class:warn={status.state === 'syncing' || status.pending} class:bad={status.state === 'error'}
			></span>
			{stateText}
		</dd>
		{#if status.configured}
			<dt>Syncing with</dt>
			<dd>{status.repo} <span class="hint">on {status.branch}</span></dd>
		{/if}
		{#if status.lastSyncAt}
			<dt>Last sync</dt>
			<dd title={dateTime(status.lastSyncAt)}>{ago(status.lastSyncAt)}{status.summary ? ` — ${status.summary}` : ''}</dd>
		{/if}
		{#if status.lastError}
			<dt>Problem</dt>
			<dd class="error-text">{status.lastError}</dd>
		{/if}
	</div>
	{#if settings.sync.configured}
		<div class="row-end" style="justify-content:flex-start">
			<button class="btn small" onclick={syncNow} disabled={status.state === 'syncing'}>Sync now</button>
		</div>
	{/if}

	<div class="sync-detail">
		<div class="form-row">
			<label class="field grow">
				Repository
				<input bind:this={repoInput} bind:value={repo} placeholder="owner/notes" spellcheck="false" autocomplete="off" />
			</label>
			<label class="field" style="width:180px">
				Branch
				<input bind:value={branch} placeholder="main" spellcheck="false" autocomplete="off" />
			</label>
		</div>
		<label class="field">
			Access token
			<input
				type="password"
				bind:value={token}
				placeholder={settings.sync.hasToken ? `Saved ${settings.sync.tokenHint} — leave blank to keep it` : 'github_pat_…'}
				spellcheck="false"
				autocomplete="off"
			/>
		</label>
		<div class="hint">
			Use a <a
				href="https://github.com/settings/personal-access-tokens/new"
				onclick={(e) => {
					e.preventDefault();
					openUrl(e.currentTarget.href).catch(() => {});
				}}>fine-grained token</a
			>
			limited to that one repository, with <b>Contents: Read and write</b>. The repository needs at least one commit already (a README will do), and the
			branch is created if it doesn't exist.
		</div>

		{#if result}
			<div class="notice" class:ok={result.ok} class:bad={!result.ok}>{result.message}</div>
		{/if}

		<div class="row-end" style="justify-content:flex-start">
			<button class="btn" onclick={test} disabled={busy !== null || !repo.trim() || !hasToken}>Test connection</button>
			<button class="btn primary" onclick={save} disabled={busy !== null || !repo.trim() || !branch.trim() || !hasToken}>
				{settings.sync.configured ? 'Save changes' : 'Save & start syncing'}
			</button>
		</div>
	</div>

	<details class="more">
		<summary><ChevronRight size={14} class="disclosure" />Where these settings are kept</summary>
		<div class="kv">
			<dt>Settings</dt>
			<dd class="mono">{settings.settingsFile}</dd>
			<dt>Secret</dt>
			<dd class="mono">{settings.secretsFile}</dd>
		</div>
		<div class="hint">
			Both files stay on this machine: they live in the vault’s <code>.ledger</code> folder, which the vault’s <code>.gitignore</code> lists, so the
			repository, branch and token are never uploaded.
		</div>
	</details>
</div>
