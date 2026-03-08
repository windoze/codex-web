use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::db::{Conversation, ConversationEvent, Project, Run};
use crate::server::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create_project).get(list_projects))
        .route("/conversations", post(create_conversation).get(list_conversations))
        .route(
            "/conversations/:conversation_id",
            get(get_conversation),
        )
        .route(
            "/conversations/:conversation_id/events",
            get(list_conversation_events),
        )
        .route(
            "/conversations/:conversation_id/messages",
            post(post_user_message),
        )
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

    let conversation = state
        .db
        .create_conversation(req.project_id, title)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(conversation))
}

async fn list_conversations(
    State(state): State<AppState>,
) -> Result<Json<Vec<Conversation>>, ApiError> {
    let conversations = state
        .db
        .list_conversations()
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
    // For now, we fetch from list and filter. Later we can add a direct DB query.
    let conversation = state
        .db
        .list_conversations()
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|c| c.id == conversation_id)
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;

    let run = state
        .db
        .get_run(conversation_id)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(ConversationWithRun { conversation, run }))
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

    // Milestone 1: only persist the user message and broadcast. Codex execution comes in Milestone 2.
    let event = state
        .db
        .append_event(
            conversation_id,
            "user_message",
            &json!({"text": req.text}),
        )
        .await
        .map_err(ApiError::internal)?;

    let _ = state.event_tx.send(event.clone());
    Ok(Json(event))
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

