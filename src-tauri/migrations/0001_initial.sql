-- Bbrain initial schema. Entities follow DEVELOPMENT.md §7.1.
-- All ids are UUIDv7 strings; all timestamps are UTC ISO 8601 strings.

CREATE TABLE papers (
    id             TEXT PRIMARY KEY,
    sha256         TEXT NOT NULL UNIQUE,
    managed_path   TEXT NOT NULL,
    title          TEXT NOT NULL,
    import_status  TEXT NOT NULL,
    page_count     INTEGER,
    is_favorite    INTEGER NOT NULL DEFAULT 0,
    last_opened_at TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
) STRICT;

CREATE INDEX idx_papers_status ON papers (import_status);
CREATE INDEX idx_papers_created ON papers (created_at DESC);

CREATE TABLE paper_metadata (
    paper_id TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
    authors  TEXT NOT NULL DEFAULT '[]', -- JSON array
    year     INTEGER,
    venue    TEXT,
    doi      TEXT,
    abstract TEXT,
    source   TEXT NOT NULL              -- pdf_metadata | heuristic | ai | user
) STRICT;

CREATE INDEX idx_paper_metadata_doi ON paper_metadata (doi);

CREATE TABLE groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    color      TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE paper_groups (
    paper_id TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    group_id TEXT NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    PRIMARY KEY (paper_id, group_id)
) STRICT;

CREATE TABLE tags (
    id              TEXT PRIMARY KEY,
    normalized_name TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    source          TEXT NOT NULL      -- user | ai
) STRICT;

CREATE TABLE paper_tags (
    paper_id TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    tag_id   TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (paper_id, tag_id)
) STRICT;

CREATE TABLE pages (
    paper_id    TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL,
    width       REAL NOT NULL,
    height      REAL NOT NULL,
    rotation    INTEGER NOT NULL DEFAULT 0,
    text        TEXT NOT NULL,
    text_hash   TEXT NOT NULL,
    PRIMARY KEY (paper_id, page_number)
) STRICT;

CREATE TABLE sentences (
    id              TEXT PRIMARY KEY,
    paper_id        TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    page_number     INTEGER NOT NULL,
    order_index     INTEGER NOT NULL,
    source_text     TEXT NOT NULL,
    normalized_rects TEXT NOT NULL,    -- JSON array of NormalizedRect
    UNIQUE (paper_id, page_number, order_index)
) STRICT;

CREATE TABLE highlights (
    id               TEXT PRIMARY KEY,
    paper_id         TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    page_number      INTEGER NOT NULL,
    group_key        TEXT,             -- one logical selection may span pages
    color            TEXT NOT NULL,
    selected_text    TEXT NOT NULL,
    normalized_rects TEXT NOT NULL,
    note             TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
) STRICT;

CREATE INDEX idx_highlights_paper_page ON highlights (paper_id, page_number);

CREATE TABLE analyses (
    paper_id        TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
    schema_version  TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    content_hash    TEXT NOT NULL,
    structured_json TEXT NOT NULL,
    markdown        TEXT NOT NULL,
    created_at      TEXT NOT NULL
) STRICT;

CREATE TABLE translations (
    paper_id        TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    page_number     INTEGER NOT NULL,
    target_language TEXT NOT NULL,
    source_hash     TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    prompt_version  TEXT NOT NULL,
    payload         TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (paper_id, page_number, target_language, source_hash, provider, model, prompt_version)
) STRICT;

CREATE TABLE chunks (
    id           TEXT PRIMARY KEY,
    paper_id     TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    section      TEXT,
    page_start   INTEGER NOT NULL,
    page_end     INTEGER NOT NULL,
    text         TEXT NOT NULL,
    token_count  INTEGER NOT NULL,
    content_hash TEXT NOT NULL
) STRICT;

CREATE INDEX idx_chunks_paper ON chunks (paper_id);

-- Vectors live in a separate table keyed by index generation so that switching
-- the embedding model never mixes incompatible vectors (DEVELOPMENT.md §5.2).
CREATE TABLE chunk_vectors (
    chunk_id         TEXT NOT NULL REFERENCES chunks (id) ON DELETE CASCADE,
    index_generation INTEGER NOT NULL,
    dimension        INTEGER NOT NULL,
    embedding        BLOB NOT NULL,
    PRIMARY KEY (chunk_id, index_generation)
) STRICT;

CREATE TABLE relations (
    source_paper_id TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    target_paper_id TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL,     -- semantic | manual | citation
    score           REAL,
    provenance      TEXT,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (source_paper_id, target_paper_id, relation_type)
) STRICT;

CREATE TABLE chat_sessions (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    scope_type TEXT NOT NULL,          -- paper | group | library
    scope_id   TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE chat_messages (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES chat_sessions (id) ON DELETE CASCADE,
    role       TEXT NOT NULL,          -- user | assistant | system
    content    TEXT NOT NULL,
    status     TEXT NOT NULL,          -- pending | streaming | complete | failed | cancelled
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX idx_chat_messages_session ON chat_messages (session_id, created_at);

CREATE TABLE chat_citations (
    message_id TEXT NOT NULL REFERENCES chat_messages (id) ON DELETE CASCADE,
    chunk_id   TEXT NOT NULL,
    paper_id   TEXT NOT NULL,
    page_start INTEGER NOT NULL,
    page_end   INTEGER NOT NULL,
    PRIMARY KEY (message_id, chunk_id)
) STRICT;

CREATE TABLE jobs (
    id           TEXT PRIMARY KEY,
    paper_id     TEXT REFERENCES papers (id) ON DELETE CASCADE,
    type         TEXT NOT NULL,
    status       TEXT NOT NULL,        -- queued | running | waiting_for_key | failed | completed | cancelled
    priority     INTEGER NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    payload      TEXT NOT NULL DEFAULT '{}',
    error_code   TEXT,
    idempotency_key TEXT UNIQUE,       -- paper_id + content_hash + task_version
    not_before   TEXT,                 -- backoff gate
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

CREATE INDEX idx_jobs_claim ON jobs (status, priority DESC, created_at);

CREATE TABLE sync_records (
    paper_id            TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
    vault_path          TEXT NOT NULL,
    base_hash           TEXT,
    app_revision        TEXT,
    vault_revision      TEXT,
    attachment_strategy TEXT,          -- hardlink | copy
    status              TEXT NOT NULL, -- synced | pending | conflict | disconnected
    updated_at          TEXT NOT NULL
) STRICT;

-- Single-row app settings. Provider secrets are NOT stored here — only the
-- reference used to look them up in the OS credential store (§16.1).
CREATE TABLE settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    language                 TEXT NOT NULL DEFAULT 'ko',
    active_provider          TEXT,
    openai_model             TEXT,
    anthropic_model          TEXT,
    openai_credential_ref    TEXT,
    anthropic_credential_ref TEXT,
    translation_language     TEXT NOT NULL DEFAULT 'ko',
    obsidian_vault_path      TEXT,
    embedding_model_id       TEXT NOT NULL DEFAULT 'intfloat/multilingual-e5-small',
    embedding_dimension      INTEGER NOT NULL DEFAULT 384,
    index_generation         INTEGER NOT NULL DEFAULT 1,
    network_notice_accepted_at TEXT,
    onboarding_completed_at  TEXT,
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL
) STRICT;

-- Keyword search over paper metadata and page text (§11.2).
CREATE VIRTUAL TABLE papers_fts USING fts5 (
    paper_id UNINDEXED,
    title,
    authors,
    abstract,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE chunks_fts USING fts5 (
    chunk_id UNINDEXED,
    paper_id UNINDEXED,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);
