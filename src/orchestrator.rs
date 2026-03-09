use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use uuid::Uuid;

use crate::codex::{CodexInvocation, CodexOutputLine, CodexRuntime};
use crate::db::{ConversationEvent, Db, RunStatus};
use crate::protocol::event_msg::EventMsg;

#[derive(Clone)]
pub struct TurnContext {
    pub db: Db,
    pub event_tx: broadcast::Sender<ConversationEvent>,
    pub codex: CodexRuntime,
    pub conversation_id: Uuid,
    pub project_root: PathBuf,
    pub session_id: Option<String>,
    pub prompt: String,
    pub ws_clients: Arc<AtomicUsize>,
    pub interaction_timeout_ms: i64,
    pub interaction_default_action: String,
    pub run_semaphore: Arc<Semaphore>,
    pub on_turn_finished_command: Option<String>,
}

pub async fn run_turn(ctx: TurnContext) {
    let conversation_id = ctx.conversation_id;
    let project_root = ctx.project_root.clone();
    let on_turn_finished_command = ctx.on_turn_finished_command.clone();
    let db = ctx.db.clone();
    let event_tx = ctx.event_tx.clone();

    if let Err(err) = run_turn_inner(ctx).await {
        tracing::error!(error = ?err, "codex turn failed");
        let _ = db
            .mark_run_completed(conversation_id, RunStatus::Failed, None, None)
            .await;
        let _ = emit(
            &db,
            &event_tx,
            conversation_id,
            "run_status",
            &serde_json::json!({ "status": "failed", "error": err.to_string() }),
        )
        .await;
        spawn_turn_finished_hook(
            on_turn_finished_command.as_deref(),
            &project_root,
            conversation_id,
            RunStatus::Failed,
            None,
        );
    }
}

async fn run_turn_inner(ctx: TurnContext) -> anyhow::Result<()> {
    let TurnContext {
        db,
        event_tx,
        codex,
        conversation_id,
        project_root,
        session_id,
        prompt,
        ws_clients,
        interaction_timeout_ms,
        interaction_default_action,
        run_semaphore,
        on_turn_finished_command,
    } = ctx;

    // Global concurrency limit across conversations.
    let permit = match run_semaphore.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            db.set_run_status(conversation_id, crate::db::RunStatus::Queued)
                .await?;
            emit(
                &db,
                &event_tx,
                conversation_id,
                "run_status",
                &serde_json::json!({ "status": "queued" }),
            )
            .await?;
            run_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| anyhow::anyhow!("run semaphore closed"))?
        }
    };

    let outcome = crate::codex::run_jsonl_events_with_input(
        codex,
        CodexInvocation {
            project_root: project_root.clone(),
            session_id,
            prompt,
        },
        |line| {
            let db = db.clone();
            let event_tx = event_tx.clone();
            async move {
                match line {
                    CodexOutputLine::Event(event) => {
                        let raw = serde_json::to_value(&event)?;
                        emit(&db, &event_tx, conversation_id, "codex_event", &raw).await?;

                        if let Some(text) = agent_message_text(&event) {
                            emit(
                                &db,
                                &event_tx,
                                conversation_id,
                                "agent_message",
                                &serde_json::json!({ "text": text }),
                            )
                            .await?;
                        }
                    }
                    CodexOutputLine::UnknownJson(raw) => {
                        emit(&db, &event_tx, conversation_id, "codex_event", &raw).await?;
                    }
                    CodexOutputLine::OutputLine(text) => {
                        emit(
                            &db,
                            &event_tx,
                            conversation_id,
                            "codex_event",
                            &serde_json::json!({ "type": "codex.output_line", "text": text }),
                        )
                        .await?;
                    }
                }

                Ok::<(), anyhow::Error>(())
            }
        },
        |event| {
            let db = db.clone();
            let event_tx = event_tx.clone();
            let ws_clients = ws_clients.clone();
            let interaction_default_action = interaction_default_action.clone();
            let event = event.clone();
            async move {
                handle_interaction_if_needed(
                    &db,
                    &event_tx,
                    conversation_id,
                    &event,
                    &ws_clients,
                    interaction_timeout_ms,
                    &interaction_default_action,
                )
                .await
            }
        },
    )
    .await?;

    drop(permit);

    db.mark_run_completed(
        conversation_id,
        RunStatus::Completed,
        outcome.session_id.as_deref(),
        None,
    )
    .await?;

    emit(
        &db,
        &event_tx,
        conversation_id,
        "run_status",
        &serde_json::json!({ "status": "completed" }),
    )
    .await?;

    spawn_turn_finished_hook(
        on_turn_finished_command.as_deref(),
        &project_root,
        conversation_id,
        RunStatus::Completed,
        outcome.session_id.as_deref(),
    );

    Ok(())
}

