<script lang="ts">
	// Encrypting the vault before it goes to GitHub: turning it on or off, and this machine's key.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Switch from '$lib/components/Switch.svelte';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Settings } from '$lib/types';

	let { settings }: { settings: Settings } = $props();

	const encryption = $derived(settings.encryption);

	let busy = $state(false);
	let importing = $state(false);
	let importText = $state('');
	/** The recovery key, shown only after it is asked for and gone when hidden or when leaving this screen. */
	let recovery = $state<string | null>(null);

	$effect(() => () => {
		recovery = null;
		importText = '';
	});

	async function toggleEncryption(on: boolean) {
		const ok = await confirmDialog(
			on
				? {
						title: 'Encrypt the vault on sync?',
						message: `Encryption starts fresh: on a new repository, or on a branch that doesn’t exist yet, which Ledger creates with nothing behind it. A branch that already holds your files in plain text is never converted, because its history would keep them.${
							encryption.needsFreshTarget
								? ` ${settings.sync.repo} on ${settings.sync.branch} is such a branch, so syncing stops until you change the repository or the branch under Sync to GitHub.`
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
		if (!ok) return;
		busy = true;
		const saved = await attempt(() => api.setEncryptionEnabled(on));
		busy = false;
		if (!saved) return;
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
		busy = true;
		const saved = await attempt(() => api.generateEncryptionKey(replace));
		busy = false;
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
		busy = true;
		const saved = await attempt(() => api.importEncryptionKey(importText, replace));
		busy = false;
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
</script>

<div class="panel">
	<div class="panel-head">
		<Switch
			checked={encryption.enabled}
			label="Encrypt vault on sync"
			disabled={busy || (!encryption.enabled && !encryption.hasKey)}
			onchange={toggleEncryption}
		/>
		<h2>Encryption</h2>
	</div>
	<p class="sub" style="margin:0">
		Seal the vault before it goes to GitHub. Each file is encrypted with AES-256 on this machine and only the encrypted version is uploaded. The vault on
		disk stays plain markdown, so Obsidian and other editors keep working.
	</p>

	{#if !encryption.enabled && !encryption.hasKey}
		<div class="hint">Generate or import a key below first: until there is one, encryption can’t be turned on.</div>
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
				{settings.sync.repo} on {settings.sync.branch} already holds this vault unencrypted, so it can’t become an encrypted branch. Under Sync to GitHub,
				change the repository to a new one, or the branch to a name that doesn’t exist yet, and save. Ledger then uploads everything encrypted as a new
				branch with no history. (Turning encryption off resumes plain syncing.)
			</div>
		{:else}
			<div class="notice info">
				{settings.sync.repo} on {settings.sync.branch} already holds this vault unencrypted. To encrypt, point sync at a new repository, or at a branch that
				doesn’t exist yet, so the plain history isn’t carried over.
			</div>
		{/if}
	{/if}

	<div class="row-end" style="justify-content:flex-start">
		<button class="btn" onclick={generateKey} disabled={busy}>Generate key</button>
		<button class="btn" onclick={() => (importing = !importing)} disabled={busy}>Import key…</button>
		<button class="btn" onclick={exportKey} disabled={busy || !encryption.hasKey}>Export recovery key</button>
	</div>

	{#if importing}
		<label class="field">
			Recovery key
			<input type="password" bind:value={importText} placeholder="ledger-key-1:…" spellcheck="false" autocomplete="off" />
		</label>
		<div class="hint">Paste the recovery key exported on another machine, to read and write the same encrypted repository.</div>
		<div class="row-end">
			<button class="btn ghost" onclick={cancelImport}>Cancel</button>
			<button class="btn primary" onclick={importKey} disabled={busy || !importText.trim()}>Import key</button>
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
		<strong>Don’t lose the key.</strong> If it is lost, what is encrypted on GitHub is lost with it: neither GitHub nor Ledger can recover it. Export the
		recovery key and keep a copy somewhere other than this machine.
	</div>

	<details class="more">
		<summary><ChevronRight size={14} class="disclosure" />How encryption works</summary>
		<div class="hint">
			The key is kept in this vault’s <code>.ledger/.env</code> file, next to the access token: in plain text, readable by anything that can read your
			files on this machine, and never uploaded.
		</div>
		<div class="hint">
			Encryption always starts fresh: on a new repository, or on a branch that doesn’t exist yet, which Ledger creates with nothing behind it. A branch
			that already holds your files in plain text is never converted, because its history would keep them. The plain copy stays on the old branch or
			repository until you delete it on GitHub. File and folder names are not encrypted, only what is inside the files.
		</div>
	</details>
</div>
