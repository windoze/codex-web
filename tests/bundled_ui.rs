#![cfg(feature = "bundled-ui")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

#[tokio::test]
async fn bundled_ui_serves_index_html() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("bundled_ui.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex: codex_web::codex::CodexRuntime::stub(vec![]),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: None,
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: None,
        },
        None,
    );

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok());
    assert!(ct.unwrap_or("").contains("text/html"));

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.to_lowercase().contains("<!doctype html"),
        "expected index.html payload"
    );
}

#[tokio::test]
async fn bundled_ui_falls_back_to_index_for_spa_paths() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("bundled_ui_spa.sqlite3");
    let db = codex_web::db::Db::connect(&db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

    let app = build_router(
        AppState {
            db,
            event_tx,
            codex: codex_web::codex::CodexRuntime::stub(vec![]),
            ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auth_token: None,
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
            run_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            on_turn_finished_command: None,
        },
        None,
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/some/nested/route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
