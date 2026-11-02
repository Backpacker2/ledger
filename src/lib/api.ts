import { invoke } from '@tauri-apps/api/core';
import type {
	Bump,
	Column,
	DocumentSummary,
	DocumentType,
	DocumentView,
	Planboard,
	Post,
	Region,
	SearchHit,
	Story,
	StoryDraft,
	StoryUpdate,
	Settings,
	SyncStatus,
	ThreadSummary,
	ThreadView
} from './types';

/** Commands reject with a plain message; surface it as an `Error` so callers can `catch (e)` uniformly. */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
	if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
		throw new Error('Ledger needs its desktop shell — start it with `npm run tauri dev`.');
	}
	try {
		return await invoke<T>(command, args);
	} catch (e) {
		throw new Error(typeof e === 'string' ? e : e instanceof Error ? e.message : String(e));
	}
}

export const api = {
	renderMarkdown: (body: string) => call<string>('render_markdown', { body }),
	search: (query: string) => call<SearchHit[]>('search', { query }),

	// threads
	listThreads: () => call<ThreadSummary[]>('list_threads'),
	createThread: (title: string) => call<ThreadSummary>('create_thread', { title }),
	renameThread: (slug: string, title: string) => call<ThreadSummary>('rename_thread', { slug, title }),
	getThread: (slug: string) => call<ThreadView>('get_thread', { slug }),
	deleteThread: (slug: string) => call<void>('delete_thread', { slug }),
	linkThreads: (slug: string, otherSlug: string) => call<void>('link_threads', { slug, otherSlug }),
	unlinkThreads: (slug: string, otherSlug: string) => call<void>('unlink_threads', { slug, otherSlug }),

	// posts
	addPost: (slug: string, body: string, replyToId: string | null = null) =>
		call<Post>('add_post', { slug, body, replyToId }),
	updatePost: (slug: string, id: string, body: string) => call<Post>('update_post', { slug, id, body }),
	deletePost: (slug: string, id: string) => call<void>('delete_post', { slug, id }),

	// document types
	listDocumentTypes: () => call<DocumentType[]>('list_document_types'),
	createDocumentType: (name: string, trigger: string, questions: string[]) =>
		call<DocumentType>('create_document_type', { name, trigger, questions }),
	updateDocumentType: (slug: string, name: string, trigger: string, questions: string[]) =>
		call<DocumentType>('update_document_type', { slug, name, trigger, questions }),
	deleteDocumentType: (slug: string) => call<void>('delete_document_type', { slug }),

	// documents
	listDocuments: () => call<DocumentSummary[]>('list_documents'),
	getDocument: (id: string) => call<DocumentView>('get_document', { id }),
	getDocumentVersion: (id: string, version: string) => call<DocumentView>('get_document_version', { id, version }),
	createDocument: (documentTypeSlug: string, title: string, threadSlug: string | null = null) =>
		call<DocumentView>('create_document', { documentTypeSlug, title, threadSlug }),
	updateDocument: (id: string, patch: { title?: string; body?: string }) =>
		call<DocumentView>('update_document', { id, title: patch.title ?? null, body: patch.body ?? null }),
	publishDocument: (id: string, bump: Bump, version: string | null = null) =>
		call<DocumentView>('publish_document', { id, bump, version }),
	startDrafting: (id: string, threadSlug: string | null, newThreadTitle: string | null) =>
		call<DocumentView>('start_drafting', { id, threadSlug, newThreadTitle }),
	deleteDocument: (id: string) => call<void>('delete_document', { id }),

	// planboard and user stories
	getPlanboard: () => call<Planboard>('get_planboard'),
	createUserStory: (draft: StoryDraft) => call<Story>('create_user_story', { draft }),
	updateUserStory: (id: string, updates: StoryUpdate) => call<Story>('update_user_story', { id, updates }),
	deleteUserStory: (id: string) => call<void>('delete_user_story', { id }),
	linkStoryThread: (id: string, threadSlug: string) => call<Story>('link_story_thread', { id, threadSlug }),
	unlinkStoryThread: (id: string, threadSlug: string) => call<Story>('unlink_story_thread', { id, threadSlug }),
	addColumn: (name: string) => call<Column[]>('add_column', { name }),
	renameColumn: (id: string, name: string) => call<Column[]>('rename_column', { id, name }),
	deleteColumn: (id: string) => call<Column[]>('delete_column', { id }),

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

	// screenshots
	saveScreenshotSettings: (hotkey: string, uploadUrl: string) =>
		call<Settings>('save_screenshot_settings', { hotkey, uploadUrl }),
	takeScreenshot: () => call<void>('take_screenshot'),
	/** Lets go of the hotkey while it is being re-recorded, and takes it back afterwards. */
	pauseScreenshotHotkey: (paused: boolean) => call<void>('pause_screenshot_hotkey', { paused }),
	screenshotImage: () => call<ArrayBuffer>('screenshot_image'),
	screenshotOverlayReady: () => call<void>('screenshot_overlay_ready'),
	screenshotFinish: (region: Region) => call<void>('screenshot_finish', { region }),
	screenshotCancel: () => call<void>('screenshot_cancel')
};
