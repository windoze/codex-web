# codex-web

Local daemon + web UI for managing **persistent Codex conversations** across multiple projects.

The core idea is simple:
- The daemon persists an **append-only event log** per conversation (SQLite).
- The web UI is stateless: it reconnects and “catches up” from the event log.
- Codex execution is **per-turn** using `codex exec --json` and `codex exec resume <SESSION_ID> --json`.

See `PLAN.md` for the implementation roadmap and milestone status.

## Requirements

- Rust toolchain (edition 2024)
- `codex` CLI available in `PATH` (required for assistant responses)

Frontend requirements (added in later milestones):
- Node.js + npm

## Run the daemon (development)

```sh
cargo run -- serve --listen 127.0.0.1:8787
```

Health check:

```sh
curl -s http://127.0.0.1:8787/healthz
```

## Run the web UI (development)

In a second terminal:

```sh
cd frontend
npm install
npm run dev
```

By default the UI expects the daemon at `http://127.0.0.1:8787`.

If you run the daemon on a different address, set:

```sh
VITE_API_BASE=http://127.0.0.1:8787 npm run dev
```

In the UI, click “New conversation…” to pick a project directory (the picker starts at your home directory).

UI tips:
- Message bubbles are collapsible. `agent_message` and user bubbles start expanded; other bubbles start collapsed. Click the triangle to expand/collapse.
- If the daemon is started with `--auth-token` / `CODEX_WEB_AUTH_TOKEN`, the UI shows a login screen on startup to enter the token.
- On smaller screens, use the top-left menu button to open the conversation list.
- On supported browsers, the composer has a microphone button for voice input (Web Speech API).
- On mobile-sized screens, “New conversation…” opens as a full-screen modal so the directory picker is usable.

## Single-binary UI (bundled assets)

To build a self-contained `codex-web` binary that serves the web UI without `CODEX_WEB_STATIC_DIR`,
compile with the `bundled-ui` feature:

```sh
cargo build --release --features bundled-ui
./target/release/codex-web serve --listen 127.0.0.1:8787
```

Notes:
- Requires Node.js + npm at build time (the build script runs `npm run build` in `frontend/`).
- If you already built the UI and want to skip the build script, set `CODEX_WEB_SKIP_UI_BUILD=1`.

## Respond to interaction requests (terminal)

List pending interactions (across all conversations):

```sh
cargo run -- interactions list
```

If the daemon requires auth, pass the token:

```sh
cargo run -- interactions list --auth-token YOUR_TOKEN
```

Respond:

```sh
cargo run -- interactions respond <INTERACTION_ID> --action accept
```

## Configuration

The daemon can be configured via CLI flags or environment variables:

- `CODEX_WEB_LISTEN` (default `127.0.0.1:8787`)
- `CODEX_WEB_DB_PATH` (default `~/.codex-web/codex-web.sqlite`)
- `CODEX_WEB_STATIC_DIR` (optional; serve prebuilt UI assets)
- `CODEX_WEB_AUTH_TOKEN` (optional; require `Authorization: Bearer <token>` for `/api/*` and `/ws`)
- `CODEX_WEB_INTERACTION_TIMEOUT_MS` (default `30000`)
- `CODEX_WEB_INTERACTION_DEFAULT_ACTION` (default `decline`)
- `CODEX_WEB_CODEX_APPROVAL_POLICY` (default `never`)
- `CODEX_WEB_CODEX_SANDBOX` (default `workspace-write`)
- `CODEX_WEB_MAX_CONCURRENT_RUNS` (default `2`)
- `RUST_LOG` (default `codex_web=info,tower_http=info`)

## Codex protocol schemas

This repo tracks Codex JSON protocol schemas in `schemas/` and generates Rust types from them
at build time (via `typify`, see `src/protocol.rs`). This is used to parse `codex exec --json`
output into strongly-typed events.

Regenerate schemas:

```sh
codex app-server generate-json-schema --out ./schemas
```

## Data storage

By default the SQLite DB lives at:

`~/.codex-web/codex-web.sqlite`

The schema is created via SQLx migrations in `migrations/`.
