// The update notice: what the backend found, whether "Later" hid it, and how far installing has got. The backend
// looks for releases by itself (release builds only) and says so with `update-available`; installing downloads the
// update once, waits until a restart cuts nothing off, and ends in the restart (see src-tauri/src/updates.rs).

import { listen } from '@tauri-apps/api/event';
import { updatesApi } from './api';
import type { RunsChoice, UpdateInfo, UpdateProgress } from './types';

/**
 * - `idle`: the notice offers *Install and restart* and *Later*.
 * - `working`: downloading (with progress), checking the signature, installing.
 * - `ask`: Claude Code is still running prompts; wait for them or stop them?
 * - `waiting`: downloaded; installs as soon as what is still going is over.
 * - `simulated`: a development build's pretend update went through every step.
 * - `failed`: said why, and nothing was installed.
 */
export type UpdatePhase = 'idle' | 'working' | 'ask' | 'waiting' | 'simulated' | 'failed';

export const updates = $state({
	available: null as UpdateInfo | null,
	/** *Later* was pressed: hidden until the next check finds the update again. */
	hidden: false,
	phase: 'idle' as UpdatePhase,
	waitingFor: [] as string[],
	runs: [] as string[],
	progress: null as UpdateProgress | null,
	error: null as string | null
});

// ---- before the restart, whatever is typed and not yet saved is saved ---------------------------------

const flushers = new Set<() => unknown>();

/** Something to save before Ledger restarts for an update, such as an editor's pending autosave. Returns the undo. */
export function beforeRestart(flush: () => unknown): () => void {
	flushers.add(flush);
	return () => {
		flushers.delete(flush);
	};
}

async function flushAll() {
	for (const flush of [...flushers]) {
		try {
			await flush();
		} catch {
			// What failed to save says so itself; the backend still checks everything else.
		}
	}
}

// ---- found, hidden, shown -----------------------------------------------------------------------------

/** Shows the notice for this update, unless installing is already under way. */
export function showUpdate(info: UpdateInfo) {
	if (updates.phase === 'working' || updates.phase === 'waiting' || updates.phase === 'ask') {
		if (updates.available?.version === info.version) updates.hidden = false;
		return;
	}
	updates.available = info;
	updates.hidden = false;
	updates.phase = 'idle';
	updates.error = null;
}

/** *Later*: hides the notice until the next check, and stops waiting to install. */
export function later() {
	stopWaiting();
	updates.hidden = true;
}

/** Listens for what the backend finds, and asks what it already found before the window was listening. */
export function startUpdates(): () => void {
	const cleanups: Array<() => void> = [];
	let alive = true;
	const keep = (unlisten: () => void) => (alive ? cleanups.push(unlisten) : unlisten());

	listen<UpdateInfo>('update-available', (event) => showUpdate(event.payload))
		.then(keep)
		.catch(() => {});
	listen<UpdateProgress>('update-progress', (event) => {
		updates.progress = event.payload;
	})
		.then(keep)
		.catch(() => {});
	updatesApi
		.status()
		.then((view) => {
			if (view.available && !updates.available) showUpdate(view.available);
		})
		.catch(() => {});

	return () => {
		alive = false;
		cleanups.forEach((fn) => fn());
	};
}

// ---- installing ---------------------------------------------------------------------------------------

/** Bumped to stop a wait in progress: a loop whose number is no longer current stops. */
let attempt = 0;

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * *Install and restart*. Asks the backend again every second while the answer is to wait; the update is downloaded
 * once. A real install that goes through never comes back: Ledger restarts.
 */
export async function installUpdate(choice: RunsChoice = 'ask') {
	const mine = ++attempt;
	updates.error = null;
	updates.phase = 'working';
	updates.progress = null;
	while (mine === attempt) {
		await flushAll();
		let step;
		try {
			step = await updatesApi.install(choice);
		} catch (e) {
			if (mine !== attempt) return;
			updates.phase = 'failed';
			updates.error = e instanceof Error ? e.message : String(e);
			return;
		}
		if (mine !== attempt) return;
		if (step.state === 'ask') {
			updates.phase = 'ask';
			updates.runs = step.runs;
			return;
		}
		if (step.state === 'simulated') {
			updates.phase = 'simulated';
			return;
		}
		updates.phase = 'waiting';
		updates.waitingFor = step.waitingFor;
		await pause(1000);
	}
}

/** Stops waiting to install; the downloaded update is kept for the next try. */
export function stopWaiting() {
	attempt++;
	if (updates.phase !== 'idle') {
		updates.phase = 'idle';
		updates.progress = null;
		updates.error = null;
	}
}
