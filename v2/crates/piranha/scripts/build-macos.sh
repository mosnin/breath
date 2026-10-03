#!/usr/bin/env bash
# Build Piranha.app and Piranha_<version>_<arch>.dmg on macOS.
#
# Usage: scripts/build-macos.sh [universal|arm64|x86_64]   (default: universal)
#
# Steps:
#   1. build the RuView sensing-server for each architecture and place it
#      in binaries/ as the Tauri sidecar (lipo'd for universal builds)
#   2. install the UI dependencies
#   3. run `tauri build`, which builds the UI, the Piranha binary, the .app
#      bundle and the .dmg, ad-hoc signed with macos/Entitlements.plist
set -euo pipefail

ARCH="${1:-universal}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WS_DIR="$(cd "$CRATE_DIR/../.." && pwd)"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: Piranha's macOS bundle must be built on macOS" >&2
  exit 1
fi

case "$ARCH" in
  universal) RUST_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin); TAURI_TARGET=universal-apple-darwin ;;
  arm64|aarch64) RUST_TARGETS=(aarch64-apple-darwin); TAURI_TARGET=aarch64-apple-darwin ;;
  x86_64|x64|intel) RUST_TARGETS=(x86_64-apple-darwin); TAURI_TARGET=x86_64-apple-darwin ;;
  *) echo "usage: $0 [universal|arm64|x86_64]" >&2; exit 2 ;;
esac

if [[ ! -f "$WS_DIR/crates/worldgraph/wifi-densepose-worldgraph/Cargo.toml" ]]; then
  echo "==> Fetching git submodules"
  git -C "$WS_DIR" submodule update --init --recursive --depth 1
fi

echo "==> Rust targets: ${RUST_TARGETS[*]}"
# Run from the workspace so rustup targets the toolchain pinned in rust-toolchain.toml.
(cd "$WS_DIR" && rustup target add "${RUST_TARGETS[@]}")

echo "==> Building sensing-server sidecar"
SIDECARS=()
for t in "${RUST_TARGETS[@]}"; do
  (cd "$WS_DIR" && cargo build --release --locked \
      -p wifi-densepose-sensing-server --bin sensing-server --target "$t")
  SIDECARS+=("$WS_DIR/target/$t/release/sensing-server")
done

mkdir -p "$CRATE_DIR/binaries"
SIDECAR_OUT="$CRATE_DIR/binaries/sensing-server-$TAURI_TARGET"
if [[ "$TAURI_TARGET" == universal-apple-darwin ]]; then
  lipo -create -output "$SIDECAR_OUT" "${SIDECARS[@]}"
else
  cp "${SIDECARS[0]}" "$SIDECAR_OUT"
fi
chmod +x "$SIDECAR_OUT"

echo "==> Installing UI dependencies"
(cd "$CRATE_DIR/ui" && npm ci)

echo "==> Building Piranha.app ($TAURI_TARGET)"
(cd "$CRATE_DIR" && ./ui/node_modules/.bin/tauri build --target "$TAURI_TARGET")

BUNDLE_DIR="$WS_DIR/target/$TAURI_TARGET/release/bundle"
echo
echo "Done:"
ls -d "$BUNDLE_DIR"/macos/*.app "$BUNDLE_DIR"/dmg/*.dmg 2>/dev/null || true
