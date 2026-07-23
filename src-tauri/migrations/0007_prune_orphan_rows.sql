-- Prune rows orphaned before foreign_keys enforcement was in place.
--
-- Early dev builds could delete a paper without cascading, leaving analyses
-- (and potentially other child rows) pointing at a paper that no longer
-- exists. Rebuilding the topic graph then re-inserts those paper ids into
-- paper_topics, hits its FOREIGN KEY, and the whole graph page fails with a
-- storage error. Remove anything whose paper is gone; current deletes cascade
-- correctly, so this is a one-time cleanup.

DELETE FROM analyses WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM pages WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM sentences WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM chunks WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM highlights WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM translations WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM paper_metadata WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM paper_groups WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM paper_tags WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM paper_topics WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM sync_records WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM relations
 WHERE source_paper_id NOT IN (SELECT id FROM papers)
    OR target_paper_id NOT IN (SELECT id FROM papers);
