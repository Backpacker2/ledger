// App-wide UI state: toasts, modal dialogs, the theme and the phone-width drawer. Kept as module-level
// runes so any component can raise a toast or ask a question without prop drilling.

import { invalidateAll } from '$app/navigation';

// ---- toasts -----------------------------------------------------------------------------------------

export interface Toast {
	id: number;
	message: string;
	error: boolean;
}

export const toasts = $state<Toast[]>([]);
let nextToastId = 1;

export function toast(message: string, error = false) {
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

export const themes = [
	{ id: 'dark', name: 'Dark', swatch: ['#0e1016', '#191c25', '#1f2330', '#7c7cf0'] },
	{ id: 'night', name: 'Night', swatch: ['#000000', '#0a0a0c', '#111114', '#8f8ff5'] },
	{ id: 'forest', name: 'Forest', swatch: ['#0b110d', '#131c17', '#18241d', '#4cc38a'] },
	{ id: 'ocean', name: 'Ocean', swatch: ['#0a111a', '#111b29', '#152234', '#4ea8ff'] },
	{ id: 'sepia', name: 'Sepia', swatch: ['#3b2f22', '#fbf5e9', '#fffaf0', '#b0602c'] },
	{ id: 'light', name: 'Light', swatch: ['#1c1f2a', '#ffffff', '#f6f6f3', '#5b5bd6'] }
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

// ---- phone-width drawer -----------------------------------------------------------------------------

export const drawer = $state({ open: false });
