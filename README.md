# Ledger

A local desktop app for thinking in **threads**, asking for **deliverables** and giving feedback on them, and planning the work on a **board** — all inside **projects**. Tauri 2 (Rust) on the back, SvelteKit (Svelte 5) on the front. Your data is a **vault** — a folder of markdown files with YAML frontmatter, like an Obsidian vault — that can be mirrored to a private GitHub repository.

This is the rewrite of the original ASP.NET ledger, which lives on the `legacy` branch. The data model carries over in spirit, and so does the idea of plain files synced to GitHub; the shell does not. The look follows the Leerjournaal app: a dark sidebar, calm cards, one accent colour, six themes.

## How it keeps your data

```
   the vault (files)  ──load at start──▶  in-memory database  ◀── every read
         ▲                                       │
         └────────── every change, written through ┘
         │
         └── GitHub (private repo): pulled at start, pushed after changes, checked every few minutes
```

- **The vault is the source of truth.** Open it in Obsidian, grep it, put it in git yourself. Edits made outside Ledger are picked up when its window regains focus, and go to GitHub like any other change.
- **At startup** the whole vault is read into an in-memory database. Screens are answered from memory; every change is written straight back to the files (only the files that actually changed, and atomically).
- **GitHub is the store you sync to.** Ledger talks to GitHub's REST API directly, so nothing else needs installing. Each round is a three-way comparison per file against what both sides last agreed on, so changes made on two machines merge file by file. Binary files (uploads, images) sync too. If the same file was edited on both sides, this machine's version wins and the other is kept in `.ledger/conflicts/`; an edit always beats a delete. Ledger refuses to sync to a public repository.

### The vault

```
projects/<project>/project.md                       a project (name, board columns)
projects/<project>/README.md                        the project's reading guide — where to start
projects/<project>/threads/<thread>/thread.md       a thread
projects/<project>/threads/<thread>/posts/<id>.md       its posts
projects/<project>/threads/<thread>/prompts/<id>.md     its prompts: requests for deliverables
projects/<project>/threads/<thread>/feedback/<id>.md    its feedback, which lands on a deliverable
projects/<project>/threads/<thread>/uploads/<id>.md     upload manifests: when, where the file is, a description
projects/<project>/userstories/<id>.md              a user story
projects/<project>/tasks/<id>.md                    a task (a card on the board)
projects/<project>/deliverables/                    what prompts produce — any kind of file;
                                                    published versions are tagged name@v0.1.ext
projects/<project>/attachments/                     files uploaded to a thread, screenshots included
projects/<project>/templates/<id>.md                templates for this project
templates/<id>.md                                   templates for every project
.ledger/                                            settings, the secret and sync bookkeeping — never synced
.gitignore                                          lists .ledger/ and .env (so the vault can also be a git repo)
```

Files Ledger doesn't know about are left alone, so the vault can hold other notes. Hand-written files are read leniently: a folder of posts without a `thread.md` is a thread, a post without frontmatter is all body, links to things that don't exist are dropped, and the repairs are written back. Files dropped into `deliverables/` by hand (or by an assistant) show up in the app.

### Choosing where the vault lives, and syncing it

