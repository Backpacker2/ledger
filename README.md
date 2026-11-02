# Ledger

A local desktop app for thinking in **threads**, drafting **documents** from them, and planning the work on a **planboard**. Tauri 2 (Rust) on the back, SvelteKit (Svelte 5) on the front, one SQLite file for the data.

This is the rewrite of the original ASP.NET ledger, which lives on the `legacy` branch. The data model carries over; the storage and the shell do not. The look follows the Leerjournaal app: a dark sidebar, calm cards, one accent colour, six themes.

## What it does

- **Threads** — one per topic you're juggling. Write markdown posts, reply to any post (replies quote their parent, Teams-style), edit and delete. Threads can be linked to each other, in both directions.
- **Document types** — a name, a "use when…" hint and a list of guiding questions. They start out empty; you define them.
- **Documents** — drafted in a markdown editor with live preview and autosave. Starting a document in a thread seeds the type's guiding questions into it as posts (a one-time snapshot); replies to a question are shown inside its card. **Publish** a version (minor, major or a custom `M.m`) to freeze an immutable snapshot, which also drops a "version published" note in the thread that links back to it. A document can be created unlinked and given a thread later with *Start drafting*.
- **User stories** — an id you choose (`MM01`), a summary, "As a / I want to / Such that", and acceptance criteria you tick off. A story links to any number of threads, and a thread's documents belong to its stories through that link.
- **Planboard** — one board with customisable columns (Backlog → Drafting → Documented by default). Every story is a card: drag it, or use its arrows, to change column. A column that still holds stories can't be deleted.
- **Search** across every post and document, with highlighted snippets.
- **Themes** — Dark, Night, Forest, Ocean, Sepia, Light.

Deleting a thread keeps its documents (unlinked) and its stories (just unlinked from it); deleting a document keeps its published versions readable; a document type can't be deleted while documents use it.

## Data model

```
threads ──< posts                      parent_id → another post (replies; no FK, so a deleted parent leaves its replies)
threads >──< threads                   thread_links, stored once per pair
document_types ──< document_type_questions
document_types ──< documents >── threads (nullable)
documents ──< document_versions        immutable snapshots; outlive the draft
user_stories ──< acceptance_criteria
user_stories >──< threads              story_threads
planboard_columns ──< user_stories
```

The schema is in [`src-tauri/src/schema.sql`](src-tauri/src/schema.sql); `PRAGMA user_version` tracks migrations.

## Running it

You need [Node.js](https://nodejs.org), a Rust toolchain and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (on Windows: the MSVC build tools and WebView2).

```bash
npm install
npm run tauri dev      # the app, with hot reload
npm run tauri build    # an installer / bundle
```

Tests and checks:

```bash
npm test               # Rust: the behaviour of every store, against an in-memory database
npm run check          # Svelte + TypeScript
```

## Your data

Everything lives in one SQLite file, `ledger.db`, in the app's data directory (`%APPDATA%\nl.molenbrugge.ledger` on Windows; *Settings* shows the exact path). Set `LEDGER_DATA_DIR` to keep it somewhere else, for example a synced folder or a scratch directory while developing. Back it up by copying the file while the app is closed.

## Layout

```
src/                     SvelteKit frontend (static SPA)
  routes/                threads/[slug], documents, document-types, planboard, search, settings
  lib/api.ts             typed wrappers over the Tauri commands
  lib/components/        sidebar, post card, composer, story modal, dialogs …
  app.css                the theme tokens and every component style
src-tauri/src/
  schema.sql             the database
  threads.rs posts.rs documents.rs document_types.rs stories.rs planboard.rs search.rs
                         one store per concern, plain functions over a rusqlite connection
  markdown.rs            markdown → HTML; raw HTML is escaped and unsafe link targets dropped
  commands.rs            the thin Tauri command layer
  tests.rs               behaviour tests
```

## Not carried over from the legacy app

Git sync of the content to a GitHub branch, the read-only MCP server, PDF export and the backfill command for old version notes. The data is a single file now, so the first is mostly a backup question; the others can come back as commands if they're missed.
