import { DesktopOnly, desktop, fileUrl, invoke, type Picked } from './platform';
import { capture, captureDebug, logError } from './log.svelte';
import type {
	ActivityEvent,
	Board,
	Bump,
	Column,
	Deliverable,
	DeliverableSummary,
	EncryptionStatus,
	Feedback,
	Post,
	ProjectSummary,
	ProjectView,
	Prompt,
	Provenance,
	PublishChoices,
	Region,
	ScreenshotTaken,
	SearchHit,
	Settings,
	Story,
	StoryDraft,
	StoryUpdate,
	SyncStatus,
	Task,
	TaskUpdate,
	Template,
	ThreadSummary,
	ThreadView,
	Upload,
	VersionUpdate
} from './types';

/** Commands reject with a plain message; surface it as an `Error` so callers can `catch (e)` uniformly. */
/** Runs a backend command; a failure is written to the app's log (unless `log` is false) and thrown. */
async function call<T>(command: string, args?: Record<string, unknown>, log = true): Promise<T> {
	const started = performance.now();
	try {
		const result = await invoke<T>(command, args);
		// A debug capture records each action by name and time only: the arguments hold what you wrote.
		if (log && capture.active) captureDebug(command, `ok in ${Math.round(performance.now() - started)} ms`);
		return result;
	} catch (e) {
		// Asking for a desktop-only thing from the phone isn't an error worth logging.
		if (e instanceof DesktopOnly) throw e;
		const message = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e);
		if (log) logError(command, `${message}${capture.active ? ` (after ${Math.round(performance.now() - started)} ms)` : ''}`);
		throw new Error(message);
	}
}

async function fetchProjectFile(project: string, path: string): Promise<ArrayBuffer> {
	const response = await fetch(fileUrl(project, path));
	if (!response.ok) {
		const message = await response.json().catch(() => null);
		throw new Error(typeof message === 'string' ? message : `Couldn't load ${path} (${response.status})`);
	}
	return response.arrayBuffer();
}

async function uploadOverHttp(project: string, thread: string, files: File[], description: string): Promise<Upload[]> {
	const form = new FormData();
	form.append('project', project);
	form.append('thread', thread);
	form.append('description', description);
	for (const file of files) form.append('file', file, file.name);
	const response = await fetch('/api/upload', { method: 'POST', body: form });
	const body = await response.json().catch(() => null);
	if (!response.ok) {
		const message = typeof body === 'string' ? body : `The upload failed (${response.status})`;
		logError('upload', message);
		throw new Error(message);
	}
	return body as Upload[];
}

