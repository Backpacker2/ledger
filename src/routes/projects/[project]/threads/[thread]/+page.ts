import { error } from '@sveltejs/kit';
import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params }) => {
	try {
		return { thread: await api.getThread(params.project, params.thread) };
	} catch (e) {
		error(404, e instanceof Error ? e.message : 'Thread not found');
	}
};