async fn emit(
    db: &Db,
    tx: &broadcast::Sender<ConversationEvent>,
    conversation_id: Uuid,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<ConversationEvent> {
    let e = db.append_event(conversation_id, event_type, payload).await?;
    let _ = tx.send(e.clone());
    Ok(e)
}

fn run_status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Idle => "idle",
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Aborted => "aborted",
        RunStatus::WaitingForInteraction => "waiting_for_interaction",
    }
}

fn spawn_turn_finished_hook(
    command: Option<&str>,
    project_root: &PathBuf,
    conversation_id: Uuid,
    status: RunStatus,
    codex_session_id: Option<&str>,
) {
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        return;
    };

    let command = command.to_string();
    let cwd = project_root.clone();
    let conversation_id = conversation_id.to_string();
    let status_str = run_status_str(status).to_string();
    let session_id = codex_session_id.unwrap_or("").to_string();

    tokio::spawn(async move {
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-lc").arg(command);
            c
        };

        cmd.current_dir(&cwd);
        cmd.env("CODEX_WEB_CONVERSATION_ID", &conversation_id);
        cmd.env(
            "CODEX_WEB_PROJECT_ROOT",
            cwd.to_string_lossy().to_string(),
        );
        cmd.env("CODEX_WEB_RUN_STATUS", &status_str);
        cmd.env("CODEX_WEB_CODEX_SESSION_ID", &session_id);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(error = ?err, "failed to spawn on-turn-finished command");
                return;
            }
        };

        match timeout(Duration::from_secs(30), child.wait()).await {
            Ok(Ok(exit)) => {
                if exit.success() {
                    tracing::info!("on-turn-finished command completed");
                } else {
                    tracing::warn!(exit_code = ?exit.code(), "on-turn-finished command failed");
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(error = ?err, "on-turn-finished command wait failed");
            }
            Err(_) => {
                let _ = child.kill().await;
                tracing::warn!("on-turn-finished command timed out (killed after 30s)");
            }
        }
    });
}

