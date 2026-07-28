#!/usr/bin/env bash
# ============================================================================
# deploy_local.sh — One-command local Soroban testnet setup
#
# Builds the subscription vault contract, deploys it to a local Soroban
# network (via Docker quickstart image), and runs a smoke-test init.
#
# Usage:
#   ./scripts/deploy_local.sh                    # full flow (build + deploy + init + smoke)
#   ./scripts/deploy_local.sh --skip-build       # skip the cargo/soroban build step
#   ./scripts/deploy_local.sh --skip-smoke       # skip the smoke-test at the end
#   ./scripts/deploy_local.sh --network standalone  # use an already-running network
#   ./scripts/deploy_local.sh --help             # print full help
#
# Requirements:
#   - Rust toolchain (rustc, cargo)
#   - Soroban CLI (binary: `soroban` or `stellar`)
#   - Docker (for local network container)
#   - curl (for network health checks)
#
# Environment variables (optional overrides):
#   SOROBAN_CLI       — path / name of the soroban/stellar CLI binary
#   CONTRACT_NAME     — package name in Cargo.toml  (default: subscription_vault)
#   ADMIN_SECRET      — secret key for admin identity (default: generates fresh)
#   NETWORK           — network name in soroban CLI  (default: local_dev)
#   RPC_URL           — Soroban RPC endpoint          (default: http://localhost:8001/soroban/rpc)
#   PASS_PHRASE       — network passphrase            (default: "Standalone Network ; February 2017")
#   TOKEN_ADDR        — pre-wrapped token contract ID (default: wraps native asset)
# ============================================================================

set -euo pipefail

# ── Script metadata ───────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CONTRACT_NAME="${CONTRACT_NAME:-subscription_vault}"
NETWORK="${NETWORK:-local_dev}"
RPC_URL="${RPC_URL:-http://localhost:8001/soroban/rpc}"
PASS_PHRASE="${PASS_PHRASE:-Standalone Network ; February 2017}"

# WASM build output path  (reused across steps)
WASM_TARGET="wasm32-unknown-unknown"
WASM_OUTPUT="$PROJECT_ROOT/target/$WASM_TARGET/release/$CONTRACT_NAME.wasm"

# Default identities created by the script
ADMIN_KEY="${ADMIN_SECRET:-}"

# Flags
SKIP_BUILD=false
SKIP_SMOKE=false

# Track whether we started the Docker container (for cleanup)
CLEANUP_CONTAINER=false
NETWORK_CONTAINER="stellabill-local-soroban"

# ── Color output (POSIX-safe: use tput if available, else raw ANSI) ───────────
if [[ -t 1 ]]; then
    RED=$(tput setaf 1 2>/dev/null || printf '\033[31m')
    GREEN=$(tput setaf 2 2>/dev/null || printf '\033[32m')
    YELLOW=$(tput setaf 3 2>/dev/null || printf '\033[33m')
    BLUE=$(tput setaf 4 2>/dev/null || printf '\033[34m')
    BOLD=$(tput bold 2>/dev/null || printf '\033[1m')
    NC=$(tput sgr0 2>/dev/null || printf '\033[0m')
else
    RED=''; GREEN=''; YELLOW=''; BLUE=''; BOLD=''; NC=''
fi

# ── Helper functions ──────────────────────────────────────────────────────────
info()  { printf '%s[INFO]%s %s\n' "$BLUE" "$NC" "$*" >&2; }
ok()    { printf '%s[ OK ]%s %s\n' "$GREEN" "$NC" "$*" >&2; }
warn()  { printf '%s[WARN]%s %s\n' "$YELLOW" "$NC" "$*" >&2; }
die()   { printf '%s[ERR ]%s %s\n' "$RED" "$NC" "$*" >&2; exit 1; }

# Print every command before executing it (transparency)
run_cmd() {
    printf '%s[CMD]%s' "$YELLOW" "$NC" >&2
    printf ' %s' "$@" >&2
    printf '\n' >&2
    "$@"
}

# Cleanup handler – stops the Docker container only if we started it
cleanup() {
    if [[ "$CLEANUP_CONTAINER" == "true" ]]; then
        info "Cleaning up container '$NETWORK_CONTAINER'..."
        docker stop "$NETWORK_CONTAINER" 2>/dev/null || true
        docker rm "$NETWORK_CONTAINER" 2>/dev/null || true
        ok "Container stopped and removed"
    fi
}
trap cleanup EXIT INT TERM

