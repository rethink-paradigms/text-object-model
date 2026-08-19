#!/bin/bash
# ── text-runtime release ─────────────────────────────────────────────────────
# Quality gates + release binary + version tag.
#
# Usage:
#   ./scripts/release.sh            # run all gates, build, show tag to create
#   ./scripts/release.sh --tag 0.2.0  # gates + create git tag v0.2.0
#
# The version tag drives releases; the MCP server and docs should reference it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT/text-runtime"

TAG_ARG=""
if [[ "${1:-}" == "--tag" ]]; then
    TAG_ARG="${2:?usage: release.sh --tag <semver>}"
fi

export PATH="$HOME/.cargo/bin:$PATH"

echo "── 1/5 fmt ────────────────────────────────────────────────"
(cd "$RUST_DIR" && cargo fmt --all -- --check)

echo "── 2/5 clippy ─────────────────────────────────────────────"
(cd "$RUST_DIR" && cargo clippy --all-targets -- -D warnings)

echo "── 3/5 tests ──────────────────────────────────────────────"
(cd "$RUST_DIR" && cargo test)

echo "── 4/5 release build ──────────────────────────────────────"
(cd "$RUST_DIR" && cargo build --release)

echo "── 5/5 binary ─────────────────────────────────────────────"
"$RUST_DIR/target/release/text-runtime" --version

if [[ -n "$TAG_ARG" ]]; then
    echo "── tagging v$TAG_ARG ───────────────────────────────────────"
    git -C "$ROOT" tag -a "v$TAG_ARG" -m "text-runtime v$TAG_ARG"
    echo "Created tag v$TAG_ARG (push with: git push origin v$TAG_ARG)"
else
    echo
    echo "All gates passed. To tag a release:"
    echo "  ./scripts/release.sh --tag <semver>"
fi
