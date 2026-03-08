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

## Respond to interaction requests (terminal)

List pending interactions (across all conversations):

```sh
cargo run -- interactions list
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
- `CODEX_WEB_INTERACTION_TIMEOUT_MS` (default `30000`)
- `CODEX_WEB_INTERACTION_DEFAULT_ACTION` (default `decline`)
- `CODEX_WEB_CODEX_APPROVAL_POLICY` (default `never`)
- `CODEX_WEB_CODEX_SANDBOX` (default `workspace-write`)
- `RUST_LOG` (default `codex_web=info,tower_http=info`)

## Data storage

By default the SQLite DB lives at:

`~/.codex-web/codex-web.sqlite`

The schema is created via SQLx migrations in `migrations/`.
