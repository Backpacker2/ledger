// Mirrors src-tauri/src/models.rs (camelCase on the wire).

export interface LinkedThread {
	slug: string;
	title: string;
}

export interface ThreadSummary {
	projectSlug: string;
	slug: string;
	title: string;
	createdAt: string;
	linkedThreadSlugs: string[];
	linkedCount: number;
	postCount: number;
	promptCount: number;
	feedbackCount: number;
	uploadCount: number;
	storyCount: number;
	lastActivity: string;
}

/** A project with its threads, as the sidebar shows it. */
export interface ProjectSummary {
	slug: string;
	name: string;
	createdAt: string;
	updatedAt: string;
	threads: ThreadSummary[];
	deliverableCount: number;
	storyCount: number;
	taskCount: number;
}

export interface ProjectView {
	slug: string;
	name: string;
	description: string;
	/** The reading guide (README.md), raw and rendered. */
	readme: string;
	readmeHtml: string;
	threadCount: number;
	deliverableCount: number;
	storyCount: number;
	taskCount: number;
	openTaskCount: number;
}

export interface Post {
	id: string;
	parentId: string | null;
	guidingQuestion: boolean;
	versionPublished: boolean;
	deliverablePath: string | null;
	deliverableVersion: string | null;
	createdAt: string;
	updatedAt: string;
	body: string;
	html: string;
}

export interface PromptDeliverable {
	path: string;
	exists: boolean;
}

export interface Prompt {
	id: string;
	threadSlug: string;
	title: string;
	templateId: string | null;
	templateName: string | null;
	body: string;
	html: string;
	createdAt: string;
	updatedAt: string;
	deliverables: PromptDeliverable[];
	/** The times it was run with Claude Code, newest first. */
	runs: ClaudeRunRecord[];
}

/** What the New prompt dialog starts from when it is opened for something other than a blank prompt. */
export interface PromptDraft {
	title: string;
	body: string;
	/** An existing deliverable the prompt asks for changes to, by its live path; null leaves the choice at "a new deliverable". */
	deliverable: string | null;
	/** Said at the top of the dialog: where the draft came from. */
	note: string;
}

export interface Feedback {
	id: string;
	threadSlug: string;
	deliverablePath: string;
	deliverableVersion: string | null;
	status: 'open' | 'resolved';
	body: string;
	html: string;
	createdAt: string;
	updatedAt: string;
}

export interface Upload {
	id: string;
	threadSlug: string;
	/** Relative to the project folder, e.g. "attachments/diagram.png". */
	file: string;
	name: string;
	size: number;
	uploadedAt: string;
	description: string;
	descriptionHtml: string;
	image: boolean;
	/** Whether the file is still where the manifest says. */
	exists: boolean;
}

export interface ThreadView {
	projectSlug: string;
	slug: string;
	title: string;
	description: string;
	createdAt: string;
	linkedThreads: LinkedThread[];
	posts: Post[];
	prompts: Prompt[];
	feedback: Feedback[];
	uploads: Upload[];
	stories: Story[];
}

// ---- deliverables -----------------------------------------------------------------------------------

export type DeliverableKind = 'markdown' | 'image' | 'other';

export interface DeliverableSummary {
	/** The live file, relative to the deliverables folder. */
	path: string;
	name: string;
	kind: DeliverableKind;
	size: number;
	modified: string;
	/** Whether the live file exists (published versions can outlive it). */
	exists: boolean;
	/** Published versions, oldest first. */
	versions: string[];
	promptCount: number;
	openFeedback: number;
}

export interface DeliverablePrompt {
	threadSlug: string;
	threadTitle: string;
	id: string;
	title: string;
}

export interface Deliverable {
	path: string;
	basePath: string;
	name: string;
	kind: DeliverableKind;
	/** Set when `path` is a published version, which is read-only. */
	version: string | null;
	versions: string[];
	exists: boolean;
	size: number;
	modified: string;
	body: string | null;
	html: string | null;
	editable: boolean;
	prompts: DeliverablePrompt[];
	feedback: Feedback[];
}

// ---- templates --------------------------------------------------------------------------------------

export interface Template {
	/** "" for a vault-wide template, else the project's slug. */
	scope: string;
	id: string;
	name: string;
	/** The "use when…" hint. */
	trigger: string;
	questions: string[];
	/** The skeleton of the deliverable. */
	body: string;
	createdAt: string;
	updatedAt: string;
	/** A vault-wide template the project has replaced with its own. */
	overridden: boolean;
}

// ---- stories, tasks, the board ----------------------------------------------------------------------

export interface Criterion {
	text: string;
	done: boolean;
}

