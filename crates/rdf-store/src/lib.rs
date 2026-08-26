//! Storage abstraction for the federated catalog cache.
//!
//! Named `rdf-store` because a crawled catalog is expected to eventually be
//! represented as an RDF named graph (one graph per participant), but the
//! [`CatalogCache`] trait itself is backend-agnostic. Which RDF store (or
//! whether RDF at all, vs. e.g. a plain document store) backs this is a
//! decision being made iteratively - see the `dataspace` study repo's
//! `docs/spikes/` for the exploration behind that choice - so this crate
//! only fixes the shape of the trait plus one in-memory implementation
//! good enough to unblock `http-api` and its own tests.
//!
//! The trait mirrors Eclipse EDC's `FederatedCatalogCache` SPI
//! (`save`/`query`/`deleteExpired`/`expireAll`) but adapted to a
//! from-scratch design: EDC's mark-then-sweep expiry (`deleteExpired` +
//! `expireAll` called every crawl tick) is replaced here by an explicit
//! `delete(node)`, since the tick/expiry policy belongs to a future
//! crawler crate, not to the storage trait itself. Like EDC, each
//! participant's crawled catalog is upserted as a whole unit keyed by
//! origin node - not decomposed into per-dataset rows - mirroring EDC's
//! choice to key the whole `Catalog` object graph by origin node URL.
//!
//! ## Intended future backend: Oxigraph
//!
//! A research spike in the `dataspace` study repo (`docs/spikes/`) surveyed
//! the available Rust RDF/quad-store crates against this trait's shape -
//! a named-graph quad store, one graph per
//! crawled source-node IRI, upserted wholesale on crawl, removed via an
//! explicit delete - and recommended **[Oxigraph](https://crates.io/crates/oxigraph)**
//! as the eventual backend:
//!
//! - It is the only surveyed candidate that both matches the shape (named
//!   graphs, full SPARQL 1.1 Query/Update/Federated Query) and has a
//!   maturity track record to build on now: ~8 years old, actively
//!   maintained, ~700k crates.io downloads, dual Apache-2.0/MIT, with a
//!   persistent RocksDB-backed store.
//! - `cool-japan/oxirs` is the closer long-term architectural fit
//!   (async-native, purpose-built on-disk format, also named-graph
//!   capable) but at spike time was a 14-month-old, effectively
//!   single-maintainer workspace whose persistent backend had shipped only
//!   five weeks earlier, with unaudited test/scale claims - not a
//!   foundation to bet this rewrite's first concrete cache backend on.
//! - No other surveyed crate (Sophia, terminusdb-store, hdt, Grafeo,
//!   kglite, rust-rdftk) beat Oxigraph on the combination of shape-fit and
//!   maturity.
//!
//! **This crate does not depend on Oxigraph yet.** Oxigraph's default
//! build pulls in `oxrocksdb-sys` (a native RocksDB C++ build), which is
//! a meaningfully heavier and slower dependency than anything else in
//! this workspace and needs build tooling (e.g. `cmake`) this repo
//! doesn't otherwise require - not something to add speculatively before
//! there's a real RDF vocabulary/graph-naming scheme to store. Wiring it
//! in is expected to be a dedicated iteration: add `oxigraph` as a
//! dependency, decide how a [`Catalog`] maps to quads (or, as a first
//! cut, one blob triple per named graph as a bridge before real
//! decomposition), and implement [`CatalogCache`] against
//! `oxigraph::store::Store` behind this same trait - so `http-api` and
//! this crate's tests don't need to change when the swap happens. Track
//! the decision as an ADR-equivalent record in the `dataspace` study repo
//! when it lands, per that repo's `docs/adr/` convention.

use async_trait::async_trait;
use catalog_core::{Catalog, NodeId};
use thiserror::Error;

