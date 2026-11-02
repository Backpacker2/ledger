// Tauri has no Node.js server to render on, so the app is a static single-page app:
// https://v2.tauri.app/start/frontend/sveltekit/
import { api } from '$lib/api';
import type { LayoutLoad } from './$types';

export const ssr = false;

export const load: LayoutLoad = async ({ url }) => {
	// The screenshot overlay is its own window and needs none of this.
	if (url.pathname.startsWith('/screenshot-overlay')) return { projects: [] };
	return { projects: await api.listProjects() };
};
