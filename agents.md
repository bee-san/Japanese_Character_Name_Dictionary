# agents.md — Yomitan Character Dictionary Builder

## Project Overview

Rust (Axum) web service that generates Yomitan-compatible character name dictionaries from VNDB and AniList APIs. Given a username or media ID, it fetches characters, parses Japanese names into hiragana readings, builds rich popup cards, and packages everything into a Yomitan ZIP.

No database. No auth. Two external API dependencies (VNDB REST, AniList GraphQL).

## Tech Stack

- Language: Rust (edition 2021)
- Web framework: Axum 0.7
- Async runtime: Tokio
- HTTP client: reqwest 0.12
- ZIP: `zip` crate v2 (requires `Cursor<Vec<u8>>` for in-memory writes)
- Serialization: serde / serde_json
- Other: base64, regex, rand, uuid, tower-http (CORS + static files)

## Repository Layout

```
yomitan-dict-builder/
├── Cargo.toml
├── Dockerfile / docker-compose.yml
├── README.md
├── src/
│   ├── main.rs              # Axum routes + orchestration logic
│   ├── models.rs            # Shared types: Character, CharacterData, CharacterTrait, UserMediaEntry
│   ├── vndb_client.rs       # VNDB REST API client (user resolution, VN title, character fetch, image download)
│   ├── anilist_client.rs    # AniList GraphQL client (user list, character fetch, image download)
│   ├── kana.rs              # Low-level kana utilities: kanji detection, romaji→hiragana, katakana↔hiragana, syllable boundary handling
│   ├── name_parser.rs       # Name handling: unified split/reading API with optional romaji hints, honorific suffixes data
│   ├── content_builder.rs   # Yomitan structured content JSON (character popup cards), spoiler stripping, DictSettings struct
│   ├── image_handler.rs     # Base64 data URI → raw bytes + file extension detection
│   ├── image_cache.rs       # SQLite-backed image cache (blob storage, hit counts, eviction)
│   ├── media_cache.rs       # SQLite-backed media/character data cache with TTL expiry
│   ├── anilist_name_test_data.rs # Bulk integration tests for name splitting/reading with real AniList character data
│   └── dict_builder.rs      # ZIP assembly: index.json, tag_bank, term_banks (chunked at 10k), img/ folder
├── static/
│   ├── index.html           # Frontend HTML (two-column layout with interactive Yomitan preview card)
│   ├── style.css            # Frontend styles (Yomitan card mimicry, responsive layout)
│   └── app.js               # Frontend logic (settings toggles, SSE progress, form handling)
└── tests/
    └── integration_tests.rs # HTTP endpoint tests (require running server)
```

Related docs (in `docs/`):
- `docs/plans/plan.md` — Exhaustive implementation plan with full API examples, romaji lookup tables, structured content format, test expectations. Read this for any deep implementation questions.
- `docs/agents_read_me.md` — Guide for agents porting this code to other languages/frameworks. Not relevant when working on the Rust codebase itself.

## Build & Run

```bash
# From yomitan-dict-builder/
cargo build --release
cargo run --release          # Serves on http://localhost:3000

# Docker
docker compose up -d         # Serves on http://localhost:9721

# Tests (549+ unit tests inline, integration tests need running server)
cargo test
```

## Module Dependency Graph

```
models.rs          ← everything depends on this
    ↓
vndb_client.rs     ← uses models, reqwest, base64
anilist_client.rs  ← uses models, reqwest, base64
    ↓
kana.rs            ← standalone (no external deps beyond std)
    ↓
name_parser.rs     ← uses kana (unified name splitting + reading generation with optional hints, honorific data)
    ↓
content_builder.rs ← uses models, name_parser
image_handler.rs   ← uses base64
image_cache.rs     ← SQLite-backed (tokio, sha2 hashing)
media_cache.rs     ← SQLite-backed (tokio, serde_json, TTL-based expiry)
    ↓
dict_builder.rs    ← uses models, kana, name_parser, content_builder, image_handler, zip
    ↓
main.rs            ← orchestrates everything via Axum routes
```

