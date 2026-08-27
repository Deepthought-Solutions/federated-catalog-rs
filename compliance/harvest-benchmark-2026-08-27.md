# Harvesting benchmark: EDC's own federated-catalog crawler vs. `crates/crawler` + `http-api`

**Date:** 2026-08-27

**Corrected re-run (same date):** the first pass through this benchmark
exposed a real bug - Rust's `POST /dsp/catalog/request` flattened every
crawled participant's datasets into one `Catalog`'s `dataset` array,
while EDC's own federated-catalog Management API kept one `Catalog`
object per crawled participant. That asymmetry was visible in this
report's own "Aggregated dataset count served" row (originally: `10
(flattened into 1 Catalog)` vs. `10 (across 2 Catalog entries, one per
crawled participant)`) and has since been fixed in
`crates/http-api/src/lib.rs`'s `catalog_request` (nest one `DspCatalog`
per origin node under a top-level `catalog[]` field, matching EDC's own
per-participant grouping, whenever the cache holds 2+ distinct origin
nodes) plus a matching gap-fix in `crates/crawler/src/lib.rs`'s response
parser (it only ever read a flat top-level `dataset`/`service`, so a
response shaped like the fixed endpoint's own new nested output would
have silently parsed as empty). `compliance/harvest-bench/check_catalog.py`
was updated to match (its Rust-side dataset-id extraction now recurses
into `catalog[]`, in addition to the pre-existing top-level `dataset`
array - the EDC-side extraction was re-read and needed no change, see
below). Every number in this report - RSS/CPU/throughput/latency and the
correctness/shape evidence - is from a **fresh, real re-run** with the
fix in place, not the original run's numbers with the table edited in
place. The original numbers are not reproduced here; see this file's own
git history for the pre-fix version if needed.

**Question:** unlike the two prior benchmark rounds
([`benchmark-2026-08-27.md`](benchmark-2026-08-27.md),
[`benchmark-dcp-2026-08-27.md`](benchmark-dcp-2026-08-27.md)), which both
measured a connector's catalog-serving endpoint in isolation against a
static seed, this round measures **harvesting**: the background crawl
loop actively re-crawling other participants, running concurrently with
real k6 load against the crawler's own aggregated-catalog-serving
endpoint - for both this project's own from-scratch Rust
crawler/store/serving stack and Eclipse EDC 0.18.0's own, real,
first-party federated-catalog crawler component.

**Answer: both work, both stayed correct under concurrent load, and the
resource-usage gap from the prior two rounds holds up again here** - Rust
used roughly **68x less peak RSS** and **about half the average CPU**
(~561% vs. ~1,150% of one core, i.e. ~5.6 vs. ~11.5 of the host's 22
cores) of EDC's own crawler, while serving **~86x higher throughput**,
under the same 20-VU/30s k6 load with the harvest loop actively
re-crawling in the background on both sides throughout. See "What this
doesn't prove" below for the real, substantial caveats on that comparison
- most importantly, EDC's crawler runtime here does meaningfully more
(a full DSP-2025/1 client stack, Management API v3, real JSON-LD
transformation of *incoming* crawled catalogs) than Rust's minimal
crawler + Oxigraph store + hand-serialized DSP endpoint.

## What was built

- `compliance/harvest-bench/edc-fedcat-runtime/` - a **new** real Eclipse
  EDC 0.18.0 runtime, built the same way as `compliance/crawler-edc-fixture/`
  (published Maven Central artifacts only, no vendored source touched),
  but running EDC's *own* federated-catalog crawler component instead of a
  participant control-plane: `org.eclipse.edc:federatedcatalog-base-bom:0.18.0`
  (confirmed on Maven Central before depending on it - a real, published
  aggregator bundling `catalog-crawler-core`, `federated-catalog-api`,
  `federated-catalog-spi`, the Management API stack, and the DSP 2025/1
  client stack) plus `org.eclipse.edc:iam-mock:0.18.0` (an `IdentityService`
  - needed both to satisfy a hard `@Inject` at boot and to mint the
  outbound token EDC's own DSP dispatcher attaches to crawl requests).
  This exact pairing was confirmed, not guessed, by reading
  eclipse-edc-connector's own end-to-end federated-catalog test
  (`system-tests/e2e-federatedcatalog-tests/end2end-test/.../FederatedCatalogTest.java`
  in the `dataspace` study repo's vendored connector), which wires the
  same two artifacts (`:dist:bom:federatedcatalog-base-bom` +
  `:extensions:common:iam:iam-mock`) for its own embedded catalog runtime
  - and its `CatalogApiClient`/`SeedNodeExtension` were the concrete
  reference for this round's own `HarvestSeedExtension` and the
  Management API request shape below.
  - `HarvestSeedExtension` (`src/main/java/harvest/HarvestSeedExtension.java`)
    - the only custom code needed - `@Inject`s `TargetNodeDirectory` and
    inserts two `TargetNode`s, env-var-driven (`HARVEST_TARGET_NODES`,
    format `id=name=url;...`). No custom DSP client, no custom query
    endpoint: `catalog-crawler-core`'s auto-discovered
    `CatalogCrawlerActionExtension`/`DspCatalogRequestAction` does the real
    crawling, and `federated-catalog-api`'s auto-discovered
    `CatalogsApiV3Controller` (`POST {management}/v3/catalogs/request`)
    serves the result - exactly the "try the real management-api module
    first" path this round's task brief asked for, and it worked on the
    **first successful boot**, no fallback to a custom query endpoint
    needed.
  - `run-fedcat-crawler.sh` - env-var-driven launch script, same pattern
    as `../crawler-edc-fixture/run-instance.sh`. Configures
    `edc.catalog.cache.execution.period.seconds=5` (short/observable, vs.
    the 60s default) via `EDC_CATALOG_CACHE_EXECUTION_PERIOD_SECONDS`.
- Two **new** real EDC 0.18.0 participant instances, HARVEST-D (3 datasets:
  `HARVEST-D-01..03`) and HARVEST-E (7 datasets: `HARVEST-E-01..07`),
  started via the **existing, unmodified**
  `compliance/crawler-edc-fixture/run-instance.sh` +
  `spike.CatalogFixtureExtension` mechanism proved out in
  [`crawler-edc-integration-test.md`](crawler-edc-integration-test.md) -
  additive, the original EDC-A/B/C instances were not touched.
- `compliance/harvest-bench/participants.toml` - a `crates/crawler` config
  pointing at the same two HARVEST-D/E instances, `interval_secs = 5` to
  match EDC's own crawl period, `requires_dcp = false` for both (same
  documented scope decision as every prior round -
  [`benchmark-dcp-2026-08-27.md`](benchmark-dcp-2026-08-27.md)).
- `compliance/harvest-bench/catalog-request.k6.js` - the same
  20-constant-VU/30s/same-thresholds k6 methodology as
  [`benchmark-2026-08-27.md`](benchmark-2026-08-27.md), generalized with a
  `BODY` env var so one script drives both targets' different request
  bodies (EDC: an empty `QuerySpec` JSON-LD object; Rust: unchanged from
  the original report's bare `CatalogRequestMessage`).
- `compliance/harvest-bench/sample-rss-cpu.sh` - the same 1s-interval
  `/proc/<pid>/status`+`/proc/<pid>/stat` RSS/CPU sampler methodology as
  both prior reports, PID always resolved via `ss -tlnp` (never `$!` after
  a wrapped background launch - the DCP round's own documented pitfall).
- `compliance/harvest-bench/check_catalog.py` - queries either system's
  own catalog-serving endpoint and asserts the result contains **exactly**
  the 10 expected dataset ids, used as the correctness check both before
  and immediately after each load-test window. Updated in this corrected
  re-run: `dataset_ids_from_catalog` now recurses into a `catalog[]`
  field, in addition to the pre-existing top-level `dataset` array, so it
  is robust to either shape - Rust's endpoint nests per-participant
  sub-catalogs there once 2+ origin nodes are cached (see the top note),
  while EDC's Management API response was re-read (not assumed) and
  never carries a `catalog[]` field on its own `Catalog` objects, so the
  recursion is a no-op for the EDC path.
- `compliance/harvest-bench/run-harvest-bench.sh` - the end-to-end driver
  that ran everything below, with trap-based cleanup.

## Getting EDC's real federated-catalog crawler working

Per the task's own explicit guidance, `federated-catalog-api` (the
Management API surface) was tried first, before any custom fallback
endpoint. It resolved and booted correctly on the **first successful
attempt** - no `CyclicDependencyException`, no missing `@Inject`, no
fallback needed. The only real friction, both resolved by reading source
rather than guessing:

1. **The Management API's `POST /v3/catalogs/request` needs a JSON-LD body
   with an absolute-IRI `@type`, not an empty object.** An empty `{}` body
   500s (arrives as a `null` `JsonObject` from Jersey in some cases) and a
   bare `{}` with content produced `Error expanding JSON-LD structure:
   result was empty` (JSON-LD expansion needs at least a recognizable
   `@type`/`@context`). Confirmed against a live instance:
   ```
   $ curl -s -w '\nHTTP_STATUS:%{http_code}\n' -X POST http://127.0.0.1:19411/api/management/v3/catalogs/request \
       -H "Content-Type: application/json" -d '{}'
   [{"message":"Failed to expand JsonObject: Error expanding JSON-LD structure: result was empty, it could be caused by missing '@context'","type":"InvalidRequest","path":null,"invalidValue":null}]
   HTTP_STATUS:400
   ```
   Fixed by reading `BaseCatalogsApiController.requestCatalogs` (`querySpecJson == null ? QuerySpec.none() : transform(...)`)
   and eclipse-edc-connector's own `TestFunctions.createEmptyQuery()`
   (`system-tests/e2e-federatedcatalog-tests/end2end-test/e2e-junit-runner/.../TestFunctions.java`):
   send `{"@type":"https://w3id.org/edc/v0.0.1/ns/QuerySpec"}` - an already
   fully-expanded IRI needs no `@context` to expand. Confirmed working:
   ```
   $ curl -s -w '\nHTTP_STATUS:%{http_code}\n' -X POST http://127.0.0.1:19411/api/management/v3/catalogs/request \
       -H "Content-Type: application/json" -d '{"@type":"https://w3id.org/edc/v0.0.1/ns/QuerySpec"}'
   []
   HTTP_STATUS:200
   ```
   (empty array here because no participant had been crawled yet at this
   point in the exploration - correct behavior, not an error).
2. **A `TargetNode`'s `url` is the participant's base DSP-2025/1 endpoint,
   not the full `.../catalog/request` path** `crates/crawler`'s own
   `participants.toml` uses. Confirmed by reading `FederatedCatalogTest`'s
   own node construction (`CONNECTOR_PROTOCOL.path() + "/" + V_2025_1_VERSION`,
   no `/catalog/request` suffix) - `DspCatalogRequestAction`/
   `ProtocolRemoteMessageDispatcher` resolve the message-type-specific
   path suffix themselves from that base. Not a bug, just a real,
   easy-to-miss difference between the two systems' own config shapes for
   "the same fact" (where a participant's catalog endpoint lives).
3. **No Management API auth was configured, deliberately, matching
   `FederatedCatalogTest`'s own choice** - that test's `CatalogApiClient`
   sends no `Authorization`/`x-api-key` header at all, and its runtime
   config never sets an API key. Reading `TokenBasedAuthenticationExtension`
   confirmed this is opt-in (`web.http.<context>.auth.key`), not a
   default-on gate - so this round's `run-fedcat-crawler.sh` also leaves
   it unset, and the k6 script sends no auth header to the EDC target
   either.
4. **No new port pitfall this round** - despite `federatedcatalog-base-bom`
   pulling in `transfer-data-plane-signaling`/`data-plane-signaling-client`
   (the same modules whose hardcoded `DEFAULT_SIGNALING_PORT=8182`
   surprised the prior crawler-fixture round), `WEB_HTTP_SIGNALING_PORT`
   was set defensively in `run-fedcat-crawler.sh` and, observed via
   `ss -tlnp`, **nothing ever bound to it** - this particular runtime
   composition doesn't stand up a signaling *server*, only client-side
   plumbing. Harmless either way, but worth recording: the port was
   reserved defensively and turned out not to be needed here.

No fallback to a custom query endpoint was needed - the real
`federated-catalog-api` module is what this report's numbers below were
measured against.

## Benchmark methodology

Both systems were run with their background harvest loop **actively
running throughout** (5s crawl period on both sides) while the same
20-constant-VU/30s k6 load hit each system's own catalog-serving endpoint
- never both systems loaded simultaneously (EDC's crawler was fully torn
down before the Rust side started), per this project's established
convention. RSS/CPU sampling (1s interval, `/proc`, PID from `ss -tlnp`)
ran for the full 35s window (before, during, and just after the 30s k6
run) so it captures the combined cost of harvesting + concurrent read
load together, not either in isolation. Full driver:
`compliance/harvest-bench/run-harvest-bench.sh`; full commands actually
run are reproduced by that script (see `compliance/harvest-bench/README.md`
to re-run it).

**EDC target:** `POST http://127.0.0.1:19411/api/management/v3/catalogs/request`,
body `{"@type":"https://w3id.org/edc/v0.0.1/ns/QuerySpec"}`, no auth
header.

