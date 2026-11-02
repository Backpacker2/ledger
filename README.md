# Ledger

A local desktop app for thinking in **threads**, drafting **documents** from them, and planning the work on a **planboard**. Tauri 2 (Rust) on the back, SvelteKit (Svelte 5) on the front. Your data is a **vault** — a folder of markdown files with YAML frontmatter, like an Obsidian vault — that can be mirrored to a private GitHub repository.

This is the rewrite of the original ASP.NET ledger, which lives on the `legacy` branch. The data model carries over, and so does the idea of plain files synced to GitHub; the shell does not. The look follows the Leerjournaal app: a dark sidebar, calm cards, one accent colour, six themes.

## How it keeps your data

```
   the vault (files)  ──load at start──▶  in-memory database  ◀── every read
         ▲                                       │
         └────────── every change, written through ┘
         │
         └── GitHub (private repo): pulled at start, pushed after changes, checked every few minutes
```

- **The vault is the source of truth.** Open it in Obsidian, grep it, put it in git yourself. Edits made outside Ledger are picked up when its window regains focus.
- **At startup** the whole vault is read into an in-memory database. Screens are answered from memory; every change is written straight back to the files (only the files that actually changed, and atomically).
- **GitHub is the store you sync to.** Ledger talks to GitHub's REST API directly, so nothing else needs installing. Each round is a three-way comparison per file against what both sides last agreed on, so changes made on two machines merge file by file. If the same file was edited on both, this machine's version wins and the other is kept in `.ledger/conflicts/`; an edit always beats a delete. Ledger refuses to sync to a public repository.

### The vault

```
threads/<thread>/thread.md          title, createdAt, linkedThreadSlugs
threads/<thread>/posts/<id>.md      one post: frontmatter (parent, flags, timestamps) + markdown body
documents/<id>.md                   the live draft of a document
documents/<id>@v1.0.md              a published version, frozen
document-types/<type>.md            name, trigger ("use when…"), questions
user-stories/<id>.md                the story, its acceptance criteria and linked threads
planboard.md                        the columns
.gitignore                          lists .env and .ledger/ (kept out of GitHub)
.env                                GitHub settings — local only
.ledger/                            sync bookkeeping — local only
```

Files Ledger doesn't know about are left alone, so the vault can hold other notes. Hand-written files are read leniently: a folder of posts without a `thread.md` is a thread, a post without frontmatter is all body, links to things that don't exist are dropped, and the repairs are written back.

### Choosing where the vault lives, and syncing it

