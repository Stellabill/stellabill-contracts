#!/usr/bin/env bash
# Helper script to install git hooks for stellabill-contracts repository.
# Idempotent: safe to run multiple times.

set -euo pipefail

# Navigate to the repository root directory
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null)
if [ -z "$REPO_ROOT" ]; then
    echo "Error: Not inside a git repository." >&2
    exit 1
fi

cd "$REPO_ROOT"

HOOKS_DIR=".githooks"
PRE_COMMIT_HOOK="$HOOKS_DIR/pre-commit"

if [ ! -f "$PRE_COMMIT_HOOK" ]; then
    echo "Error: Pre-commit hook script ($PRE_COMMIT_HOOK) not found." >&2
    exit 1
fi

echo "Setting permissions on $PRE_COMMIT_HOOK..."
chmod +x "$PRE_COMMIT_HOOK"

echo "Configuring git core.hooksPath to $HOOKS_DIR..."
git config core.hooksPath "$HOOKS_DIR"

# Verify configuration
CURRENT_HOOKS_PATH=$(git config --get core.hooksPath || true)
if [ "$CURRENT_HOOKS_PATH" = "$HOOKS_DIR" ]; then
    echo "✅ Git hooks installed successfully! (core.hooksPath set to '$HOOKS_DIR')"
else
    echo "⚠️ Warning: Failed to confirm git core.hooksPath setting." >&2
fi
