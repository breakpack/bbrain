-- Where a paper's title came from, so the title found inside the PDF can
-- replace the filename-derived placeholder without ever clobbering a name the
-- user typed. 'file' = derived from the imported file name, 'detected' = read
-- from the PDF itself (metadata or page-1 layout), 'user' = renamed by hand.

ALTER TABLE papers ADD COLUMN title_source TEXT NOT NULL DEFAULT 'file';
