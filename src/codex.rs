use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use std::future::Future;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::protocol::event_msg::EventMsg;

#[derive(Debug, Clone)]
pub enum CodexRuntime {
    /// Call the real `codex` binary.
    Real(CodexReal),
    /// Deterministic stub for tests (replays a fixed event list).
    Stub(CodexStub),
}

#[derive(Debug, Clone)]
pub struct CodexReal {
    pub ask_for_approval: String,
    pub sandbox: String,
    pub skip_git_repo_check: bool,
}

impl Default for CodexReal {
    fn default() -> Self {
        Self {
            ask_for_approval: "never".to_string(),
            sandbox: "workspace-write".to_string(),
            skip_git_repo_check: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexStub {
    pub events: Arc<Vec<Value>>,
    pub exit_success: bool,
}

#[derive(Debug, Clone)]
pub enum CodexOutputLine {
    /// A JSON line that successfully parsed as a Codex protocol event.
    Event(EventMsg),
    /// A JSON line that did not match the Codex protocol schema we know.
    UnknownJson(Value),
    /// A non-JSON stdout line (should be rare with `--json`).
    OutputLine(String),
}

impl CodexRuntime {
    pub fn real() -> Self {
        Self::Real(CodexReal::default())
    }

    pub fn stub(events: Vec<Value>) -> Self {
        Self::Stub(CodexStub {
            events: Arc::new(events),
            exit_success: true,
        })
    }

    pub fn stub_failing(events: Vec<Value>) -> Self {
        Self::Stub(CodexStub {
            events: Arc::new(events),
            exit_success: false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CodexInvocation {
    pub project_root: std::path::PathBuf,
    pub session_id: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct CodexOutcome {
    pub session_id: Option<String>,
}

pub async fn run_jsonl_events<F, Fut>(
    runtime: CodexRuntime,
    invocation: CodexInvocation,
    on_event: F,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(CodexOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    run_jsonl_events_with_input(runtime, invocation, on_event, |_event| async { Ok(None) }).await
}

pub async fn run_jsonl_events_with_input<F, Fut, I, IFut>(
    runtime: CodexRuntime,
    invocation: CodexInvocation,
    mut on_event: F,
    mut on_input: I,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(CodexOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&EventMsg) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    match runtime {
        CodexRuntime::Real(cfg) => run_real_with_input(cfg, invocation, &mut on_event, &mut on_input).await,
        CodexRuntime::Stub(stub) => run_stub_with_input(stub, invocation, &mut on_event, &mut on_input).await,
    }
}

async fn run_stub_with_input<F, Fut, I, IFut>(
    stub: CodexStub,
    _invocation: CodexInvocation,
    on_event: &mut F,
    on_input: &mut I,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(CodexOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&EventMsg) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let mut session_id: Option<String> = None;
    for e in stub.events.iter().cloned() {
        let parsed: CodexOutputLine = match serde_json::from_value::<EventMsg>(e.clone()) {
            Ok(ev) => CodexOutputLine::Event(ev),
            Err(_) => CodexOutputLine::UnknownJson(e.clone()),
        };

        if session_id.is_none() {
            if let CodexOutputLine::Event(ev) = &parsed {
                session_id = session_id_from_event(ev);
            }
        }

        let needs_input = matches!(parsed, CodexOutputLine::Event(ref ev) if stdin_needed(ev));
        on_event(parsed.clone()).await?;

        if needs_input {
            if let CodexOutputLine::Event(ev) = &parsed {
                let _ = on_input(ev).await?;
            }
        }
    }
    if !stub.exit_success {
        anyhow::bail!("stubbed codex failure");
    }
    Ok(CodexOutcome { session_id })
}

async fn run_real_with_input<F, Fut, I, IFut>(
    cfg: CodexReal,
    invocation: CodexInvocation,
    on_event: &mut F,
    on_input: &mut I,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(CodexOutputLine) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
    I: FnMut(&EventMsg) -> IFut + Send,
    IFut: Future<Output = anyhow::Result<Option<String>>> + Send,
{
    let CodexInvocation {
        project_root,
        session_id,
        prompt,
    } = invocation;

    let mut cmd = tokio::process::Command::new("codex");
    cmd.current_dir(&project_root);

    cmd.arg("--cd")
        .arg(&project_root)
        .arg("--ask-for-approval")
        .arg(&cfg.ask_for_approval)
        .arg("--sandbox")
        .arg(&cfg.sandbox)
        .arg("exec");

    if cfg.skip_git_repo_check {
        cmd.arg("--skip-git-repo-check");
    }

    if let Some(session_id) = &session_id {
        cmd.arg("resume").arg(session_id);
    }

    cmd.arg("--json").arg(prompt);

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawn codex process")?;
    let stdout = child.stdout.take().context("codex stdout missing")?;
    let mut stdin = child.stdin.take().context("codex stdin missing")?;
    let mut lines = BufReader::new(stdout).lines();

    let mut session_id_out: Option<String> = None;

    while let Some(line) = lines.next_line().await.context("read codex stdout")? {
        if line.trim().is_empty() {
            continue;
        }

        let parsed: CodexOutputLine = match serde_json::from_str::<EventMsg>(&line) {
            Ok(ev) => CodexOutputLine::Event(ev),
            Err(_) => match serde_json::from_str::<Value>(&line) {
                Ok(v) => CodexOutputLine::UnknownJson(v),
                Err(_) => CodexOutputLine::OutputLine(line),
            },
        };

        if session_id_out.is_none() {
            if let CodexOutputLine::Event(ev) = &parsed {
                session_id_out = session_id_from_event(ev);
            }
        }

        let needs_input = matches!(parsed, CodexOutputLine::Event(ref ev) if stdin_needed(ev));
        on_event(parsed.clone()).await?;

        if needs_input {
            if let CodexOutputLine::Event(ev) = &parsed {
                if let Some(input) = on_input(ev).await? {
                    use tokio::io::AsyncWriteExt;
                    stdin
                        .write_all(input.as_bytes())
                        .await
                        .context("write codex stdin")?;
                    stdin.flush().await.context("flush codex stdin")?;
                }
            }
        }
    }

    let status = child.wait().await.context("wait codex process")?;
    if !status.success() {
        anyhow::bail!("codex exec failed with status: {status}");
    }

    Ok(CodexOutcome {
        session_id: session_id_out.or(session_id),
    })
}

fn stdin_needed(event: &EventMsg) -> bool {
    use crate::protocol::event_msg::EventMsg as M;

    matches!(
        event,
        M::ExecApprovalRequest { .. } | M::ApplyPatchApprovalRequest { .. } | M::ElicitationRequest { .. }
    )
}

fn session_id_from_event(event: &EventMsg) -> Option<String> {
    use crate::protocol::event_msg::EventMsg as M;

    match event {
        M::SessionConfigured { session_id, .. } => Some(session_id.to_string()),
        M::ItemStarted { thread_id, .. } => Some(thread_id.to_string()),
        M::ItemCompleted { thread_id, .. } => Some(thread_id.to_string()),
        _ => None,
    }
}