export const api = {
	renderMarkdown: (body: string) => call<string>('render_markdown', { body }),
	search: (query: string) => call<SearchHit[]>('search', { query }),
	/** Everything made in the vault and when, oldest first: the contribution graph's events. */
	listActivity: () => call<ActivityEvent[]>('list_activity'),

	// projects
	listProjects: () => call<ProjectSummary[]>('list_projects'),
	createProject: (name: string) => call<ProjectSummary>('create_project', { name }),
	renameProject: (slug: string, name: string) => call<ProjectSummary>('rename_project', { slug, name }),
	getProject: (slug: string) => call<ProjectView>('get_project', { slug }),
	saveReadme: (slug: string, text: string) => call<ProjectView>('save_readme', { slug, text }),
	deleteProject: (slug: string) => call<void>('delete_project', { slug }),

	// threads
	createThread: (project: string, title: string) => call<ThreadSummary>('create_thread', { project, title }),
	renameThread: (project: string, slug: string, title: string) => call<ThreadSummary>('rename_thread', { project, slug, title }),
	getThread: (project: string, slug: string) => call<ThreadView>('get_thread', { project, slug }),
	deleteThread: (project: string, slug: string) => call<void>('delete_thread', { project, slug }),
	linkThreads: (project: string, slug: string, otherSlug: string) => call<void>('link_threads', { project, slug, otherSlug }),
	unlinkThreads: (project: string, slug: string, otherSlug: string) => call<void>('unlink_threads', { project, slug, otherSlug }),

	// posts
	addPost: (project: string, thread: string, body: string, replyToId: string | null = null) =>
		call<Post>('add_post', { project, thread, body, replyToId }),
	updatePost: (project: string, thread: string, id: string, body: string) => call<Post>('update_post', { project, thread, id, body }),
	deletePost: (project: string, thread: string, id: string) => call<void>('delete_post', { project, thread, id }),

	// prompts
	/** `deliverable` is an existing deliverable the prompt asks for changes to; null asks for a new one. */
	createPrompt: (project: string, thread: string, title: string, body: string, templateId: string | null, deliverable: string | null = null) =>
		call<Prompt>('create_prompt', { project, thread, title, body, templateId, deliverable }),
	updatePrompt: (project: string, thread: string, id: string, title: string, body: string) =>
		call<Prompt>('update_prompt', { project, thread, id, title, body }),
	deletePrompt: (project: string, thread: string, id: string) => call<void>('delete_prompt', { project, thread, id }),
	linkPromptDeliverable: (project: string, thread: string, id: string, path: string) =>
		call<Prompt>('link_prompt_deliverable', { project, thread, id, path }),
	unlinkPromptDeliverable: (project: string, thread: string, id: string, path: string) =>
		call<Prompt>('unlink_prompt_deliverable', { project, thread, id, path }),
	createPromptDeliverable: (project: string, thread: string, id: string, name: string) =>
		call<Prompt>('create_prompt_deliverable', { project, thread, id, name }),

	// feedback
	addFeedback: (project: string, thread: string, deliverablePath: string, body: string) =>
		call<Feedback>('add_feedback', { project, thread, deliverablePath, body }),
	setFeedbackStatus: (project: string, thread: string, id: string, status: 'open' | 'resolved') =>
		call<Feedback>('set_feedback_status', { project, thread, id, status }),
	updateFeedback: (project: string, thread: string, id: string, body: string) =>
		call<Feedback>('update_feedback', { project, thread, id, body }),
	deleteFeedback: (project: string, thread: string, id: string) => call<void>('delete_feedback', { project, thread, id }),

	// uploads and project files
	/** Paths on this machine in the desktop app; files the browser picked on the phone (`POST /api/upload`). */
	uploadFiles: (project: string, thread: string, files: Picked[], description: string) =>
		desktop
			? call<Upload[]>('upload_files', { project, thread, paths: files.filter((f) => typeof f === 'string'), description })
			: uploadOverHttp(project, thread, files.filter((f) => f instanceof File), description),
	updateUpload: (project: string, thread: string, id: string, description: string) =>
		call<Upload>('update_upload', { project, thread, id, description }),
	deleteUpload: (project: string, thread: string, id: string) => call<void>('delete_upload', { project, thread, id }),
	/** A file in the project's folder ("attachments/…", "deliverables/…"), as raw bytes. */
	readProjectFile: (project: string, path: string) =>
		desktop ? call<ArrayBuffer>('read_project_file', { project, path }) : fetchProjectFile(project, path),
	/** With the system's program (desktop), or in a new tab (phone; `reveal` isn't possible there). */
	openProjectFile: (project: string, path: string, reveal = false) => {
		if (desktop) return call<void>('open_project_file', { project, path, reveal });
		if (reveal) return Promise.reject(new DesktopOnly('Showing a file in its folder'));
		window.open(fileUrl(project, path), '_blank', 'noopener');
		return Promise.resolve();
	},

	// deliverables
	listDeliverables: (project: string) => call<DeliverableSummary[]>('list_deliverables', { project }),
	getDeliverable: (project: string, path: string) => call<Deliverable>('get_deliverable', { project, path }),
	createDeliverable: (project: string, name: string, body = '') => call<Deliverable>('create_deliverable', { project, name, body }),
	saveDeliverable: (project: string, path: string, body: string) => call<Deliverable>('save_deliverable', { project, path, body }),
	publishDeliverable: (project: string, path: string, bump: Bump, version: string | null = null, choices: PublishChoices | null = null) =>
		call<Deliverable>('publish_deliverable', { project, path, bump, version, choices }),
	/** How the versions of a deliverable came to be, by thread. `path` is the live file or any of its versions. */
	getProvenance: (project: string, path: string) => call<Provenance>('get_provenance', { project, path }),
	/** Says who made a version, or sets its AI-use label or change note. */
	updateVersionRecord: (project: string, path: string, version: string, update: VersionUpdate) =>
		call<Provenance>('update_version_record', { project, path, version, update }),
	deleteDeliverable: (project: string, path: string) => call<void>('delete_deliverable', { project, path }),

	// templates
	listTemplates: (scope: string) => call<Template[]>('list_templates', { scope }),
	projectTemplates: (project: string) => call<Template[]>('project_templates', { project }),
	effectiveTemplates: (project: string) => call<Template[]>('effective_templates', { project }),
	createTemplate: (scope: string, name: string, trigger: string, questions: string[], body: string) =>
		call<Template>('create_template', { scope, name, trigger, questions, body }),
	/** Copies a vault-wide template into the project under the same id, so the project can change its own. */
	overrideTemplate: (project: string, id: string) => call<Template>('override_template', { project, id }),
	updateTemplate: (scope: string, id: string, name: string, trigger: string, questions: string[], body: string) =>
		call<Template>('update_template', { scope, id, name, trigger, questions, body }),
	deleteTemplate: (scope: string, id: string) => call<void>('delete_template', { scope, id }),

	// user stories
	listStories: (project: string) => call<Story[]>('list_stories', { project }),
	createStory: (project: string, draft: StoryDraft) => call<Story>('create_story', { project, draft }),
	updateStory: (project: string, id: string, updates: StoryUpdate) => call<Story>('update_story', { project, id, updates }),
	deleteStory: (project: string, id: string) => call<void>('delete_story', { project, id }),
	linkStoryThread: (project: string, id: string, threadSlug: string) => call<Story>('link_story_thread', { project, id, threadSlug }),
	unlinkStoryThread: (project: string, id: string, threadSlug: string) => call<Story>('unlink_story_thread', { project, id, threadSlug }),

	// the board
	getBoard: (project: string) => call<Board>('get_board', { project }),
	createTask: (project: string, title: string, body: string, storyId: string | null, threadSlug: string | null) =>
		call<Task>('create_task', { project, title, body, storyId, threadSlug }),
	updateTask: (project: string, id: string, updates: TaskUpdate) => call<Task>('update_task', { project, id, updates }),
	deleteTask: (project: string, id: string) => call<void>('delete_task', { project, id }),
	addColumn: (project: string, name: string) => call<Column[]>('add_column', { project, name }),
	renameColumn: (project: string, id: string, name: string) => call<Column[]>('rename_column', { project, id, name }),
	deleteColumn: (project: string, id: string) => call<Column[]>('delete_column', { project, id }),

	// the vault and GitHub sync
	getSettings: () => call<Settings>('get_settings'),
	syncStatus: () => call<SyncStatus>('sync_status'),
	/** A blank token keeps the saved one. */
	saveSyncSettings: (repo: string, branch: string, token: string | null) =>
		call<Settings>('save_sync_settings', { repo, branch, token }),
	disconnectSync: () => call<Settings>('disconnect_sync'),
	testSync: (repo: string, branch: string, token: string | null) => call<string>('test_sync', { repo, branch, token }),
	syncNow: () => call<void>('sync_now'),
	setVaultPath: (path: string, copy: boolean) => call<Settings>('set_vault_path', { path, copy }),
	reloadVault: () => call<void>('reload_vault'),
	checkVaultChanges: () => call<boolean>('check_vault_changes'),

	// encrypting what is synced (the key sits in the vault's .ledger/.env, which is never synced)
	encryptionStatus: () => call<EncryptionStatus>('encryption_status'),
	/** Turning it on needs a key. It also starts a sync, which re-uploads every file encrypted. */
	setEncryptionEnabled: (enabled: boolean) => call<Settings>('set_encryption_enabled', { enabled }),
	/** With a key already on this machine, `replace` must be true. */
	generateEncryptionKey: (replace: boolean) => call<Settings>('generate_encryption_key', { replace }),
	/** A recovery string, or the bare key as hex or base64. With a key already on this machine, `replace` must be true. */
	importEncryptionKey: (key: string, replace: boolean) => call<Settings>('import_encryption_key', { key, replace }),
	/** The one call that returns the key (as its recovery string). Only on an explicit request. */
	exportRecoveryKey: () => call<string>('export_recovery_key'),

	// screenshots
	saveScreenshotHotkey: (hotkey: string) => call<Settings>('save_screenshot_hotkey', { hotkey }),
	saveScreenshotTool: (useCommand: boolean, command: string) => call<Settings>('save_screenshot_tool', { useCommand, command }),
	pendingScreenshot: () => call<ScreenshotTaken | null>('pending_screenshot'),
	takeScreenshot: () => call<void>('take_screenshot'),
	/** Lets go of the hotkey while it is being re-recorded, and takes it back afterwards. */
	pauseScreenshotHotkey: (paused: boolean) => call<void>('pause_screenshot_hotkey', { paused }),
	screenshotImage: () => call<ArrayBuffer>('screenshot_image'),
	screenshotOverlayReady: () => call<void>('screenshot_overlay_ready'),
	screenshotFinish: (region: Region) => call<void>('screenshot_finish', { region }),
	screenshotCancel: () => call<void>('screenshot_cancel'),
	pendingScreenshotImage: () => call<ArrayBuffer>('pending_screenshot_image'),
	attachScreenshot: (project: string, thread: string, description: string) =>
		call<Upload>('attach_screenshot', { project, thread, description }),
	discardScreenshot: () => call<void>('discard_screenshot')
};

