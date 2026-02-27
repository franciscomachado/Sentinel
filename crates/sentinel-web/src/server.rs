use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::Json,
    routing::get,
};
use serde_json::{Value, json};
use tower_http::cors::{Any, CorsLayer};

use sentinel_membrane::audit::AuditLog;
use sentinel_memory::ledger::Ledger;
use sentinel_memory::state::StateManager;

/// Shared state for the dashboard endpoints.
pub struct DashboardState {
    pub ledger: Ledger,
    pub state_mgr: StateManager,
    pub audit: AuditLog,
}

type AppState = Arc<DashboardState>;

/// Build the Axum router for the dashboard.
pub fn router(dashboard: DashboardState) -> Router {
    let state: AppState = Arc::new(dashboard);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/health", get(health))
        .route("/api/memories", get(get_memories))
        .route("/api/ledger", get(get_ledger))
        .route("/api/cost", get(get_cost))
        .route("/api/engagement", get(get_engagement))
        .layer(cors)
        .with_state(state)
}

/// Start the dashboard server on the given port.
///
/// Binds to `127.0.0.1` by default. Set `SENTINEL_DASHBOARD_BIND=0.0.0.0`
/// to listen on all interfaces (needed inside Docker containers).
pub async fn serve(dashboard: DashboardState, port: u16) -> anyhow::Result<()> {
    let app = router(dashboard);
    let host: std::net::Ipv4Addr = std::env::var("SENTINEL_DASHBOARD_BIND")
        .unwrap_or_else(|_| "127.0.0.1".into())
        .parse()
        .unwrap_or(std::net::Ipv4Addr::LOCALHOST);
    let addr = std::net::SocketAddr::from((host, port));
    tracing::info!(%addr, "starting web dashboard");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn get_memories(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let memories = state
        .state_mgr
        .get_memories()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items: Vec<Value> = memories
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "content": m.content,
                "tags": m.tags,
                "source": m.source,
            })
        })
        .collect();

    Ok(Json(json!({ "memories": items, "count": items.len() })))
}

async fn get_ledger(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let entries = state
        .ledger
        .recent(50)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "timestamp": e.timestamp.to_rfc3339(),
                "category": format!("{:?}", e.category),
                "content": e.content,
            })
        })
        .collect();

    Ok(Json(json!({ "entries": items, "count": items.len() })))
}

async fn get_cost(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let since = chrono::Utc::now() - chrono::Duration::days(30);
    let cost = state
        .audit
        .total_cost_since(since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "period": "last_30_days",
        "input_tokens": cost.input_tokens,
        "output_tokens": cost.output_tokens,
        "cached_tokens": cost.cached_tokens,
        "estimated_cost_eur": cost.estimated_cost_eur(),
    })))
}

async fn get_engagement(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let level = state
        .state_mgr
        .engagement_level()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "level": level.to_string(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_app() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = sentinel_memory::db::open(&db_path).await.unwrap();

        let dashboard = DashboardState {
            ledger: Ledger::new(pool.clone()),
            state_mgr: StateManager::new(pool.clone()),
            audit: AuditLog::new(pool),
        };

        (router(dashboard), dir)
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (app, _dir) = test_app().await;
        let resp = app
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn memories_returns_empty_list() {
        let (app, _dir) = test_app().await;
        let resp = app
            .oneshot(Request::get("/api/memories").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 0);
    }

    #[tokio::test]
    async fn cost_returns_zeros() {
        let (app, _dir) = test_app().await;
        let resp = app
            .oneshot(Request::get("/api/cost").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["input_tokens"], 0);
    }
}
