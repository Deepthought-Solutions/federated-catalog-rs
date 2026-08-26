# rust-federated-catalog

An iterative, from-scratch Rust rewrite of [Eclipse EDC](https://projects.eclipse.org/projects/technology.edc)'s
**Federated Catalog** module.

## What this is (and isn't)

This is a rewrite, not a port. The goal is a Federated Catalog that behaves
like EDC's — a crawler periodically pulls catalogs from known dataspace
participants and makes the aggregate queryable — designed from scratch in
Rust, taking the *shape* of the problem from EDC's Java implementation
without transliterating its classes, package layout, or internal
abstractions. Where a design choice here differs from EDC's (see each
crate's doc comments for specifics), that's deliberate, not an oversight.

The rewrite proceeds crate by crate, iteration by iteration: each crate
starts as a minimal skeleton that compiles and passes its own tests, and
gets built out once the next layer needs more from it. Nothing here is
meant to be feature-complete on first commit.

## Relationship to Eclipse EDC Federated Catalog

Eclipse EDC's Federated Catalog crawls a directory of known target nodes,
fetches their Dataspace Protocol (DSP) catalogs, and caches the result so
it can be queried locally without crawling on every request. The
reference implementation (Java, Gradle, OSGi-style extension model) lives
upstream at [eclipse-edc/Connector](https://github.com/eclipse-edc/Connector);
this repo's starting point is v0.18.0 of that project.

The crate boundaries here echo the module boundaries EDC draws between
its `crawler-spi` (generic, protocol-agnostic crawling contracts) and
`federated-catalog-spi` (the catalog-specific cache and query layer on
top), but as separate Rust crates rather than Java SPI modules:

- `catalog-core` — domain types, loosely modeled on EDC's
  `federated-catalog-spi` / `catalog-spi`: a participant/node identifier,
  a crawl work item, and the `Catalog` / `Dataset` / `DataService` model.
- `rdf-store` — the storage abstraction a federated catalog cache needs:
  upsert a participant's crawled catalog as a named graph, query stored
  catalogs, delete one by node id. Storage-agnostic by design (see next
  section) with one in-memory implementation for now.
- `http-api` — an Axum HTTP server exposing the catalog cache over HTTP,
  starting from a health check and a stub `GET /catalog`.

Not yet present: the crawler/scheduler loop itself (EDC's
`CatalogCrawlerManager`), a real DSP client, and the dataspace protocol
message types. Those arrive in later iterations once the storage and API
shape have proven out.

## Relationship to the `dataspace` study repo

This project originates from prior research done in the
[`dataspace`](https://labs.deepthought-solutions.net/Deepthought-Solutions/dataspace)
repository — a study of what's needed to deploy an EDC connector able to
host multiple participants. That repo's `docs/spikes/` directory holds
time-boxed, non-binding research spikes on the surrounding ecosystem
(other EDC-based connectors, integration points, framework comparisons),
and its `docs/adr/` directory records the architecture decisions that came
out of that research. The crawler architecture, cache semantics, and
query API described above were reconstructed there by reading EDC's
source directly (vendored as a submodule in that repo) before any Rust
code was written here.

## Why the RDF backend isn't wired in yet

`rdf-store` defines the cache trait as backend-agnostic on purpose. EDC
itself supports multiple `FederatedCatalogCache` backends (in-memory,
Postgres via a JSON column) behind one SPI; this project intends to land
on an actual RDF store — since a federated catalog is naturally a set of
named graphs. A research spike in the `dataspace` repo's `docs/spikes/`
surveyed the Rust RDF/quad-store ecosystem and recommends
[Oxigraph](https://crates.io/crates/oxigraph) as the target backend (see
the rationale in `rdf-store`'s module docs) — but that crate isn't
depended on here yet: it pulls in a native RocksDB build by default,
meaningfully heavier than anything else in this workspace, and isn't
worth adding before there's a concrete graph-naming/vocabulary scheme to
store against. When it lands, expect a real ADR-equivalent record of the
choice in the `dataspace` repo alongside the implementation, not just a
crate added quietly.

Until then, `rdf-store` ships one in-memory implementation — enough for
`http-api` and the crate's own tests to run against — behind the same
trait the eventual Oxigraph-backed implementation will satisfy.

## Layout

```
crates/
  catalog-core/   domain types (Catalog, Dataset, DataService, TargetNode, CrawlWorkItem)
  rdf-store/      CatalogCache trait + in-memory implementation
  http-api/       Axum server: /health, /catalog
```

## Building and testing

```bash
cargo build --workspace
cargo test --workspace
```

## License

Apache-2.0, matching upstream Eclipse EDC. See [LICENSE](LICENSE).
