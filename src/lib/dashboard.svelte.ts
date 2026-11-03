// The dashboard's display preferences: which projects are pinned to the top, and whether the contribution graph is
// shown. Like the theme and the order, they are kept in the browser's storage, per machine, not in the vault. Pins are
// project slugs, which a rename keeps; a pin whose project is gone is simply never matched.

const PINS = 'ledger-pinned-projects';
const ACTIVITY = 'ledger-show-activity';

function read<T>(key: string, fallback: T, valid: (value: unknown) => value is T): T {
	try {
		const saved: unknown = JSON.parse(localStorage.getItem(key) ?? 'null');
		if (valid(saved)) return saved;
	} catch {
		// Storage can be unavailable or hold something else; the default is fine.
	}
	return fallback;
}

function write(key: string, value: unknown) {
	try {
		localStorage.setItem(key, JSON.stringify(value));
	} catch {
		// Not persisted, still applied for this session.
	}
}

const isSlugs = (v: unknown): v is string[] => Array.isArray(v) && v.every((s) => typeof s === 'string');
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

export const dashboard = $state<{ pins: string[]; showActivity: boolean }>({
	/** Pinned slugs, in the order they were pinned. */
	pins: read(PINS, [], isSlugs),
	showActivity: read(ACTIVITY, true, isBool)
});

export const isPinned = (slug: string) => dashboard.pins.includes(slug);

export function togglePin(slug: string) {
	dashboard.pins = isPinned(slug) ? dashboard.pins.filter((s) => s !== slug) : [...dashboard.pins, slug];
	write(PINS, dashboard.pins);
}

/** Settings → Dashboard: whether the contribution graph is shown. */
export function setShowActivity(show: boolean) {
	dashboard.showActivity = show;
	write(ACTIVITY, show);
}
