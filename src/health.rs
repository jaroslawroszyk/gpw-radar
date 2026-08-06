use crate::metrics::DB_STATUS_GAUGE;
use axum::{Router, routing::get};
use prometheus::{Encoder, TextEncoder};
use rusqlite::Connection;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

async fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!(error = %e, "Błąd podczas kodowania metryk Prometheusa");
    }
    String::from_utf8(buffer).unwrap_or_default()
}

pub async fn start_health_check_server(db: Arc<Mutex<Connection>>) {
    let app = Router::new()
        .route(
            "/health",
            get(move || {
                let db_clone = Arc::clone(&db);
                async move {
                    let is_ok = db_clone
                        .lock()
                        .is_ok_and(|conn| conn.execute_batch("SELECT 1;").is_ok());
                    if is_ok {
                        DB_STATUS_GAUGE.set(1);
                        "OK"
                    } else {
                        DB_STATUS_GAUGE.set(0);
                        "ERROR - DB UNREACHABLE"
                    }
                }
            }),
        )
        .route("/metrics", get(metrics_handler));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    info!(
        url = "http://0.0.0.0:8080/health",
        metrics = "http://0.0.0.0:8080/metrics",
        "🌐 HTTP Health Check & Metrics serwer uruchomiony"
    );

    if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
        let _ = axum::serve(listener, app).await;
    }
}
