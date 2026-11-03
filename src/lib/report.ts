// Reporting an error from the app's log: as a GitHub issue (a filled-in "new issue" link: nothing is posted until
// you submit it there), or as a prompt to paste into an AI assistant. Plain functions, no app state.

import type { AboutInfo, LogEntry } from './types';

export const REPO_URL = 'https://github.com/openledger-nl/ledger';

/**
 * The longest new-issue link that is sent. GitHub answers "Your request URL is too long" well before browsers give
 * up, because the request's headers (its cookies for github.com) count against the same limit, so this stays far
 * below it.
 */
export const MAX_ISSUE_URL = 4000;

/**
 * Takes out what shouldn't leave this machine: the home folder becomes `~`, and anything shaped like a GitHub
 * token or a recovery key is replaced. File and folder names inside the vault stay, so read a report before you
 * send it.
 */
export function redact(text: string, homeDir: string): string {
	let out = text;
	if (homeDir) out = out.split(homeDir).join('~');
	return out
		.replace(/\b(github_pat_|ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9_]+/g, '[token removed]')
		.replace(/ledger-key-\d+:\S+/g, '[recovery key removed]')
		.replace(/\b(token|key|secret)=([^\s&]+)/gi, '$1=[removed]');
}

const cut = (text: string, max: number) => (text.length > max ? `${text.slice(0, max)} …` : text);

export function entryLine(e: LogEntry, maxMessage = Infinity) {
	return `${e.at} ${e.level.toUpperCase().padEnd(5)} [${e.source}] ${cut(e.message, maxMessage)}`;
}

/** How much of the error and the log goes into a report. */
interface Size {
	/** How many log lines, counted back from the error. */
	lines: number;
	/** The most characters of the error's message. */
	message: number;
	/** The most characters of each log line's message. */
	line: number;
}

const FULL: Size = { lines: 20, message: Infinity, line: 500 };

function build(about: AboutInfo | null) {
	return about ? `Ledger ${about.version} (Tauri ${about.tauriVersion}) on ${about.os} ${about.arch}` : 'Ledger';
}

/** The issue's body: the error, the build, and the log just before it. */
export function issueBody(entry: LogEntry, about: AboutInfo | null, recent: LogEntry[], size: Size = FULL): string {
	const home = about?.homeDir ?? '';
	const log = redact(
		(size.lines ? recent.slice(-size.lines) : [])
			.map((e) => entryLine(e, size.line))
			.join('\n'),
		home
	);
	return [
		'### What went wrong',
		'',
		'```',
		cut(redact(entry.message, home), size.message),
		'```',
		'',
		`- **Where:** ${entry.source}`,
		`- **When:** ${entry.at}`,
		`- **Build:** ${build(about)}`,
		'',
		'### What I was doing',
		'',
		'<!-- A sentence or two: what you clicked or expected. -->',
		...(size.lines ? ['', '### Recent log', '', '```', log, '```'] : [])
	].join('\n');
}

function issueTitle(entry: LogEntry, about: AboutInfo | null) {
	return `[${entry.source}] ${cut(redact(entry.message, about?.homeDir ?? '').split('\n')[0], 80)}`;
}

const newIssue = (title: string, body: string) => `${REPO_URL}/issues/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;

/**
 * A new-issue link with as much of the report as fits in `MAX_ISSUE_URL`: the log is shortened first, then the
 * error. `complete` says whether the whole report is in it; when it isn't, `body` is the whole report, for the
 * clipboard, and the link asks for it to be pasted.
 */
export function issueLink(entry: LogEntry, about: AboutInfo | null, recent: LogEntry[]): { url: string; complete: boolean; body: string } {
	const title = issueTitle(entry, about);
	const body = issueBody(entry, about, recent);
	const full = newIssue(title, body);
	if (full.length <= MAX_ISSUE_URL) return { url: full, complete: true, body };

	const sizes: Size[] = [
		{ lines: 20, message: 1500, line: 200 },
		{ lines: 10, message: 1000, line: 160 },
		{ lines: 5, message: 600, line: 120 },
		{ lines: 0, message: 600, line: 0 },
		{ lines: 0, message: 200, line: 0 }
	];
	for (const size of sizes) {
		const shorter = issueBody(entry, about, recent, size);
		const url = newIssue(title, `${shorter}\n\n<!-- Shortened to fit a link: paste the full report from your clipboard here. -->`);
		if (url.length <= MAX_ISSUE_URL) return { url, complete: false, body };
	}
	const placeholder = `<!-- The report was too long for a link. It is on your clipboard: paste it here. -->\n\n- **Where:** ${entry.source}\n- **Build:** ${build(about)}`;
	return { url: newIssue(cut(title, 60), placeholder), complete: false, body };
}

/** A prompt to paste into an AI assistant: what Ledger is, the error, and the context to work it out. */
export function aiPrompt(entry: LogEntry, about: AboutInfo | null, recent: LogEntry[]): string {
	const home = about?.homeDir ?? '';
	const log = redact(
		recent
			.slice(-40)
			.map((e) => entryLine(e, 2000))
			.join('\n'),
		home
	);
	return [
		`I use Ledger (${REPO_URL}), a local-first desktop app built with Tauri 2 (Rust, in src-tauri/) and SvelteKit with Svelte 5 (in src/). It keeps its data as a vault of markdown files and can sync it to a private GitHub repository and to Trello.`,
		'',
		'It showed me this error:',
		'',
		'```',
		redact(entry.message, home),
		'```',
		'',
		`It came from: ${entry.source} (a command name, or sync, trello, ui, panic), at ${entry.at}.`,
		`Build: ${build(about)}.`,
		'',
		'The app log just before it:',
		'',
		'```',
		log,
		'```',
		'',
		'Explain in plain words what most likely went wrong and what I can do about it. If it looks like a bug in Ledger itself, say where in the code it probably comes from and how it could be fixed.'
	].join('\n');
}

