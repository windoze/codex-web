use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::db::RunStatus;
use codex_web::server::{AppState, build_router};

async fn setup() -> (axum::Router, codex_web::db::Db, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("lifecycle.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let db_for_state = db.clone();
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
        AppState {
            db: db_for_state,
            event_tx,
            runners: codex_web::runners::RunnerSet::new(
                codex_web::codex::CodexRuntime::stub(vec![]),
                codex_web::claude::ClaudeRuntime::stub(vec![]),
            ),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: None,
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: None,
            turn_manager: codex_web::turns::TurnManager::default(),
        },
        None,
    );

    (app, db, temp_dir)
}

async fn create_project_and_conversation(app: &axum::Router, root: &std::path::Path) -> String {
    // Create project
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "root_path": root.to_string_lossy() }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let project_id = project.get("id").unwrap().as_str().unwrap().to_string();

    // Create conversation
    let req = Request::builder()
        .method("POST")
        .uri("/api/conversations")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "project_id": project_id, "title": "Lifecycle Test" }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let conversation: serde_json::Value = serde_json::from_slice(&body).unwrap();
    conversation
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn delete_conversation_removes_it_from_list() {
    let (app, _db, temp_dir) = setup().await;
    let conversation_id = create_project_and_conversation(&app, temp_dir.path()).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/conversations/{conversation_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/conversations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let conversations: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(
        conversations
            .iter()
            .all(|c| c.get("id").and_then(|v| v.as_str()) != Some(&conversation_id)),
        "expected deleted conversation to be absent from list",
    );
}

#[tokio::test]
async fn delete_conversation_refuses_when_running() {
    let (app, db, temp_dir) = setup().await;
    let conversation_id = create_project_and_conversation(&app, temp_dir.path()).await;

    // Mark the run as running directly in the DB (no in-memory turn handle).
    let marked = db
        .try_mark_run_running(conversation_id.parse().unwrap())
        .await
        .expect("mark run running");
    assert!(marked);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/conversations/{conversation_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cancel_conversation_marks_aborted_when_no_in_memory_turn() {
    let (app, db, temp_dir) = setup().await;
    let conversation_id = create_project_and_conversation(&app, temp_dir.path()).await;

    // Mark the run as running directly in the DB (simulates daemon restart).
    let convo_uuid = conversation_id.parse().unwrap();
    let marked = db
        .try_mark_run_running(convo_uuid)
        .await
        .expect("mark run running");
    assert!(marked);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/conversations/{conversation_id}/cancel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let run = db.get_run(convo_uuid).await.expect("get run");
    assert_eq!(run.status, RunStatus::Aborted);
}
