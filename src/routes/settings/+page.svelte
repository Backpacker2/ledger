<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import HotkeyInput from '$lib/components/HotkeyInput.svelte';
	import { api } from '$lib/api';
	import { ago, dateTime } from '$lib/format';
	import { attempt, confirmDialog, refresh, setTheme, sync, theme, themes, toast } from '$lib/ui.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const settings = $derived(data.settings);
	const status = $derived(sync.status ?? settings.status);

	/** The form's starting values, read once; edits stay local until saved. */
	function start() {
		const { sync, vaultPath, screenshot } = data.settings;
		return { repo: sync.repo, branch: sync.branch || 'main', path: vaultPath, hotkey: screenshot.hotkey, uploadUrl: screenshot.uploadUrl };
	}
	const initial = start();

	let repo = $state(initial.repo);
	let branch = $state(initial.branch);
	let token = $state('');
	let vaultInput = $state(initial.path);
	let copy = $state(true);
	let hotkey = $state(initial.hotkey);
	let uploadUrl = $state(initial.uploadUrl);
	let shotError = $state<string | null>(null);
	let busy = $state<'test' | 'save' | 'vault' | 'shot' | null>(null);
	let result = $state<{ ok: boolean; message: string } | null>(null);

	const vaultChanged = $derived(vaultInput.trim() !== '' && vaultInput.trim() !== settings.vaultPath);

	// ---- the vault -----------------------------------------------------------------------------

	async function browse() {
		const chosen = await attempt(() => open({ directory: true, multiple: false, defaultPath: settings.vaultPath }));
		if (typeof chosen === 'string') vaultInput = chosen;
	}

	async function useFolder() {
		const ok = await confirmDialog({
			title: 'Switch vault?',
			message: `Ledger will use ${vaultInput.trim()} from now on. ${copy ? 'If the folder is empty, your current notes are copied into it first. ' : ''}Your current vault is left where it is.`,
			confirmLabel: 'Switch vault'
		});
		if (!ok) return;
		busy = 'vault';
		const next = await attempt(() => api.setVaultPath(vaultInput.trim(), copy));
		busy = null;
		if (!next) return;
		toast('Vault changed');
		await refresh();
	}

	async function reload() {
		if ((await attempt(() => api.reloadVault())) !== undefined) {
			toast('Reloaded from disk');
			await refresh();
		}
	}

	// ---- GitHub --------------------------------------------------------------------------------

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
			message: 'The access token is removed from this machine. Your notes stay where they are, here and on GitHub.',
			confirmLabel: 'Turn off',
			danger: true
		});
		if (!ok) return;
		if (await attempt(() => api.disconnectSync())) {
			result = null;
			await refresh();
		}
	}

	// ---- screenshots ---------------------------------------------------------------------------

	const screenshot = $derived(settings.screenshot);
	const shotChanged = $derived(hotkey !== screenshot.hotkey || uploadUrl.trim() !== screenshot.uploadUrl);

	async function saveScreenshot() {
		busy = 'shot';
		shotError = null;
		try {
			await api.saveScreenshotSettings(hotkey, uploadUrl);
			toast('Screenshot settings saved');
			await refresh();
		} catch (e) {
			// A shortcut the system refuses stays on screen as a warning; the previous one keeps working.
			shotError = e instanceof Error ? e.message : String(e);
		}
		busy = null;
	}

	const stateText = $derived.by(() => {
		if (!status.configured) return 'Not syncing';
		if (status.state === 'syncing') return 'Syncing…';
		if (status.state === 'error') return 'Last sync failed';
		return status.pending ? 'Changes waiting to be pushed' : 'Up to date';
	});
</script>

<svelte:head><title>Settings · Ledger</title></svelte:head>

<div class="header-wrap flat">
	<div class="header-top">
		<div>
			<h1>Settings</h1>
		</div>
	</div>
</div>

