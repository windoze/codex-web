use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ClaudeRuntime {
    /// Call the configured Claude Code binary (or a bridge wrapper).
    Real(ClaudeReal),
    /// Deterministic stub for tests (replays a fixed JSON event list).
    Stub(ClaudeStub),
}

#[derive(Debug, Clone)]
pub struct ClaudeReal {
    pub bin: String,
    pub args: Vec<String>,
}

impl Default for ClaudeReal {
    fn default() -> Self {
        Self {
            bin: "claude-code".to_string(),
            args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeStub {
    pub events: Arc<Vec<Value>>,
    pub exit_success: bool,
}

#[derive(Debug, Clone)]
pub struct ClaudeInvocation {
    pub project_root: PathBuf,
    pub session_id: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeOutcome {
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ClaudeOutputLine {
    Json(Value),
    OutputLine(String),
}

impl ClaudeRuntime {
    pub fn real(bin: String, args: Vec<String>) -> Self {
        Self::Real(ClaudeReal { bin, args })
    }

    pub fn stub(events: Vec<Value>) -> Self {
        Self::Stub(ClaudeStub {
            events: Arc::new(events),
            exit_success: true,
        })
    }

    pub fn stub_failing(events: Vec<Value>) -> Self {
        Self::Stub(ClaudeStub {
            events: Arc::new(events),
            exit_success: false,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeCliMode {
    /// Bridge/wrapper contract:
    ///   <bin> exec [resume <SESSION_ID>] --json <PROMPT>
    BridgeExecJson,
    /// Native Claude Code CLI (`claude`) contract:
    ///   claude --print --output-format=stream-json [--resume <UUID> | --session-id <UUID>] <PROMPT>
    NativeClaudePrintStreamJson,
}

pub async fn run_jsonl_events_with_input<F, Fut, I, IFut>(
    runtime: ClaudeRuntime,
    invocation: ClaudeInvocation,
    mut on_event: F,
    mut on_input: I,
) -> anyhow::Result<ClaudeOutcome>
where
    F: FnMut(ClaudeOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&Value) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    match runtime {
        ClaudeRuntime::Real(cfg) => {
            run_real_with_input(cfg, invocation, &mut on_event, &mut on_input).await
        }
        ClaudeRuntime::Stub(stub) => {
            run_stub_with_input(stub, invocation, &mut on_event, &mut on_input).await
        }
    }
}

async fn run_stub_with_input<F, Fut, I, IFut>(
    stub: ClaudeStub,
    invocation: ClaudeInvocation,
    on_event: &mut F,
    on_input: &mut I,
) -> anyhow::Result<ClaudeOutcome>
where
    F: FnMut(ClaudeOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&Value) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let mut session_id_out: Option<String> = invocation.session_id.clone();

    for e in stub.events.iter() {
        if session_id_out.is_none() {
            session_id_out = session_id_from_value(e);
        }

        let needs_input = stdin_needed(e);
        on_event(ClaudeOutputLine::Json(e.clone())).await?;

        if needs_input {
            let _ = on_input(e).await?;
        }
    }

    if !stub.exit_success {
        anyhow::bail!("stubbed claude-code failure");
    }

    Ok(ClaudeOutcome {
        session_id: session_id_out,
    })
}

async fn run_real_with_input<F, Fut, I, IFut>(
    cfg: ClaudeReal,
    invocation: ClaudeInvocation,
    on_event: &mut F,
    on_input: &mut I,
) -> anyhow::Result<ClaudeOutcome>
where
    F: FnMut(ClaudeOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&Value) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let mode = detect_cli_mode(&cfg.bin);
    match mode {
        ClaudeCliMode::BridgeExecJson => {
            run_bridge_exec_json(cfg, invocation, on_event, on_input).await
        }
        ClaudeCliMode::NativeClaudePrintStreamJson => {
            run_native_claude_print_stream_json(cfg, invocation, on_event, on_input).await
        }
    }
}

async fn run_bridge_exec_json<F, Fut, I, IFut>(
    cfg: ClaudeReal,
    invocation: ClaudeInvocation,
    on_event: &mut F,
    on_input: &mut I,
) -> anyhow::Result<ClaudeOutcome>
where
    F: FnMut(ClaudeOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&Value) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let ClaudeInvocation {
        project_root,
        session_id,
        prompt,
    } = invocation;

    let mut cmd = tokio::process::Command::new(&cfg.bin);
    cmd.current_dir(&project_root);
    cmd.args(&cfg.args);

    // Bridge/wrapper contract (mirrors Codex):
    //   claude-code exec [resume <SESSION_ID>] --json <PROMPT>
    cmd.arg("exec");
    if let Some(session_id) = &session_id {
        cmd.arg("resume").arg(session_id);
    }
    cmd.arg("--json").arg(prompt);

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().with_context(|| format!("spawn {}", cfg.bin))?;
    let stdout = child.stdout.take().context("claude-code stdout missing")?;
    let mut stdin = child.stdin.take().context("claude-code stdin missing")?;
    let mut lines = BufReader::new(stdout).lines();

    let mut session_id_out: Option<String> = None;

    while let Some(line) = lines.next_line().await.context("read claude-code stdout")? {
        if line.trim().is_empty() {
            continue;
        }

        let parsed = match serde_json::from_str::<Value>(&line) {
            Ok(v) => ClaudeOutputLine::Json(v),
            Err(_) => ClaudeOutputLine::OutputLine(line),
        };

        if session_id_out.is_none()
            && let ClaudeOutputLine::Json(v) = &parsed
        {
            session_id_out = session_id_from_value(v);
        }

        let needs_input = matches!(parsed, ClaudeOutputLine::Json(ref v) if stdin_needed(v));
        on_event(parsed.clone()).await?;

        if needs_input
            && let ClaudeOutputLine::Json(v) = &parsed
            && let Some(input) = on_input(&v).await?
        {
            stdin
                .write_all(input.as_bytes())
                .await
                .context("write claude-code stdin")?;
            stdin.flush().await.context("flush claude-code stdin")?;
        }
    }

    let status = child.wait().await.context("wait claude-code process")?;
    if !status.success() {
        anyhow::bail!("claude-code exec failed with status: {status}");
    }

    Ok(ClaudeOutcome {
        session_id: session_id_out.or(session_id),
    })
}

#[derive(Debug, Default)]
struct NativeClaudeState {
    message_id: Option<String>,
}

async fn run_native_claude_print_stream_json<F, Fut, I, IFut>(
    cfg: ClaudeReal,
    invocation: ClaudeInvocation,
    on_event: &mut F,
    _on_input: &mut I,
) -> anyhow::Result<ClaudeOutcome>
where
    F: FnMut(ClaudeOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&Value) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let ClaudeInvocation {
        project_root,
        session_id,
        prompt,
    } = invocation;

    // The native `claude` CLI requires a UUID for `--session-id` / `--resume`.
    let is_resume = session_id.is_some();
    let desired_session_id = session_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut cmd = tokio::process::Command::new(&cfg.bin);
    cmd.current_dir(&project_root);
    cmd.args(&cfg.args);

    // Native Claude Code CLI contract:
    //   claude --print --output-format=stream-json [--resume <UUID> | --session-id <UUID>] <PROMPT>
    //
    // Notes:
    // - We force `--output-format=stream-json` so stdout is JSONL and can be incrementally parsed.
    // - We set a stable session id so codex-web can resume across turns.
    cmd.arg("--print")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--include-partial-messages")
        .arg("--verbose");

    if is_resume {
        cmd.arg("--resume").arg(&desired_session_id);
    } else {
        cmd.arg("--session-id").arg(&desired_session_id);
    }

    cmd.arg(prompt);

    // IMPORTANT: when `claude` sees a non-TTY stdin, it may attempt to read from it (even when the
    // prompt is provided as a CLI argument), and can block waiting for EOF. codex-web runs are
    // per-turn and provide the prompt as an argument, so we keep stdin closed to avoid hangs.
    //
    // This means native `claude` mode currently cannot answer interactive prompts via stdin. If you
    // need programmatic interactions, use a bridge/wrapper binary that implements the exec/resume
    // JSONL contract.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().with_context(|| format!("spawn {}", cfg.bin))?;
    let stdout = child.stdout.take().context("claude stdout missing")?;
    let mut lines = BufReader::new(stdout).lines();

    // Emit a synthetic session event so the UI/db has a reliable resume handle even if the CLI
    // doesn't include it in its stream output.
    on_event(ClaudeOutputLine::Json(json!({
        "type": "session_configured",
        "session_id": desired_session_id.clone(),
        "source": "claude.native",
    })))
    .await?;

    let mut state = NativeClaudeState::default();
    let mut session_id_out: Option<String> = Some(desired_session_id);

    while let Some(line) = lines.next_line().await.context("read claude stdout")? {
        if line.trim().is_empty() {
            continue;
        }

        let parsed = match serde_json::from_str::<Value>(&line) {
            Ok(raw) => {
                let mapped = canonicalize_native_stream_event(raw, &mut state);
                ClaudeOutputLine::Json(mapped)
            }
            Err(_) => ClaudeOutputLine::OutputLine(line),
        };

        if session_id_out.is_none()
            && let ClaudeOutputLine::Json(v) = &parsed
        {
            session_id_out = session_id_from_value(v);
        }

        let needs_input = matches!(parsed, ClaudeOutputLine::Json(ref v) if stdin_needed(v));
        on_event(parsed.clone()).await?;

        if needs_input {
            // Native `claude` mode runs with stdin closed (see comment above).
            // We intentionally do not block here waiting for interaction responses.
            on_event(ClaudeOutputLine::Json(json!({
                "type": "claude.native_event",
                "warning": "interaction_request cannot be answered in native mode (stdin closed)",
                "raw": match &parsed { ClaudeOutputLine::Json(v) => v, _ => &json!(null) },
            })))
            .await?;
        }
    }

    let status = child.wait().await.context("wait claude process")?;
    if !status.success() {
        anyhow::bail!("claude exec failed with status: {status}");
    }

    Ok(ClaudeOutcome {
        session_id: session_id_out,
    })
}

fn stdin_needed(v: &Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("interaction_request")
}

fn session_id_from_value(v: &Value) -> Option<String> {
    v.get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("thread_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

fn detect_cli_mode(bin: &str) -> ClaudeCliMode {
    if is_native_claude_bin(bin) {
        return ClaudeCliMode::NativeClaudePrintStreamJson;
    }
    ClaudeCliMode::BridgeExecJson
}

fn is_native_claude_bin(bin: &str) -> bool {
    let file = Path::new(bin).file_name().and_then(|s| s.to_str());
    matches!(file, Some("claude") | Some("claude.exe"))
}

fn canonicalize_native_stream_event(raw: Value, state: &mut NativeClaudeState) -> Value {
    // Pass through bridge-style events unchanged if the native CLI happens to emit them.
    if let Some(t) = raw.get("type").and_then(|t| t.as_str())
        && matches!(
            t,
            "session_configured"
                | "assistant_message_delta"
                | "assistant_message"
                | "assistant_message_completed"
                | "interaction_request"
        )
    {
        return raw;
    }

    // Newer Claude Code versions wrap Anthropic streaming events like:
    //   { "type": "stream_event", "event": { "type": "content_block_delta", ... }, ... }
    if raw.get("type").and_then(|t| t.as_str()) == Some("stream_event") {
        let inner_type = raw
            .get("event")
            .and_then(|e| e.get("type"))
            .and_then(|t| t.as_str());

        if inner_type == Some("message_start") {
            if let Some(id) = raw
                .get("event")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
            {
                state.message_id = Some(id.to_string());
            }
            return json!({ "type": "claude.native_event", "raw": raw });
        }

        if inner_type == Some("content_block_delta") {
            if let Some(delta) = raw
                .get("event")
                .and_then(|e| e.get("delta"))
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
            {
                return json!({
                    "type": "assistant_message_delta",
                    "delta": delta,
                    "message_id": state.message_id.clone(),
                    "raw": raw,
                    "source": "claude.native",
                });
            }
        }

        return json!({ "type": "claude.native_event", "raw": raw });
    }

    // Claude Code `--print --output-format=stream-json` also emits summary objects like:
    //   { "type": "assistant", "message": { "content": [ { "type": "text", "text": "..." } ] } }
    if raw.get("type").and_then(|t| t.as_str()) == Some("assistant") {
        if let Some(id) = raw
            .get("message")
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
        {
            state.message_id = Some(id.to_string());
        }

        let mut out = String::new();
        if let Some(content) = raw
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_array())
        {
            for part in content {
                if part.get("type").and_then(|t| t.as_str()) != Some("text") {
                    continue;
                }
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }

        if !out.is_empty() {
            return json!({
                "type": "assistant_message_completed",
                "text": out,
                "raw": raw,
                "source": "claude.native",
            });
        }

        return json!({ "type": "claude.native_event", "raw": raw });
    }

    // Final "result" event (contains the full assistant text in `result`).
    if raw.get("type").and_then(|t| t.as_str()) == Some("result") {
        if let Some(text) = raw.get("result").and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return json!({
                    "type": "assistant_message_completed",
                    "text": text,
                    "raw": raw,
                    "source": "claude.native",
                });
            }
        }
        return json!({ "type": "claude.native_event", "raw": raw });
    }

    let typ = raw.get("type").and_then(|t| t.as_str());
    if matches!(typ, Some("error") | Some("exception") | Some("stderr")) {
        return json!({ "type": "claude.native_event", "raw": raw });
    }

    // Try to track a stable message id (used by the UI to group deltas into a single bubble).
    if let Some(message_id) = raw
        .get("message_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            raw.get("message")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    {
        state.message_id = Some(message_id);
    }

    // Anthropic-style streaming events (common shape for "stream-json" CLIs):
    //   { "type": "content_block_delta", "delta": { "text": "..." }, ... }
    //   { "type": "message_start", "message": { "id": "...", ... } }
    if let Some(t) = typ {
        if t == "message_start" {
            if let Some(id) = raw
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
            {
                state.message_id = Some(id.to_string());
            }
            return json!({ "type": "claude.native_event", "raw": raw });
        }

        if t == "content_block_delta" {
            if let Some(delta) = raw
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
            {
                return json!({
                    "type": "assistant_message_delta",
                    "delta": delta,
                    "message_id": state.message_id.clone(),
                    "raw": raw,
                    "source": "claude.native",
                });
            }
        }
    }

    // Generic fallback: if the event has a plausible text/delta field, treat it as a delta.
    if let Some(delta) = raw.get("delta").and_then(|v| v.as_str()) {
        return json!({
            "type": "assistant_message_delta",
            "delta": delta,
            "message_id": state.message_id.clone(),
            "raw": raw,
            "source": "claude.native",
        });
    }

    if let Some(text) = raw.get("text").and_then(|v| v.as_str()) {
        let role = raw.get("role").and_then(|v| v.as_str());
        let looks_like_assistant_text = matches!(role, Some("assistant"))
            || matches!(typ, Some("message") | Some("assistant") | Some("content") | Some("output"));
        if looks_like_assistant_text {
            return json!({
                "type": "assistant_message_delta",
                "delta": text,
                "message_id": state.message_id.clone(),
                "raw": raw,
                "source": "claude.native",
            });
        }
    }

    // Otherwise just wrap it so "raw messages" toggle can still show something.
    json!({ "type": "claude.native_event", "raw": raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_stream_event_wrapped_text_deltas() {
        let mut state = NativeClaudeState::default();

        let mapped = canonicalize_native_stream_event(
            json!({
                "type": "stream_event",
                "event": { "type": "message_start", "message": { "id": "msg_123" } }
            }),
            &mut state,
        );
        assert_eq!(
            mapped.get("type").and_then(|v| v.as_str()),
            Some("claude.native_event")
        );
        assert_eq!(state.message_id.as_deref(), Some("msg_123"));

        let mapped = canonicalize_native_stream_event(
            json!({
                "type": "stream_event",
                "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": "hel" } }
            }),
            &mut state,
        );
        assert_eq!(
            mapped.get("type").and_then(|v| v.as_str()),
            Some("assistant_message_delta")
        );
        assert_eq!(mapped.get("delta").and_then(|v| v.as_str()), Some("hel"));
        assert_eq!(
            mapped.get("message_id").and_then(|v| v.as_str()),
            Some("msg_123")
        );
    }

    #[test]
    fn canonicalizes_assistant_summary_objects() {
        let mut state = NativeClaudeState::default();

        let mapped = canonicalize_native_stream_event(
            json!({
                "type": "assistant",
                "message": {
                    "id": "msg_456",
                    "content": [
                        { "type": "text", "text": "Hello" },
                        { "type": "text", "text": " world" }
                    ]
                }
            }),
            &mut state,
        );
        assert_eq!(
            mapped.get("type").and_then(|v| v.as_str()),
            Some("assistant_message_completed")
        );
        assert_eq!(mapped.get("text").and_then(|v| v.as_str()), Some("Hello world"));
        assert_eq!(state.message_id.as_deref(), Some("msg_456"));
    }

    #[test]
    fn canonicalizes_result_objects() {
        let mut state = NativeClaudeState::default();
        let mapped = canonicalize_native_stream_event(
            json!({ "type": "result", "result": "Done" }),
            &mut state,
        );
        assert_eq!(
            mapped.get("type").and_then(|v| v.as_str()),
            Some("assistant_message_completed")
        );
        assert_eq!(mapped.get("text").and_then(|v| v.as_str()), Some("Done"));
    }
}
