# SSH Backend Plan (Remote Projects + Remote Codex Runs)

This document outlines a plan to add an **SSH backend** to `codex-web` so a user can:
- create/open a project directory on a **remote machine**,
- browse/select that remote directory from the web UI,
- run `codex exec --json` / `codex exec resume <SESSION_ID> --json` **remotely** over SSH,
- keep the existing durability/reconnect guarantees (event log in SQLite, UI reconnects safely),
- preserve per-conversation non-reentrancy (no overlapping turns for the same conversation).

This is a plan only (no implementation in this file).

---

## 1) Goals (What “done” means)

### Core UX goals
- Users can create a conversation backed by either:
  - a **local** project directory (current behavior), or
  - a **remote SSH** project directory.
- When a project is remote:
  - the **directory picker** in “New conversation…” starts at the remote `$HOME` and can navigate directories.
  - sending a message starts a Codex turn by running Codex **on the remote host**, in the selected directory.
  - streamed Codex JSONL is parsed and persisted exactly as with local runs (typed schema parsing).
  - interaction requests (approval / elicitation) are surfaced in the UI and answered by the UI or auto-policy, with responses fed back to the remote Codex process stdin.
- Multiple conversations can run concurrently across different hosts (bounded by the existing global max-concurrency).
- If the browser UI disconnects, the conversation continues and the UI can reconnect and catch up from persisted events (unchanged invariant).

### Operational goals
- No need to install/run a daemon on the remote machine (initially).
- SSH authentication uses standard SSH mechanisms:
  - existing `~/.ssh/config`, agent, keys, known_hosts.
  - no password storage in `codex-web`.
- Clear errors when:
  - SSH connection fails,
  - remote `codex` binary is missing,
  - remote directory does not exist / is not a directory.

---

## 2) Non-goals (initial)

- Multi-user SSH credential management (this is still a single-user local daemon).
- Remote GUI for editing files.
- Perfect remote filesystem parity (special filesystems, Windows remotes).
- Agent forwarding, jump hosts, complex SSH config features (these can be follow-ups).
- Running Codex on a remote machine *without* SSH (e.g., cloud APIs) — out of scope for this plan.

---

## 3) Terminology / Entities

- **Project**: a working directory target. Proposed to become a tagged union:
  - `local`: `{ root_path }`
  - `ssh`: `{ ssh_target, remote_root_path, ... }`
- **SSH target**: how to connect to the remote machine.
  - Minimal format: `user@host` (with optional port).
  - Future: named SSH config host (e.g. `prod-box`) or full URI-like string.
- **Remote project root**: an absolute path on the remote machine (e.g. `/home/alice/repo`).
- **Remote runner**: the mechanism in the local daemon that starts a remote process and streams stdout/stderr.

---

## 4) Architecture Options

### Option A (recommended): Local daemon + SSH remote runner
**codex-web stays local.** For remote projects:
- The daemon runs `ssh` to execute `codex exec --json ...` on the remote host.
- The daemon streams stdout lines back, parses them into typed Codex events, and persists events to SQLite.
- The web UI connects to the local daemon exactly as today.

Pros:
- Single place for persistence and UI (local laptop).
- No remote setup beyond “SSH access + Codex installed”.
- Keeps the current “close/reopen UI safely” model.

Cons:
- Need to implement remote filesystem browsing (directory picker) and remote process I/O over SSH robustly.

### Option B (optional later): Run codex-web daemon on remote, connect via port-forward
- User runs `codex-web serve` on remote machine.
- User port-forwards `127.0.0.1:8787` back to local.

Pros:
- Minimal code changes; remote already “is local” from the daemon’s perspective.
- Remote FS picker becomes “local” on the remote daemon.

Cons:
- Requires remote daemon lifecycle management.
- Harder to manage multiple remotes/projects at once from a single UI unless the UI becomes multi-server.

This plan focuses on **Option A**, but Option B is a good fallback for early adopters.

---

## 5) Data Model Changes (SQLite + API types)

