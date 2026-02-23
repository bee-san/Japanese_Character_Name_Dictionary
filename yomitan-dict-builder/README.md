# Yomitan Character Dictionary Builder

A web application that generates **Yomitan-compatible character dictionaries** from VNDB (Visual Novel Database) and AniList (Anime/Manga). When installed in Yomitan (a browser dictionary extension for Japanese), looking up a character's Japanese name shows a rich popup card with their portrait, role, stats, and description.

## Features

- **VNDB Support**: Fetch characters from any visual novel by VN ID (e.g., `v17` for Steins;Gate)
- **AniList Support**: Fetch characters from any anime or manga by AniList ID (e.g., `9253`)
- **Spoiler Control**: Three levels — No spoilers, Minor spoilers, Full spoilers
- **Rich Character Cards**: Portrait image, role badge, description, stats, and traits
- **Name Variants**: Generates entries for full name, family name, given name, with/without space, honorific suffixes (さん, 様, 先生, etc.), and aliases
- **Auto-Update**: Generated dictionaries include URLs for Yomitan to auto-update
- **Japanese Name Parsing**: Romaji→hiragana conversion, katakana→hiragana, mixed kanji/kana handling

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (1.70+)

### Build & Run

```bash
cargo build --release
cargo run --release
```

Visit **http://localhost:3000** in your browser.

### Docker

The easiest way to run the application — no Rust toolchain required.

**Using Docker Compose (recommended):**

```bash
docker compose up -d
```

The app will be available at **http://localhost:9721**.

**Using Docker directly:**

```bash
docker build -t yomitan-dict-builder .
docker run -d -p 9721:3000 yomitan-dict-builder
```

Visit **http://localhost:9721** in your browser (or `http://localhost:3000` if running with cargo directly).

### Usage

1. Select source (VNDB or AniList)
2. Enter the media ID (e.g., `v17` for VNDB, `9253` for AniList)
3. Select media type (Anime/Manga) if using AniList
4. Choose spoiler level
5. Click "Generate Dictionary"
6. Import the downloaded ZIP file into Yomitan

## Architecture

```
yomitan-dict-builder/
├── Cargo.toml
├── src/
│   ├── main.rs              # Axum server, routes
│   ├── models.rs            # Shared data structures
│   ├── vndb_client.rs       # VNDB API client
│   ├── anilist_client.rs    # AniList GraphQL client
│   ├── name_parser.rs       # Japanese name → hiragana readings + honorifics
│   ├── content_builder.rs   # Yomitan structured content JSON builder
│   ├── image_handler.rs     # Base64 decode, format detection
│   └── dict_builder.rs      # ZIP assembly orchestrator
├── static/
│   └── index.html           # Frontend (single file with embedded CSS+JS)
└── README.md
```

## API Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/` | GET | Serves the web frontend |
| `/api/yomitan-dict` | GET | Generates and returns dictionary ZIP |
| `/api/yomitan-index` | GET | Returns lightweight index metadata (for update checks) |

### Query Parameters

| Parameter | Required | Values | Description |
|---|---|---|---|
| `source` | Yes | `vndb`, `anilist` | Data source |
| `id` | Yes | String | Media ID (e.g., `v17`, `9253`) |
| `spoiler_level` | No | `0`, `1`, `2` | Spoiler filtering (default: `0`) |
| `media_type` | No | `ANIME`, `MANGA` | AniList media type (default: `ANIME`) |

## Spoiler Levels

- **Level 0 (No Spoilers)**: Name, image, game title, role badge only
- **Level 1 (Minor Spoilers)**: + Description (spoiler tags stripped) + stats + traits (spoiler ≤ 1)
- **Level 2 (Full Spoilers)**: + Full unmodified description + all traits

## License

MIT