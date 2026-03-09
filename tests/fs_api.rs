use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use codex_web::server::{AppState, build_router};

async fn test_app(db_path: &Path) -> axum::Router {
    let db = codex_web::db::Db::connect(db_path)
        .await
        .expect("db connect");
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

    build_router(
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
    )
}

#[tokio::test]
async fn fs_home_returns_an_absolute_directory() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("fs_home.sqlite3");
    let app = test_app(&db_path).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/fs/home")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let home = json.get("path").and_then(|v| v.as_str()).unwrap();
    assert!(!home.is_empty());
    assert!(PathBuf::from(home).is_absolute(), "expected absolute path");
    let md = std::fs::metadata(home).expect("home metadata");
    assert!(md.is_dir(), "expected home path to be a directory");
}

#[tokio::test]
async fn fs_list_returns_entries_for_a_directory() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp_dir.path().join("subdir")).expect("create subdir");
    std::fs::write(temp_dir.path().join("file.txt"), "hi").expect("write file");

    let db_path = temp_dir.path().join("fs_list.sqlite3");
    let app = test_app(&db_path).await;

    let path = temp_dir.path().to_string_lossy();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/fs/list?path={path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries array");

    let mut kinds_by_name = std::collections::HashMap::new();
    for e in entries {
        let name = e
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = e
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        kinds_by_name.insert(name, kind);
    }

    assert_eq!(kinds_by_name.get("subdir").map(String::as_str), Some("dir"));
    assert_eq!(
        kinds_by_name.get("file.txt").map(String::as_str),
        Some("file")
    );
}

#[tokio::test]
async fn fs_list_defaults_to_home_when_missing_path_param() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("fs_list_default.sqlite3");
    let app = test_app(&db_path).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/fs/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let path = json.get("path").and_then(|v| v.as_str()).unwrap();
    assert!(PathBuf::from(path).is_absolute(), "expected absolute path");
}

#[tokio::test]
async fn fs_list_rejects_relative_paths() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("fs_list_relative.sqlite3");
    let app = test_app(&db_path).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/fs/list?path=relative/path")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn fs_list_rejects_file_paths() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp_dir.path().join("file.txt"), "hi").expect("write file");

    let db_path = temp_dir.path().join("fs_list_file.sqlite3");
    let app = test_app(&db_path).await;

    let file_path = temp_dir.path().join("file.txt");
    let file_path = file_path.to_string_lossy().to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/fs/list?path={file_path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
