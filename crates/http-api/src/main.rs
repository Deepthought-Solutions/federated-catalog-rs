use std::sync::Arc;

use http_api::{build_router, AppState};
use rdf_store::memory::InMemoryCatalogCache;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("HTTP_API_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());

    // The in-memory cache is a placeholder: the real backend (an RDF
    // store, choice deferred - see the top-level README) will be wired in
    // here once selected, behind the same `CatalogCache` trait.
    let state = AppState::new(Arc::new(InMemoryCatalogCache::new()));
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|err| panic!("failed to bind {addr}: {err}"));
    tracing::info!("http-api listening on {addr}");

    axum::serve(listener, app)
        .await
        .expect("http-api server failed");
}
