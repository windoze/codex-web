# Claude Code support (plan)

Status: **phases 1–4 implemented** (tool persisted; runner abstraction; Claude runner MVP; UI tool selector + badges). Hardening pending.

This document describes how to add **Claude Code** (referred to as `claude-code`) as an alternative
execution backend alongside **Codex CLI** in `codex-web`.

The goal is to keep the product model the same:
- Conversations are persisted as **append-only events** in SQLite.
- The web UI is **stateless** and “catches up” from the event log.
- Each user message triggers a **per-turn** assistant execution (not a permanently running agent).
- “Blocking prompts” are represented as **interaction requests** that can be answered from either UI or terminal.

---

## 1) Goals (what “done” means)

### 1.1 User-visible goals
- When creating a new conversation, the user can choose which tool to use:
  - `codex` (current behavior)
  - `claude-code` (new)
- The conversation list shows an **icon/badge** indicating which tool is used.
- The active conversation header/title shows the same icon/badge.
- Claude Code runs behave like Codex runs in the UI:
  - user message → assistant run starts
  - streaming assistant output appears as events/messages
  - if the assistant needs approval/input, an **interaction card** appears
  - user can respond from UI or terminal, and the run continues

### 1.2 Backend goals
- Add a generalized “assistant runner” abstraction so we can support multiple tools without forking the whole orchestrator.
- Persist enough metadata to reliably resume a conversation:
  - tool selection (`codex` vs `claude-code`)
  - tool session/thread identifier (if the tool supports session resume)
- Keep **non-reentrancy**: one in-flight turn per conversation regardless of tool.

### 1.3 Compatibility goals
- Existing Codex conversations continue working unchanged.
- New DB columns/migrations default existing rows to `tool = codex`.

---

## 2) Non-goals (for the first Claude Code milestone)
- Perfect parity with all Claude Code UX features.
- Switching an existing conversation from Codex → Claude (or vice versa) after creation.
- Multi-user authentication/permissions beyond what `codex-web` already supports.
- Cross-tool transcript portability (e.g., converting Codex session state into Claude session state).

---

## 3) Assumptions & open questions (must resolve before implementation)

Claude Code’s CLI/protocol details determine how “deep” this integration can be.

### 3.1 CLI surface
Questions to answer during a short spike:
- What is the canonical executable name (`claude-code`, `claude`, something else)?
- Does it support **non-interactive** operation suitable for a daemon?
- Does it support a **session/thread id** that can be resumed per turn (analogous to `codex exec resume <SESSION_ID>`)?
- Is there a **structured output mode** (ideally JSONL) that can be parsed incrementally for streaming?

### 3.2 Interaction / approval model
We need to understand how Claude Code requests approvals:
- Does it emit explicit events for approvals/choices, or does it prompt via TTY text?
- Can responses be provided programmatically (stdin / flags / API) so a web UI can answer?

### 3.3 Proposed constraint (recommended)
For a robust `codex-web` integration, Claude Code must provide **machine-readable events**.

If Claude Code does not provide JSON output or a stable programmatic prompt/response protocol,
we should implement one of these fallbacks:
1) A small “bridge” wrapper that translates Claude Code into a JSONL event stream, or
2) Limit Claude Code support to “best-effort transcript capture” without interactive approvals (not recommended).

### 3.4 Implemented contract (current MVP)
The current Claude runner is implemented against a **bridge-style** contract (it may work with a native CLI
if it happens to match the same shape).

Execution command (from the daemon, `cwd = project_root`):
- `claude-code exec --json <PROMPT>`
- `claude-code exec resume <SESSION_ID> --json <PROMPT>` (when `tool_session_id` is present)

Stdout is expected to be **JSONL**, where each line is a JSON object. codex-web persists each JSON object
as a raw `claude_event` conversation event, and derives an `agent_message` at the end of the turn.

Recognized JSON event shapes (minimal):
- `{ "type": "session_configured", "session_id": "..." }`
- `{ "type": "assistant_message_delta", "delta": "..." }`
- `{ "type": "assistant_message_completed", "text": "..." }` (optional)
- `{ "type": "interaction_request", "kind": "...", "prompt": "..." }`

Interaction responses are written back to the tool’s stdin using a simple terminal-style protocol:
- Accept/decline → `y\\n` / `n\\n`
- Text input (kind `claude.input`) → `<text>\\n`

---

## 4) Product model changes: introduce `tool`

### 4.1 New concept: conversation tool
Each conversation gets a `tool` field:
- `codex` (default)
- `claude-code`

This field is immutable for MVP (simplifies correctness and avoids confusing mixed histories).

### 4.2 Run model
A “run” remains “an in-flight turn”. Runs become tool-agnostic:
- `run.status`: queued/running/waiting_for_interaction/completed/failed/aborted
- `run.tool_session_id`: the tool’s resume handle (if any)

### 4.3 Event model
We keep the existing event sourcing pattern:
- `conversation_events.type` remains a small set of derived UI events (`user_message`, `agent_message`, `run_status`, …)
- additionally store “raw tool protocol” events for debuggability:
  - existing: `codex_event`
  - new: `claude_event`