# ── Detect CLI binary (supports both `soroban` and `stellar`) ─────────────────
find_cli() {
    if [[ -n "${SOROBAN_CLI:-}" ]]; then
        if command -v "$SOROBAN_CLI" &>/dev/null; then
            echo "$SOROBAN_CLI"
            return 0
        fi
        die "SOROBAN_CLI set to '$SOROBAN_CLI' but binary not found in PATH"
    fi
    if command -v soroban &>/dev/null; then
        echo "soroban"
        return 0
    fi
    if command -v stellar &>/dev/null; then
        echo "stellar"
        return 0
    fi
    return 1
}

# ── Usage ─────────────────────────────────────────────────────────────────────
usage() {
    cat >&2 <<EOF
Usage: $0 [OPTIONS]

Options:
  --skip-build       Skip the WASM build step (reuse existing target/)
  --skip-smoke       Skip the smoke-test after init
  --network NAME     Soroban CLI network name  (default: $NETWORK)
  --rpc-url URL      RPC endpoint              (default: $RPC_URL)
  --help             Print this help message

Environment variables:
  SOROBAN_CLI       Binary name/path for the soroban/stellar CLI
  CONTRACT_NAME     Cargo package name          (default: $CONTRACT_NAME)
  ADMIN_SECRET      Secret key for admin identity (generated if empty)
  NETWORK           Network alias               (default: $NETWORK)
  RPC_URL           RPC endpoint                (default: $RPC_URL)
  PASS_PHRASE       Network passphrase          (default: Standalone Network)
  TOKEN_ADDR        Pre-wrapped token contract ID  (optional)

Tip: set ADMIN_SECRET to a persistent key to re-deploy without losing
     the admin identity across runs.
EOF
    exit 0
}

# ── Parse arguments ───────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)   SKIP_BUILD=true; shift ;;
        --skip-smoke)   SKIP_SMOKE=true; shift ;;
        --network)      NETWORK="$2";  shift 2 ;;
        --rpc-url)      RPC_URL="$2";  shift 2 ;;
        --help|-h)      usage ;;
        *)              warn "Ignoring unknown argument: $1"; shift ;;
    esac
done

# ── Step 0: Check dependencies ────────────────────────────────────────────────
info "=== Step 0: Checking dependencies ==="

CLI="$(find_cli)" || die "Soroban CLI not found. Install from: https://github.com/stellar/stellar-cli"
ok "Soroban CLI: $(command -v "$CLI")"

if ! command -v cargo &>/dev/null; then
    die "Rust/Cargo not found. Install from: https://rustup.rs"
fi
ok "Cargo: $(cargo --version | head -1)"

if ! command -v docker &>/dev/null; then
    die "Docker not found. Install from: https://docs.docker.com/get-docker/"
fi
ok "Docker: $(docker --version | head -1)"

if ! command -v curl &>/dev/null; then
    die "curl not found. Install from: https://curl.se/"
fi
ok "curl available"

# ── Step 1: Build contract WASM ───────────────────────────────────────────────
if [[ "$SKIP_BUILD" == "true" ]]; then
    info "=== Step 1: Skipping build (--skip-build) ==="
else
    info "=== Step 1: Building contract WASM ==="

    if [[ -f "$WASM_OUTPUT" ]]; then
        warn "Existing WASM at $WASM_OUTPUT — rebuilding..."
    fi

    # Prefer soroban/stellar CLI build; fall back to cargo + wasm target.
    # Note: '|| true' prevents set -e from aborting if CLI build is unavailable.
    if "$CLI" contract build 2>/dev/null; then
        ok "Build succeeded via '$CLI contract build'"
    else
        warn "'$CLI contract build' failed, falling back to cargo build."
        run_cmd rustup target add "$WASM_TARGET"
        run_cmd cargo build -p "$CONTRACT_NAME" --target "$WASM_TARGET" --release
    fi

    if [[ ! -f "$WASM_OUTPUT" ]]; then
        die "WASM not found at $WASM_OUTPUT after build"
    fi
    ok "WASM built: $WASM_OUTPUT ($(du -h "$WASM_OUTPUT" | cut -f1))"
fi

# ── Step 2: Start local Soroban network ───────────────────────────────────────
info "=== Step 2: Starting local Soroban network ==="

if docker inspect "$NETWORK_CONTAINER" &>/dev/null 2>&1; then
    ok "Container '$NETWORK_CONTAINER' already exists"
    if [[ "$(docker inspect -f '{{.State.Running}}' "$NETWORK_CONTAINER")" != "true" ]]; then
        info "Container exists but is not running — starting it..."
        run_cmd docker start "$NETWORK_CONTAINER"
    else
        info "Container already running"
    fi
