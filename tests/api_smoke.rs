use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::db::ConversationEvent;
use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn projects_conversations_and_events_roundtrip() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("api.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
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
                "title": "Test Conversation"
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

    // List conversations should include a run_status per item.
    let req = Request::builder()
        .method("GET")
        .uri("/api/conversations")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let conversations: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    let created = conversations
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(&conversation_id))
        .expect("created conversation present in list");
    let run_status = created.get("run_status").and_then(|v| v.as_str()).unwrap();
    assert!(!run_status.is_empty(), "expected non-empty run_status");

    // Post a user message -> should broadcast an event.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/{conversation_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "text": "hello" }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let event: ConversationEvent = serde_json::from_slice(&body).unwrap();
    assert_eq!(event.event_type, "user_message");

    let broadcast_event = event_rx.recv().await.unwrap();
    assert_eq!(broadcast_event.id, event.id);

    // List events (should include at least the user message + run_status).
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
    let events: Vec<ConversationEvent> = serde_json::from_slice(&body).unwrap();
    assert!(
        events.len() >= 2,
        "expected >= 2 events, got {}",
        events.len()
    );
    assert_eq!(events[0].id, event.id);
}