**Rust target:** `POST http://127.0.0.1:19501/dsp/catalog/request`, body
unchanged from the original report's `CatalogRequestMessage`, no auth
header (`DspAuthMode::Disabled`, the default).

## Correctness under concurrent harvest + load

`check_catalog.py` queried each system's own endpoint and asserted the
result's dataset ids equal exactly
`{HARVEST-D-01, HARVEST-D-02, HARVEST-D-03, HARVEST-E-01..07}` (10 ids) -
once right after the first crawl cycle completed (before k6 started), and
once again immediately after the 30s k6 run finished (i.e. while the
background crawl loop had kept re-crawling and re-writing the store
throughout the load). Real output, both systems, both checkpoints:

```
$ python3 check_catalog.py edc  http://127.0.0.1:19411/api/management/v3/catalogs/request   # before load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
$ python3 check_catalog.py edc  http://127.0.0.1:19411/api/management/v3/catalogs/request   # after 22,577 requests of load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
$ python3 check_catalog.py rust http://127.0.0.1:19501/dsp/catalog/request                  # before load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
$ python3 check_catalog.py rust http://127.0.0.1:19501/dsp/catalog/request                  # after 1,932,236 requests of load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
```

Both systems stayed correct and stable across the whole window - neither a
concurrently-running crawler that keeps overwriting the store, nor
sustained concurrent reads, corrupted or dropped data on either side, for
this scenario (small, static seed data on the crawled participants - see
caveats below).

