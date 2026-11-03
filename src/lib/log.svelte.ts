// The app's log, from the window's side: every failed command and every uncaught error is written to it (the
// backend keeps the file, in the app's log folder, never in the vault), and the latest error is kept at hand so
// it can be reported (see report.ts).
//
// A debug capture records more for a while: from Start to Stop the backend also keeps `debug` entries (each action
// asked for, each page opened, each message shown), and Stop turns them into a file to send to the developer.
//
// This module talks to the backend with `invoke` directly, not through api.ts: api.ts logs its failures here.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { captureText } from './report';
import type { AboutInfo, CaptureStatus, LogEntry } from './types';

export const appLog = $state<{ latestError: LogEntry | null; changes: number }>({ latestError: null, changes: 0 });

/** A finished capture: the file it was saved as, and what it holds. */
export interface FinishedCapture {
	path: string;
	entries: LogEntry[];
	text: string;
	startedAt: string;
	endedAt: string;
}

export const capture = $state<{ active: boolean; startedAt: string | null; events: number; busy: boolean; last: FinishedCapture | null }>({
	active: false,
	startedAt: null,
	events: 0,
	busy: false,
	last: null
});

const inTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Writes an error to the log. Never throws: logging must not be what breaks. */
export function logError(source: string, message: string) {
	if (!inTauri()) {
		console.error(`[${source}]`, message);
		return;
	}
	invoke('log_write', { level: 'error', source, message }).catch(() => {});
}

/** Adds a step to the running debug capture; nothing happens without one. Never throws. */
export function captureDebug(source: string, message: string) {
	if (!capture.active || !inTauri()) return;
	invoke('capture_debug', { source, message }).catch(() => {});
}

function showStatus(status: CaptureStatus) {
	capture.active = status.active;
	capture.startedAt = status.startedAt;
	capture.events = status.events;
}

export async function startCapture() {
	capture.busy = true;
	try {
		showStatus(await invoke<CaptureStatus>('capture_start'));
		capture.last = null;
	} finally {
		capture.busy = false;
	}
}

/** Ends the capture, takes out what shouldn't be shared, and saves it as a file next to the log. */
export async function stopCapture(): Promise<FinishedCapture> {
	capture.busy = true;
	try {
		const entries = await invoke<LogEntry[]>('capture_stop');
		const about = await invoke<AboutInfo>('about_info').catch(() => null);
		const text = captureText(entries, about);
		const path = await invoke<string>('capture_save', { text });
		const finished = { path, entries, text, startedAt: entries[0]?.at ?? '', endedAt: entries.at(-1)?.at ?? '' };
		capture.last = finished;
		showStatus({ active: false, startedAt: null, events: 0 });
		return finished;
	} finally {
		capture.busy = false;
	}
}

/**
 * Picks up the latest error already in the log, follows new entries as they are written, and logs errors the
 * window doesn't catch. Returns the cleanup.
 */
export function startLogEvents(): () => void {
	if (!inTauri()) return () => {};
	let alive = true;
	let unlisten: (() => void) | null = null;

	invoke<LogEntry[]>('log_read', { limit: 200 })
		.then((entries) => {
			const last = entries.findLast((e) => e.level === 'error');
			if (alive && last && !appLog.latestError) appLog.latestError = last;
		})
		.catch(() => {});
	// A capture a crash cut short carries on after the restart: the bar shows it again.
	invoke<CaptureStatus>('capture_status').then(showStatus, () => {});
	listen<LogEntry>('log-entry', (event) => {
		if (capture.active) capture.events++;
		if (event.payload.level === 'debug') return;
		if (event.payload.level === 'error') appLog.latestError = event.payload;
		appLog.changes++;
	})
		.then((u) => (alive ? (unlisten = u) : u()))
		.catch(() => {});

	const onError = (e: ErrorEvent) => {
		const where = e.filename ? ` (${e.filename.replace(location.origin, '')}:${e.lineno}:${e.colno})` : '';
		logError('ui', `${e.message}${where}${e.error instanceof Error && e.error.stack ? `\n${e.error.stack}` : ''}`);
	};
	const onRejection = (e: PromiseRejectionEvent) => {
		const reason = e.reason instanceof Error ? `${e.reason.message}${e.reason.stack ? `\n${e.reason.stack}` : ''}` : String(e.reason);
		logError('ui', `Unhandled: ${reason}`);
	};
	window.addEventListener('error', onError);
	window.addEventListener('unhandledrejection', onRejection);

	return () => {
		alive = false;
		unlisten?.();
		window.removeEventListener('error', onError);
		window.removeEventListener('unhandledrejection', onRejection);
	};
}
