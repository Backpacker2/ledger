import { error } from '@sveltejs/kit';
import { api } from '$lib/api';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async ({ params }) => {
	try {
		return { thread: await api.getThread(params.slug) };
	} catch (e) {
		error(404, e instanceof Error ? e.message : 'Thread not found');
	}
};
