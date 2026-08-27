//! Hermetic, in-process multi-participant crawl integration test.
//!
//! Everything here runs inside this one test binary: no real EDC/JVM
//! process, no external network. Three `http-api` server instances are
//! bound to OS-assigned `127.0.0.1` ports and driven with real HTTP calls
//! (`reqwest`) through the crawler's actual `crawler::crawl_once`:
//!
//! - **O ("open")**: `DspAuthMode::Disabled`, seeded (directly via the
//!   cache's own `upsert`, not `seed_sample_catalog`) with distinct
//!   `OPEN-01`/`OPEN-02` datasets. No auth needed.
//! - **P ("provider", DCP-gated)**: `DspAuthMode::Dcp`, seeded with
//!   distinct `GATED-01`/`GATED-02` datasets. Only reachable with a valid
//!   DCP self-issued token backed by a Verifiable Credential that grants
//!   `GATED-01` (and, deliberately, *not* `GATED-02`).
//! - **H ("holder")**: the crawler's own `dcp_core::HolderIdentity` -
//!   `/dsp/holder/presentations/query` is the real, unmodified route
//!   `http_api::build_router` already serves; `answer_presentation_query`
//!   is exercised exactly as production code calls it.
//!
//! ## Three real bugs found while first writing this test, now fixed
//!
//! The first version of this test surfaced three genuine, confirmed bugs
//! that made even the DCP happy path unreachable via real, unmodified
//! routes. All three are now fixed in the implementation (not worked
//! around test-side) - the tests below exercise the real, unmodified
//! `/dsp/did.json`, `/dsp/holder/did.json`, and
//! `/dsp/holder/presentations/query` routes directly, with no
//! test-side route substitution.
//!
//! 1. `dcp_core::did_web_to_url`'s non-empty-path-segments branch never
//!    appended the `/did.json` suffix its own doc comment described, and
//!    `HolderIdentity::new` built a holder's own DID with a single
//!    hyphenated `dsp-holder` path segment rather than the two-segment
//!    `dsp:holder` that resolves (via `did_web_to_url`'s
//!    segments-joined-by-"/" rule) to the real registered route
//!    `/dsp/holder/did.json`. See
//!    [`resolve_did_reaches_the_real_dsp_did_routes`] below - it directly
//!    exercises `did_web_to_url` against both the verifier's and the
//!    holder's real DID and confirms the computed URL now matches the
//!    real route exactly.
//! 2. `http-api::dcp::verify_dcp_bearer_token` builds the Presentation API
//!    query URL by appending `/presentations/query` to whatever the
//!    `CredentialService` `serviceEndpoint` is - a convention already
//!    validated against a real running `eclipse-edc/IdentityHub` (see
//!    `compliance/benchmark-dcp-2026-08-27.md`), i.e. that endpoint is
//!    expected to be a *base* URL. `HolderIdentity::own_did_document` was
//!    publishing the *already-complete* endpoint instead, so the append
//!    landed on a URL with the suffix doubled and 404'd. Fixed by making
//!    `HolderIdentity::own_did_document` publish the base URL, conforming
//!    to the verifier's pre-existing, already-proven convention. See
//!    [`credential_service_endpoint_is_the_real_reachable_presentation_api`]
//!    below.
//! 3. `verify_dcp_bearer_token`'s per-VC loop treated an expired
//!    credential as "skip it, no error" - if the *only* VC in a
//!    presentation was expired, the function still returned
//!    `Ok(VerifiedCaller { catalog_access: {} })`, indistinguishable from
//!    a caller genuinely, correctly authorized for zero datasets.
//!    `crawler::crawl_one` then saw `Ok` and `crawl_once` overwrote that
//!    node's previously-good cached catalog with an empty one. Fixed: an
//!    all-expired presentation now returns `Err`, recorded as a crawl
//!    failure. See
//!    [`crawl_once_records_a_failure_for_an_expired_dcp_credential_and_preserves_prior_cache_data`]
//!    below - this test's assertions were never weakened; it simply
//!    passes now.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use catalog_core::{Catalog, Dataset, NodeId};
use crawler::{ParticipantEntry, crawl_once};
use dcp_core::{DcpKeyPair, HolderIdentity, did_web_to_url, now_secs, sign_jws};
use http_api::{AppState, DcpConfig, DspAuthConfig, DspAuthMode, build_router};
use rdf_store::memory::InMemoryCatalogCache;
use rdf_store::{CatalogCache, CatalogQuery};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const SCOPE: &str = "org.eclipse.dspace.dcp.vc.type:FederatedCatalogAccessCredential:read";
/// `http-api::main::load_dsp_auth` always derives `did:web:<host>:dsp` for
/// `DspAuthMode::Dcp`.
const VERIFIER_DID_PATH_SEGMENT: &str = "dsp";

