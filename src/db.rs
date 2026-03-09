use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::tool::ToolKind;

#[derive(Clone, Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn connect(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create db dir {}", parent.display()))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .with_context(|| format!("connect sqlite {}", db_path.display()))?;

        sqlx::migrate!()
            .run(&pool)
            .await
            .context("run db migrations")?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn create_project(&self, name: &str, root_path: &Path) -> anyhow::Result<Project> {
        let now = now_ms();
        let root_path_str = root_path.to_string_lossy().to_string();

        if let Some(existing) = sqlx::query_as::<_, ProjectRow>(
            r#"
            SELECT id, name, root_path, created_at_ms, updated_at_ms
            FROM projects
            WHERE root_path = ?1
            "#,
        )
        .bind(&root_path_str)
        .fetch_optional(&self.pool)
        .await
        .context("lookup project by root_path")?
        {
            let existing_project = Project::from(existing);
            sqlx::query(
                r#"
                UPDATE projects
                SET name = ?2, updated_at_ms = ?3
                WHERE id = ?1
                "#,
            )
            .bind(existing_project.id.to_string())
            .bind(name)
            .bind(now)
            .execute(&self.pool)
            .await
            .context("update existing project metadata")?;

            return Ok(Project {
                updated_at_ms: now,
                name: name.to_owned(),
                ..existing_project
            });
        }

        let project = Project {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            root_path: root_path_str,
            created_at_ms: now,
            updated_at_ms: now,
        };

        sqlx::query(
            r#"
            INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(project.id.to_string())
        .bind(&project.name)
        .bind(&project.root_path)
        .bind(project.created_at_ms)
        .bind(project.updated_at_ms)
        .execute(&self.pool)
        .await
        .context("insert project")?;

        Ok(project)
    }

    pub async fn list_projects(&self) -> anyhow::Result<Vec<Project>> {
        let rows = sqlx::query_as::<_, ProjectRow>(
            r#"
            SELECT id, name, root_path, created_at_ms, updated_at_ms
            FROM projects
            ORDER BY updated_at_ms DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list projects")?;

        Ok(rows.into_iter().map(Project::from).collect())
    }

    pub async fn get_project(&self, project_id: Uuid) -> anyhow::Result<Project> {
        let row = sqlx::query_as::<_, ProjectRow>(
            r#"
            SELECT id, name, root_path, created_at_ms, updated_at_ms
            FROM projects
            WHERE id = ?1
            "#,
        )
        .bind(project_id.to_string())
        .fetch_one(&self.pool)
        .await
        .context("get project")?;
        Ok(Project::from(row))
    }

    pub async fn get_project_optional(&self, project_id: Uuid) -> anyhow::Result<Option<Project>> {
        let row = sqlx::query_as::<_, ProjectRow>(
            r#"
            SELECT id, name, root_path, created_at_ms, updated_at_ms
            FROM projects
            WHERE id = ?1
            "#,
        )
        .bind(project_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .context("get project (optional)")?;
        Ok(row.map(Project::from))
    }

    pub async fn create_conversation(
        &self,
        project_id: Option<Uuid>,
        title: &str,
        tool: ToolKind,
    ) -> anyhow::Result<Conversation> {
        let now = now_ms();
        let conversation = Conversation {
            id: Uuid::new_v4(),
            project_id,
            title: title.to_owned(),
            tool,
            created_at_ms: now,
            updated_at_ms: now,
            archived_at_ms: None,
        };

        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, title, tool, created_at_ms, updated_at_ms, archived_at_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
            "#,
        )
        .bind(conversation.id.to_string())
        .bind(project_id.map(|p| p.to_string()))
        .bind(&conversation.title)
        .bind(conversation.tool.as_str())
        .bind(conversation.created_at_ms)
        .bind(conversation.updated_at_ms)
        .execute(&self.pool)
        .await
        .context("insert conversation")?;

        self.ensure_run_row(conversation.id).await?;

        Ok(conversation)
    }

    pub async fn list_conversations(&self) -> anyhow::Result<Vec<Conversation>> {
        let rows = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, project_id, title, tool, created_at_ms, updated_at_ms, archived_at_ms
            FROM conversations
            WHERE archived_at_ms IS NULL
            ORDER BY updated_at_ms DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list conversations")?;

        Ok(rows.into_iter().map(Conversation::from).collect())
    }

    pub async fn list_conversation_list_items(&self) -> anyhow::Result<Vec<ConversationListItem>> {
        let rows = sqlx::query_as::<_, ConversationListRow>(
            r#"
            SELECT
              c.id,
              c.project_id,
              c.title,
              c.tool,
              c.created_at_ms,
              c.updated_at_ms,
              c.archived_at_ms,
              r.status AS run_status
            FROM conversations c
            LEFT JOIN runs r ON r.conversation_id = c.id
            WHERE c.archived_at_ms IS NULL
            ORDER BY c.updated_at_ms DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list conversation list items")?;

        Ok(rows.into_iter().map(ConversationListItem::from).collect())
    }

    pub async fn get_conversation(&self, conversation_id: Uuid) -> anyhow::Result<Conversation> {
        let row = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, project_id, title, tool, created_at_ms, updated_at_ms, archived_at_ms
            FROM conversations
            WHERE id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .fetch_one(&self.pool)
        .await
        .context("get conversation")?;
        Ok(Conversation::from(row))
    }

    pub async fn get_conversation_optional(
        &self,
        conversation_id: Uuid,
    ) -> anyhow::Result<Option<Conversation>> {
        let row = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, project_id, title, tool, created_at_ms, updated_at_ms, archived_at_ms
            FROM conversations
            WHERE id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .context("get conversation (optional)")?;
        Ok(row.map(Conversation::from))
    }

    pub async fn update_conversation_title(
        &self,
        conversation_id: Uuid,
        title: &str,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            r#"
            UPDATE conversations
            SET title = ?2, updated_at_ms = ?3
            WHERE id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(title)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("update conversation title")?;
        Ok(())
    }

    pub async fn set_conversation_archived(
        &self,
        conversation_id: Uuid,
        archived: bool,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        let archived_at_ms = if archived { Some(now) } else { None };
        sqlx::query(
            r#"
            UPDATE conversations
            SET archived_at_ms = ?2, updated_at_ms = ?3
            WHERE id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(archived_at_ms)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("archive conversation")?;
        Ok(())
    }

    pub async fn append_event(
        &self,
        conversation_id: Uuid,
        event_type: &str,
        payload: &Value,
    ) -> anyhow::Result<ConversationEvent> {
        let now = now_ms();
        let payload_json = serde_json::to_string(payload).context("serialize event payload")?;

        let result = sqlx::query(
            r#"
            INSERT INTO conversation_events (conversation_id, ts_ms, type, payload_json)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(now)
        .bind(event_type)
        .bind(&payload_json)
        .execute(&self.pool)
        .await
        .context("insert conversation event")?;

        let id = result.last_insert_rowid();

        sqlx::query(
            r#"
            UPDATE conversations
            SET updated_at_ms = ?2
            WHERE id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .context("update conversation updated_at")?;

        Ok(ConversationEvent {
            id,
            conversation_id,
            ts_ms: now,
            event_type: event_type.to_owned(),
            payload: payload.clone(),
        })
    }

    pub async fn list_events_after(
        &self,
        conversation_id: Uuid,
        after_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<ConversationEvent>> {
        let rows = sqlx::query_as::<_, ConversationEventRow>(
            r#"
            SELECT id, conversation_id, ts_ms, type, payload_json
            FROM conversation_events
            WHERE conversation_id = ?1 AND id > ?2
            ORDER BY id ASC
            LIMIT ?3
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("list events")?;

        rows.into_iter()
            .map(ConversationEvent::try_from)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    pub async fn get_run(&self, conversation_id: Uuid) -> anyhow::Result<Run> {
        let row = sqlx::query_as::<_, RunRow>(
            r#"
            SELECT conversation_id, status, started_at_ms, ended_at_ms, tool_session_id, active_pid, metadata_json, updated_at_ms
            FROM runs
            WHERE conversation_id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .fetch_one(&self.pool)
        .await
        .context("get run")?;

        Run::try_from(row)
    }

    pub async fn try_mark_run_running(&self, conversation_id: Uuid) -> anyhow::Result<bool> {
        let now = now_ms();
        let result = sqlx::query(
            r#"
            UPDATE runs
            SET status = 'running', started_at_ms = ?2, ended_at_ms = NULL, updated_at_ms = ?2
            WHERE conversation_id = ?1 AND status IN ('idle', 'completed', 'failed', 'aborted')
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .context("mark run running")?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn set_run_status(
        &self,
        conversation_id: Uuid,
        status: RunStatus,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            r#"
            UPDATE runs
            SET status = ?2, updated_at_ms = ?3
            WHERE conversation_id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(status.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .context("set run status")?;
        Ok(())
    }

    pub async fn mark_run_completed(
        &self,
        conversation_id: Uuid,
        status: RunStatus,
        tool_session_id: Option<&str>,
        active_pid: Option<i64>,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        let status_str = status.as_str();
        sqlx::query(
            r#"
            UPDATE runs
            SET status = ?2,
                ended_at_ms = ?3,
                updated_at_ms = ?3,
                tool_session_id = COALESCE(?4, tool_session_id),
                active_pid = ?5
            WHERE conversation_id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(status_str)
        .bind(now)
        .bind(tool_session_id)
        .bind(active_pid)
        .execute(&self.pool)
        .await
        .context("mark run completed")?;
        Ok(())
    }

    pub async fn create_interaction_request(
        &self,
        conversation_id: Uuid,
        kind: &str,
        payload: &Value,
        timeout_ms: i64,
        default_action: &str,
    ) -> anyhow::Result<InteractionRequest> {
        let now = now_ms();
        let req = InteractionRequest {
            id: Uuid::new_v4(),
            conversation_id,
            kind: kind.to_string(),
            status: InteractionStatus::Pending,
            payload: payload.clone(),
            created_at_ms: now,
            timeout_ms,
            default_action: default_action.to_string(),
            resolved_at_ms: None,
            resolved_by: None,
            response: None,
        };

        let payload_json = serde_json::to_string(&req.payload).context("serialize payload")?;

        sqlx::query(
            r#"
            INSERT INTO interaction_requests
              (id, conversation_id, kind, status, payload_json, created_at_ms, timeout_ms, default_action, resolved_at_ms, resolved_by, response_json)
            VALUES
              (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL)
            "#,
        )
        .bind(req.id.to_string())
        .bind(req.conversation_id.to_string())
        .bind(&req.kind)
        .bind(req.status.as_str())
        .bind(payload_json)
        .bind(req.created_at_ms)
        .bind(req.timeout_ms)
        .bind(&req.default_action)
        .execute(&self.pool)
        .await
        .context("insert interaction_request")?;

        Ok(req)
    }

    pub async fn get_interaction_request(
        &self,
        request_id: Uuid,
    ) -> anyhow::Result<Option<InteractionRequest>> {
        let row = sqlx::query_as::<_, InteractionRequestRow>(
            r#"
            SELECT
              id, conversation_id, kind, status, payload_json, created_at_ms, timeout_ms,
              default_action, resolved_at_ms, resolved_by, response_json
            FROM interaction_requests
            WHERE id = ?1
            "#,
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .context("get interaction_request")?;

        match row {
            Some(row) => Ok(Some(InteractionRequest::try_from(row)?)),
            None => Ok(None),
        }
    }

    pub async fn list_pending_interactions(
        &self,
        conversation_id: Uuid,
    ) -> anyhow::Result<Vec<InteractionRequest>> {
        let rows = sqlx::query_as::<_, InteractionRequestRow>(
            r#"
            SELECT
              id, conversation_id, kind, status, payload_json, created_at_ms, timeout_ms,
              default_action, resolved_at_ms, resolved_by, response_json
            FROM interaction_requests
            WHERE conversation_id = ?1 AND status = 'pending'
            ORDER BY created_at_ms ASC
            "#,
        )
        .bind(conversation_id.to_string())
        .fetch_all(&self.pool)
        .await
        .context("list pending interactions")?;

        rows.into_iter()
            .map(InteractionRequest::try_from)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    pub async fn list_all_pending_interactions(&self) -> anyhow::Result<Vec<InteractionRequest>> {
        let rows = sqlx::query_as::<_, InteractionRequestRow>(
            r#"
            SELECT
              id, conversation_id, kind, status, payload_json, created_at_ms, timeout_ms,
              default_action, resolved_at_ms, resolved_by, response_json
            FROM interaction_requests
            WHERE status = 'pending'
            ORDER BY created_at_ms ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list all pending interactions")?;

        rows.into_iter()
            .map(InteractionRequest::try_from)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    pub async fn try_resolve_interaction(
        &self,
        request_id: Uuid,
        response: &Value,
        resolved_by: &str,
    ) -> anyhow::Result<bool> {
        let now = now_ms();
        let response_json = serde_json::to_string(response).context("serialize response")?;

        let result = sqlx::query(
            r#"
            UPDATE interaction_requests
            SET status = 'resolved',
                resolved_at_ms = ?2,
                resolved_by = ?3,
                response_json = ?4
            WHERE id = ?1 AND status = 'pending'
            "#,
        )
        .bind(request_id.to_string())
        .bind(now)
        .bind(resolved_by)
        .bind(response_json)
        .execute(&self.pool)
        .await
        .context("resolve interaction_request")?;

        Ok(result.rows_affected() == 1)
    }

    async fn ensure_run_row(&self, conversation_id: Uuid) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            r#"
            INSERT INTO runs (conversation_id, status, started_at_ms, ended_at_ms, codex_session_id, active_pid, metadata_json, updated_at_ms)
            VALUES (?1, 'idle', NULL, NULL, NULL, NULL, '{}', ?2)
            ON CONFLICT(conversation_id) DO NOTHING
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .context("ensure run row")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub root_path: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub tool: ToolKind,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub archived_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListItem {
    #[serde(flatten)]
    pub conversation: Conversation,
    pub run_status: RunStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub id: i64,
    pub conversation_id: Uuid,
    pub ts_ms: i64,
    pub event_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Idle,
    Queued,
    Running,
    Completed,
    Failed,
    Aborted,
    WaitingForInteraction,
}

impl RunStatus {
    fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Idle => "idle",
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Aborted => "aborted",
            RunStatus::WaitingForInteraction => "waiting_for_interaction",
        }
    }
}

impl std::str::FromStr for RunStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(RunStatus::Idle),
            "queued" => Ok(RunStatus::Queued),
            "running" => Ok(RunStatus::Running),
            "completed" => Ok(RunStatus::Completed),
            "failed" => Ok(RunStatus::Failed),
            "aborted" => Ok(RunStatus::Aborted),
            "waiting_for_interaction" => Ok(RunStatus::WaitingForInteraction),
            other => Err(anyhow::anyhow!("unknown run status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub conversation_id: Uuid,
    pub status: RunStatus,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub tool_session_id: Option<String>,
    pub active_pid: Option<i64>,
    pub metadata: Value,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Pending,
    Resolved,
}

impl InteractionStatus {
    fn as_str(&self) -> &'static str {
        match self {
            InteractionStatus::Pending => "pending",
            InteractionStatus::Resolved => "resolved",
        }
    }
}

impl std::str::FromStr for InteractionStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(InteractionStatus::Pending),
            "resolved" => Ok(InteractionStatus::Resolved),
            other => Err(anyhow::anyhow!("unknown interaction status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRequest {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub kind: String,
    pub status: InteractionStatus,
    pub payload: Value,
    pub created_at_ms: i64,
    pub timeout_ms: i64,
    pub default_action: String,
    pub resolved_at_ms: Option<i64>,
    pub resolved_by: Option<String>,
    pub response: Option<Value>,
}

#[derive(Debug, sqlx::FromRow)]
struct ProjectRow {
    id: String,
    name: String,
    root_path: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl From<ProjectRow> for Project {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).unwrap_or_else(|_| Uuid::nil()),
            name: row.name,
            root_path: row.root_path,
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ConversationRow {
    id: String,
    project_id: Option<String>,
    title: String,
    tool: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    archived_at_ms: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct ConversationListRow {
    id: String,
    project_id: Option<String>,
    title: String,
    tool: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    archived_at_ms: Option<i64>,
    run_status: Option<String>,
}

impl From<ConversationRow> for Conversation {
    fn from(row: ConversationRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).unwrap_or_else(|_| Uuid::nil()),
            project_id: row
                .project_id
                .and_then(|p| Uuid::parse_str(&p).ok())
                .filter(|p| !p.is_nil()),
            title: row.title,
            tool: row.tool.parse().unwrap_or_default(),
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
            archived_at_ms: row.archived_at_ms,
        }
    }
}

impl From<ConversationListRow> for ConversationListItem {
    fn from(row: ConversationListRow) -> Self {
        let run_status = row
            .run_status
            .as_deref()
            .unwrap_or("idle")
            .parse()
            .unwrap_or(RunStatus::Idle);
        let tool: ToolKind = row.tool.parse().unwrap_or_default();

        Self {
            conversation: Conversation {
                id: Uuid::parse_str(&row.id).unwrap_or_else(|_| Uuid::nil()),
                project_id: row
                    .project_id
                    .and_then(|p| Uuid::parse_str(&p).ok())
                    .filter(|p| !p.is_nil()),
                title: row.title,
                tool,
                created_at_ms: row.created_at_ms,
                updated_at_ms: row.updated_at_ms,
                archived_at_ms: row.archived_at_ms,
            },
            run_status,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ConversationEventRow {
    id: i64,
    conversation_id: String,
    ts_ms: i64,
    #[sqlx(rename = "type")]
    event_type: String,
    payload_json: String,
}

impl TryFrom<ConversationEventRow> for ConversationEvent {
    type Error = anyhow::Error;

    fn try_from(row: ConversationEventRow) -> Result<Self, Self::Error> {
        let conversation_id =
            Uuid::parse_str(&row.conversation_id).context("parse conversation_id")?;
        let payload: Value =
            serde_json::from_str(&row.payload_json).context("parse payload_json")?;
        Ok(Self {
            id: row.id,
            conversation_id,
            ts_ms: row.ts_ms,
            event_type: row.event_type,
            payload,
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    conversation_id: String,
    status: String,
    started_at_ms: Option<i64>,
    ended_at_ms: Option<i64>,
    tool_session_id: Option<String>,
    active_pid: Option<i64>,
    metadata_json: String,
    updated_at_ms: i64,
}

impl TryFrom<RunRow> for Run {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        let conversation_id = Uuid::parse_str(&row.conversation_id).context("parse run id")?;
        let status = row.status.parse()?;
        let metadata = serde_json::from_str(&row.metadata_json).context("parse metadata_json")?;
        Ok(Self {
            conversation_id,
            status,
            started_at_ms: row.started_at_ms,
            ended_at_ms: row.ended_at_ms,
            tool_session_id: row.tool_session_id,
            active_pid: row.active_pid,
            metadata,
            updated_at_ms: row.updated_at_ms,
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct InteractionRequestRow {
    id: String,
    conversation_id: String,
    kind: String,
    status: String,
    payload_json: String,
    created_at_ms: i64,
    timeout_ms: i64,
    default_action: String,
    resolved_at_ms: Option<i64>,
    resolved_by: Option<String>,
    response_json: Option<String>,
}

impl TryFrom<InteractionRequestRow> for InteractionRequest {
    type Error = anyhow::Error;

    fn try_from(row: InteractionRequestRow) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&row.id).context("parse interaction id")?;
        let conversation_id =
            Uuid::parse_str(&row.conversation_id).context("parse interaction conversation_id")?;
        let status = row.status.parse()?;
        let payload =
            serde_json::from_str(&row.payload_json).context("parse interaction payload")?;
        let response = match row.response_json {
            Some(s) => Some(serde_json::from_str(&s).context("parse interaction response")?),
            None => None,
        };

        Ok(Self {
            id,
            conversation_id,
            kind: row.kind,
            status,
            payload,
            created_at_ms: row.created_at_ms,
            timeout_ms: row.timeout_ms,
            default_action: row.default_action,
            resolved_at_ms: row.resolved_at_ms,
            resolved_by: row.resolved_by,
            response,
        })
    }
}

fn now_ms() -> i64 {
    let elapsed: Duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    elapsed.as_millis().min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_project_conversation_events() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let db_path = temp_dir.path().join("codex-web-test.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("Test Project", temp_dir.path()).await?;
        let projects = db.list_projects().await?;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, project.id);

        let conversation = db
            .create_conversation(Some(project.id), "Test Conversation", ToolKind::Codex)
            .await?;
        let conversations = db.list_conversations().await?;
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, conversation.id);
        assert_eq!(conversations[0].tool, ToolKind::Codex);

        let e1 = db
            .append_event(
                conversation.id,
                "user_message",
                &serde_json::json!({"text":"hi"}),
            )
            .await?;
        let e2 = db
            .append_event(
                conversation.id,
                "agent_message",
                &serde_json::json!({"text":"hello"}),
            )
            .await?;
        assert!(e2.id > e1.id);

        let events = db.list_events_after(conversation.id, 0, 100).await?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, e1.id);
        assert_eq!(events[1].id, e2.id);

        let run = db.get_run(conversation.id).await?;
        assert_eq!(run.status, RunStatus::Idle);
        Ok(())
    }

    #[tokio::test]
    async fn interaction_requests_roundtrip() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let db_path = temp_dir.path().join("codex-web-test-interactions.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let conversation = db
            .create_conversation(Some(project.id), "c", ToolKind::Codex)
            .await?;

        let req = db
            .create_interaction_request(
                conversation.id,
                "exec_approval_request",
                &serde_json::json!({"call_id":"call_1","command":"echo hi"}),
                10_000,
                "decline",
            )
            .await?;

        let pending = db.list_pending_interactions(conversation.id).await?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, req.id);

        let resolved = db
            .try_resolve_interaction(req.id, &serde_json::json!({"action":"decline"}), "test")
            .await?;
        assert!(resolved);

        let pending = db.list_pending_interactions(conversation.id).await?;
        assert_eq!(pending.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn create_conversation_persists_tool() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let db_path = temp_dir.path().join("codex-web-test-conversation-tool.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let conversation = db
            .create_conversation(Some(project.id), "c", ToolKind::ClaudeCode)
            .await?;

        let fetched = db.get_conversation(conversation.id).await?;
        assert_eq!(fetched.tool, ToolKind::ClaudeCode);

        Ok(())
    }

    #[tokio::test]
    async fn interaction_first_response_wins() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let db_path = temp_dir
            .path()
            .join("codex-web-test-interactions-first-wins.sqlite3");
        let db = Db::connect(&db_path).await?;

        let project = db.create_project("p", temp_dir.path()).await?;
        let conversation = db
            .create_conversation(Some(project.id), "c", ToolKind::Codex)
            .await?;

        let req = db
            .create_interaction_request(
                conversation.id,
                "exec_approval_request",
                &serde_json::json!({"call_id":"call_1","command":"echo hi"}),
                10_000,
                "decline",
            )
            .await?;

        let first = db
            .try_resolve_interaction(req.id, &serde_json::json!({"action":"accept"}), "t1")
            .await?;
        assert!(first, "expected first resolve to succeed");

        let second = db
            .try_resolve_interaction(req.id, &serde_json::json!({"action":"decline"}), "t2")
            .await?;
        assert!(!second, "expected second resolve to be rejected");

        let fetched = db
            .get_interaction_request(req.id)
            .await?
            .context("missing request")?;
        assert_eq!(fetched.status, InteractionStatus::Resolved);
        assert_eq!(fetched.resolved_by.as_deref(), Some("t1"));
        assert_eq!(
            fetched
                .response
                .as_ref()
                .and_then(|v| v.get("action"))
                .and_then(|v| v.as_str()),
            Some("accept")
        );

        Ok(())
    }

    #[tokio::test]
    async fn create_project_reuses_same_root_path() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let db_path = temp_dir.path().join("codex-web-test-project-dedup.sqlite3");
        let db = Db::connect(&db_path).await?;

        let p1 = db.create_project("P1", temp_dir.path()).await?;
        let p2 = db.create_project("P2", temp_dir.path()).await?;
        assert_eq!(p1.id, p2.id);
        assert_eq!(p2.name, "P2");
        Ok(())
    }
}
