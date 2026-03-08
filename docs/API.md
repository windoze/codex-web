# HTTP API (Milestone 1)

Base URL (default): `http://127.0.0.1:8787`

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
  - Body: `{ "project_id": "<uuid>|null", "title": "Optional" }`
  - Response: `Conversation`
- `GET /api/conversations`
  - Response: `Conversation[]`
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
    - Spawns a Codex turn via `codex exec --json` / `codex exec resume <SESSION_ID> --json`
    - Emits additional conversation events:
      - `run_status` (`running` → `completed` / `failed`)
      - `codex_event` (raw Codex JSONL)
      - `agent_message` (derived from Codex `item.completed` / `agent_message`)

## WebSocket

- `GET /ws?conversation_id=<uuid>`
  - Sends JSON-encoded `ConversationEvent` objects for that conversation.
