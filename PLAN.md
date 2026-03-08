# Codex Web Interface — Implementation Plan

## 1) Goals / Requirements (What “done” means)

### Core requirements (from prompt)
1. **Full-featured conversation interface** that:
   - Feels like a modern chat product (streaming responses, markdown/code blocks, copy actions, attachments later).
   - **Does not interrupt conversation flow** when the web UI is closed/reopened (conversation continues; UI can reconnect and catch up without losing state).
2. **Start new conversations from a project directory** (local path now; GitHub repository support later).
3. **Handle interaction requests from the Codex CLI tool**:
   - Surface “requests for user input/approval” (e.g., confirmations, permissions, choices) in the web UI.
   - Allow users to respond from **either** the terminal or the web UI.
   - Provide configurable **default responses** when the user is away / UI is closed (to avoid deadlocks).
4. **Multiple conversation management**:
   - Switch between conversations seamlessly.
   - Support different projects/topics concurrently.

### Non-goals (initially, to reduce scope)
- Multi-user / remote hosted service (start as local-only, single-user).
- Enterprise auth/SSO.
- Full GitHub integration (auth, PR review flows, etc.) beyond “open repo path / clone later”.
- Perfect feature parity with every existing chat product on day one.

## 2) Product Model (Concepts & Definitions)

### Entities
- **Project**: A local working directory (later: a GitHub repo reference + local checkout).
- **Conversation**: A persistent thread of messages + events associated with a project (or “general”).
- **Run**: An active execution of Codex for a conversation (preferably a per-turn `codex exec --json` / `codex exec resume <SESSION_ID> --json` invocation; not necessarily a long-lived background process).
- **Interaction request**: A “blocking” prompt from Codex that requires user input/approval (or a default decision).
- **Event log**: Append-only record of everything that happened (messages, tool calls, interaction requests/responses, status).

### Core UX invariants
- UI is **stateless by default**: closing the tab should not break a conversation, and reopening should resume from the server’s persisted event log.
- Conversations are **addressable** by stable IDs, not by browser session state.
- Interaction requests always have a clear status: `pending`, `answered_by_user`, `auto_answered`, `timed_out`, `canceled`.

## 3) Architecture Overview

### High-level components
1. **codex-web daemon (Rust)** (this repo)
   - Runs locally, owns persistence and process/session orchestration.
   - Exposes:
     - REST API for conversation/project management
     - WebSocket (or SSE) for realtime event streaming to the browser UI
     - Local IPC endpoint for terminal-client integration (and future “attach” to external Codex runs)
2. **Web frontend**
   - Single-page app (SPA) connecting to daemon.
   - Renders conversation list + active chat + interaction panel.
3. **Codex CLI**
   - Invoked by the daemon per turn via `codex exec --json` (persisting `<SESSION_ID>` for resume), with an optional future path to “attach” external runs.

### Key design choice: event-sourced conversation state
- Store an **append-only event stream** per conversation in SQLite.
- Rebuild current view state from events (or maintain projection tables for speed).
- Benefits:
  - Reconnect/catch-up becomes trivial: UI asks “give me events since cursor N”.
  - Multiple clients can attach safely (future-proof).
  - Auditing/debugging is easier.

### Communication channels
- **Browser UI ↔ daemon**: WebSocket for events; REST for commands.
- **Codex ↔ daemon**: `codex exec --json` subprocess I/O (plus optional local IPC for terminal clients / future attach).

## 4) Persistence Plan (SQLite)

### Storage approach
- Use a single SQLite DB under a predictable location (e.g., `~/.codex-web/codex-web.sqlite`).
- Keep a migration system (e.g., `sqlx` migrations) from day 1.

### Suggested schema (initial)
- `projects`
  - `id`, `name`, `root_path`, `created_at`, `updated_at`
- `conversations`
  - `id`, `project_id` (nullable), `title`, `created_at`, `updated_at`, `archived_at`
- `conversation_events`
  - `id` (monotonic), `conversation_id`, `ts`, `type`, `payload_json`
  - `type` examples: `user_message`, `assistant_message_delta`, `assistant_message_final`,
    `tool_call`, `tool_result`, `interaction_request`, `interaction_response`, `run_status`
- `runs`
  - `id`, `conversation_id`, `status`, `started_at`, `ended_at`, `codex_session_id`, `active_pid` (optional), `metadata_json`
- Optional “projection” tables for fast listing:
  - `conversation_latest` (last_message_preview, last_event_id, unread_count)