The UI should treat raw tool events as optional: they are primarily for debugging and diagnostics,
and only some derived metrics (like token usage) depend on them.

---

## 5) Claude Code basic functions to support (MVP)

This section defines what `codex-web` means by “Claude Code support” at a product level.

### 5.1 Start a new Claude Code conversation
When a new conversation is created with `tool = claude-code`:
- The daemon records `conversations.tool = claude-code`.
- The first user message triggers a Claude Code run.
- On first successful run, persist a `tool_session_id` (if Claude Code supports resume).

### 5.2 Continue an existing Claude Code conversation
On subsequent user messages:
- The daemon uses the persisted `tool_session_id` (if available) to keep context consistent.
- If Claude Code does not support session resume, the daemon must reconstruct context from the event log
  (this is possible but may be expensive and may not behave identically to native sessions).

### 5.3 Stream assistant output into the event log
Desired UX parity with Codex:
- As Claude Code produces tokens/segments, the daemon appends events:
  - `conversation_events.type = agent_message` (delta or chunk-based)
  - optionally `conversation_events.type = claude_event` for the raw protocol
- The WebSocket stream pushes these events to connected clients.

### 5.4 Capture tool calls / execution steps (optional for MVP)
If Claude Code exposes tool-call events (file edits, commands, searches, etc.):
- Persist them as raw `claude_event` events.
- Optionally derive a higher-level `tool_call` / `tool_result` event type later.

For MVP, it’s acceptable to only support:
- user messages
- assistant messages
- interaction requests/responses
- run lifecycle

---

## 6) Interactions: mapping Claude Code → codex-web interaction requests

`codex-web` already models “blocking prompts” as `interaction_requests` with:
- `kind` (string)
- `status` (`pending`/`resolved`)
- `payload_json` (arbitrary JSON)
- `default_action` (`accept`/`decline`, today)
- `timeout_ms`

Claude Code interactions should reuse this model so that:
- The existing terminal commands (`cargo run -- interactions …`) continue to work.
- The web UI continues to render interaction cards uniformly.

### 6.1 Interaction kinds (proposal)
Define a small stable set of `kind` values for Claude Code, for example:
- `claude.confirm` — generic “Continue?” confirmations
- `claude.permission.exec` — permission to run a command
- `claude.permission.write` — permission to modify files
- `claude.select` — choose one option from a list
- `claude.input` — free-form text input

Exact mapping depends on Claude Code’s native categories/events.

### 6.2 Payload shape (proposal)
Standardize payload fields so the UI can render without tool-specific branches:
```json
{
  "prompt": "Allow running `npm test`?",
  "detail": "Claude wants to run a command in the project directory.",
  "choices": [
    { "id": "accept", "label": "Allow", "style": "primary" },
    { "id": "decline", "label": "Deny", "style": "danger" }
  ],
  "default_choice_id": "decline",
  "tool": "claude-code",
  "raw": { "any": "tool-specific fields for debugging" }
}
```

Notes:
- `choices` can be omitted for free-form input prompts (`claude.input`).
- Keep `raw` for debugging and lossless storage of upstream details.

### 6.3 Responding to interactions
The response path should be identical to Codex:
- UI → `POST /api/interactions/:interaction_id/respond`
- terminal → `cargo run -- interactions respond …`

Backend behavior:
- First response wins (atomic DB update).
- The runner unblocks and continues the tool execution.
- Persist an explicit `interaction_response` event in `conversation_events` (existing behavior).

### 6.4 Default / timeout behavior
Claude Code integration should respect existing daemon config:
- `CODEX_WEB_INTERACTION_TIMEOUT_MS`
- `CODEX_WEB_INTERACTION_DEFAULT_ACTION`

If Claude Code has richer choices than accept/decline, we may need:
- `default_choice_id` in payload
- a more expressive `RespondInteractionRequest` API (e.g. `choice_id` instead of `action`)

This can be phased:
1) MVP: map everything to `accept`/`decline` where possible
2) Next: support multi-choice / text responses

---

## 7) Backend changes (design)

### 7.1 Database migrations (proposal)
Add tool selection to conversations, and generalize run session id:

Implemented in `migrations/0003_tool_and_tool_session.sql`:

1) ✅ `conversations.tool TEXT NOT NULL DEFAULT 'codex'`
2) ✅ Generalize the tool resume handle:
   - `runs.tool_session_id TEXT NULL` (code uses this now)
   - `runs.codex_session_id` remains as a legacy column for backward compatibility (can be removed in a later cleanup migration)

Migration strategy (to keep compatibility simple):
- ✅ Introduced new columns first; kept `codex_session_id` temporarily.
- ✅ Backfilled `runs.tool_session_id = runs.codex_session_id` where present.
- ✅ Updated code to use `runs.tool_session_id`.
- ⏳ Future cleanup: remove `runs.codex_session_id` once stable.

