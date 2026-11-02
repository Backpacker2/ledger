import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ url }) => {
	const q = (url.searchParams.get('q') ?? '').trim();
	return { q, hits: q ? await api.search(q) : [] };
};