## Key Data Flow

```
Username/Media ID
  → vndb_client / anilist_client: fetch character list (paginated, rate-limited)
  → vndb_client / anilist_client: download portrait images → base64 data URIs
  → name_parser: parse Japanese names → hiragana readings
  → content_builder: build structured content JSON cards
  → dict_builder: generate term entries (base + honorifics + aliases), deduplicate, assemble ZIP
  → HTTP response: application/zip
```

## API Endpoints

| Endpoint | Description |
|---|---|
| `GET /` | Static frontend |
| `GET /api/user-lists?vndb_user=X&anilist_user=Y` | Preview user's in-progress media |
| `GET /api/generate-stream?vndb_user=X&...` | SSE progress + download token |
| `GET /api/download?token=UUID` | Download ZIP by token (single-use, 5min expiry) |
| `GET /api/yomitan-dict?source=vndb&id=v17` | Direct ZIP generation (blocks until done) |
| `GET /api/yomitan-dict?vndb_user=X&anilist_user=Y` | Username-based ZIP generation |
| `GET /api/yomitan-index?...` | Lightweight index.json metadata (for Yomitan update checks) |

### Query Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `vndb_user` | string | — | VNDB username or profile URL |
| `anilist_user` | string | — | AniList username |
| `source` | string | — | `"vndb"` or `"anilist"` (single-media mode) |
| `id` | string | — | Media ID, e.g. `v17` or `9253` (single-media mode) |
| `media_type` | string | `"ANIME"` | `"ANIME"` or `"MANGA"` (AniList only) |
| `honorifics` | bool | `true` | Generate honorific suffix entries (さん, ちゃん, 先生, etc.) |
| `image` | bool | `true` | Include character portrait images in the dictionary |
| `tag` | bool | `true` | Include role badges (Main Character, etc.) |
| `description` | bool | `true` | Include character descriptions |
| `traits` | bool | `true` | Include character traits/information |
| `spoilers` | bool | `true` | Include spoiler content in descriptions and traits |

### URL-as-Settings Pattern

All dictionary options (usernames, spoiler level, source, media type) are encoded as query parameters in the URL. This is intentional — the URLs themselves act as persistent settings for Yomitan's update mechanism:

1. User imports a dictionary via the index URL, e.g. `http://host/api/yomitan-index?vndb_user=foo&spoilers=false`
2. Yomitan stores that full index URL internally
3. On update check, Yomitan re-fetches the index URL → the `generate_index` handler reconstructs a `downloadUrl` with all the same query params baked in
4. Yomitan downloads the fresh ZIP from that URL → dictionary is regenerated with the original settings

This means changing a setting (e.g. toggling spoilers) requires re-importing with a new URL. There is no server-side state or user accounts — the URL IS the configuration. When adding new options, they must be added to the `DictQuery` struct and threaded through `generate_index`'s `downloadUrl` construction so they survive the update cycle.

## Critical Implementation Details

Things that are easy to break and hard to debug:

1. **Name parsing API**: `name_parser.rs` exposes two public functions — `split_japanese_name_with_hints()` and `generate_name_readings()`. Both accept optional `first_name_hint` (given) and `last_name_hint` (family) romaji hints. When hints are `None` (VNDB path), they delegate to internal helpers that use VNDB's positional romaji mapping where `romanized_parts[0]` → family reading, `romanized_parts[1]` → given reading (Western→Japanese order swap — looks wrong but is correct). When hints are provided (AniList path), they use the hints to split spaceless native names and generate per-part readings directly. The old `split_japanese_name()` and `generate_mixed_name_readings()` still exist as private internal helpers but are not part of the public API.

2. **Image flow**: Images must be downloaded and base64-encoded BEFORE passing characters to dict_builder. The builder extracts raw bytes from the base64 data URI and writes them as binary files in the ZIP's `img/` folder.

