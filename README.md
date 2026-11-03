# Ledger

A local desktop app for thinking in **threads**, asking for **deliverables** and giving feedback on them, and planning the work on a **board** — all inside **projects**. Tauri 2 (Rust) on the back, SvelteKit (Svelte 5) on the front. Your data is a **vault** — a folder of markdown files with YAML frontmatter, like an Obsidian vault — that can be mirrored to a private GitHub repository.

This is the rewrite of the original ASP.NET ledger, which is kept in the author's private repository (MilanMolenbrugge/ledger, commit `3ec6769`). The data model carries over in spirit, and so does the idea of plain files synced to GitHub; the shell does not. The look follows the Leerjournaal app: a dark sidebar, calm cards, one accent colour, six themes.

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
projects/<project>/threads/<thread>/runs/<id>.md        a run of a prompt with Claude Code: how it ended, what it did
projects/<project>/userstories/<id>.md              a user story
projects/<project>/tasks/<id>.md                    a task (a card on the board)
projects/<project>/deliverables/                    what prompts produce — any kind of file;
                                                    published versions are tagged name@v0.1.ext
projects/<project>/versions/<id>.md                 how a published version came to be: who made it, the thread, prompt
                                                    and run behind it, its AI-use label
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
- **Deliverables** — files in the project's `deliverables/`. Markdown ones are edited in the app, autosaved, in source mode (the raw markdown), live preview or reading mode, as in Obsidian; any other kind opens with the system's program. **Publish** a version (minor, major or a custom `M.m`) to freeze an immutable copy named `name@v0.1.ext`, which also drops a note in the thread of the prompt that produced it. Each deliverable has a **Produced by** window beside it (see below) and lists the feedback on it.
- **Produced by, and AI use** — every published version records who made the change: you (you typed it), Claude Code (a run started from the app, which also links the version to its prompt), or AI you say you used elsewhere. The **Produced by** window on the right of a deliverable lists its threads and under each thread what happened there, in the order set under Settings → Order (newest first unless you change it): the prompt that asked for the file, the run, each version with who made it, its size and a SHA-256, and the feedback on it. The "version published" note in a thread links to the prompt that caused the version, and quotes the changelog you wrote for it ("from changelog: …"), if you wrote one. When you publish, a checkbox says the change was made with generative AI (it starts ticked when a Claude Code run changed the file and nothing published it), and you can attach a label from the [Me & My Machine](https://mmmlabel.tech) scheme — Lazy Prompter, Piggybacker, Cyborg, Handyman or Man-Made. Ledger records the facts; the label is yours to declare, per version, because it describes the product as it stands then. Versions published before this existed show what the runs and notes prove (versions a run published are known to be AI) and otherwise "origin not recorded"; **Details** has a button to mark them. The records are plain files, so they can be edited by hand.
- **Feedback** — written in a thread, landing on a deliverable or on one of its published versions; open or resolved.
- **Uploads** — pick files in a thread (they are copied into the project's `attachments/` and noted in an `uploads/<id>.md` manifest with a description). Screenshots land the same way.
- **Templates** — a name, a "use when…" hint, guiding questions and the skeleton of the deliverable. Global ones (sidebar → Templates) serve every project; a project can *customise* one, which copies it into the project, where it replaces the shared one.
- **User stories** — an id you choose (`MM01`), a summary, "As a / I want to / Such that", notes and acceptance criteria you tick off. A story links to any number of threads.
- **Board** — per project, with customisable columns (Backlog → Doing → Done by default). Tasks can point at a story and a thread. Drag a card, or use its arrows, to change column; a column that still holds tasks can't be deleted.
- **Screenshots** — press a global hotkey (default Ctrl/⌘+Shift+S) anywhere: your own screenshot tool (Flameshot, say) or the built-in capture takes the picture, and Ledger offers to attach it to the thread you have open. Pictures sent to Ledger with "Open with" land the same way. See below.
- **Writing** — every markdown field (notes, prompts, feedback, descriptions, deliverables, templates, the reading guide) is the same inline editor ([CodeMirror 6](https://codemirror.net)). It edits the markdown itself, so the file gets exactly what you typed, but reads like the rendered page: headings, bold, code and links are shown as such and their marks fade away except where the cursor is, tables are drawn as tables until you click into one, and pictures are shown in place. Task boxes can be ticked, Ctrl/⌘+click opens a link, and Ctrl/⌘+B, I and K make bold, italic and a link.
- **Dashboard** (Home) — a card per project, the most recently active first, with its three latest threads and two latest deliverables, and links to its board and a new thread. **Pin** a project (the pin on its card) to keep it at the top. Above them, a contribution graph like GitHub's: a square per day for the last month, the last twelve months or a chosen year, shaded by how much you made that day, for all projects or one, with your active days and streaks; switch it off under Settings → Dashboard. It counts what you make: threads, your posts, prompts, feedback, uploads, published versions, Claude Code runs, stories and tasks; not edits, which leave no history, nor what the app writes for you. Pins and that setting are kept per machine, like the theme.
- **Search** across every post, prompt, feedback and deliverable, with highlighted snippets. **Themes**: Dark, Night, Forest, Ocean, Sepia, Light. **Order** (Settings): one choice, newest first by default, for the posts of a thread, the threads in the sidebar, and the feedback, versions and "Produced by" window of a deliverable. It only changes how things are shown. **Run prompts by themselves** (Settings → Claude Code, off by default): a prompt you create starts running in Claude Code at once, without the confirmation of what will be sent.

## Screenshots

**Settings → Screenshots** holds the hotkey and the tool. Both are stored per vault in `.ledger/settings.json`, so they are never synced, and each machine can use its own tool. The hotkey is set with a key recorder, checked (it needs a modifier, and the system's own shortcuts are refused) and registered immediately, replacing the old one without a restart; if the system won't register it (usually because another program owns it) you get a warning and the old shortcut keeps working.

When a picture arrives, Ledger holds the PNG in memory, emits `screenshot-ready`, brings its main window forward and asks where it goes. With a thread open that is a short confirmation (that thread, an optional description, Enter); *Change…* or no open thread gives the full choice of project and thread (or a new one). *Attach* saves the PNG in the project's `attachments/`, writes the upload manifest and opens the thread at it; *Discard* drops it. Failures are shown the same way.

### Your own tool (bring your own)

Choose **My own tool** and give a command. Ledger runs it when the hotkey is pressed; it is split into a program and its arguments by Ledger itself (spaces separate, quotes group, backslashes are kept), so it means the same everywhere and needs no shell.

- With `{file}` in it, Ledger fills in a fresh temporary `.png` path, waits for the tool to exit and reads the file.
- Without `{file}`, Ledger reads the PNG the tool prints.
- No picture means cancelled, unless the tool printed an error (anything but an "aborted"/"cancelled" message), which is shown.

Presets, offered in Settings with whether the program was found:

| | Command |
|---|---|
| Linux, Flameshot | `flameshot gui --raw` |
| macOS, built-in picker | `screencapture -i {file}` |
| macOS, Flameshot | `/Applications/flameshot.app/Contents/MacOS/flameshot gui -p {file}` |
| Windows, Flameshot | `"C:\Program Files\Flameshot\bin\flameshot.exe" gui -p {file}` |

### Opened with Ledger

A PNG passed to Ledger on the command line becomes the screenshot waiting to be attached; a second launch is handed to the running app by [tauri-plugin-single-instance](https://crates.io/crates/tauri-plugin-single-instance), and on macOS opened files arrive as `RunEvent::Opened`. That is what Flameshot's *Open with* (Ctrl+O in its editor) uses.

- **Linux**: the .deb and .rpm install a desktop entry with `Exec=ledger %F` and `MimeType=image/png` (`src-tauri/linux/ledger.desktop`, set in `tauri.linux.conf.json`), so Ledger is listed. With `tauri dev` there is no desktop entry; to try it, put one in `~/.local/share/applications` whose `Exec` is `src-tauri/target/debug/ledger %F`.
- **macOS**: `tauri.macos.conf.json` declares PNG with rank *Alternate*: Ledger appears under Finder's *Open With* but never becomes the default viewer.
- **Windows**: no file association, because Tauri's installer would make Ledger the default PNG program. Pick `Ledger.exe` through *Open with → Look for another app on this PC*.

### The built-in capture

Used when no tool is chosen. It is meant to be removable (the list of what to delete is at `builtin` in `src-tauri/src/screenshot.rs`).

- **Windows, macOS**: the monitor under the cursor is captured ([xcap](https://crates.io/crates/xcap)); a frameless, always-on-top window shows it so you can drag out a region (Esc or a right-click cancels); the backend crops it, scaling the drag from window units to capture pixels so HiDPI screens come out exact. macOS needs the Screen Recording permission: Ledger checks it before capturing, triggers the system prompt the first time, and otherwise says where to switch it on (Ledger must be restarted afterwards). The overlay is opaque on macOS (a transparent window needs Tauri's `macos-private-api`).
- **Linux**: the desktop's screenshot portal ([ashpd](https://crates.io/crates/ashpd)), with the compositor's own area picker. xcap isn't built on Linux: it pulls in PipeWire, which needs `libpipewire-0.3-dev` and libclang just to compile.

Under Wayland the hotkey is bound through the global-shortcuts portal; if the desktop doesn't offer it, Settings says so and the *Take a screenshot now* button still works.

The built-in capture on Windows is exercised end to end. The macOS paths, and the Flameshot presets on Windows and macOS, have been type-checked but not run.

## Running it

You need [Node.js](https://nodejs.org) 26 (the version in `.nvmrc`; `nvm use` picks it) with [pnpm](https://pnpm.io) (`corepack enable` activates the version pinned in `package.json`), a Rust toolchain and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (on Windows: the MSVC build tools and WebView2).

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

## Versions and releases

Every push to `main` that brings [conventional commits](https://www.conventionalcommits.org) is released by CI
([.github/workflows/release.yml](.github/workflows/release.yml)). The commits since the last release decide the bump,
the largest one wins:

| Commit | Bump | Example |
|---|---|---|
| a breaking change: `feat!:`, `fix(sync)!:`, or a `BREAKING CHANGE:` footer | major | 0.4.2 → 1.0.0 |
| `feat:` | minor | 0.4.2 → 0.5.0 |
| any other type: `fix`, `perf`, `refactor`, `docs`, `style`, `test`, `build`, `ci`, `chore`, `revert` | patch | 0.4.2 → 0.4.3 |

Commits that aren't conventional are left out; with none that count, there is no release. The version is written to
`package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `Cargo.lock` (the app shows the Cargo one), the
notes go to [CHANGELOG.md](CHANGELOG.md), and CI commits that as `chore(release): vX.Y.Z`, tags it `vX.Y.Z` and
publishes a GitHub release. Don't bump versions by hand.

`main` only takes pull requests (the `protected-main` ruleset), so CI pushes that release commit and tag with a deploy
key the ruleset lets through: the write key whose private half is the Actions secret `RELEASE_DEPLOY_KEY`.

```bash
pnpm release:preview   # what the next release would be, and its notes; changes nothing
```

## Layout

```
src/                     SvelteKit frontend (static SPA)
  routes/                / · projects/[project] (overview) with threads/[thread], deliverables(/view),
                         stories, board, templates · templates · search · settings
  lib/api.ts             typed wrappers over the Tauri commands
  lib/ui.svelte.ts       toasts, dialogs, theme, and the backend's sync/vault/screenshot events
  lib/markdownEditor.ts  the markdown editor (CodeMirror 6): looks, live preview of the marks, tables, source mode, keys
  lib/vaultImages.ts     pictures in markdown: read from the vault, beside the file that shows them
  lib/components/        sidebar tree, post/prompt/feedback/upload cards, composer, modals, hotkey recorder …
  routes/screenshot-overlay  the built-in capture's region picker (its own window; not used on Linux)
scripts/release.mjs      the next version from the conventional commits (run by CI)
src-tauri/src/
  schema.sql             the in-memory database's shape and rules
  projects.rs threads.rs posts.rs prompts.rs feedback.rs uploads.rs deliverables.rs templates.rs
  provenance.rs          how published versions came to be: the records, what old versions prove, the "Produced by" window
  stories.rs board.rs search.rs
                         one store per concern, plain functions over the in-memory connection
  vault.rs frontmatter.rs   files ⇄ database: layout, lenient import, canonical export, write-through
  settings.rs            vault location, .ledger/settings.json and .env, .gitignore
  sync.rs                the three-way reconcile, behind a `Remote` trait
  github.rs              the GitHub REST implementation of `Remote`
  syncer.rs              the background worker: debounce, poll, retry, status events
  screenshot.rs          hotkey, your own tool, pictures opened with Ledger, the built-in capture; Linux portals
  state.rs commands.rs   the running app and the thin Tauri command layer
  markdown.rs            markdown → HTML; raw HTML is escaped and unsafe link targets dropped
  tests*.rs              behaviour tests
```

## Not carried over from the legacy app

The read-only MCP server, PDF export, and the backfill command for old version notes. There is no migration: the legacy content repository used `meta.json` and `*.json` files and a different structure, and Ledger starts from an empty vault.
