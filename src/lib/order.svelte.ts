// How posts, feedback and the explorer's threads are ordered, as chosen in Settings. A display preference like the
// theme, so it is kept the same way: in the browser's storage, not in the vault. Read once when this module loads
// (the app has no server rendering), so the first paint is already in the chosen order.

import type { Order } from './order';

const POSTS_KEY = 'ledger-post-order';
const FEEDBACK_KEY = 'ledger-feedback-order';
// Kept under its original name: this is still the list of threads in the sidebar and on a project's page.
const EXPLORER_KEY = 'ledger-thread-order';

function load(key: string, fallback: Order): Order {
	try {
		const saved = localStorage.getItem(key);
		if (saved === 'asc' || saved === 'desc') return saved;
	} catch {
		// Storage can be unavailable; the default is fine.
	}
	return fallback;
}

function remember(key: string, order: Order) {
	try {
		localStorage.setItem(key, order);
	} catch {
		// Not persisted, still applied for this session.
	}
}

/**
 * `posts` is what a thread's page is made of, read inside the thread; `feedback` is the list on a deliverable's
 * page; `explorer` is the list of threads in the sidebar and on a project's page.
 *
 * Posts start oldest first, as a thread has always read. Feedback starts newest first. The explorer starts newest
 * activity first, which is the order the backend has always listed threads in.
 */
export const listOrder = $state<{ posts: Order; feedback: Order; explorer: Order }>({
	posts: load(POSTS_KEY, 'asc'),
	feedback: load(FEEDBACK_KEY, 'desc'),
	explorer: load(EXPLORER_KEY, 'desc')
});

export function setPostOrder(order: Order) {
	listOrder.posts = order;
	remember(POSTS_KEY, order);
}

export function setFeedbackOrder(order: Order) {
	listOrder.feedback = order;
	remember(FEEDBACK_KEY, order);
}

export function setExplorerOrder(order: Order) {
	listOrder.explorer = order;
	remember(EXPLORER_KEY, order);
}
