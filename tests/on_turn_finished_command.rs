#![cfg(unix)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn runs_configured_on_turn_finished_command() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("turn_finished_hook.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(128);

    // A deterministic stub turn that completes successfully.
    let codex = codex_web::codex::CodexRuntime::stub(vec![serde_json::json!({
        "type": "item_completed",
        "thread_id": "00000000-0000-0000-0000-000000000001",
        "turn_id": "turn_0",
        "item": {
            "type": "AgentMessage",
            "id": "item_0",
            "content": [
                { "type": "Text", "text": "hello" }
            ]
        }
    })]);

    // Writes the final run status to a file in the project root.
    let hook_path = "turn_finished_hook.txt";
    let hook_cmd = format!("printf \"%s\" \"$CODEX_WEB_RUN_STATUS\" > {hook_path}");

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex,
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: None,
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: Some(hook_cmd),
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
                "title": "Hook Conversation"
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

    // Post message (triggers background stub turn + hook).
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/{conversation_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "text": "hi" }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait for the hook file to appear and be populated.
    let target = temp_dir.path().join(hook_path);
    let mut contents = None;
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&target) {
            if !text.trim().is_empty() {
                contents = Some(text);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let contents = contents.expect("expected hook command to write output file");
    assert_eq!(contents.trim(), "completed");
}