### Project model
Today `projects` is effectively local-only (`root_path`). Add a project “kind”:

#### Suggested schema direction (flexible but explicit)
- `projects.kind` (TEXT): `"local"` | `"ssh"`
- Existing `projects.root_path` remains for local.
- New nullable columns for SSH projects:
  - `projects.ssh_target` (TEXT) — e.g. `alice@host` or `host` (to use SSH config)
  - `projects.ssh_port` (INTEGER, nullable)
  - `projects.remote_root_path` (TEXT) — absolute remote path
  - `projects.ssh_identity_file` (TEXT, nullable) — optional path to key file (future)
  - `projects.ssh_known_hosts_policy` (TEXT, nullable) — future (“strict” / “accept-new”)

Alternative: `projects.metadata_json` containing all backend-specific fields.
- Pros: easiest for future backends (GitHub, containers, etc.).
- Cons: weaker typing at DB level.

Recommendation:
- Use **explicit columns for the minimal SSH fields** now, and add `metadata_json` later only if needed.

### Conversation model
No changes required besides referencing `project_id` as today.

### Run model
No changes required; `codex_session_id` remains stored per conversation.
Important note:
- The stored session id must be considered **scoped to the project backend** (local vs SSH) and host.
- Ensure “resume” always happens via the same backend/host that created the session.

### API responses
Extend `Project` JSON schema to include:
- `kind`
- `root_path` for local, or `remote_root_path` + `ssh_target` for ssh

This should be backward compatible (old clients ignore new fields).

---

## 6) API Plan

### Projects
- `POST /api/projects` should accept:
  - Local: `{ kind: "local", root_path, name? }` (existing defaults to local if kind omitted)
  - SSH: `{ kind: "ssh", ssh_target, ssh_port?, remote_root_path, name? }`
- `GET /api/projects` returns `Project[]` with the union fields.

### Remote filesystem for directory picker
Add endpoints for SSH-backed listing (two approaches):

#### Approach 1 (explicit): dedicated SSH FS endpoints
- `GET /api/ssh/fs/home?ssh_target=...&ssh_port=...` → `{ path: "/home/alice" }`
- `GET /api/ssh/fs/list?ssh_target=...&ssh_port=...&path=/abs/remote/path`
  → `{ path, parent, entries: FsEntry[] }`

#### Approach 2 (preferred long term): backend-agnostic FS endpoints
Generalize the existing FS endpoints to include a “backend ref”:
- `GET /api/fs/home?backend=local|ssh&project_id=...` (or `backend_config=...`)
- `GET /api/fs/list?backend=...&project_id=...&path=...`

Recommendation:
- Start with **Approach 1** (faster to ship; isolates risk).
- Later migrate to backend-agnostic FS endpoints once SSH is stable.

### Remote connectivity checks (nice-to-have)
Add an endpoint to validate that SSH works before creating a project:
- `POST /api/ssh/check` with `{ ssh_target, ssh_port? }`
  - returns `{ ok: true, remote_user, remote_home, codex_found: boolean }`

---

## 7) UI Plan

### New conversation dialog changes
Extend the “New conversation…” dialog to support a “Project source” choice:
- Source: `Local` | `SSH`

If `Local`:
- Keep current directory picker flow.

If `SSH`:
- Fields:
  - SSH target (text): `user@host` or `host` (SSH config host)
  - Optional port (number)
  - Remote directory picker
  - Optional conversation title

Remote directory picker behavior:
- On open:
  - call `/api/ssh/fs/home` to get remote home
  - load `/api/ssh/fs/list` on that path
- Navigation:
  - same UX as local picker (parent, list of dirs)

### Conversation list display
For SSH-backed conversations, show more helpful identity:
- Title line:
  - If conversation title set → show title
  - Else show something like: `repo (host)` or `remote_path_basename (host)`
- Consider adding a small “SSH” badge (future).

