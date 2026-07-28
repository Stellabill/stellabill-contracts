#!/bin/sh
#
# scripts/deploy_local.sh — One-command local Soroban testnet setup
#
# Builds the subscription-vault contract, deploys it to a local Stellar
# quickstart network, creates test identities, wraps a test token, runs
# init as a smoke test, and exercises the full subscription lifecycle.
#
# Usage:
#   ./scripts/deploy_local.sh                       # full setup (Docker + deploy)
#   ./scripts/deploy_local.sh --no-docker            # skip Docker, use existing network
#   ./scripts/deploy_local.sh --skip-smoke           # skip the lifecycle smoke test
#   ./scripts/deploy_local.sh --help                 # print help and exit
#
# On re-run, the script re-uses any existing Docker container, CLI identity,
# and already-deployed token (stored in .deploy-state). If the contract is
# already initialized, init is skipped.
#
# Environment variables (optional):
#   STELLAR_CLI       Path to stellar/soroban CLI binary (auto-detected)
#   TOKEN_ADDR        Override token contract address (skip token deploy)
#   CONTRACT_DIR      Contract crate directory (default: contracts/subscription_vault)
#   NETWORK_NAME      Network alias (default: local-dev)
#   RPC_URL           Soroban RPC URL (default: http://localhost:8000/soroban/rpc)
#   NETWORK_PASSPHRASE (default: "Standalone Network ; February 2017")
#
# Exit codes:
#   0 — success
#   1 — dependency missing or configuration error
#   2 — build or deploy failure

set -eu

# ── Color helpers (POSIX-safe, fall back to plain text) ─────────────────────
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; BLUE=''; BOLD=''; NC=''
fi

# ── Paths ───────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
STATE_FILE="${ROOT_DIR}/.deploy-state"

# ── Default configuration ───────────────────────────────────────────────────
CONTRACT_DIR="${CONTRACT_DIR:-${ROOT_DIR}/contracts/subscription_vault}"
CONTRACT_NAME="subscription_vault"
NETWORK_NAME="${NETWORK_NAME:-local-dev}"
RPC_URL="${RPC_URL:-http://localhost:8000/soroban/rpc}"
NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Standalone Network ; February 2017}"
ADMIN_IDENTITY="admin-local"
SUBSCRIBER_IDENTITY="subscriber-local"
MERCHANT_IDENTITY="merchant-local"
TOKEN_DECIMALS=7
NETWORK_CONTAINER="stellabill-local"

# ── Flags ───────────────────────────────────────────────────────────────────
SKIP_SMOKE=false
NO_DOCKER=false
CLEANUP_CONTAINER=false

# ── Cleanup trap ────────────────────────────────────────────────────────────
cleanup() {
    if [ "${CLEANUP_CONTAINER}" = "true" ]; then
        info "Cleaning up container '${NETWORK_CONTAINER}' ..."
        docker stop "${NETWORK_CONTAINER}" 2>/dev/null || true
        docker rm "${NETWORK_CONTAINER}" 2>/dev/null || true
        ok "Container stopped and removed."
    fi
}
trap cleanup EXIT INT TERM

# ── Help ────────────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

One-command local Soroban testnet setup for the subscription-vault contract.

Builds the contract, deploys to a local Stellar quickstart network,
creates test identities, wraps a test token (native XLM via SAC), and
calls init as a smoke test.

OPTIONS:
  --no-docker        Skip Docker container start; assume network is already running
  --skip-smoke       Skip the full subscription lifecycle smoke test
  --help             Show this help message and exit

ENVIRONMENT VARIABLES:
  STELLAR_CLI       Path to stellar/soroban CLI (auto-detected if unset)
  TOKEN_ADDR        Pre-existing token contract address (skip token deploy)
  CONTRACT_DIR      Contract crate directory (default: contracts/subscription_vault)
  NETWORK_NAME      Network alias used by the CLI (default: local-dev)
  RPC_URL           Soroban RPC endpoint (default: http://localhost:8000/soroban/rpc)
  NETWORK_PASSPHRASE (default: "Standalone Network ; February 2017")
EOF
    exit 0
}

# ── Logging helpers ─────────────────────────────────────────────────────────
info()  { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[ OK ]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC} %s\n" "$*" >&2; }
err()   { printf "${RED}[ERR ]${NC} %s\n" "$*" >&2; }
step()  { printf "\n${BLUE}==>${NC} ${BOLD}%s${NC}\n" "$*"; }
run()   {
    printf "${YELLOW}\$ %s${NC}\n" "$*"
    "$@"
}

