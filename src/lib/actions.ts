import { goto } from '$app/navigation';
import { api } from './api';
import { attempt, promptDialog, refresh } from './ui.svelte';

/** Asks for a title, creates the thread and opens it. */
export async function newThreadFlow() {
	const title = await promptDialog({
		title: 'New thread',
		label: 'What are you thinking about?',
		placeholder: 'e.g. Pricing model',
		confirmLabel: 'Create'
	});
	if (!title) return;
	const created = await attempt(() => api.createThread(title));
	if (!created) return;
	await refresh();
	await goto(`/threads/${created.slug}`);
}

/** The next version a bump would publish, mirroring the backend: "M.m" from the highest published one. */
export function nextVersion(versions: string[], bump: 'minor' | 'major'): string {
	const [major, minor] = (versions.at(-1) ?? '0.0').split('.').map(Number);
	return bump === 'major' ? `${major + 1}.0` : `${major}.${minor + 1}`;
}
