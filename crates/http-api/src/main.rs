use std::sync::Arc;

use http_api::{AppState, build_router, seed_sample_catalog};
use rdf_store::memory::InMemoryCatalogCache;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("HTTP_API_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());

    // The in-memory cache is a placeholder: the real backend (Oxigraph,
    // per the rdf-store module docs) will be wired in here once the
    // graph-naming scheme is decided, behind the same `CatalogCache`
    // trait.
    let cache = Arc::new(InMemoryCatalogCache::new());
    // No crawler exists yet, so seed one sample catalog to serve - this
    // stands in for a real crawl result until `CatalogCrawlerManager`'s
    // Rust analogue lands.
    seed_sample_catalog(&*cache)
        .await
        .expect("seeding sample catalog failed");
    let state = AppState::new(cache);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|err| panic!("failed to bind {addr}: {err}"));
    tracing::info!("http-api listening on {addr}");

    axum::serve(listener, app)
        .await
        .expect("http-api server failed");
}
