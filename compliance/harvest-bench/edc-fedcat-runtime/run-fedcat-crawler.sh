#!/usr/bin/env bash
# Launches EDC's own federated-catalog crawler runtime (this Gradle
# project's classpath) as a single, real Eclipse EDC 0.18.0 process built
# from federatedcatalog-base-bom + iam-mock. Mirrors
# ../../crawler-edc-fixture/run-instance.sh's env-var-driven, `exec java`
# pattern (see that script's own doc comment for why `exec`, not a
# backgrounded subshell, matters for PID tracking) - run
# `./gradlew printClasspath` here once before using this script.
#
# Required env vars:
#   BASE_PORT           - root HTTP port; every other port below is
#                          BASE_PORT + a fixed offset, same scheme as
#                          run-instance.sh.
#   HARVEST_TARGET_NODES - passed straight through to HarvestSeedExtension,
#                          see that class's doc comment for the format.
#
# Optional:
#   CRAWL_PERIOD_SECONDS - edc.catalog.cache.execution.period.seconds
#                          (default 5 - short, so a crawl cycle completes
#                          well within a 30s load-test window).
#   LOG_FILE              - defaults to logs/fedcat-crawler.log.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

: "${BASE_PORT:?set BASE_PORT}"
: "${HARVEST_TARGET_NODES:?set HARVEST_TARGET_NODES}"
CRAWL_PERIOD_SECONDS="${CRAWL_PERIOD_SECONDS:-5}"

CLASSPATH_FILE="$SCRIPT_DIR/build/classpath.txt"
if [ ! -f "$CLASSPATH_FILE" ]; then
  echo "Missing $CLASSPATH_FILE - run './gradlew printClasspath' in $SCRIPT_DIR first." >&2
  exit 1
fi
CP="$(cat "$CLASSPATH_FILE")"

LOG_FILE="${LOG_FILE:-$SCRIPT_DIR/logs/fedcat-crawler.log}"
mkdir -p "$(dirname "$LOG_FILE")"

export WEB_HTTP_PORT=$((BASE_PORT))
export WEB_HTTP_PATH="/api"
export WEB_HTTP_MANAGEMENT_PORT=$((BASE_PORT + 10))
export WEB_HTTP_MANAGEMENT_PATH="/api/management"
export WEB_HTTP_PROTOCOL_PORT=$((BASE_PORT + 20))
export WEB_HTTP_PROTOCOL_PATH="/api/dsp"
export WEB_HTTP_CONTROL_PORT=$((BASE_PORT + 30))
export WEB_HTTP_CONTROL_PATH="/api/control"
export WEB_HTTP_VERSION_PORT=$((BASE_PORT + 50))
export WEB_HTTP_VERSION_PATH="/api/version"
# Same hardcoded-default-port pitfall as ../../crawler-edc-fixture/run-instance.sh
# documents for data-plane-signaling-core (DEFAULT_SIGNALING_PORT=8182) -
# federatedcatalog-base-bom pulls in transfer-data-plane-signaling /
# data-plane-signaling-client too, so give it its own port defensively.
export WEB_HTTP_SIGNALING_PORT=$((BASE_PORT + 60))
export WEB_HTTP_SIGNALING_PATH="/api/signaling"

export EDC_PARTICIPANT_ID="edc-fedcat-crawler"
export EDC_PARTICIPANT_CONTEXT_ID="edc-fedcat-crawler"
export EDC_IAM_DID_WEB_USE_HTTPS="false"
export EDC_DSP_CALLBACK_ADDRESS="http://localhost:$((BASE_PORT + 20))/api/dsp"

export EDC_CATALOG_CACHE_EXECUTION_ENABLED="true"
export EDC_CATALOG_CACHE_EXECUTION_PERIOD_SECONDS="$CRAWL_PERIOD_SECONDS"
export EDC_CATALOG_CACHE_EXECUTION_DELAY_SECONDS="0"
export EDC_CATALOG_CACHE_PARTITION_NUM_CRAWLERS="2"

export HARVEST_TARGET_NODES="$HARVEST_TARGET_NODES"

echo "[edc-fedcat-crawler] management (Management API) port=$WEB_HTTP_MANAGEMENT_PORT protocol (DSP) port=$WEB_HTTP_PROTOCOL_PORT period=${CRAWL_PERIOD_SECONDS}s log=$LOG_FILE" >&2

exec java -cp "$CP" org.eclipse.edc.boot.system.runtime.BaseRuntime >>"$LOG_FILE" 2>&1