// ---- Run a prompt with Claude Code (a block of its own; see src-tauri/src/claude_code) ---------------

import type {
	ClaudeCodeCheck,
	ClaudeCodeSettings,
	ClaudeRun,
	ClaudeRunPreview,
	ClaudeRunSnapshot
} from './types';

export const claudeCodeApi = {
	settings: () => call<ClaudeCodeSettings>('claude_code_settings'),
	saveSettings: (cliPath: string, permissionMode: string, model: string, confine: boolean, autoRun: boolean) =>
		call<ClaudeCodeSettings>('save_claude_code_settings', { cliPath, permissionMode, model, confine, autoRun }),
	/** Runs `claude --version` with the path as typed (blank: look on the PATH). Doesn't save anything. */
	check: (cliPath: string) => call<ClaudeCodeCheck>('check_claude_code', { cliPath }),
	/** Everything a run would send and where, for the confirmation. */
	preview: (project: string, thread: string, id: string) => call<ClaudeRunPreview>('claude_code_preview', { project, thread, id }),
	/** `confirmed` is the prompt text from the preview: if the prompt changed since, nothing starts. */
	start: (project: string, thread: string, id: string, confirmed: string) =>
		call<ClaudeRun>('claude_code_start', { project, thread, id, confirmed }),
	cancel: (runId: string) => call<ClaudeRun>('claude_code_cancel', { runId }),
	runs: () => call<ClaudeRunSnapshot[]>('claude_code_runs'),
	run: (runId: string) => call<ClaudeRunSnapshot>('claude_code_run', { runId })
};

