# codex-web

Local daemon + web UI for managing **persistent Codex conversations** across multiple projects.

The core idea is simple:
- The daemon persists an **append-only event log** per conversation (SQLite).
- The web UI is stateless: it reconnects and “catches up” from the event log.
- Codex execution is **per-turn** using `codex exec --json` and `codex exec resume <SESSION_ID> --json`.

See `PLAN.md` for the implementation roadmap and milestone status.

## Requirements

- Rust toolchain (edition 2024)
- `codex` CLI available in `PATH` (for run orchestration milestones)

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

## Configuration

The daemon can be configured via CLI flags or environment variables:

- `CODEX_WEB_LISTEN` (default `127.0.0.1:8787`)
- `CODEX_WEB_DB_PATH` (default `~/.codex-web/codex-web.sqlite`)
- `CODEX_WEB_STATIC_DIR` (optional; serve prebuilt UI assets)
- `RUST_LOG` (default `codex_web=info,tower_http=info`)

## Data storage

By default the SQLite DB lives at:

`~/.codex-web/codex-web.sqlite`

The schema is created via SQLx migrations in `migrations/`.