### Real response shape, both sides (evidence for the fix)

`check_catalog.py OK` only proves the recovered *id set* is right; it
doesn't by itself show the *shape* the fix claims. Both raw responses
below are real captures from this corrected re-run (Rust: a live `curl`
against the actual benchmarked `http-api` process, mid-run, while k6 load
was in flight; EDC: a live `curl` against a real EDC federated-catalog
crawler instance, brought up the same way as the benchmarked one, for
this specific evidence capture).

**Rust** (`POST /dsp/catalog/request`, 2 cached origin nodes) - top-level
`dataset`/`service` are empty, both participants' 10 datasets are nested
under `catalog[]`, one entry per origin node:

```json
{
  "@context": ["https://w3id.org/dspace/2025/1/context.jsonld"],
  "@id": "urn:uuid:344e561f-c145-47af-af5e-6f467feac2c6",
  "@type": "Catalog",
  "participantId": "urn:connector:federated-catalog-rs",
  "dataset": [],
  "service": [],
  "catalog": [
    {
      "@id": "urn:uuid:ef2cd79a-8055-44c6-a6db-56d65caae464",
      "@type": "Catalog",
      "participantId": "HARVEST-D",
      "dataset": [
        { "@id": "HARVEST-D-03", "@type": "Dataset", "...": "..." },
        { "@id": "HARVEST-D-01", "@type": "Dataset", "...": "..." },
        { "@id": "HARVEST-D-02", "@type": "Dataset", "...": "..." }
      ]
    },
    {
      "@id": "...",
      "@type": "Catalog",
      "participantId": "HARVEST-E",
      "dataset": [ "...7 HARVEST-E-* datasets..." ]
    }
  ]
}
```

