<script lang="ts">
	// Taking screenshots: the global shortcut, and the built-in picker or a tool of your own such as Flameshot.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import HotkeyInput from '$lib/components/HotkeyInput.svelte';
	import { api } from '$lib/api';
	import { refresh, toast } from '$lib/ui.svelte';
	import type { Settings } from '$lib/types';

	let { settings }: { settings: Settings } = $props();

	const screenshot = $derived(settings.screenshot);

	/** The form's starting values, read once; edits stay local until saved. */
	function start() {
		const { hotkey, useCommand, command } = settings.screenshot;
		return { hotkey, useCommand, command };
	}
	const initial = start();

	let hotkey = $state(initial.hotkey);
	let useCommand = $state(initial.useCommand);
	let command = $state(initial.command);
	let shotError = $state<string | null>(null);
	let busy = $state(false);

	const hotkeyChanged = $derived(hotkey !== screenshot.hotkey);
	const toolChanged = $derived(useCommand !== screenshot.useCommand || command.trim() !== screenshot.command);
	const shotChanged = $derived(hotkeyChanged || toolChanged);
	const flameshot = $derived(screenshot.presets.find((p) => p.name === 'Flameshot'));

	async function saveScreenshot() {
		busy = true;
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
		busy = false;
	}
</script>

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

		<details class="more">
			<summary><ChevronRight size={14} class="disclosure" />Setting up Flameshot</summary>
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
		</details>
	{/if}

	{#if shotError}
		<div class="notice bad">{shotError}</div>
	{:else if screenshot.problem}
		<div class="notice">{screenshot.problem}</div>
	{/if}
	{#if screenshot.note}<div class="notice info">{screenshot.note}</div>{/if}
	<div class="row-end" style="justify-content:flex-start">
		<button class="btn primary" onclick={saveScreenshot} disabled={busy || !shotChanged || !hotkey || (useCommand && !command.trim())}
			>Save</button
		>
		<button class="btn" onclick={() => api.takeScreenshot()} disabled={shotChanged} title={shotChanged ? 'Save first' : undefined}
			>Take a screenshot now</button
		>
	</div>
</div>