# =============================================================================
# STEP 0 — Dependency checks
# =============================================================================
detect_cli() {
    if [ -n "${STELLAR_CLI:-}" ]; then
        CLI="$STELLAR_CLI"
    elif command -v stellar >/dev/null 2>&1; then
        CLI="$(command -v stellar)"
    elif command -v soroban >/dev/null 2>&1; then
        CLI="$(command -v soroban)"
    else
        printf '%s\n' \
            "ERROR: Neither 'stellar' nor 'soroban' CLI found in PATH." \
            "" \
            "Install the Soroban CLI:" \
            "  https://developers.stellar.org/docs/tools/soroban-cli/install" \
            "" \
            "Quick install (macOS/Linux):" \
            "  curl -fsSL https://github.com/stellar/stellar-cli/raw/main/install.sh | sh" \
            >&2
        exit 1
    fi
    CLI_BASENAME="$(basename "${CLI}")"

    # Detect CLI version: old `soroban` uses --name flag, new `stellar` uses positional
    if "${CLI}" network add --help 2>&1 | grep -qF -- "--name" 2>/dev/null; then
        CLI_OLD_STYLE=1
    else
        CLI_OLD_STYLE=0
    fi

    if ! "${CLI}" --version >/dev/null 2>&1; then
        err "CLI binary '${CLI}' does not run. Is it a valid executable?"
        exit 1
    fi
    ok "CLI: ${CLI} $("${CLI}" --version 2>&1 | head -1)"
}

check_rust() {
    if ! command -v rustc >/dev/null 2>&1; then
        printf '%s\n' \
            "ERROR: Rust (rustc) not found. Install from: https://rustup.rs/" \
            "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" \
            >&2
        exit 1
    fi
    if ! rustup target list --installed 2>/dev/null | grep -q '^wasm32-unknown-unknown$'; then
        info "Adding wasm32-unknown-unknown target..."
        run rustup target add wasm32-unknown-unknown
    fi
    ok "Rust: $(rustc --version)"
}

check_curl() {
    if ! command -v curl >/dev/null 2>&1; then
        err "curl not found. Install curl to use this script."
        err "  apt-get install curl   # Debian/Ubuntu"
        err "  brew install curl      # macOS"
        exit 1
    fi
    ok "curl available."
}

check_docker() {
    if ! command -v docker >/dev/null 2>&1; then
        printf '%s\n' \
            "ERROR: Docker not found." \
            "" \
            "The local Soroban network requires a container running" \
            "stellar/quickstart:testing." \
            "" \
            "Install Docker: https://docs.docker.com/get-docker/" \
            "Or re-run with --no-docker if you already have a network." \
            >&2
        exit 1
    fi
    ok "Docker: $(docker --version 2>/dev/null)"
}

# =============================================================================
# STEP 1 — Build contract WASM
# =============================================================================
build_contract() {
    WASM_PATH="${ROOT_DIR}/target/wasm32-unknown-unknown/release/${CONTRACT_NAME}.wasm"
    if [ -f "${WASM_PATH}" ]; then
        info "Existing WASM found at ${WASM_PATH} — rebuilding."
    fi

    info "Building ${CONTRACT_NAME} WASM (release profile)..."
    run cargo build \
        --manifest-path "${CONTRACT_DIR}/Cargo.toml" \
        --release \
        --target wasm32-unknown-unknown

    if [ ! -f "${WASM_PATH}" ]; then
        # Try alternative path from workspace root
        WASM_PATH="${ROOT_DIR}/target/wasm32-unknown-unknown/release/${CONTRACT_NAME}.wasm"
    fi
    if [ ! -f "${WASM_PATH}" ]; then
        err "WASM not found after build. Searched: ${WASM_PATH}"
        ls -la "${ROOT_DIR}/target/wasm32-unknown-unknown/release/" 2>/dev/null || true
        exit 2
    fi
    WASM_SIZE="$(ls -lh "${WASM_PATH}" 2>/dev/null | awk '{print $5}')"
    [ -n "${WASM_SIZE}" ] || WASM_SIZE="unknown"
    ok "WASM built: ${WASM_PATH} (${WASM_SIZE})"
}

