use std::path::PathBuf;

use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::codex::{CodexInvocation, CodexRuntime};
use crate::db::{ConversationEvent, Db, RunStatus};

#[derive(Clone)]
pub struct TurnContext {
    pub db: Db,
    pub event_tx: broadcast::Sender<ConversationEvent>,
    pub codex: CodexRuntime,
    pub conversation_id: Uuid,
    pub project_root: PathBuf,
    pub session_id: Option<String>,
    pub prompt: String,
}

pub async fn run_turn(ctx: TurnContext) {
    let conversation_id = ctx.conversation_id;
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
    } = ctx;

    let outcome = crate::codex::run_jsonl_events(
        codex,
        CodexInvocation {
            project_root,
            session_id,
            prompt,
        },
        |event| {
            let db = db.clone();
            let event_tx = event_tx.clone();
            async move {
                // Always persist raw codex events for debugging/auditing.
                emit(&db, &event_tx, conversation_id, "codex_event", &event).await?;

                // Derive agent_message events from codex items.
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

                Ok::<(), anyhow::Error>(())
            }
        },
    )
    .await?;

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

fn agent_message_text(event: &Value) -> Option<String> {
    let t = event.get("type")?.as_str()?;
    if t != "item.completed" {
        return None;
    }
    let item = event.get("item")?.as_object()?;
    let item_type = item.get("type")?.as_str()?;
    if item_type != "agent_message" {
        return None;
    }
    item.get("text").and_then(|v| v.as_str()).map(str::to_string)
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
            serde_json::json!({"type":"thread.started","thread_id":"00000000-0000-0000-0000-000000000001"}),
            serde_json::json!({"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}),
        ];

        run_turn(TurnContext {
            db: db.clone(),
            event_tx: event_tx.clone(),
            codex: CodexRuntime::stub(stub_events),
            conversation_id: convo.id,
            project_root: temp_dir.path().to_path_buf(),
            session_id: None,
            prompt: "hi".to_string(),
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
}