export interface Story {
	projectSlug: string;
	id: string;
	summary: string;
	role: string;
	want: string;
	reason: string;
	notes: string;
	criteria: Criterion[];
	threads: LinkedThread[];
	taskCount: number;
	createdAt: string;
	updatedAt: string;
}

export interface Column {
	id: string;
	name: string;
}

export interface Task {
	id: string;
	title: string;
	body: string;
	columnId: string;
	storyId: string | null;
	storySummary: string | null;
	threadSlug: string | null;
	threadTitle: string | null;
	createdAt: string;
	updatedAt: string;
}

export interface Board {
	columns: Column[];
	backlogColumnId: string;
	tasks: Task[];
	stories: { id: string; summary: string }[];
}

// ---- search -----------------------------------------------------------------------------------------

export interface SearchHit {
	kind: 'post' | 'prompt' | 'feedback' | 'deliverable';
	/** The post, prompt or feedback id; the path for a deliverable. */
	id: string;
	projectSlug: string;
	projectName: string;
	title: string;
	threadSlug: string | null;
	createdAt: string;
	snippet: string;
}

// ---- settings, sync, screenshots --------------------------------------------------------------------

export interface SyncStatus {
	configured: boolean;
	repo: string | null;
	branch: string | null;
	state: 'disabled' | 'idle' | 'syncing' | 'error';
	/** Changes made here that have not been pushed yet. */
	pending: boolean;
	/** Whether syncs encrypt what they upload (the setting as it stood when the last sync started). */
	encrypted: boolean;
	lastSyncAt: string | null;
	lastError: string | null;
	summary: string | null;
}

export interface SyncSettings {
	repo: string;
	branch: string;
	/** The token itself never reaches the screen. */
	hasToken: boolean;
	tokenHint: string;
	configured: boolean;
}

export interface EncryptionStatus {
	/** Whether what is synced is encrypted. */
	enabled: boolean;
	/** The key lives in the vault's .ledger/.env; the screen only learns whether it is there. */
	hasKey: boolean;
	/** The key's one-byte label as two hex digits, to tell keys apart between machines. */
	keyId: string | null;
	/** Why the key can't be read, if it can't (a damaged line in .ledger/.env, an unreadable file). */
	problem: string | null;
	/** This vault already synced the configured repository and branch without encryption: that branch can't become an encrypted one. */
	needsFreshTarget: boolean;
}

export interface ScreenshotSettings {
	hotkey: string;
	defaultHotkey: string;
	/** Why the saved hotkey isn't working, if it isn't. */
	problem: string | null;
	/** Something about this platform worth knowing (a missing permission, Wayland). */
	note: string | null;
}

export interface Settings {
	vaultPath: string;
	/** Fixed by the LEDGER_VAULT environment variable. */
	vaultLocked: boolean;
	settingsFile: string;
	secretsFile: string;
	version: string;
	sync: SyncSettings;
	status: SyncStatus;
	screenshot: ScreenshotSettings;
	encryption: EncryptionStatus;
}

/** A dragged rectangle, in the capture's own pixels. */
export interface Region {
	x: number;
	y: number;
	width: number;
	height: number;
}

/** The payload of the `screenshot-ready` event: a screenshot is waiting to be attached to a thread. */
export interface ScreenshotTaken {
	width: number;
	height: number;
}

export interface StoryDraft {
	id: string;
	summary: string;
	role: string;
	want: string;
	reason: string;
	notes: string;
	criteria: Criterion[];
	threadSlugs: string[];
}

export interface StoryUpdate {
	summary?: string;
	role?: string;
	want?: string;
	reason?: string;
	notes?: string;
	criteria?: Criterion[];
}

/** For `storyId` and `threadSlug`, an empty string clears the link. */
export interface TaskUpdate {
	title?: string;
	body?: string;
	columnId?: string;
	storyId?: string;
	threadSlug?: string;
}

export type Bump = 'minor' | 'major';

// ---- Run a prompt with Claude Code (a block of its own; see src-tauri/src/claude_code) ---------------

/** What Claude Code may do on its own. There is deliberately no mode that skips the permission checks. */
export type ClaudePermissionMode = 'acceptEdits' | 'plan' | 'dontAsk';

export interface ClaudeCodeSettings {
	/** Empty means "find claude on the PATH". */
	cliPath: string;
	permissionMode: ClaudePermissionMode;
	/** Empty means the CLI's own default. */
	model: string;
	/** Extra guard rails: ignore the project's own Claude settings, and refuse reads and writes outside the project. */
	confine: boolean;
	/** The program that would be run, when it can be found. */
	resolvedPath: string | null;
	/** Why it can't be found. */
	problem: string | null;
	settingsFile: string;
	maxConcurrent: number;
}

