#!/usr/bin/env python3
"""Build the compact static AniList anime autocomplete index.

The raw Kaggle dataset is large and should stay out of static assets. This
script downloads/extracts the public Kaggle dump when no input CSV is supplied,
then writes only fields needed by the browser media picker.
"""

from __future__ import annotations

import argparse
import ast
import csv
import json
import re
import shutil
import subprocess
import sys
import unicodedata
import zipfile
from pathlib import Path
from typing import Any


DATASET = "calebmwelsh/anilist-anime-dataset"
SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_DIR = SCRIPT_DIR.parent
DEFAULT_WORK_DIR = PROJECT_DIR / "data/input/anilist-media-index"
DEFAULT_OUTPUT = PROJECT_DIR / "static/data/anilist-media-index.json"
PREFERRED_CSV = "anilist_anime_data_complete.csv"


def raise_csv_field_limit() -> None:
    limit = sys.maxsize
    while True:
        try:
            csv.field_size_limit(limit)
            return
        except OverflowError:
            limit //= 10


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input-csv",
        type=Path,
        help="Use an existing CSV instead of downloading from Kaggle.",
    )
    parser.add_argument(
        "--work-dir",
        type=Path,
        default=DEFAULT_WORK_DIR,
        help="Directory for the downloaded/extracted Kaggle dataset.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="Compact JSON index to write.",
    )
    parser.add_argument(
        "--dataset",
        default=DATASET,
        help="Kaggle dataset slug to download.",
    )
    return parser.parse_args()


def normalize_text(value: str) -> str:
    normalized = unicodedata.normalize("NFKC", value).casefold()
    return re.sub(r"[\W_]+", " ", normalized, flags=re.UNICODE).strip()


def parse_int(value: Any) -> int | None:
    if value is None:
        return None
    raw = str(value).strip()
    if not raw:
        return None
    try:
        return int(raw)
    except ValueError:
        try:
            return int(float(raw))
        except ValueError:
            return None


def clean_string(value: Any) -> str | None:
    if value is None:
        return None
    raw = str(value).strip()
    if not raw or raw.lower() == "nan":
        return None
    return raw


def parse_synonyms(value: Any) -> list[str]:
    raw = clean_string(value)
    if not raw:
        return []

    parsed: Any
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        try:
            parsed = ast.literal_eval(raw)
        except (ValueError, SyntaxError):
            parsed = [raw]

    if not isinstance(parsed, list):
        return []

    return unique_strings(str(item).strip() for item in parsed if item is not None)


def unique_strings(values: Any) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if not value:
            continue
        key = normalize_text(value)
        if not key or key in seen:
            continue
        seen.add(key)
        result.append(value)
    return result


def compact_media_row(row: dict[str, str]) -> dict[str, Any] | None:
    media_id = parse_int(row.get("id"))
    if media_id is None:
        return None

    titles = {
        "romaji": clean_string(row.get("title_romaji")),
        "english": clean_string(row.get("title_english")),
        "native": clean_string(row.get("title_native")),
        "userPreferred": clean_string(row.get("title_userPreferred")),
    }
    titles = {key: value for key, value in titles.items() if value}

    synonyms = parse_synonyms(row.get("synonyms"))
    search_values = unique_strings([*titles.values(), *synonyms])
    if not search_values:
        return None

    media_type = clean_string(row.get("type")) or "ANIME"
    media_type = media_type.upper()
    domain = "manga" if media_type == "MANGA" else "anime"

    item: dict[str, Any] = {
        "id": media_id,
        "url": clean_string(row.get("siteUrl")) or f"https://anilist.co/{domain}/{media_id}",
        "type": media_type,
        "titles": titles,
        "synonyms": synonyms,
        "search": normalize_text(" ".join(search_values)),
    }

    media_format = clean_string(row.get("format"))
    if media_format:
        item["format"] = media_format

    year = parse_int(row.get("startDate_year")) or parse_int(row.get("seasonYear"))
    if year is not None:
        item["year"] = year

    popularity = parse_int(row.get("popularity"))
    if popularity is not None:
        item["popularity"] = popularity

    return item


def build_index(input_csv: Path) -> dict[str, Any]:
    raise_csv_field_limit()
    media: list[dict[str, Any]] = []
    with input_csv.open("r", encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            item = compact_media_row(row)
            if item is not None:
                media.append(item)

    media.sort(key=lambda item: item["id"])
    return {
        "schemaVersion": 1,
        "source": f"kaggle:{DATASET}",
        "media": media,
    }


def write_index(index: dict[str, Any], output: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(index, ensure_ascii=False, separators=(",", ":")) + "\n"
    tmp_path = output.with_suffix(output.suffix + ".tmp")
    tmp_path.write_text(payload, encoding="utf-8")
    tmp_path.replace(output)


def download_dataset(dataset: str, work_dir: Path) -> None:
    kaggle = shutil.which("kaggle")
    if not kaggle:
        raise RuntimeError(
            "Kaggle CLI executable 'kaggle' was not found on PATH. "
            "Install kaggle-api or pass --input-csv."
        )

    work_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            kaggle,
            "datasets",
            "download",
            "-d",
            dataset,
            "-p",
            str(work_dir),
            "--unzip",
            "--force",
        ],
        check=True,
    )

    for archive in work_dir.glob("*.zip"):
        with zipfile.ZipFile(archive) as zip_file:
            zip_file.extractall(work_dir)


def find_dataset_csv(work_dir: Path) -> Path:
    preferred = work_dir / PREFERRED_CSV
    if preferred.exists():
        return preferred

    candidates = sorted(work_dir.rglob("*.csv"), key=lambda path: path.stat().st_size, reverse=True)
    if not candidates:
        raise FileNotFoundError(f"No CSV file found under {work_dir}")
    return candidates[0]


def main() -> int:
    args = parse_args()
    try:
        input_csv = args.input_csv
        if input_csv is None:
            download_dataset(args.dataset, args.work_dir)
            input_csv = find_dataset_csv(args.work_dir)

        index = build_index(input_csv)
        write_index(index, args.output)
        size_mb = args.output.stat().st_size / (1024 * 1024)
        print(f"Wrote {len(index['media'])} AniList anime records to {args.output} ({size_mb:.2f} MiB)")
    except Exception as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
