import { goto } from '$app/navigation';
import { api } from './api';
import { attempt, confirmDialog, promptDialog, refresh } from './ui.svelte';
import type { Bump } from './types';

export const projectUrl = (project: string) => `/projects/${project}`;
export const threadUrl = (project: string, thread: string) => `/projects/${project}/threads/${thread}`;
export const deliverableUrl = (project: string, path: string) =>
	`/projects/${project}/deliverables/view?path=${encodeURIComponent(path)}`;

/** Asks for a name, creates the project and opens it. */
export async function newProjectFlow() {
	const name = await promptDialog({
		title: 'New project',
		label: 'What is it called?',
		placeholder: 'e.g. Pricing overhaul',
		confirmLabel: 'Create'
	});
	if (!name) return;
	const created = await attempt(() => api.createProject(name));
	if (!created) return;
	await refresh();
	await goto(projectUrl(created.slug));
}

/** Asks for a title, creates the thread in the project and opens it. */
export async function newThreadFlow(project: string) {
	const title = await promptDialog({
		title: 'New thread',
		label: 'What are you thinking about?',
		placeholder: 'e.g. Pricing model',
		confirmLabel: 'Create'
	});
	if (!title) return;
	const created = await attempt(() => api.createThread(project, title));
	if (!created) return;
	await refresh();
	await goto(threadUrl(project, created.slug));
}

export async function renameProjectFlow(slug: string, current: string) {
	const name = await promptDialog({ title: 'Rename project', label: 'Name', initial: current, confirmLabel: 'Rename' });
	if (!name || name === current) return;
	if (await attempt(() => api.renameProject(slug, name))) await refresh();
}

export async function deleteProjectFlow(slug: string, name: string) {
	const ok = await confirmDialog({
		title: `Delete “${name}”?`,
		message: 'Its threads, deliverables, stories, tasks and uploaded files are removed from the vault (and from GitHub at the next sync).',
		confirmLabel: 'Delete project',
		danger: true
	});
	if (!ok) return;
	if ((await attempt(() => api.deleteProject(slug))) === undefined) return;
	await refresh();
	await goto('/');
}

/** The next version a publish would create, mirroring the backend: "M.m" from the highest published one. */
export function nextVersion(versions: string[], bump: Bump): string {
	const [major, minor] = (versions.at(-1) ?? '0.0').split('.').map(Number);
	return bump === 'major' ? `${major + 1}.0` : `${major}.${minor + 1}`;
}

/** "reports/plan.md" + "1.2" → "reports/plan@v1.2.md", as the backend names a published version. */
export function snapshotPath(base: string, version: string): string {
	const slash = base.lastIndexOf('/');
	const dir = slash < 0 ? '' : base.slice(0, slash + 1);
	const file = base.slice(slash + 1);
	const dot = file.lastIndexOf('.');
	return dir + (dot > 0 ? `${file.slice(0, dot)}@v${version}${file.slice(dot)}` : `${file}@v${version}`);
}

/** The file name of a deliverable path, without its folders. */
export const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1);

/** Where a project's deliverable lives relative to the project's folder (what file commands take). */
export const deliverableFile = (path: string) => `deliverables/${path}`;

export function mimeOf(name: string): string {
	const ext = name.split('.').pop()?.toLowerCase() ?? '';
	const types: Record<string, string> = {
		png: 'image/png',
		jpg: 'image/jpeg',
		jpeg: 'image/jpeg',
		gif: 'image/gif',
		webp: 'image/webp',
		svg: 'image/svg+xml',
		bmp: 'image/bmp',
		avif: 'image/avif',
		pdf: 'application/pdf'
	};
	return types[ext] ?? 'application/octet-stream';
}

export function formatSize(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10_240 ? 1 : 0)} KB`;
	return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