# =============================================================================
# STEP 2 — Start / verify local network
# =============================================================================
ensure_network() {
    if [ "${NO_DOCKER}" = "true" ]; then
        info "Skipping Docker (--no-docker). Checking network reachability..."
        wait_for_rpc 10
        return 0
    fi

    if docker inspect "${NETWORK_CONTAINER}" >/dev/null 2>&1; then
        container_running="$(docker inspect -f '{{.State.Running}}' "${NETWORK_CONTAINER}" 2>/dev/null)"
        if [ "${container_running}" = "true" ]; then
            info "Docker container '${NETWORK_CONTAINER}' already running."
            wait_for_rpc 30
            return 0
        else
            info "Container '${NETWORK_CONTAINER}' exists but is stopped. Starting..."
            run docker start "${NETWORK_CONTAINER}"
            wait_for_rpc 30
            return 0
        fi
    fi

    info "Starting Stellar quickstart container (detached)..."
    run docker run -d \
        --name "${NETWORK_CONTAINER}" \
        -p 8000:8000 \
        stellar/quickstart:testing \
        --standalone \
        --enable-soroban

    # Track that we started it so cleanup trap stops it
    CLEANUP_CONTAINER=true

    wait_for_rpc 60
    ok "Local Stellar network ready at ${RPC_URL}"
}

wait_for_rpc() {
    max_seconds="${1:-30}"
    max_tries=$((max_seconds / 2))
    info "Waiting for RPC at ${RPC_URL} (up to ${max_seconds}s)..."
    i=0
    while [ "${i}" -lt "${max_tries}" ]; do
        if curl -sf "${RPC_URL}" >/dev/null 2>&1; then
            ok "RPC is ready."
            return 0
        fi
        sleep 2
        i=$((i + 1))
    done
    err "RPC at ${RPC_URL} not available after ${max_seconds}s."
    err "Check: docker logs ${NETWORK_CONTAINER}"
    exit 1
}

# =============================================================================
# STEP 3 — Configure CLI network
# =============================================================================
configure_network() {
    network_exists=0
    "${CLI}" network ls 2>/dev/null | grep -qF "${NETWORK_NAME}" && network_exists=1

    if [ "${network_exists}" = "1" ]; then
        info "Network '${NETWORK_NAME}' already configured. Skipping."
        return 0
    fi

    info "Adding network '${NETWORK_NAME}'..."
    if [ "${CLI_OLD_STYLE}" = "1" ]; then
        run "${CLI}" network add \
            --name "${NETWORK_NAME}" \
            --rpc-url "${RPC_URL}" \
            --network-passphrase "${NETWORK_PASSPHRASE}"
    else
        run "${CLI}" network add \
            "${NETWORK_NAME}" \
            --rpc-url "${RPC_URL}" \
            --network-passphrase "${NETWORK_PASSPHRASE}"
    fi
    ok "Network '${NETWORK_NAME}' configured."
}

# =============================================================================
# STEP 4 — Create and fund identities
# =============================================================================
ensure_identity() {
    label="$1"
    ident_exists=0
    "${CLI}" keys ls 2>/dev/null | grep -qF "${label}" && ident_exists=1

    if [ "${ident_exists}" = "1" ]; then
        info "Identity '${label}' already exists."
    else
        info "Generating identity '${label}'..."
        run "${CLI}" keys generate "${label}"
        ok "Identity '${label}' created."
    fi

    addr="$("${CLI}" keys address "${label}" 2>/dev/null || true)"
    if [ -z "${addr}" ]; then
        err "Failed to retrieve address for '${label}'."
        exit 1
    fi
    echo "${addr}"
}

fund_identity() {
    addr="$1"
    # Derive friendbot URL from RPC_URL
    case "${RPC_URL}" in
        */soroban/rpc)
            FRIENDBOT_URL="${RPC_URL%/soroban/rpc}/friendbot"
            ;;
        *)
            FRIENDBOT_URL="http://localhost:8000/friendbot"
            ;;
    esac

    friendbot_output=""
    friendbot_output="$(curl -sf "${FRIENDBOT_URL}?addr=${addr}" 2>&1 || true)"
    if echo "${friendbot_output}" | grep -q '"hash"' 2>/dev/null; then
        ok "Funded ${addr}"
    else
        warn "Friendbot response (may already be funded) for ${addr}"
    fi
}

