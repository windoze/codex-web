use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::db::ConversationEvent;
use codex_web::server::{build_router, AppState};

#[tokio::test]
async fn projects_conversations_and_events_roundtrip() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("api.sqlite3");
    let db = codex_web::db::Db::connect(&db_path).await.expect("db connect");
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex: codex_web::codex::CodexRuntime::stub(vec![]),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
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

    // Post a user message -> should broadcast an event.
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/conversations/{conversation_id}/messages"
        ))
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
    assert!(events.len() >= 2, "expected >= 2 events, got {}", events.len());
    assert_eq!(events[0].id, event.id);
}
