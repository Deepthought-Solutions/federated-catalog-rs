// Harvesting benchmark load-test script (compliance/harvest-benchmark-2026-08-27.md).
// Same methodology as compliance/benchmark-2026-08-27.md's own
// catalog-request.k6.js (20 constant VUs, 30s, same thresholds) -
// generalized here with a BODY env var so the same script drives both:
//
//   - EDC's own federated-catalog crawler's Management API endpoint
//     (POST {mgmt}/v3/catalogs/request, body: an empty QuerySpec JSON-LD
//     object - see run-harvest-bench.sh's EDC_BODY).
//   - This project's own http-api DSP endpoint (POST /dsp/catalog/request,
//     body: a bare CatalogRequestMessage - see run-harvest-bench.sh's
//     RUST_BODY), unchanged from the original report's request shape.
import http from 'k6/http';
import { check } from 'k6';

const TARGET_URL = __ENV.TARGET_URL;
const AUTH_HEADER = __ENV.AUTH_HEADER; // optional
const BODY = __ENV.BODY || JSON.stringify({
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