# =============================================================================
# STEP 5 — Deploy or reuse a test token
# =============================================================================
resolve_token() {
    if [ -n "${TOKEN_ADDR:-}" ]; then
        info "Using TOKEN_ADDR from environment: ${TOKEN_ADDR}"
        return 0
    fi

    if [ -f "${STATE_FILE}" ]; then
        . "${STATE_FILE}"
        if [ -n "${SAVED_TOKEN_ADDR:-}" ]; then
            TOKEN_ADDR="${SAVED_TOKEN_ADDR}"
            info "Reusing token from previous deploy: ${TOKEN_ADDR}"
            return 0
        fi
    fi

    info "Deploying test token (native XLM via Stellar Asset Contract)..."
    token_output=$("${CLI}" lab token wrap \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --asset "native" \
        2>&1 || true)

    TOKEN_ADDR=$(echo "${token_output}" | grep -o 'C[A-Z0-9]\{55\}' | head -1)

    if [ -z "${TOKEN_ADDR}" ]; then
        err "Failed to obtain test token contract address."
        err ""
        err "Diagnosis:"
        err "  1. Is the local network running and reachable?"
        err "  2. Does the admin identity have XLM balance?"
        err "  3. Was the token already wrapped? Set TOKEN_ADDR=<addr> to reuse."
        err ""
        err "  ${CLI} lab token wrap --network ${NETWORK_NAME} --source ${ADMIN_IDENTITY} --asset native --verbose"
        exit 2
    fi
    ok "Test token deployed at: ${TOKEN_ADDR}"
}

# =============================================================================
# STEP 6 — Deploy the subscription-vault contract
# =============================================================================
deploy_vault() {
    info "Deploying ${CONTRACT_NAME} contract..."

    # Install WASM to network
    info "Installing WASM..."
    WASM_HASH=$("${CLI}" contract install \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --wasm "${WASM_PATH}" \
        2>&1 || true)

    if [ -z "${WASM_HASH}" ]; then
        err "WASM install failed. Check network connectivity and admin key."
        exit 2
    fi
    ok "WASM installed: ${WASM_HASH}"

    # Deploy contract instance
    info "Deploying contract instance..."
    CONTRACT_ID=$("${CLI}" contract deploy \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --wasm-hash "${WASM_HASH}" \
        2>&1 || true)

    if [ -z "${CONTRACT_ID}" ]; then
        # Fallback: some CLI versions need --wasm directly
        CONTRACT_ID=$("${CLI}" contract deploy \
            --network "${NETWORK_NAME}" \
            --source "${ADMIN_IDENTITY}" \
            --wasm "${WASM_PATH}" \
            2>&1 || true)
    fi

    if [ -z "${CONTRACT_ID}" ]; then
        err "Contract deployment failed."
        exit 2
    fi

    # Persist both token and contract ID atomically
    printf "SAVED_TOKEN_ADDR='%s'\nSAVED_CONTRACT_ID='%s'\n" \
      "${TOKEN_ADDR}" "${CONTRACT_ID}" > "${STATE_FILE}" 2>/dev/null || true

    ok "Contract deployed at: ${CONTRACT_ID}"
}

# =============================================================================
# STEP 7 — Initialize the contract
# =============================================================================
init_contract() {
    info "Initializing contract..."
    info "  token=${TOKEN_ADDR}"
    info "  admin=${ADMIN_ADDR}"
    info "  min_topup=10000000"
    info "  grace_period=86400 (1 day)"

    INIT_RESULT=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        init \
        --token "${TOKEN_ADDR}" \
        --token_decimals "${TOKEN_DECIMALS}" \
        --admin "${ADMIN_ADDR}" \
        --min_topup 10000000 \
        --grace_period 86400 \
        2>&1) || {
        if echo "${INIT_RESULT}" | grep -qi "already\|AlreadyInitialized\|was already" 2>/dev/null; then
            warn "Contract already initialized (re-run). Skipping."
            return 0
        fi
        err "Init failed: ${INIT_RESULT}"
        exit 2
    }

    ok "Contract initialized successfully."
}

