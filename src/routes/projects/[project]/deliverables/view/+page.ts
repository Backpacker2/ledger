import { error } from '@sveltejs/kit';
import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, url }) => {
	const path = url.searchParams.get('path') ?? '';
	if (!path) error(404, 'No deliverable was asked for');
	try {
		const [deliverable, provenance] = await Promise.all([api.getDeliverable(params.project, path), api.getProvenance(params.project, path)]);
		return { deliverable, provenance };
	} catch (e) {
		error(404, e instanceof Error ? e.message : 'Deliverable not found');
	}
};
