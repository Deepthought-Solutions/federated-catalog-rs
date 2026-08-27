# DSP catalog-request benchmark: `federated-catalog-rs` vs. Eclipse EDC 0.18.0

**Date:** 2026-08-27
**Endpoint under test:** `POST {baseUrl}/catalog/request` (DSP v2025-1 catalog protocol),
comparing:

- **Rust** — this repo's `http-api` crate, `POST /dsp/catalog/request`, serving one
  seeded dataset (`sample-dataset`) from an in-memory `CatalogCache`.
- **Java (EDC)** — `vendor/eclipse-edc-connector` (v0.18.0) from the
  `dataspace` study repo, run via its purpose-built
  `system-tests/tck/dsp-tck-connector-under-test` runtime (bundles
  `tck-extension`'s `TckSetupExtension`), `POST /api/dsp/2025-1/catalog/request`,
  serving 17 seeded assets (including `CAT0101`/`CAT0102`) under one contract
  definition (`CD123`)/policy (`P123`, permission action `odrl:use`).

Both were built and run locally in the same sandbox, one at a time under load
(never both loaded simultaneously — see step 4 of the task brief this
benchmark follows).

## Summary table

| Metric | Rust (`http-api`) | EDC 0.18.0 (Java) |
|---|---:|---:|
| Idle RSS (post-warmup) | 10.3 MB (10,536 KB) | 347.5 MB (355,812 KB) |
| Peak RSS under load | 13.3 MB (13,640 KB) | 2,504.99 MB (2,564,112 KB) |
| Avg CPU during load | ~291% of 1 core (~2.9 cores) | ~1,475% of 1 core (~14.75 cores) |
| Throughput | 92,714.8 req/s | 708.9 req/s |
| Latency avg | 163.1 µs | 28.08 ms |
| Latency p50 (median) | 136.8 µs | 26.97 ms |
| Latency p95 | 341.8 µs | 42.05 ms |
| Latency p99 | 562.0 µs | 54.02 ms |
| Latency max | 14.17 ms | 408.87 ms |
| Error rate | 0.00% (2,781,479/2,781,479 OK) | 0.00% (21,275/21,275 OK) |
| Seeded catalog size | 1 dataset | 17 datasets (incl. `CAT0101`, `CAT0102`) |

Environment: 22 logical cores (`nproc`), `CLK_TCK=100`. Both processes ran on
the same otherwise-idle host, one at a time under load.

**Caveat on the throughput/latency gap**: this is not solely (or even mostly)
an implementation-quality comparison. See "What this doesn't prove" below —
the two servers are doing genuinely different amounts of work per request
(1 dataset with a synthesized placeholder policy vs. 17 datasets each with a
real per-asset contract-offer id computed through EDC's policy/contract
engine, JSON-LD framing via Titanium, Jersey/Jetty request dispatch, and a
full ODRL policy evaluation pass), and Rust is a native, minimal Axum service
against a JVM connector with 88 service extensions booted.

## How the servers were run

**Rust:**
```bash
cd federated-catalog-rs
cargo build --release -p http-api
HTTP_API_ADDR=0.0.0.0:18080 ./target/release/http-api &
```

**EDC:** Following the task's config from `tck-runtime.env`, but with two
corrections discovered while getting the runtime to actually serve a
catalog (see "Problems encountered" below):

```bash
cd vendor/eclipse-edc-connector
export WEB_HTTP_PORT=18081 \       # not 8080 — see below
       WEB_HTTP_PATH="/api" \
       WEB_HTTP_MANAGEMENT_PORT=8081 \
       WEB_HTTP_MANAGEMENT_PATH="/api/management/" \
       WEB_HTTP_PROTOCOL_PORT=8082 \
       WEB_HTTP_PROTOCOL_PATH="/api/dsp" \
       WEB_HTTP_CONTROL_PORT=8183 \
       WEB_HTTP_CONTROL_PATH="/api/control" \
       WEB_HTTP_CATALOG_PORT=8184 \
       WEB_HTTP_CATALOG_PATH="/api/catalog" \
       WEB_HTTP_VERSION_PORT=8185 \
       WEB_HTTP_VERSION_PATH="/api/version" \
       EDC_API_AUTH_KEY="password" \
       EDC_IAM_DID_WEB_USE_HTTPS="false" \
       EDC_DSP_CALLBACK_ADDRESS="http://localhost:8082/api/dsp" \
       EDC_PARTICIPANT_ID="CONNECTOR_UNDER_TEST" \
       EDC_PARTICIPANT_CONTEXT_ID="CONNECTOR_UNDER_TEST" \   # not in tck-runtime.env — see below
       EDC_MANAGEMENT_CONTEXT_ENABLED=true
./gradlew :system-tests:tck:dsp-tck-connector-under-test:run &
```

The actual catalog-request calls (once running) need the versioned DSP path
and a dummy bearer token (see "Problems encountered"):

```bash
curl -X POST http://127.0.0.1:8082/api/dsp/2025-1/catalog/request \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer 1234" \
  -d '{"@context": ["https://w3id.org/dspace/2025/1/context.jsonld"], "@type": "CatalogRequestMessage"}'
```

### Problems encountered getting a live EDC instance (both real, both fixed)

1. **Port collision on `WEB_HTTP_PORT=8080`.** This sandbox already has an
   unrelated `pa-server` process bound to `127.0.0.1:8080` (same conflict the
   task brief already flagged for the Rust side's default port). EDC's
   control-plane base API also defaults to 8080 per `tck-runtime.env`, so the
   first run's Jetty server threw `BindException` while trying to open *all*
   configured connectors together — and critically, the JVM process did not
   exit cleanly afterward; it hung with **zero** ports actually listening
   (confirmed via `ss -tlnp` and repeated `curl` failures) until manually
   killed. Fix: moved `WEB_HTTP_PORT` to 18081, leaving 8081/8082 (the ports
   that actually matter for this benchmark) as specified.

2. **Empty catalog despite confirmed seeding (participant-context-id
   mismatch).** After fixing the port and getting the runtime to boot
   cleanly (`88 service extensions started`), `POST /catalog/request`
   returned `200 OK` with **no `dataset` array at all** — not even an empty
   one. Root cause, found by reading source directly:
   `TckSetupExtension` (`system-tests/tck/tck-extension/.../setup/TckSetupExtension.java:40`)
   and `ClassicParticipantContextDefaultServicesExtension`
   (`core/common/participant-context-connector-classic-core/.../ClassicParticipantContextDefaultServicesExtension.java:41-42`)
   both read the **same** config key, `edc.participant.context.id`
   (env `EDC_PARTICIPANT_CONTEXT_ID`), but declare **different fallback
   defaults** when it's unset: the TCK seed extension defaults to the literal
   string `"participantContextId"`, while the connector's single-participant
   runtime falls back to `edc.participant.id` (`EDC_PARTICIPANT_ID`, which
   `tck-runtime.env` sets to `CONNECTOR_UNDER_TEST`). Since
   `tck-runtime.env` — the config file the task pointed at — never sets
   `EDC_PARTICIPANT_CONTEXT_ID`, running it as documented seeds all 17 assets
   under context id `"participantContextId"` while the runtime's actual
   (and only) participant context is `"CONNECTOR_UNDER_TEST"` — so the
   catalog resolver finds nothing. The runtime even logs the tell:
   `WARNING: The runtime is not configured with a participant context id.
   Using the participant id as the context id.` Fix: explicitly export
   `EDC_PARTICIPANT_CONTEXT_ID=CONNECTOR_UNDER_TEST` so both extensions agree.
   **This looks like a genuine gap in `tck-runtime.env` as shipped** — it's
   worth a note/PR upstream or at minimum flagging in this repo's own EDC
   vendoring notes, since anyone following that env file verbatim gets a
   silently empty catalog with no error.
3. **The DSP endpoint requires a bearer token, even under `NoopIdentityService`.**
   `POST /api/dsp/catalog/request` (unversioned) 404s; the real path is
   version-qualified: `/api/dsp/2025-1/catalog/request`. Calling it without
   an `Authorization` header returns `401 Unauthorized`. The TCK harness
   installs `NoopIdentityService`
   (`system-tests/tck/tck-extension/.../identity/NoopIdentityService.java`),
   which accepts **any** bearer token and always claims `client_id:
   TCK_PARTICIPANT` — so `Authorization: Bearer 1234` (or literally anything)
   is sufficient; this isn't real authentication, just a presence check.
4. **Cosmetic bug in `tck-runtime.env` itself**: line 17,
   `EDC_PARTICIPANT_ID=CONNECTOR_UNDER_TEST"`, has a stray trailing double
   quote. Harmless if you hand-export the value without it (as done here),
   but a literal `source`/`export $(cat ...)` of that file would bake a
   trailing `"` into the participant id.

None of the above required modifying `federated-catalog-rs` or EDC source —
only correct configuration of the already-vendored EDC runtime.

## Fidelity comparison

### Both pass basic DSP catalog shape

Checked against `eclipse-dataspacetck/dsp-tck`'s own JSON schemas
(`dsp-catalog/src/main/resources/catalog/{catalog,dataset}-schema.json` and
`negotiation/contract-schema.json`, fetched directly from GitHub for this
comparison). Both responses satisfy:

- `@context` present (`RootCatalog` requirement).
- `@type: "Catalog"`, `@id` present.
- `participantId` present, a string.
- `dataset` array (`minItems: 1` when present — Rust's and EDC's both are).
- Each dataset: `@id`, `@type: "Dataset"`, non-empty `hasPolicy` array of
  `Offer`-typed policies, non-empty `distribution` array.
- Each `Offer`: `@type: "Offer"`, at least `permission` present, and
  (correctly) **no** `target` key — the schema explicitly forbids `target`
  on a bare `Offer` (that's an `Agreement`-only field) and neither
  implementation adds one.
- Each `Distribution`: `format` and `accessService` present (schema allows
  `accessService` as either a plain string id or a nested `DataService`
  object — see below, the two implementations pick different variants).
- `service` array of `DataService` objects, each with `@type: "DataService"`
  and `endpointURL`.

Raw responses, for reference:

**Rust** (`POST /dsp/catalog/request`, no auth needed):
```json
{"@context":["https://w3id.org/dspace/2025/1/context.jsonld"],"@id":"urn:uuid:f28b3b0d-877e-4056-9c28-57f65d298497","@type":"Catalog","participantId":"urn:connector:federated-catalog-rs","dataset":[{"@id":"sample-dataset","@type":"Dataset","hasPolicy":[{"@id":"urn:uuid:34f7de8c-99ad-4d01-aeb1-0e36bc7fc991","@type":"Offer","permission":[{"@type":"Permission","action":"http://www.w3.org/ns/odrl/2/use"}]}],"distribution":[{"@type":"Distribution","format":"application/json","accessService":"sample-data-service"}]}],"service":[{"@id":"sample-data-service","@type":"DataService","endpointURL":"https://sample.example.org/dsp"}]}
```

**EDC** (`POST /api/dsp/2025-1/catalog/request`, `Authorization: Bearer 1234`)
— one dataset from the 17-dataset response, trimmed:
```json
{
  "@id": "6d3d7a0c-bf06-4160-839f-853350446179",
  "@type": "Catalog",
  "dataset": [
    {
      "@id": "CAT0101",
      "@type": "Dataset",
      "hasPolicy": [
        {
          "@id": "Q0QxMjM=:Q0FUMDEwMQ==:YmUyYzRkZDctZjlkNC00YzA4LWJlYzUtNWFkN2E0NjJiZGJk",
          "@type": "Offer",
          "permission": [{ "action": "use" }]
        }
      ],
      "distribution": [
        {
          "@type": "Distribution",
          "format": "HttpData-PULL",
          "accessService": {
            "@id": "5ee7e0fa-a2bc-4251-bab3-d3a1454dcfc8",
            "@type": "DataService",
            "endpointDescription": "dspace:connector",
            "endpointURL": "http://localhost:8082/api/dsp/2025-1"
          }
        }
      ],
      "id": "CAT0101"
    }
    /* ... 16 more datasets ... */
  ],
  "service": [{
    "@id": "7fc04829-496f-4b5b-8866-2aa7978cdf83",
    "@type": "DataService",
    "endpointDescription": "dspace:connector",
    "endpointURL": "http://localhost:8082/api/dsp/2025-1"
  }],
  "participantId": "CONNECTOR_UNDER_TEST",
  "@context": [
    "https://w3id.org/dspace/2025/1/context.jsonld",
    "https://w3id.org/edc/dspace/v0.0.1"
  ]
}
```

### Concrete, specific differences

1. **`@context` scope.** Rust emits exactly the one standard DSP 2025-1
   context URL. EDC emits *two* — the standard DSP context **plus** its own
   `https://w3id.org/edc/dspace/v0.0.1` extension context. That second
   context is what licenses the extra, non-DSP-standard terms below (`id`,
   `endpointDescription`) — a real consumer resolving only the DSP context
   would see those as un-namespaced/undefined terms. This is a legitimate
   EDC-specific extension pattern, not a spec violation, but it means EDC's
   wire format is DSP-plus-EDC-vocabulary, not pure DSP.

2. **Duplicate id field on every EDC dataset.** Each EDC dataset has *both*
   `"@id": "CAT0101"` (JSON-LD) *and* a plain `"id": "CAT0101"` key with the
   identical value — evidently an artifact of Jackson POJO serialization
   (`getId()`) running alongside JSON-LD framing/`@context` term-mapping,
   both firing on the same object. Harmless (extra properties are legal
   JSON-LD), but a real, observed quirk in EDC's own reference output that
   a byte-for-byte "does this look like EDC's wire format" check would need
   to reproduce, and Rust's output does not have.

3. **`accessService` representation.** DSP's `dataset-schema.json` allows
   `accessService` to be *either* a plain string (a reference to a
   `DataService.@id` elsewhere in the document) or a full nested
   `DataService` object. Rust always emits the compact string form
   (`"accessService": "sample-data-service"`, matching the `service` array
   entry's `@id`). EDC always emits the full nested object, duplicated once
   per distribution (each of the 17 datasets repeats a materially identical
   `DataService` object, just with a fresh random `@id` each time, rather
   than referencing the single `service` array entry by id). Both are
   individually schema-valid; EDC's is more verbose and — notably — doesn't
   actually reuse its own top-level `service` array entry per dataset, so a
   client can't assume "one canonical `DataService` per catalog" holds even
   within EDC's own responses.

4. **Contract-offer `@id` semantics.** Rust's offer id is a fresh random
   `urn:uuid:...` with no relationship to anything else in the system —
   it's a placeholder (see `crates/http-api/src/lib.rs`'s
   `placeholder_offer()`, which is explicitly documented as synthesizing an
   identical default-permission offer for every dataset regardless of any
   real policy). EDC's offer id is a structured, three-part
   base64-segment-joined string —
   decoding `Q0QxMjM=:Q0FUMDEwMQ==:YmUy...` gives
   `CD123:CAT0101:be2c4dd7-...` — i.e. it encodes the originating
   *contract definition id*, the *asset id*, and a fresh random component,
   so a subsequent contract-negotiation request referencing this offer id
   can be resolved back to a specific (contract-definition, asset) pairing.
   This is a real, structural fidelity gap: EDC's catalog offers are
   negotiation-ready identifiers; Rust's are inert.

5. **Policy compaction: `action` value.** Rust emits the full IRI
   (`"action": "http://www.w3.org/ns/odrl/2/use"`) as a literal string.
   EDC emits the JSON-LD-compacted short form (`"action": "use"`), which
   only round-trips to the same IRI because the ODRL vocabulary and its
   prefix are defined by the (unfetched, but implied) real DSP/ODRL context
   documents. Per the TCK's own schema, `Action` is typed as a bare
   `string` with no format constraint, so *both* pass validation — but they
   are not byte-identical, and a consumer that does naive string matching
   against `"use"` (rather than IRI-expanding) would fail against Rust's
   output, while one that expects a full IRI would fail against EDC's.
   Neither is "more correct" per the schema; they represent different
   (both legal) JSON-LD compaction choices, and it means "does the Rust
   response structurally match the wire format" is true at the schema
   level but not at the byte level.

6. **`permission[].@type`.** Rust adds `"@type": "Permission"` to every
   permission entry. EDC's output has **no** `@type` on `permission`
   entries at all. The TCK's `Rule` schema doesn't require `@type` either
   way, so this is a harmless but real point of non-parity — Rust adds a
   field EDC's own reference output doesn't produce.

7. **`endpointDescription` on `DataService`.** EDC populates
   `"endpointDescription": "dspace:connector"` on every `DataService`
   (both the catalog-level `service` array and each distribution's nested
   `accessService`). Rust's `DspDataService` struct
   (`crates/http-api/src/lib.rs`) has no `endpointDescription` field at
   all — even though `catalog-core`'s underlying `DataService` type *does*
   model an `endpoint_description` (seeded as
   `Some("dataspace-protocol-http:1.0")` in `seed_sample_catalog`), the DSP
   serialization path silently drops it. This is a genuine, fixable gap:
   the data exists in Rust's domain model but isn't wired into the wire
   format.

8. **Request-body filtering.** The DSP `CatalogRequestMessage` schema
   allows an optional `filter` array. Rust's `catalog_request` handler
   explicitly ignores the request body entirely (documented in its own doc
   comment — "intentionally ignored for now: this always returns the full
   flattened catalog"). This benchmark did not test EDC's `filter` handling
   either (both requests sent the same bare, filter-less body), so this
   remains an *observed capability gap* (Rust structurally cannot filter;
   whether EDC's does anything with `filter` on this exact endpoint was not
   verified here) rather than a demonstrated behavioral difference.

9. **Multiple offers per dataset / richer constraint shapes: not actually
   demonstrated by this seed data.** The task brief anticipated EDC might
   show "richer policy/constraint shapes... multiple contract offers per
   dataset" — in this specific TCK seed, it does not: every asset is
   governed by exactly one contract definition (`CD123`) and one trivial
   policy (`P123`, a bare `permission` with action `use`, no
   `constraint`/`prohibition`/`obligation`). So on the *content* actually
   observed, EDC's policy shape is exactly as flat as Rust's placeholder.
   The real difference is *capability*, not demonstrated output: EDC's
   policy model (`policy-model` SPI, ODRL `Constraint`/`LogicalConstraint`/
   `Duty` types) can represent constraints, prohibitions, obligations, and
   multiple offers per contract definition — Rust's `placeholder_offer()`
   is a hardcoded function that cannot represent any of that regardless of
   the underlying data, because `catalog-core`'s `Dataset` has no ODRL
   policy model at all yet (per that crate's own doc comments). A seed with
   a more elaborate policy would have shown this; this one didn't.

10. **Nested sub-catalogs (`catalog` array).** Neither implementation
    populates `Catalog`'s optional `catalog` field (federated/nested
    catalog listings) — parity here, both skip it, no fidelity gap
    observed.

### Bottom line on fidelity

At the schema level (does it validate against `dsp-tck`'s own JSON
Schemas), both pass on every structural point checked. At the byte/wire
level, they diverge in ways that all trace back to the same root cause:
Rust's implementation is a hand-written, minimal serializer with a single
hardcoded placeholder policy, while EDC's is a real JSON-LD framing +
policy-engine + multi-context pipeline. None of the observed differences
would fail `dsp-tck`'s `CAT` group (which is schema/behavior-driven, not
byte-diff-driven) — but a client written against "whatever EDC happens to
emit" rather than "the DSP spec" could plausibly break against Rust's
output (the reverse is less likely, since Rust's output is simpler and a
strict subset of what a schema-driven client would expect).

## Load test methodology

k6 script (`catalog-request.k6.js`, saved alongside this report's working
files — reproduced here in full):

```javascript
import http from 'k6/http';
import { check } from 'k6';

const TARGET_URL = __ENV.TARGET_URL;
const AUTH_HEADER = __ENV.AUTH_HEADER; // optional, only needed for EDC's NoopIdentityService gate

const BODY = JSON.stringify({
  '@context': ['https://w3id.org/dspace/2025/1/context.jsonld'],
  '@type': 'CatalogRequestMessage',
});

const headers = { 'Content-Type': 'application/json' };
if (AUTH_HEADER) {
  headers['Authorization'] = AUTH_HEADER;
}
const PARAMS = { headers };

export const options = {
  scenarios: {
    catalog_request: {
      executor: 'constant-vus',
      vus: 20,
      duration: '30s',
    },
  },
  thresholds: {
    http_req_duration: ['p(95)<5000', 'p(99)<5000'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const res = http.post(TARGET_URL, BODY, PARAMS);
  check(res, {
    'status is 200': (r) => r.status === 200,
  });
}
```

Invocations (run sequentially, never concurrently):

```bash
# Rust
k6 run -e TARGET_URL=http://127.0.0.1:18080/dsp/catalog/request catalog-request.k6.js

# EDC
k6 run -e TARGET_URL=http://127.0.0.1:8082/api/dsp/2025-1/catalog/request \
       -e AUTH_HEADER="Bearer 1234" catalog-request.k6.js
```

Both runs: 20 constant VUs, 30 seconds, identical request body, identical
thresholds. RSS/CPU sampling ran concurrently with each k6 invocation via a
1-second-interval loop reading `/proc/<pid>/status` (`VmRSS`) and
`/proc/<pid>/stat` (fields 14+15, utime+stime in jiffies), converting the
jiffies delta over the sampling window's wall-clock delta into a CPU
percentage (`CLK_TCK=100` on this host, confirmed via `getconf CLK_TCK`;
22 logical cores available, confirmed via `nproc`, so 100% = one core's
worth of the reported percentage — the EDC figure of ~1475% means EDC used
roughly 14-15 cores' worth of CPU time during the run, well within the
22-core budget but a large multiple of what Rust used).

## What this doesn't prove

- **Not apples-to-apples on data volume.** EDC serves 17 seeded assets;
  Rust serves 1. EDC's per-request work (17x the JSON-LD framing, 17x the
  policy-engine evaluation, 17x the contract-offer-id computation) is
  strictly larger than Rust's, so *some* of the latency/throughput gap is
  attributable to serving 17x more data, not solely to implementation
  overhead (JVM vs. native, JSON-LD library vs. hand serialization, etc.).
  This benchmark does not attempt to normalize for that — an apples-to-apples
  version would need either a 1-asset EDC seed or a 17-dataset Rust seed,
  neither of which this run produced.
- **JVM warmup/JIT is a real confound in a 30-second run.** EDC's JVM had
  already served two manual warmup requests plus whatever class-loading
  happened at boot before the load test started, but 30 seconds of
  sustained load is short relative to typical JIT tiered-compilation
  timelines (C1 → C2 warmup can take tens of seconds to minutes under real
  load). The reported EDC latency/throughput numbers plausibly understate
  EDC's steady-state performance; a longer run (multiple minutes) would be
  needed to see whether p95/p99 continue improving. Rust has no comparable
  warmup curve (native, ahead-of-time compiled), so this asymmetry favors
  Rust's numbers looking relatively better than they might in an extended
  comparison.
- **One endpoint, not the whole system.** This measures exactly one HTTP
  route on each side. It says nothing about contract negotiation, transfer
  process, persistence-layer behavior under real (non-in-memory) storage,
  or behavior under the EDC connector's full intended feature set
  (data-plane transfer, policy monitor, callback dispatch, etc. — all of
  which were booted as part of EDC's 88 service extensions and consuming
  idle RSS, even though none of them were exercised).
- **RSS/CPU sampling resolution is coarse.** 1-second polling on a 30-second
  run gives ~30 samples; genuine spikes between samples (e.g. a GC pause
  or a burst allocation) could be missed, especially on the JVM side where
  GC behavior is bursty by nature. The "peak RSS" and "avg CPU%" figures
  are a reasonable approximation, not a precise trace.
- **Single run, no repetition.** Each configuration was benchmarked exactly
  once. No variance/confidence-interval data was collected; a single
  14.17ms (Rust) or 408.87ms (EDC) max-latency outlier could be sampling
  noise, a GC pause, OS scheduling jitter, or something systematic — this
  run can't distinguish those.
- **Idle RSS measured after only 1-2 warmup requests**, not after a
  sustained idle period following load (which might show different
  memory-retention behavior, especially for the JVM's heap sizing
  (`-Xms256m -Xmx512m` per the runtime's JVM args) versus what was actually
  observed under load, 2.5 GB peak RSS — well above the configured
  `-Xmx512m` heap, meaning most of that growth is off-heap: JVM metaspace,
  thread stacks (Jetty/Jersey worker pools sized to the request
  concurrency), JIT-compiled code cache, and native library memory
  (Titanium JSON-LD, BouncyCastle, etc.) — not just Java heap.
- **Rust's simpler wire format may be an artifact of unfinished, not
  intentionally minimal, work.** Several of the fidelity gaps noted above
  (dropped `endpointDescription`, ignored `filter`, no real policy model)
  are explicitly documented in `http-api`'s own source as known,
  intentional placeholders pending future work — not a claim that Rust's
  approach is the "correct" or final DSP wire shape.
- **This did not run the actual `dsp-tck` conformance suite** against
  either server in this session — only manual, targeted `curl` comparisons
  against the TCK's published JSON schemas. A full `dsp-tck` `CAT`/`MET`
  group run (as described in
  `docs/spikes/2026-08-27-dataspacetck-compliance-suites.md` and this
  repo's own `compliance/README.md`) would be a more authoritative fidelity
  check than this manual comparison.

## Cleanup

Both server processes and the Gradle daemon were stopped after this
benchmark (`pkill` on the `http-api` binary path and the EDC `BaseRuntime`
process, followed by `./gradlew --stop` in
`vendor/eclipse-edc-connector`) — nothing from this run was left running.
