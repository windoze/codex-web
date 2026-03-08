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

## Events

- `GET /api/conversations/:conversation_id/events?after=<id>&limit=<n>`
  - Response: `ConversationEvent[]` (ordered by id ASC)

## Messages

- `POST /api/conversations/:conversation_id/messages`
  - Body: `{ "text": "..." }`
  - Response: `ConversationEvent` (type = `user_message`)

## WebSocket

- `GET /ws?conversation_id=<uuid>`
  - Sends JSON-encoded `ConversationEvent` objects for that conversation.

