# SSH Backend — Implementation Status

This document describes the **SSH backend** for `codex-web`: remote project support via SSH.

**Status: Fully Implemented** (all milestones complete)

---

## Overview

Users can create conversations backed by remote SSH projects. The system:
- Creates SSH projects with `ssh_target`, optional `ssh_port`, and `remote_root_path`
- Browses remote directories via SSH for the directory picker
- Runs `codex exec --json` remotely over SSH, streaming JSONL back
- Handles interaction requests (approval/elicitation) via stdin/stdout piping
- Shows SSH badges in the conversation list

---

## Architecture

**Option A (implemented): Local daemon + SSH remote runner**

The local `codex-web` daemon spawns `ssh -T <target> -- <command>` to execute Codex on remote hosts. JSONL stdout is parsed into typed events and persisted to SQLite identically to local runs. The web UI connects to the local daemon as before.

No remote daemon is needed — only SSH access and `codex` installed on the remote host.

---

## Implementation Details

### Data Model (SSH-0)

**Migration**: `migrations/0004_ssh_projects.sql`

New columns on `projects` table:
- `kind` TEXT NOT NULL DEFAULT 'local' — `"local"` | `"ssh"`
- `ssh_target` TEXT NULL — e.g. `user@host`
- `ssh_port` INTEGER NULL
- `remote_root_path` TEXT NULL — absolute path on remote
- `ssh_identity_file` TEXT NULL
- `ssh_known_hosts_policy` TEXT NULL

**Rust types** (`src/db.rs`):
- `ProjectKind` enum: `Local` | `Ssh`
- `Project` struct extended with all SSH fields
- `create_ssh_project()` with deduplication by `ssh_target + remote_root_path`

### SSH Module (SSH-1)

**File**: `src/ssh.rs` (~600 lines, 12 unit tests)

Core functions:
- `shell_quote(s) -> String` — POSIX single-quote escaping (`'` → `'\''`)
- `SshTarget { target, port, identity_file }` — connection config
- `build_ssh_command(target, remote_command) -> Command` — constructs `ssh -T -o BatchMode=yes -- <target> <cmd>`
- `build_remote_codex_command(...)` — builds `sh -lc 'cd <path> && codex exec --json ...'`
- `spawn_remote_streaming(target, command) -> Child` — spawns SSH with piped stdin/stdout/stderr
- `remote_home(target) -> String` — gets remote `$HOME`
- `remote_fs_list(target, path) -> (path, parent, Vec<RemoteFsEntry>)` — parses `ls -1Ap` output
- `ssh_check(target) -> SshCheckResult` — validates connectivity, user, home, codex presence

### Remote Codex Runner (SSH-2)

**File**: `src/codex.rs`

- `SshCodexConfig { ssh_target, ssh_port, ssh_identity_file, remote_root_path }` added to `CodexInvocation`
- `run_real_with_input` branches: SSH config → `spawn_remote_streaming`; else → local process spawn
- Both paths share identical JSONL parsing and stdin interaction logic

**File**: `src/runners/codex.rs`

- Builds `SshCodexConfig` from project when `project.kind == Ssh`
- Passes through `CodexInvocation` to the execution pipeline

### SSH API Endpoints (SSH-3)

**File**: `src/api.rs`

New endpoints:
- `GET /api/ssh/fs/home?ssh_target=...&ssh_port=...` → `{ path }`
- `GET /api/ssh/fs/list?ssh_target=...&path=...&ssh_port=...` → `{ path, parent, entries }`
- `POST /api/ssh/check` with `{ ssh_target, ssh_port? }` → `{ ok, remote_user, remote_home, codex_found }`

Project creation extended:
- `POST /api/projects` accepts `kind: "ssh"` with SSH fields
- Validates `ssh_target` and `remote_root_path` are present for SSH projects

### Frontend (SSH-4)

**File**: `frontend/src/lib/api.ts`
- `ProjectKind` type, `createSshProject()`, `sshFsHome()`, `sshFsList()`, `sshCheck()` functions
- `SshFsEntry`, `SshFsListResponse`, `SshCheckResponse` types

**File**: `frontend/src/App.tsx`
- "Project source" selector: Local | SSH Remote
- SSH connection fields (host, port) with "Connect" button that calls `sshCheck`
- Remote directory picker uses `sshFsHome`/`sshFsList` when SSH is selected
- `onConfirmNewConversation` calls `createSshProject` for SSH projects
- SSH badge in conversation list for SSH-backed projects

**File**: `frontend/src/styles.css`
- `.sshBadge` styling (green badge matching existing tool badge pattern)

### Test Coverage (SSH-5)

**Unit tests** (`src/ssh.rs`): 12 tests
- Shell quoting (basic, with quotes, injection attempts)
- SSH command construction (basic, with port, with identity file)
- Remote codex command building (new session, resume session)
- Command injection safety

**Integration tests**:
- `tests/ssh_project_api.rs` (5 tests):
  - Create SSH project via API (all fields)
  - Deduplication (same target+path returns same project)
  - Validation (missing ssh_target or remote_root_path → 400)
  - Create conversation on SSH project
  - Local project kind verification

- `tests/ssh_codex_stub_turn.rs` (2 tests):
  - SSH project stub turn produces agent_message events
  - SSH project DB fields roundtrip (all columns persist and read back)

**Frontend tests** (`frontend/src/App.test.ts`):
- Updated fixtures with `kind: "local"` field

---

## Decisions Made

1. **Directory picker strategy**: `ls -1Ap` parsing (no native SSH library deps)
2. **SSH config support**: Raw `ssh_target` (user@host format); SSH config hosts work via system SSH
3. **Project field storage**: Explicit columns (not metadata_json)
4. **Remote OS**: POSIX-only (Linux/macOS)
5. **Session scoping**: Session ID tied to project; resume always uses same backend
6. **Transport**: System `ssh` binary via `tokio::process::Command` (no Rust SSH library)

---

## Git History

- `SSH-0`: Add SSH project data model and API scaffolding
- `SSH-1`: Add SSH module with command builder, quoting, and remote execution
- `SSH-2`: Wire SSH runner into Codex execution pipeline
- `SSH-3`: Add SSH filesystem and connectivity check API endpoints
- `SSH-4`: Add frontend SSH support for new conversation dialog
- `SSH-5`: Add integration tests for SSH project API and runner
