# federated-catalog-rs

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
  catalogs, delete one by node id. Storage-agnostic by design (see below),
  with an in-memory implementation and an Oxigraph-backed implementation.
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

## The RDF backend

`rdf-store` defines the cache trait as backend-agnostic on purpose. EDC
itself supports multiple `FederatedCatalogCache` backends (in-memory,
Postgres via a JSON column) behind one SPI; this project has now landed
on an actual RDF store — since a federated catalog is naturally a set of
named graphs. A research spike in the `dataspace` repo's `docs/spikes/`
surveyed the Rust RDF/quad-store ecosystem and recommended
[Oxigraph](https://crates.io/crates/oxigraph) as the target backend, and
`rdf-store`'s `oxigraph_backend::OxigraphCatalogCache` now implements
`CatalogCache` on top of it — via `contreforts-kg`, an existing internal
Oxigraph wrapper from a separate private repo, rather than the bare
`oxigraph` crate directly. See `rdf-store`'s module docs for the full
rationale, the quad-mapping scheme, and why it's still a "first cut"
JSON-blob bridge rather than full RDF decomposition.

`http-api` still defaults to the in-memory implementation; swapping the
running server over to the Oxigraph-backed one is a separate, future
decision. Both implementations satisfy the same `CatalogCache` trait and
are exercised by equivalent test suites, so either can back `http-api`
without changing its code.

## Vendored dependencies

`contreforts-kg` and its own two hard dependencies (`contreforts-core`,
`contreforts-config`) are vendored as git submodules under `vendor/` and
are real members of this workspace - not a separate, excluded one - so
this repo's own root `Cargo.toml` decides their shared dependency
versions and feature defaults (including Oxigraph's `rocksdb` feature).
See [`vendor/README.md`](vendor/README.md) for what's vendored, why, and
a known metadata caveat (inherited `license`/`edition` on those crates
doesn't match their own upstream `Cargo.toml`).

## Layout

```
crates/
  catalog-core/   domain types (Catalog, Dataset, DataService, TargetNode, CrawlWorkItem)
  rdf-store/      CatalogCache trait + in-memory and Oxigraph-backed implementations
  http-api/       Axum server: /health, /catalog
vendor/
  contreforts-kg/      Oxigraph wrapper (GraphStore, QueryEngine) - rdf-store's real backend
  contreforts-core/    contreforts-kg's own dependency (shared error/connector types)
  contreforts-config/  contreforts-kg's own dependency (a second, separate Oxigraph store)
```

## Building and testing

```bash
cargo build --workspace
cargo test --workspace
```

## License

Apache-2.0, matching upstream Eclipse EDC. See [LICENSE](LICENSE).