**EDC** (`POST /api/management/v3/catalogs/request`) - a top-level JSON
array of 2 `Catalog` objects, each with its own flat `dataset` array and
`participantId` (this shape is unchanged by the fix - it's EDC's own,
pre-existing behavior, reproduced here for a real side-by-side, not
reused from the original run):

```json
[
  {
    "@id": "cf5b6b6b-a970-4ae8-97d1-be30fead18a9",
    "@type": "Catalog",
    "participantId": "HARVEST-D",
    "dataset": [
      { "@id": "HARVEST-D-03", "@type": "Dataset", "...": "..." },
      { "@id": "HARVEST-D-01", "@type": "Dataset", "...": "..." },
      { "@id": "HARVEST-D-02", "@type": "Dataset", "...": "..." }
    ],
    "service": [{ "@type": "DataService", "endpointURL": "http://localhost:19221/api/dsp/2025-1" }]
  },
  {
    "@id": "6e393b64-2bf7-44a5-bd30-c64ac1c61049",
    "@type": "Catalog",
    "participantId": "HARVEST-E",
    "dataset": [ "...7 HARVEST-E-* datasets..." ],
    "service": [{ "@type": "DataService", "endpointURL": "http://localhost:19321/api/dsp/2025-1" }]
  }
]
```

Both are genuinely one-entry-per-crawled-participant now: EDC nests via a
top-level array of `Catalog`, Rust nests via one `Catalog`'s own
`catalog[]` field - a different DSP-legal encoding of the same
"federation of catalogs, not one merged catalog" structure, which is
exactly the asymmetry the fix closes (see the top note and the summary
table's "Aggregated dataset count served" row below).

**One more genuine, previously-unobserved fidelity difference, visible
only now that both responses are captured side by side for the same
scenario:** EDC's Management API response is a **bare JSON array** at the
document root, with no `@context`/`@id`/`@type` wrapper around the two
`Catalog` entries at all - not itself a single JSON-LD document. Rust's
response, even in the federated/nested case, stays **one self-consistent
JSON-LD document throughout** (`@context` at the root, `@type: "Catalog"`,
the two participants nested under `catalog[]` rather than hoisted to the
response root). This tracks a real, structural difference between the two
APIs being compared, not just an implementation quirk: EDC's Management
API is that operator's own internal REST surface (JSON-LD is used
per-object, not for API envelope shape), while `http-api`'s `/dsp/...`
endpoints are DSP-protocol-facing throughout, so every response - single-
participant or federated - is DSP/JSON-LD-framed the same way. Neither is
"wrong"; it's a real consequence of comparing a Management API against a
DSP-protocol endpoint for the same underlying question ("give me the
federated catalog"), which is itself one of this comparison's limits (see
"What this doesn't prove" below).

