#!/usr/bin/env bash
# Run the GitHub Actions CI locally with `act` (https://github.com/nektos/act)
# while the org's hosted Actions are unavailable.
#
# The private secure-log git dependency needs a token; we read it from the
# `gh` CLI at run time so it is never written to disk or committed.
#
# Usage:
#   scripts/ci-local.sh                 # run all jobs on the default (push) event
#   scripts/ci-local.sh -j rust-checks  # run a single job
#   scripts/ci-local.sh -n              # dry run (list steps without executing)
#
# Requires: act, Docker (running), and `gh auth login` (token with repo scope).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

if ! command -v act >/dev/null 2>&1; then
    echo "error: act not found. Install with: brew install act" >&2
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    echo "error: Docker daemon not running." >&2
    exit 1
fi

TOKEN="$(gh auth token)"
if [[ -z "$TOKEN" ]]; then
    echo "error: no gh token. Run: gh auth login" >&2
    exit 1
fi

# Pass the token as the workflow secret AND as GITHUB_TOKEN so any
# token-authenticated step works. Token is passed in-process, not on disk.
exec act "$@" \
    -s "TEGMENTUM_CI_TOKEN=$TOKEN" \
    -s "GITHUB_TOKEN=$TOKEN"
