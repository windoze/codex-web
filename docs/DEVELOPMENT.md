# Development workflow

## Backend (daemon)

Run:

```sh
cargo run -- serve --listen 127.0.0.1:8787
```

If you want to require a token for API access:

```sh
cargo run -- serve --listen 127.0.0.1:8787 --auth-token YOUR_TOKEN
```

Smoke check:

```sh
curl -s http://127.0.0.1:8787/healthz
```

## Frontend (UI)

```sh
cd frontend
npm install
npm run dev
```

If the daemon is not on the default address, set `VITE_API_BASE`:

```sh
VITE_API_BASE=http://127.0.0.1:8787 npm run dev
```

## Bundled UI (single binary)

Build and run the daemon with embedded UI assets:

```sh
cargo run --features bundled-ui -- serve --listen 127.0.0.1:8787
```

This runs the frontend build via `npm run build` (in `frontend/`) as part of the Rust build.

## Manual reconnect verification (Milestone 1)

1. Start the daemon and UI.
2. Click “New conversation…” and pick a project directory (starts at your home directory).
3. Send a few messages.
4. Close the browser tab.
5. Reopen the UI and reselect the conversation.

Expected:
- The conversation history is loaded from the server event log.
- New messages continue appending without duplicates.

## Codex run verification (Milestone 2)

1. Start the daemon and UI.
2. Create a conversation from a project directory.
3. Send a message.

Expected:
- The server invokes the `codex` CLI in that project directory.
- The UI receives `run_status` events and then an `agent_message` event.

## Codex protocol schemas

codex-web parses `codex exec --json` output into schema-typed Rust structs/enums generated from the JSON schemas in `schemas/`.

Regenerate schemas (requires a recent `codex` CLI):

```sh
codex app-server generate-json-schema --out ./schemas
```

The Rust types are generated at compile time via `typify` (see `src/protocol.rs`).

## Interaction verification (Milestone 3)

Interaction requests are emitted when the Codex CLI produces approval/elicitation events (e.g. `exec_approval_request`).

Expected:
- The UI shows an “Input required” panel with Accept/Decline actions.
- If no UI is connected, codex-web auto-responds based on `CODEX_WEB_INTERACTION_DEFAULT_ACTION`.
