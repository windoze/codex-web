use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
#[cfg(feature = "bundled-ui")]
use axum::body::Body;
#[cfg(feature = "bundled-ui")]
use axum::extract::OriginalUri;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
#[cfg(feature = "bundled-ui")]
use axum::http::{HeaderValue, StatusCode};
use axum::http::{Method, header};
#[cfg(feature = "bundled-ui")]
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::db::Db;

#[cfg(feature = "bundled-ui")]
use rust_embed::RustEmbed;

#[cfg(feature = "bundled-ui")]
#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct BundledUiAssets;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub event_tx: broadcast::Sender<crate::db::ConversationEvent>,
    pub codex: crate::codex::CodexRuntime,
    pub ws_clients: Arc<AtomicUsize>,
    pub interaction_timeout_ms: i64,
    pub interaction_default_action: String,
    pub run_semaphore: Arc<Semaphore>,
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    init_tracing();

    let db = Db::connect(&config.db_path).await?;
    let (event_tx, _rx) = broadcast::channel(1024);
    let ws_clients = Arc::new(AtomicUsize::new(0));
    let run_semaphore = Arc::new(Semaphore::new(config.max_concurrent_runs.max(1)));
    let app = build_router(
        AppState {
            db,
            event_tx,
            codex: crate::codex::CodexRuntime::Real(crate::codex::CodexReal {
                ask_for_approval: config.codex_ask_for_approval.clone(),
                sandbox: config.codex_sandbox.clone(),
                skip_git_repo_check: true,
            }),
            ws_clients,
            interaction_timeout_ms: config.interaction_timeout_ms,
            interaction_default_action: config.interaction_default_action.clone(),
            run_semaphore,
        },
        config.static_dir.as_deref(),
    );

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;

    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await.context("serve")?;
    Ok(())
}

pub fn build_router(state: AppState, static_dir: Option<&std::path::Path>) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_origin(Any)
        .allow_headers(Any)
        .expose_headers([header::CONTENT_TYPE]);

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", crate::api::router())
        .route("/ws", get(ws))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    if let Some(dir) = static_dir {
        let service = ServeDir::new(dir).not_found_service(ServeFile::new(dir.join("index.html")));
        app = app.nest_service("/", service);
    } else {
        #[cfg(feature = "bundled-ui")]
        {
            app = app.fallback(get(serve_bundled_ui));
        }
    }

    app
}

async fn healthz(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"status":"ok"}))
}

#[derive(Debug, serde::Deserialize)]
struct WsQuery {
    conversation_id: uuid::Uuid,
}

async fn ws(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    let rx = state.event_tx.subscribe();
    let ws_clients = state.ws_clients.clone();
    ws_clients.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| async move {
        ws_loop(socket, q.conversation_id, rx).await;
        ws_clients.fetch_sub(1, Ordering::Relaxed);
    })
}

async fn ws_loop(
    mut socket: WebSocket,
    conversation_id: uuid::Uuid,
    mut rx: broadcast::Receiver<crate::db::ConversationEvent>,
) {
    while let Ok(event) = rx.recv().await {
        if event.conversation_id != conversation_id {
            continue;
        }

        let Ok(text) = serde_json::to_string(&event) else {
            continue;
        };

        if socket.send(Message::Text(text)).await.is_err() {
            break;
        }
    }
}

fn init_tracing() {
    let env_filter = std::env::var("RUST_LOG")
        .ok()
        .unwrap_or_else(|| "codex_web=info,tower_http=info".to_string());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .try_init();
}

#[cfg(feature = "bundled-ui")]
async fn serve_bundled_ui(OriginalUri(uri): OriginalUri) -> axum::response::Response {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }

    // Basic path traversal hardening for safety.
    if path.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let is_asset_request = path.contains('.');
    let asset = BundledUiAssets::get(path).or_else(|| {
        if is_asset_request {
            None
        } else {
            BundledUiAssets::get("index.html")
        }
    });

    let Some(asset) = asset else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut res = axum::response::Response::new(Body::from(asset.data.into_owned()));
    if let Ok(ct) = HeaderValue::from_str(mime.as_ref()) {
        res.headers_mut().insert(header::CONTENT_TYPE, ct);
    }

    // Aggressive caching for hashed assets, conservative for HTML.
    let cache = if path == "index.html" {
        "no-cache"
    } else if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_works() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let db_path = temp_dir.path().join("healthz.sqlite3");
        let db = Db::connect(&db_path).await.expect("db connect");
        let (event_tx, _rx) = broadcast::channel(16);
        let app = build_router(
            AppState {
                db,
                event_tx,
                codex: crate::codex::CodexRuntime::stub(vec![]),
                ws_clients: Arc::new(AtomicUsize::new(0)),
                interaction_timeout_ms: 30_000,
                interaction_default_action: "decline".to_string(),
                run_semaphore: Arc::new(Semaphore::new(1)),
            },
            None,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(response.status(), 200);
    }
}
