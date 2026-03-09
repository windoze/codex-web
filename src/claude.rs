use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
        ClaudeRuntime::Real(cfg) => run_real_with_input(cfg, invocation, &mut on_event, &mut on_input).await,
        ClaudeRuntime::Stub(stub) => run_stub_with_input(stub, invocation, &mut on_event, &mut on_input).await,
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

