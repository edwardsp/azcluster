use anyhow::Result;
use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::process::Command;
use tracing::{error, info};

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

#[derive(Deserialize)]
struct ScaleRequest {
    count: u32,
}

#[derive(Serialize)]
struct ScaleResponse {
    pool: String,
    vmss: String,
    requested_count: u32,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

async fn scale_pool(
    Path(pool): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> Result<Json<ScaleResponse>, (StatusCode, Json<ErrorBody>)> {
    let cluster = std::env::var("AZCLUSTER_CLUSTER_NAME")
        .map_err(|_| internal("AZCLUSTER_CLUSTER_NAME not set"))?;
    let resource_group = std::env::var("AZCLUSTER_RESOURCE_GROUP")
        .map_err(|_| internal("AZCLUSTER_RESOURCE_GROUP not set"))?;

    let vmss_name = format!("vmss-{cluster}-{pool}");

    info!(%pool, %vmss_name, requested_count = req.count, "scaling VMSS");

    let output = Command::new("az")
        .args([
            "vmss",
            "scale",
            "--resource-group",
            &resource_group,
            "--name",
            &vmss_name,
            "--new-capacity",
            &req.count.to_string(),
        ])
        .output()
        .await
        .map_err(|e| internal(&format!("failed to spawn az: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(%vmss_name, %stderr, "az vmss scale failed");
        return Err(internal(&format!("az vmss scale failed: {stderr}")));
    }

    Ok(Json(ScaleResponse {
        pool,
        vmss: vmss_name,
        requested_count: req.count,
    }))
}

fn internal(msg: &str) -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: msg.to_string(),
        }),
    )
}

fn router() -> Router {
    Router::new()
        .route("/v1/healthz", get(healthz))
        .route("/v1/pools/:name/scale", post(scale_pool))
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
    async fn scale_route_exists() {
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
        assert_ne!(res.status(), StatusCode::NOT_FOUND);
    }
}