# =============================================================================
# STEP 8 — Verify deployment
# =============================================================================
verify_deployment() {
    step "Verifying deployment..."

    ADMIN_RESPONSE=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        get_admin \
        2>&1) || { warn "get_admin failed."; return; }

    ADMIN_GOT=$(echo "${ADMIN_RESPONSE}" | grep -o 'G[A-Z0-9]\{55\}' | head -1)
    if [ -n "${ADMIN_GOT}" ]; then
        if [ "${ADMIN_GOT}" = "${ADMIN_ADDR}" ]; then
            ok "Admin verified: ${ADMIN_GOT}"
        else
            warn "Admin mismatch: expected ${ADMIN_ADDR}, got ${ADMIN_GOT}"
        fi
    else
        info "Admin response: ${ADMIN_RESPONSE}"
    fi

    VERSION_RESPONSE=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        version \
        2>&1) || true

    if echo "${VERSION_RESPONSE}" | grep -q '^[0-9]' 2>/dev/null; then
        ok "Contract version: ${VERSION_RESPONSE}"
    fi
}

# =============================================================================
# STEP 9 — Full subscription lifecycle smoke test
# =============================================================================
smoke_test() {
    step "Smoke test: full subscription lifecycle..."

    # 9a. Fund subscriber with native tokens (best-effort)
    info "Minting test tokens to subscriber..."
    if "${CLI}" lab token mint \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --asset "native" \
        --amount "100000000000" \
        --to "${SUBSCRIBER_ADDR}" 2>/dev/null; then
        ok "Tokens minted to subscriber."
    else
        warn "Token mint not available on standalone. Friendbot may have funded XLM."
    fi

    # 9b. Check subscriber balance
    SUB_BALANCE=$("${CLI}" lab token balance \
        --network "${NETWORK_NAME}" \
        --asset "native" \
        --address "${SUBSCRIBER_ADDR}" 2>/dev/null || echo "unknown")
    info "Subscriber balance: ${SUB_BALANCE}"

    # 9c. Create subscription
    info "Creating subscription..."
    info "  subscriber=${SUBSCRIBER_ADDR}"
    info "  merchant=${MERCHANT_ADDR}"
    info "  amount=1000000 (0.01 token)"
    info "  interval=86400s (1 day)"

    SUB_CREATE_RESULT=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${SUBSCRIBER_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        create_subscription \
        --subscriber "${SUBSCRIBER_ADDR}" \
        --merchant "${MERCHANT_ADDR}" \
        --amount 1000000 \
        --interval_seconds 86400 \
        --usage_enabled false \
        --lifetime_cap 100000000 \
        --expires_at 9999999999 \
        2>&1 || true)

    SUB_ID=$(echo "${SUB_CREATE_RESULT}" | grep -oE '[0-9]+' | head -1)
    if [ -z "${SUB_ID}" ]; then
        warn "Subscription creation failed or returned unexpected: ${SUB_CREATE_RESULT}"
        warn "Skipping remaining smoke test steps."
        return
    fi
    ok "Subscription created with ID: ${SUB_ID}"

    # 9d. Deposit funds
    info "Depositing 50000000 tokens to subscription ${SUB_ID}..."
    DEPOSIT_RESULT=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${SUBSCRIBER_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        deposit_funds \
        --subscription_id "${SUB_ID}" \
        --subscriber "${SUBSCRIBER_ADDR}" \
        --amount 50000000 \
        2>&1 || true)

    if echo "${DEPOSIT_RESULT}" | grep -qi "error"; then
        warn "Deposit failed: ${DEPOSIT_RESULT}"
    else
        ok "Deposit succeeded."
    fi

    # 9e. Query subscription state
    QUERY_RESULT=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --id "${CONTRACT_ID}" \
        -- \
        get_subscription \
        --subscription_id "${SUB_ID}" \
        2>&1 || true)

    if echo "${QUERY_RESULT}" | grep -qi "error"; then
        warn "Query failed: ${QUERY_RESULT}"
    else
        ok "Subscription query succeeded."
    fi

    # 9f. Charge the subscription (admin-only)
    info "Charging subscription ${SUB_ID}..."
    CHARGE_RESULT=$("${CLI}" contract invoke \
        --network "${NETWORK_NAME}" \
        --source "${ADMIN_IDENTITY}" \
        --id "${CONTRACT_ID}" \
        -- \
        charge_subscription \
        --subscription_id "${SUB_ID}" \
        2>&1 || true)

    if echo "${CHARGE_RESULT}" | grep -qi "error"; then
        warn "Charge failed: ${CHARGE_RESULT}"
        warn "This may be expected: interval may not have elapsed yet, or insufficient balance."
    else
        ok "Charge succeeded."
    fi

    ok "Smoke test complete."
}

