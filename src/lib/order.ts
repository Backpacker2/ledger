// The order lists are shown in. Display only: nothing here touches how things are stored, or what "latest" means
// anywhere else — it only decides which way a list is read. Kept free of Svelte and Tauri imports so it can be
// exercised on its own.

/** Ascending reads oldest first, descending newest first. */
export type Order = 'asc' | 'desc';

const cmp = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

/**
 * A sorted copy of `items`. `ascending` says which of two comes first when reading oldest first; descending is
 * its exact mirror, so flipping the order flips the whole list, ties included. Items that tie on everything keep
 * the order they came in (and mirror it), so the result never depends on the sort's mood.
 */
function arrange<T>(items: readonly T[], order: Order, ascending: (a: T, b: T) => number): T[] {
	const entries = items.map((item, index) => ({ item, index }));
	const forward = (a: (typeof entries)[number], b: (typeof entries)[number]) => ascending(a.item, b.item) || a.index - b.index;
	entries.sort(order === 'asc' ? forward : (a, b) => forward(b, a));
	return entries.map((entry) => entry.item);
}

/**
 * Feedback by the moment it was written. Timestamps are compared as the ISO strings the backend stores and sorts
 * by; entries from the same moment fall back to their ids, which sort by creation.
 */
export function orderFeedback<T extends { id: string; createdAt: string }>(items: readonly T[], order: Order): T[] {
	return arrange(items, order, (a, b) => cmp(a.createdAt, b.createdAt) || cmp(a.id, b.id));
}

/**
 * Threads by their latest activity. Newest first is how the backend has always listed them: newest activity
 * first, then slug A–Z. Ties are ordered by slug the same way, so that default comes out exactly as it arrives.
 */
export function orderThreads<T extends { slug: string; lastActivity: string }>(items: readonly T[], order: Order): T[] {
	return arrange(items, order, (a, b) => cmp(a.lastActivity, b.lastActivity) || cmp(b.slug, a.slug));
}

/**
 * What a thread's page is made of — notes, prompts, feedback and uploads — by when each happened. They are ordered
 * together, never one kind apart from the rest, so the thread still reads as one story either way. Ties keep the
 * order they were given in (and mirror it).
 */
export function orderFlow<T extends { at: string }>(items: readonly T[], order: Order): T[] {
	return arrange(items, order, (a, b) => cmp(a.at, b.at));
}

/** "1.10" comes after "1.9": the parts are numbers, as the backend sorts published versions. */
function compareVersions(a: string, b: string): number {
	const [aMajor, aMinor] = a.split('.').map(Number);
	const [bMajor, bMinor] = b.split('.').map(Number);
	return aMajor - bMajor || aMinor - bMinor || cmp(a, b);
}

/** Published versions, newest first, for showing next to "Live". The list itself stays oldest first. */
export const versionsNewestFirst = (versions: readonly string[]): string[] => [...versions].sort((a, b) => compareVersions(b, a));
