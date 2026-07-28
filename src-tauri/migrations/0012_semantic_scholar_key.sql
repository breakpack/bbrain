-- Semantic Scholar keys are stored in the OS credential store. SQLite keeps
-- only a presence/reference marker so the frontend can render connection state.
ALTER TABLE settings ADD COLUMN semantic_scholar_credential_ref TEXT;
