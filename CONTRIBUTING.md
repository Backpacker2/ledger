# Contributing to Ledger

Ledger is young and moves fast. Changes come in as pull requests, and every merge into `main` is released straight
away. This page is what that takes. The [README](README.md) explains the data model, the vault and the layout.

## Setting up

You need:

- [Node.js](https://nodejs.org), the version in `.nvmrc` (`nvm use` picks it);
- [pnpm](https://pnpm.io), the version pinned under `packageManager` in `package.json` (`corepack enable` activates it);
- stable Rust (`rustup default stable`);
- the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (on Windows: the MSVC build
  tools and WebView2).

```bash
pnpm install
pnpm run tauri dev   # the app, with hot reload
```

## Before you push

```bash
pnpm run check   # Svelte + TypeScript
pnpm test        # Rust tests
```

CI runs both on every pull request, with the commits check below, and a pull request is merged only when they pass.
On your first pull request here, the checks wait until a maintainer lets them run.

## Commits

Pull requests are merged with a merge commit, so every commit on your branch lands on `main` as it is, and CI reads
each one to decide the next version and write the release notes. So every commit is a
[conventional commit](https://www.conventionalcommits.org), with a type and a scope from the tables below:

```
<type>(<scope>): <what changes, imperative, lowercase>

<optional body: why, and anything a reviewer can't see in the diff>

<optional footers, e.g. BREAKING CHANGE: …>
```

- One concern per commit. Don't mix a feature and an unrelated fix: they bump and read differently. A pull request
  can hold several commits.
- Merge commits don't count: neither GitHub's own "Merge pull request #…" nor a merge of `main` into your branch.
- The pull request's title and description are yours to write as you like; they don't go into the release.

A check called `Commits` tests every commit on the branch and names each one that needs a new message. Try it before
you push:

```bash
# the commits on your branch that main doesn't have yet
node scripts/check-commits.mjs
# a message, before you commit it
node scripts/check-commits.mjs --message "feat(board): drag cards between columns"
```

To fix a message, change the commit and push again:

- the last commit: `git commit --amend`;
- an earlier one: `git rebase -i origin/main`, change `pick` to `reword` on its line, and save;
- then `git push --force-with-lease`, which refuses if someone else pushed to the branch in the meantime.

### Types

| Type | Use it for | Bump |
|---|---|---|
| `feat` | something a user can newly do or see | minor (0.4.2 → 0.5.0) |
| `fix` | a bug a user could hit | patch (0.4.2 → 0.4.3) |
| `perf` | faster or lighter, same behaviour | patch |
| `refactor` | code restructured, no visible change | patch |
| `style` | look-only UI changes: spacing, colours, icons (in this repo, not code formatting) | patch |
| `docs` | README, CONTRIBUTING.md, CLAUDE.md, comments | patch |
| `test` | tests added or fixed, no app change | patch |
| `build` | dependencies, Node/Rust versions, Tauri config, icons | patch |
| `ci` | GitHub workflows, the release script and the commits check | patch |
| `chore` | anything that fits none of the above; keep it rare | patch |
| `revert` | undoing an earlier commit | patch |
| any type with `!` (`feat!:`, `fix(sync)!:`) or a `BREAKING CHANGE:` footer | existing vaults, settings or synced files stop working as before | major (0.4.2 → 1.0.0) |

- Only these types count. A commit like `Update stuff` or `feet: …` is not conventional: the release would skip it
  and ship no version, silently. The check fails it.
- Mark something breaking only when users must act (see below).

### Scopes

Give every commit a scope from this list when the change sits in one area: the release notes put it in front of every
line, so it's how a reader finds what changed where. Leave the scope out only when a change really spans the whole
repo (`build: move to Node 26`).

| Area | Scopes |
|---|---|
| The main features | `projects`, `threads`, `prompts`, `feedback`, `deliverables`, `uploads`, `stories`, `board`, `templates`, `search` |
| Data and sync | `vault`, `sync`, `crypto`, `settings` |
| Integrations | `screenshots`, `trello`, `claude` (runs with Claude Code) |
| Across the app | `ui` (shared components, layout, icons), `brand` (logo, tokens, colours), `logs` (the error log, debug captures, reporting a problem) |
| Reserved | `release`: only CI's own `chore(release)` commits; never use it yourself |

- Name the scope after what the user sees, not the file: a fix in `syncer.rs` is `fix(sync)`.
- One scope per commit. A change that needs two scopes is usually two commits.
- An area not on the list (a new feature): add its scope to this table in the same pull request. The check reads the
  table from your branch, so the new scope passes.

Examples:

```
feat(board): drag cards between columns
fix(sync)!: store the token per vault
style(ui): align the icon buttons
build: move to Node 26
```

## Breaking changes

A change is breaking when existing vaults, settings or synced files stop working as before, so users must do
something. Mark it in the commit that makes it, in two places:

- a `!` after the type or scope in its subject: `fix(sync)!: store the token per vault`;
- a footer in its message: a line that starts with `BREAKING CHANGE:` and says what users must do. The release notes
  quote it under *Breaking changes*.

```
fix(sync)!: store the token per vault

Each vault can now sync to its own repository, with its own token.

BREAKING CHANGE: re-enter your GitHub token in Settings
```

From the command line, each `-m` is a paragraph:
`git commit -m "fix(sync)!: store the token per vault" -m "BREAKING CHANGE: re-enter your GitHub token in Settings"`.

The check fails a `!` without the footer. Either one makes the next release a major version, so use them only when
users must act. The footer counts wherever it starts a line in the body, so don't write one you don't mean.

## What merging does

The pull request is merged into `main` with a merge commit, so its commits land there as they are, and that push is
released by CI ([.github/workflows/release.yml](.github/workflows/release.yml)):
[scripts/release.mjs](scripts/release.mjs) reads the commits since the last release, leaving out merge commits, and
works out the version (the largest bump wins). It writes the version to `package.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`, and adds the notes to `CHANGELOG.md`, one line per commit. CI
commits that as `chore(release): vX.Y.Z`, tags it and publishes a GitHub release.

So never change a version number or `CHANGELOG.md` yourself: a version changed by hand would be overwritten or skew
the next bump.