## Summary table

| Metric | Rust (`crates/crawler` + `http-api`) | EDC 0.18.0 federated-catalog crawler (Java) |
|---|---:|---:|
| RSS at sampling start (pre-load, harvest loop already warm) | 13.45 MB (13,772 KB) | 304.4 MB (311,676 KB) |
| Peak RSS (harvest + load combined) | 18.36 MB (18,804 KB) | 1,258.2 MB (1,288,420 KB) |
| Avg CPU during the 30s load window | ~561% (~5.6 cores of 22) | ~1,150% (~11.5 cores of 22) |
| Throughput | 64,407.27 req/s | 752.08 req/s |
| Latency avg | 251.47 µs | 26.48 ms |
| Latency p50 (median) | 225.6 µs | 26.49 ms |
| Latency p90 | 390.24 µs | 37.87 ms |
| Latency p95 | 464.15 µs | 41.23 ms |
| Latency p99 | 719.15 µs | 49.44 ms |
| Latency max | 10.35 ms | 125.88 ms |
| Error rate | 0.00% (1,932,236/1,932,236 OK) | 0.00% (22,577/22,577 OK) |
| Aggregated dataset count served | 10 (across 2 nested `Catalog` entries under `catalog[]`, one per crawled participant) | 10 (across 2 `Catalog` entries, one per crawled participant) |
| Correctness under concurrent harvest+load | OK (both checkpoints) | OK (both checkpoints) |

