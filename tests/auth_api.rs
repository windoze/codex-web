use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn api_requires_bearer_token_when_configured() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("auth_api.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
        AppState {
            db,
            event_tx,
            runners: codex_web::runners::RunnerSet::new(
                codex_web::codex::CodexRuntime::stub(vec![]),
                codex_web::claude::ClaudeRuntime::stub(vec![]),
            ),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: Some("secret".to_string()),
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: None,
            turn_manager: codex_web::turns::TurnManager::default(),
        },
        None,
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/projects")
                .header("authorization", "Bearer secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
