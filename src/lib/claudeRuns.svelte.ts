// What this window knows about the runs of prompts with Claude Code. The backend keeps the runs going whatever
// screen is open; this keeps what the window has heard about them, for as long as the window lives, so a run is
// still on its prompt when you come back to it. It starts listening the first time a screen asks for it.

import { listen } from '@tauri-apps/api/event';
import { claudeCodeApi } from './api';
import { toast } from './ui.svelte';
import type { ClaudeRun, ClaudeRunEvent } from './types';

export interface TrackedRun {
	run: ClaudeRun;
	/** The output so far. */
	output: string;
	/** How many pieces of output `output` is made of; the backend numbers them, so a missed one is noticed. */
	have: number;
}

export const claudeRuns = $state<{ byId: Record<string, TrackedRun> }>({ byId: {} });

let started = false;
const catchingUp = new Set<string>();

/** Takes a run and its output as the truth, unless this window already knows more. */
function adopt(run: ClaudeRun, output: string) {
	const known = claudeRuns.byId[run.id];
	if (known && known.have > run.chunks) return;
	claudeRuns.byId[run.id] = { run, output, have: run.chunks };
}

/** Asks the backend for the whole output of a run, after a piece went missing. */
async function catchUp(id: string) {
	if (catchingUp.has(id)) return;
	catchingUp.add(id);
	try {
		const snapshot = await claudeCodeApi.run(id);
		adopt(snapshot.run, snapshot.output);
	} catch {
		// The run is no longer listed; whatever is shown stays.
	} finally {
		catchingUp.delete(id);
	}
}

function announce(known: TrackedRun | undefined, run: ClaudeRun) {
	// Only a run seen going and then ending is news: not one that was already over when the window found out.
	if (known?.run.status !== 'running' || run.status === 'running') return;
	const title = `“${run.promptTitle || 'prompt'}”`;
	if (run.status === 'finished') {
		const added = run.linked.length;
		toast(`Claude Code finished ${title}${added ? `: ${added} version${added === 1 ? '' : 's'} published and linked to the prompt` : ''}`);
	} else if (run.status === 'cancelled') {
		toast(`Stopped the Claude Code run for ${title}`);
	} else {
		toast(`The Claude Code run for ${title} failed${run.message ? `: ${run.message}` : ''}`, true);
	}
}

function apply({ run, chunk }: ClaudeRunEvent) {
	const known = claudeRuns.byId[run.id];
	let output = known?.output ?? '';
	let have = known?.have ?? 0;
	let missed = false;
	if (chunk) {
		if (chunk.seq === have) {
			output += chunk.text;
			have += 1;
		} else if (chunk.seq > have) {
			missed = true;
		}
		// A lower number is a piece already held.
	}
	announce(known, run);
	claudeRuns.byId[run.id] = { run, output, have };
	if (missed) void catchUp(run.id);
}

/** Starts listening, once. Safe to call from every screen that shows runs. */
export function ensureClaudeRuns() {
	if (started) return;
	started = true;
	listen<ClaudeRunEvent>('claude-run', (event) => apply(event.payload)).catch(() => {
		started = false;
	});
	claudeCodeApi
		.runs()
		.then((all) => all.forEach((s) => adopt(s.run, s.output)))
		.catch(() => {});
}

/** A run that was just started: shown at once, before its first event arrives. */
export function trackRun(run: ClaudeRun) {
	adopt(run, '');
}

/** The latest run of a prompt, still running or not. */
export function latestRunFor(promptId: string): TrackedRun | undefined {
	let latest: TrackedRun | undefined;
	for (const tracked of Object.values(claudeRuns.byId)) {
		if (tracked.run.promptId !== promptId) continue;
		if (!latest || tracked.run.startedAt > latest.run.startedAt) latest = tracked;
	}
	return latest;
}