Environment: 22 logical cores (`nproc`), `CLK_TCK=100` - same host as both
prior reports.

**This round's data volumes are symmetric for the first time** - unlike
the original report's 17-vs-1-dataset mismatch, both systems here
aggregate the same real 10 datasets from the same two real EDC
participants. As of this corrected re-run, the **structural shape** is
now symmetric too, not just the count: both sides return one `Catalog`
per crawled participant rather than Rust merging everything into a
single flat list (see "Real response shape, both sides" above). This
makes the throughput/latency comparison meaningfully tighter than the
prior two rounds, though still not a full apples-to-apples implementation
comparison - see below.

## What this doesn't prove

- **EDC's crawler runtime does more per request than this comparison
  charges it for.** Every crawl cycle on EDC's side does real JSON-LD
  *expansion* of each crawled participant's incoming DSP response (Titanium)
  in addition to the *transformation* work both systems already do when
  serving their own catalog - Rust's crawler only deserializes plain JSON
  into its own domain type, no JSON-LD engine involved on the ingest side
  at all. Some of EDC's higher CPU/RSS reflects doing strictly more
  standards-faithful work on the harvest side, not just "JVM vs. native"
  or "same work, slower".
- **Not the same serialization pipeline on the serving side either.**
  EDC's Management API response goes through the same real JSON-LD
  framing/policy-engine pipeline documented in
  [`benchmark-2026-08-27.md`](benchmark-2026-08-27.md)'s fidelity
  section (structured negotiation-ready offer ids, `endpointDescription`,
  dual `@id`/`id`, etc.) - Rust's is still a hand-written, minimal
  serializer over data now sourced from a real Oxigraph store instead of
  a hardcoded seed. That fidelity gap, not just the throughput number, is
  the more informative comparison; it's unchanged from the original
  report and not re-litigated in full here.
- **Two participants with a handful of static datasets each, not a
  churning catalog.** Both HARVEST-D/E seed once at boot and never change
  their assets afterward - "does a concurrent writer corrupt a concurrent
  reader" was exercised (the crawler rewriting the aggregate store every
  5s while k6 read it), but "does the aggregate store correctly converge
  when the *underlying* data is *also* changing mid-benchmark" was not.
- **`rust-http-api.log` came back empty again in this corrected re-run**
  despite the Rust side demonstrably working correctly (correctness
  checks + 1.93M successful k6 requests are the real evidence) -
  `tracing_subscriber`'s buffered writer never flushed before the process
  was `kill`ed (SIGTERM, not a graceful shutdown path), so no textual
  startup/crawl log survives from this run on the Rust side, same as the
  original run. Noted honestly rather than fabricating log content; it
  does not affect any number in the table above, all of which come from
  k6's own output and the `/proc`-based sampler.
- **Single run, no repetition, coarse 1s RSS/CPU sampling, short windows**
  - same caveats as both prior reports, not repeated in full here.
  **JVM warmup/JIT** is a real confound for the EDC crawler figures here
  too, for the same reasons as the original report.