**Settings → Vault** shows the folder and lets you pick another (optionally copying the current vault into it when it's empty). The location itself is the one thing kept outside the vault, in the app's config folder.

**Settings → Sync to GitHub** takes a repository (`owner/name` or a github.com URL), a branch (created on the first sync if it doesn't exist) and a repository-scoped access token. The repository and branch go in `.ledger/settings.json`, next to the other per-vault settings (the screenshot hotkey); the token goes in `.ledger/.env`:

```
GITHUB_TOKEN=github_pat_…
```

`.ledger/` is gitignored — and never uploaded even if the `.gitignore` is edited — so neither file leaves this machine. The app reads them at startup to see whether sync is set up; if everything is filled in it pulls straight away. The token never reaches the UI (only that one is saved, and its last four characters).

Use a [fine-grained token](https://github.com/settings/personal-access-tokens/new) limited to that one repository with **Contents: Read and write**. The repository must already have a commit (a README will do).

## What it does

- **Projects** — each has a reading guide (its `README.md`, rendered and editable on the overview), threads, deliverables, user stories, a board and templates. Deleting a project removes its folder.
- **Threads** — one per topic you're juggling. A timeline of markdown posts (replies quote their parent; edit and delete), prompts, feedback and uploads, grouped by day. Threads can be linked to each other, in both directions, and to user stories.
- **Prompts** — a request for a deliverable, written in a thread. Started from a **template**, its guiding questions are added to the thread as posts to answer first (a one-time snapshot), and replies to a question are shown inside its card. A prompt can create a deliverable (starting from the template's skeleton) or link existing ones.
- **Deliverables** — files in the project's `deliverables/`. Markdown ones are edited in the app (write / split / preview, autosave); any other kind opens with the system's program. **Publish** a version (minor, major or a custom `M.m`) to freeze an immutable copy named `name@v0.1.ext`, which also drops a note in the thread of the prompt that produced it. Each deliverable lists the prompts that produced it and the feedback on it.
- **Feedback** — written in a thread, landing on a deliverable or on one of its published versions; open or resolved.
- **Uploads** — pick files in a thread (they are copied into the project's `attachments/` and noted in an `uploads/<id>.md` manifest with a description). Screenshots land the same way.
- **Templates** — a name, a "use when…" hint, guiding questions and the skeleton of the deliverable. Global ones (sidebar → Templates) serve every project; a project can *customise* one, which copies it into the project, where it replaces the shared one.
- **User stories** — an id you choose (`MM01`), a summary, "As a / I want to / Such that", notes and acceptance criteria you tick off. A story links to any number of threads.
- **Board** — per project, with customisable columns (Backlog → Doing → Done by default). Tasks can point at a story and a thread. Drag a card, or use its arrows, to change column; a column that still holds tasks can't be deleted.
- **Screenshots** — press a global hotkey (default Ctrl/⌘+Shift+S) anywhere: the screen under the cursor is captured, a full-screen overlay lets you drag out a region, and Ledger asks which project and thread the picture belongs to. See below.
- **Search** across every post, prompt, feedback and deliverable, with highlighted snippets. **Themes**: Dark, Night, Forest, Ocean, Sepia, Light.

## Screenshots

**Settings → Screenshots** holds the hotkey. It is stored per vault in `.ledger/settings.json`, so it is never synced. The hotkey is set with a key recorder, checked (it needs a modifier, and the system's own shortcuts are refused) and registered immediately, replacing the old one without a restart; if the system won't register it (usually because another program owns it) you get a warning and the old shortcut keeps working.

The flow: the hotkey captures the monitor under the cursor ([xcap](https://crates.io/crates/xcap)); a frameless, always-on-top window shows the capture so you can drag out a region (Esc or a right-click cancels); the backend crops it — scaling the drag from window units to capture pixels, so HiDPI screens come out exact — and encodes a PNG, which it holds in memory. Ledger then emits `screenshot-ready`, brings its main window forward and asks for a project, a thread (or a new one) and an optional description. *Attach* saves the PNG in the project's `attachments/`, writes the upload manifest and opens the thread at it; *Discard* drops it. Failures (a missing permission, no screen) are shown the same way.

Platforms:

- **Windows, Linux (X11)** — as above.
- **macOS** needs the Screen Recording permission. Ledger checks it before capturing, triggers the system prompt the first time, and otherwise says where to switch it on (System Settings → Privacy & Security → Screen & System Audio Recording; Ledger must be restarted afterwards). A transparent overlay window needs Tauri's `macos-private-api`, which isn't enabled; the overlay is opaque (it shows the capture anyway).
- **Linux under Wayland** can't capture the screen or grab a key directly, so Ledger asks the XDG desktop portals ([ashpd](https://crates.io/crates/ashpd)): the compositor's own area picker replaces the overlay, and the hotkey is bound through the global-shortcuts portal. If the desktop doesn't offer that portal, Settings says so and the *Take a screenshot now* button still works.

The macOS and Linux code has been type-checked but not run; the Windows flow is exercised end to end.

## Running it

You need [Node.js](https://nodejs.org) with [pnpm](https://pnpm.io) (`corepack enable` activates the version pinned in `package.json`), a Rust toolchain and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (on Windows: the MSVC build tools and WebView2).

```bash
pnpm install
pnpm run tauri dev     # the app, with hot reload
pnpm run tauri build   # an installer / bundle
```

Checks:

```bash
pnpm test              # Rust: stores, vault round-trips, and the sync algorithm against a fake GitHub
pnpm run check         # Svelte + TypeScript
```

Environment variables, mostly for development: `LEDGER_VAULT` fixes the vault location (and locks it in Settings); `LEDGER_CONFIG_DIR` moves the per-machine settings (the vault location); `LEDGER_GITHUB_API` points the sync at another API base (GitHub Enterprise, or a mock).

## Layout

```
src/                     SvelteKit frontend (static SPA)
  routes/                / · projects/[project] (overview) with threads/[thread], deliverables(/view),
                         stories, board, templates · templates · search · settings
  lib/api.ts             typed wrappers over the Tauri commands
  lib/ui.svelte.ts       toasts, dialogs, theme, and the backend's sync/vault/screenshot events
  lib/components/        sidebar tree, post/prompt/feedback/upload cards, composer, modals, hotkey recorder …
  routes/screenshot-overlay  the full-screen region picker (its own window)
src-tauri/src/
  schema.sql             the in-memory database's shape and rules
  projects.rs threads.rs posts.rs prompts.rs feedback.rs uploads.rs deliverables.rs templates.rs
  stories.rs board.rs search.rs
                         one store per concern, plain functions over the in-memory connection
  vault.rs frontmatter.rs   files ⇄ database: layout, lenient import, canonical export, write-through
  settings.rs            vault location, .ledger/settings.json and .env, .gitignore
  sync.rs                the three-way reconcile, behind a `Remote` trait
  github.rs              the GitHub REST implementation of `Remote`
  syncer.rs              the background worker: debounce, poll, retry, status events
  screenshot.rs          hotkey registration, capture, the overlay window, crop; Wayland portals
  state.rs commands.rs   the running app and the thin Tauri command layer
  markdown.rs            markdown → HTML; raw HTML is escaped and unsafe link targets dropped
  tests*.rs              behaviour tests
```

## Not carried over from the legacy app

The read-only MCP server, PDF export, and the backfill command for old version notes. There is no migration: the legacy content repository used `meta.json` and `*.json` files and a different structure, and Ledger starts from an empty vault.
