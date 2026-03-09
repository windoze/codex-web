use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::db::{
    Conversation, ConversationEvent, ConversationListItem, InteractionRequest, Project, Run,
};
use crate::server::AppState;
use crate::tool::ToolKind;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create_project).get(list_projects))
        .route(
            "/conversations",
            post(create_conversation).get(list_conversations),
        )
        .route("/fs/home", get(fs_home))
        .route("/fs/list", get(fs_list))
        .route(
            "/conversations/:conversation_id",
            get(get_conversation).patch(update_conversation),
        )
        .route(
            "/conversations/:conversation_id/export",
            get(export_conversation),
        )
        .route(
            "/conversations/:conversation_id/interactions",
            get(list_pending_interactions),
        )
        .route(
            "/conversations/:conversation_id/events",
            get(list_conversation_events),
        )
        .route(
            "/conversations/:conversation_id/messages",
            post(post_user_message),
        )
        .route(
            "/interactions/:interaction_id/respond",
            post(respond_interaction),
        )
        .route("/interactions/pending", get(list_all_pending_interactions))
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

#[derive(Debug, Serialize)]
struct FsHomeResponse {
    path: String,
}

async fn fs_home(State(_state): State<AppState>) -> Result<Json<FsHomeResponse>, ApiError> {
    let home = home_dir();
    Ok(Json(FsHomeResponse {
        path: home.to_string_lossy().to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct FsListQuery {
    path: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FsEntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Serialize)]
struct FsEntry {
    name: String,
    path: String,
    kind: FsEntryKind,
}

#[derive(Debug, Serialize)]
struct FsListResponse {
    path: String,
    parent: Option<String>,
    entries: Vec<FsEntry>,
}

async fn fs_list(
    State(_state): State<AppState>,
    Query(q): Query<FsListQuery>,
) -> Result<Json<FsListResponse>, ApiError> {
    let raw = q.path.as_deref().unwrap_or("~");
    let path = expand_user_path(raw);
    if !path.is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }

    let md = tokio::fs::metadata(&path)
        .await
        .map_err(|e| ApiError::bad_request(format!("invalid path: {e}")))?;
    if !md.is_dir() {
        return Err(ApiError::bad_request("path must be a directory"));
    }

    let parent = path.parent().map(|p| p.to_string_lossy().to_string());

    let mut dir = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| ApiError::bad_request(format!("failed to read directory: {e}")))?;

    let mut entries: Vec<FsEntry> = Vec::new();
    loop {
        let entry = dir
            .next_entry()
            .await
            .map_err(|e| ApiError::bad_request(format!("failed to read directory: {e}")))?;
        let Some(entry) = entry else {
            break;
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let full_path = entry.path();
        let kind = match entry.file_type().await {
            Ok(ft) if ft.is_dir() => FsEntryKind::Dir,
            Ok(ft) if ft.is_file() => FsEntryKind::File,
            Ok(ft) if ft.is_symlink() => FsEntryKind::Symlink,
            Ok(_) => FsEntryKind::Other,
            Err(_) => FsEntryKind::Other,
        };

        entries.push(FsEntry {
            name,
            path: full_path.to_string_lossy().to_string(),
            kind,
        });
        if entries.len() > 5000 {
            break;
        }
    }

    entries.sort_by(|a, b| {
        let rank = |k: &FsEntryKind| match k {
            FsEntryKind::Dir => 0,
            FsEntryKind::Symlink => 1,
            FsEntryKind::File => 2,
            FsEntryKind::Other => 3,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(FsListResponse {
        path: path.to_string_lossy().to_string(),
        parent,
        entries,
    }))
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    root_path: String,
    name: Option<String>,
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<Project>, ApiError> {
    let root = PathBuf::from(&req.root_path);
    let md = tokio::fs::metadata(&root)
        .await
        .map_err(|e| ApiError::bad_request(format!("invalid root_path: {e}")))?;
    if !md.is_dir() {
        return Err(ApiError::bad_request("root_path must be a directory"));
    }

    let name = match req.name {
        Some(n) if !n.trim().is_empty() => n,
        _ => root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Project")
            .to_string(),
    };

    let project = state
        .db
        .create_project(&name, &root)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(project))
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<Project>>, ApiError> {
    let projects = state.db.list_projects().await.map_err(ApiError::internal)?;
    Ok(Json(projects))
}

#[derive(Debug, Deserialize)]
struct CreateConversationRequest {
    project_id: Option<Uuid>,
    title: Option<String>,
    tool: Option<ToolKind>,
}

async fn create_conversation(
    State(state): State<AppState>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<Json<Conversation>, ApiError> {
    let title = req
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("New conversation");
    let tool = req.tool.unwrap_or_default();

    let conversation = state
        .db
        .create_conversation(req.project_id, title, tool)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(conversation))
}

async fn list_conversations(
    State(state): State<AppState>,
) -> Result<Json<Vec<ConversationListItem>>, ApiError> {
    let conversations = state
        .db
        .list_conversation_list_items()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(conversations))
}

#[derive(Debug, Serialize)]
struct ConversationWithRun {
    conversation: Conversation,
    run: Run,
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<ConversationWithRun>, ApiError> {
    let conversation = state
        .db
        .get_conversation_optional(conversation_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;

    let run = state
        .db
        .get_run(conversation_id)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(ConversationWithRun { conversation, run }))
}

#[derive(Debug, Deserialize)]
struct UpdateConversationRequest {
    title: Option<String>,
    archived: Option<bool>,
}

async fn update_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(req): Json<UpdateConversationRequest>,
) -> Result<Json<Conversation>, ApiError> {
    let existing = state
        .db
        .get_conversation_optional(conversation_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;

    if let Some(title) = req.title.as_deref() {
        let title = title.trim();
        if title.is_empty() {
            return Err(ApiError::bad_request("title must not be empty"));
        }
        state
            .db
            .update_conversation_title(conversation_id, title)
            .await
            .map_err(ApiError::internal)?;
    }

    if let Some(archived) = req.archived {
        state
            .db
            .set_conversation_archived(conversation_id, archived)
            .await
            .map_err(ApiError::internal)?;
    }

    let updated = state
        .db
        .get_conversation(conversation_id)
        .await
        .map_err(ApiError::internal)?;

    if updated.title != existing.title || updated.archived_at_ms != existing.archived_at_ms {
        let ev = state
            .db
            .append_event(
                conversation_id,
                "conversation_updated",
                &json!({
                    "title": updated.title,
                    "archived_at_ms": updated.archived_at_ms,
                }),
            )
            .await
            .map_err(ApiError::internal)?;
        let _ = state.event_tx.send(ev);
    }

    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

async fn export_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let conversation = state
        .db
        .get_conversation_optional(conversation_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;

    let events = state
        .db
        .list_events_after(conversation_id, 0, 100_000)
        .await
        .map_err(ApiError::internal)?;

    let format = q.format.as_deref().unwrap_or("json");
    match format {
        "json" => {
            Ok(Json(json!({ "conversation": conversation, "events": events })).into_response())
        }
        "md" | "markdown" => {
            let mut out = String::new();
            out.push_str("# ");
            out.push_str(&conversation.title);
            out.push('\n');
            out.push('\n');

            for e in events {
                match e.event_type.as_str() {
                    "user_message" => {
                        if let Some(text) = e.payload.get("text").and_then(|v| v.as_str()) {
                            out.push_str("## User\n\n");
                            out.push_str(text);
                            out.push_str("\n\n");
                        }
                    }
                    "agent_message" => {
                        if let Some(text) = e.payload.get("text").and_then(|v| v.as_str()) {
                            out.push_str("## Assistant\n\n");
                            out.push_str(text);
                            out.push_str("\n\n");
                        }
                    }
                    _ => {}
                }
            }

            Ok((
                StatusCode::OK,
                [("content-type", "text/markdown; charset=utf-8")],
                out,
            )
                .into_response())
        }
        _ => Err(ApiError::bad_request(
            "unknown export format (use json or md)",
        )),
    }
}

#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

async fn list_conversation_events(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Query(q): Query<ListEventsQuery>,
) -> Result<Json<Vec<ConversationEvent>>, ApiError> {
    let after_id = q.after.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(1000).clamp(1, 5000);
    let events = state
        .db
        .list_events_after(conversation_id, after_id, limit)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(events))
}

async fn list_pending_interactions(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Vec<InteractionRequest>>, ApiError> {
    let pending = state
        .db
        .list_pending_interactions(conversation_id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(pending))
}

#[derive(Debug, Deserialize)]
struct PostMessageRequest {
    text: String,
}

async fn post_user_message(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(req): Json<PostMessageRequest>,
) -> Result<Json<ConversationEvent>, ApiError> {
    if req.text.trim().is_empty() {
        return Err(ApiError::bad_request("message text must not be empty"));
    }

    let conversation = state
        .db
        .get_conversation_optional(conversation_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;

    let project_id = conversation
        .project_id
        .ok_or_else(|| ApiError::bad_request("conversation has no project"))?;
    let project = state
        .db
        .get_project_optional(project_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::bad_request("project not found"))?;

    // Non-reentrant: only one turn per conversation may run at a time.
    let marked = state
        .db
        .try_mark_run_running(conversation_id)
        .await
        .map_err(ApiError::internal)?;
    if !marked {
        return Err(ApiError::conflict(
            "conversation is already running; wait for it to finish",
        ));
    }

    let prompt = req.text;
    let user_text = prompt.clone();

    let event = state
        .db
        .append_event(
            conversation_id,
            "user_message",
            &json!({ "text": user_text }),
        )
        .await
        .map_err(ApiError::internal)?;
    let _ = state.event_tx.send(event.clone());

    let running = state
        .db
        .append_event(
            conversation_id,
            "run_status",
            &json!({ "status": "running" }),
        )
        .await
        .map_err(ApiError::internal)?;
    let _ = state.event_tx.send(running);

    let run = state
        .db
        .get_run(conversation_id)
        .await
        .map_err(ApiError::internal)?;

    let runner = state.runners.for_tool(conversation.tool);
    let ctx = crate::orchestrator::TurnContext {
        db: state.db.clone(),
        event_tx: state.event_tx.clone(),
        runner,
        conversation_id,
        project_root: PathBuf::from(project.root_path),
        tool_session_id: run.tool_session_id,
        prompt,
        ws_clients: state.ws_clients.clone(),
        interaction_timeout_ms: state.interaction_timeout_ms,
        interaction_default_action: state.interaction_default_action.clone(),
        run_semaphore: state.run_semaphore.clone(),
        on_turn_finished_command: state.on_turn_finished_command.clone(),
    };

    tokio::spawn(async move {
        crate::orchestrator::run_turn(ctx).await;
    });

    Ok(Json(event))
}

#[derive(Debug, Deserialize)]
struct RespondInteractionRequest {
    action: String,
    text: Option<String>,
}

async fn respond_interaction(
    State(state): State<AppState>,
    Path(interaction_id): Path<Uuid>,
    Json(req): Json<RespondInteractionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let interaction = state
        .db
        .get_interaction_request(interaction_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("interaction not found"))?;

    let response = json!({
        "action": req.action,
        "text": req.text,
    });

    let resolved = state
        .db
        .try_resolve_interaction(interaction_id, &response, "web")
        .await
        .map_err(ApiError::internal)?;
    if !resolved {
        return Err(ApiError::conflict("interaction already resolved"));
    }

    let response_event = state
        .db
        .append_event(
            interaction.conversation_id,
            "interaction_response",
            &json!({
                "interaction_id": interaction_id,
                "kind": interaction.kind,
                "response": response,
                "resolved_by": "web",
            }),
        )
        .await
        .map_err(ApiError::internal)?;
    let _ = state.event_tx.send(response_event);

    Ok(Json(json!({"status":"ok"})))
}

async fn list_all_pending_interactions(
    State(state): State<AppState>,
) -> Result<Json<Vec<InteractionRequest>>, ApiError> {
    let pending = state
        .db
        .list_all_pending_interactions()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(pending))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }

    fn internal(err: anyhow::Error) -> Self {
        tracing::error!(error = ?err, "api error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
