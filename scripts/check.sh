#!/usr/bin/env bash
# Project gate: formatting, lints (warnings are errors) and tests for the whole workspace.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check
echo "==> cargo clippy -D warnings"
cargo clippy --workspace --all-targets --locked -- -D warnings
echo "==> cargo test"
cargo test --workspace --locked
echo "==> OK"
