use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

fn test_state(db: codex_web::db::Db) -> AppState {
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    AppState {
        db,
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
    }
}

#[tokio::test]
async fn create_ssh_project_via_api() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_project.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db");
    let app = build_router(test_state(db), None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "kind": "ssh",
                "ssh_target": "user@remote-host",
                "ssh_port": 2222,
                "remote_root_path": "/home/user/project",
                "name": "remote-project"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(project.get("kind").and_then(|v| v.as_str()), Some("ssh"));
    assert_eq!(
        project.get("ssh_target").and_then(|v| v.as_str()),
        Some("user@remote-host")
    );
    assert_eq!(
        project.get("ssh_port").and_then(|v| v.as_i64()),
        Some(2222)
    );
    assert_eq!(
        project.get("remote_root_path").and_then(|v| v.as_str()),
        Some("/home/user/project")
    );
    assert_eq!(
        project.get("name").and_then(|v| v.as_str()),
        Some("remote-project")
    );

    // Verify project appears in list
    let req = Request::builder()
        .method("GET")
        .uri("/api/projects")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let projects: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    let ssh_projects: Vec<_> = projects
        .iter()
        .filter(|p| p.get("kind").and_then(|v| v.as_str()) == Some("ssh"))
        .collect();
    assert_eq!(ssh_projects.len(), 1);
}

#[tokio::test]
async fn create_ssh_project_deduplicates() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_dedup.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db");
    let app = build_router(test_state(db), None);

    let body_json = serde_json::json!({
        "kind": "ssh",
        "ssh_target": "user@host",
        "remote_root_path": "/home/user/repo"
    });

    // Create first
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project1: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id1 = project1.get("id").unwrap().as_str().unwrap();

    // Create second with same target+path -> should return same project
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project2: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id2 = project2.get("id").unwrap().as_str().unwrap();

    assert_eq!(id1, id2, "deduplication should return the same project");
}

#[tokio::test]
async fn create_ssh_project_requires_target_and_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_validation.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db");
    let app = build_router(test_state(db), None);

    // Missing ssh_target
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "kind": "ssh",
                "remote_root_path": "/home/user/repo"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Missing remote_root_path
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "kind": "ssh",
                "ssh_target": "user@host"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_conversation_on_ssh_project() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_convo.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db");
    let app = build_router(test_state(db), None);

    // Create SSH project
    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "kind": "ssh",
                "ssh_target": "user@host",
                "remote_root_path": "/home/user/repo"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let project_id = project.get("id").unwrap().as_str().unwrap();

    // Create conversation on SSH project
    let req = Request::builder()
        .method("POST")
        .uri("/api/conversations")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "project_id": project_id,
                "title": "SSH Test Conversation"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let conversation: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        conversation.get("project_id").and_then(|v| v.as_str()),
        Some(project_id)
    );
}

#[tokio::test]
async fn local_project_has_local_kind() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("local_kind.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db");
    let app = build_router(test_state(db), None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "root_path": temp_dir.path().to_string_lossy()
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let project: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        project.get("kind").and_then(|v| v.as_str()),
        Some("local")
    );
}