// ---- debug captures ---------------------------------------------------------------------------------

function duration(from: string, to: string) {
	const seconds = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
	return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/** The capture file: a header with the build and the span, then every step, made fit to share. */
export function captureText(entries: LogEntry[], about: AboutInfo | null): string {
	const first = entries[0]?.at ?? '';
	const last = entries.at(-1)?.at ?? '';
	const errors = entries.filter((e) => e.level === 'error').length;
	const header = [
		'# Ledger debug capture',
		`# ${build(about)}`,
		`# From ${first} to ${last} (${first && last ? duration(first, last) : '?'}), ${entries.length} steps, ${errors} errors`,
		'# What you type is not recorded; the home folder and anything shaped like a token are taken out.',
		''
	];
	return redact([...header, ...entries.map((e) => entryLine(e))].join('\n') + '\n', about?.homeDir ?? '');
}

function fileName(path: string) {
	return path.split(/[\\/]/).pop() ?? path;
}

/** A new-issue link for a capture: a short summary, and a reminder to drag the file in. */
export function captureIssueUrl(path: string, entries: LogEntry[], about: AboutInfo | null): string {
	const home = about?.homeDir ?? '';
	const errors = entries.filter((e) => e.level === 'error');
	const first = entries[0]?.at ?? '';
	const last = entries.at(-1)?.at ?? '';
	const title = errors.length ? `[capture] ${cut(redact(errors[0].message, home).split('\n')[0], 80)}` : '[capture] Something went wrong';
	const body = (shown: number) =>
		[
			'### What I was doing',
			'',
			'<!-- What you did during the capture, and what you expected to happen. -->',
			'',
			'### Debug capture',
			'',
			`<!-- Drag ${fileName(path)} from the folder Ledger just opened into this box. -->`,
			'',
			`- **Build:** ${build(about)}`,
			`- **Span:** ${first && last ? duration(first, last) : '?'}, ${entries.length} steps, ${errors.length} errors`,
			...(shown ? ['', '**Errors in it:**', '', '```', ...errors.slice(0, shown).map((e) => cut(redact(entryLine(e), home), 300)), '```'] : [])
		].join('\n');
	// Fewer errors are listed until the link fits; the file has them all.
	for (let shown = Math.min(5, errors.length); shown > 0; shown--) {
		const url = newIssue(title, body(shown));
		if (url.length <= MAX_ISSUE_URL) return url;
	}
	return newIssue(title, body(0));
}

/** A prompt to paste into an AI assistant with the whole capture in it. */
export function captureAiPrompt(text: string): string {
	return [
		`I use Ledger (${REPO_URL}), a local-first desktop app built with Tauri 2 (Rust, in src-tauri/) and SvelteKit with Svelte 5 (in src/). Below is a debug capture: every action the window asked the backend for (by command name, with how long it took), each page it opened, each message it showed, sync rounds and errors, from when I started the capture to when I stopped it.`,
		'',
		'```',
		text.trimEnd(),
		'```',
		'',
		'Tell me in plain words what went wrong and when, and what I can do about it. If it looks like a bug in Ledger itself, say where in the code it probably comes from and how it could be fixed.'
	].join('\n');
}
