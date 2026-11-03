import { api } from '$lib/api';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async () => ({ settings: await api.getSettings() });
