# HTTP API (Milestone 1)

Base URL (default): `http://127.0.0.1:8787`

## Authentication (optional)

If the daemon is started with `--auth-token` (or `CODEX_WEB_AUTH_TOKEN`), all `/api/*` requests must include:

`Authorization: Bearer <token>`

The browser WebSocket endpoint (`/ws`) cannot set custom headers, so the UI uses a `token` query param:

`/ws?conversation_id=<uuid>&token=<token>`

## Health

- `GET /healthz` → `{ "status": "ok" }`

## Projects

- `POST /api/projects`
  - Body: `{ "root_path": "/abs/path", "name": "Optional" }`
  - Response: `Project`
- `GET /api/projects`
  - Response: `Project[]`

## Conversations

- `POST /api/conversations`
  - Body: `{ "project_id": "<uuid>|null", "title": "Optional", "tool": "codex"|"claude-code" (optional) }`
  - Response: `Conversation`
- `GET /api/conversations`
  - Response: `(Conversation & { run_status: string })[]`
- `GET /api/conversations/:conversation_id`
  - Response: `{ "conversation": Conversation, "run": Run }`
- `PATCH /api/conversations/:conversation_id`
  - Body: `{ "title": "Optional", "archived": true|false }`
  - Response: `Conversation`
- `GET /api/conversations/:conversation_id/export?format=json|md`
  - Exports a transcript; Markdown export includes only `user_message` / `agent_message`

## Events

- `GET /api/conversations/:conversation_id/events?after=<id>&limit=<n>`
  - Response: `ConversationEvent[]` (ordered by id ASC)

## Interaction requests

Interaction requests represent “blocking” prompts (e.g., approvals) that can be answered from the web UI.

- `GET /api/conversations/:conversation_id/interactions`
  - Response: `InteractionRequest[]` (currently only pending requests)
- `GET /api/interactions/pending`
  - Response: `InteractionRequest[]` (all pending interactions across conversations)
- `POST /api/interactions/:interaction_id/respond`
  - Body: `{ "action": "accept" | "decline", "text": "optional" }`
  - Returns `409` if the interaction was already resolved

## Messages

- `POST /api/conversations/:conversation_id/messages`
  - Body: `{ "text": "..." }`
  - Response: `ConversationEvent` (type = `user_message`)
  - Side effects:
    - Marks the conversation run as `running` (non-reentrant; concurrent sends return `409`)
    - Spawns a per-turn assistant run based on `conversation.tool`:
      - `codex`: `codex exec --json` / `codex exec resume <SESSION_ID> --json`
      - `claude-code`: `claude-code exec --json` / `claude-code exec resume <SESSION_ID> --json`
        - The Claude runner expects JSONL on stdout (either native or via a small wrapper/bridge).
    - Emits additional conversation events:
      - `run_status` (`running` → `completed` / `failed`)
      - `codex_event` (raw Codex JSONL; only when `conversation.tool = codex`)
      - `claude_event` (raw Claude Code/bridge JSONL; only when `conversation.tool = claude-code`)
      - `agent_message` (derived assistant output)

## WebSocket

- `GET /ws?conversation_id=<uuid>`
  - Sends JSON-encoded `ConversationEvent` objects for that conversation.

## Filesystem (directory picker)

These endpoints exist to support the UI’s “New conversation…” directory picker.

Notes:
- Paths must be absolute (the server will reject relative paths).
- The default list path is `~` (expanded to the user’s home directory).

- `GET /api/fs/home`
  - Response: `{ "path": "/abs/path/to/home" }`
- `GET /api/fs/list?path=/abs/path`
  - Response:
    - `{ "path": "/abs/path", "parent": "/abs", "entries": FsEntry[] }`
    - `FsEntry` = `{ "name": "src", "path": "/abs/path/src", "kind": "dir"|"file"|"symlink"|"other" }`
