# Working in this repo

Ledger is a Tauri 2 desktop app: Rust in `src-tauri/`, SvelteKit (Svelte 5) in `src/`. The README explains the data
model, the vault and the layout; read it before larger changes.

## Commits: conventional, because they make the releases

Every commit on `main` must be a [conventional commit](https://www.conventionalcommits.org). CI reads them to decide
the next version, so the type you pick changes the version that ships:

```
<type>(<optional scope>): <what changes, imperative, lowercase>

<optional body: why, and anything a reviewer can't see in the diff>

<optional footers, e.g. BREAKING CHANGE: …>
```

| Type | Use it for | Version bump |
|---|---|---|
| `feat` | something a user can now do or see | minor (0.4.2 → 0.5.0) |
| `fix` | a bug fixed | patch (0.4.2 → 0.4.3) |
| `perf`, `refactor`, `style`, `docs`, `test`, `build`, `ci`, `chore`, `revert` | everything else | patch |
| any type with `!` (`feat!:`, `fix(sync)!:`) or a `BREAKING CHANGE:` footer | a change that breaks existing vaults, settings or files | major (0.4.2 → 1.0.0) |

- Only those types count. A message like `Update stuff` or `feet: …` is not conventional: CI skips it and it ships
  no version, silently.
- Scopes are optional and name the area: `threads`, `board`, `sync`, `vault`, `screenshots`, `ui`, `settings`, … .
- Mark something breaking only when users must act (their vault, settings or synced files no longer work as before).
  When it is, say in the footer what they must do.
- One concern per commit; don't mix a feature and an unrelated fix, they bump and read differently.

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
- The logo and wordmark files are in `src/lib/assets/brand/`; the app icons in `src-tauri/icons/` are generated from
  the mark. Don't redraw them in code.
