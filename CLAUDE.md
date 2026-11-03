# Working in this repo

Ledger is a Tauri 2 desktop app: Rust in `src-tauri/`, SvelteKit (Svelte 5) in `src/`. The README explains the data
model, the vault and the layout; read it before larger changes.

## Commits: conventional, because they make the releases

Every commit on `main` must be a [conventional commit](https://www.conventionalcommits.org). CI reads them to decide
the next version, so the type you pick changes the version that ships:

```
<type>(<scope>): <what changes, imperative, lowercase>

<optional body: why, and anything a reviewer can't see in the diff>

<optional footers, e.g. BREAKING CHANGE: …>
```

### Types

| Type | Use it for | Bump |
|---|---|---|
| `feat` | something a user can newly do or see | minor (0.4.2 → 0.5.0) |
| `fix` | a bug a user could hit | patch (0.4.2 → 0.4.3) |
| `perf` | faster or lighter, same behaviour | patch |
| `refactor` | code restructured, no visible change | patch |
| `style` | look-only UI changes: spacing, colours, icons (in this repo, not code formatting) | patch |
| `docs` | README, CLAUDE.md, comments | patch |
| `test` | tests added or fixed, no app change | patch |
| `build` | dependencies, Node/Rust versions, Tauri config, icons | patch |
| `ci` | GitHub workflows, the release script | patch |
| `chore` | anything that fits none of the above; keep it rare | patch |
| `revert` | undoing an earlier commit | patch |
| any type with `!` (`feat!:`, `fix(sync)!:`) or a `BREAKING CHANGE:` footer | existing vaults, settings or synced files stop working as before | major (0.4.2 → 1.0.0) |

- Only these types count. A message like `Update stuff` or `feet: …` is not conventional: CI skips it and it ships
  no version, silently.
- Mark something breaking only when users must act. When it is, say in the footer what they must do.
- One concern per commit; don't mix a feature and an unrelated fix, they bump and read differently.

### Scopes

Give every commit a scope from this list when the change sits in one area: the release notes put it in front of every
line, so it's how a reader finds what changed where. Leave the scope out only when a change really spans the whole
repo (`build: move to Node 26`).

| Area | Scopes |
|---|---|
| The main features | `projects`, `threads`, `prompts`, `feedback`, `deliverables`, `uploads`, `stories`, `board`, `templates`, `search` |
| Data and sync | `vault`, `sync`, `crypto`, `settings` |
| Integrations | `screenshots`, `trello`, `claude` (runs with Claude Code) |
| Across the app | `ui` (shared components, layout, icons), `brand` (logo, tokens, colours), `updates` (installing new releases from inside the app) |
| Reserved | `release`: only CI's own `chore(release)` commits; never use it yourself |

- Name the scope after what the user sees, not the file: a fix in `syncer.rs` is `fix(sync)`.
- One scope per commit. A change that needs two scopes is usually two commits.
- An area not on the list (a new feature): add its scope to this table in the same commit.

Examples:

```
feat(board): drag cards between columns
fix(sync)!: store the token per vault          (footer: BREAKING CHANGE: re-enter your GitHub token in Settings)
style(ui): align the icon buttons
build: move to Node 26
```

## Versions and releases are automatic: don't do them by hand

On every push to `main`, [.github/workflows/release.yml](.github/workflows/release.yml) runs
[scripts/release.mjs](scripts/release.mjs), which:

1. reads the conventional commits since the last `vX.Y.Z` tag and takes the largest bump;
2. writes the new version to `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and
   `src-tauri/Cargo.lock`, and the notes to `CHANGELOG.md`;
3. commits that as `chore(release): vX.Y.Z`, tags it and publishes a GitHub release.

So, as an agent:

- Never edit a version number, `CHANGELOG.md`, or create `v*` tags or `chore(release)` commits; CI owns them.
  A version changed by hand would be overwritten or skew the next bump.
- After pushing, `main` on GitHub gets one more commit (the release). Pull before pushing again.
- To see what your commits will release before pushing: `pnpm release:preview` (changes nothing).

## Before you commit

```bash
pnpm run check   # Svelte + TypeScript
pnpm test        # Rust tests
```

## Look and brand

- Colours and fonts come from the tokens at the top of `src/app.css` (`--brand-*`, the theme tokens, `--font-sans`,
  `--font-mono`). Use a token, never a literal colour, in components and CSS.
- Icons are Lucide: `import X from '@lucide/svelte/icons/x'` (one file per icon, not the package root).
- In the UI the wordmark is `<Wordmark />` (`src/lib/components/Wordmark.svelte`): drawn inline so its L and cursor
  take the theme's `--accent`. The files in `src/lib/assets/brand/` are the fixed-colour (indigo) logo for everything
  outside the app; the app icons in `src-tauri/icons/` are generated from the mark. Don't draw new versions in code.
