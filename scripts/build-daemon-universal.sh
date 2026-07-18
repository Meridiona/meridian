#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Build the `meridian` daemon as a universal (arm64 + x86_64) binary at
# target/release/meridian — the fixed path `tauri.conf.json`'s
# `bundle.resources` embeds into the app (`backend/meridian`).
#
# Why this exists: `tauri build --target universal-apple-darwin` auto-lipos
# the TRAY's own Rust binary, but it does NOT touch arbitrary
# `bundle.resources` files — the daemon is one of those (a fixed-path
# resource, not a Tauri externalBin/sidecar), so it needs its own two builds +
# manual `lipo` before `tauri build` runs. `scripts/sign-daemon.sh` (which
# runs right after this) already signs whatever binary — thin or fat — ends
# up at target/release/meridian, so it needs no changes for a universal build.
#
#   bash scripts/build-daemon-universal.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  echo "→ build-daemon-universal: cargo build --release --target ${target}"
  cargo build --locked --release --bin meridian --target "${target}"
done

mkdir -p target/release
lipo -create \
  "target/aarch64-apple-darwin/release/meridian" \
  "target/x86_64-apple-darwin/release/meridian" \
  -output "target/release/meridian"

echo "✓ build-daemon-universal: target/release/meridian ($(lipo -archs target/release/meridian))"
