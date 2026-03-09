use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn interaction_can_be_resolved_via_api() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("interaction_flow.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(128);

    // Simulate a "present" user so the orchestrator waits instead of auto-responding.
    let ws_clients = Arc::new(AtomicUsize::new(1));

    let codex = codex_web::codex::CodexRuntime::stub(vec![
        serde_json::json!({
            "type": "exec_approval_request",
            "call_id": "call_1",
            "command": ["echo", "hi"],
            "cwd": ".",
            "parsed_cmd": []
        }),
        serde_json::json!({
            "type": "item_completed",
            "thread_id": "00000000-0000-0000-0000-000000000010",
            "turn_id": "turn_0",
            "item": {
                "type": "AgentMessage",
                "id": "item_0",
                "content": [
                    { "type": "Text", "text": "after approval" }
                ]
            }
        }),
    ]);

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex,
            ws_clients,
            auth_token: None,
            interaction_timeout_ms: 5_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: None,
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
                "title": "Interaction Conversation"
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

    // Trigger the turn.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/{conversation_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "text": "hi" }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait for the pending interaction to appear.
    let mut interaction_id: Option<String> = None;
    for _ in 0..50 {
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/conversations/{conversation_id}/interactions"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pending: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        if let Some(id) = pending
            .get(0)
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
        {
            interaction_id = Some(id.to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let interaction_id = interaction_id.expect("interaction id");

    // Resolve it.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/interactions/{interaction_id}/respond"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "action": "accept" }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Turn should complete and eventually produce agent_message.
    let mut saw_agent = false;
    for _ in 0..100 {
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/conversations/{conversation_id}/events?after=0&limit=200"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        if events
            .iter()
            .any(|e| e.get("event_type").and_then(|v| v.as_str()) == Some("agent_message"))
        {
            saw_agent = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        saw_agent,
        "expected agent_message after resolving interaction"
    );
}
