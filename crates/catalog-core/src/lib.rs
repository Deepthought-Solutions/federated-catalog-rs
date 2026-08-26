//! Minimal domain types for the federated catalog rewrite.
//!
//! Modeled loosely on Eclipse EDC's `federated-catalog-spi` / `catalog-spi`
//! Java modules, but deliberately smaller: only what the `rdf-store` cache
//! trait needs to operate on. This is not a port - fields and shapes are a
//! from-scratch Rust design, not a transliteration of the Java classes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifier of a dataspace participant / crawl target node.
///
/// Corresponds to the `id` field of EDC's `TargetNode` record
/// (spi/crawler-spi).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A participant known to the crawler: enough to address it and pick a
/// protocol to speak.
///
/// Analogous to EDC's `TargetNode` record (spi/crawler-spi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetNode {
    pub id: NodeId,
    pub name: String,
    pub target_url: String,
    pub supported_protocols: Vec<String>,
}

/// One unit of crawl work: a target node plus how many times it has
/// already been retried in the current cycle.
///
/// EDC has no standalone `WorkItem` type at v0.18.0 - the equivalent is a
/// private `TargetNodeRetryCount` record local to `CatalogCrawlerManager`,
/// scoped to a single crawl attempt. It's promoted to a first-class,
/// public type here because a from-scratch design is free to name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlWorkItem {
    pub node: TargetNode,
    pub retries: u32,
}

impl CrawlWorkItem {
    pub fn new(node: TargetNode) -> Self {
        Self { node, retries: 0 }
    }
}

/// One concrete access method for a dataset: a data-plane endpoint plus
/// the format it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distribution {
    pub format: String,
    pub access_service: String,
}

/// A dataspace protocol-facing description of a data service (e.g. a
/// connector's DSP endpoint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataService {
    pub id: String,
    pub endpoint_url: String,
    #[serde(default)]
    pub endpoint_description: Option<String>,
}

/// One offered dataset: its id, arbitrary properties, and the
/// distributions it's available through.
///
/// EDC's `Dataset` also carries `offers: Map<String, Policy>`; policy
/// modeling is out of scope for this skeleton and will be added once a
/// consuming crate needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dataset {
    pub id: String,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
    #[serde(default)]
    pub distributions: Vec<Distribution>,
}

/// A crawled catalog: one participant's advertised datasets and data
/// services, as fetched by a single crawl of `origin_node`.
///
/// Modeled after EDC's `Catalog extends Dataset` (spi/control-plane/catalog-spi),
/// flattened here rather than inheriting from `Dataset` since Rust has no
/// class inheritance and the cache only ever stores whole catalogs, never
/// a bare `Dataset` standing in for one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub id: String,
    pub origin_node: NodeId,
    #[serde(default)]
    pub participant_id: Option<String>,
    #[serde(default)]
    pub datasets: Vec<Dataset>,
    #[serde(default)]
    pub data_services: Vec<DataService>,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

impl Catalog {
    pub fn new(id: impl Into<String>, origin_node: NodeId) -> Self {
        Self {
            id: id.into(),
            origin_node,
            participant_id: None,
            datasets: Vec::new(),
            data_services: Vec::new(),
            properties: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_new_has_empty_collections() {
        let cat = Catalog::new("cat-1", NodeId::new("node-1"));
        assert_eq!(cat.id, "cat-1");
        assert_eq!(cat.origin_node, NodeId::new("node-1"));
        assert!(cat.datasets.is_empty());
        assert!(cat.data_services.is_empty());
    }

    #[test]
    fn crawl_work_item_starts_at_zero_retries() {
        let node = TargetNode {
            id: NodeId::new("node-1"),
            name: "node-1".into(),
            target_url: "https://example.org/dsp".into(),
            supported_protocols: vec!["dataspace-protocol-http".into()],
        };
        let item = CrawlWorkItem::new(node);
        assert_eq!(item.retries, 0);
    }

    #[test]
    fn node_id_display_matches_inner_string() {
        let id = NodeId::new("abc");
        assert_eq!(id.to_string(), "abc");
    }
}