3. **ZIP writer needs Seek**: Use `std::io::Cursor<Vec<u8>>`, not bare `Vec<u8>`.

4. **Term bank chunking**: Max 10,000 entries per `term_bank_N.json` file.

5. **Entry deduplication**: All term entries are deduplicated via `HashSet<String>` on the term+reading key. Family name matching an alias → only one entry.

6. **Characters without `name_original` are logged and skipped**: No Japanese name = no dictionary entries. A `warn!` log is emitted with the character's ID and romanized name.

7. **Rate limits**: VNDB 200ms between paginated requests (200 req/5min). AniList 300ms (90 req/min).

8. **Spoiler stripping**: VNDB uses `[spoiler]...[/spoiler]`, AniList uses `~!...!~`. Both must be handled.

9. **Revision field**: Must be random on every generation (triggers Yomitan update detection). Not deterministic.

10. **VNDB user input parsing**: Users paste URLs like `https://vndb.org/u306587`. Must extract user ID from URL before API calls. See `vndb_client.rs::parse_user_input()`.

11. **Port is configurable**: Via `PORT` env var, defaults to 3000. `BASE_URL` env var controls auto-update URLs and defaults to `http://127.0.0.1:{PORT}`.

## Honorific Suffixes (257 entries across 14 categories)

Categories: Respectful/Formal, Casual/Friendly, Academic/Educational, Corporate/Business, Government/Political, Military/Law Enforcement, Religious/Spiritual, Medical, Martial Arts/Traditional, Family/Kinship, Historical/Feudal, Fantasy/Fictional, Otaku/Internet/Modern Slang, and kana-form duplicates.

Applied to: family name, given name, combined name, original (with space), and each alias.

## Testing Strategy

- Unit tests are inline in each module (`#[cfg(test)]` blocks). Run with `cargo test`.
- Integration tests in `tests/integration_tests.rs` require a running server instance.
- Key test areas: romaji→hiragana conversion, name splitting, spoiler stripping, birthday formatting, structured content shape, entry deduplication.

## Common Tasks

**Adding a new API source**: Create a new client module following the pattern of `vndb_client.rs` / `anilist_client.rs`. Must produce `CharacterData` and a title string. Wire it into `main.rs` orchestration.

**Changing the popup card layout**: Edit `content_builder.rs`. The structured content format is Yomitan-specific JSON using HTML-like tags. See `docs/plans/plan.md` section 8 for the full spec.

**Adding new honorifics**: Edit the `HONORIFICS` constant in `name_parser.rs` and update `dict_builder.rs` if the generation logic needs changes.

**Modifying term entry generation**: Edit `dict_builder.rs::add_character()`. This is where base names, honorific variants, and alias entries are created.

## Yomitan Structured Content — Allowed HTML Tags & CSS Properties

Source of truth: Yomitan source code at `github.com/yomidevs/yomitan` (master branch), specifically:
- `ext/data/schemas/dictionary-term-bank-v3-schema.json` (JSON Schema)
- `types/ext/structured-content.d.ts` (TypeScript types)
- `ext/js/display/structured-content-generator.js` (rendering engine)

All schemas use `"additionalProperties": false`, meaning ONLY the properties listed below are accepted. Anything else is silently dropped or rejected.

### Allowed HTML Tags (Exhaustive)

