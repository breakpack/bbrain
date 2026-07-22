-- Topic graph ("second brain"): concepts extracted from each paper's AI analysis
-- (keywords + suggested tags), merged across the library by embedding similarity.
-- Nodes are topics, not papers; a paper is a member of the topics it discusses.
-- The whole graph is rebuilt from scratch when the set of analyses changes, so
-- these tables are treated as a derived cache, not a source of truth.

CREATE TABLE topics (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL,          -- representative display name for the concept
    normalized TEXT NOT NULL,     -- lowercased/collapsed form of the label
    paper_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
) STRICT;

-- Which papers a topic was extracted from.
CREATE TABLE paper_topics (
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    PRIMARY KEY (paper_id, topic_id)
) STRICT;

CREATE INDEX idx_paper_topics_topic ON paper_topics(topic_id);

-- Topic-to-topic edges. cooccurrence weight = number of papers discussing both;
-- semantic weight = cosine similarity of the topics' centroid vectors.
CREATE TABLE topic_edges (
    source_topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    target_topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    edge_type TEXT NOT NULL,      -- cooccurrence | semantic
    weight REAL NOT NULL,
    PRIMARY KEY (source_topic_id, target_topic_id, edge_type)
) STRICT;

-- Single row recording what the current topic graph was built from, so a rebuild
-- is skipped when the analyses have not changed since.
CREATE TABLE topic_build (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    signature TEXT NOT NULL,
    built_at TEXT NOT NULL
) STRICT;