If we want fewer migrations, we can instead add:
- `runs.claude_session_id TEXT NULL`
…but that becomes awkward as more tools are added.

### 7.2 API changes (proposal)
Update conversation creation:
- `POST /api/conversations`
  - add: `{ tool?: "codex" | "claude-code" }`

Update conversation list items to include tool:
- `GET /api/conversations` returns `(Conversation & { run_status: string, tool: string })[]`
…or embed tool inside `Conversation` itself.

### 7.3 Orchestrator changes (proposal)
Introduce a tool-agnostic runner interface:
```text
trait Runner {
  fn tool() -> ToolKind;
  async fn start_turn(conversation_id, project_root, tool_session_id, user_message, …) -> TurnHandle;
  async fn resume_from_interaction(turn_handle, interaction_response, …);
}
```

Then:
- existing Codex code becomes `CodexRunner`
- Claude Code support becomes `ClaudeRunner`
- orchestrator selects runner based on `conversation.tool`

### 7.4 Tool configuration
Add config/env vars for locating and controlling Claude Code:
- ✅ `CODEX_WEB_CLAUDE_CODE_BIN` (default: `claude-code`)
- ✅ `CODEX_WEB_CLAUDE_CODE_ARGS` (optional additional args; whitespace-delimited)
- Optionally a “disable Claude” flag if the binary is missing.

---

## 8) Frontend changes (design)

### 8.1 New conversation flow: tool selector
In the “New conversation…” modal, add a tool selector above the directory picker:
- Label: “Assistant”
- Options:
  - “Codex” (default)
  - “Claude Code”

UX notes:
- Remember last selection in `localStorage` (nice-to-have).
- If the server reports Claude is unavailable, disable the Claude option with a tooltip.

### 8.2 Conversation list: tool icon/badge
Add a small icon/badge next to each conversation title.

Because the frontend currently avoids icon libraries, a simple approach is:
- A `ToolBadge` component that renders:
  - `CX` for Codex (blue)
  - `CL` for Claude Code (purple)
- Include `aria-label` (“Uses Codex” / “Uses Claude Code”).

### 8.3 Conversation header/title: tool icon/badge
In the active conversation header (top of the chat view), render the same badge next to the title.

This ensures users can always tell which backend is active, even after deep-linking to a conversation.

### 8.4 Events rendering considerations
The UI currently has some Codex-specific logic (e.g., token usage derived from `codex_event`).

Plan:
- Keep Codex-specific rendering as-is.
- Add parallel support for Claude if Claude provides usage/metadata.
- Ensure the UI never crashes if a conversation has `tool = claude-code` and there are no `codex_event`s.

---

## 9) Implementation phases (recommended order)

### Phase 0: Claude Code protocol spike (required)
- Confirm executable name and how to run it non-interactively.
- Confirm whether it supports:
  - resumable session id
  - JSONL (or other structured streaming output)
  - machine-readable interaction prompts and programmatic responses
- Decide between:
  - “native JSON integration”
  - “bridge wrapper” integration

### Phase 1: Data model + API plumbing
- ✅ Add `conversations.tool` to DB and API types.
- ✅ Add generalized `runs.tool_session_id` storage (keeping backward compatibility).
- ✅ Surface tool in `GET /api/conversations` list items.

### Phase 2: Runner abstraction
- ✅ Refactor current Codex execution into `CodexRunner` behind a common interface.
- ✅ Add a `ClaudeRunner` stub that returns a clear “not yet supported” error.

### Phase 3: ClaudeRunner MVP
- Implement `ClaudeRunner` to:
  - ✅ run a turn
  - ✅ stream assistant output (raw `claude_event` deltas; final `agent_message`)
  - ✅ persist raw `claude_event` protocol events (if available)
  - ✅ create interaction requests and resume after responses
  - (See “Implemented contract” above for the current JSONL contract.)

### Phase 4: UI/UX changes
- ✅ Add assistant selector to the New Conversation modal.
- ✅ Add tool badge/icon in:
  - ✅ conversation list
  - ✅ conversation header/title
- ✅ Ensure “send message” disables correctly during Claude runs (`run_status` parity).

### Phase 5: Hardening
- Tests:
  - DB migration correctness (existing conversations default to Codex)
  - UI rendering for mixed tool lists
  - interaction response races (“first response wins”)
- Better error surfaces when the Claude binary is missing/misconfigured.

---

## 10) Acceptance criteria checklist

### Backend
- [x] `POST /api/conversations` accepts a tool choice and persists it.
- [x] `GET /api/conversations` returns tool per conversation.
- [x] Existing conversations without tool data behave as `codex`.
- [x] Runs remain non-reentrant per conversation.
- [x] Claude interactions appear in `/api/interactions/pending` and can be answered from terminal.

### Frontend
- [x] “New conversation…” offers `codex` vs `claude-code`.
- [x] Conversation list shows a tool badge/icon for every conversation.
- [x] Active conversation title/header shows the same badge/icon.
- [x] Claude conversations do not display Codex-specific UI in a broken way (no crashes).
