<script lang="ts">
	import Dropdown, { type DropdownOption } from '$lib/components/Dropdown.svelte';
	import { dayKey, plural } from '$lib/format';
	import type { ActivityEvent, ActivityKind, ProjectSummary } from '$lib/types';

	// A year of days, a square each, shaded by how much was made that day: like GitHub's contribution graph. Weeks
	// run down the columns, Monday first. Days are local days, so an evening's work lands on that evening.

	let { events, projects }: { events: ActivityEvent[]; projects: ProjectSummary[] } = $props();

	/** '' is every project. */
	let project = $state('');
	/** 'month' is the last 30 days, 'last' the last twelve months; otherwise a year, "2026". */
	let period = $state('last');

	const KIND: Record<ActivityKind, [string, string]> = {
		post: ['post', 'posts'],
		prompt: ['prompt', 'prompts'],
		feedback: ['feedback', 'feedback'],
		thread: ['thread', 'threads'],
		upload: ['upload', 'uploads'],
		version: ['published version', 'published versions'],
		run: ['Claude Code run', 'Claude Code runs'],
		story: ['story', 'stories'],
		task: ['task', 'tasks']
	};
	const ORDER = Object.keys(KIND) as ActivityKind[];

	const addDays = (d: Date, n: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
	const mondayOf = (d: Date) => addDays(d, -((d.getDay() + 6) % 7));
	const keyOf = (d: Date) => dayKey(d.toISOString());
	const today = addDays(new Date(), 0);
	const monthFormat = new Intl.DateTimeFormat(undefined, { month: 'short' });
	const longDate = new Intl.DateTimeFormat(undefined, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
	const shortDate = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });

	type Day = { date: Date; key: string; kinds: Partial<Record<ActivityKind, number>>; total: number; level: number };

	const mine = $derived(project ? events.filter((e) => e.projectSlug === project) : events);

	/** What was made each local day: "2026-11-03" → the counts by kind. */
	const byDay = $derived.by(() => {
		const days = new Map<string, Partial<Record<ActivityKind, number>>>();
		for (const e of mine) {
			const key = dayKey(e.at);
			const kinds = days.get(key) ?? {};
			kinds[e.kind] = (kinds[e.kind] ?? 0) + 1;
			days.set(key, kinds);
		}
		return days;
	});
	const totalOf = (key: string) => Object.values(byDay.get(key) ?? {}).reduce((a, b) => a + b, 0);

	const years = $derived([...new Set(events.map((e) => new Date(e.at).getFullYear()))].sort((a, b) => b - a));

	const range = $derived(
		period === 'month'
			? { from: addDays(today, -29), to: today }
			: period === 'last'
				? { from: addDays(mondayOf(today), -52 * 7), to: today }
				: { from: new Date(+period, 0, 1), to: new Date(+period, 11, 31) }
	);

	const projectOptions = $derived<DropdownOption[]>([
		{ value: '', label: 'All projects' },
		...projects.map((p) => ({ value: p.slug, label: p.name, group: 'Projects' }))
	]);
	const periodOptions = $derived<DropdownOption[]>([
		{ value: 'month', label: 'Last month' },
		{ value: 'last', label: 'Last 12 months' },
		...years.map((y) => ({ value: String(y), label: String(y), group: 'Year' }))
	]);

	/** The weeks shown, each seven days from Monday; null where a day falls outside the period or in the future. */
	const weeks = $derived.by(() => {
		const out: (Day | null)[][] = [];
		for (let monday = mondayOf(range.from); monday <= range.to; monday = addDays(monday, 7)) {
			const week: (Day | null)[] = [];
			for (let i = 0; i < 7; i++) {
				const date = addDays(monday, i);
				if (date < range.from || date > range.to || date > today) {
					week.push(null);
					continue;
				}
				const key = keyOf(date);
				week.push({ date, key, kinds: byDay.get(key) ?? {}, total: totalOf(key), level: 0 });
			}
			out.push(week);
		}
		// Shade by quartiles of the active days, so one huge day doesn't wash the rest out.
		const active = out
			.flat()
			.filter((d): d is Day => !!d && d.total > 0)
			.map((d) => d.total)
			.sort((a, b) => a - b);
		const cut = [0.25, 0.5, 0.75].map((q) => active[Math.floor(q * (active.length - 1))] ?? 0);
		for (const d of out.flat()) {
			if (d && d.total > 0) d.level = 1 + cut.filter((c) => d.total > c).length;
		}
		return out;
	});

	const days = $derived(weeks.flat().filter((d): d is Day => !!d));
	const total = $derived(days.reduce((n, d) => n + d.total, 0));

	/** A month's name above the first week in which it starts; dropped when it would crowd the one before. */
	const months = $derived.by(() => {
		const labels: { column: number; label: string }[] = [];
		weeks.forEach((week, column) => {
			const first = week.find((d) => d && (column === 0 || d.date.getDate() === 1));
			if (!first) return;
			if (column > 0 && labels.length && column - labels.at(-1)!.column < (period === 'month' ? 2 : 3)) labels.pop();
			labels.push({ column, label: monthFormat.format(first.date) });
		});
		return labels;
	});

	const stats = $derived.by(() => {
		let longest = 0;
		let run = 0;
		let busiest: Day | null = null;
		for (const d of days) {
			run = d.total > 0 ? run + 1 : 0;
			longest = Math.max(longest, run);
			if (d.total > (busiest?.total ?? 0)) busiest = d;
		}
		// The current streak runs back from today, or from yesterday while nothing has been made yet today.
		let current = 0;
		let day = totalOf(keyOf(today)) > 0 ? today : addDays(today, -1);
		while (totalOf(keyOf(day)) > 0) {
			current++;
			day = addDays(day, -1);
		}
		return { active: days.filter((d) => d.total > 0).length, longest, current, busiest };
	});

	const breakdown = (d: Day) =>
		ORDER.filter((k) => d.kinds[k])
			.map((k) => {
				const n = d.kinds[k] as number;
				return `${n} ${KIND[k][n === 1 ? 0 : 1]}`;
			})
			.join(' · ');

	// The hover card: placed over the hovered day, inside the graph's box.
	let box: HTMLElement | undefined = $state();
	let tip = $state<{ day: Day; x: number; y: number } | null>(null);
	const lookup = $derived(new Map(days.map((d) => [d.key, d])));

	function hover(e: PointerEvent) {
		const cell = (e.target as HTMLElement).closest<HTMLElement>('[data-key]');
		const day = cell && lookup.get(cell.dataset.key ?? '');
		if (!cell || !day || !box) return (tip = null);
		const a = cell.getBoundingClientRect();
		const b = box.getBoundingClientRect();
		// Kept inside the box, so the first and last weeks' cards don't hang over its edges.
		const x = Math.min(Math.max(a.left + a.width / 2 - b.left, 140), b.width - 140);
		tip = { day, x, y: a.top - b.top };
	}

	// The newest weeks are on the right: start there when the graph is wider than its box.
	let scroller: HTMLElement | undefined = $state();
	$effect(() => {
		void weeks;
		if (scroller) scroller.scrollLeft = scroller.scrollWidth;
	});

	const periodLabel = $derived(period === 'month' ? 'in the last month' : period === 'last' ? 'in the last year' : `in ${period}`);
