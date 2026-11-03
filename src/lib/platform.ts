// Where the screens run: inside the desktop app (Tauri), or in a browser talking to `ledger-server` (the mobile
// version, see README → "The mobile server"). Everything that differs between the two lives here, so the rest of
// the app imports `invoke`, `listen`, `openUrl` and friends from this file instead of from `@tauri-apps/*`.

import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event';
import { open as tauriOpenDialog } from '@tauri-apps/plugin-dialog';
import { openUrl as tauriOpenUrl, revealItemInDir as tauriReveal } from '@tauri-apps/plugin-opener';

/** True inside the desktop app; false in a browser served by `ledger-server`. */
export const desktop = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** True in a browser served by `ledger-server`. */
export const web = typeof window !== 'undefined' && !desktop;

/** Thrown for something only the desktop app can do. */
export class DesktopOnly extends Error {
	constructor(what = 'This') {
		super(`${what} is only available in the desktop app`);
		this.name = 'DesktopOnly';
	}
}

/** Runs a backend command: over IPC in the desktop app, as `POST /api/<command>` in the browser. */
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
	if (desktop) return tauriInvoke<T>(command, args);
	if (typeof window === 'undefined') throw new Error('Ledger needs its backend: the desktop app or ledger-server.');
	let response: Response;
	try {
		response = await fetch(`/api/${command}`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(args ?? {})
		});
	} catch {
		throw new Error('Ledger can’t reach its server. Is the Mac at home on, and are you on Tailscale?');
	}
	const text = await response.text();
	let body: unknown = null;
	try {
		body = text ? JSON.parse(text) : null;
	} catch {
		body = text;
	}
	if (!response.ok) {
		const message = typeof body === 'string' && body ? body : `${command} failed (${response.status})`;
		// The same shape `invoke` rejects with in the desktop app: a plain message.
		throw response.status === 404 && message.includes('only available in the desktop app') ? new DesktopOnly(command) : message;
	}
	return body as T;
}

// ---- events -----------------------------------------------------------------------------------------

type Handler = (event: { event: string; id: number; payload: unknown }) => void;
const handlers = new Map<string, Set<Handler>>();
let source: EventSource | null = null;
let nextId = 0;

/** One stream for the whole page, opened when the first listener comes. The browser reconnects it by itself. */
function ensureSource() {
	if (source || typeof EventSource === 'undefined') return;
	source = new EventSource('/api/events');
}

function attach(event: string) {
	ensureSource();
	source?.addEventListener(event, (message) => {
		let payload: unknown = null;
		try {
			payload = JSON.parse((message as MessageEvent<string>).data);
		} catch {
			// An event without a payload.
		}
		for (const handler of handlers.get(event) ?? []) handler({ event, id: nextId++, payload });
	});
}

/** Listens for something the backend announces; resolves to the function that stops listening. */
export async function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
	if (desktop) return tauriListen<T>(event, handler);
	if (!handlers.has(event)) {
		handlers.set(event, new Set());
		attach(event);
	}
	const wrapped = handler as unknown as Handler;
	handlers.get(event)!.add(wrapped);
	return () => {
		handlers.get(event)?.delete(wrapped);
	};
}

// ---- the system: links, files, the file dialog --------------------------------------------------------

/** Opens a web link in the system's browser (desktop) or a new tab. */
export async function openUrl(url: string): Promise<void> {
	if (desktop) return tauriOpenUrl(url);
	window.open(url, '_blank', 'noopener,noreferrer');
}

/** Shows a file in the system's file manager. Desktop only. */
export async function revealItemInDir(path: string): Promise<void> {
	if (desktop) return tauriReveal(path);
	throw new DesktopOnly('Showing a file in its folder');
}

/** The system's open dialog. Desktop only: the browser picks files with [`pickFiles`] instead. */
export const openDialog: typeof tauriOpenDialog = (options) => {
	if (desktop) return tauriOpenDialog(options);
	throw new DesktopOnly('Choosing a folder');
};

/** A file to upload: a path on this machine (desktop) or a file the browser picked (phone). */
export type Picked = string | File;

export const pickedName = (file: Picked) => (typeof file === 'string' ? (file.split(/[\\/]/).pop() ?? file) : file.name);

/** Lets the person pick files to upload; `null` when they cancel. On a phone this offers the camera too. */
export async function pickFiles(title: string): Promise<Picked[] | null> {
	if (desktop) {
		const picked = await tauriOpenDialog({ multiple: true, title });
		if (!picked) return null;
		return Array.isArray(picked) ? picked : [picked];
	}
	return new Promise((resolve) => {
		const input = document.createElement('input');
		input.type = 'file';
		input.multiple = true;
		input.style.display = 'none';
		input.addEventListener('change', () => {
			resolve(input.files?.length ? Array.from(input.files) : null);
			input.remove();
		});
		input.addEventListener('cancel', () => {
			resolve(null);
			input.remove();
		});
		document.body.appendChild(input);
		input.click();
	});
}

/** The URL of a file in a project's folder, as `ledger-server` serves it. */
export const fileUrl = (project: string, path: string) =>
	`/api/file?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`;
