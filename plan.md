# Ultimate Name Snapshot Branch

## Summary

- Create branch `feat/ultimate-name-snapshot` from `main`, and keep the current untracked `.codex` out of commits.
- Build a separate one-shot offline snapshot pipeline inside this repo; do not turn the existing request-time generator into the ingest engine.
- Scope the first snapshot to all proper names: real people, fictional characters, organizations, works, products, and places.
- Allow official/open sources plus community snapshots, but treat source legality, attribution, and image rights as first-class metadata on every record.
- Produce two outputs from the same run: a canonical metadata snapshot and a full local image mirror. Public redistribution is not part of v1.

## Implementation Changes

- Keep `yomitan-dict-builder/src/main.rs` behaviorally unchanged for serving Yomitan dictionaries; the new work lives in a new binary, `src/bin/ultimate_snapshot.rs`.
- Do not overload `yomitan-dict-builder/src/models.rs`. Add a new snapshot schema with exact tables/types for `entity`, `name_variant`, `reading`, `external_id`, `relationship`, `source_record`, `source_assertion`, `license`, and `image_asset`.
- Add a config file, `config/ultimate_snapshot.toml`, with fixed source toggles, source-specific input locations, rate limits, output directory, and image policy. The default command is `cargo run --bin ultimate_snapshot -- build --config config/ultimate_snapshot.toml --out data/snapshots/2026-04-08`.
- Structure the pipeline as four hard phases: stage raw source rows, normalize strings/scripts/readings, resolve entities into a graph, and export SQLite + Parquet + manifests + image mirror.
- Use SQLite as the canonical build artifact, Parquet as the scan/export format, and a content-addressed filesystem tree for image binaries keyed by SHA-256. Do not store image blobs inside SQLite.
- Normalize every name into raw + normalized forms: NFKC, punctuation folding, kana harmonization, script detection, and derived romaji marked as computed rather than sourced.
- Resolve entities in fixed order: external IDs first, then exact normalized Japanese-script matches within the same domain/context, then conservative probabilistic clustering. Ambiguous merges remain separate entities.
- Every field in the canonical snapshot must be backed by a `source_assertion` row containing source ID, source record ID/URI, retrieval date, field path, license class, and whether the value was sourced or derived.
- Image handling is split from name handling: mirror all selected source images locally, attach `rights_status` and source provenance to each image, and generate two export manifests: `local_full_image_manifest` and `shareable_image_manifest`.

## Source Matrix