| Tag | Category | Supports `style`? | Supports `content` (children)? | Notes |
|---|---|---|---|---|
| `br` | Empty | No | No | Line break only. Supports `data`. |
| `ruby` | Unstyled container | No | Yes | Ruby annotation base. |
| `rt` | Unstyled container | No | Yes | Ruby annotation text. |
| `rp` | Unstyled container | No | Yes | Ruby fallback parenthesis. |
| `table` | Unstyled container | No | Yes | Wrapped in a `div.gloss-sc-table-container` at render time. |
| `thead` | Unstyled container | No | Yes | Table head. |
| `tbody` | Unstyled container | No | Yes | Table body. |
| `tfoot` | Unstyled container | No | Yes | Table foot. |
| `tr` | Unstyled container | No | Yes | Table row. |
| `td` | Table cell | Yes | Yes | Also supports `colSpan`, `rowSpan`. |
| `th` | Table cell | Yes | Yes | Also supports `colSpan`, `rowSpan`. |
| `span` | Styled container | Yes | Yes | Inline container. Also supports `title`. |
| `div` | Styled container | Yes | Yes | Block container. Also supports `title`. |
| `ol` | Styled container | Yes | Yes | Ordered list. Also supports `title`. |
| `ul` | Styled container | Yes | Yes | Unordered list. Also supports `title`. |
| `li` | Styled container | Yes | Yes | List item. Also supports `title`. |
| `details` | Styled container | Yes | Yes | Collapsible section. Also supports `title`, `open` (boolean). |
| `summary` | Styled container | Yes | Yes | Summary for `details`. Also supports `title`. |
| `img` | Image | No (has own props) | No | Requires `path`. See image properties below. |
| `a` | Link | No | Yes | Requires `href`. URLs starting with `?` are internal dictionary links. External links must match `^(?:https?:|\?)[\w\W]*`. Also supports `lang`. |

That's it. No `<p>`, no `<h1>`–`<h6>`, no `<b>`, no `<i>`, no `<em>`, no `<strong>`, no `<u>`, no `<s>`, no `<sub>`, no `<sup>`, no `<pre>`, no `<code>`, no `<blockquote>`, no `<hr>`, no `<input>`, no `<button>`, no `<form>`, no `<video>`, no `<audio>`, no `<canvas>`, no `<iframe>`, no `<script>`, no `<style>`.

### Common Attributes (All Elements)

| Attribute | Type | Description |
|---|---|---|
| `tag` | string (required) | The HTML tag name. |
| `data` | `{[key: string]: string}` | Custom `data-sc*` attributes added to the DOM element. |
| `lang` | string | Language code (RFC 5646). Sets `lang` attribute on the element. |
| `content` | string, Element, or Content[] | Child content. Not supported on `br` or `img`. |

### Allowed CSS Properties in `style` Object (Exhaustive)

Only the styled containers (`span`, `div`, `ol`, `ul`, `li`, `details`, `summary`) and table cells (`td`, `th`) accept a `style` object. The following properties are the ONLY ones recognized:

| Property | Type | Allowed Values |
|---|---|---|
| `fontStyle` | string | `"normal"`, `"italic"` |
| `fontWeight` | string | `"normal"`, `"bold"` |
| `fontSize` | string | Any CSS font-size string (e.g. `"1.2em"`, `"small"`) |
| `color` | string | Any CSS color string |
| `background` | string | Any CSS background shorthand string |
| `backgroundColor` | string | Any CSS color string |
| `textDecorationLine` | string or string[] | `"none"`, `"underline"`, `"overline"`, `"line-through"` (or array of the non-none values) |
| `textDecorationStyle` | string | `"solid"`, `"double"`, `"dotted"`, `"dashed"`, `"wavy"` |
| `textDecorationColor` | string | Any CSS color string |
| `borderColor` | string | Any CSS color string |
| `borderStyle` | string | Any CSS border-style string |
| `borderRadius` | string | Any CSS border-radius string |
| `borderWidth` | string | Any CSS border-width string |
| `clipPath` | string | Any CSS clip-path string |
| `verticalAlign` | string | `"baseline"`, `"sub"`, `"super"`, `"text-top"`, `"text-bottom"`, `"middle"`, `"top"`, `"bottom"` |
| `textAlign` | string | `"start"`, `"end"`, `"left"`, `"right"`, `"center"`, `"justify"`, `"justify-all"`, `"match-parent"` |
| `textEmphasis` | string | Any CSS text-emphasis shorthand string |
| `textShadow` | string | Any CSS text-shadow string |
| `margin` | string | Any CSS margin shorthand string |
| `marginTop` | number or string | Number → converted to `em`. String → used as-is. |
| `marginLeft` | number or string | Number → converted to `em`. String → used as-is. |
| `marginRight` | number or string | Number → converted to `em`. String → used as-is. |
| `marginBottom` | number or string | Number → converted to `em`. String → used as-is. |
| `padding` | string | Any CSS padding shorthand string |
| `paddingTop` | string | Any CSS length string |
| `paddingLeft` | string | Any CSS length string |
| `paddingRight` | string | Any CSS length string |
| `paddingBottom` | string | Any CSS length string |
| `wordBreak` | string | `"normal"`, `"break-all"`, `"keep-all"` |
| `whiteSpace` | string | Any CSS white-space string |
| `cursor` | string | Any CSS cursor string |
| `listStyleType` | string | Any CSS list-style-type string |