// ---- Trello sync (feat/trello-sync) -----------------------------------------------------------------

import type {
	TrelloBoard,
	TrelloMapping,
	TrelloMember,
	TrelloStatus,
	TrelloView
} from './types';

export const trelloApi = {
	trelloGet: () => call<TrelloView>('trello_get'),
	trelloStatus: () => call<TrelloStatus>('trello_status'),
	/** Tries the key and token on screen (a blank one means the saved one) without saving them. Resolves to a message naming the account. */
	trelloTest: (apiKey: string | null, token: string | null) => call<string>('trello_test', { apiKey, token }),
	/** A blank key or token keeps the saved one. */
	trelloSaveCredentials: (apiKey: string | null, token: string | null) =>
		call<TrelloView>('trello_save_credentials', { apiKey, token }),
	trelloDisconnect: () => call<TrelloView>('trello_disconnect'),
	trelloBoards: () => call<TrelloBoard[]>('trello_boards'),
	trelloBoardMembers: (board: string) => call<TrelloMember[]>('trello_board_members', { board }),
	/** Chooses the one person to sync for, from the members of `board`. */
	trelloSaveMember: (board: string, member: string) => call<TrelloView>('trello_save_member', { board, member }),
	/** Links a project to a board; `null` stops syncing the project. */
	trelloLinkProject: (project: string, board: string | null) => call<TrelloView>('trello_link_project', { project, board }),
	trelloProjectMapping: (project: string) => call<TrelloMapping>('trello_project_mapping', { project }),
	/** `choice` is "auto", "none" or a list id. */
	trelloSetColumnList: (project: string, column: string, choice: string) =>
		call<void>('trello_set_column_list', { project, column, choice }),
	trelloSyncNow: () => call<void>('trello_sync_now'),
	/** Gives an item that stopped syncing a new card on the next sync (the old card is left alone). */
	trelloRelinkItem: (project: string, kind: 'task' | 'story', id: string) =>
		call<TrelloView>('trello_relink_item', { project, kind, id })
};

// ---- Updating Ledger from inside the app (see src-tauri/src/updates.rs) -----------------------------

import type { InstallStep, RunsChoice, UpdateCheck, UpdateInfo, UpdateView } from './types';

export const updatesApi = {
	status: () => call<UpdateView>('update_status'),
	/** Whether to look for updates by itself; kept for this machine, not in the vault. */
	saveSettings: (automatic: boolean) => call<UpdateView>('save_update_settings', { automatic }),
	/** Looks now, whether or not automatic checks are on. */
	check: () => call<UpdateCheck>('check_for_updates'),
	/** Downloads (once), waits for syncs and runs, installs and restarts. Call again while the answer is to wait. */
	install: (choice: RunsChoice) => call<InstallStep>('install_update', { choice }),
	/** Development builds only: pretend a release was found. */
	simulate: (major: boolean) => call<UpdateInfo>('simulate_update', { major })
};

import type { AboutInfo, LogEntry } from './types';

export const logApi = {
	/** The last entries of the log, oldest first. */
	// Not logged: reading the log must not write to it, or a screen that follows the log could go round in circles.
	read: (limit?: number) => call<LogEntry[]>('log_read', { limit }, false),
	clear: () => call<void>('log_clear', undefined, false),
	/** Sets the most the log may take on disk; resolves to the limit as set. */
	setMaxBytes: (bytes: number) => call<number>('log_set_max_bytes', { bytes }),
	about: () => call<AboutInfo>('about_info', undefined, false)
};
