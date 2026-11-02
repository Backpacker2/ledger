// Timestamps arrive as UTC ISO strings; everything shown to people is in local time.

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
const dateFormat = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
const longDayFormat = new Intl.DateTimeFormat(undefined, { weekday: 'long', day: 'numeric', month: 'long' });

export const time = (iso: string) => timeFormat.format(new Date(iso));

export const date = (iso: string) => dateFormat.format(new Date(iso));

export const dateTime = (iso: string) => `${date(iso)}, ${time(iso)}`;

/** Local calendar day, "2026-10-16": what posts are grouped under. */
export function dayKey(iso: string): string {
	const d = new Date(iso);
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function dayLabel(key: string): string {
	const [y, m, d] = key.split('-').map(Number);
	const day = new Date(y, m - 1, d);
	const today = new Date();
	const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate());
	const diff = Math.round((startOfToday.getTime() - day.getTime()) / 86_400_000);
	if (diff === 0) return 'Today';
	if (diff === 1) return 'Yesterday';
	return longDayFormat.format(day) + (y !== today.getFullYear() ? ` ${y}` : '');
}

/** Markdown flattened to one line of plain text, for quoting a post. */
export function plainText(markdown: string, max = 140): string {
	const text = markdown
		.replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
		.replace(/^\s*[-+*]\s+/gm, '')
		.replace(/[*_`~>#]+/g, '')
		.replace(/\s+/g, ' ')
		.trim();
	return text.length > max ? `${text.slice(0, max).trimEnd()}…` : text;
}

/** "3 posts", "1 post". */
export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Escapes text and wraps case-insensitive matches of `query` in <mark>, for search snippets. */
export function highlight(text: string, query: string): string {
	const escape = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
	const needle = query.trim();
	if (!needle) return escape(text);
	const pattern = new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
	let out = '';
	let last = 0;
	for (const match of text.matchAll(pattern)) {
		out += escape(text.slice(last, match.index)) + `<mark>${escape(match[0])}</mark>`;
		last = match.index + match[0].length;
	}
	return out + escape(text.slice(last));
}