### Error UX
When SSH fails:
- show a clear message and common fixes:
  - “Check that you can run `ssh <target>` from this machine”
  - “Install codex on remote host”
  - “Verify remote path exists”

---

## 8) SSH Implementation Plan (Remote Runner)

### Principle: reuse the existing Codex JSONL pipeline
Keep the exact same event flow:
- Read line-delimited JSON from remote stdout
- Parse into typed Codex events using `schemas/`-generated Rust types
- Persist `codex_event`, derive `agent_message` / `interaction_request` events, etc.
- Feed interaction responses back to stdin when requested

### Transport choice

#### Option 1 (recommended): spawn the system `ssh` binary
Use `tokio::process::Command` to run:
- `ssh -T <target> -- <remote_command>`

Key details:
- Use `-T` to disable PTY, preventing terminal control sequences and reducing buffering surprises.
- Pass remote command via `--` to prevent option-injection.
- Use `sh -lc '<command string>'` on the remote side to:
  - `cd` into the remote project directory
  - run `codex exec --json ...`

Input/output handling:
- stdout: stream lines and parse JSONL
- stderr: optionally persist as `codex_event` “output_line” or separate event type (design choice)
- stdin: write newline-delimited responses when Codex requests input

Pros:
- No native deps; works anywhere `ssh` exists.
- Uses user’s SSH config + known_hosts naturally.

Cons:
- Remote FS listing via SFTP is not automatic (needs extra approach).

#### Option 2: library SSH (ssh2 / russh)
Use a Rust SSH library for:
- interactive sessions
- SFTP directory listing

Pros:
- Programmatic FS listing is clean.
Cons:
- native deps (`ssh2` → libssh2) or more complexity (russh).

Recommendation:
- Start with **system `ssh` for the Codex runner**, then decide separately whether FS listing should use SFTP via library or remote helper commands.

### Command construction safety
Treat all user input as untrusted:
- The remote path must be safely quoted.
- The prompt text is already passed to Codex as stdin or CLI arg depending on current design; continue to avoid shell interpolation.

Avoid:
- building remote commands by naive string concatenation without quoting.

Plan:
- Implement a small “remote shell quoting” utility for POSIX shells:
  - single-quote escaping strategy: `'` → `'\''`
- Construct remote command as:
  - `sh -lc 'cd <quoted_path> && codex exec ... --json'`

### Remote prerequisites
At run start, optionally validate:
- remote `codex` exists: `command -v codex`
- remote dir exists and is directory

If validation fails:
- persist a `run_status: failed` and an explanatory event payload.

---

## 9) Remote Filesystem Listing Plan (for picker)

This is the hardest piece to get right without native deps.

### Option A (preferred): SFTP via `ssh2` (libssh2)
- Implement listing via SFTP readdir/stat.
- Produce `FsEntry[]` with robust file type detection.

Tradeoffs:
- Adds native dependency; CI/build might need adjustments.

### Option B (no native deps): remote JSON helper command
Use `ssh` to run a command that emits JSON describing directory entries.

Examples (not final):
- `python3 -c ...` (depends on python3)
- `node -e ...` (depends on node)

Risk:
- Not all hosts have Python/Node.

### Option C (lowest deps): parse `ls` output
Run something like:
- `ls -1Ap` and infer dirs from trailing `/`.

Risk:
- Filenames can contain newlines and weird characters → parsing ambiguity.
- Locale and permissions issues.

Recommendation:
- Ship in two stages:
  1) v1: allow users to **type** the remote path (no picker) to unblock remote Codex execution.
  2) v2: add robust picker using either **SFTP** or a small remote helper strategy.

Even if v1 is shipped, keep local picker unchanged.

---

## 10) Interaction Requests over SSH

Goal: keep current behavior:
- The daemon persists `interaction_request` events and:
  - waits for a response if a web client is present, or
  - auto-resolves if user is away (existing policy engine).
- Once resolved, the daemon writes the response to the Codex process stdin.

For SSH runs this becomes:
- write to stdin of the local `ssh` process, which forwards to the remote Codex stdin.

