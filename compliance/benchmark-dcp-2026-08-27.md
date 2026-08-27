# DSP catalog-request benchmark: real DCP auth overhead, and vs. Eclipse EDC 0.18.0

**Date:** 2026-08-27
**Endpoint under test:** `POST {baseUrl}/catalog/request` (DSP v2025-1 catalog protocol).

This extends `compliance/benchmark-2026-08-27.md` (read that report first for the
original no-auth Rust vs. EDC comparison and its own methodology/caveats, which
this one inherits and does not repeat in full) now that `http-api` has a real,
independently-verified `DspAuthMode::Dcp` (see the two commits on `main` titled
"Add DspAuthMode::Dcp..." and "Add compliance/dcp-test-env..." for what was
verified and how). Three configurations were benchmarked, one at a time, on the
same otherwise-idle host as the original report:

1. **Rust, no auth** (`DspAuthMode::Disabled`) — reproduces the original
   baseline in this session, for an apples-to-apples "what does adding real DCP
   cost, on the same binary, same host, same load profile" comparison.
2. **Rust, real DCP** (`DspAuthMode::Dcp`) — every request performs the full,
   real verification flow documented in `crates/http-api/src/dcp.rs`: resolve
   the caller's `did:web` over HTTP, verify the incoming self-issued token's
   ES256 signature, re-package its nested token and sign it with this
   connector's own key, POST it to the caller's real Presentation API
   (`compliance/dcp-test-env`'s live, seeded `eclipse-edc/IdentityHub`), and
   verify the returned Verifiable Presentation's and Verifiable Credential's
   ES256 signatures against their respective DIDs.
3. **Java (EDC 0.18.0)** — same `dsp-tck-connector-under-test` runtime and
   config as the original report, **still gated by the TCK's
   `NoopIdentityService` stub, not real DCP** — see "What this doesn't prove"
   below for why a genuinely DCP-secured EDC endpoint was not built for this
   round, and why comparing it to Rust's real-DCP numbers is not a like-for-like
   DCP comparison.

## Summary table

| Metric | Rust, no auth | Rust, real DCP | EDC 0.18.0 (stub auth) |
|---|---:|---:|---:|
| Idle RSS (post-warmup) | 15.15 MB (15,516 KB) | 12.79 MB (13,096 KB) | 344.0 MB (352,260 KB) |
| Peak RSS under load | 15.51 MB (15,880 KB) | 23.83 MB (24,396 KB) | 2,611.6 MB (2,674,300 KB) |
| Avg CPU during load | ~321% of 1 core (~3.2 cores) | ~461% of 1 core (~4.6 cores) | ~1,340% of 1 core (~13.4 cores) |
| Throughput | 101,522.7 req/s | 2,278.2 req/s | 655.3 req/s |
| Latency avg | 148.66 µs | 8.66 ms | 30.38 ms |
| Latency p50 (median) | 127.82 µs | 7.94 ms | 28.48 ms |
| Latency p90 | 243.49 µs | 11.74 ms | 44.92 ms |
| Latency p95 | 296.59 µs | 13.94 ms | 48.98 ms |
| Latency p99 | 466.43 µs | 23.64 ms | 57.77 ms |
| Latency max | 12.42 ms | 85.06 ms | 242.27 ms |
| Error rate | 0.00% (3,045,714/3,045,714 OK) | 0.00% (68,365/68,365 OK) | 0.00% (19,673/19,673 OK) |
| Seeded catalog size | 2 datasets (`CAT0101`, `CAT0102`) | 2 datasets (same) | 18 datasets (incl. `CAT0101`, `CAT0102`) |
| Auth performed | none | **real**: DID resolution + ES256 verify + presentation-query round trip + VP/VC verify | **stub**: `NoopIdentityService`, any bearer token accepted, no signature/identity check at all |

Environment: 22 logical cores (`nproc`), `CLK_TCK=100`, same host as the
original report. Only one server process (plus, for the DCP run, the
`compliance/dcp-test-env` IdentityHub/Issuer Service pair it depends on) was
under load at any one time.

**Note on the Rust seed data changing since the original report:** the DCP
work also changed `seed_sample_catalog` to seed two datasets (`CAT0101`,
`CAT0102`, aligned with EDC's own TCK ids — see that function's doc comment)
where the original benchmark measured against one (`sample-dataset`). The
"Rust, no auth" column here is a fresh same-session re-run against the
*current* two-dataset seed, not a copy of the original report's numbers, so
the two are close but not directly identical-input comparisons. EDC's own
seed produced **18** datasets this run vs. **17** in the original report; this
was observed, not investigated further (both runs used the same
`tck-runtime.env`-derived config), and doesn't change any conclusion below.

## The headline finding: real DCP's cost is dominated by network round trips to live identity infrastructure, not by Rust's own crypto

Turning on `DspAuthMode::Dcp` on the exact same Rust binary and host:

| | No auth | Real DCP | Change |
|---|---:|---:|---:|
| Throughput | 101,522.7 req/s | 2,278.2 req/s | **−97.8%** (44.6× fewer req/s) |
| Latency avg | 148.66 µs | 8.66 ms | **+8.51 ms** (~58×) |
| Peak RSS | 15.51 MB | 23.83 MB | +8.32 MB (~1.5×) |
| Avg CPU | ~3.2 cores | ~4.6 cores | +~1.4 cores |

That looks like a large regression, and it is — but it is not primarily a
cost of ES256 signature math or JSON handling in Rust. During the DCP run,
`compliance/dcp-test-env`'s IdentityHub process (the caller's own real
identity infrastructure, which `dcp.rs` must call out to twice per request —
once implicitly via `did:web` resolution, once explicitly via the
presentation-query POST) was independently sampled at the same time:

| | Rust (`http-api`, DCP mode) | IdentityHub (dependency) |
|---|---:|---:|
| Peak RSS during the run | 23.83 MB | **1,188.0 MB (1.16 GB)** |
| Avg CPU during the run | ~4.6 cores | **~8.6 cores** |

IdentityHub used roughly **50× the RSS and 1.9× the CPU** of the Rust
connector calling it, during the identical 30-second window, and was the
actual throughput ceiling: `dcp.rs` has no caching of DID documents,
presentation results, or verified credentials — every single catalog request
triggers a fresh `did:web` GET and a fresh, real, ES256-signed
presentation-query round trip to a live JVM service. At 20 concurrent
callers, IdentityHub's own request-handling and JWS-signing capacity, not
Rust's, is what limits throughput to ~2,278 req/s. A caching layer (VCs
already carry their own `exp`; a real deployment could safely cache a
verified `VerifiedCaller` for some bounded TTL) is the obvious next
optimization and was explicitly out of scope for this round — this benchmark
measures the current, uncached implementation, not a ceiling on what a DCP
verifier could achieve.

## Rust-with-real-DCP vs. EDC-with-stub-auth

With that caveat firmly in mind (Rust is paying the full cost of real
cryptographic identity verification; EDC here is paying almost nothing for
auth at all — see "What this doesn't prove"):

| | Rust, real DCP | EDC, stub auth | Rust's factor |
|---|---:|---:|---:|
| Throughput | 2,278.2 req/s | 655.3 req/s | **3.5× higher** |
| Latency avg | 8.66 ms | 30.38 ms | **3.5× lower** |
| Peak RSS | 23.83 MB | 2,611.6 MB | **~110× smaller** |
| Avg CPU | ~4.6 cores | ~13.4 cores | **~2.9× less** |