### Cursor-based replay
- UI maintains `last_event_id`.
- On reconnect, UI requests `GET /api/conversations/{id}/events?after={last_event_id}`.

## 5) Codex CLI Integration Plan (Interaction Requests + Terminal UX)

### Target behavior
- Codex can prompt for user input while running in a terminal.
- The daemon can route that prompt to:
  1) the terminal (existing behavior), and/or
  2) the web UI as a rich prompt card,
  3) an auto-response policy if the user is away.

### Integration strategy (phased)
**Phase A (preferred): per-turn session-based execution with `codex exec --json`**
- First message in a conversation:
  - Run `codex exec --json` in the project directory.
  - Parse JSON output/events and persist them to `conversation_events`.
  - Persist returned `<SESSION_ID>` (if provided) as `runs.codex_session_id`.
- Subsequent messages:
  - Run `codex exec resume <SESSION_ID> --json` for continuity.
- Pros: no long-lived background Codex process; daemon restarts cleanly; aligns with Codex CLI’s native session model.
- Cons: must implement robust JSON streaming/decoding and enforce per-conversation non-reentrancy.

**Phase B (optional): attach external terminal runs**
- Add a small “bridge” (wrapper or terminal client) that:
  - connects to daemon (IPC/HTTP)
  - forwards Codex JSON events and interaction requests
  - receives interaction responses (from web or terminal)
- Enables: “use Codex from terminal, but answer prompts from web (and vice versa)”.

### Non-reentrancy (one active turn per conversation)
Because `codex exec resume <SESSION_ID> --json` is not re-entrant, enforce:
- At most one in-flight Codex invocation per conversation.
- Backend guardrails:
  - In-memory per-conversation mutex (prevents concurrent HTTP requests from racing).
  - DB-level state transition (“claim the run”) so restarts/retries don’t create overlaps.
- API behavior:
  - `POST /api/conversations/{id}/messages` returns `409 Conflict` if the conversation is currently `running` (or `waiting_for_interaction`), and the UI disables the send box while active.

### Local IPC protocol (suggestion)
- JSON messages over a framed stream (length-prefixed) or newline-delimited JSON.
- Message types:
  - `register_run { conversation_id, metadata }`
  - `event { conversation_id, event_type, payload }`
  - `interaction_request { request_id, conversation_id, prompt, choices, timeout_ms, default_policy_key }`
  - `interaction_response { request_id, response, source }`

### Default response policies (away mode)
Implement a policy engine that can answer requests automatically when:
- No web clients are connected, or
- Request is not answered within `timeout_ms`, or
- User explicitly enables “away mode”.

Policy should be:
- **Configurable globally** and optionally per project/conversation.
- **Explicit and safe-by-default**:
  - Example defaults:
    - “ask” for destructive actions
    - “deny” for elevated permissions
    - “accept” only for low-risk prompts (e.g., “continue?”)

### Resolving dual-input (web + terminal)
If both the terminal and web UI can answer:
- First response wins (atomic update in DB).
- Subsequent responses are rejected with a clear reason.
- UI shows “answered by terminal” (and vice versa).

## 6) Web UI Plan (Full-featured conversation experience)

### Core screens
1. **Conversation list**
   - Filter by project
   - Show last message preview + status (running, waiting-for-input, idle)
2. **Conversation view**
   - Virtualized message list
   - Streaming assistant output (delta events)
   - Markdown rendering w/ code blocks, copy buttons
   - Message actions: retry, edit last user message (optional), export transcript
3. **Interaction drawer/panel**
   - Always visible when a request is pending
   - Shows request details + safe defaults + time remaining
4. **Project picker**
   - Create project from directory path (file picker or typed path)
   - Recent projects

### UX for reconnect / resume
- On load:
  - Fetch conversations list (REST)
  - Connect WS and subscribe to active conversation(s)
  - Backfill events since last cursor to avoid gaps
- If WS disconnects:
  - UI switches to “reconnecting…” but remains usable for browsing history

### Frontend tech (choose one path)
- Option 1: React + Vite + TypeScript + Tailwind (fast iteration, common stack)
- Option 2: Rust-native (Leptos / Dioxus) (single-language, but potentially slower iteration)

Given this repo is Rust-only today, prefer **Option 1** initially to maximize UI velocity, while keeping the daemon API stable.

## 7) Backend API Plan (Rust)

### Server framework
- Use `axum` (Tokio-based) for REST + WebSocket.
- Add structured logging (`tracing`) and config (`figment` or `config`).

