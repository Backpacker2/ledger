import { redirect } from '@sveltejs/kit';

// Settings has no page of its own: it opens on its first group.
export const load = () => redirect(307, '/settings/features');
