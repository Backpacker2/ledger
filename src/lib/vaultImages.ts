// Pictures in markdown point at files beside it (`![](chart.png)`, `![](../attachments/shot.png)`), which the
// webview can't open itself: they are read through the backend, as FileImage does, and shown from blob URLs. Web
// pictures (http, https) are shown as they are. Used by the rendered page (Markdown.svelte) and the editor.

import { mimeOf } from '$lib/actions';
import { api } from '$lib/api';

/** Where a markdown file's pictures are looked for: its project, and its folder in that project. */
export interface ImageBase {
	project: string;
	dir: string;
}

/** The folder of a project path, for `ImageBase.dir`. */
export const dirOf = (path: string) => path.slice(0, path.lastIndexOf('/') + 1);

/** Whether a picture's address is a web one, shown as it is. */
export const isWebSrc = (src: string) => /^https?:/i.test(src.trim());

/** Whether a picture's address is a file in the vault: no scheme (so not `data:` or `javascript:` either). */
export const isVaultSrc = (src: string) => !/^[a-z][a-z0-9+.-]*:/i.test(src.trim()) && !src.startsWith('//') && !src.startsWith('#');

/** The project path `src` points at from `dir`, or null when it would leave the project. A leading `/` is the project's own folder. */
export function resolveSrc(dir: string, src: string): string | null {
	let path = src.trim().split(/[?#]/)[0];
	try {
		path = decodeURIComponent(path);
	} catch {
		// Not %-encoded after all: as written.
	}
	const parts = path.startsWith('/') ? [] : dir.split('/').filter(Boolean);
	for (const part of path.split('/')) {
		if (part === '' || part === '.') continue;
		if (part !== '..') parts.push(part);
		else if (!parts.pop()) return null;
	}
	return parts.length ? parts.join('/') : null;
}

export type ImageLoad = (src: string) => Promise<string | null>;

/**
 * Loads pictures for one view: `load` gives the URL to show a picture's address from (null: it can't be shown), and
 * reads each vault file once; `dispose` lets go of them. `base` is asked each time, so it may change along the way.
 */
export function imageLoader(base: () => ImageBase | undefined): { load: ImageLoad; dispose: () => void } {
	const cache = new Map<string, Promise<string | null>>();
	let disposed = false;

	const load: ImageLoad = (src) => {
		if (isWebSrc(src)) return Promise.resolve(src.trim());
		const at = base();
		const path = at && isVaultSrc(src) ? resolveSrc(at.dir, src) : null;
		if (!at || !path) return Promise.resolve(null);
		const key = `${at.project}/${path}`;
		let url = cache.get(key);
		if (!url) {
			url = api.readProjectFile(at.project, path).then(
				(bytes) => (disposed ? null : URL.createObjectURL(new Blob([bytes], { type: mimeOf(path) }))),
				() => {
					cache.delete(key); // not there (yet): try again next time
					return null;
				}
			);
			cache.set(key, url);
		}
		return url;
	};

	const dispose = () => {
		disposed = true;
		// A moment later, as FileImage does: a picture may still be on screen for the rest of this update.
		const urls = [...cache.values()];
		setTimeout(() => urls.forEach((url) => url.then((u) => u && URL.revokeObjectURL(u))), 2000);
	};

	return { load, dispose };
}
