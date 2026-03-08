use anyhow::Context;
use axum::extract::State;
use axum::http::{header, Method};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    init_tracing();

    let db = Db::connect(&config.db_path).await?;
    let app = build_router(AppState { db }, config.static_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;

    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await.context("serve")?;
    Ok(())
}

pub fn build_router(state: AppState, static_dir: Option<&std::path::Path>) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_origin(Any)
        .allow_headers(Any)
        .expose_headers([header::CONTENT_TYPE]);

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    if let Some(dir) = static_dir {
        let service = ServeDir::new(dir);
        app = app.nest_service("/", service);
    }

    app
}

async fn healthz(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"status":"ok"}))
}

fn init_tracing() {
    let env_filter = std::env::var("RUST_LOG")
        .ok()
        .unwrap_or_else(|| "codex_web=info,tower_http=info".to_string());

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .init();
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
        let app = build_router(AppState { db }, None);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .expect("oneshot");
        assert_eq!(response.status(), 200);
    }
}
