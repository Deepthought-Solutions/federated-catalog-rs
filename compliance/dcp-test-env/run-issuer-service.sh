#!/usr/bin/env bash
# Builds (if needed) and runs eclipse-edc/IdentityHub's "issuer-service"
# launcher with the dcp-test-env seed extension attached, publishing a
# real, resolvable did:web document for the "issuer" identity whose
# private key IdentityHubSeedExtension uses to sign the seeded
# credential (see that class's doc comment for what this environment
# does and doesn't do end to end).
#
# Same prerequisites/caveats as run-identityhub.sh - see that script and
# README.md.
#
# Ports (see README.md "Port map"):
#   9090 base, 9091 identity API, 9092 issuer admin API,
#   9093 DID hosting, 9094 STS, 9095 issuance protocol API.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IDENTITY_HUB_DIR="${IDENTITY_HUB_DIR:-$SCRIPT_DIR/../../../dataspace/vendor/identity-hub}"
KEYS_DIR="$SCRIPT_DIR/keys"
SEED_DIR="$SCRIPT_DIR/seed"

mkdir -p "$KEYS_DIR"

NIMBUS_JAR=$(find ~/.gradle/caches -name 'nimbus-jose-jwt-*.jar' | head -1)
if [ -z "$NIMBUS_JAR" ]; then
  echo "Could not find nimbus-jose-jwt jar in ~/.gradle/caches - build identity-hub/issuer-service first." >&2
  exit 1
fi

mkdir -p "$SEED_DIR/out"
javac -cp "$NIMBUS_JAR" -d "$SEED_DIR/out" "$SEED_DIR/src/GenerateIssuerKey.java"
if [ ! -f "$KEYS_DIR/issuer-key-private.jwk.json" ]; then
  java -cp "$NIMBUS_JAR:$SEED_DIR/out" GenerateIssuerKey "$KEYS_DIR" issuer-key
fi

CLASSPATH_FILE="$SEED_DIR/issuer-classpath.txt"
if [ ! -f "$CLASSPATH_FILE" ]; then
  echo "First run: capturing issuer-service's classpath via a throwaway run (needed once per checkout)..." >&2
  (cd "$IDENTITY_HUB_DIR" && nohup ./gradlew :launcher:issuer-service:run > /tmp/issuer-classpath-capture.log 2>&1 &)
  for _ in $(seq 1 180); do
    PID=$(pgrep -f "launcher/issuer-service/build/classes" | head -1 || true)
    [ -n "$PID" ] && break
    sleep 1
  done
  python3 -c "
with open('/proc/$PID/cmdline', 'rb') as f:
    args = f.read().split(b'\x00')
args = [a.decode() for a in args if a]
cp = args[args.index('-cp') + 1]
open('$CLASSPATH_FILE', 'w').write(cp)
"
  kill -9 "$PID" 2>/dev/null || true
fi
CP="$(cat "$CLASSPATH_FILE")"

mkdir -p "$SEED_DIR/out/META-INF/services"
javac -cp "$CP" -d "$SEED_DIR/out" "$SEED_DIR/src/IssuerServiceSeedExtension.java"
printf 'IssuerServiceSeedExtension\n' > "$SEED_DIR/out/META-INF/services/org.eclipse.edc.spi.system.ServiceExtension"
(cd "$SEED_DIR/out" && jar cf ../issuer-service-seed.jar IssuerServiceSeedExtension.class META-INF/services/org.eclipse.edc.spi.system.ServiceExtension)

export WEB_HTTP_PORT=9090 WEB_HTTP_PATH=/api \
  WEB_HTTP_IDENTITY_PORT=9091 WEB_HTTP_IDENTITY_PATH=/api/identity \
  WEB_HTTP_ISSUERADMIN_PORT=9092 WEB_HTTP_ISSUERADMIN_PATH=/api/issueradmin \
  WEB_HTTP_DID_PORT=9093 WEB_HTTP_DID_PATH=/ \
  WEB_HTTP_STS_PORT=9094 WEB_HTTP_STS_PATH=/sts \
  WEB_HTTP_ISSUANCE_PORT=9095 WEB_HTTP_ISSUANCE_PATH=/api/issuance \
  EDC_IAM_DID_WEB_USE_HTTPS=false \
  SEED_ISSUER_DID_HOST=localhost:9093 \
  SEED_KEYS_DIR="$KEYS_DIR"

echo "Starting seeded Issuer Service on :9090-9095 ..."
exec java -cp "$CP:$SEED_DIR/issuer-service-seed.jar" org.eclipse.edc.boot.system.runtime.BaseRuntime
