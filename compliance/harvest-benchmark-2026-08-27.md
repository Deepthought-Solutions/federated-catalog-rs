# Harvesting benchmark: EDC's own federated-catalog crawler vs. `crates/crawler` + `http-api`

**Date:** 2026-08-27

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
used roughly **70x less peak RSS** and **about half the average CPU**
(~566% vs. ~1,158% of one core, i.e. ~5.7 vs. ~11.6 of the host's 22
cores) of EDC's own crawler, while serving **~89x higher throughput**,
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
  and immediately after each load-test window.
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
$ python3 check_catalog.py edc  http://127.0.0.1:19411/api/management/v3/catalogs/request   # after 23,542 requests of load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
$ python3 check_catalog.py rust http://127.0.0.1:19501/dsp/catalog/request                  # before load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
$ python3 check_catalog.py rust http://127.0.0.1:19501/dsp/catalog/request                  # after 2,100,017 requests of load
OK ['HARVEST-D-01', 'HARVEST-D-02', 'HARVEST-D-03', 'HARVEST-E-01', 'HARVEST-E-02', 'HARVEST-E-03', 'HARVEST-E-04', 'HARVEST-E-05', 'HARVEST-E-06', 'HARVEST-E-07']
```

Both systems stayed correct and stable across the whole window - neither a
concurrently-running crawler that keeps overwriting the store, nor
sustained concurrent reads, corrupted or dropped data on either side, for
this scenario (small, static seed data on the crawled participants - see
caveats below).

## Summary table

| Metric | Rust (`crates/crawler` + `http-api`) | EDC 0.18.0 federated-catalog crawler (Java) |
|---|---:|---:|
| RSS at sampling start (pre-load, harvest loop already warm) | 13.2 MB (13,516 KB) | 304.6 MB (311,860 KB) |
| Peak RSS (harvest + load combined) | 18.73 MB (19,176 KB) | 1,302.1 MB (1,333,360 KB) |
| Avg CPU during the 30s load window | ~566% (~5.7 cores of 22) | ~1,158% (~11.6 cores of 22) |
| Throughput | 69,999.4 req/s | 784.2 req/s |
| Latency avg | 229.14 µs | 25.40 ms |
| Latency p50 (median) | 205.37 µs | 25.33 ms |
| Latency p90 | 352.17 µs | 35.46 ms |
| Latency p95 | 416.6 µs | 38.92 ms |
| Latency p99 | 650.47 µs | 48.17 ms |
| Latency max | 13.37 ms | 129.32 ms |
| Error rate | 0.00% (2,100,017/2,100,017 OK) | 0.00% (23,542/23,542 OK) |
| Aggregated dataset count served | 10 (flattened into 1 `Catalog`) | 10 (across 2 `Catalog` entries, one per crawled participant) |
| Correctness under concurrent harvest+load | OK (both checkpoints) | OK (both checkpoints) |

Environment: 22 logical cores (`nproc`), `CLK_TCK=100` - same host as both
prior reports.

**This round's data volumes are symmetric for the first time** - unlike
the original report's 17-vs-1-dataset mismatch, both systems here
aggregate the same real 10 datasets from the same two real EDC
participants. This makes the throughput/latency comparison meaningfully
tighter than the prior two rounds, though still not a full
apples-to-apples implementation comparison - see below.

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
- **`rust-http-api.log` came back empty** despite the Rust side
  demonstrably working correctly (correctness checks + 2.1M successful k6
  requests are the real evidence) - `tracing_subscriber`'s buffered writer
  never flushed before the process was `kill`ed (SIGTERM, not a graceful
  shutdown path), so no textual startup/crawl log survives from this run
  on the Rust side. Noted honestly rather than fabricating log content;
  it does not affect any number in the table above, all of which come
  from k6's own output and the `/proc`-based sampler.
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

```
$ ps -ef | grep "BaseRuntime" | grep -v grep | grep "java "
(no output - clean)
$ pgrep -x java
(no output - clean, after also stopping a leftover Gradle daemon:)
$ (cd compliance/crawler-edc-fixture && ./gradlew --stop)
Stopping Daemon(s)
1 Daemon stopped
$ (cd compliance/harvest-bench/edc-fedcat-runtime && ./gradlew --stop)
No Gradle daemons are running.
$ ss -tlnp | grep -E "19201|19211|19221|19231|19241|19251|19261|19301|19311|19321|19331|19341|19351|19361|19401|19411|19421|19431|19451|19461|19501"
(no output - clean)
```

`run-harvest-bench.sh` itself also runs a full port sweep and PID-based
kill (with a `kill -9` fallback) in an `EXIT` trap, verified in the actual
run this report's numbers came from:

```
[run-harvest-bench] cleanup: killing any tracked PIDs and sweeping ports 19201|19211|...|19501
[run-harvest-bench] post-cleanup port sweep:
[run-harvest-bench]   (clean - no listeners in this benchmark's port range)
```

One real, honestly-reported slip during cleanup verification: a Gradle
daemon from this session's own `./gradlew printClasspath` invocations was
still running after the driver script's own trap-based cleanup finished
(expected - the driver never manages Gradle daemons, only the long-running
`java`/`k6` processes it starts). Caught by a `pgrep -x java` sweep (not
just the ports the driver script itself tracks) and stopped via
`./gradlew --stop` in both Gradle projects, then re-verified clean.

## Files

- `compliance/harvest-bench/edc-fedcat-runtime/` - the new EDC
  federated-catalog crawler Gradle project (source committed;
  `build/`, `.gradle/`, `classpath.txt`, `logs/` gitignored).
- `compliance/harvest-bench/participants.toml` - the `crates/crawler`
  config used.
- `compliance/harvest-bench/catalog-request.k6.js` - the load-test script.
- `compliance/harvest-bench/sample-rss-cpu.sh` - the RSS/CPU sampler.
- `compliance/harvest-bench/check_catalog.py` - the correctness check.
- `compliance/harvest-bench/run-harvest-bench.sh` - the end-to-end driver
  (results saved under a gitignored `results/`, regenerated by every run).
- `compliance/harvest-bench/README.md` - how to re-run all of the above.