- Ingest `anime-offline-database` only as a title/crosswalk source. Its published schema exposes titles, synonyms, picture, thumbnail, and provider URLs, but not character rows. Source: `manami-project/anime-offline-database` (<https://github.com/manami-project/anime-offline-database>).
- Ingest the existing Kaggle MAL anime/manga snapshot only for media titles, alt titles, author/studio/publisher strings, and descriptions. Its published schema lists anime/manga metadata but not character entities or image columns. Source: `MyAnimeList Anime & Manga Dataset (July 2025)` (<https://www.kaggle.com/datasets/hamzaashfaque1999/myanimelist-scraped-data>).
- Ingest the existing Kaggle AniList snapshot as a major character/staff/media source. Its published schema explicitly includes characters, staff, `coverImage_*`, and `bannerImage`. Source: `Anilist Anime Dataset` (<https://www.kaggle.com/datasets/calebmwelsh/anilist-anime-dataset>).
- Add the community MAL character snapshot as the MAL-side character/person complement. Use it for `name_kanji`, bios, favorites, and image coverage, and join to AniList via `idMal` where present. Source: `Anime Character Database (July 2025)` (<https://www.kaggle.com/datasets/sazzadsiddiquelikhon/anime-character-database-july-2025>).
- Add JMnedict/ENAMDICT as the broad proper-name lexicon seed for person, place, work, company, and product names, with EDRDG attribution/share-alike carried into the manifests. Sources: `JMnedict docs` (<https://www.edrdg.org/enamdict/enamdict_doc.html>), `EDRDG licence` (<https://www.edrdg.org/edrdg/licence.html>).
- Add Web NDL Authorities as the canonical Japanese authority source for real people/corporate bodies with kana transcription and variant-name semantics. Use SPARQL for harvests and keep the terms-of-use notice with each export. Sources: `SPARQL docs` (<https://id.ndl.go.jp/information/sparql/>), `terms of use` (<https://id.ndl.go.jp/information/use/>).
- Add Wikidata and Wikipedia dumps for cross-domain alias breadth, redirects, and ID linkage. Use Wikidata entity dumps plus ja/en Wikipedia title/redirect dumps; treat Wikipedia text/images separately from Wikidata structured data. Sources: `Wikidata dumps` (<https://www.wikidata.org/wiki/Wikidata:Database_download>), `Wikimedia dump licensing` (<https://dumps.wikimedia.org/legal.html>), `Wikidata entities index` (<https://dumps.wikimedia.org/wikidatawiki/entities/>).
- Add Bangumi as the Japanese media-domain graph for subject, character, actor, and staff links, including image URLs and alias/kana fields where present. Source: `Bangumi Subject API` (<https://github.com/bangumi/api/wiki/Subject-API>).
- Add MusicBrainz as the music-side authority source for artists, labels, works, aliases, and characters, using dumps over live crawling. Sources: `MusicBrainz database overview` (<https://musicbrainz.org/doc/MusicBrainz_Database>), `download docs` (<https://musicbrainz.org/doc/MusicBrainz_Database/Download>), `API/alias docs` (<https://musicbrainz.org/doc/Development/XML_Web_Service/Version_2>).
- Add VIAF as an authority-file merge layer for real-person alias reconciliation and library crosswalks. Source: `VIAF dataset` (<https://viaf.org/en/viaf/data>).
- Keep direct full-crawl AniList API ingestion out of v1 because AniList's own terms prohibit hoarding and mass collection; rely on your existing snapshot and community exports instead. Source: `AniList terms` (<https://anilist.gitbook.io/anilist-apiv2-docs/docs/guide/terms-of-use>).
- Keep Pixiv, AniDB, X, YouTube, Instagram, and similar scrape-hostile or high-compliance platforms in a disabled registry for v1. Record them as future/manual sources only. Sources: `Pixiv anti-crawler post` (<https://inside.pixiv.blog/2023/05/17/102629>), `X developer guidelines` (<https://docs.x.com/developer-guidelines>).

## Interfaces

- CLI surface is fixed to `build`, `verify`, and `export-report`.
- Canonical artifact names are fixed to `snapshot.sqlite`, `parquet/`, `image_store/`, `source_manifest.json`, `license_manifest.json`, `build_report.md`, and `shareable_export_report.md`.
- Existing web routes stay unchanged; the snapshot pipeline is a producer only.
- The current VNDB daily dump you already identified remains an input to the new source registry; use VNDB API v2 only for schema validation, targeted backfills, and ID lookups. Source: `VNDB API v2` (<https://api.vndb.org/kana>).

## Test Plan

- Parser fixture tests for every enabled source, with one locked sample payload/file per source family.
- Normalization tests covering kanji, kana, mixed-script names, middle dots, long-vowel marks, iteration marks, whitespace variants, and duplicate alias collapse.
- Resolution tests for exact ID merges, kana-backed real-person merges, fictional character merges within shared work context, and deliberate non-merges for homographs.
- Source-policy tests proving that `anime-offline` never emits character entities, the MAL anime/manga dataset never emits character/image records, and direct AniList API full-crawl code paths are absent.
- Image tests for dedupe-by-hash, rights tagging, per-source path layout, broken URL handling, and manifest generation.
- End-to-end acceptance run on a small fixture bundle that emits all artifacts, produces deterministic row counts, and leaves every canonical field traceable to at least one `source_assertion`.

## Assumptions

- Chosen defaults from your answers: all proper names, community scrapes allowed, offline snapshot pipeline as the branch deliverable, and full image mirror required.
- Full image mirroring is for local/private build outputs; v1 does not promise that every mirrored image is publicly redistributable.
- The branch should prefer new snapshot-specific modules over invasive edits to the existing generator so you can keep shipping the current site while the snapshot pipeline matures.
- As of April 8, 2026, this plan treats source availability/licensing according to the linked pages above and your existing VNDB dump research.
