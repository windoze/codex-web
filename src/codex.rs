use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use std::future::Future;
use tokio::io::{AsyncBufReadExt, BufReader};

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
    mut on_event: F,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(Value) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    match runtime {
        CodexRuntime::Real(cfg) => run_real(cfg, invocation, &mut on_event).await,
        CodexRuntime::Stub(stub) => run_stub(stub, invocation, &mut on_event).await,
    }
}

async fn run_stub<F, Fut>(
    stub: CodexStub,
    _invocation: CodexInvocation,
    on_event: &mut F,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(Value) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    let mut session_id: Option<String> = None;
    for e in stub.events.iter().cloned() {
        if session_id.is_none() {
            session_id = thread_id_from_event(&e);
        }
        on_event(e).await?;
    }
    if !stub.exit_success {
        anyhow::bail!("stubbed codex failure");
    }
    Ok(CodexOutcome { session_id })
}

async fn run_real<F, Fut>(
    cfg: CodexReal,
    invocation: CodexInvocation,
    on_event: &mut F,
) -> anyhow::Result<CodexOutcome>
where
    F: FnMut(Value) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    let CodexInvocation {
        project_root,
        session_id,
        prompt,
    } = invocation;

    let mut cmd = tokio::process::Command::new("codex");

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

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawn codex process")?;
    let stdout = child.stdout.take().context("codex stdout missing")?;
    let mut lines = BufReader::new(stdout).lines();

    let mut session_id_out: Option<String> = None;

    while let Some(line) = lines.next_line().await.context("read codex stdout")? {
        if line.trim().is_empty() {
            continue;
        }

        let json: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                // Preserve non-JSON output to help debugging.
                let _ = on_event(serde_json::json!({
                    "type": "codex.output_line",
                    "text": line,
                }))
                .await;
                continue;
            }
        };

        if session_id_out.is_none() {
            session_id_out = thread_id_from_event(&json);
        }

        on_event(json).await?;
    }

    let status = child.wait().await.context("wait codex process")?;
    if !status.success() {
        anyhow::bail!("codex exec failed with status: {status}");
    }

    Ok(CodexOutcome {
        session_id: session_id_out.or(session_id),
    })
}

fn thread_id_from_event(event: &Value) -> Option<String> {
    let t = event.get("type")?.as_str()?;
    if t != "thread.started" {
        return None;
    }
    event
        .get("thread_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
