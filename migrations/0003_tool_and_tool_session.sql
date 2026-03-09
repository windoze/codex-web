-- Add per-conversation tool selection (default existing rows to Codex).
ALTER TABLE conversations
ADD COLUMN tool TEXT NOT NULL DEFAULT 'codex';

-- Generalize the tool resume handle (Codex uses a session id today).
ALTER TABLE runs
ADD COLUMN tool_session_id TEXT NULL;

-- Backfill existing Codex session ids.
UPDATE runs
SET tool_session_id = codex_session_id
WHERE tool_session_id IS NULL AND codex_session_id IS NOT NULL;

