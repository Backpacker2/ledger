#!/usr/bin/env node
// Checks that every commit a pull request brings can be released. Pull requests are merged with a merge commit, so
// each commit on the branch lands on main as it is, and scripts/release.mjs reads every one of them (merge commits it
// leaves out, and so does this). CI runs it on every pull request (.github/workflows/commits.yml).
//
// For each commit that isn't a merge, it fails when:
//   the type isn't one of scripts/conventional.mjs's (then release.mjs skips the commit, silently)
//   the scope isn't in the Scopes table in CONTRIBUTING.md, or is CI's reserved `release`
//   the subject has a `!` but the body has no `BREAKING CHANGE:` footer saying what users must do
// It only warns about style: a capital first word, a full stop at the end. No scope is fine. At the end it says which
// release the commits make together: the largest bump any of them asks for.
//
// The scopes are read from the CONTRIBUTING.md next to this script, which in CI is the pull request's own: a branch
// that adds an area adds its scope row too, and its commits pass.
//
// usage: node scripts/check-commits.mjs               the commits on this branch that main doesn't have
//                                                     (origin/main..HEAD)
//        node scripts/check-commits.mjs <range>       the commits in another range, such as main..my-branch
//        node scripts/check-commits.mjs --message "<subject>" [--body "<body>" | --body-file <file>]
//                                                     a message, before you commit it
// In CI the range is BASE_SHA..HEAD_SHA, leaving out what the base branch BASE_REF already has.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { BREAKING_FOOTER, RELEASE_SCOPE, TYPES, parseHeader } from './conventional.mjs';

const CONTRIBUTING = new URL('../CONTRIBUTING.md', import.meta.url);
const RANK = { patch: 1, minor: 2, major: 3 };
const FIX =
	'To change a message: the last commit with `git commit --amend`; an earlier one with `git rebase -i origin/main`, ' +
	'marking it `reword`. Then `git push --force-with-lease`.';
const inActions = process.env.GITHUB_ACTIONS === 'true';
const git = (...args) => execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });

// ---- what to check
let message = null;
let body = null;
let range = null;
const args = process.argv.slice(2);
for (let i = 0; i < args.length; i++) {
	const arg = args[i];
	if (arg === '--message') message = args[++i] ?? '';
	else if (arg === '--body') body = args[++i] ?? '';
	else if (arg === '--body-file') body = readOrStop(args[++i]);
	else if (arg === '--help' || arg === '-h') usage(0);
	else if (arg.startsWith('-') || range !== null) usage(2);
	else range = arg;
}

let commits;
let where;
if (message !== null) {
	// A whole message in --message works too: its first line is the subject, the rest the body.
	const [subject, ...rest] = message.split(/\r?\n/);
	if (!subject.trim()) usage(2);
	commits = [{ hash: '', subject, body: body ?? rest.join('\n') }];
	where = 'the message';
} else {
	const revs = [];
	const { BASE_SHA: base, HEAD_SHA: head, BASE_REF: baseRef } = process.env;
	if (range !== null) {
		revs.push(range);
		where = range;
	} else if (base || head) {
		if (!isSha(base) || !isSha(head)) stop('BASE_SHA and HEAD_SHA must both be commit hashes.');
		revs.push(`${base}..${head}`);
		where = `${base.slice(0, 7)}..${head.slice(0, 7)}`;
		// base.sha can lag behind the base branch, and a merge of main into the branch would then bring main's own
		// commits (release commits too) into the range. Leave out whatever the base branch has now.
		const baseTip = `refs/remotes/origin/${baseRef}`;
		if (baseRef && /^[\w./-]+$/.test(baseRef) && hasCommit(baseTip)) revs.push('--not', baseTip);
	} else {
		revs.push('origin/main..HEAD');
		where = 'origin/main..HEAD';
	}
	commits = readCommits(revs);
}

if (!commits.length) {
	report('notice', `Nothing to check: no commits besides merges in ${where}.`);
	process.exit(0);
}

// ---- the scopes, from the table in CONTRIBUTING.md
const scopes = readScopes();
if (!scopes.length) {
	report(
		'error',
		'Found no scopes in CONTRIBUTING.md, so no commit can be checked. It needs a table under a "Scopes" heading ' +
			'with each scope in backticks in the second column, like | Data and sync | `vault`, `sync` |.',
	);
}

