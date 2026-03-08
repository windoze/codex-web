CREATE TABLE IF NOT EXISTS interaction_requests (
  id TEXT PRIMARY KEY NOT NULL,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  timeout_ms INTEGER NOT NULL,
  default_action TEXT NOT NULL,
  resolved_at_ms INTEGER NULL,
  resolved_by TEXT NULL,
  response_json TEXT NULL
);

CREATE INDEX IF NOT EXISTS interaction_requests_conversation_status_idx
ON interaction_requests(conversation_id, status);

CREATE INDEX IF NOT EXISTS interaction_requests_status_idx
ON interaction_requests(status);