export interface ClaudeCodeCheck {
	ok: boolean;
	version: string;
	path: string | null;
	message: string;
}

export interface ClaudeRunPreview {
	/** Where it runs. */
	folder: string;
	/** For reading only; it is never run through a shell. */
	command: string;
	/** The text sent on standard input, exactly. */
	prompt: string;
	/** The settings file given with --settings, when the guard rails are on. */
	guard: string | null;
	permissionMode: ClaudePermissionMode;
	model: string;
	/** Set when it can't be run. */
	problem: string | null;
}

export type ClaudeRunStatus = 'running' | 'finished' | 'failed' | 'cancelled';

export interface ClaudeRun {
	id: string;
	project: string;
	thread: string;
	promptId: string;
	promptTitle: string;
	status: ClaudeRunStatus;
	startedAt: string;
	finishedAt: string | null;
	exitCode: number | null;
	/** Why it failed or stopped. */
	message: string | null;
	/** Cancel was pressed and the process is being stopped. */
	cancelling: boolean;
	/** How many pieces of output there have been so far. */
	chunks: number;
	/** The versions the app published for what the run wrote and linked to the prompt, like "plan@v0.2.md". */
	linked: string[];
	vaultReloaded: boolean;
}

/** The note kept in the thread (`runs/<id>.md`) about a run of a prompt that is over. */
export interface ClaudeRunRecord {
	/** The id of the run, as it was while it ran. */
	id: string;
	promptId: string;
	status: Exclude<ClaudeRunStatus, 'running'>;
	model: string;
	permissionMode: string;
	startedAt: string;
	finishedAt: string;
	seconds: number;
	turns: number | null;
	/** Deliverables it wrote or changed, relative to the project's deliverables folder. */
	files: string[];
	/** How it ended and what it did, rendered. */
	html: string;
}

export interface ClaudeRunSnapshot {
	run: ClaudeRun;
	output: string;
}

/** The payload of the `claude-run` event: the run as it now is, and the output just added, if any. */
export interface ClaudeRunEvent {
	run: ClaudeRun;
	chunk: { seq: number; text: string } | null;
}

// ---- Trello sync (feat/trello-sync) -----------------------------------------------------------------

export interface TrelloMember {
	id: string;
	username: string;
	fullName: string;
}

export interface TrelloBoard {
	id: string;
	name: string;
}

export interface TrelloList {
	id: string;
	name: string;
}

/** What the last Trello round did, and what it wants a person to know. */
export interface TrelloReport {
	cardsCreated: number;
	cardsUpdated: number;
	itemsCreated: number;
	itemsUpdated: number;
	converted: number;
	detached: number;
	conflicts: number;
	/** Tasks without a list and cards without a column: left out. */
	unmatched: number;
	skipped: number;
	/** Items that are not syncing because their card is gone or no longer the person's. */
	unlinked: number;
	vaultWrites: number;
	notes: string[];
}

export interface TrelloStatus {
	/** There are a key and token, a person and at least one linked project. */
	configured: boolean;
	state: 'disabled' | 'idle' | 'syncing' | 'error';
	/** Changes made here that the sync hasn't looked at yet. */
	pending: boolean;
	lastSyncAt: string | null;
	lastError: string | null;
	summary: string | null;
	report: TrelloReport | null;
}

export interface TrelloProject {
	slug: string;
	name: string;
	/** The Trello board the project is linked to; a project with none isn't synced. */
	board: string | null;
	boardName: string | null;
}

/** An item that stopped syncing because its card was archived, deleted, moved or is no longer the person's. */
export interface TrelloUnlinkedItem {
	project: string;
	projectName: string;
	kind: 'task' | 'story';
	id: string;
	title: string;
	reason: string;
}

export interface TrelloView {
	/** The key and token themselves never reach the screen. */
	hasKey: boolean;
	keyHint: string;
	hasToken: boolean;
	tokenHint: string;
	/** The one person whose cards are synced. */
	member: TrelloMember | null;
	configured: boolean;
	projects: TrelloProject[];
	unlinked: TrelloUnlinkedItem[];
	status: TrelloStatus;
	settingsFile: string;
	secretsFile: string;
}

export interface TrelloColumnChoice {
	id: string;
	name: string;
	/** "auto" (the list with the same name), "none" (not synced) or the id of a list. */
	choice: string;
	listId: string | null;
	listName: string | null;
}

export interface TrelloMapping {
	lists: TrelloList[];
	columns: TrelloColumnChoice[];
	/** Lists no column uses: their cards aren't added. */
	unusedLists: string[];
}
