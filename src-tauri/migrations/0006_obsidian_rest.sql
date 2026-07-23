-- Obsidian Local REST API integration (the channel Obsidian MCP servers use).
--
-- When configured, vault writes go through Obsidian's Local REST API plugin so
-- a running Obsidian picks them up immediately; the file-based path stays as
-- the fallback. Only the endpoint URL lives here — the API key goes to the OS
-- credential store, and this row keeps a reference marker at most (§16.1).

ALTER TABLE settings ADD COLUMN obsidian_rest_url TEXT;
ALTER TABLE settings ADD COLUMN obsidian_rest_credential_ref TEXT;
