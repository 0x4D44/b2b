#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export RE_SRC_DIR="$ROOT_DIR/third_party/src/re"
export BARESIP_SRC_DIR="$ROOT_DIR/third_party/src/baresip"

echo "Building with vendored static libre/baresip..." >&2
cargo build --release "$@"
echo "Binary: $ROOT_DIR/target/release/b2b" >&2
echo "Linked libs:" >&2
ldd "$ROOT_DIR/target/release/b2b" || true

