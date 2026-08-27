//! Axum HTTP server skeleton for the federated catalog rewrite.
//!
//! Exposes a health check and a `GET /catalog` endpoint backed by
//! `rdf-store`'s `CatalogCache` trait. The catalog endpoint is
//! intentionally thin - it exists to prove the wiring between
//! `http-api`, `catalog-core` types, and the `rdf-store` cache trait works
//! end to end, not to be a finished Management API. Query parameters,
//! pagination shape, and response JSON-LD framing (EDC's Management API
//! returns `dspace:`/`edc:` JSON-LD) are all deferred to a later
//! iteration.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use catalog_core::{Catalog, DataService, Dataset, Distribution, NodeId};
use rdf_store::{CatalogCache, CatalogQuery, StoreResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Shared application state: just the cache, behind a trait object so the
/// concrete backend (in-memory today, RDF-backed later) is an
/// implementation detail of `main`, not of the router.
#[derive(Clone)]
pub struct AppState {
    pub cache: Arc<dyn CatalogCache>,
}

impl AppState {
    pub fn new(cache: Arc<dyn CatalogCache>) -> Self {
        Self { cache }
    }
}

/// Seed `cache` with one sample catalog, so a freshly started server (or a
/// test) has something real to serve from `GET /catalog` before any
/// crawler exists to populate it.
///
/// This stands in for the not-yet-built crawler: it upserts exactly the
/// same way a real crawl result would, through the public `CatalogCache`
/// trait, so it exercises the same end-to-end path as production code
/// rather than poking the cache's internals.
pub async fn seed_sample_catalog(cache: &dyn CatalogCache) -> StoreResult<()> {
    let node = NodeId::new("sample-participant");
    let mut catalog = Catalog::new("sample-catalog", node);
    catalog.participant_id = Some("did:example:sample-participant".to_string());
    catalog.datasets.push(Dataset {
        id: "sample-dataset".to_string(),
        properties: Default::default(),
        distributions: vec![Distribution {
            format: "application/json".to_string(),
            access_service: "sample-data-service".to_string(),
        }],
    });
    catalog.data_services.push(DataService {
        id: "sample-data-service".to_string(),
        endpoint_url: "https://sample.example.org/dsp".to_string(),
        endpoint_description: Some("dataspace-protocol-http:1.0".to_string()),
    });

    cache.upsert(catalog).await
}

/// Build the router. Kept separate from `main` so tests (and, later,
/// alternative binaries) can exercise it without binding a real socket.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/catalog", get(get_catalog))
        .route("/.well-known/dspace-version", get(dspace_version))
        .route("/dsp/catalog/request", post(catalog_request))
        .route("/dsp/catalog/datasets/{id}", get(get_dsp_dataset))
        .with_state(state)
}

