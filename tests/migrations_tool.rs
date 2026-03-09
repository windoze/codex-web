use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use codex_web::tool::ToolKind;

#[tokio::test]
async fn migration_adds_tool_and_backfills_tool_session_id() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("migrations_tool.sqlite3");

    // Simulate a pre-0003 database (no conversations.tool, no runs.tool_session_id).
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE projects (
          id TEXT PRIMARY KEY NOT NULL,
          name TEXT NOT NULL,
          root_path TEXT NOT NULL,
          created_at_ms INTEGER NOT NULL,
          updated_at_ms INTEGER NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE conversations (
          id TEXT PRIMARY KEY NOT NULL,
          project_id TEXT NULL REFERENCES projects(id) ON DELETE SET NULL,
          title TEXT NOT NULL,
          created_at_ms INTEGER NOT NULL,
          updated_at_ms INTEGER NOT NULL,
          archived_at_ms INTEGER NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE runs (
          conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
          status TEXT NOT NULL,
          started_at_ms INTEGER NULL,
          ended_at_ms INTEGER NULL,
          codex_session_id TEXT NULL,
          active_pid INTEGER NULL,
          metadata_json TEXT NOT NULL DEFAULT '{}',
          updated_at_ms INTEGER NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    let now_ms: i64 = 1_700_000_000_000;
    let project_id = Uuid::new_v4();
    let conversation_id = Uuid::new_v4();
    let codex_session_id = "stub_session_123";

    sqlx::query(
        r#"
        INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )
    .bind(project_id.to_string())
    .bind("p")
    .bind(temp_dir.path().to_string_lossy().to_string())
    .bind(now_ms)
    .bind(now_ms)
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, title, created_at_ms, updated_at_ms, archived_at_ms)
        VALUES (?1, ?2, ?3, ?4, ?5, NULL)
        "#,
    )
    .bind(conversation_id.to_string())
    .bind(project_id.to_string())
    .bind("c")
    .bind(now_ms)
    .bind(now_ms)
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO runs (conversation_id, status, started_at_ms, ended_at_ms, codex_session_id, active_pid, metadata_json, updated_at_ms)
        VALUES (?1, 'completed', ?2, ?2, ?3, NULL, '{}', ?2)
        "#,
    )
    .bind(conversation_id.to_string())
    .bind(now_ms)
    .bind(codex_session_id)
    .execute(&pool)
    .await?;

    drop(pool);

    // Connecting runs migrations (including 0003).
    let db = codex_web::db::Db::connect(&db_path).await?;

    let convo = db.get_conversation(conversation_id).await?;
    assert_eq!(convo.tool, ToolKind::Codex);

    let run = db.get_run(conversation_id).await?;
    assert_eq!(run.tool_session_id.as_deref(), Some(codex_session_id));

    Ok(())
}