else
    info "Starting Soroban quickstart container (this may take a moment)..."
    run_cmd docker run -d \
        --name "$NETWORK_CONTAINER" \
        -p 8000:8000 \
        -p 8001:8001 \
        -p 11625:11625 \
        -p 11626:11626 \
        stellar/quickstart:soroban-latest \
        --standalone \
        --enable-soroban-rpc

    CLEANUP_CONTAINER=true
    ok "Container starting — waiting for Soroban RPC to become available..."
fi

# Wait for RPC to be ready
MAX_RETRIES=30
RETRY_INTERVAL=3
RPC_READY=false
for _ in $(seq 1 "$MAX_RETRIES"); do
    if curl -sf -X POST -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
        "$RPC_URL" >/dev/null 2>&1; then
        RPC_READY=true
        break
    fi
    sleep "$RETRY_INTERVAL"
done

if [[ "$RPC_READY" != "true" ]]; then
    die "Soroban RPC at $RPC_URL not available after ${MAX_RETRIES}x${RETRY_INTERVAL}s. Check: docker logs $NETWORK_CONTAINER"
fi
ok "Soroban RPC is ready at $RPC_URL"

# ── Step 3: Configure Soroban CLI network ─────────────────────────────────────
info "=== Step 3: Configuring Soroban CLI network ==="

# Add/update network in soroban CLI config; ignore error if it already exists
if "$CLI" network add \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$PASS_PHRASE" \
    "$NETWORK" 2>/dev/null; then
    ok "Network '$NETWORK' configured"
else
    warn "Network '$NETWORK' may already exist — continuing"
fi

# ── Step 4: Generate identities ───────────────────────────────────────────────
info "=== Step 4: Setting up identities ==="

# Helpers: generate a key identity if it doesn't already exist
ensure_identity() {
    local label="$1"
    if ! "$CLI" keys address "$label" --network "$NETWORK" &>/dev/null; then
        "$CLI" keys generate "$label" --network "$NETWORK" || \
            die "Failed to generate key '$label'"
    fi
}

# Admin identity
if [[ -n "$ADMIN_KEY" ]]; then
    info "Using provided ADMIN_SECRET"
    if ! "$CLI" keys add "admin_$NETWORK" --secret-key "$ADMIN_KEY" --network "$NETWORK" 2>/dev/null; then
        warn "Could not import admin key (may already exist)"
    fi
else
    info "Generating fresh admin identity..."
    ensure_identity "admin_$NETWORK"
fi

ADMIN_ADDR=$("$CLI" keys address "admin_$NETWORK" --network "$NETWORK")
ok "Admin address: $ADMIN_ADDR"

# Subscriber identity
ensure_identity "subscriber_$NETWORK"
SUBSCRIBER_ADDR=$("$CLI" keys address "subscriber_$NETWORK" --network "$NETWORK")
ok "Subscriber address: $SUBSCRIBER_ADDR"

# Merchant identity
ensure_identity "merchant_$NETWORK"
MERCHANT_ADDR=$("$CLI" keys address "merchant_$NETWORK" --network "$NETWORK")
ok "Merchant address: $MERCHANT_ADDR"

# ── Step 5: Fund accounts ─────────────────────────────────────────────────────
info "=== Step 5: Funding accounts ==="

for label in "admin_$NETWORK" "subscriber_$NETWORK" "merchant_$NETWORK"; do
    addr=$("$CLI" keys address "$label" --network "$NETWORK" 2>/dev/null || true)
    if [[ -z "$addr" ]]; then
        continue
    fi

    if "$CLI" account fund \
            --network "$NETWORK" \
            --address "$addr" 2>/dev/null; then
        ok "Funded $label ($addr)"
    else
        warn "Could not fund $label (may already be funded or root key unavailable)"
    fi
done

# ── Step 6: Wrap native asset as test token ────────────────────────────────────
info "=== Step 6: Deploying test token ==="

if [[ -z "${TOKEN_ADDR:-}" ]]; then
    info "Wrapping native asset to use as test token..."

    # Try the modern `lab token wrap` syntax, fall back to the older
    # `--source-account` flag if the first variant fails.
    TOKEN_ADDR=$("$CLI" lab token wrap \
        --network "$NETWORK" \
        --source "admin_$NETWORK" \
        --asset "native" 2>/dev/null || \
        "$CLI" lab token wrap \
            --network "$NETWORK" \
            --source-account "admin_$NETWORK" \
            --asset "native" 2>/dev/null || true)

    if [[ -z "$TOKEN_ADDR" ]]; then
        die "Could not wrap native asset. Set TOKEN_ADDR env var to a pre-deployed token contract ID."
    fi
