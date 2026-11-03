<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import ClaudeCodeSettings from '$lib/components/ClaudeCodeSettings.svelte';
	import HotkeyInput from '$lib/components/HotkeyInput.svelte';
	import OrderSettings from '$lib/components/OrderSettings.svelte';
	import TrelloSettings from '$lib/components/TrelloSettings.svelte';
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
		return {
			repo: sync.repo,
			branch: sync.branch || 'main',
			path: vaultPath,
			hotkey: screenshot.hotkey,
			useCommand: screenshot.useCommand,
			command: screenshot.command
		};
	}
	const initial = start();

	let repo = $state(initial.repo);
	let branch = $state(initial.branch);
	let token = $state('');
	let vaultInput = $state(initial.path);
	let copy = $state(true);
	let hotkey = $state(initial.hotkey);
	let useCommand = $state(initial.useCommand);
	let command = $state(initial.command);
	let shotError = $state<string | null>(null);
	let busy = $state<'test' | 'save' | 'vault' | 'shot' | 'key' | null>(null);
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
			message: `Ledger will use ${vaultInput.trim()} from now on. ${copy ? 'If the folder is empty, your current vault is copied into it first. ' : ''}Your current vault is left where it is.`,
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

	// ---- encryption ----------------------------------------------------------------------------

	const encryption = $derived(settings.encryption);

	let importing = $state(false);
	let importText = $state('');
	/** The recovery key, shown only after it is asked for and gone when hidden or when leaving this screen. */
	let recovery = $state<string | null>(null);

	$effect(() => () => {
		recovery = null;
		importText = '';
	});

	async function toggleEncryption(e: Event) {
		const box = e.currentTarget as HTMLInputElement;
		const on = box.checked;
		const ok = await confirmDialog(
			on
				? {
						title: 'Encrypt the vault on sync?',
						message: `Encryption starts fresh: on a new repository, or on a branch that doesn’t exist yet, which Ledger creates with nothing behind it. A branch that already holds your files in plain text is never converted, because its history would keep them.${
							encryption.needsFreshTarget
								? ` ${settings.sync.repo} on ${settings.sync.branch} is such a branch, so syncing stops until you change the repository or the branch above.`
								: ''
						} If the key is lost, the encrypted copy on GitHub is lost with it.`,
						confirmLabel: 'Turn on encryption'
					}
				: {
						title: 'Turn encryption off?',
						message:
							'GitHub already holds encrypted files, so syncing stops with an error while encryption is off. Turn it back on to carry on, or sync to a new empty repository to go back to plain files.',
						confirmLabel: 'Turn off',
						danger: true
					}
		);
		if (!ok) {
			box.checked = !on;
			return;
		}
		busy = 'key';
		const saved = await attempt(() => api.setEncryptionEnabled(on));
		busy = null;
		if (!saved) {
			box.checked = !on;
			return;
		}
		toast(on ? (encryption.needsFreshTarget ? 'Encryption is on. Sync is paused until you pick a new repository or branch.' : 'Encryption is on') : 'Encryption is off');
		await refresh();
	}

	async function generateKey() {
		const replace = encryption.hasKey;
		if (replace) {
			const ok = await confirmDialog({
				title: 'Replace the key?',
				message:
					'This machine’s key is replaced by a new one. Anything already encrypted with the old key can only be read if you have its recovery key.',
				confirmLabel: 'Replace key',
				danger: true
			});
			if (!ok) return;
		}
		busy = 'key';
		const saved = await attempt(() => api.generateEncryptionKey(replace));
		busy = null;
		if (!saved) return;
		recovery = null;
		toast('Key created. Export the recovery key and keep it somewhere safe.');
		await refresh();
	}

	async function importKey() {
		const replace = encryption.hasKey;
		if (replace) {
			const ok = await confirmDialog({
				title: 'Replace the key?',
				message:
					'This machine’s key is replaced by the one you pasted. Anything encrypted with the old key can only be read if you have its recovery key.',
				confirmLabel: 'Replace key',
				danger: true
			});
			if (!ok) return;
		}
		busy = 'key';
		const saved = await attempt(() => api.importEncryptionKey(importText, replace));
		busy = null;
		if (!saved) return;
		importText = '';
		importing = false;
		recovery = null;
		toast('Key imported');
		await refresh();
	}

	function cancelImport() {
		importText = '';
		importing = false;
	}

	async function exportKey() {
		const text = await attempt(() => api.exportRecoveryKey());
		if (text !== undefined) recovery = text;
	}

	async function copyRecovery() {
		if (recovery === null) return;
		try {
			await navigator.clipboard.writeText(recovery);
			toast('Recovery key copied');
		} catch {
			toast('Could not copy. Select the key and copy it by hand.', true);
		}
	}

	// ---- screenshots ---------------------------------------------------------------------------

	const screenshot = $derived(settings.screenshot);
	const hotkeyChanged = $derived(hotkey !== screenshot.hotkey);
	const toolChanged = $derived(useCommand !== screenshot.useCommand || command.trim() !== screenshot.command);
	const shotChanged = $derived(hotkeyChanged || toolChanged);
	const flameshot = $derived(screenshot.presets.find((p) => p.name === 'Flameshot'));

	async function saveScreenshot() {
		busy = 'shot';
		shotError = null;
		try {
			// The tool first: a command that can't be used is refused before the hotkey changes.
			if (toolChanged) await api.saveScreenshotTool(useCommand, command);
			if (hotkeyChanged) await api.saveScreenshotHotkey(hotkey);
			toast('Screenshot settings saved');
			await refresh();
		} catch (e) {
			// A shortcut the system refuses (or a broken command) stays on screen as a warning; the previous one keeps working.
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
			<pre class="mono tree">projects/&lt;project&gt;/project.md                     a project
projects/&lt;project&gt;/README.md                      its reading guide
projects/&lt;project&gt;/threads/&lt;thread&gt;/thread.md      a thread, with posts/ prompts/
                                                 feedback/ uploads/ (&lt;id&gt;.md each)
projects/&lt;project&gt;/userstories/&lt;id&gt;.md          a user story
projects/&lt;project&gt;/tasks/&lt;id&gt;.md                a task on the board
projects/&lt;project&gt;/deliverables/                  what prompts produce (name@v1.0.md = published)
projects/&lt;project&gt;/attachments/                   uploaded files and screenshots
projects/&lt;project&gt;/templates/&lt;id&gt;.md            templates for this project
templates/&lt;id&gt;.md                               templates for every project
.ledger/                                         settings and secrets, never synced</pre>
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
					<dt>Settings</dt>
					<dd class="mono">{settings.settingsFile}</dd>
					<dt>Secret</dt>
					<dd class="mono">{settings.secretsFile}</dd>
				</div>
				<div class="hint">
					Both files stay on this machine: they live in the vault’s <code>.ledger</code> folder, which the vault’s <code>.gitignore</code> lists,
					so the repository, branch and token are never uploaded.
				</div>
				{#if settings.sync.configured}
					<div class="row-end" style="justify-content:flex-start">
						<button class="btn small" onclick={syncNow} disabled={status.state === 'syncing'}>Sync now</button>
						<button class="btn small ghost danger" onclick={disconnect}>Turn off sync</button>
					</div>
				{/if}
			</div>
			<div class="sync-detail">
				<h3 style="font-size:14px">Encryption</h3>
				<p class="sub" style="margin:0">
					Seal the vault before it goes to GitHub. Each file is encrypted with AES-256 on this machine and only the encrypted version is uploaded. The
					vault on disk stays plain markdown, so Obsidian and other editors keep working. The key is kept in this vault’s <code>.ledger/.env</code>
					file, next to the access token: in plain text, readable by anything that can read your files on this machine, and never uploaded.
				</p>

				<label class="check">
					<input
						type="checkbox"
						checked={encryption.enabled}
						disabled={busy !== null || (!encryption.enabled && !encryption.hasKey)}
						onchange={toggleEncryption}
					/>
					Encrypt vault on sync
				</label>
				{#if !encryption.enabled && !encryption.hasKey}
					<div class="hint">Generate or import a key first: until there is one, encryption can’t be turned on.</div>
				{/if}

				<div class="kv">
					<dt>Key</dt>
					<dd>
						<span class="dot" class:ok={encryption.hasKey} class:warn={!encryption.hasKey}></span>
						{#if encryption.hasKey}
							On this machine <span class="hint">· key id {encryption.keyId}</span>
						{:else}
							No key on this machine
						{/if}
					</dd>
				</div>
				{#if encryption.problem}
					<div class="notice bad">{encryption.problem}</div>
				{/if}
				{#if encryption.needsFreshTarget}
					{#if encryption.enabled}
						<div class="notice bad">
							<strong>Sync is paused.</strong>
							{settings.sync.repo} on {settings.sync.branch} already holds this vault unencrypted, so it can’t become an encrypted branch. Change the repository
							or the branch above to a new repository, or to a branch name that doesn’t exist yet, and save. Ledger then uploads everything encrypted as a new
							branch with no history. (Turning encryption off resumes plain syncing.)
						</div>
					{:else}
						<div class="notice info">
							{settings.sync.repo} on {settings.sync.branch} already holds this vault unencrypted. To encrypt, point sync at a new repository, or at a branch
							that doesn’t exist yet, so the plain history isn’t carried over.
						</div>
					{/if}
				{/if}

				<div class="row-end" style="justify-content:flex-start">
					<button class="btn" onclick={generateKey} disabled={busy !== null}>Generate key</button>
					<button class="btn" onclick={() => (importing = !importing)} disabled={busy !== null}>Import key…</button>
					<button class="btn" onclick={exportKey} disabled={busy !== null || !encryption.hasKey}>Export recovery key</button>
				</div>

				{#if importing}
					<label class="field">
						Recovery key
						<input type="password" bind:value={importText} placeholder="ledger-key-1:…" spellcheck="false" autocomplete="off" />
					</label>
					<div class="hint">Paste the recovery key exported on another machine, to read and write the same encrypted repository.</div>
					<div class="row-end">
						<button class="btn ghost" onclick={cancelImport}>Cancel</button>
						<button class="btn primary" onclick={importKey} disabled={busy !== null || !importText.trim()}>Import key</button>
					</div>
				{/if}

				{#if recovery !== null}
					<div class="notice info">
						<div>Keep this somewhere safe, such as a password manager. Anyone who has it can read the encrypted copy on GitHub.</div>
						<pre class="mono tree" style="white-space:pre-wrap;word-break:break-all;user-select:all;margin:8px 0">{recovery}</pre>
						<div class="row-end" style="justify-content:flex-start">
							<button class="btn small" onclick={copyRecovery}>Copy</button>
							<button class="btn small ghost" onclick={() => (recovery = null)}>Hide</button>
						</div>
					</div>
				{/if}

				<div class="notice">
					<strong>Don’t lose the key.</strong> If it is lost, what is encrypted on GitHub is lost with it: neither GitHub nor Ledger can recover it.
					Export the recovery key and keep a copy somewhere other than this machine.
				</div>
				<div class="hint">
					Encryption always starts fresh: on a new repository, or on a branch that doesn’t exist yet, which Ledger creates with nothing behind it. A
					branch that already holds your files in plain text is never converted, because its history would keep them. The plain copy stays on the old
					branch or repository until you delete it on GitHub. File and folder names are not encrypted, only what is inside the files.
				</div>
			</div>
		</div>

		<!-- Trello sync (feat/trello-sync) -->
		<TrelloSettings />

		<div class="panel">
			<h2>Screenshots</h2>
			<p class="sub" style="margin:0">
				Press the shortcut anywhere to take a screenshot. Ledger then comes forward with it, ready to attach to the thread you have open (or one you
				pick), and saves it in that project’s attachments folder.
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

			<div class="field">
				Screenshot tool
				<div class="seg" role="radiogroup" aria-label="Screenshot tool" style="align-self:flex-start">
					<button role="radio" aria-checked={!useCommand} class:on={!useCommand} onclick={() => (useCommand = false)}>Built-in</button>
					<button role="radio" aria-checked={useCommand} class:on={useCommand} onclick={() => (useCommand = true)}>My own tool</button>
				</div>
			</div>
			{#if !useCommand}
				<div class="hint">
					{screenshot.builtin} For arrows, boxes, blur and text, use your own tool, such as Flameshot{flameshot?.found ? ', which is installed' : ''}.
				</div>
			{:else}
				<label class="field">
					Command
					<input bind:value={command} placeholder={screenshot.presets[0]?.command ?? 'flameshot gui --raw'} spellcheck="false" autocomplete="off" />
				</label>
				<div class="row-end" style="justify-content:flex-start">
					{#each screenshot.presets as preset (preset.name)}
						<button class="btn ghost" onclick={() => (command = preset.command)} disabled={command.trim() === preset.command}>
							Use {preset.name}{preset.found ? '' : ' (not found)'}
						</button>
					{/each}
				</div>
				<div class="hint">
					Ledger runs this command when you press the shortcut. If it contains <code>{'{file}'}</code>, Ledger puts a temporary file name there: the
					tool saves the PNG to it and Ledger reads it when the tool closes. Without <code>{'{file}'}</code>, Ledger reads the PNG the tool prints.
					Closing the tool without a picture (Esc) just cancels. Put a path with spaces in double quotes.
				</div>

				<div class="notice info">
					<strong>Setting up Flameshot</strong>
					{#if screenshot.platform === 'linux'}
						<ol>
							<li>Install it: <code>sudo apt install flameshot</code>, your distribution’s package manager, or flameshot.org.</li>
							<li>Click <b>Use Flameshot</b> (it fills in <code>flameshot gui --raw</code>) and <b>Save</b>.</li>
							<li>Press the shortcut above. Flameshot opens: drag out an area, draw on it, and press <b>Enter</b>. Esc cancels.</li>
							<li>
								Took one with Flameshot’s own key? In Flameshot’s editor press <b>Ctrl+O</b> (Open with) and choose <b>Ledger</b>. That works when Ledger is
								installed from its .deb or .rpm package.
							</li>
							<li>Flameshot usually has the Print key. Keep Ledger’s shortcut on a different key.</li>
						</ol>
					{:else if screenshot.platform === 'windows'}
						<ol>
							<li>Install it from flameshot.org. It goes to <code>C:\Program Files\Flameshot</code>.</li>
							<li>Click <b>Use Flameshot</b> and <b>Save</b>. Installed somewhere else? Change the path in the command.</li>
							<li>Press the shortcut above. Flameshot opens: drag out an area, draw on it, and press <b>Ctrl+S</b>. Esc cancels.</li>
							<li>
								Took one with Flameshot’s own key? In Flameshot’s editor press <b>Ctrl+O</b> (Open with), choose <b>More apps</b> → <b>Look for another
								app on this PC</b> and pick <code>Ledger.exe</code>. Ledger doesn’t register itself for pictures on Windows, so it never takes over
								your picture viewer.
							</li>
							<li>Flameshot usually has the Print key. Keep Ledger’s shortcut on a different key.</li>
						</ol>
					{:else}
						<ol>
							<li>
								No install needed: <b>Use macOS Screenshot</b> runs the system’s own picker (Space switches to a window, Esc cancels). No drawing,
								and macOS may ask once to let Ledger record the screen.
							</li>
							<li>
								For drawing on it, install Flameshot (<code>brew install --cask flameshot</code>, or flameshot.org) into Applications, give Flameshot the
								Screen Recording permission, then click <b>Use Flameshot</b> and <b>Save</b>.
							</li>
							<li>Press the shortcut above, drag out an area, draw on it, and press <b>Cmd+S</b>. Esc cancels.</li>
							<li>Any PNG can also be sent to Ledger from Finder: right-click → Open With → Ledger.</li>
						</ol>
					{/if}
				</div>
			{/if}

			{#if shotError}
				<div class="notice bad">{shotError}</div>
			{:else if screenshot.problem}
				<div class="notice">{screenshot.problem}</div>
			{/if}
			{#if screenshot.note}<div class="notice info">{screenshot.note}</div>{/if}
			<div class="row-end" style="justify-content:flex-start">
				<button class="btn primary" onclick={saveScreenshot} disabled={busy !== null || !shotChanged || !hotkey || (useCommand && !command.trim())}
					>Save</button
				>
				<button class="btn" onclick={() => api.takeScreenshot()} disabled={shotChanged} title={shotChanged ? 'Save first' : undefined}
					>Take a screenshot now</button
				>
			</div>
		</div>

		<ClaudeCodeSettings />

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

		<!-- Display order of feedback and threads -->
		<OrderSettings />

		<div class="hint" style="padding:0 4px 8px">Ledger {settings.version}</div>
	</div>
</div>
