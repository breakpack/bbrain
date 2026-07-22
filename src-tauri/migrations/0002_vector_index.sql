-- Replaces the placeholder vector table with a sqlite-vec `vec0` index.
-- Vectors are keyed by index generation so switching the embedding model never
-- mixes incompatible vectors into one search (DEVELOPMENT.md §5.2).

DROP TABLE IF EXISTS chunk_vectors;

CREATE VIRTUAL TABLE chunk_vectors USING vec0 (
    chunk_id         TEXT PRIMARY KEY,
    index_generation INTEGER PARTITION KEY,
    embedding        FLOAT[384] distance_metric = cosine
);

-- Paper-level embedding used for semantic relations (§12.1). Same generation
-- rule applies.
CREATE VIRTUAL TABLE paper_vectors USING vec0 (
    paper_id         TEXT PRIMARY KEY,
    index_generation INTEGER PARTITION KEY,
    embedding        FLOAT[384] distance_metric = cosine
);
