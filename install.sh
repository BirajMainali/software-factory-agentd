#!/bin/sh
# Build and install agentd for the current user, from this checkout.
set -eu

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found; install Rust from https://rustup.rs" >&2
    exit 1
fi

cargo install --path . --locked
echo "installed; check with: agentd help"
