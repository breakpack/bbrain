-- Sentences carry the paragraph they belong to, so translation can offer a
-- paragraph granularity (group consecutive sentences) alongside per-sentence.
ALTER TABLE sentences ADD COLUMN paragraph_index INTEGER NOT NULL DEFAULT 0;
