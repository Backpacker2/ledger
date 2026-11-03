// App-wide UI state: toasts, modal dialogs, the theme and the phone-width drawer. Kept as module-level
// runes so any component can raise a toast or ask a question without prop drilling.

import { listen } from '@tauri-apps/api/event';
import { invalidateAll } from '$app/navigation';
import { api } from './api';
import type { ScreenshotTaken, SyncStatus } from './types';
import { captureDebug } from './log.svelte';

// ---- toasts -----------------------------------------------------------------------------------------

export interface Toast {
	id: number;
	message: string;
	error: boolean;
}

export const toasts = $state<Toast[]>([]);
let nextToastId = 1;

export function toast(message: string, error = false) {
	captureDebug('toast', error ? `error: ${message}` : message);
	const id = nextToastId++;
	toasts.push({ id, message, error });
	setTimeout(() => {
		const index = toasts.findIndex((t) => t.id === id);
		if (index >= 0) toasts.splice(index, 1);
	}, error ? 5000 : 2600);
}

/** Runs an action, toasting the failure instead of throwing. Resolves to `undefined` when it failed. */
export async function attempt<T>(action: () => Promise<T>): Promise<T | undefined> {
	try {
		return await action();
	} catch (e) {
		toast(e instanceof Error ? e.message : String(e), true);
		return undefined;
	}
}

/** Re-runs every load function, so lists and the open page pick up what a command just changed. */
export const refresh = () => invalidateAll();

// ---- dialogs ----------------------------------------------------------------------------------------

export interface ConfirmOptions {
	title: string;
	message?: string;
	confirmLabel?: string;
	danger?: boolean;
}

export interface PromptOptions {
	title: string;
	label?: string;
	initial?: string;
	placeholder?: string;
	confirmLabel?: string;
}

type DialogState =
	| ({ kind: 'confirm'; resolve: (ok: boolean) => void } & ConfirmOptions)
	| ({ kind: 'prompt'; resolve: (value: string | null) => void } & PromptOptions);

export const dialog = $state<{ current: DialogState | null }>({ current: null });

export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
	return new Promise((resolve) => {
		dialog.current = { kind: 'confirm', ...options, resolve };
	});
}

export function promptDialog(options: PromptOptions): Promise<string | null> {
	return new Promise((resolve) => {
		dialog.current = { kind: 'prompt', ...options, resolve };
	});
}

// ---- theme ------------------------------------------------------------------------------------------

// Each theme's colours live in app.css; the settings swatches preview them with data-theme.
export const themes = [
	{ id: 'dark', name: 'Dark' },
	{ id: 'night', name: 'Night' },
	{ id: 'forest', name: 'Forest' },
	{ id: 'ocean', name: 'Ocean' },
	{ id: 'sepia', name: 'Sepia' },
	{ id: 'light', name: 'Light' }
] as const;

const THEME_KEY = 'ledger-theme';

export const theme = $state({ id: 'dark' as string });

export function initTheme() {
	try {
		const saved = localStorage.getItem(THEME_KEY);
		if (saved && themes.some((t) => t.id === saved)) theme.id = saved;
	} catch {
		// Storage can be unavailable; the default theme is fine.
	}
	document.documentElement.dataset.theme = theme.id;
}

export function setTheme(id: string) {
	theme.id = id;
	document.documentElement.dataset.theme = id;
	try {
		localStorage.setItem(THEME_KEY, id);
	} catch {
		// Not persisted, still applied for this session.
	}
}

// ---- the backend's news ----------------------------------------------------------------------------

/** Where the sync worker has got to; kept current by events from the backend. */
export const sync = $state<{ status: SyncStatus | null }>({ status: null });

/** A screenshot has been taken and waits for a thread to be attached to (the backend holds the picture). */
export const screenshot = $state<{ pending: ScreenshotTaken | null }>({ pending: null });

/**
 * Listens for what the backend announces — sync progress, files changed by a pull — and checks the vault for
 * edits made elsewhere (in Obsidian, say) whenever the window regains focus. Returns the cleanup.
 */
export function startBackendEvents(): () => void {
	const cleanups: Array<() => void> = [];
	let alive = true;
	const keep = (unlisten: () => void) => (alive ? cleanups.push(unlisten) : unlisten());

	api.syncStatus()
		.then((status) => {
			sync.status = status;
		})
		.catch(() => {});
	listen<SyncStatus>('sync-status', (event) => {
		sync.status = event.payload;
	})
		.then(keep)
		.catch(() => {});
	listen('vault-changed', () => {
		void refresh();
	})
		.then(keep)
		.catch(() => {});

	// A screenshot was taken: the main window asks where it belongs. A failure is said out loud.
	listen<ScreenshotTaken>('screenshot-ready', (event) => {
		screenshot.pending = event.payload;
	})
		.then(keep)
		.catch(() => {});
	listen<string>('screenshot-error', (event) => toast(event.payload, true))
		.then(keep)
		.catch(() => {});
	// Ledger may have been launched with a picture (Flameshot's "Open with") before anything was listening.
	api.pendingScreenshot()
		.then((taken) => {
			if (taken && !screenshot.pending) screenshot.pending = taken;
		})
		.catch(() => {});

	let lastCheck = 0;
	const onFocus = async () => {
		if (Date.now() - lastCheck < 2000) return;
		lastCheck = Date.now();
		try {
			if (await api.checkVaultChanges()) {
				toast('Reloaded changes made outside Ledger');
				await refresh();
			}
		} catch {
			// Not worth interrupting anyone for.
		}
	};
	window.addEventListener('focus', onFocus);

	return () => {
		alive = false;
		window.removeEventListener('focus', onFocus);
		cleanups.forEach((fn) => fn());
	};
}

// ---- phone-width drawer -----------------------------------------------------------------------------

export const drawer = $state({ open: false });
