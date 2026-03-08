use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Clone)]
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
        let project = Project {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            root_path: root_path.to_string_lossy().to_string(),
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

    pub async fn create_conversation(
        &self,
        project_id: Option<Uuid>,
        title: &str,
    ) -> anyhow::Result<Conversation> {
        let now = now_ms();
        let conversation = Conversation {
            id: Uuid::new_v4(),
            project_id,
            title: title.to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
            archived_at_ms: None,
        };

        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, title, created_at_ms, updated_at_ms, archived_at_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, NULL)
            "#,
        )
        .bind(conversation.id.to_string())
        .bind(project_id.map(|p| p.to_string()))
        .bind(&conversation.title)
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
            SELECT id, project_id, title, created_at_ms, updated_at_ms, archived_at_ms
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
            SELECT conversation_id, status, started_at_ms, ended_at_ms, codex_session_id, active_pid, metadata_json, updated_at_ms
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

    pub async fn mark_run_completed(
        &self,
        conversation_id: Uuid,
        status: RunStatus,
        codex_session_id: Option<&str>,
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
                codex_session_id = COALESCE(?4, codex_session_id),
                active_pid = ?5
            WHERE conversation_id = ?1
            "#,
        )
        .bind(conversation_id.to_string())
        .bind(status_str)
        .bind(now)
        .bind(codex_session_id)
        .bind(active_pid)
        .execute(&self.pool)
        .await
        .context("mark run completed")?;
        Ok(())
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
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub archived_at_ms: Option<i64>,
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
    pub codex_session_id: Option<String>,
    pub active_pid: Option<i64>,
    pub metadata: Value,
    pub updated_at_ms: i64,
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
    created_at_ms: i64,
    updated_at_ms: i64,
    archived_at_ms: Option<i64>,
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
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
            archived_at_ms: row.archived_at_ms,
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
        let payload: Value = serde_json::from_str(&row.payload_json).context("parse payload_json")?;
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
    codex_session_id: Option<String>,
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
            codex_session_id: row.codex_session_id,
            active_pid: row.active_pid,
            metadata,
            updated_at_ms: row.updated_at_ms,
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
            .create_conversation(Some(project.id), "Test Conversation")
            .await?;
        let conversations = db.list_conversations().await?;
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, conversation.id);

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
}