No `display`, no `position`, no `float`, no `width`/`height` (use image props for images), no `overflow`, no `opacity`, no `transform`, no `transition`, no `animation`, no `z-index`, no `flex`/`grid` properties, no `visibility`, no `box-shadow`, no `outline`, no `max-width`/`min-width`, no `font-family`, no `line-height`, no `letter-spacing`.

### Image Element Properties (`tag: "img"`)

| Property | Type | Description |
|---|---|---|
| `path` | string (required) | Path to image file in the ZIP archive. |
| `width` | number | Preferred width (minimum 0). |
| `height` | number | Preferred height (minimum 0). |
| `title` | string | Hover text. |
| `alt` | string | Alt text. |
| `description` | string | Description of the image. |
| `pixelated` | boolean | Pixelated rendering at larger sizes. Default `false`. |
| `imageRendering` | string | `"auto"`, `"pixelated"`, `"crisp-edges"`. Supersedes `pixelated`. |
| `appearance` | string | `"auto"`, `"monochrome"`. Monochrome masks opaque parts with text color. |
| `background` | boolean | Show background color behind image. Default `true`. |
| `collapsed` | boolean | Image collapsed by default. Default `false`. |
| `collapsible` | boolean | Image can be collapsed. Default `true`. |
| `verticalAlign` | string | Same enum as style verticalAlign. |
| `border` | string | CSS border shorthand. |
| `borderRadius` | string | CSS border-radius. |
| `sizeUnits` | string | `"px"` or `"em"`. |

### Link Element Properties (`tag: "a"`)

| Property | Type | Description |
|---|---|---|
| `href` | string (required) | URL. Must match `^(?:https?:|\?)[\w\W]*`. URLs starting with `?` become internal dictionary search links. |
| `content` | Content | Child content for the link text. |
| `lang` | string | Language code (RFC 5646). |

### Workarounds for Missing Tags

Since `<b>`, `<i>`, `<em>`, `<strong>`, `<u>`, `<s>`, `<sub>`, `<sup>` are not available:

| Desired Effect | Workaround |
|---|---|
| Bold | `{"tag": "span", "style": {"fontWeight": "bold"}, "content": "text"}` |
| Italic | `{"tag": "span", "style": {"fontStyle": "italic"}, "content": "text"}` |
| Underline | `{"tag": "span", "style": {"textDecorationLine": "underline"}, "content": "text"}` |
| Strikethrough | `{"tag": "span", "style": {"textDecorationLine": "line-through"}, "content": "text"}` |
| Subscript | `{"tag": "span", "style": {"verticalAlign": "sub", "fontSize": "smaller"}, "content": "text"}` |
| Superscript | `{"tag": "span", "style": {"verticalAlign": "super", "fontSize": "smaller"}, "content": "text"}` |
| Heading-like | `{"tag": "span", "style": {"fontWeight": "bold", "fontSize": "1.2em"}, "content": "text"}` |
| Paragraph spacing | `{"tag": "div", "style": {"marginBottom": 0.5}, "content": "text"}` |
