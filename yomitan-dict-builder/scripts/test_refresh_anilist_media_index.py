#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parent / "refresh_anilist_media_index.py"
PROJECT_DIR = SCRIPT_PATH.parent.parent
FIXTURE = PROJECT_DIR / "tests/fixtures/anilist_media_index/anilist_media_sample.csv"


def load_module():
    spec = importlib.util.spec_from_file_location("refresh_anilist_media_index", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class RefreshAnilistMediaIndexTests(unittest.TestCase):
    def test_builds_compact_deterministic_index_from_fixture(self):
        module = load_module()

        index = module.build_index(FIXTURE)

        self.assertEqual(index["schemaVersion"], 1)
        self.assertEqual([item["id"] for item in index["media"]], [1, 9253, 16498])

        steins_gate = next(item for item in index["media"] if item["id"] == 9253)
        self.assertEqual(steins_gate["url"], "https://anilist.co/anime/9253")
        self.assertEqual(steins_gate["type"], "ANIME")
        self.assertEqual(steins_gate["titles"]["native"], "シュタインズ・ゲート")
        self.assertEqual(steins_gate["synonyms"], ["StG", "Steins Gate"])
        self.assertEqual(steins_gate["format"], "TV")
        self.assertEqual(steins_gate["year"], 2011)
        self.assertEqual(steins_gate["popularity"], 493871)
        self.assertIn("steins gate", steins_gate["search"])

    def test_writes_minified_json(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "anilist-media-index.json"
            module.write_index(module.build_index(FIXTURE), output)
            raw = output.read_text(encoding="utf-8")

        self.assertFalse("\n  " in raw)
        parsed = json.loads(raw)
        self.assertEqual(len(parsed["media"]), 3)


if __name__ == "__main__":
    unittest.main()