async fn bind_localhost() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    (listener, addr)
}

fn spawn(listener: TcpListener, app: Router) -> JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum::serve");
    })
}

fn dataset(id: &str) -> Dataset {
    Dataset {
        id: id.to_string(),
        properties: Default::default(),
        distributions: Vec::new(),
    }
}

// --- Regression tests for the three bugs described in the module doc --

/// Regression test for bug #1: `did_web_to_url` (the function
/// `resolve_did`/`answer_presentation_query` use internally) now computes
/// the exact URL a real, unmodified `http-api` instance actually serves,
/// for both the verifier's own DID and a holder's own DID - no test-side
/// route substitution.
#[tokio::test]
async fn resolve_did_reaches_the_real_dsp_did_routes() {
    let (listener, addr) = bind_localhost().await;
    let host = format!("127.0.0.1:{}", addr.port());
    let holder = Arc::new(HolderIdentity::new(host.clone(), true, "unused.unused.unused".to_string(), SCOPE.to_string()));
    let holder_did = holder.key_pair.own_did.clone();
    let verifier_did = format!("did:web:{}:{VERIFIER_DID_PATH_SEGMENT}", host.replace(':', "%3A"));
    let dcp_config = DcpConfig::generate(verifier_did.clone(), true, SCOPE.to_string());

    let dsp_auth = DspAuthConfig {
        mode: DspAuthMode::Dcp,
        catalog_access: HashMap::new(),
        dcp: Some(dcp_config),
    };
    let state = AppState::new(Arc::new(InMemoryCatalogCache::new())).with_dsp_auth(dsp_auth).with_holder(Some(holder));
    let app = build_router(state);
    let _server = spawn(listener, app);

    let client = reqwest::Client::new();
    let base = format!("http://{host}");

    let holder_url = did_web_to_url(&holder_did, true).expect("did_web_to_url");
    assert_eq!(holder_url, format!("{base}/dsp/holder/did.json"));
    let holder_response = client.get(&holder_url).send().await.expect("GET holder DID doc");
    assert_eq!(holder_response.status(), reqwest::StatusCode::OK, "computed holder DID URL reaches the real route");

    let verifier_url = did_web_to_url(&verifier_did, true).expect("did_web_to_url");
    assert_eq!(verifier_url, format!("{base}/dsp/did.json"));
    let verifier_response = client.get(&verifier_url).send().await.expect("GET verifier DID doc");
    assert_eq!(verifier_response.status(), reqwest::StatusCode::OK, "computed verifier DID URL reaches the real route");
}

/// Regression test for bug #2: `HolderIdentity::own_did_document` now
/// publishes a *base* `CredentialService` endpoint, and
/// `format!("{endpoint}/presentations/query")` - exactly what
/// `verify_dcp_bearer_token` builds - reaches the real, unmodified route.
#[tokio::test]
async fn credential_service_endpoint_is_the_real_reachable_presentation_api() {
    let (listener, addr) = bind_localhost().await;
    let host = format!("127.0.0.1:{}", addr.port());
    let holder = Arc::new(HolderIdentity::new(host.clone(), true, "unused.unused.unused".to_string(), SCOPE.to_string()));
    let published_endpoint = holder
        .own_did_document()
        .get("service")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get("serviceEndpoint"))
        .and_then(|v| v.as_str())
        .expect("CredentialService entry with a serviceEndpoint")
        .to_string();
    assert_eq!(published_endpoint, format!("http://{host}/dsp/holder"), "published endpoint is the base URL");

    let state = AppState::new(Arc::new(InMemoryCatalogCache::new())).with_holder(Some(holder));
    let app = build_router(state);
    let _server = spawn(listener, app);

    let client = reqwest::Client::new();
    let query_url = format!("{published_endpoint}/presentations/query");
    assert_eq!(query_url, format!("http://{host}/dsp/holder/presentations/query"));

    // No auth header: 401 (the real handler ran and rejected it), not 404
    // (which would mean the route doesn't exist at this address).
    let response = client.post(&query_url).send().await.expect("POST presentations/query");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED, "the real route is reachable at exactly the URL verify_dcp_bearer_token builds");
}