**Settings → Vault** shows the folder and lets you pick another (optionally copying the current vault into it when it's empty). The location itself is the one thing kept outside the vault, in the app's config folder.

**Settings → Sync to GitHub** takes a repository (`owner/name` or a github.com URL), a branch (created on the first sync if it doesn't exist) and a repository-scoped access token. They're stored in the vault's `.env`:

```
GITHUB_REPO=owner/notes
GITHUB_BRANCH=main
GITHUB_TOKEN=github_pat_…
```

That file sits next to your notes but is gitignored — and never uploaded even if the `.gitignore` is edited — so the token stays on this machine. The app reads it at startup to see whether sync is set up; if all three are filled in it pulls straight away. The token never reaches the UI (only that one is saved, and its last four characters).

Use a [fine-grained token](https://github.com/settings/personal-access-tokens/new) limited to that one repository with **Contents: Read and write**. The repository must already have a commit (a README will do).

## What it does

- **Threads** — one per topic you're juggling. Markdown posts, replies that quote their parent, edit and delete. Threads can be linked to each other, in both directions.
- **Document types** — a name, a "use when…" hint and a list of guiding questions.
- **Documents** — drafted in a markdown editor with live preview and autosave. Starting a document in a thread seeds the type's guiding questions into it as posts (a one-time snapshot); replies to a question are shown inside its card. **Publish** a version (minor, major or a custom `M.m`) to freeze an immutable snapshot, which also drops a note in the thread that links back to it. A document can be created unlinked and given a thread later with *Start drafting*.
- **User stories** — an id you choose (`MM01`), a summary, "As a / I want to / Such that", and acceptance criteria you tick off. A story links to any number of threads.
- **Planboard** — one board with customisable columns (Backlog → Drafting → Documented by default). Drag a card, or use its arrows, to change column. A column that still holds stories can't be deleted.
- **Screenshots** — press a global hotkey (default Ctrl/⌘+Shift+S) anywhere: the screen under the cursor is captured, a full-screen overlay lets you drag out a region, and the cropped PNG is uploaded to an endpoint you configure, then opened in Ledger. See below.
- **Search** across every post and document, with highlighted snippets. **Themes**: Dark, Night, Forest, Ocean, Sepia, Light.

Deleting a thread keeps its documents (unlinked) and its stories (just unlinked from it); deleting a document keeps its published versions readable; a document type can't be deleted while documents use it.

## Screenshots

**Settings → Screenshots** holds the hotkey and the upload URL. Both are per-machine (stored in the app's config folder, not in the vault, so they aren't synced). The hotkey is set with a key recorder, checked (it needs a modifier, and the system's own shortcuts are refused) and registered immediately, replacing the old one without a restart; if the system won't register it (usually because another program owns it) you get a warning and the old shortcut keeps working.

The flow: the hotkey captures the monitor under the cursor ([xcap](https://crates.io/crates/xcap)); a frameless, always-on-top window shows the capture so you can drag out a region (Esc or a right-click cancels); the backend crops it — scaling the drag from window units to capture pixels, so HiDPI screens come out exact — encodes a PNG and POSTs it as a multipart form (field `file`) to the upload URL. The answer should be JSON with the image's link in `url` (`link` and `data.url` also work), or just the link. Ledger then emits `screenshot-ready`, brings its main window forward and opens the screenshot. Failures (no URL set, the endpoint down, a missing permission) are shown the same way.

Platforms:

- **Windows, Linux (X11)** — as above.
- **macOS** needs the Screen Recording permission. Ledger checks it before capturing, triggers the system prompt the first time, and otherwise says where to switch it on (System Settings → Privacy & Security → Screen & System Audio Recording; Ledger must be restarted afterwards). A transparent overlay window needs Tauri's `macos-private-api`, which isn't enabled; the overlay is opaque (it shows the capture anyway).
- **Linux under Wayland** can't capture the screen or grab a key directly, so Ledger asks the XDG desktop portals ([ashpd](https://crates.io/crates/ashpd)): the compositor's own area picker replaces the overlay, and the hotkey is bound through the global-shortcuts portal. If the desktop doesn't offer that portal, Settings says so and the *Take a screenshot now* button still works.

## Running it

You need [Node.js](https://nodejs.org), a Rust toolchain and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (on Windows: the MSVC build tools and WebView2).

```bash
npm install
npm run tauri dev      # the app, with hot reload
npm run tauri build    # an installer / bundle
```

Checks:

```bash
npm test               # Rust: stores, vault round-trips, and the sync algorithm against a fake GitHub
npm run check          # Svelte + TypeScript
```

Environment variables, mostly for development: `LEDGER_VAULT` fixes the vault location (and locks it in Settings); `LEDGER_CONFIG_DIR` moves the per-machine settings (vault location, screenshot hotkey and URL); `LEDGER_GITHUB_API` points the sync at another API base (GitHub Enterprise, or a mock); `LEDGER_SCREENSHOT_URL` fixes the screenshot upload URL (and locks it in Settings).

## Layout

```
src/                     SvelteKit frontend (static SPA)
  routes/                threads/[slug], documents, document-types, planboard, search, settings
  lib/api.ts             typed wrappers over the Tauri commands
  lib/ui.svelte.ts       toasts, dialogs, theme, and the backend's sync/vault events
  lib/components/        sidebar, post card, composer, story modal, dialogs, hotkey recorder …
  routes/screenshot-overlay  the full-screen region picker (its own window)
src-tauri/src/
  schema.sql             the in-memory database's shape and rules
  threads.rs posts.rs documents.rs document_types.rs stories.rs planboard.rs search.rs
                         one store per concern, plain functions over the in-memory connection
  vault.rs frontmatter.rs   files ⇄ database: layout, lenient import, canonical export, write-through
  settings.rs            vault location, the vault's .env, .gitignore
  sync.rs                the three-way reconcile, behind a `Remote` trait
  github.rs              the GitHub REST implementation of `Remote`
  syncer.rs              the background worker: debounce, poll, retry, status events
  screenshot.rs          hotkey registration, capture, the overlay window, crop, upload; Wayland portals
  state.rs commands.rs   the running app and the thin Tauri command layer
  markdown.rs            markdown → HTML; raw HTML is escaped and unsafe link targets dropped
  tests*.rs              behaviour tests
```

## Not carried over from the legacy app

The read-only MCP server, PDF export, and the backfill command for old version notes. The legacy content repository used `meta.json` and `*.json` files for threads, stories, types and the planboard; Ledger reads and writes markdown with frontmatter for all of them, so a legacy vault would need converting first.