fi
ok "Token address: $TOKEN_ADDR"

# ── Step 7: Deploy the subscription vault contract ───────────────────────────
info "=== Step 7: Deploying subscription vault contract ==="

if [[ ! -f "$WASM_OUTPUT" ]]; then
    die "WASM file not found at $WASM_OUTPUT — run build first (or pass --skip-build if already built)"
fi

info "Installing WASM to network..."
WASM_HASH=$("$CLI" contract install \
    --network "$NETWORK" \
    --source "admin_$NETWORK" \
    --wasm "$WASM_OUTPUT" 2>/dev/null || true)

if [[ -z "$WASM_HASH" ]]; then
    die "WASM install failed. Check network connectivity and admin key."
fi
ok "WASM installed with hash: $WASM_HASH"

info "Deploying contract instance..."
CONTRACT_ID=$("$CLI" contract deploy \
    --network "$NETWORK" \
    --source "admin_$NETWORK" \
    --wasm-hash "$WASM_HASH" 2>/dev/null || true)

if [[ -z "$CONTRACT_ID" ]]; then
    # Fallback: some CLI versions require `--wasm` directly instead of hash
    CONTRACT_ID=$("$CLI" contract deploy \
        --network "$NETWORK" \
        --source "admin_$NETWORK" \
        --wasm "$WASM_OUTPUT" 2>/dev/null || true)
fi

if [[ -z "$CONTRACT_ID" ]]; then
    die "Contract deployment failed. Check logs above."
fi
ok "Contract deployed at: $CONTRACT_ID"

# Save contract ID for future reference
echo "$CONTRACT_ID" > "$PROJECT_ROOT/.contract_id"
ok "Contract ID saved to .contract_id"

# ── Step 8: Initialize the contract ───────────────────────────────────────────
info "=== Step 8: Initializing contract ==="

# init(env, token, token_decimals, admin, min_topup, grace_period)
# Stellar native asset has 7 decimals, same as USDC.
TOKEN_DECIMALS=7
MIN_TOPUP=1000000     # 0.1 XLM (in stroop-like units for 7-decimal asset)
GRACE_PERIOD=259200   # 3 days in seconds

info "Calling init with: token=$TOKEN_ADDR, admin=$ADMIN_ADDR, min_topup=$MIN_TOPUP, grace=$GRACE_PERIOD"
INIT_RESULT=$("$CLI" contract invoke \
    --network "$NETWORK" \
    --source "admin_$NETWORK" \
    --id "$CONTRACT_ID" \
    -- \
    init \
    --token "$TOKEN_ADDR" \
    --token_decimals "$TOKEN_DECIMALS" \
    --admin "$ADMIN_ADDR" \
    --min_topup "$MIN_TOPUP" \
    --grace_period "$GRACE_PERIOD" 2>&1 || true)

if echo "$INIT_RESULT" | grep -qi "AlreadyInitialized"; then
    warn "Contract already initialized (idempotent) — skipping"
elif echo "$INIT_RESULT" | grep -qi "error"; then
    die "Init failed: $INIT_RESULT"
else
    ok "Contract initialized successfully"
fi

# ── Step 9: Verify deployment ─────────────────────────────────────────────────
info "=== Step 9: Verifying deployment ==="

VERIFIED_ADMIN=$("$CLI" contract invoke \
    --network "$NETWORK" \
    --id "$CONTRACT_ID" \
    -- \
    get_admin 2>/dev/null || echo "query_failed")

if [[ "$VERIFIED_ADMIN" == "query_failed" ]]; then
    warn "Could not verify deployment via get_admin(). Container may need more time."
else
    ok "Admin verified: $VERIFIED_ADMIN"
fi

# ── Step 10: Smoke test ───────────────────────────────────────────────────────
if [[ "$SKIP_SMOKE" == "true" ]]; then
    info "=== Step 10: Smoke test skipped (--skip-smoke) ==="
