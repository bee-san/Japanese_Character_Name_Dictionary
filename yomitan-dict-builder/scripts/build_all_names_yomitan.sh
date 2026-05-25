#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$ROOT_DIR/data/snapshots/all-names}"
ZIP_PATH="${2:-$OUT_DIR/all-names-yomitan.zip}"

cd "$ROOT_DIR"

cargo run --bin ultimate_snapshot -- build --config config/all_names_snapshot.toml --out "$OUT_DIR"
cargo run --bin snapshot_yomitan -- --snapshot "$OUT_DIR" --out "$ZIP_PATH" --title "All Names Offline Snapshot" --honorifics

echo "Snapshot output: $OUT_DIR"
echo "Yomitan ZIP: $ZIP_PATH"