// ---- each commit
console.log(
	message !== null ? 'Checking the message:' : `Checking ${commits.length === 1 ? 'one commit' : `${commits.length} commits`} in ${where}:`,
);
let failed = 0;
let bump = null;
for (const commit of commits) {
	const name = commit.hash ? `${commit.hash.slice(0, 7)} ${commit.subject}` : commit.subject;
	const result = check(commit.subject, commit.body);
	console.log(`  ${result.errors.length ? 'FAIL' : 'ok  '}  ${name}`);
	const about = { title: commit.hash ? `Commit ${commit.hash.slice(0, 7)}` : 'Message', name };
	for (const text of result.warnings) report('warning', text, about);
	for (const text of result.errors) report('error', text, about);
	if (result.errors.length) failed++;
	else if (!bump || RANK[result.bump] > RANK[bump]) bump = result.bump;
}

// ---- the verdict
if (message !== null) {
	if (failed || !scopes.length) process.exit(1);
	report('notice', `OK: this message makes a ${bump} release.`);
	process.exit(0);
}
if (failed) {
	const count = commits.length === 1 ? 'The commit needs' : `${failed} of ${commits.length} commits ${failed === 1 ? 'needs' : 'need'}`;
	console.log(`\n${count} a new message. ${FIX}`);
}
if (failed || !scopes.length) process.exit(1);
const what = commits.length === 1 ? 'this commit makes' : `these ${commits.length} commits make`;
report('notice', `OK: merged, ${what} a ${bump} release.`);

// ---- helpers
// The rules for one commit, and the bump it asks for: breaking → major, feat → minor, any other type → patch, as in
// release.mjs.
function check(subject, text) {
	const errors = [];
	const warnings = [];
	const header = parseHeader(subject);
	if (!header) return { errors: [notConventional(subject)], warnings, bump: null };
	const { type, scope, bang, description } = header;

	if (scope === '') {
		errors.push(`The parentheses after "${type}" are empty. Name a scope, as in ${type}(sync): …, or leave them out.`);
	} else if (scope === RELEASE_SCOPE) {
		errors.push(
			`The scope "${RELEASE_SCOPE}" is reserved for CI's own chore(release) commits, which release.mjs leaves out. ` +
				'Use the scope of the area you changed.',
		);
	} else if (scope !== undefined && scopes.length && !scopes.includes(scope)) {
		errors.push(
			`"${scope}" is not a scope in CONTRIBUTING.md. Use one of: ${scopes.join(', ')}. ` +
				'If this adds a new area, add a row with its scope to the Scopes table in CONTRIBUTING.md on this branch.',
		);
	}

	const footer = BREAKING_FOOTER.test(text);
	if (bang && !footer) {
		errors.push(
			'The "!" marks a breaking change, which releases a new major version, but the message has no ' +
				'"BREAKING CHANGE:" footer. Add a line to its body that starts with "BREAKING CHANGE: " and says what users ' +
				"must do; it becomes the release note. If users don't have to do anything, take the \"!\" out.",
		);
	} else if (footer && !bang) {
		warnings.push(
			'The body has a "BREAKING CHANGE:" line, so this commit releases a new major version. Add "!" to the subject ' +
				`to show it (${type}${scope ? `(${scope})` : ''}!: …), or remove the line if users don't have to act.`,
		);
	}

	if (/^[A-Z][a-z]*\b/.test(description)) {
		warnings.push(`Start the text after the colon in lowercase ("${lowerFirst(description)}"), unless its first word is a name.`);
	}
	if (/\.$/.test(description)) warnings.push('Leave out the full stop at the end of the subject.');

	return { errors, warnings, bump: bang || footer ? 'major' : type === 'feat' ? 'minor' : 'patch' };
}

