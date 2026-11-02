import { error } from '@sveltejs/kit';
import { api } from '$lib/api';
import type { PageLoad } from './$types';

/** `?v=1.2` opens a published version (read-only); without it the live draft. */
export const load: PageLoad = async ({ params, url }) => {
	const version = url.searchParams.get('v');
	try {
		const doc = version ? await api.getDocumentVersion(params.id, version) : await api.getDocument(params.id);
		return { doc };
	} catch (e) {
		error(404, e instanceof Error ? e.message : 'Document not found');
	}
};
