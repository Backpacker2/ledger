import { api } from '$lib/api';
import type { PageLoad } from './$types';

export const load: PageLoad = async () => {
	const [documents, documentTypes] = await Promise.all([api.listDocuments(), api.listDocumentTypes()]);
	return { documents, documentTypes };
};