// --- Wiring for the happy-path / negative-path scenarios below --------

struct OpenParticipant {
    entry: ParticipantEntry,
    _server: JoinHandle<()>,
}

async fn spawn_open_participant() -> OpenParticipant {
    let (listener, addr) = bind_localhost().await;
    let cache = Arc::new(InMemoryCatalogCache::new());
    let mut catalog = Catalog::new("open-catalog", NodeId::new("open-participant"));
    catalog.participant_id = Some("did:example:open-participant".to_string());
    catalog.datasets.push(dataset("OPEN-01"));
    catalog.datasets.push(dataset("OPEN-02"));
    cache.upsert(catalog).await.expect("seed open catalog");

    let state = AppState::new(cache);
    let server = spawn(listener, build_router(state));

    OpenParticipant {
        entry: ParticipantEntry {
            id: "open-participant".to_string(),
            name: "Open participant".to_string(),
            catalog_request_url: format!("http://{addr}/dsp/catalog/request"),
            requires_dcp: false,
            provider_did: None,
        },
        _server: server,
    }
}

struct GatedParticipant {
    entry: ParticipantEntry,
    _server: JoinHandle<()>,
}

/// Spin up the DCP-gated provider (Instance P), seeded with
/// `GATED-01`/`GATED-02`, serving its real, unmodified `/dsp/did.json`
/// route.
async fn spawn_gated_participant() -> GatedParticipant {
    let (listener, addr) = bind_localhost().await;
    let host = format!("127.0.0.1:{}", addr.port());
    let own_did = format!("did:web:{}:{VERIFIER_DID_PATH_SEGMENT}", host.replace(':', "%3A"));
    let dcp_config = DcpConfig::generate(own_did.clone(), true, SCOPE.to_string());

    let cache = Arc::new(InMemoryCatalogCache::new());
    let mut catalog = Catalog::new("gated-catalog", NodeId::new("gated-participant"));
    catalog.participant_id = Some("did:example:gated-participant".to_string());
    catalog.datasets.push(dataset("GATED-01"));
    catalog.datasets.push(dataset("GATED-02"));
    cache.upsert(catalog).await.expect("seed gated catalog");

    let dsp_auth = DspAuthConfig {
        mode: DspAuthMode::Dcp,
        catalog_access: HashMap::new(),
        dcp: Some(dcp_config),
    };
    let state = AppState::new(cache).with_dsp_auth(dsp_auth);
    let server = spawn(listener, build_router(state));

    GatedParticipant {
        entry: ParticipantEntry {
            id: "gated-participant".to_string(),
            name: "DCP-gated participant".to_string(),
            catalog_request_url: format!("http://{addr}/dsp/catalog/request"),
            requires_dcp: true,
            provider_did: Some(own_did),
        },
        _server: server,
    }
}

struct HolderRig {
    holder: Arc<HolderIdentity>,
    _server: JoinHandle<()>,
}

/// Spin up Instance H (the crawler's own DCP holder identity, reused as
/// the Presentation API callback target): a real `HolderIdentity` serving
/// the real, unmodified `/dsp/holder/did.json` and
/// `/dsp/holder/presentations/query` routes - no test-side route
/// substitution needed now that bugs #1/#2 are fixed.
///
/// `credential_jws_for` receives the holder's own DID (only known once
/// `HolderIdentity::new` has run inside this function) and returns the
/// already-signed VC JWS `answer_presentation_query` will serve.
async fn spawn_holder(credential_jws_for: impl FnOnce(&str) -> String) -> HolderRig {
    let (listener, addr) = bind_localhost().await;
    let host = format!("127.0.0.1:{}", addr.port());

    // HolderIdentity::new generates a fresh key every call (see its doc
    // comment) - this must be the one and only instance we build and then
    // finalize below, not a second throwaway.
    let mut holder = HolderIdentity::new(host.clone(), true, String::new(), SCOPE.to_string());
    let holder_did = holder.key_pair.own_did.clone();
    holder.credential_jws = credential_jws_for(&holder_did);

    let holder = Arc::new(holder);
    let state = AppState::new(Arc::new(InMemoryCatalogCache::new())).with_holder(Some(holder.clone()));
    let server = spawn(listener, build_router(state));

    HolderRig { holder, _server: server }
}

