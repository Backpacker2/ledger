import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params }) => ({ templates: await api.projectTemplates(params.project) });
