use anyhow::Result;
use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::net::SocketAddr;
use tracing::info;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: azcluster_core::VERSION,
    })
}

fn router() -> Router {
    Router::new().route("/v1/healthz", get(healthz))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind: SocketAddr = std::env::var("AZCLUSTER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8443".to_string())
        .parse()?;

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(version = %azcluster_core::VERSION, %bind, "azcluster-server listening");

    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = router();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/v1/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["version"], azcluster_core::VERSION);
    }

    #[tokio::test]
    async fn scale_route_removed() {
        // The /scale endpoint moved to direct ARM calls from the CLI in v0.14.
        // This test ensures it cannot be re-added accidentally without intent.
        let app = router();
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/pools/gpu/scale")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"count":2}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