### REST endpoints (initial)
- `POST /api/projects` (create from local path)
- `GET /api/projects`
- `POST /api/conversations` (create; includes project_id and initial message optionally)
- `GET /api/conversations`
- `GET /api/conversations/{id}`
- `GET /api/conversations/{id}/events?after=...`
- `POST /api/conversations/{id}/messages` (user message; triggers run/continue)
- `POST /api/interaction/{request_id}/respond`
- `POST /api/runs/{conversation_id}/start` / `POST /api/runs/{conversation_id}/stop`

### WebSocket channels
- `/ws` with subscription messages:
  - `subscribe { conversation_id }`
  - server pushes `event { ... }`

### Concurrency model
- Per-conversation actor/task manages:
  - run lifecycle
  - buffering and persistence of events
  - interaction request timeouts and auto-responses

## 8) Multi-conversation & Multi-project Management

### Switching conversations
- UI: clicking a conversation only changes which conversation is actively rendered.
- Backend: conversations are independent; runs can be active concurrently (bounded by resource limits).

### Resource limiting
- Configurable max concurrent runs (e.g., 2–4 by default).
- Queue or “pause” runs when over limit.

### Conversation lifecycle
- Create, rename, archive, delete.
- Export transcript (JSON or Markdown) for portability.

## 9) GitHub Repository Support (Later Milestone)

### Phase 1 (minimal)
- Accept a GitHub URL + branch.
- Clone into a managed workspace directory (e.g., `~/.codex-web/repos/<owner>/<repo>`).
- Create a project pointing to that directory.

### Phase 2 (auth + richer UX)
- GitHub OAuth device flow (for private repos).
- Repo list picker, branch selector, sync/pull controls.

## 10) Milestones (Implementation Phases)

### Milestone 0 — Foundations (1–2 days)
- Choose frontend stack and create workspace layout (Rust server + frontend folder).
- Add config, logging, and SQLite migrations.
- Implement conversation/event persistence.

### Milestone 1 — Basic chat + persistence (3–5 days)
- REST: create/list projects and conversations.
- WS: stream events to UI.
- UI: conversation list + basic chat view.
- Validate reconnect: close tab → reopen → continues, history intact.

### Milestone 2 — Run orchestration (3–7 days)
- Daemon runs `codex exec --json` per user message and persists `<SESSION_ID>` for resume.
- Capture/stream Codex JSON output as events (no brittle stdout parsing).
- Enforce per-conversation non-reentrancy (one in-flight invocation at a time).
- Show running status + streaming assistant messages.

### Milestone 3 — Interaction requests + defaults (5–10 days)
- Implement interaction request event type + response handling.
- UI interaction panel with timer and safe default preview.
- Auto-response policy engine + timeouts.
- Ensure “first response wins” between web and terminal.

### Milestone 4 — Multi-conversation polish (3–7 days)
- Concurrent runs + resource limits.
- Search, rename, archive, export.
- Project switching UX improvements.

### Milestone 5 — Schema-typed Codex protocol (1–2 days)
- Track upstream Codex JSON schemas in `schemas/`.
- Parse `codex exec --json` output into schema-derived Rust types (no ad-hoc `serde_json::Value` branching).
- Derive `agent_message` and interaction requests from typed events.

### Milestone 6 — GitHub (optional, later)
- Clone workflow + repo projects.
- Authentication if needed.

## 11) Testing & Verification Strategy

### Backend
- Unit tests for:
  - event append + cursor replay
  - interaction request timeout + policy decisions
- Integration tests (tokio) for:
  - WS reconnect and catch-up correctness
  - “first response wins” race handling

### Frontend
- E2E smoke tests (Playwright) for:
  - reconnect behavior
  - switching conversations
  - responding to interaction requests

## 12) Deliverables Checklist
- [x] Local daemon with SQLite persistence
- [x] REST + WS APIs
- [x] Web UI with multi-conversation chat
- [x] Run orchestration for project-based conversations
- [x] Interaction request routing + default responses
- [x] Documented config knobs (ports, DB path, policies, limits)
- [x] Schema-typed parsing for Codex `--json` output

## 13) Progress
- [x] Milestone 0 — Foundations
- [x] Milestone 1 — Basic chat + persistence
- [x] Milestone 2 — Run orchestration
- [x] Milestone 3 — Interaction requests + defaults
- [x] Milestone 4 — Multi-conversation polish
- [x] Milestone 5 — Schema-typed Codex protocol
- [ ] Milestone 6 — GitHub (optional, later)
