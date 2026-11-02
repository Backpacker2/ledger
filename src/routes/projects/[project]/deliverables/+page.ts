import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params }) => ({
	deliverables: await api.listDeliverables(params.project),
	templates: await api.effectiveTemplates(params.project)
});
