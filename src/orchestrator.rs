use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use tokio::sync::{Semaphore, broadcast, watch};
use tokio::time::timeout;
use uuid::Uuid;

use crate::db::{ConversationEvent, Db, Project, RunStatus};
use crate::events::emit;
use crate::runners::Runner;

#[derive(Debug, thiserror::Error)]
#[error("turn cancelled")]
pub struct TurnCancelled;

#[derive(Clone)]
pub struct TurnContext {
    pub db: Db,
    pub event_tx: broadcast::Sender<ConversationEvent>,
    pub runner: Arc<dyn Runner>,
    pub conversation_id: Uuid,
    pub project_root: PathBuf,
    pub project: Project,
    pub tool_session_id: Option<String>,
    pub prompt: String,
    pub ws_clients: Arc<AtomicUsize>,
    pub interaction_timeout_ms: i64,
    pub interaction_default_action: String,
    pub run_semaphore: Arc<Semaphore>,
    pub on_turn_finished_command: Option<String>,
    pub cancel_rx: watch::Receiver<bool>,
    pub turn_manager: crate::turns::TurnManager,
}

pub async fn run_turn(ctx: TurnContext) {
    let conversation_id = ctx.conversation_id;
    let project_root = ctx.project_root.clone();
    let on_turn_finished_command = ctx.on_turn_finished_command.clone();
    let turn_manager = ctx.turn_manager.clone();
    let db = ctx.db.clone();
    let event_tx = ctx.event_tx.clone();
    let tool = ctx.runner.tool();

    let result = run_turn_inner(ctx).await;
    turn_manager.unregister(conversation_id);

    if let Err(err) = result {
        tracing::error!(tool = %tool, error = ?err, "turn failed");

        let (status, payload) = if err.downcast_ref::<TurnCancelled>().is_some() {
            (
                RunStatus::Aborted,
                serde_json::json!({ "status": "aborted" }),
            )
        } else {
            (
                RunStatus::Failed,
                serde_json::json!({ "status": "failed", "error": err.to_string() }),
            )
        };

        let _ = db
            .resolve_all_pending_interactions(
                conversation_id,
                &serde_json::json!({ "action": "decline", "reason": "run_cancelled" }),
                "cancel",
            )
            .await;

        let _ = db
            .mark_run_completed(conversation_id, status.clone(), None, None)
            .await;
        let _ = emit(&db, &event_tx, conversation_id, "run_status", &payload).await;

        let tool_session_id = db
            .get_run(conversation_id)
            .await
            .ok()
            .and_then(|r| r.tool_session_id);
        spawn_turn_finished_hook(
            on_turn_finished_command.as_deref(),
            &project_root,
            conversation_id,
            status.clone(),
            tool_session_id.as_deref(),
        );
    }
}

async fn run_turn_inner(ctx: TurnContext) -> anyhow::Result<()> {
    let TurnContext {
        db,
        event_tx,
        runner,
        conversation_id,
        project_root,
        project,
        tool_session_id,
        prompt,
        ws_clients,
        interaction_timeout_ms,
        interaction_default_action,
        run_semaphore,
        on_turn_finished_command,
        mut cancel_rx,
        turn_manager: _turn_manager,
    } = ctx;

    if *cancel_rx.borrow() {
        return Err(TurnCancelled.into());
    }

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

            let permit = tokio::select! {
                p = run_semaphore.clone().acquire_owned() => {
                    p.map_err(|_| anyhow::anyhow!("run semaphore closed"))?
                }
                res = cancel_rx.changed() => {
                    let _ = res;
                    return Err(TurnCancelled.into());
                }
            };

            if *cancel_rx.borrow() {
                return Err(TurnCancelled.into());
            }

            db.set_run_status(conversation_id, crate::db::RunStatus::Running)
                .await?;
            emit(
                &db,
                &event_tx,
                conversation_id,
                "run_status",
                &serde_json::json!({ "status": "running" }),
            )
            .await?;

            permit
        }
    };

    let run_fut = runner.run_turn(crate::runners::RunnerTurnContext {
            db: db.clone(),
            event_tx: event_tx.clone(),
            conversation_id,
            project_root: project_root.clone(),
            project: project.clone(),
            tool_session_id,
            prompt,
            ws_clients: ws_clients.clone(),
            interaction_timeout_ms,
            interaction_default_action: interaction_default_action.clone(),
        });
    tokio::pin!(run_fut);

    let outcome = tokio::select! {
        res = &mut run_fut => res?,
        res = cancel_rx.changed() => {
            let _ = res;
            return Err(TurnCancelled.into());
        }
    };

    drop(permit);

    if *cancel_rx.borrow() {
        return Err(TurnCancelled.into());
    }

    db.mark_run_completed(
        conversation_id,
        RunStatus::Completed,
        outcome.tool_session_id.as_deref(),
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
        outcome.tool_session_id.as_deref(),
    );

    Ok(())
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
    project_root: &Path,
    conversation_id: Uuid,
    status: RunStatus,
    tool_session_id: Option<&str>,
) {
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        return;
    };

    let command = command.to_string();
    let cwd = project_root.to_path_buf();
    let conversation_id = conversation_id.to_string();
    let status_str = run_status_str(status).to_string();
    let session_id = tool_session_id.unwrap_or("").to_string();

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
        cmd.env("CODEX_WEB_PROJECT_ROOT", cwd.to_string_lossy().to_string());
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