Rust doing real DCP verification still outperforms EDC's stub-authenticated
endpoint on every measured dimension, by a wide margin — but this is not a
fair "DCP overhead" comparison in either direction: EDC is serving 18
datasets with a full JSON-LD/policy-engine pipeline and paying essentially
zero auth cost, while Rust is serving 2 placeholder datasets and paying the
full real-DCP round-trip cost. It demonstrates that Rust's architecture has
enough headroom to absorb real DCP verification and still beat EDC's
unauthenticated-equivalent numbers, not that "Rust with DCP is 3.5× faster
than EDC with DCP" — that comparison was not run (see below).

## How the servers were run

**Rust, no auth:**
```bash
cd federated-catalog-rs
cargo build --release -p http-api
HTTP_API_ADDR=0.0.0.0:18080 ./target/release/http-api &
```

**Rust, real DCP** (after starting `compliance/dcp-test-env`'s IdentityHub and
Issuer Service per its README — `./run-identityhub.sh & ./run-issuer-service.sh &`):
```bash
cd federated-catalog-rs
DSP_AUTH_MODE=dcp \
DSP_DCP_OWN_DID_HOST=localhost:18080 \
DSP_DCP_INSECURE_HTTP=true \
HTTP_API_ADDR=0.0.0.0:18080 \
./target/release/http-api &
```
A single valid self-issued token (T1) was minted once, from the real seeded
holder identity, via IdentityHub's own STS (`POST /sts/token`, `audience` set
to the Rust connector's own `did:web:localhost%3A18080:dsp`, `bearer_access_scope`
set to the seeded credential's required scope), and reused as the
`Authorization: Bearer` header for every k6 request in the run — the same way
a real caller would cache and reuse a token across multiple catalog requests
rather than re-minting one per call (T1's `exp` is 300s from issuance;
IdentityHub's `edc.iam.accesstoken.jti.validation` is disabled in this test
environment, matching its own documented settings, so replay is not rejected).
This benchmark does not measure the cost of a caller repeatedly re-minting T1
from their own STS — that cost is external to `federated-catalog-rs` entirely.

**EDC:** identical to the original report's config and startup —
`WEB_HTTP_PROTOCOL_PORT=8082`, `EDC_PARTICIPANT_CONTEXT_ID=CONNECTOR_UNDER_TEST`,
etc. — see `compliance/benchmark-2026-08-27.md` for the full env-var list and
the two real bugs that config works around.
```bash
curl -X POST http://127.0.0.1:8082/api/dsp/2025-1/catalog/request \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer 1234" \
  -d '{"@context": ["https://w3id.org/dspace/2025/1/context.jsonld"], "@type": "CatalogRequestMessage"}'
```

## Load test methodology

Same k6 script as the original report, generalized so `AUTH_HEADER` is
whatever bearer value the target config needs (unset for Rust-no-auth,
the real T1 token for Rust-DCP, `Bearer 1234` for EDC):

```javascript
import http from 'k6/http';
import { check } from 'k6';

const TARGET_URL = __ENV.TARGET_URL;
const AUTH_HEADER = __ENV.AUTH_HEADER; // optional bearer header

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
k6 run -e TARGET_URL=http://127.0.0.1:18080/dsp/catalog/request \
       catalog-request.k6.js                              # Rust, no auth

k6 run -e TARGET_URL=http://127.0.0.1:18080/dsp/catalog/request \
       -e AUTH_HEADER="Bearer $T1" \
       catalog-request.k6.js                              # Rust, real DCP

k6 run -e TARGET_URL=http://127.0.0.1:8082/api/dsp/2025-1/catalog/request \
       -e AUTH_HEADER="Bearer 1234" \
       catalog-request.k6.js                              # EDC, stub auth
```