# =============================================================================
# SUMMARY
# =============================================================================
print_summary() {
    cat <<EOF

${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}
${BOLD}          Deployment Summary${NC}
${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}

  Contract ID:  ${BOLD}${CONTRACT_ID}${NC}
  Token:        ${TOKEN_ADDR}
  Admin:        ${ADMIN_ADDR}
  Subscriber:   ${SUBSCRIBER_ADDR}
  Merchant:     ${MERCHANT_ADDR}
  Network:      ${NETWORK_NAME}
  RPC URL:      ${RPC_URL}

  State saved:  ${STATE_FILE}

  Quick reference:
    ${CLI_BASENAME} contract invoke \\
      --network ${NETWORK_NAME} \\
      --source ${ADMIN_IDENTITY} \\
      --id ${CONTRACT_ID} \\
      -- \\
      version

    ${CLI_BASENAME} contract invoke \\
      --network ${NETWORK_NAME} \\
      --id ${CONTRACT_ID} \\
      -- \\
      get_admin

  To clean up:
    docker stop ${NETWORK_CONTAINER} && docker rm ${NETWORK_CONTAINER}
    ${CLI_BASENAME} keys rm ${ADMIN_IDENTITY}
    ${CLI_BASENAME} keys rm ${SUBSCRIBER_IDENTITY}
    ${CLI_BASENAME} keys rm ${MERCHANT_IDENTITY}
    ${CLI_BASENAME} network rm ${NETWORK_NAME}
    rm -f ${STATE_FILE}

${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}
EOF
}

# =============================================================================
# MAIN
# =============================================================================
main() {
    # Parse arguments
    for arg in "$@"; do
        case "${arg}" in
            --help|-h) usage ;;
            --no-docker) NO_DOCKER=true ;;
            --skip-smoke) SKIP_SMOKE=true ;;
            --*) warn "Unknown option: ${arg}" ;;
        esac
    done

    # ── Banner ──
    cat <<'EOF'
╔══════════════════════════════════════════════════════════════╗
║  Stellabill — Local Soroban Deploy Script                    ║
║  subscription_vault contract                                 ║
╚══════════════════════════════════════════════════════════════╝
EOF

    # ── Step 0: Dependency checks ──
    step "0/9 — Checking dependencies"
    detect_cli
    check_rust
    check_curl
    if [ "${NO_DOCKER}" = "false" ]; then
        check_docker
    else
        info "Skipping Docker check (--no-docker)."
    fi

    # ── Step 1: Build ──
    step "1/9 — Building contract WASM"
    build_contract

    # ── Step 2: Network ──
    step "2/9 — Starting local network"
    ensure_network

    # ── Step 3: CLI network config ──
    step "3/9 — Configuring CLI network"
    configure_network

    # ── Step 4: Identities ──
    step "4/9 — Creating identities"
    ADMIN_ADDR="$(ensure_identity "${ADMIN_IDENTITY}")"
    SUBSCRIBER_ADDR="$(ensure_identity "${SUBSCRIBER_IDENTITY}")"
    MERCHANT_ADDR="$(ensure_identity "${MERCHANT_IDENTITY}")"
    ok "Admin:      ${ADMIN_ADDR}"
    ok "Subscriber: ${SUBSCRIBER_ADDR}"
    ok "Merchant:   ${MERCHANT_ADDR}"

    # ── Step 5: Fund ──
    step "5/9 — Funding identities"
    fund_identity "${ADMIN_ADDR}"
    fund_identity "${SUBSCRIBER_ADDR}"
    fund_identity "${MERCHANT_ADDR}"

    # ── Step 6: Token ──
    step "6/9 — Resolving test token"
    resolve_token

    # ── Step 7: Deploy contract ──
    step "7/9 — Deploying subscription-vault contract"
    deploy_vault

    # ── Step 8: Init + verify ──
    step "8/9 — Initializing and verifying contract"
    init_contract
    verify_deployment

    # ── Step 9: Smoke test ──
    if [ "${SKIP_SMOKE}" = "true" ]; then
        step "9/9 — Smoke test skipped (--skip-smoke)"
    else
        step "9/9 — Running full subscription lifecycle smoke test"
        smoke_test
    fi

    # ── Done ──
    print_summary
    ok "All done."
}

main "$@"
