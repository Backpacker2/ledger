import { api } from '$lib/api';
import type { DeliverableSummary } from '$lib/types';
import type { PageLoad } from './$types';

/** What the dashboard shows of a project beyond its summary: its deliverables and how many tasks are still open. */
export interface ProjectDetail {
	deliverables: DeliverableSummary[];
	openTasks: number;
}

export const load: PageLoad = async ({ parent }) => {
	const { projects } = await parent();
	const [activity, details] = await Promise.all([
		api.listActivity(),
		Promise.all(
			projects.map(async (p) => {
				const [deliverables, view] = await Promise.all([api.listDeliverables(p.slug), api.getProject(p.slug)]);
				return [p.slug, { deliverables, openTasks: view.openTaskCount }] as const;
			})
		)
	]);
	return { activity, details: Object.fromEntries(details) as Record<string, ProjectDetail> };
};
