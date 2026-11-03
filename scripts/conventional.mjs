// What counts as a conventional commit in Ledger, in one place: the types, how a first line (a commit's subject, or
// a pull request's title, which becomes one when it is squashed) is read, and how a body marks a breaking change.
// scripts/release.mjs reads the commits on main with it, and scripts/pr-title.mjs checks pull request titles with it,
// so the two can't disagree. The types and scopes are explained for people in CONTRIBUTING.md.

// ---- the types that count
export const TYPES = ['feat', 'fix', 'perf', 'refactor', 'style', 'docs', 'test', 'build', 'ci', 'chore', 'revert'];

// ---- the first line: type(scope)!: description
export const HEADER = new RegExp(`^(${TYPES.join('|')})(?:\\(([^)]*)\\))?(!)?: (.+)$`);

// The scope of CI's own `chore(release): vX.Y.Z` commits; nobody else uses it.
export const RELEASE_SCOPE = 'release';

// A first line's parts, or null when it isn't conventional. The scope is undefined without parentheses and '' with
// empty ones; bang is whether a `!` marks it breaking.
export function parseHeader(subject) {
	const m = HEADER.exec(subject);
	if (!m) return null;
	const [, type, scope, bang, description] = m;
	return { type, scope, bang: Boolean(bang), description };
}

// ---- the body: a breaking change says what users must do, on a line of its own
export const BREAKING_FOOTER = /^BREAKING[ -]CHANGE: (.+)$/m;
