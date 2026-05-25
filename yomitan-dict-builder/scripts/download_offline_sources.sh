#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INPUT_DIR="$ROOT_DIR/data/input"
VNDB_URL="https://dl.vndb.org/dump/vndb-db-latest.tar.zst"
VNDB_OUT="$INPUT_DIR/vndb-db-latest.tar.zst"
EXISTING_VNDB_DUMP="$(find "$INPUT_DIR" -maxdepth 1 -type f -name 'vndb-db-*.tar.zst' | sort | tail -n 1 || true)"
KAGGLE_DATASET="calebmwelsh/anilist-anime-dataset"
KAGGLE_ZIP="$INPUT_DIR/anilist-anime-dataset.zip"
KAGGLE_TARGET="$INPUT_DIR/anilist_anime_dataset.csv"
KAGGLE_BIN="${KAGGLE_BIN:-}"
JMNEDICT_URL="ftp://ftp.edrdg.org/pub/Nihongo/JMnedict.xml.gz"
JMNEDICT_OUT="$INPUT_DIR/JMnedict.xml.gz"

mkdir -p "$INPUT_DIR"

echo "==> VNDB dump"
if [[ -f "$VNDB_OUT" ]]; then
  echo "Already present: $VNDB_OUT"
elif [[ -n "$EXISTING_VNDB_DUMP" ]]; then
  echo "Already present: $EXISTING_VNDB_DUMP"
  ln -sf "$(basename "$EXISTING_VNDB_DUMP")" "$VNDB_OUT"
else
  curl -L "$VNDB_URL" -o "$VNDB_OUT"
fi

echo "==> AniList Kaggle export"
if [[ -f "$KAGGLE_TARGET" ]]; then
  echo "Already present: $KAGGLE_TARGET"
else
  if [[ -z "$KAGGLE_BIN" ]]; then
    if command -v kaggle >/dev/null 2>&1; then
      KAGGLE_BIN="$(command -v kaggle)"
    elif [[ -x /tmp/kaggle-cli/bin/kaggle ]]; then
      KAGGLE_BIN="/tmp/kaggle-cli/bin/kaggle"
    fi
  fi

  if [[ -z "$KAGGLE_BIN" ]]; then
    echo "Kaggle CLI is not installed. Install it with:"
    echo "  pip install --user kaggle"
    exit 2
  fi

  if ! "$KAGGLE_BIN" datasets download -d "$KAGGLE_DATASET" -p "$INPUT_DIR"; then
    echo "Kaggle download failed."
    echo "If this machine requires auth for the dataset, place kaggle.json in ~/.config/kaggle/ or ~/.kaggle/ and rerun this script."
    exit 3
  fi

  if [[ -f "$KAGGLE_ZIP" ]]; then
    unzip -o "$KAGGLE_ZIP" -d "$INPUT_DIR" >/dev/null
  fi

  FIRST_CSV="$(find "$INPUT_DIR" -maxdepth 1 -type f -iname '*.csv' | sort | head -n 1 || true)"
  if [[ -n "$FIRST_CSV" && "$FIRST_CSV" != "$KAGGLE_TARGET" ]]; then
    cp "$FIRST_CSV" "$KAGGLE_TARGET"
  fi

  if [[ ! -f "$KAGGLE_TARGET" ]]; then
    echo "Failed to locate AniList Kaggle CSV after download."
    exit 4
  fi
fi

echo "==> JMnedict lexicon"
if [[ -f "$JMNEDICT_OUT" ]]; then
  echo "Already present: $JMNEDICT_OUT"
else
  curl -L "$JMNEDICT_URL" -o "$JMNEDICT_OUT"
fi

echo "Downloaded:"
echo "  $VNDB_OUT"
echo "  $KAGGLE_TARGET"
echo "  $JMNEDICT_OUT"
