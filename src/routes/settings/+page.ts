import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async () => ({ info: await api.appInfo() });
