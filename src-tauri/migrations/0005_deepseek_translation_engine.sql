-- DeepSeek provider support and a selectable translation engine.
--
-- DeepSeek is an OpenAI-compatible provider; like the other providers it keeps
-- only a credential reference here, never the key material (§16.1). The active
-- model is stored per provider, so a `deepseek_model` column joins the existing
-- `openai_model` / `anthropic_model` pair.
--
-- `translation_engine` chooses how the reader's page/selection translation runs:
--   'google' — the free machine-translation endpoint (default, unchanged).
--   'llm'    — the currently selected AI provider translates the page.
-- The default stays 'google' so upgrading changes nothing until the user opts in.

ALTER TABLE settings ADD COLUMN deepseek_model TEXT;
ALTER TABLE settings ADD COLUMN deepseek_credential_ref TEXT;
ALTER TABLE settings ADD COLUMN translation_engine TEXT NOT NULL DEFAULT 'google';