/// Spin up a minimal, standalone `did:web` identity for a credential
/// issuer - a real party distinct from the holder, as real DCP has it
/// (rather than co-residing the issuer's key in the holder's own DID
/// document, a simplification that isn't necessary now that DID
/// resolution actually works end to end). Path-segment-free DIDs
/// (`did:web:<host>`, no further `:segment`s) resolve to
/// `/.well-known/did.json` per `did_web_to_url` - that branch was never
/// affected by bug #1, so no `http-api` dependency is needed here at all.
async fn spawn_issuer() -> (DcpKeyPair, JoinHandle<()>) {
    let (listener, addr) = bind_localhost().await;
    let issuer_did = format!("did:web:127.0.0.1%3A{}", addr.port());
    let issuer_key = DcpKeyPair::generate(issuer_did);
    let did_doc = issuer_key.did_document(&[]);

    let app = Router::new().route(
        "/.well-known/did.json",
        get(move || {
            let did_doc = did_doc.clone();
            async move { Json(did_doc) }
        }),
    );
    let server = spawn(listener, app);
    (issuer_key, server)
}

/// Sign a `FederatedCatalogAccessCredential` VC JWS shaped exactly as
/// `http_api::dcp::verify_dcp_bearer_token`'s VC-verification steps expect
/// to parse: `iss` = the issuer's own DID, `sub` = holder DID, `vc.type`
/// includes `FederatedCatalogAccessCredential`,
/// `vc.credentialSubject.catalogAccess` = the granted dataset ids, `exp` =
/// the given expiry (a past timestamp produces a deliberately expired
/// credential).
fn issue_credential(issuer_key: &DcpKeyPair, holder_did: &str, catalog_access: &[&str], exp: u64) -> String {
    let payload = json!({
        "iss": issuer_key.own_did,
        "sub": holder_did,
        "vc": {
            "type": ["VerifiableCredential", "FederatedCatalogAccessCredential"],
            "credentialSubject": {
                "catalogAccess": catalog_access,
            }
        },
        "exp": exp,
    });
    sign_jws(&payload, &issuer_key.signing_key(), &issuer_key.own_key_id)
}

// --- Happy path: two participants, one gated with real per-caller ------
// --- DCP filtering ------------------------------------------------------

#[tokio::test]
async fn crawl_once_pulls_open_and_dcp_gated_catalogs_with_real_per_caller_filtering() {
    let _ = tracing_subscriber::fmt().with_env_filter("warn,http_api=debug,dcp_core=debug").try_init();
    let open = spawn_open_participant().await;
    let gated = spawn_gated_participant().await;

    let (issuer_key, _issuer_server) = spawn_issuer().await;
    let holder_rig = spawn_holder(move |holder_did| {
        // Grants GATED-01 only - GATED-02 must never appear in the
        // crawler's result for this participant.
        issue_credential(&issuer_key, holder_did, &["GATED-01"], now_secs() + 3600)
    })
    .await;

    let participants = vec![clone_entry(&open.entry), clone_entry(&gated.entry)];
    let http = reqwest::Client::new();
    let cache = InMemoryCatalogCache::new();

    let summary = crawl_once(&http, &participants, Some(holder_rig.holder.as_ref()), &cache).await;

    assert_eq!(summary.attempted, 2, "attempted: {summary:?}");
    assert_eq!(summary.failed, 0, "failed (failures: {:?}): {summary:?}", summary.failures);
    assert_eq!(summary.succeeded, 2, "succeeded: {summary:?}");

    let open_catalogs = cache.query(CatalogQuery::for_node(NodeId::new("open-participant"))).await.unwrap();
    assert_eq!(open_catalogs.len(), 1);
    let mut open_ids: Vec<&str> = open_catalogs[0].datasets.iter().map(|d| d.id.as_str()).collect();
    open_ids.sort_unstable();
    assert_eq!(open_ids, vec!["OPEN-01", "OPEN-02"]);

    let gated_catalogs = cache.query(CatalogQuery::for_node(NodeId::new("gated-participant"))).await.unwrap();
    assert_eq!(gated_catalogs.len(), 1);
    let gated_ids: Vec<&str> = gated_catalogs[0].datasets.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(gated_ids, vec!["GATED-01"], "GATED-02 was never granted by the credential and must not be visible");
}