- **Only two participants, both reachable and healthy for the whole run.**
  Crawler resilience under a genuinely unreachable/flaky participant
  (timeouts, partial cycles, retry behavior under concurrent load) was
  not exercised in this round - `crates/crawler`'s own unit/integration
  tests already cover that path in isolation
  ([`benchmark-2026-08-27.md`](benchmark-2026-08-27.md) and
  [`crawler-edc-integration-test.md`](crawler-edc-integration-test.md)),
  just not combined with concurrent read load here.
- **No real DCP on either side**, same documented scope decision as every
  prior round.

## Cleanup

`run-harvest-bench.sh` itself runs a full port sweep and PID-based kill
(with a `kill -9` fallback) in an `EXIT` trap. Real output from this
corrected re-run:

```
[run-harvest-bench] === SUMMARY ===
[run-harvest-bench] EDC   correctness: early=OK  late=OK
[run-harvest-bench] Rust  correctness: early=OK  late=OK
[run-harvest-bench] cleanup: killing any tracked PIDs and sweeping ports 19201|19211|19221|19231|19241|19251|19261|19301|19311|19321|19331|19341|19351|19361|19401|19411|19421|19431|19451|19461|19501
[run-harvest-bench] post-cleanup port sweep:
[run-harvest-bench]   (clean - no listeners in this benchmark's port range)
```

Re-verified independently afterward (not just trusting the trap), same
pattern as every prior round:

```
$ ss -tlnp | grep -E "19201|19211|19221|19231|19241|19251|19261|19301|19311|19321|19331|19341|19351|19361|19401|19411|19421|19431|19451|19461|19501"
(no output - clean)
$ pgrep -x java
(no output - clean)
$ docker ps -a | grep -iE "harvest|edc|fedcat"
(no output - clean)
```

One real, honestly-reported slip during cleanup verification, same as the
original run: a Gradle daemon (from this session's own
`./gradlew printClasspath` pre-flight invocations) was still running
after the driver script's own trap-based cleanup finished (expected - the
driver never manages Gradle daemons, only the long-running `java`/`k6`
processes it starts). Caught by a `pgrep -fa java` sweep and stopped:

```
$ (cd compliance/crawler-edc-fixture && ./gradlew --stop)
Stopping Daemon(s)
1 Daemon stopped
$ (cd compliance/harvest-bench/edc-fedcat-runtime && ./gradlew --stop)
No Gradle daemons are running.
```

Separately, this report's "Real response shape, both sides" evidence
section above required one extra short-lived, ad hoc verification run
(HARVEST-D/E + the EDC crawler brought back up just long enough for one
`curl` against the real Management API endpoint, no k6 load involved -
the numbers in the summary table above are unaffected, they come only
from the main `run-harvest-bench.sh` run) - that ad hoc script had its
own trap-based cleanup and was independently re-verified clean afterward
the same way (`ss -tlnp`, `pgrep -x java`, `docker ps -a`, all empty), and
the throwaway script itself was deleted rather than left in the repo.

## Files

- `compliance/harvest-bench/edc-fedcat-runtime/` - the new EDC
  federated-catalog crawler Gradle project (source committed;
  `build/`, `.gradle/`, `classpath.txt`, `logs/` gitignored).
- `compliance/harvest-bench/participants.toml` - the `crates/crawler`
  config used.
- `compliance/harvest-bench/catalog-request.k6.js` - the load-test script.
- `compliance/harvest-bench/sample-rss-cpu.sh` - the RSS/CPU sampler.
- `compliance/harvest-bench/check_catalog.py` - the correctness check
  (updated in this corrected re-run to recurse into a nested `catalog[]`
  field on the Rust side - see the top note).
- `compliance/harvest-bench/run-harvest-bench.sh` - the end-to-end driver
  (results saved under a gitignored `results/`, regenerated by every run).
- `compliance/harvest-bench/README.md` - how to re-run all of the above.
