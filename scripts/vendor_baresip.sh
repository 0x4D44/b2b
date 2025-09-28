#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$ROOT_DIR/third_party/src"
mkdir -p "$SRC_DIR"

if [ ! -d "$SRC_DIR/re/.git" ]; then
  echo "Cloning baresip/re into $SRC_DIR/re" >&2
  git clone --depth 1 https://github.com/baresip/re.git "$SRC_DIR/re"
else
  echo "Updating existing $SRC_DIR/re" >&2
  git -C "$SRC_DIR/re" fetch --depth 1 origin
  git -C "$SRC_DIR/re" reset --hard origin/HEAD
fi

if [ ! -d "$SRC_DIR/baresip/.git" ]; then
  echo "Cloning baresip/baresip into $SRC_DIR/baresip" >&2
  git clone --depth 1 https://github.com/baresip/baresip.git "$SRC_DIR/baresip"
else
  echo "Updating existing $SRC_DIR/baresip" >&2
  git -C "$SRC_DIR/baresip" fetch --depth 1 origin
  git -C "$SRC_DIR/baresip" reset --hard origin/HEAD
fi

echo "Vendored sources ready under $SRC_DIR" >&2
echo "To build using vendored static libs:" >&2
echo "  RE_SRC_DIR=$SRC_DIR/re \\" >&2
echo "  BARESIP_SRC_DIR=$SRC_DIR/baresip \\" >&2
echo "  cargo build --release" >&2

