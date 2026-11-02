// Mirrors src-tauri/src/models.rs (camelCase on the wire).

export interface LinkedThread {
	slug: string;
	title: string;
}

export interface ThreadSummary {
	slug: string;
	title: string;
	createdAt: string;
	linkedThreadSlugs: string[];
	postCount: number;
	linkedCount: number;
	lastActivity: string;
	documentCount: number;
	storyCount: number;
}

export interface Post {
	id: string;
	parentId: string | null;
	guidingQuestion: boolean;
	versionPublished: boolean;
	documentId: string | null;
	documentVersion: string | null;
	createdAt: string;
	updatedAt: string;
	body: string;
	html: string;
}

export interface ThreadView {
	slug: string;
	title: string;
	createdAt: string;
	linkedThreads: LinkedThread[];
	posts: Post[];
	documents: DocumentSummary[];
	stories: Story[];
}

export interface DocumentType {
	slug: string;
	name: string;
	/** The "use when…" hint. */
	trigger: string;
	questions: string[];
	createdAt: string;
	updatedAt: string;
	documentCount: number;
}

export interface DocumentSummary {
	id: string;
	documentTypeSlug: string;
	documentTypeName: string;
	title: string;
	threadSlug: string | null;
	threadTitle: string | null;
	version: string | null;
	createdAt: string;
	updatedAt: string;
}

export interface DocumentView extends DocumentSummary {
	body: string;
	html: string;
	/** Published versions, oldest first. */
	versions: string[];
	/** A read-only published version rather than the live draft. */
	snapshot: boolean;
	/** A snapshot whose draft has since been deleted. */
	deleted: boolean;
}

export interface Criterion {
	text: string;
	done: boolean;
}

export interface Story {
	id: string;
	summary: string;
	role: string;
	want: string;
	reason: string;
	criteria: Criterion[];
	columnId: string;
	threads: LinkedThread[];
	createdAt: string;
	updatedAt: string;
}

export interface Column {
	id: string;
	name: string;
}

export interface Planboard {
	columns: Column[];
	backlogColumnId: string;
	userStories: Story[];
}

export interface SearchHit {
	kind: 'post' | 'document';
	id: string;
	title: string;
	threadSlug: string | null;
	threadTitle: string | null;
	createdAt: string;
	snippet: string;
}

export interface SyncStatus {
	configured: boolean;
	repo: string | null;
	branch: string | null;
	state: 'disabled' | 'idle' | 'syncing' | 'error';
	/** Changes made here that have not been pushed yet. */
	pending: boolean;
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

export interface Settings {
	vaultPath: string;
	/** Fixed by the LEDGER_VAULT environment variable. */
	vaultLocked: boolean;
	envFile: string;
	version: string;
	sync: SyncSettings;
	status: SyncStatus;
}

export interface StoryDraft {
	id: string;
	summary: string;
	role: string;
	want: string;
	reason: string;
	criteria: Criterion[];
	threadSlugs: string[];
}

export interface StoryUpdate {
	summary?: string;
	role?: string;
	want?: string;
	reason?: string;
	criteria?: Criterion[];
	columnId?: string;
}

export type Bump = 'minor' | 'major';