// --- Negative path: an expired credential must not be treated as a ----
// --- silent, empty success --------------------------------------------

/// This test is written to assert the behavior the task briefing calls
/// for, and - once bugs #1 and #2 above are worked around the same way as
/// the happy-path test - it runs far enough to reach real signal. That
/// signal is a **third confirmed bug**: with only one VC in the
/// presentation and that VC expired, `verify_dcp_bearer_token`'s per-VC
/// loop (`http-api/src/dcp.rs`) does `continue` (skip granting that VC's
/// access) rather than treating the presentation as a failure, so the
/// function still returns `Ok(VerifiedCaller { catalog_access: {} })`. The
/// DSP endpoint therefore answers `200 OK` with an empty `dataset: []`
/// (identical to "authenticated, but genuinely granted nothing" - see
/// `visible_datasets`'s doc comment), so `crawler::crawl_one` sees `Ok`,
/// `crawl_once` calls `cache.upsert` on the resulting *empty* catalog, and
/// that overwrites this node's prior good cached data. Run directly (see
/// this crate's PR/report), the actual observed result was
/// `CrawlSummary { attempted: 1, succeeded: 1, failed: 0, failures: [] }`
/// and the cache ended up holding a fresh, empty `Catalog` in place of the
/// seeded `prior_good` one - i.e. exactly the failure mode this test's
/// assertions describe. This test is intentionally left asserting the
/// *correct* behavior (a recorded failure, prior data preserved) rather
/// than the current one, per the task's instruction not to weaken
/// assertions to force a pass - so it is expected to fail until that gap
/// is fixed.
#[tokio::test]
async fn crawl_once_records_a_failure_for_an_expired_dcp_credential_and_preserves_prior_cache_data() {
    let gated = spawn_gated_participant().await;

    let (issuer_key, _issuer_server) = spawn_issuer().await;
    let holder_rig = spawn_holder(move |holder_did| {
        // exp in the past: this credential is already expired the moment
        // it's presented.
        issue_credential(&issuer_key, holder_did, &["GATED-01"], now_secs().saturating_sub(3600))
    })
    .await;

    let participants = vec![clone_entry(&gated.entry)];
    let http = reqwest::Client::new();
    let cache = InMemoryCatalogCache::new();

    // Seed the cache with prior, good crawl data for this same node -
    // proof that a failed re-crawl must not clobber it.
    let mut prior_good = Catalog::new("gated-catalog-prior", NodeId::new("gated-participant"));
    prior_good.datasets.push(dataset("GATED-01"));
    cache.upsert(prior_good.clone()).await.unwrap();

    let summary = crawl_once(&http, &participants, Some(holder_rig.holder.as_ref()), &cache).await;

    assert_eq!(summary.attempted, 1, "attempted: {summary:?}");
    assert_eq!(
        summary.failed, 1,
        "an expired credential must be recorded as a crawl failure, not a silent empty success: {summary:?}"
    );
    assert_eq!(summary.succeeded, 0, "succeeded: {summary:?}");

    let stored = cache.query(CatalogQuery::for_node(NodeId::new("gated-participant"))).await.unwrap();
    assert_eq!(stored.len(), 1, "prior cached catalog for this node must still be present");
    assert_eq!(
        stored[0], prior_good,
        "a failed crawl must not overwrite the previously cached good catalog for this node"
    );
}

/// `ParticipantEntry` has no `Clone` derive (it's not needed anywhere in
/// production code), but this test file wants to keep each fixture's
/// `entry` alongside its still-alive server handle while also building a
/// `Vec<ParticipantEntry>` to hand to `crawl_once` - so build the vec by
/// hand instead of adding a `Clone` derive to production code for a
/// test-only convenience.
fn clone_entry(entry: &ParticipantEntry) -> ParticipantEntry {
    ParticipantEntry {
        id: entry.id.clone(),
        name: entry.name.clone(),
        catalog_request_url: entry.catalog_request_url.clone(),
        requires_dcp: entry.requires_dcp,
        provider_did: entry.provider_did.clone(),
    }
}
