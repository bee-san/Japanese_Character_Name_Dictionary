#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SNAPSHOT_DIR="${1:-$ROOT_DIR/data/snapshots/available-names}"
ZIP_PATH="${2:-$SNAPSHOT_DIR/available-names-yomitan.zip}"
export YOMITAN_EXPORT_THREADS="${YOMITAN_EXPORT_THREADS:-6}"

cd "$ROOT_DIR"

cargo run --bin snapshot_yomitan -- \
  --snapshot "$SNAPSHOT_DIR" \
  --out "$ZIP_PATH" \
  --title "Available Names Offline Snapshot" \
  --honorifics true

echo "Snapshot: $SNAPSHOT_DIR"
echo "Yomitan ZIP: $ZIP_PATH"