function notConventional(subject) {
	const example = 'fix(sync): keep the token when the branch changes';
	const loose = /^\s*([A-Za-z]+)(?:\s*\(([^)]*)\))?\s*(!)?\s*:/.exec(subject);
	if (loose && TYPES.includes(loose[1])) {
		return `This is almost a conventional commit: write it as type(scope): description, with no spaces before the colon and one after it, as in "${example}".`;
	}
	if (loose && TYPES.includes(loose[1].toLowerCase())) {
		return `Write the type in lowercase: "${loose[1].toLowerCase()}", not "${loose[1]}".`;
	}
	if (loose) {
		return `"${loose[1]}" is not a type. Use one of: ${TYPES.join(', ')}; CONTRIBUTING.md says which fits.`;
	}
	return (
		'This is not a conventional commit, so release.mjs would skip it and release nothing. Start the subject with ' +
		`a type, an optional scope and a colon, as in "${example}". Types: ${TYPES.join(', ')}.`
	);
}

function readScopes() {
	let text;
	try {
		text = fs.readFileSync(CONTRIBUTING, 'utf8');
	} catch {
		return [];
	}
	const lines = text.split(/\r?\n/);
	const start = lines.findIndex((line) => /^#{1,6}\s+Scopes\b/.test(line));
	if (start < 0) return [];
	const found = [];
	let inTable = false;
	for (const line of lines.slice(start + 1)) {
		if (/^#{1,6}\s/.test(line)) break;
		if (!line.trimStart().startsWith('|')) {
			if (inTable) break;
			continue;
		}
		inTable = true;
		const cells = line.split('|').slice(1, -1);
		// The header row and the |---| row have no scopes in backticks, and `chore(release)` isn't one name.
		for (const [, scope] of (cells[1] ?? '').matchAll(/`([a-z][a-z0-9-]*)`/g)) found.push(scope);
	}
	return [...new Set(found)].filter((scope) => scope !== RELEASE_SCOPE);
}

// The same reading of the log as release.mjs: merge commits left out, oldest first here.
function readCommits(revs) {
	let log;
	try {
		log = git('log', '--no-merges', '--reverse', '--format=%H%x1f%s%x1f%b%x1e', ...revs, '--');
	} catch (e) {
		stop(`Can't read the commits in ${where}: ${String(e.stderr || e.message).trim()}`);
	}
	return log
		.split('\x1e')
		.map((entry) => entry.trim())
		.filter(Boolean)
		.map((entry) => {
			const [hash, subject, text = ''] = entry.split('\x1f');
			return { hash, subject, body: text };
		});
}

function hasCommit(ref) {
	try {
		git('rev-parse', '--verify', '--quiet', `${ref}^{commit}`);
		return true;
	} catch {
		return false;
	}
}

function isSha(value) {
	return /^[0-9a-f]{7,64}$/i.test(value ?? '');
}

function lowerFirst(text) {
	return text.charAt(0).toLowerCase() + text.slice(1);
}

// In GitHub Actions a message becomes an annotation on the run (error, warning or notice), which names the commit it
// is about; locally it's a line under that commit's.
function report(level, text, { title = 'Commits', name = '' } = {}) {
	if (!inActions) {
		console.log(level === 'notice' ? text : `${name ? '        ' : ''}${level}: ${text}`);
		return;
	}
	const data = `${name ? `${name}: ` : ''}${text}`.replace(/%/g, '%25').replace(/\r/g, '%0D').replace(/\n/g, '%0A');
	console.log(`::${level} title=${title}::${data}`);
}

function readOrStop(file) {
	try {
		return fs.readFileSync(file, 'utf8');
	} catch {
		stop(`Can't read the body from ${file ?? '(no file given)'}.`);
	}
}

function stop(text) {
	console.error(text);
	process.exit(2);
}

function usage(code) {
	const text = [
		'usage: node scripts/check-commits.mjs               the commits on this branch that main doesn\'t have (origin/main..HEAD)',
		'       node scripts/check-commits.mjs <range>       the commits in another range, such as main..my-branch',
		'       node scripts/check-commits.mjs --message "<subject>" [--body "<body>" | --body-file <file>]',
		'                                                    a message, before you commit it',
		'In CI the range is BASE_SHA..HEAD_SHA, leaving out what the base branch BASE_REF already has.',
	].join('\n');
	(code ? console.error : console.log)(text);
	process.exit(code);
}
