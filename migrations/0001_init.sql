PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  root_path TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY NOT NULL,
  project_id TEXT NULL REFERENCES projects(id) ON DELETE SET NULL,
  title TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  archived_at_ms INTEGER NULL
);

CREATE INDEX IF NOT EXISTS conversations_project_updated_at_idx
ON conversations(project_id, updated_at_ms);

CREATE INDEX IF NOT EXISTS conversations_updated_at_idx
ON conversations(updated_at_ms);

CREATE TABLE IF NOT EXISTS conversation_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  ts_ms INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS conversation_events_conversation_id_id_idx
ON conversation_events(conversation_id, id);

CREATE TABLE IF NOT EXISTS runs (
  conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  status TEXT NOT NULL,
  started_at_ms INTEGER NULL,
  ended_at_ms INTEGER NULL,
  codex_session_id TEXT NULL,
  active_pid INTEGER NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  updated_at_ms INTEGER NOT NULL
);

