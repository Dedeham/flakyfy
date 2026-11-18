#!/usr/bin/env bash
# The fast checks from CI (.github/workflows/ci.yml), run here before a push so a red CI run is
# rare and nobody waits 20 minutes to learn about a clippy warning. The Rust server, the
# dashboards and the shell scripts; sonic, cargo-deny and the Python images stay with CI.
#   scripts/check.sh            everything below
#   scripts/check.sh --quick    fmt, clippy, dashboards and shellcheck, no Rust tests
# As the pre-push hook: ln -s ../../scripts/check.sh .git/hooks/pre-push
# (git push --no-verify skips it when you mean to).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

step() { printf '\n== %s\n' "$*"; }

step "cargo fmt"
cargo fmt --all --check

step "cargo clippy"
cargo clippy --workspace --tests -- -D warnings

step "dashboard"
for f in app/wwwroot/admin/*.js app/wwwroot/family/*.js; do node --check "$f"; done
node --test app/wwwroot/admin/tests/*.test.mjs

if command -v shellcheck >/dev/null; then
  step "shellcheck"
  shellcheck -S warning install.sh scripts/*.sh scripts/updater/*.sh mornye/flakyfy-start \
    mornye/flakyfy-chromium mornye/tests/*.sh
fi

if [ "${1:-}" = "--quick" ]; then
  step "quick checks passed"
  exit 0
fi

step "cargo test"
cargo test --workspace

step "all checks passed"