Considerations:
- Ensure stdin writes are synchronized to the same per-turn task.
- On SSH disconnect, fail the turn and persist a clear error event.

---

## 11) Security Considerations

### SSH credential handling
- Do not store passwords.
- Prefer:
  - SSH agent
  - existing keys referenced via `~/.ssh/config`
- If supporting identity files:
  - store only the **path** to the identity file (never store the key contents)
  - consider restricting identity-file usage to local filesystem paths and document risks

### Host key verification
- Default to OpenSSH behavior (known_hosts).
- Do not implement “accept any host key” defaults.

### API security
The daemon already supports Bearer token authentication for `/api/*` and `/ws`.
SSH endpoints must remain protected by the same auth requirement when enabled.

### Limiting blast radius
Remote FS listing and command execution are powerful. Consider:
- only allowing SSH actions for hosts that are stored as Projects in the DB (“configured hosts”)
- adding a `CODEX_WEB_SSH_ALLOWED_HOSTS` allowlist (future)

---

## 12) Testing Strategy

### Unit tests
- Remote command builder:
  - path quoting correctness
  - `ssh` argv construction uses `--` to avoid option injection
- Runner I/O:
  - simulate stdout JSONL stream and verify event parsing + persistence
  - simulate stdin writes when interaction request is resolved

### Integration tests (recommended)
Use a disposable SSH server:
- Spin up `sshd` in a container (e.g. `testcontainers`) with:
  - a test user + key
  - a fake `codex` script on PATH that outputs deterministic JSONL
- Verify:
  - `POST /messages` over an SSH project produces persisted `codex_event` + derived `agent_message`
  - interaction request flow works (daemon sends stdin response)

If container-based tests are too heavy initially:
- Keep integration tests behind a feature flag (e.g. `--features ssh-tests`) and document how to run them locally.

### Frontend tests
- Unit tests around UI:
  - SSH new-conversation form validation
  - picker navigation state transitions

---

## 13) Step-by-step Milestones

### Milestone SSH-0: Data model + API scaffolding
- Add `Project.kind` with `"local"` default.
- Add SSH project fields in DB + migrations.
- Extend `POST /api/projects` and `GET /api/projects`.
- Minimal UI to create an SSH project by manually entering:
  - `ssh_target`, optional `port`, and `remote_root_path` (typed, no picker yet).

### Milestone SSH-1: Remote Codex runner (core functionality)
- Implement SSH-backed `CodexRuntime` variant or a “remote runner” abstraction.
- Run `codex exec --json` remotely and stream JSONL back.
- Support `resume <SESSION_ID>` remotely.
- Persist/derive events as today.
- Ensure per-conversation non-reentrancy still holds.

### Milestone SSH-2: Interaction requests over SSH
- Ensure stdin responses are written to the remote process properly.
- Add test coverage using a stub remote “codex” script that emits an interaction request.

### Milestone SSH-3: Remote directory picker
- Add `/api/ssh/fs/home` and `/api/ssh/fs/list`.
- Implement with the chosen FS strategy (prefer SFTP; fallback if necessary).
- UI: picker navigation identical to local picker.

### Milestone SSH-4: UX polish + hardening
- Show host/path clearly in conversation list and header.
- Add connectivity check endpoint and UI feedback.
- Add better error messages and retry behavior.
- Add observability: log SSH connection failures with redaction.

---

## 14) Open Questions / Decisions to Lock In

1. **Directory picker strategy**:
   - SFTP (library) vs helper command vs parsing `ls`.
2. **SSH config support**:
   - Do we accept raw `ssh_target` only, or named hosts from `~/.ssh/config`?
3. **Where to store SSH project fields**:
   - explicit columns vs `metadata_json`.
4. **Remote OS assumptions**:
   - POSIX-only for v1 (Linux/macOS) is simplest; Windows remotes can be later.
5. **Session scoping**:
   - session id must be tied to host+project; ensure no accidental “resume” across different targets.

