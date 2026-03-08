use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{build_router, AppState};

#[tokio::test]
async fn posting_message_runs_codex_stub_and_persists_agent_message() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("codex_stub.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(128);

    let codex = codex_web::codex::CodexRuntime::stub(vec![
        serde_json::json!({"type":"thread.started","thread_id":"00000000-0000-0000-0000-000000000001"}),
        serde_json::json!({"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello from stub"}}),
    ]);

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex,
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        },
        None,
    );

    // Create project
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
    let project_id = project.get("id").unwrap().as_str().unwrap().to_string();

    // Create conversation
    let req = Request::builder()
        .method("POST")
        .uri("/api/conversations")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "project_id": project_id,
                "title": "Stub Conversation"
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
    let conversation_id = conversation
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // Post message (triggers background stub turn).
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/conversations/{conversation_id}/messages"
        ))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "text": "hi" }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Poll events until we see the derived agent_message.
    let mut saw_agent = false;
    for _ in 0..25 {
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/conversations/{conversation_id}/events?after=0&limit=100"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        if events.iter().any(|e| e.get("event_type").and_then(|v| v.as_str()) == Some("agent_message")) {
            saw_agent = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(saw_agent, "expected agent_message to be persisted");
}
