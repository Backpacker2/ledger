// How lists are ordered, as chosen in Settings: one choice for all of them. A display preference like the theme, so it
// is kept the same way: in the browser's storage, not in the vault. Read once when this module loads (the app has no
// server rendering), so the first paint is already in the chosen order.

import type { Order } from './order';

const KEY = 'ledger-order';

function load(): Order {
	try {
		const saved = localStorage.getItem(KEY);
		if (saved === 'asc' || saved === 'desc') return saved;
	} catch {
		// Storage can be unavailable; the default is fine.
	}
	return 'desc';
}

/**
 * Newest first by default. It decides the posts of a thread (notes, prompts, feedback and uploads together), the
 * threads in the sidebar and on a project's page, the feedback and the versions of a deliverable, and the
 * "Produced by" window beside it.
 */
export const listOrder = $state<{ order: Order }>({ order: load() });

export function setOrder(order: Order) {
	listOrder.order = order;
	try {
		localStorage.setItem(KEY, order);
	} catch {
		// Not persisted, still applied for this session.
	}
}
