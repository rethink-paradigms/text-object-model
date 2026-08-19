#!/bin/bash
# ── text-runtime install ─────────────────────────────────────────────────────
# Builds the release binary and installs it to ~/.local/bin/text-runtime.
# Usage: ./scripts/install.sh [PREFIX]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${1:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

echo "▸ Building release binary (text-runtime)"
(cd "$ROOT/text-runtime" && cargo build --release)

mkdir -p "$BIN_DIR"
cp "$ROOT/text-runtime/target/release/text-runtime" "$BIN_DIR/text-runtime"

echo "▸ Installed: $BIN_DIR/text-runtime"
"$BIN_DIR/text-runtime" --version

echo
echo "Next steps:"
echo "  - Ensure $BIN_DIR is on your PATH"
echo "  - Configure the daemon: ~/.config/text-runtime/config.toml (see docs/RUNBOOK.md)"
echo "  - Run as a service: deploy/text-runtime.plist (macOS launchd) or deploy/text-runtime.service (Linux systemd)"