fn agent_message_text(event: &EventMsg) -> Option<String> {
    use crate::protocol::event_msg::{AgentMessageContent, EventMsg as M, TurnItem};

    let item = match event {
        M::ItemCompleted { item, .. } => item,
        _ => return None,
    };

    let TurnItem::AgentMessage { content, .. } = item else {
        return None;
    };

    let mut out = String::new();
    for part in content {
        match part {
            AgentMessageContent::Text(text_part) => out.push_str(text_part),
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

async fn handle_interaction_if_needed(
    db: &Db,
    tx: &broadcast::Sender<ConversationEvent>,
    conversation_id: Uuid,
    event: &EventMsg,
    ws_clients: &Arc<AtomicUsize>,
    timeout_ms: i64,
    default_action: &str,
) -> anyhow::Result<Option<String>> {
    use crate::protocol::event_msg::EventMsg as M;

    let kind = match event {
        M::ExecApprovalRequest { .. } => "exec_approval_request",
        M::ApplyPatchApprovalRequest { .. } => "apply_patch_approval_request",
        M::ElicitationRequest { .. } => "elicitation_request",
        _ => return Ok(None),
    };

    let payload = serde_json::to_value(event)?;

    let request = db
        .create_interaction_request(conversation_id, kind, &payload, timeout_ms, default_action)
        .await?;

    emit(
        db,
        tx,
        conversation_id,
        "interaction_request",
        &serde_json::json!({
            "interaction_id": request.id,
            "kind": request.kind,
            "timeout_ms": request.timeout_ms,
            "default_action": request.default_action,
            "payload": request.payload,
        }),
    )
    .await?;

    let user_present = ws_clients.load(Ordering::Relaxed) > 0;
    if !user_present {
        return auto_resolve_interaction(db, tx, conversation_id, request.id, &request.kind, default_action).await;
    }

    db.set_run_status(conversation_id, crate::db::RunStatus::WaitingForInteraction)
        .await?;
    emit(
        db,
        tx,
        conversation_id,
        "run_status",
        &serde_json::json!({ "status": "waiting_for_interaction" }),
    )
    .await?;

    // Poll for a user-provided response (web or terminal), then fall back to default on timeout.
    let start = tokio::time::Instant::now();
    loop {
        if start.elapsed().as_millis() as i64 >= timeout_ms {
            break;
        }

        if let Some(current) = db.get_interaction_request(request.id).await? {
            if current.status == crate::db::InteractionStatus::Resolved {
                db.set_run_status(conversation_id, crate::db::RunStatus::Running)
                    .await?;
                emit(
                    db,
                    tx,
                    conversation_id,
                    "run_status",
                    &serde_json::json!({ "status": "running" }),
                )
                .await?;
                return Ok(response_to_stdin(&current.kind, current.response.as_ref()));
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    db.set_run_status(conversation_id, crate::db::RunStatus::Running)
        .await?;
    emit(
        db,
        tx,
        conversation_id,
        "run_status",
        &serde_json::json!({ "status": "running" }),
    )
    .await?;

    auto_resolve_interaction(db, tx, conversation_id, request.id, &request.kind, default_action).await
}

async fn auto_resolve_interaction(
    db: &Db,
    tx: &broadcast::Sender<ConversationEvent>,
    conversation_id: Uuid,
    interaction_id: Uuid,
    kind: &str,
    default_action: &str,
) -> anyhow::Result<Option<String>> {
    let response = serde_json::json!({ "action": default_action });
    let resolved = db
        .try_resolve_interaction(interaction_id, &response, "auto")
        .await?;

    if resolved {
        emit(
            db,
            tx,
            conversation_id,
            "interaction_response",
            &serde_json::json!({
                "interaction_id": interaction_id,
                "kind": kind,
                "response": response,
                "resolved_by": "auto",
            }),
        )
        .await?;
    }

    Ok(response_to_stdin(kind, Some(&response)))
}

fn response_to_stdin(kind: &str, response: Option<&Value>) -> Option<String> {
    match kind {
        "exec_approval_request" | "apply_patch_approval_request" => {
            let action = response
                .and_then(|r| r.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("decline");
            match action {
                "accept" => Some("y\n".to_string()),
                "decline" => Some("n\n".to_string()),
                _ => Some("n\n".to_string()),
            }
        }
        "elicitation_request" => response
            .and_then(|r| r.get("text"))
            .and_then(|v| v.as_str())
            .map(|s| format!("{s}\n")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn derives_agent_message_from_codex_item() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("orchestrator.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let convo = db.create_conversation(Some(project.id), "c").await?;
        db.try_mark_run_running(convo.id).await?;

        let (event_tx, mut event_rx) = broadcast::channel(32);
        let stub_events = vec![
            serde_json::json!({
                "type": "item_completed",
                "thread_id": "00000000-0000-0000-0000-000000000001",
                "turn_id": "turn_0",
                "item": {
                    "type": "AgentMessage",
                    "id": "item_0",
                    "content": [
                        { "type": "Text", "text": "hello" }
                    ]
                }
            }),
        ];

        run_turn(TurnContext {
            db: db.clone(),
            event_tx: event_tx.clone(),
            codex: CodexRuntime::stub(stub_events),
            conversation_id: convo.id,
            project_root: temp_dir.path().to_path_buf(),
            session_id: None,
            prompt: "hi".to_string(),
            ws_clients: Arc::new(AtomicUsize::new(0)),
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: Arc::new(Semaphore::new(1)),
            on_turn_finished_command: None,
        })
        .await;

        // We should see at least one agent_message event broadcast.
        let mut saw_agent = false;
        for _ in 0..10 {
            if let Ok(e) = tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
                let e = e?;
                if e.event_type == "agent_message" {
                    saw_agent = true;
                    break;
                }
            }
        }
        assert!(saw_agent, "expected agent_message event");

        let run = db.get_run(convo.id).await?;
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(
            run.codex_session_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );

        Ok(())
    }

    #[tokio::test]
    async fn auto_resolves_interaction_when_no_clients() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("orchestrator-interaction.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let convo = db.create_conversation(Some(project.id), "c").await?;
        db.try_mark_run_running(convo.id).await?;

        let (event_tx, _event_rx) = broadcast::channel(64);
        let stub_events = vec![
            serde_json::json!({
                "type": "exec_approval_request",
                "call_id": "call_1",
                "command": ["echo", "hi"],
                "cwd": ".",
                "parsed_cmd": []
            }),
            serde_json::json!({
                "type": "item_completed",
                "thread_id": "00000000-0000-0000-0000-000000000002",
                "turn_id": "turn_0",
                "item": {
                    "type": "AgentMessage",
                    "id": "item_0",
                    "content": [
                        { "type": "Text", "text": "ok" }
                    ]
                }
            }),
        ];

        run_turn(TurnContext {
            db: db.clone(),
            event_tx: event_tx.clone(),
            codex: CodexRuntime::stub(stub_events),
            conversation_id: convo.id,
            project_root: temp_dir.path().to_path_buf(),
            session_id: None,
            prompt: "hi".to_string(),
            ws_clients: Arc::new(AtomicUsize::new(0)),
            interaction_timeout_ms: 1_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: Arc::new(Semaphore::new(1)),
            on_turn_finished_command: None,
        })
        .await;

        let pending = db.list_pending_interactions(convo.id).await?;
        assert_eq!(pending.len(), 0);

        let events = db.list_events_after(convo.id, 0, 1000).await?;
        assert!(events.iter().any(|e| e.event_type == "interaction_request"));
        assert!(events.iter().any(|e| e.event_type == "interaction_response"));

        Ok(())
    }
}