/// Errors a [`CatalogCache`] backend can report.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("catalog store backend error: {0}")]
    Backend(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// A query over stored catalogs.
///
/// Deliberately minimal - a from-scratch analogue of EDC's `QuerySpec`,
/// sized to what the in-memory backend here can serve. A real RDF backend
/// will likely need a richer query shape (e.g. SPARQL passthrough); that
/// is expected to extend or replace this type, not be forced through it.
#[derive(Debug, Clone, Default)]
pub struct CatalogQuery {
    pub origin_node: Option<NodeId>,
    pub offset: usize,
    pub limit: Option<usize>,
}

impl CatalogQuery {
    /// No filter, no offset, no limit.
    pub fn all() -> Self {
        Self::default()
    }

    /// Only the catalog crawled from `node`, if any.
    pub fn for_node(node: NodeId) -> Self {
        Self {
            origin_node: Some(node),
            ..Self::default()
        }
    }
}

/// Storage for crawled catalogs, one named graph per origin node.
///
/// Implementations must be safe to share across crawl tasks (`Send +
/// Sync`) since a real crawler runs multiple concurrent fetches.
#[async_trait]
pub trait CatalogCache: Send + Sync {
    /// Insert or replace the named graph for `catalog.origin_node`.
    ///
    /// Re-crawling the same node always overwrites its prior catalog
    /// wholesale, matching EDC's upsert-by-origin-node-url behavior.
    async fn upsert(&self, catalog: Catalog) -> StoreResult<()>;

    /// Return stored catalogs matching `query`.
    async fn query(&self, query: CatalogQuery) -> StoreResult<Vec<Catalog>>;

    /// Remove the named graph for `node`, if present.
    ///
    /// Returns `true` if a catalog was actually removed.
    async fn delete(&self, node: &NodeId) -> StoreResult<bool>;
}

/// A simple, non-persistent [`CatalogCache`] backed by an in-process map.
///
/// This exists so `http-api` (and this crate's own tests) have something
/// to run against while the real RDF-backed implementation is chosen; it
/// is not intended to be the production backend.
pub mod memory {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    #[derive(Default)]
    pub struct InMemoryCatalogCache {
        graphs: RwLock<HashMap<NodeId, Catalog>>,
    }

    impl InMemoryCatalogCache {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait]
    impl CatalogCache for InMemoryCatalogCache {
        async fn upsert(&self, catalog: Catalog) -> StoreResult<()> {
            let mut graphs = self.graphs.write().await;
            graphs.insert(catalog.origin_node.clone(), catalog);
            Ok(())
        }

        async fn query(&self, query: CatalogQuery) -> StoreResult<Vec<Catalog>> {
            let graphs = self.graphs.read().await;
            let mut results: Vec<Catalog> = graphs
                .values()
                .filter(|catalog| match &query.origin_node {
                    Some(node) => &catalog.origin_node == node,
                    None => true,
                })
                .cloned()
                .collect();
            // Deterministic ordering: HashMap iteration order isn't
            // stable, and callers (e.g. http-api) need reproducible
            // pagination.
            results.sort_by(|a, b| a.id.cmp(&b.id));

            let skipped = results.into_iter().skip(query.offset);
            let limited: Vec<Catalog> = match query.limit {
                Some(limit) => skipped.take(limit).collect(),
                None => skipped.collect(),
            };
            Ok(limited)
        }

        async fn delete(&self, node: &NodeId) -> StoreResult<bool> {
            let mut graphs = self.graphs.write().await;
            Ok(graphs.remove(node).is_some())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn sample_catalog(node: &str, id: &str) -> Catalog {
            Catalog::new(id, NodeId::new(node))
        }

        #[tokio::test]
        async fn upsert_then_query_all_returns_it() {
            let cache = InMemoryCatalogCache::new();
            cache
                .upsert(sample_catalog("node-1", "cat-1"))
                .await
                .unwrap();

            let results = cache.query(CatalogQuery::all()).await.unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].id, "cat-1");
        }

        #[tokio::test]
        async fn upsert_replaces_prior_catalog_for_same_node() {
            let cache = InMemoryCatalogCache::new();
            cache
                .upsert(sample_catalog("node-1", "cat-1"))
                .await
                .unwrap();
            cache
                .upsert(sample_catalog("node-1", "cat-2"))
                .await
                .unwrap();

            let results = cache.query(CatalogQuery::all()).await.unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].id, "cat-2");
        }

        #[tokio::test]
        async fn query_for_node_filters_by_origin() {
            let cache = InMemoryCatalogCache::new();
            cache
                .upsert(sample_catalog("node-1", "cat-1"))
                .await
                .unwrap();
            cache
                .upsert(sample_catalog("node-2", "cat-2"))
                .await
                .unwrap();

            let results = cache
                .query(CatalogQuery::for_node(NodeId::new("node-2")))
                .await
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].id, "cat-2");
        }

        #[tokio::test]
        async fn query_respects_offset_and_limit() {
            let cache = InMemoryCatalogCache::new();
            for i in 0..5 {
                cache
                    .upsert(sample_catalog(&format!("node-{i}"), &format!("cat-{i}")))
                    .await
                    .unwrap();
            }

            let results = cache
                .query(CatalogQuery {
                    origin_node: None,
                    offset: 2,
                    limit: Some(2),
                })
                .await
                .unwrap();
            assert_eq!(
                results.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
                vec!["cat-2", "cat-3"]
            );
        }

        #[tokio::test]
        async fn delete_removes_catalog_and_reports_result() {
            let cache = InMemoryCatalogCache::new();
            cache
                .upsert(sample_catalog("node-1", "cat-1"))
                .await
                .unwrap();

            let removed = cache.delete(&NodeId::new("node-1")).await.unwrap();
            assert!(removed);

            let results = cache.query(CatalogQuery::all()).await.unwrap();
            assert!(results.is_empty());

            let removed_again = cache.delete(&NodeId::new("node-1")).await.unwrap();
            assert!(!removed_again);
        }
    }
}
