#!/usr/bin/env bash
# Launches one real Eclipse EDC 0.18.0 control-plane instance from this
# project's own compiled classpath (see build.gradle.kts's
# `printClasspath` task - run once, via `./gradlew printClasspath`,
# before using this script). Parameterized entirely by env vars so the
# same script starts N independently-seeded instances on N different
# ports - see ../crawler-edc-integration-test.md for how this was used
# to start three.
#
# Required env vars:
#   INSTANCE_NAME        - a short label, used only in log messages here.
#   BASE_PORT             - the default/root HTTP port; every other port
#                            below is BASE_PORT + a fixed offset (keeps
#                            each instance's whole port block contiguous
#                            and easy to reason about).
#   FIXTURE_PARTICIPANT_ID  - sets both EDC_PARTICIPANT_ID and
#                            EDC_PARTICIPANT_CONTEXT_ID to the same
#                            value, deliberately - see
#                            CatalogFixtureExtension's doc comment and
#                            the known-pitfall list in
#                            ../crawler-edc-integration-test.md.
#   FIXTURE_ASSET_IDS     - comma-separated dataset ids to seed, e.g.
#                            "EDC-A-01,EDC-A-02" - see
#                            CatalogFixtureExtension.java.
#
# Optional:
#   LOG_FILE               - defaults to logs/$INSTANCE_NAME.log.
#
# Uses `exec` (not a backgrounded subshell) so that if the *caller*
# backgrounds this script (`run-instance.sh & PID=$!`), $! is the actual
# `java` process's PID, not a wrapper shell's - required for the "record
# each PID from $!" / "confirm via ss -tlnp that each PID actually owns
# the port" verification this fixture's integration test doc requires.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

: "${INSTANCE_NAME:?set INSTANCE_NAME}"
: "${BASE_PORT:?set BASE_PORT}"
: "${FIXTURE_PARTICIPANT_ID:?set FIXTURE_PARTICIPANT_ID}"
: "${FIXTURE_ASSET_IDS:?set FIXTURE_ASSET_IDS}"

CLASSPATH_FILE="$SCRIPT_DIR/build/classpath.txt"
if [ ! -f "$CLASSPATH_FILE" ]; then
  echo "Missing $CLASSPATH_FILE - run './gradlew printClasspath' in $SCRIPT_DIR first." >&2
  exit 1
fi
CP="$(cat "$CLASSPATH_FILE")"

LOG_FILE="${LOG_FILE:-$SCRIPT_DIR/logs/$INSTANCE_NAME.log}"
mkdir -p "$(dirname "$LOG_FILE")"

export WEB_HTTP_PORT=$((BASE_PORT))
export WEB_HTTP_PATH="/api"
export WEB_HTTP_MANAGEMENT_PORT=$((BASE_PORT + 10))
export WEB_HTTP_MANAGEMENT_PATH="/api/management"
export WEB_HTTP_PROTOCOL_PORT=$((BASE_PORT + 20))
export WEB_HTTP_PROTOCOL_PATH="/api/dsp"
export WEB_HTTP_CONTROL_PORT=$((BASE_PORT + 30))
export WEB_HTTP_CONTROL_PATH="/api/control"
export WEB_HTTP_CATALOG_PORT=$((BASE_PORT + 40))
export WEB_HTTP_CATALOG_PATH="/api/catalog"
export WEB_HTTP_VERSION_PORT=$((BASE_PORT + 50))
export WEB_HTTP_VERSION_PATH="/api/version"
# data-plane-signaling-core's SignalingApiConfiguration hardcodes a
# DEFAULT_SIGNALING_PORT of 8182 (not one of the ports the tck-runtime.env
# recipe in compliance/benchmark-2026-08-27.md happened to set) - without
# overriding it, every instance's Jetty tries to bind the same fixed 8182
# and only the first one to start wins; discovered here by reading the
# `java.net.BindException: Address already in use` on 0.0.0.0:8182 from
# the second/third instance's own log (see
# ../crawler-edc-integration-test.md).
export WEB_HTTP_SIGNALING_PORT=$((BASE_PORT + 60))
export WEB_HTTP_SIGNALING_PATH="/api/signaling"
export EDC_API_AUTH_KEY="fixture-password-$INSTANCE_NAME"
export EDC_IAM_DID_WEB_USE_HTTPS="false"
export EDC_DSP_CALLBACK_ADDRESS="http://localhost:$((BASE_PORT + 20))/api/dsp"
export EDC_PARTICIPANT_ID="$FIXTURE_PARTICIPANT_ID"
export EDC_PARTICIPANT_CONTEXT_ID="$FIXTURE_PARTICIPANT_ID"
export EDC_MANAGEMENT_CONTEXT_ENABLED=true
export FIXTURE_ASSET_IDS="$FIXTURE_ASSET_IDS"

echo "[$INSTANCE_NAME] protocol (DSP) port=$WEB_HTTP_PROTOCOL_PORT participantContextId=$FIXTURE_PARTICIPANT_ID assets=$FIXTURE_ASSET_IDS log=$LOG_FILE" >&2

exec java -cp "$CP" org.eclipse.edc.boot.system.runtime.BaseRuntime >>"$LOG_FILE" 2>&1