#[derive(Debug, Serialize, Deserialize)]
struct HealthResponse {
    status: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

/// Optional filter for `GET /catalog`: `?node_id=...` narrows to the
/// catalog crawled from a single origin node.
#[derive(Debug, Deserialize)]
struct CatalogParams {
    node_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CatalogListResponse {
    catalogs: Vec<Catalog>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

async fn get_catalog(
    State(state): State<AppState>,
    Query(params): Query<CatalogParams>,
) -> impl IntoResponse {
    let query = match params.node_id {
        Some(node_id) => CatalogQuery::for_node(NodeId::new(node_id)),
        None => CatalogQuery::all(),
    };

    match state.cache.query(query).await {
        Ok(catalogs) => Json(CatalogListResponse { catalogs }).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

// --- Dataspace Protocol (DSP) v2025-1 endpoints -----------------------
//
// The routes below implement just enough of the real DSP wire protocol
// (as opposed to the Management-API-style `/catalog` stub above) to pass
// the `eclipse-dataspacetck/dsp-tck` suite's MET:01-01 and CAT:01-01/02/03
// tests: connector metadata discovery, and the catalog protocol's
// "request the whole catalog" / "look up one dataset" operations.
// Contract negotiation and transfer process are out of scope and are
// expected to keep failing the rest of that suite.

const DSP_CONTEXT_URL: &str = "https://w3id.org/dspace/2025/1/context.jsonld";

/// This connector's own participant id, as advertised in DSP catalog
/// responses. Matches `dataspacetck.dsp.connector.agent.id` in
/// `compliance/tck.properties` - both name the same connector.
const CONNECTOR_PARTICIPANT_ID: &str = "urn:connector:federated-catalog-rs";

fn new_urn_uuid() -> String {
    format!("urn:uuid:{}", Uuid::new_v4())
}

#[derive(Debug, Serialize)]
struct ProtocolVersionEntry {
    version: String,
    path: String,
    binding: String,
}

#[derive(Debug, Serialize)]
struct DspaceVersionResponse {
    #[serde(rename = "protocolVersions")]
    protocol_versions: Vec<ProtocolVersionEntry>,
}

/// `GET /.well-known/dspace-version` - plain JSON, no JSON-LD framing (the
/// TCK's `MetadataClient` does a plain Jackson deserialize of this one).
/// Lives at the HTTP root, not under `/dsp`. This alone is MET:01-01.
async fn dspace_version() -> Json<DspaceVersionResponse> {
    Json(DspaceVersionResponse {
        protocol_versions: vec![ProtocolVersionEntry {
            version: "2025-1".to_string(),
            path: "/dsp".to_string(),
            binding: "HTTPS".to_string(),
        }],
    })
}

#[derive(Debug, Serialize)]
struct DspPermission {
    #[serde(rename = "@type")]
    ld_type: String,
    action: String,
}

#[derive(Debug, Serialize)]
struct DspOffer {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@type")]
    ld_type: String,
    permission: Vec<DspPermission>,
}

/// Synthesize a single default "use" Offer for a dataset.
///
/// Placeholder: `catalog-core`'s `Dataset` has no real ODRL policy model
/// yet (see its doc comment), so every dataset gets exactly one
/// synthesized default-permission Offer here just so DSP's
/// `hasPolicy` (required, non-empty per the TCK's schema) is populated.
/// Real per-dataset policy modeling is future work, not built here.
fn placeholder_offer() -> DspOffer {
    DspOffer {
        id: new_urn_uuid(),
        ld_type: "Offer".to_string(),
        permission: vec![DspPermission {
            ld_type: "Permission".to_string(),
            action: "http://www.w3.org/ns/odrl/2/use".to_string(),
        }],
    }
}

#[derive(Debug, Serialize)]
struct DspDistribution {
    #[serde(rename = "@type")]
    ld_type: String,
    format: String,
    #[serde(rename = "accessService")]
    access_service: String,
}

impl From<Distribution> for DspDistribution {
    fn from(d: Distribution) -> Self {
        Self {
            ld_type: "Distribution".to_string(),
            format: d.format,
            access_service: d.access_service,
        }
    }
}

/// A DSP `Dataset` document. `context` is only present when this struct is
/// serialized as its own top-level document (`GET
/// /dsp/catalog/datasets/{id}`); when nested inside a `Catalog`'s
/// `dataset` array it is omitted, since JSON-LD framing is only needed at
/// the document root.
#[derive(Debug, Serialize)]
struct DspDataset {
    #[serde(rename = "@context", skip_serializing_if = "Option::is_none")]
    context: Option<Vec<String>>,
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@type")]
    ld_type: String,
    #[serde(rename = "hasPolicy")]
    has_policy: Vec<DspOffer>,
    distribution: Vec<DspDistribution>,
}

impl From<Dataset> for DspDataset {
    fn from(dataset: Dataset) -> Self {
        Self {
            context: None,
            id: dataset.id,
            ld_type: "Dataset".to_string(),
            has_policy: vec![placeholder_offer()],
            distribution: dataset.distributions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct DspDataService {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@type")]
    ld_type: String,
    #[serde(rename = "endpointURL")]
    endpoint_url: String,
}

impl From<DataService> for DspDataService {
    fn from(service: DataService) -> Self {
        Self {
            id: service.id,
            ld_type: "DataService".to_string(),
            endpoint_url: service.endpoint_url,
        }
    }
}

#[derive(Debug, Serialize)]
struct DspCatalog {
    #[serde(rename = "@context")]
    context: Vec<String>,
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@type")]
    ld_type: String,
    #[serde(rename = "participantId")]
    participant_id: String,
    dataset: Vec<DspDataset>,
    service: Vec<DspDataService>,
}

#[derive(Debug, Serialize)]
struct DspCatalogError {
    #[serde(rename = "@context")]
    context: Vec<String>,
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@type")]
    ld_type: String,
    code: String,
}

impl DspCatalogError {
    fn not_found() -> Self {
        Self {
            context: vec![DSP_CONTEXT_URL.to_string()],
            id: new_urn_uuid(),
            ld_type: "CatalogError".to_string(),
            code: "NOT_FOUND".to_string(),
        }
    }
}

/// Flatten every dataset (with its origin catalog's data services) out of
/// every catalog currently in the cache. This project's `CatalogCache`
/// models one crawled catalog per participant; the DSP catalog protocol
/// exposed here is this connector's own aggregate/federated view over all
/// of them - conceptually the same flattening EDC's own federated catalog
/// does over its crawled catalogs.
async fn flatten_cache(cache: &dyn CatalogCache) -> StoreResult<(Vec<Dataset>, Vec<DataService>)> {
    let catalogs = cache.query(CatalogQuery::all()).await?;
    let mut datasets = Vec::new();
    let mut services = Vec::new();
    for catalog in catalogs {
        datasets.extend(catalog.datasets);
        services.extend(catalog.data_services);
    }
    Ok((datasets, services))
}

/// `POST /dsp/catalog/request` - the DSP catalog protocol's "give me the
/// catalog" operation. The request body (a `CatalogRequestMessage`, which
/// may carry filters in a real implementation) is intentionally ignored
/// for now: this always returns the full flattened catalog.
///
/// Must never return 404, even when the cache is empty - the TCK's HTTP
/// client treats any 404 on this path as a hard failure. An empty
/// `dataset: []` is a perfectly valid response.
async fn catalog_request(State(state): State<AppState>) -> impl IntoResponse {
    match flatten_cache(&*state.cache).await {
        Ok((datasets, services)) => {
            let body = DspCatalog {
                context: vec![DSP_CONTEXT_URL.to_string()],
                id: new_urn_uuid(),
                ld_type: "Catalog".to_string(),
                participant_id: CONNECTOR_PARTICIPANT_ID.to_string(),
                dataset: datasets.into_iter().map(Into::into).collect(),
                service: services.into_iter().map(Into::into).collect(),
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

/// `GET /dsp/catalog/datasets/{id}` - look up one dataset by id across the
/// same flattened view `catalog_request` serves. Found: 200 with a
/// `Dataset` JSON-LD document. Not found: 404 with a `CatalogError`
/// document (this route, unlike `catalog_request`, is allowed - and
/// expected by the TCK - to 404).
async fn get_dsp_dataset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let (datasets, _services) = match flatten_cache(&*state.cache).await {
        Ok(flattened) => flattened,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };

    match datasets.into_iter().find(|dataset| dataset.id == id) {
        Some(dataset) => {
            let mut body: DspDataset = dataset.into();
            body.context = Some(vec![DSP_CONTEXT_URL.to_string()]);
            (StatusCode::OK, Json(body)).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(DspCatalogError::not_found())).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use rdf_store::memory::InMemoryCatalogCache;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState::new(Arc::new(InMemoryCatalogCache::new()))
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "ok");
    }

    #[tokio::test]
    async fn catalog_endpoint_returns_empty_list_when_cache_is_empty() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: CatalogListResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.catalogs.is_empty());
    }

    #[tokio::test]
    async fn catalog_endpoint_returns_upserted_catalog() {
        let state = test_state();
        let node = NodeId::new("node-1");
        state
            .cache
            .upsert(Catalog::new("cat-1", node.clone()))
            .await
            .unwrap();

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/catalog?node_id=node-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: CatalogListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.catalogs.len(), 1);
        assert_eq!(parsed.catalogs[0].id, "cat-1");
    }

    #[tokio::test]
    async fn catalog_endpoint_serves_seeded_sample_catalog() {
        let state = test_state();
        seed_sample_catalog(&*state.cache).await.unwrap();

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/catalog?node_id=sample-participant")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: CatalogListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.catalogs.len(), 1);
        let catalog = &parsed.catalogs[0];
        assert_eq!(catalog.id, "sample-catalog");
        assert_eq!(catalog.origin_node, NodeId::new("sample-participant"));
        assert_eq!(catalog.datasets.len(), 1);
        assert_eq!(catalog.datasets[0].id, "sample-dataset");
        assert_eq!(catalog.data_services.len(), 1);
    }

    #[tokio::test]
    async fn catalog_endpoint_filters_by_unknown_node_id() {
        let state = test_state();
        state
            .cache
            .upsert(Catalog::new("cat-1", NodeId::new("node-1")))
            .await
            .unwrap();

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/catalog?node_id=does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: CatalogListResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.catalogs.is_empty());
    }
}
