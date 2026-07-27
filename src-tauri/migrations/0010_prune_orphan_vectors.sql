-- vec0 virtual tables do not participate in foreign-key cascades, so deleting
-- a paper left its rows in paper_vectors/chunk_vectors behind. The semantic
-- KNN then returned those deleted paper ids as neighbours, and inserting a
-- relation edge pointing at a missing paper hit the relations FOREIGN KEY —
-- every relations job failed with a storage error. Remove the orphans;
-- `paper_repo::delete` now cleans both vec tables on every delete.

DELETE FROM paper_vectors WHERE paper_id NOT IN (SELECT id FROM papers);
DELETE FROM chunk_vectors WHERE chunk_id NOT IN (SELECT id FROM chunks);
