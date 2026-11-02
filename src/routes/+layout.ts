// Tauri has no Node.js server to render on, so the app is a static single-page app:
// https://v2.tauri.app/start/frontend/sveltekit/
import { api } from '$lib/api';
import type { LayoutLoad } from './$types';

export const ssr = false;

export const load: LayoutLoad = async () => ({ threads: await api.listThreads() });