else
    info "=== Step 10: Running smoke test ==="

    # 10a. Fund subscriber with test tokens (best-effort; standalone may not allow mint)
    info "  Minting test tokens to subscriber..."
    if "$CLI" lab token mint \
        --network "$NETWORK" \
        --source "admin_$NETWORK" \
        --asset "native" \
        --amount "10000000000" \
        --to "$SUBSCRIBER_ADDR" 2>/dev/null; then
        ok "  Tokens minted to subscriber"
    else
        warn "  Could not mint tokens (may not be possible on standalone net)"
    fi

    # Check balance
    SUB_BALANCE=$("$CLI" lab token balance \
        --network "$NETWORK" \
        --asset "native" \
        --address "$SUBSCRIBER_ADDR" 2>/dev/null || echo "0")
    info "  Subscriber token balance: $SUB_BALANCE"

    # 10b. Create a subscription
    info "  Creating subscription (subscriber=$SUBSCRIBER_ADDR, merchant=$MERCHANT_ADDR, amount=1000000)..."
    SUB_ID=$("$CLI" contract invoke \
        --network "$NETWORK" \
        --source "subscriber_$NETWORK" \
        --id "$CONTRACT_ID" \
        -- \
        create_subscription \
        --subscriber "$SUBSCRIBER_ADDR" \
        --merchant "$MERCHANT_ADDR" \
        --amount 1000000 \
        --interval_seconds 86400 \
        --usage_enabled false \
        --lifetime_cap 100000000 \
        --expires_at 9999999999 2>&1 || true)

    if echo "$SUB_ID" | grep -qi "error"; then
        warn "  Could not create subscription: $SUB_ID"
        warn "  (Smoke test continues despite this failure)"
    else
        # Extract subscription ID from output (format depends on CLI version)
        SUB_ID_CLEAN=$(echo "$SUB_ID" | grep -oE '[0-9]+' | head -1 || echo "1")
        ok "  Subscription created with ID: $SUB_ID_CLEAN"

        # 10c. Deposit funds
        info "  Depositing 50000000 tokens to subscription $SUB_ID_CLEAN..."
        DEPOSIT_RESULT=$("$CLI" contract invoke \
            --network "$NETWORK" \
            --source "subscriber_$NETWORK" \
            --id "$CONTRACT_ID" \
            -- \
            deposit_funds \
            --subscription_id "$SUB_ID_CLEAN" \
            --subscriber "$SUBSCRIBER_ADDR" \
            --amount 50000000 2>&1 || true)

        if echo "$DEPOSIT_RESULT" | grep -qi "error"; then
            warn "  Deposit failed: $DEPOSIT_RESULT"
        else
            ok "  Deposit succeeded"
        fi

        # 10d. Query subscription state
        info "  Querying subscription state..."
        if "$CLI" contract invoke \
            --network "$NETWORK" \
            --id "$CONTRACT_ID" \
            -- \
            get_subscription \
            --subscription_id "$SUB_ID_CLEAN" >/dev/null 2>&1; then
            ok "  Subscription query succeeded"
        else
            warn "  Query failed"
        fi

        # 10e. Charge the subscription (admin)
        info "  Charging subscription $SUB_ID_CLEAN (via admin)..."
        CHARGE_RESULT=$("$CLI" contract invoke \
            --network "$NETWORK" \
            --source "admin_$NETWORK" \
            --id "$CONTRACT_ID" \
            -- \
            charge_subscription \
            --subscription_id "$SUB_ID_CLEAN" 2>&1 || true)

        if echo "$CHARGE_RESULT" | grep -qi "error"; then
            warn "  Charge failed: $CHARGE_RESULT"
        else
            ok "  Charge succeeded"
        fi
    fi
    ok "=== Smoke test complete ==="
fi

# ── Summary ───────────────────────────────────────────────────────────────────
{
    printf '\n'
    printf '%s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$GREEN" "$NC"
    printf '%s          Deployment Summary%s\n' "$BOLD" "$NC"
    printf '%s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$GREEN" "$NC"
    printf '  Contract ID: %s%s%s\n' "$BOLD" "$CONTRACT_ID" "$NC"
    printf '  Token:        %s\n' "$TOKEN_ADDR"
    printf '  Admin:        %s\n' "$ADMIN_ADDR"
    printf '  Subscriber:   %s\n' "$SUBSCRIBER_ADDR"
    printf '  Merchant:     %s\n' "$MERCHANT_ADDR"
    printf '  Network:      %s\n' "$NETWORK"
    printf '  RPC URL:      %s\n' "$RPC_URL"
    printf '\n'
    printf '  Config saved to: %s\n' "$PROJECT_ROOT/.contract_id"
    printf '  To interact:\n'
    printf '    %s contract invoke --network %s --id %s -- <fn> <args>\n' "$CLI" "$NETWORK" "$CONTRACT_ID"
    printf '\n'
    printf '  To stop the local network:\n'
    printf '    docker stop %s && docker rm %s\n' "$NETWORK_CONTAINER" "$NETWORK_CONTAINER"
    printf '%s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$GREEN" "$NC"
} >&2
