-- Ledger schema, version 1.
--
-- Threads hold posts (markdown notes, optionally replies); documents are drafted from the thread's
-- answers to a document type's guiding questions; user stories live on the planboard and point at the
-- threads that serve them.

CREATE TABLE threads (
    slug       TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Links between threads are bidirectional: each pair is stored once, smaller slug first.
CREATE TABLE thread_links (
    a TEXT NOT NULL REFERENCES threads(slug) ON DELETE CASCADE,
    b TEXT NOT NULL REFERENCES threads(slug) ON DELETE CASCADE,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);

CREATE TABLE posts (
    id                TEXT PRIMARY KEY,
    thread_slug       TEXT NOT NULL REFERENCES threads(slug) ON DELETE CASCADE,
    -- Deliberately not a foreign key: deleting a post leaves its replies in place.
    parent_id         TEXT,
    guiding_question  INTEGER NOT NULL DEFAULT 0,
    version_published INTEGER NOT NULL DEFAULT 0,
    document_id       TEXT,
    document_version  TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    body              TEXT NOT NULL
);
CREATE INDEX posts_by_thread ON posts (thread_slug, created_at);

CREATE TABLE document_types (
    slug       TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    "trigger"  TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE document_type_questions (
    type_slug TEXT NOT NULL REFERENCES document_types(slug) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    question  TEXT NOT NULL,
    PRIMARY KEY (type_slug, position)
);

CREATE TABLE documents (
    id                 TEXT PRIMARY KEY,
    document_type_slug TEXT NOT NULL REFERENCES document_types(slug),
    title              TEXT NOT NULL,
    -- Deleting a thread keeps its documents, just unlinked.
    thread_slug        TEXT REFERENCES threads(slug) ON DELETE SET NULL,
    version            TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    body               TEXT NOT NULL DEFAULT ''
);
CREATE INDEX documents_by_thread ON documents (thread_slug);

-- Snapshots of published versions. Not a foreign key: a published version outlives its draft.
CREATE TABLE document_versions (
    document_id        TEXT NOT NULL,
    version            TEXT NOT NULL,
    major              INTEGER NOT NULL,
    minor              INTEGER NOT NULL,
    document_type_slug TEXT NOT NULL,
    title              TEXT NOT NULL,
    body               TEXT NOT NULL,
    published_at       TEXT NOT NULL,
    PRIMARY KEY (document_id, version)
);

CREATE TABLE planboard_columns (
    id       TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    position INTEGER NOT NULL
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The id is chosen by the user (e.g. MM01) and unique ignoring case.
CREATE TABLE user_stories (
    id         TEXT PRIMARY KEY COLLATE NOCASE,
    summary    TEXT NOT NULL,
    role       TEXT NOT NULL DEFAULT '',
    want       TEXT NOT NULL DEFAULT '',
    reason     TEXT NOT NULL DEFAULT '',
    column_id  TEXT NOT NULL REFERENCES planboard_columns(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE acceptance_criteria (
    story_id TEXT NOT NULL REFERENCES user_stories(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    text     TEXT NOT NULL,
    done     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (story_id, position)
);

-- Many-to-many: a thread can serve several stories and a story can span several threads.
CREATE TABLE story_threads (
    story_id    TEXT NOT NULL REFERENCES user_stories(id) ON DELETE CASCADE,
    thread_slug TEXT NOT NULL REFERENCES threads(slug) ON DELETE CASCADE,
    PRIMARY KEY (story_id, thread_slug)
);

INSERT INTO planboard_columns (id, name, position) VALUES
    ('backlog', 'Backlog', 0),
    ('drafting', 'Drafting', 1),
    ('documented', 'Documented', 2);

INSERT INTO settings (key, value) VALUES ('backlog_column_id', 'backlog');
