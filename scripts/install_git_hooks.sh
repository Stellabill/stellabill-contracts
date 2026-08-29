#!/usr/bin/env bash
# scripts/install_git_hooks.sh
#
# Idempotent installer for the stellabill-contracts Git hooks.
#
# Usage:
#   ./scripts/install_git_hooks.sh          # install / re-install
#   ./scripts/install_git_hooks.sh --check  # verify without modifying
#   ./scripts/install_git_hooks.sh --help   # show this message
#
# What it does:
#   1. Verifies you are inside the correct Git repository.
#   2. Confirms that .githooks/pre-commit exists and is a valid bash script.
#   3. Sets the executable bit on every hook in .githooks/.
#   4. Runs `git config core.hooksPath .githooks` (idempotent — safe to re-run).
#   5. Smoke-tests the hook with SKIP_PRE_COMMIT=1 to confirm it exits 0.
#
# Idempotency: running the script multiple times produces the same result.
# A second run simply confirms the configuration is already correct.

set -euo pipefail

# ── Argument handling ────────────────────────────────────────────────────────
CHECK_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --check)  CHECK_ONLY=1 ;;
        --help|-h)
            sed -n '2,/^set /{ /^set /d; s/^# \{0,1\}//; p }' "$0"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            echo "Usage: $0 [--check] [--help]" >&2
            exit 1
            ;;
    esac
done

# ── Locate repo root ─────────────────────────────────────────────────────────
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "Error: not inside a Git repository." >&2
    exit 1
}
cd "$REPO_ROOT"

HOOKS_SOURCE_DIR=".githooks"
PRE_COMMIT="$HOOKS_SOURCE_DIR/pre-commit"

# ── Validate the hook source file ────────────────────────────────────────────
if [ ! -f "$PRE_COMMIT" ]; then
    echo "Error: $PRE_COMMIT not found in the repository." >&2
    echo "       The hooks directory is checked into version control." >&2
    echo "       Make sure you have the latest code: git pull" >&2
    exit 1
fi

# Confirm the file starts with a bash shebang (basic sanity check).
SHEBANG=$(head -1 "$PRE_COMMIT")
if [[ "$SHEBANG" != "#!/"* ]]; then
    echo "Error: $PRE_COMMIT does not look like a valid script (no shebang)." >&2
    exit 1
fi

# ── Check-only mode: report status without modifying anything ────────────────
if [ "$CHECK_ONLY" -eq 1 ]; then
    echo "Checking Git hooks configuration..."
    CURRENT=$(git config --get core.hooksPath 2>/dev/null || true)
    if [ "$CURRENT" = "$HOOKS_SOURCE_DIR" ]; then
        echo "✅ core.hooksPath is already set to '$HOOKS_SOURCE_DIR'"
    else
        echo "❌ core.hooksPath is '${CURRENT:-<not set>}', expected '$HOOKS_SOURCE_DIR'"
        echo "   Run \`./scripts/install_git_hooks.sh\` to install."
        exit 1
    fi
    if [ -x "$PRE_COMMIT" ]; then
        echo "✅ $PRE_COMMIT is executable"
    else
        echo "❌ $PRE_COMMIT is not executable"
        echo "   Run \`./scripts/install_git_hooks.sh\` to fix."
        exit 1
    fi
    echo "✅ Hooks look correctly installed."
    exit 0
fi

# ── Install ──────────────────────────────────────────────────────────────────
echo "Installing Git hooks from $HOOKS_SOURCE_DIR/ ..."

# Make every hook in the source directory executable.
HOOK_COUNT=0
while IFS= read -r -d '' hook; do
    chmod +x "$hook"
    HOOK_COUNT=$((HOOK_COUNT + 1))
    echo "  chmod +x $hook"
done < <(find "$HOOKS_SOURCE_DIR" -maxdepth 1 -type f -print0)

if [ "$HOOK_COUNT" -eq 0 ]; then
    echo "Warning: no hook files found in $HOOKS_SOURCE_DIR/." >&2
fi

# Point Git at the hooks directory.
CURRENT=$(git config --get core.hooksPath 2>/dev/null || true)
if [ "$CURRENT" = "$HOOKS_SOURCE_DIR" ]; then
    echo "  core.hooksPath already set to '$HOOKS_SOURCE_DIR' — no change needed."
else
    git config core.hooksPath "$HOOKS_SOURCE_DIR"
    echo "  git config core.hooksPath $HOOKS_SOURCE_DIR"
fi

# ── Smoke-test ───────────────────────────────────────────────────────────────
echo "Smoke-testing pre-commit hook (SKIP_PRE_COMMIT=1) ..."
if SKIP_PRE_COMMIT=1 bash "$PRE_COMMIT"; then
    echo "  ✅ Smoke test passed (hook exits 0 when bypass is set)."
else
    echo "  ❌ Smoke test failed — hook exited non-zero even with SKIP_PRE_COMMIT=1." >&2
    exit 1
fi

echo ""
echo "✅ Git hooks installed successfully."
echo ""
echo "The pre-commit hook will now run automatically on every \`git commit\`."
echo "It checks:"
echo "  • cargo fmt --check  (staged .rs files only)"
echo "  • cargo clippy --all-targets -- -D warnings"
echo ""
echo "To bypass when needed:   git commit --no-verify"
echo "To skip via env-var:     SKIP_PRE_COMMIT=1 git commit  (logs a warning)"
echo "To verify installation:  ./scripts/install_git_hooks.sh --check"
