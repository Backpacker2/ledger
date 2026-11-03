<script lang="ts">
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	// "Run in Claude Code" for one prompt: the button, a confirmation that shows exactly what will be sent, and
	// the live state of the latest run (status, output, cancel). The run itself lives in the backend, so it goes
	// on when this card is not on screen; the state it reports is kept in `claudeRuns`.
	import { untrack } from 'svelte';
	import { claudeCodeApi } from '$lib/api';
	import { ensureClaudeRuns, latestRunFor, trackRun } from '$lib/claudeRuns.svelte';
	import { time } from '$lib/format';
	import { attempt } from '$lib/ui.svelte';
	import type { ClaudePermissionMode, ClaudeRunPreview, Prompt } from '$lib/types';
	import Markdown from './Markdown.svelte';
	import Modal from './Modal.svelte';

	let { project, thread, prompt }: { project: string; thread: string; prompt: Prompt } = $props();

	ensureClaudeRuns();

	const tracked = $derived(latestRunFor(prompt.id));
	const run = $derived(tracked?.run);
	const running = $derived(run?.status === 'running');

	let showOutput = $state(untrack(() => latestRunFor(prompt.id)?.run.status === 'running'));
	let preview = $state<ClaudeRunPreview | null>(null);
	let asking = $state(false);
	let starting = $state(false);
	let error = $state<string | null>(null);

	// ---- starting ------------------------------------------------------------------------------

	async function ask() {
		asking = true;
		const shown = await attempt(() => claudeCodeApi.preview(project, thread, prompt.id));
		asking = false;
		if (!shown) return;
		error = null;
		preview = shown;
	}

	async function start() {
		if (!preview) return;
		starting = true;
		error = null;
		try {
			trackRun(await claudeCodeApi.start(project, thread, prompt.id, preview.prompt));
			preview = null;
			showOutput = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		starting = false;
	}

	async function cancel() {
		if (run) await attempt(() => claudeCodeApi.cancel(run.id));
	}

	// ---- what the card shows -------------------------------------------------------------------

	const modeNotes: Record<ClaudePermissionMode, string> = {
		acceptEdits:
			'It can read this project and create or edit files in its folder. Other commands and network access are refused, because nothing can approve them while it runs.',
		plan: 'Plan only: it reads the project and answers with a plan. Nothing is changed.',
		dontAsk: 'Read only: it reads the project. Anything that would need approval, such as every edit, is refused.'
	};

	const labels = { running: 'Running', finished: 'Finished', failed: 'Failed', cancelled: 'Cancelled' } as const;
	const dots = { running: 'pulse', finished: 'ok', failed: 'bad', cancelled: 'off' } as const;

	// A clock that ticks while a run is going.
	let now = $state(Date.now());
	$effect(() => {
		if (!running) return;
		now = Date.now();
		const timer = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(timer);
	});
	const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
	const elapsed = $derived.by(() => {
		if (!run) return '';
		const end = run.finishedAt ? new Date(run.finishedAt).getTime() : now;
		return clock(Math.max(0, Math.round((end - new Date(run.startedAt).getTime()) / 1000)));
	});

	// Every run is kept in the thread as a note when it is over. The latest is shown above, with its output, so
	// the list holds the others; and once a run has its note, the card says where it is.
	const history = $derived(prompt.runs.filter((r) => r.id !== run?.id));
	const noted = $derived(!!run && prompt.runs.some((r) => r.id === run.id));

	// Follow the end of the output while it grows, unless it was scrolled up to read.
	let outputBox = $state<HTMLPreElement>();
	let following = true;
	function onscroll() {
		if (outputBox) following = outputBox.scrollTop + outputBox.clientHeight >= outputBox.scrollHeight - 24;
	}
	$effect(() => {
		void tracked?.output.length;
		if (outputBox && following) outputBox.scrollTop = outputBox.scrollHeight;
	});
</script>

<div class="claude">
	<div class="claude-row">
		<button class="link accent" onclick={ask} disabled={asking || running}>Run in Claude Code…</button>
		{#if run}
			<span class="state"><span class="dot {dots[run.status]}"></span>{run.cancelling ? 'Cancelling…' : labels[run.status]}</span>
			<span class="hint">{elapsed}{run.exitCode !== null && run.status !== 'finished' ? ` · exit code ${run.exitCode}` : ''}</span>
			{#if running}
				<button class="link danger" onclick={cancel} disabled={run.cancelling}>Cancel</button>
			{/if}
			<button class="link" onclick={() => (showOutput = !showOutput)}>{showOutput ? 'Hide output' : 'Show output'}</button>
		{/if}
	</div>

	{#if run}
		{#if run.message && run.status !== 'finished'}
			<div class="notice" class:bad={run.status === 'failed'} class:info={run.status === 'cancelled'}>{run.message}</div>
		{/if}
		{#if run.linked.length}
			<div class="hint">Published, and linked to this prompt: {run.linked.join(', ')}</div>
		{/if}
		{#if showOutput}
			<pre class="mono output" bind:this={outputBox} {onscroll}>{tracked?.output || (running ? 'Waiting for the first output…' : 'No output.')}</pre>
		{/if}
		{#if noted}
			<div class="hint">Kept in the thread as a note: <span class="mono">runs/{run.id}.md</span></div>
		{/if}
	{/if}

	{#if history.length}
		<details class="history">
			<summary><ChevronRight size={14} class="disclosure" />Earlier runs ({history.length})</summary>
			<div class="past-list">
				{#each history as past (past.id)}
					<details class="past">
						<summary>
							<ChevronRight size={14} class="disclosure" /><span class="dot {dots[past.status]}"></span>{labels[past.status]}
							<span class="hint">
								{time(past.startedAt)} · {clock(past.seconds)}{past.turns ? ` · ${past.turns} turn${past.turns === 1 ? '' : 's'}` : ''}{past.model
									? ` · ${past.model}`
									: ''}
							</span>
						</summary>
						<Markdown html={past.html} />
						{#if past.files.length}<div class="hint">Wrote: {past.files.join(', ')}</div>{/if}
						<div class="hint">Kept in the thread as <span class="mono">runs/{past.id}.md</span></div>
					</details>
				{/each}
			</div>
		</details>
	{/if}
</div>

{#if preview}
	{@const shown = preview}
	<Modal title="Run in Claude Code" wide onclose={() => (preview = null)}>
		<p style="margin:0;color:var(--muted)">
			Claude Code runs on this machine in the project’s folder, and writes what it makes into <code>deliverables/</code> there:
		</p>
		<div class="mono folder">{shown.folder}</div>

		<div class="notice info">
			{modeNotes[shown.permissionMode]}
			{shown.guard
				? 'The extra guard rails are on: it is told not to read or write outside this folder, nor to touch the vault’s .ledger folder or any .env file, and neither the folder’s own Claude settings nor any MCP servers are used.'
				: 'The extra guard rails are off.'}
		</div>
		<div class="notice info">
			When it finishes, each deliverable it wrote is published as a version and that version is linked to this prompt: the first version for a new
			file, the next minor for one that already existed. If you had edited a file since its last version, those edits are published as a version first,
			so nothing of yours is lost under the run’s changes; that one is not linked.
		</div>
		{#if shown.problem}
			<div class="notice bad">{shown.problem} <a href="/settings/tools" onclick={() => (preview = null)}>Open Settings</a></div>
		{/if}

		<details open>
			<summary><ChevronRight size={14} class="disclosure" />What will be sent</summary>
			<div class="hint" style="margin:6px 0">This text goes to Claude Code on its standard input, exactly as shown.</div>
			<pre class="mono sent">{shown.prompt}</pre>
		</details>
		<details>
			<summary><ChevronRight size={14} class="disclosure" />The command</summary>
			<div class="hint" style="margin:6px 0">Shown for reading. It is started directly with these arguments, never through a shell.</div>
			<pre class="mono sent">{shown.command}</pre>
		</details>
		{#if shown.guard}
			<details>
				<summary><ChevronRight size={14} class="disclosure" />The guard rails</summary>
				<div class="hint" style="margin:6px 0">The settings file given with <code>--settings</code>.</div>
				<pre class="mono sent">{shown.guard}</pre>
			</details>
		{/if}
		{#if error}<div class="notice bad">{error}</div>{/if}

		{#snippet footer()}
			<span class="spacer-h"></span>
			<button class="btn ghost" onclick={() => (preview = null)}>Cancel</button>
			<button class="btn primary" onclick={start} disabled={starting || !!shown.problem} data-autofocus>Run in Claude Code</button>
		{/snippet}
	</Modal>
{/if}

<style>
	.claude {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.claude-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 14px;
	}
	.state {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 12px;
		font-weight: 600;
	}
	.output,
	.sent {
		margin: 0;
		padding: 10px 12px;
		background: var(--bg);
		border: 1px solid var(--line);
		border-radius: 8px;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		max-height: 320px;
		overflow: auto;
		color: var(--ink);
	}
	.folder {
		padding: 6px 10px;
		background: var(--bg);
		border: 1px solid var(--line);
		border-radius: 8px;
		overflow-wrap: anywhere;
	}
	summary {
		cursor: pointer;
		font-size: 13px;
		font-weight: 600;
	}
	.past-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}
	.past {
		padding-left: 10px;
		border-left: 2px solid var(--line);
	}
	.past > summary {
		font-weight: 500;
	}
	.past > summary .hint {
		margin-left: 6px;
	}
</style>