<div class="page">
	<div class="page-inner">
		<div class="panel">
			<h2>Vault</h2>
			<p class="sub" style="margin:0">
				Everything you write lives in a folder of plain markdown files with YAML frontmatter — a vault, much like an Obsidian one. Open the
				folder in Obsidian or any editor; Ledger picks up your edits when you come back to its window. Ledger loads the vault into memory
				when it starts and writes every change straight back to the files.
			</p>
			<label class="field">
				Vault folder
				<div class="form-row" style="flex-wrap:nowrap">
					<input class="mono" bind:value={vaultInput} disabled={settings.vaultLocked} spellcheck="false" />
					<button class="btn" onclick={browse} disabled={settings.vaultLocked}>Browse…</button>
				</div>
			</label>
			{#if settings.vaultLocked}
				<div class="notice info">The location is fixed by the <code>LEDGER_VAULT</code> environment variable.</div>
			{:else if vaultChanged}
				<label class="check">
					<input type="checkbox" bind:checked={copy} /> If that folder is empty, copy my current vault into it
				</label>
				<div class="row-end">
					<button class="btn ghost" onclick={() => (vaultInput = settings.vaultPath)}>Cancel</button>
					<button class="btn primary" onclick={useFolder} disabled={busy === 'vault'}>Use this folder</button>
				</div>
			{/if}
			<pre class="mono tree">threads/&lt;thread&gt;/thread.md           a thread
threads/&lt;thread&gt;/posts/&lt;id&gt;.md      its posts
documents/&lt;id&gt;.md, &lt;id&gt;@v1.0.md      drafts and published versions
document-types/&lt;type&gt;.md             guiding questions
user-stories/&lt;id&gt;.md                 stories and their criteria
planboard.md                         the columns</pre>
			<div><button class="btn small" onclick={reload}>Reload from disk</button></div>
		</div>

		<div class="panel">
			<h2>Sync to GitHub</h2>
			<p class="sub" style="margin:0">
				Keep the vault in a <strong>private</strong> GitHub repository, so it survives this machine and follows you to others. Ledger pulls when it
				starts, pushes shortly after you make changes, and checks for new changes every few minutes. Ledger refuses to sync to a public repository.
			</p>

			<div class="form-row">
				<label class="field grow">
					Repository
					<input bind:value={repo} placeholder="owner/notes" spellcheck="false" autocomplete="off" />
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
				<button class="btn" onclick={test} disabled={busy !== null || !repo.trim() || (!token.trim() && !settings.sync.hasToken)}>Test connection</button>
				<button class="btn primary" onclick={save} disabled={busy !== null || !repo.trim() || !branch.trim() || (!token.trim() && !settings.sync.hasToken)}>
					{settings.sync.configured ? 'Save changes' : 'Save & start syncing'}
				</button>
			</div>

			<div class="sync-detail">
				<div class="kv">
					<dt>Status</dt>
					<dd>
						<span class="dot" class:ok={status.state === 'idle' && !status.pending} class:warn={status.state === 'syncing' || status.pending} class:bad={status.state === 'error'}></span>
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
					<dt>Saved in</dt>
					<dd class="mono">{settings.envFile}</dd>
				</div>
				<div class="hint">That file stays on this machine: the vault's <code>.gitignore</code> lists it, so the token is never uploaded.</div>
				{#if settings.sync.configured}
					<div class="row-end" style="justify-content:flex-start">
						<button class="btn small" onclick={syncNow} disabled={status.state === 'syncing'}>Sync now</button>
						<button class="btn small ghost danger" onclick={disconnect}>Turn off sync</button>
					</div>
				{/if}
			</div>
		</div>

		<div class="panel">
			<h2>Screenshots</h2>
			<p class="sub" style="margin:0">
				Press the shortcut anywhere to capture the screen you're on, drag out the area you want, and the picture is uploaded and opened here.
			</p>
			<div class="form-row">
				<div class="field grow">
					Screenshot hotkey
					<HotkeyInput bind:value={hotkey} onrecording={(on) => api.pauseScreenshotHotkey(on).catch(() => {})} />
				</div>
				<button class="btn ghost" onclick={() => (hotkey = screenshot.defaultHotkey)} disabled={hotkey === screenshot.defaultHotkey}>
					Reset to default
				</button>
			</div>
			<label class="field">
				Upload URL
				<input
					bind:value={uploadUrl}
					disabled={screenshot.uploadUrlLocked}
					placeholder="https://example.com/upload"
					spellcheck="false"
					autocomplete="off"
				/>
			</label>
			<div class="hint">
				The image is sent as a multipart form (field <code>file</code>, a PNG). The answer should be JSON with the image's link in
				<code>url</code>, or just the link.
				{#if screenshot.uploadUrlLocked}The address is fixed by <code>LEDGER_SCREENSHOT_URL</code>.{/if}
			</div>
			{#if shotError}
				<div class="notice bad">{shotError}</div>
			{:else if screenshot.problem}
				<div class="notice">{screenshot.problem}</div>
			{/if}
			{#if screenshot.note}<div class="notice info">{screenshot.note}</div>{/if}
			<div class="row-end" style="justify-content:flex-start">
				<button class="btn primary" onclick={saveScreenshot} disabled={busy !== null || !shotChanged || !hotkey}>Save</button>
				<button class="btn" onclick={() => api.takeScreenshot()} disabled={shotChanged}>Take a screenshot now</button>
			</div>
		</div>

		<div class="panel">
			<h2>Theme</h2>
			<div class="theme-grid">
				{#each themes as t (t.id)}
					<button class="theme-card" class:on={theme.id === t.id} onclick={() => setTheme(t.id)}>
						<span class="theme-swatch">
							{#each t.swatch as colour (colour)}<span style="background:{colour}"></span>{/each}
						</span>
						<span class="theme-name">{t.name}</span>
					</button>
				{/each}
			</div>
		</div>

		<div class="hint" style="padding:0 4px 8px">Ledger {settings.version}</div>
	</div>
</div>
