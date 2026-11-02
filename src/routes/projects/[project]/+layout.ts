import { api } from '$lib/api';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async ({ params }) => ({ project: await api.getProject(params.project) });