Each: 20 constant VUs, 30 seconds, identical request body, identical
thresholds — all thresholds passed (`p(95)<5000ms`, `p(99)<5000ms`,
`http_req_failed rate<0.01`) in all three runs; error rate was 0.00% in all
three. RSS/CPU sampling ran concurrently with each k6 invocation via a
1-second-interval loop reading `/proc/<pid>/status` (`VmRSS`) and
`/proc/<pid>/stat` (fields 14+15, utime+stime in jiffies, converted to a CPU%
using `CLK_TCK=100`) — same method as the original report. One real mechanical
error was caught and corrected while doing this: the shell's `$!` after a
backgrounded, wrapped `nohup ... &` invocation captured the *wrapper shell's*
PID, not the actual `http-api` binary's — this was caught because the
resulting sampler showed an impossible flat 0% CPU and static RSS under
100k req/s of real load, and was fixed by resolving the PID from `ss -tlnp`
(which reports the PID actually bound to the listening socket) instead of `$!`.
This is flagged explicitly, not silently corrected, per this task's
requirement to report the real state rather than a plausible-looking one.

## What this doesn't prove

- **EDC's side was not configured with real DCP identity verification.** It
  uses the same `NoopIdentityService` stub as the original report — any
  bearer token is accepted, with no signature check, no DID resolution, and
  no credential validation at all. Building a genuinely DCP-secured EDC
  endpoint for this comparison (registering the connector as an IdentityHub
  participant, hosting its own `did:web` document, wiring EDC's own
  `DcpIdentityService` to the same `compliance/dcp-test-env` instances,
  minting real credentials scoped for it) is a substantially larger task than
  this benchmark round budgeted for, and was not attempted. **The
  "Rust-with-real-DCP vs. EDC-with-stub-auth" table above is not a
  like-for-like DCP comparison** — it shows Rust absorbing real DCP's cost and
  still beating EDC's near-zero-auth-cost numbers, nothing more or less than
  that.
- **Not apples-to-apples on data volume, same as the original report.** EDC
  serves 18 seeded assets with a full JSON-LD/policy-engine pipeline; Rust
  serves 2 with a hardcoded placeholder policy. See
  `compliance/benchmark-2026-08-27.md`'s fidelity section for the full,
  still-applicable breakdown of wire-format differences.
- **No caching in `dcp.rs`.** Every request does a full DID resolution and a
  full presentation-query round trip, even for the same caller and the same
  still-valid credential requested a moment ago. The ~2,278 req/s DCP figure
  reflects that specific, current implementation choice, not an inherent
  limit of verifying DCP credentials in Rust — see "The headline finding"
  above.
- **IdentityHub's own resource usage is a real cost of this architecture, not
  measured as a first-class line item elsewhere.** ~1.16 GB peak RSS and
  ~8.6 cores of CPU were consumed by IdentityHub during the DCP load test,
  external to both connectors being compared. A deployment that terminates
  real DCP traffic at scale needs to budget for the *caller's* identity
  infrastructure cost too, not just the relying party's.
- **Single run, no repetition, coarse 1-second RSS/CPU sampling, short 30s
  window** — all the same caveats the original report already lists (JVM
  warmup/JIT for EDC, one endpoint not the whole system, no dsp-tck
  conformance run this round either) apply again here and are not repeated in
  full.
- **The DCP round trip's cost was measured with a single, fixed, already-valid
  token reused across all 20 VUs and the whole 30s window** — this measures
  the connector's verification cost per request, not the cost of every VU
  independently minting and using a fresh token, nor any behavior under many
  distinct concurrent holder identities (all 20 VUs here authenticate as the
  same holder).

## Cleanup

All processes started for this benchmark were stopped afterward and verified
gone via `pgrep`/`ss`: the Rust `http-api` binary (both configurations), the
EDC `dsp-tck-connector-under-test` runtime, its Gradle daemon (`./gradlew
--stop` in `vendor/eclipse-edc-connector`), and both `compliance/dcp-test-env`
Java processes (IdentityHub, Issuer Service, killed by PID after `BaseRuntime`
pattern matching was double-checked against `pgrep -af java`). No Gradle
daemon, JVM, or `http-api` process was left running; ports
8080–8185/9080–9095/18080–18081 were confirmed free of anything started by
this session (8080 itself remains bound by this sandbox's pre-existing,
unrelated `pa-server` process, as in the original report).
