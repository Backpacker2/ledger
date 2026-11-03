<script lang="ts">
	// Where the vault lives on this machine: the folder, switching to another, and reloading it from disk.
	import { open } from '@tauri-apps/plugin-dialog';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { api } from '$lib/api';
	import { attempt, confirmDialog, refresh, toast } from '$lib/ui.svelte';
	import type { Settings } from '$lib/types';

	let { settings }: { settings: Settings } = $props();

	/** The folder the form starts with, read once; an edit stays local until it is used. */
	const initialPath = () => settings.vaultPath;

	let vaultInput = $state(initialPath());
	let copy = $state(true);
	let busy = $state(false);

	const vaultChanged = $derived(vaultInput.trim() !== '' && vaultInput.trim() !== settings.vaultPath);

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
		busy = true;
		const next = await attempt(() => api.setVaultPath(vaultInput.trim(), copy));
		busy = false;
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
</script>

<div class="panel">
	<h2>Vault</h2>
	<p class="sub" style="margin:0">
		Everything you write lives in a folder of plain markdown files with YAML frontmatter, much like an Obsidian vault. Edit them in any editor; Ledger
		picks up your changes when you come back to its window.
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
			<button class="btn primary" onclick={useFolder} disabled={busy}>Use this folder</button>
		</div>
	{/if}
	<div><button class="btn small" onclick={reload}>Reload from disk</button></div>

	<details class="more">
		<summary><ChevronRight size={14} class="disclosure" />What is in the folder</summary>
		<p class="sub">
			Ledger loads the vault into memory when it starts and writes every change straight back to the files.
		</p>
		<pre class="mono tree">projects/&lt;project&gt;/project.md                     a project
projects/&lt;project&gt;/README.md                      its reading guide
projects/&lt;project&gt;/threads/&lt;thread&gt;/thread.md      a thread, with posts/ prompts/
                                                 feedback/ uploads/ (&lt;id&gt;.md each)
projects/&lt;project&gt;/userstories/&lt;id&gt;.md          a user story
projects/&lt;project&gt;/tasks/&lt;id&gt;.md                a task on the board
projects/&lt;project&gt;/deliverables/                  what prompts produce (name@v1.0.md = published)
projects/&lt;project&gt;/versions/&lt;id&gt;.md              how a published version came to be
projects/&lt;project&gt;/attachments/                   uploaded files and screenshots
projects/&lt;project&gt;/templates/&lt;id&gt;.md            templates for this project
templates/&lt;id&gt;.md                               templates for every project
.ledger/                                         settings and secrets, never synced</pre>
	</details>
</div>