</script>

<section class="panel contrib" aria-label="Contributions" bind:this={box}>
	<div class="header-top">
		<h2>
			{total.toLocaleString()} {total === 1 ? 'contribution' : 'contributions'}
			{periodLabel}{#if project}<span class="contrib-in"> · {projects.find((p) => p.slug === project)?.name}</span>{/if}
		</h2>
		<div class="contrib-filters">
			<Dropdown bind:value={project} options={projectOptions} label="Project" align="end" />
			<Dropdown bind:value={period} options={periodOptions} label="Period" align="end" />
		</div>
	</div>

	<div class="contrib-scroll" bind:this={scroller}>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="contrib-grid"
			class:big={period === 'month'}
			style="--weeks: {weeks.length}"
			role="img"
			aria-label="{plural(total, 'contribution')} on {plural(stats.active, 'day')} {periodLabel}"
			onpointerover={hover}
			onpointerleave={() => (tip = null)}
		>
			{#each months as m (m.column)}
				<span class="contrib-month" style="grid-column: {m.column + 2}">{m.label}</span>
			{/each}
			<span class="contrib-wd" style="grid-row: 2">Mon</span>
			<span class="contrib-wd" style="grid-row: 4">Wed</span>
			<span class="contrib-wd" style="grid-row: 6">Fri</span>
			{#each weeks as week, w (w)}
				{#each week as d, i (i)}
					{#if d}
						<span class="contrib-day l{d.level}" class:today={d.key === keyOf(today)} style="grid-column: {w + 2}; grid-row: {i + 2}" data-key={d.key}></span>
					{/if}
				{/each}
			{/each}
		</div>
	</div>

	<div class="contrib-foot">
		<div class="contrib-stats">
			<span><b>{stats.active}</b> active {stats.active === 1 ? 'day' : 'days'}</span>
			<span><b>{stats.current}</b> {stats.current === 1 ? 'day' : 'days'} current streak</span>
			<span><b>{stats.longest}</b> {stats.longest === 1 ? 'day' : 'days'} longest streak</span>
			{#if stats.busiest}
				<span>Busiest: <b>{shortDate.format(stats.busiest.date)}</b> ({stats.busiest.total})</span>
			{/if}
		</div>
		<div class="contrib-legend" aria-hidden="true">
			Less
			{#each [0, 1, 2, 3, 4] as l (l)}<span class="contrib-day l{l}"></span>{/each}
			More
		</div>
	</div>

	{#if tip}
		<div class="contrib-tip" style="left: {tip.x}px; top: {tip.y}px" role="tooltip">
			<b>{tip.day.total ? plural(tip.day.total, 'contribution') : 'No contributions'}</b> on {longDate.format(tip.day.date)}
			{#if tip.day.total}<span>{breakdown(tip.day)}</span>{/if}
		</div>
	{/if}
</section>
