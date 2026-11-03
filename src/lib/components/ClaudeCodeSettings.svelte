<script lang="ts">
	// The settings screen's panel for running prompts with Claude Code. Its settings live in the vault's
	// .ledger/claude-code.json (never synced), apart from the rest of the settings.
	import { onMount } from 'svelte';
	import { open } from '@tauri-apps/plugin-dialog';
	import { claudeCodeApi } from '$lib/api';
	import { attempt, toast } from '$lib/ui.svelte';
	import type { ClaudeCodeCheck, ClaudeCodeSettings, ClaudePermissionMode } from '$lib/types';

	const modes: { id: ClaudePermissionMode; label: string; hint: string }[] = [
		{
			id: 'acceptEdits',
			label: 'Edit files',
			hint: 'It reads the project and creates or edits files inside the project’s folder, which is what writing a deliverable needs. Other commands and network access are refused: a run is not interactive, so nothing can approve them.'
		},
		{
			id: 'plan',
			label: 'Plan only',
			hint: 'It reads the project and answers with a plan. Nothing is changed. Good for looking before a real run.'
		},
		{
			id: 'dontAsk',
			label: 'Read only',
			hint: 'It reads the project. Anything that would need approval, every edit included, is refused.'
		}
	];

	let saved = $state<ClaudeCodeSettings | null>(null);
	let cliPath = $state('');
	let mode = $state<ClaudePermissionMode>('acceptEdits');
	let model = $state('');
	let confine = $state(true);
	let autoRun = $state(false);
	let busy = $state<'save' | 'check' | null>(null);
	let checked = $state<ClaudeCodeCheck | null>(null);
	let error = $state<string | null>(null);

	const changed = $derived(
		saved !== null && (cliPath.trim() !== saved.cliPath || mode !== saved.permissionMode || model.trim() !== saved.model || confine !== saved.confine || autoRun !== saved.autoRun)
	);

	function show(settings: ClaudeCodeSettings) {
		saved = settings;
		cliPath = settings.cliPath;
		mode = settings.permissionMode;
		model = settings.model;
		confine = settings.confine;
		autoRun = settings.autoRun;
	}

	onMount(async () => {
		const settings = await attempt(() => claudeCodeApi.settings());
		if (settings) show(settings);
	});

	async function browse() {
		const chosen = await attempt(() => open({ multiple: false, directory: false, title: 'Find the claude program' }));
		if (typeof chosen === 'string') {
			cliPath = chosen;
			checked = null;
		}
	}

	async function check() {
		busy = 'check';
		checked = null;
		try {
			checked = await claudeCodeApi.check(cliPath);
		} catch (e) {
			checked = { ok: false, version: '', path: null, message: e instanceof Error ? e.message : String(e) };
		}
		busy = null;
	}

	async function save() {
		busy = 'save';
		error = null;
		try {
			show(await claudeCodeApi.saveSettings(cliPath, mode, model, confine, autoRun));
			toast('Claude Code settings saved');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		busy = null;
	}
</script>

<div class="panel">
	<h2>Claude Code</h2>
	<p class="sub" style="margin:0">
		Every prompt has a <b>Run in Claude Code</b> button. It starts the <code>claude</code> program on this machine, in that project’s folder, with the
		prompt, and the deliverable it writes lands in the project’s deliverables, where it is published as a version and linked to the prompt. You see what will
			be sent before anything starts, and a run can be
		cancelled while it works. When a run is over, a note about it (how it ended, what it said, which tools it used) is kept in the prompt’s
			thread, in a <code>runs</code> folder next to the posts, so it is part of the vault: it syncs with the rest, and any secrets are taken out first.
	</p>

	<label class="field">
		The claude program
		<div class="form-row" style="flex-wrap:nowrap">
			<input class="mono" bind:value={cliPath} oninput={() => (checked = null)} placeholder="Blank: find claude on the PATH" spellcheck="false" autocomplete="off" />
			<button class="btn" onclick={browse}>Browse…</button>
		</div>
	</label>
	{#if saved}
		<div class="hint">
			{#if saved.resolvedPath}
				With the saved setting, Ledger runs <span class="mono">{saved.resolvedPath}</span>
			{:else}
				{saved.problem}
			{/if}
		</div>
	{/if}

	<div class="field">
		What it may do
		<div class="seg" role="group" aria-label="What Claude Code may do" style="align-self:flex-start">
			{#each modes as m (m.id)}
				<button class:on={mode === m.id} aria-pressed={mode === m.id} onclick={() => (mode = m.id)}>{m.label}</button>
			{/each}
		</div>
		<span class="hint">{modes.find((m) => m.id === mode)?.hint}</span>
	</div>

	<label class="field" style="max-width:320px">
		Model
		<input bind:value={model} placeholder="Blank: the default of Claude Code" spellcheck="false" autocomplete="off" />
		<span class="hint">A name or alias Claude Code accepts, such as <code>sonnet</code> or <code>opus</code>.</span>
	</label>

	<label class="check">
		<input type="checkbox" bind:checked={confine} /> Extra guard rails (recommended)
	</label>
	<div class="hint">
		Passes <code>--settings</code> (a file you can read in the confirmation before each run), <code>--setting-sources user</code> and
		<code>--strict-mcp-config</code>: reads and writes outside the project folder are refused, and so are the vault’s <code>.ledger</code> folder and
		any <code>.env</code> file; the project folder’s own Claude settings (hooks, say) are ignored, and no MCP servers are started, so files that arrive
		in the vault through sync can’t run anything. It needs a recent Claude Code: if a run fails with “unknown option”, update Claude Code or turn this
		off. There is no setting that skips Claude Code’s permission checks.
	</div>

	<label class="check">
		<input type="checkbox" bind:checked={autoRun} /> Run a prompt as soon as it is created
	</label>
	<div class="hint">
		Off by default. When it is on, saving a new prompt starts it in Claude Code at once, so you don’t press <b>Run in Claude Code</b> and don’t see the
		confirmation of what will be sent first. It runs with the permissions chosen above and uses your Claude allowance, and it starts before any guiding
		questions from a template are answered. You can still cancel it from the prompt’s card, and a prompt you already have still needs the button.
	</div>

	{#if error}<div class="notice bad">{error}</div>{/if}
	{#if checked}
		<div class="notice" class:ok={checked.ok} class:bad={!checked.ok}>
			{checked.message}
			{#if checked.path}<div class="mono" style="margin-top:4px">{checked.path}</div>{/if}
		</div>
	{/if}

	<div class="row-end" style="justify-content:flex-start">
		<button class="btn" onclick={check} disabled={busy !== null}>{busy === 'check' ? 'Checking…' : 'Check'}</button>
		<button class="btn primary" onclick={save} disabled={busy !== null || !changed}>Save</button>
	</div>
	{#if saved}
		<div class="hint">Settings file: <span class="mono">{saved.settingsFile}</span> · at most {saved.maxConcurrent} runs at once</div>
	{/if}
</div>
