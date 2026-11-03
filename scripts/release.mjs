#!/usr/bin/env node
// Works out Ledger's next version from the conventional commits since the last release, and with --write puts it in
// every place the version lives (package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and Cargo.lock) and
// at the top of CHANGELOG.md. CI runs it on every push to main (.github/workflows/release.yml); run it bare to see
// what the next release would be.
//
// The bump is the largest one any commit asks for:
//   a breaking change (`feat!: …`, `fix(api)!: …`, or a `BREAKING CHANGE:` footer)  → major  0.4.2 → 1.0.0
//   feat                                                                             → minor  0.4.2 → 0.5.0
//   any other conventional type (fix, perf, refactor, docs, chore, …)               → patch  0.4.2 → 0.4.3
// Commits that aren't conventional don't count; with none that do, there is no release. What counts is defined in
// scripts/conventional.mjs, which the pull request title check (scripts/pr-title.mjs) uses too.
//
// usage: node scripts/release.mjs            show the next version and its notes, change nothing
//        node scripts/release.mjs --write    write them (what CI does)
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { BREAKING_FOOTER, RELEASE_SCOPE, parseHeader } from './conventional.mjs';

const RANK = { patch: 1, minor: 2, major: 3 };

const git = (...args) => execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
const write = process.argv.includes('--write');

// ---- where we start from
let lastTag = '';
try {
	lastTag = git('describe', '--tags', '--abbrev=0', '--match', 'v[0-9]*.[0-9]*.[0-9]*');
} catch {
	// No release yet: every commit counts, from the version the files hold now.
}
const current = JSON.parse(fs.readFileSync('package.json', 'utf8')).version;
const range = lastTag ? [`${lastTag}..HEAD`] : ['HEAD'];

// ---- read the commits
const log = git('log', ...range, '--no-merges', '--format=%H%x1f%s%x1f%b%x1e');
const commits = log
	.split('\x1e')
	.map((entry) => entry.trim())
	.filter(Boolean)
	.map((entry) => {
		const [hash, subject, body = ''] = entry.split('\x1f');
		const header = parseHeader(subject);
		if (!header) return { hash, subject, conventional: false };
		const { type, scope, bang, description } = header;
		const footer = BREAKING_FOOTER.exec(body);
		return { hash, subject, conventional: true, type, scope, description, breaking: Boolean(bang || footer), breakingNote: footer?.[1] };
	})
	// Release commits are the result of a release, not a reason for one.
	.filter((c) => !(c.conventional && c.type === 'chore' && c.scope === RELEASE_SCOPE));

const counted = commits.filter((c) => c.conventional);
const bump = counted
	.map((c) => (c.breaking ? 'major' : c.type === 'feat' ? 'minor' : 'patch'))
	.reduce((best, level) => (!best || RANK[level] > RANK[best] ? level : best), null);

const skipped = commits.filter((c) => !c.conventional);
if (skipped.length) {
	console.error(`Not conventional, so not counted (${skipped.length}):`);
	for (const c of skipped) console.error(`  ${c.hash.slice(0, 7)} ${c.subject}`);
}

if (!bump) {
	console.error(`No conventional commits since ${lastTag || 'the start'}; no release.`);
	output({ released: 'false' });
	process.exit(0);
}

// ---- the next version
const [major, minor, patch] = current.split('.').map(Number);
const next =
	bump === 'major' ? `${major + 1}.0.0` : bump === 'minor' ? `${major}.${minor + 1}.0` : `${major}.${minor}.${patch + 1}`;

// ---- release notes, grouped
const repo = process.env.GITHUB_REPOSITORY;
const ref = (c) => (repo ? `[${c.hash.slice(0, 7)}](https://github.com/${repo}/commit/${c.hash})` : c.hash.slice(0, 7));
const line = (c) => `- ${c.scope ? `**${c.scope}:** ` : ''}${c.description} (${ref(c)})`;
const sections = [
	['Breaking changes', counted.filter((c) => c.breaking), (c) => `- ${c.scope ? `**${c.scope}:** ` : ''}${c.breakingNote ?? c.description} (${ref(c)})`],
	['Features', counted.filter((c) => !c.breaking && c.type === 'feat'), line],
	['Fixes', counted.filter((c) => !c.breaking && c.type === 'fix'), line],
	['Other changes', counted.filter((c) => !c.breaking && c.type !== 'feat' && c.type !== 'fix'), line],
];
const date = new Date().toISOString().slice(0, 10);
const notes = sections
	.filter(([, list]) => list.length)
	.map(([title, list, fmt]) => `### ${title}\n\n${list.map(fmt).join('\n')}`)
	.join('\n\n');

console.log(`${current} → ${next} (${bump}, ${counted.length} commit${counted.length === 1 ? '' : 's'} since ${lastTag || 'the start'})\n`);
console.log(notes);

if (!write) process.exit(0);

// ---- write the version everywhere it lives
const replaceOnce = (file, pattern, replacement) => {
	const text = fs.readFileSync(file, 'utf8');
	if (!pattern.test(text)) throw new Error(`No version found in ${file}`);
	fs.writeFileSync(file, text.replace(pattern, replacement));
};
replaceOnce('package.json', /("version":\s*")[^"]+(")/, `$1${next}$2`);
replaceOnce('src-tauri/tauri.conf.json', /("version":\s*")[^"]+(")/, `$1${next}$2`);
replaceOnce('src-tauri/Cargo.toml', /(\[package\][^[]*?\nversion\s*=\s*")[^"]+(")/, `$1${next}$2`);
replaceOnce('src-tauri/Cargo.lock', /(\[\[package\]\]\nname = "ledger"\nversion = ")[^"]+(")/, `$1${next}$2`);

const changelog = fs.existsSync('CHANGELOG.md') ? fs.readFileSync('CHANGELOG.md', 'utf8').replace(/^# Changelog\n+/, '') : '';
fs.writeFileSync('CHANGELOG.md', `# Changelog\n\n## ${next} (${date})\n\n${notes}\n\n${changelog}`.trimEnd() + '\n');
fs.writeFileSync('.release-notes.md', `${notes}\n`);

output({ released: 'true', version: next, tag: `v${next}` });
console.error(`\nWrote ${next}.`);

function output(values) {
	if (!process.env.GITHUB_OUTPUT) return;
	fs.appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(values).map(([k, v]) => `${k}=${v}\n`).join(''));
}
