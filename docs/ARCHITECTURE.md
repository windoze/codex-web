# Architecture (codex-web)

## High-level

`codex-web` is a **local** daemon that provides:
- a REST API for projects/conversations
- a WebSocket event stream for realtime updates
- durable persistence in SQLite (append-only conversation events)

The UI (added later) reconnects safely by replaying events from the database and then subscribing for new ones.

## Conversation model

Each conversation is stored as an **event stream**:
- user messages
- agent messages (Codex output)
- tool call / tool result events
- run lifecycle events
- interaction request / interaction response events

The server is the source of truth for conversation state; the UI is a projection.

## Codex execution model

To avoid keeping a background Codex process running, codex-web uses the Codex CLI session model:
- Start: `codex exec --json ...` → persist returned `thread_id` (session id)
- Continue: `codex exec resume <SESSION_ID> --json ...`

This enables:
- daemon restarts without losing conversation history (events are persisted)
- resilient UI reconnects (UI can close/reopen without interrupting work)

## Non-reentrancy

`codex exec resume <SESSION_ID>` is not re-entrant.

codex-web enforces **one in-flight turn per conversation**:
- API rejects concurrent sends for the same conversation (`409 Conflict`)
- runs are tracked in the database so the rule is consistently enforced

