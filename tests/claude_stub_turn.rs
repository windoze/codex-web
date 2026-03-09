use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn posting_message_runs_claude_stub_and_persists_events() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("claude_stub.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(128);

    let codex = codex_web::codex::CodexRuntime::stub(vec![]);
    let claude = codex_web::claude::ClaudeRuntime::stub(vec![
        serde_json::json!({ "type": "session_configured", "session_id": "sess_1" }),
        serde_json::json!({ "type": "assistant_message_delta", "delta": "hello from claude stub" }),
    ]);

    let app = build_router(
        AppState {
            db,
            event_tx,
            runners: codex_web::runners::RunnerSet::new(codex, claude),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: None,
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
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

    // Create conversation with tool = claude-code
    let req = Request::builder()
        .method("POST")
        .uri("/api/conversations")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "project_id": project_id,
                "title": "Claude Stub Conversation",
                "tool": "claude-code"
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

    // Post message (triggers background claude stub turn).
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/{conversation_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "text": "hi" }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Poll events until we see both the raw claude_event and the derived agent_message.
    let mut saw_agent = false;
    let mut saw_raw = false;
    for _ in 0..25 {
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

        saw_agent |= events
            .iter()
            .any(|e| e.get("event_type").and_then(|v| v.as_str()) == Some("agent_message"));
        saw_raw |= events
            .iter()
            .any(|e| e.get("event_type").and_then(|v| v.as_str()) == Some("claude_event"));

        if saw_agent && saw_raw {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(saw_raw, "expected claude_event to be persisted");
    assert!(saw_agent, "expected agent_message to be persisted");

    // Ensure the run recorded the session id returned by the tool.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/conversations/{conversation_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let convo_with_run: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = convo_with_run
        .get("run")
        .and_then(|r| r.get("tool_session_id"))
        .and_then(|v| v.as_str());
    assert_eq!(session_id, Some("sess_1"));
}

