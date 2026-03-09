use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::claude::{ClaudeInvocation, ClaudeOutputLine, ClaudeRuntime};
use crate::db::{ConversationEvent, Db, InteractionStatus, RunStatus};
use crate::events::emit;
use crate::runners::{Runner, RunnerOutcome, RunnerTurnContext};
use crate::tool::ToolKind;

#[derive(Clone)]
pub struct ClaudeRunner {
    pub runtime: ClaudeRuntime,
}

impl Runner for ClaudeRunner {
    fn tool(&self) -> ToolKind {
        ToolKind::ClaudeCode
    }

    fn run_turn<'a>(
        &'a self,
        ctx: RunnerTurnContext,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<RunnerOutcome>> + Send + 'a>> {
        Box::pin(async move { run_claude_turn(self.runtime.clone(), ctx).await })
    }
}

async fn run_claude_turn(runtime: ClaudeRuntime, ctx: RunnerTurnContext) -> anyhow::Result<RunnerOutcome> {
    let RunnerTurnContext {
        db,
        event_tx,
        conversation_id,
        project_root,
        tool_session_id,
        prompt,
        ws_clients,
        interaction_timeout_ms,
        interaction_default_action,
    } = ctx;

    let assistant_text = Arc::new(tokio::sync::Mutex::new(String::new()));
    let assistant_text_for_events = assistant_text.clone();

    let outcome = crate::claude::run_jsonl_events_with_input(
        runtime,
        ClaudeInvocation {
            project_root: project_root.clone(),
            session_id: tool_session_id,
            prompt,
        },
        |line| {
            let db = db.clone();
            let event_tx = event_tx.clone();
            let assistant_text = assistant_text_for_events.clone();
            async move {
                match line {
                    ClaudeOutputLine::Json(v) => {
                        emit(&db, &event_tx, conversation_id, "claude_event", &v).await?;

                        match v.get("type").and_then(|t| t.as_str()) {
                            Some("assistant_message_delta") => {
                                if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                    let mut buf = assistant_text.lock().await;
                                    buf.push_str(delta);
                                }
                            }
                            Some("assistant_message") | Some("assistant_message_completed") => {
                                if let Some(text) = v.get("text").and_then(|d| d.as_str()) {
                                    let mut buf = assistant_text.lock().await;
                                    buf.clear();
                                    buf.push_str(text);
                                }
                            }
                            _ => {}
                        }
                    }
                    ClaudeOutputLine::OutputLine(text) => {
                        emit(
                            &db,
                            &event_tx,
                            conversation_id,
                            "claude_event",
                            &serde_json::json!({ "type": "claude.output_line", "text": text }),
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

    // NOTE: We only support a single derived `agent_message` per turn for now. Streaming is
    // expected to come from raw `claude_event` deltas in the UI.
    let final_text = assistant_text.lock().await.clone();
    if !final_text.is_empty() {
        emit(
            &db,
            &event_tx,
            conversation_id,
            "agent_message",
            &serde_json::json!({ "text": final_text }),
        )
        .await?;
    }

    Ok(RunnerOutcome {
        tool_session_id: outcome.session_id,
    })
}

async fn handle_interaction_if_needed(
    db: &Db,
    tx: &broadcast::Sender<ConversationEvent>,
    conversation_id: Uuid,
    event: &Value,
    ws_clients: &Arc<AtomicUsize>,
    timeout_ms: i64,
    default_action: &str,
) -> anyhow::Result<Option<String>> {
    let typ = event.get("type").and_then(|t| t.as_str());
    if typ != Some("interaction_request") {
        return Ok(None);
    }

    let raw_kind = event
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("interaction");
    let kind = normalize_kind(raw_kind);

    let payload = serde_json::json!({
        "tool": "claude-code",
        "prompt": event.get("prompt"),
        "detail": event.get("detail"),
        "choices": event.get("choices"),
        "default_choice_id": event.get("default_choice_id"),
        "raw": event,
    });

    let request = db
        .create_interaction_request(conversation_id, &kind, &payload, timeout_ms, default_action)
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
        return auto_resolve_interaction(
            db,
            tx,
            conversation_id,
            request.id,
            &request.kind,
            default_action,
        )
        .await;
    }

    db.set_run_status(conversation_id, RunStatus::WaitingForInteraction)
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

        if let Some(current) = db.get_interaction_request(request.id).await?
            && current.status == InteractionStatus::Resolved
        {
            db.set_run_status(conversation_id, RunStatus::Running).await?;
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

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    db.set_run_status(conversation_id, RunStatus::Running).await?;
    emit(
        db,
        tx,
        conversation_id,
        "run_status",
        &serde_json::json!({ "status": "running" }),
    )
    .await?;

    auto_resolve_interaction(
        db,
        tx,
        conversation_id,
        request.id,
        &request.kind,
        default_action,
    )
    .await
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

fn normalize_kind(raw_kind: &str) -> String {
    if raw_kind.starts_with("claude.") {
        return raw_kind.to_string();
    }

    match raw_kind {
        "confirm" => "claude.confirm".to_string(),
        "permission.exec" => "claude.permission.exec".to_string(),
        "permission.write" => "claude.permission.write".to_string(),
        "select" => "claude.select".to_string(),
        "input" => "claude.input".to_string(),
        other => format!("claude.{other}"),
    }
}

fn response_to_stdin(kind: &str, response: Option<&Value>) -> Option<String> {
    if kind == "claude.input" {
        return response
            .and_then(|r| r.get("text"))
            .and_then(|v| v.as_str())
            .map(|s| format!("{s}\n"))
            .or_else(|| Some("\n".to_string()));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emits_agent_message_from_deltas() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("claude-runner.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let convo = db
            .create_conversation(Some(project.id), "c", ToolKind::ClaudeCode)
            .await?;
        db.try_mark_run_running(convo.id).await?;

        let (event_tx, mut event_rx) = broadcast::channel(32);
        let stub_events = vec![
            serde_json::json!({ "type": "session_configured", "session_id": "sess_1" }),
            serde_json::json!({ "type": "assistant_message_delta", "delta": "hello" }),
            serde_json::json!({ "type": "assistant_message_delta", "delta": " world" }),
        ];

        let runner = ClaudeRunner {
            runtime: ClaudeRuntime::stub(stub_events),
        };

        let outcome = runner
            .run_turn(RunnerTurnContext {
                db: db.clone(),
                event_tx: event_tx.clone(),
                conversation_id: convo.id,
                project_root: temp_dir.path().to_path_buf(),
                tool_session_id: None,
                prompt: "hi".to_string(),
                ws_clients: Arc::new(AtomicUsize::new(0)),
                interaction_timeout_ms: 30_000,
                interaction_default_action: "decline".to_string(),
            })
            .await?;

        assert_eq!(outcome.tool_session_id.as_deref(), Some("sess_1"));

        let mut saw_agent = false;
        for _ in 0..10 {
            if let Ok(e) =
                tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await
            {
                let e = e?;
                if e.event_type == "agent_message" {
                    saw_agent = true;
                    break;
                }
            }
        }
        assert!(saw_agent, "expected agent_message event");

        Ok(())
    }

    #[tokio::test]
    async fn auto_resolves_interaction_when_no_clients() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("claude-runner-interaction.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let convo = db
            .create_conversation(Some(project.id), "c", ToolKind::ClaudeCode)
            .await?;
        db.try_mark_run_running(convo.id).await?;

        let (event_tx, _event_rx) = broadcast::channel(64);
        let stub_events = vec![
            serde_json::json!({
                "type": "interaction_request",
                "kind": "confirm",
                "prompt": "Continue?",
            }),
            serde_json::json!({ "type": "assistant_message_delta", "delta": "ok" }),
        ];

        let runner = ClaudeRunner {
            runtime: ClaudeRuntime::stub(stub_events),
        };

        let _ = runner
            .run_turn(RunnerTurnContext {
                db: db.clone(),
                event_tx: event_tx.clone(),
                conversation_id: convo.id,
                project_root: temp_dir.path().to_path_buf(),
                tool_session_id: None,
                prompt: "hi".to_string(),
                ws_clients: Arc::new(AtomicUsize::new(0)),
                interaction_timeout_ms: 1_000,
                interaction_default_action: "decline".to_string(),
            })
            .await?;

        let pending = db.list_pending_interactions(convo.id).await?;
        assert_eq!(pending.len(), 0);

        let events = db.list_events_after(convo.id, 0, 1000).await?;
        assert!(events.iter().any(|e| e.event_type == "interaction_request"));
        assert!(
            events
                .iter()
                .any(|e| e.event_type == "interaction_response")
        );

        Ok(())
    }
}
