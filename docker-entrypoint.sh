#!/bin/bash
set -e

DATA_DIR="/data"
LORE_HOME="/root/.lore"
CONFIG_FILE="${LORE_HOME}/config.toml"
DB_PATH="${DATA_DIR}/memory.db"

mkdir -p "${LORE_HOME}" "${DATA_DIR}"

# Generate config.toml from env vars (unless user mounted one)
if [ ! -f "${CONFIG_FILE}" ]; then
    INTERVAL="${LORE_CONSOLIDATION_INTERVAL:-7200}"
    IDLE="${LORE_IDLE_THRESHOLD:-300}"
    EXTRACTION_MODEL="${LORE_EXTRACTION_MODEL:-claude-sonnet-4-20250514}"
    COMPRESSION_MODEL="${LORE_COMPRESSION_MODEL:-claude-haiku-4-5-20251001}"

    cat > "${CONFIG_FILE}" <<EOF
[database]
path = "${DB_PATH}"

[consolidation]
interval_secs = ${INTERVAL}
idle_threshold_secs = ${IDLE}

[claude]
extraction_model = "${EXTRACTION_MODEL}"
compression_model = "${COMPRESSION_MODEL}"
EOF
    echo "Generated config at ${CONFIG_FILE}"
fi

# Start lore-server
echo "Starting lore-server on port 8080..."
lore-server --db "${DB_PATH}" &
SERVER_PID=$!

# Start consolidation daemon if API key is available
DAEMON_PID=""
if [ -n "${ANTHROPIC_API_KEY}" ]; then
    echo "Starting consolidation daemon..."
    lore start --config "${CONFIG_FILE}" &
    DAEMON_PID=$!
else
    echo "WARNING: No ANTHROPIC_API_KEY set. Consolidation disabled."
    echo "Staged turns will accumulate but not be processed."
fi

# Forward signals to children
cleanup() {
    echo "Shutting down..."
    kill -TERM "${SERVER_PID}" 2>/dev/null || true
    [ -n "${DAEMON_PID}" ] && kill -TERM "${DAEMON_PID}" 2>/dev/null || true
    wait
    exit 0
}
trap cleanup SIGTERM SIGINT

# Wait for either process to exit
wait -n
EXIT_CODE=$?
echo "A process exited with code ${EXIT_CODE}, shutting down..."
cleanup
