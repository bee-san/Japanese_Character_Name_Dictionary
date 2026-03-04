# Seiyuu (Voice Actress/Actor) Section — Design Plan

## Goal

Add a new toggleable "Seiyuu" section to the Yomitan popup card that displays voice actor information for each character. This section is independent from the existing "Character Information" traits section and has its own toggle (`seiyuu=true/false`).

## Current State

- The `Character` struct in `models.rs` has no voice actor field.
- VNDB's `POST /character` endpoint does **not** expose voice actor data (the docs explicitly say "Missing: voice actor").
- Voice actor data on VNDB is available via the `POST /vn` endpoint's `va` field, which maps staff (voice actors) to characters for a given VN.
- AniList's `CharacterEdge` type has a `voiceActors(language: StaffLanguage)` field that returns `[Staff]`, each with a `name { full, native }` object.
- The current code does not fetch VA data from either source.

## Data Sources

### VNDB — `POST /vn` with `va` field

The `va` field on the VN endpoint returns an array of objects, each linking a voice actor (staff) to a character:

```
POST https://api.vndb.org/kana/vn
{
    "filters": ["id", "=", "v17"],
    "fields": "va.staff.id,va.staff.name,va.staff.original,va.character.id"
}
```

Response shape:
```json
{
    "results": [{
        "id": "v17",
        "va": [
            {
                "staff": { "id": "s123", "name": "Hanazawa Kana", "original": "花澤香菜" },
                "character": { "id": "c45" }
            },
            ...
        ]
    }]
}
```

The same character may appear multiple times if voiced by different actors (e.g. different editions). The same actor may appear multiple times for different characters.

**Key detail:** `va.staff.original` gives the Japanese name (e.g. "花澤香菜"), `va.staff.name` gives the romanized name (e.g. "Hanazawa Kana"). We want to display the Japanese name primarily, with romanized as fallback.

### AniList — `voiceActors` on `CharacterEdge`

Add `voiceActors(language: JAPANESE)` to the existing `CHARACTERS_QUERY`:

```graphql
edges {
    role
    voiceActors(language: JAPANESE) {
        name {
            full
            native
        }
    }
    node {
        # ... existing fields ...
    }
}
```

Response shape per edge:
```json
{
    "role": "MAIN",
    "voiceActors": [
        { "name": { "full": "Kana Hanazawa", "native": "花澤香菜" } }
    ],
    "node": { ... }
}
```

Prefer `native` (Japanese name), fall back to `full` (romanized).

## Design

### 1. Model Changes (`models.rs`)

Add a single field to `Character`:

```rust
pub struct Character {
    // ... existing fields ...
    pub seiyuu: Option<String>,  // Voice actor name, e.g. "花澤香菜" or "Hanazawa Kana"
}
```

Just a display string. No need for a complex struct — we only show the name in the popup card.

### 2. VNDB Client Changes (`vndb_client.rs`)

Voice actor data is NOT on the character endpoint. Two options:

#### Option A: Fetch `va` alongside `fetch_vn_title` (chosen)

The `fetch_vn_title` function already calls `POST /vn` for the target VN. Extend it to also request `va.staff.name,va.staff.original,va.character.id` and return a `HashMap<String, String>` mapping character ID → VA display name.

Rename or extend `fetch_vn_title` to return the VA map alongside the title:

```rust
pub async fn fetch_vn_info(&self, vn_id: &str) -> Result<VnInfo, String> {
    let payload = serde_json::json!({
        "filters": ["id", "=", &vn_id],
        "fields": "title,alttitle,va.staff.name,va.staff.original,va.character.id"
    });
    // ... single API call ...
}

pub struct VnInfo {
    pub title: String,        // romanized
    pub alttitle: String,     // Japanese
    pub va_map: HashMap<String, String>,  // character_id → VA display name
}
```

Build the `va_map` by iterating the `va` array:
```rust
let mut va_map = HashMap::new();
if let Some(va_arr) = vn["va"].as_array() {
    for entry in va_arr {
        let char_id = entry["character"]["id"].as_str().unwrap_or("");
        let va_name = entry["staff"]["original"].as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| entry["staff"]["name"].as_str())
            .unwrap_or("")
            .to_string();
        if !char_id.is_empty() && !va_name.is_empty() {
            // First VA wins (don't overwrite if character has multiple VAs)
            va_map.entry(char_id.to_string()).or_insert(va_name);
        }
    }
}
```

After `fetch_characters` returns, iterate all characters and assign `seiyuu` from the VA map:
```rust
for char in char_data.all_characters_mut() {
    if let Some(va_name) = va_map.get(&char.id) {
        char.seiyuu = Some(va_name.clone());
    }
}
```

**Why not Option B (separate API call)?** Adding a second VN endpoint call just for VA data doubles the VN API requests. Since `fetch_vn_title` already hits the VN endpoint, bundling `va` fields into the same request is free.

**Why not fetch from the character endpoint?** The VNDB character endpoint explicitly does not support voice actor data.

#### Backward compatibility for `fetch_vn_title`

The existing function signature `fetch_vn_title(&self, vn_id: &str) -> Result<(String, String), String>` is used in `main.rs`. Change it to return `VnInfo` and update the call sites. This is a mechanical refactor — the callers just destructure differently.

### 3. AniList Client Changes (`anilist_client.rs`)

**GraphQL query:** Add `voiceActors` to `CHARACTERS_QUERY`:

```graphql
edges {
    role
    voiceActors(language: JAPANESE) {
        name {
            full
            native
        }
    }
    node {
        # ... existing fields unchanged ...
    }
}
```

**`process_character`:** Extract VA name from the edge (not the node):

```rust
fn process_character(&self, edge: &serde_json::Value) -> Option<Character> {
    // ... existing code ...

    let seiyuu = edge["voiceActors"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|va| {
            va["name"]["native"].as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| va["name"]["full"].as_str())
        })
        .map(|s| s.to_string());

    Some(Character {
        // ... existing fields ...
        seiyuu,
    })
}
```

Note: `voiceActors(language: JAPANESE)` filters server-side, so we only get Japanese VAs. If the array is empty (no Japanese VA), `seiyuu` will be `None`. Taking the first entry is sufficient — multiple Japanese VAs for the same character in the same media is extremely rare.

### 4. DictSettings & Toggle (`content_builder.rs`, `main.rs`)

Add a `show_seiyuu: bool` field to `DictSettings`:

```rust
pub struct DictSettings {
    pub show_image: bool,
    pub show_tag: bool,
    pub show_description: bool,
    pub show_traits: bool,
    pub show_spoilers: bool,
    pub honorifics: bool,
    pub show_seiyuu: bool,  // NEW
}
```

Default: `true` (shown by default).

### 5. Query Parameter (`main.rs`)

Add `seiyuu: bool` to both `DictQuery` and `GenerateStreamQuery`:

```rust
#[serde(default = "default_true")]
seiyuu: bool,
```

Wire through `to_settings()`:
```rust
fn to_settings(&self) -> DictSettings {
    DictSettings {
        // ... existing ...
        show_seiyuu: self.seiyuu,
    }
}
```

Wire through `append_settings_params()`:
```rust
if !self.seiyuu {
    parts.push("seiyuu=false".to_string());
}
```

URL example: `&seiyuu=false` to hide the section. Default (omitted) = shown.

### 6. Content Builder — Rendering (`content_builder.rs`)

Add a new `<details>` section in `build_content()`, placed after the "Character Information" section and before the closing of the content array:

```rust
// ===== Seiyuu section (gated by show_seiyuu) =====
if self.settings.show_seiyuu {
    if let Some(ref va) = char.seiyuu {
        if !va.is_empty() {
            content.push(json!({
                "tag": "details",
                "content": [
                    { "tag": "summary", "content": "Seiyuu" },
                    {
                        "tag": "div",
                        "style": { "fontSize": "0.9em", "marginTop": "4px" },
                        "content": va.as_str()
                    }
                ]
            }));
        }
    }
}
```

This renders as a collapsible "Seiyuu" section in the Yomitan popup, consistent with the existing "Description" and "Character Information" sections.

### 7. Frontend Changes (`static/index.html`, `static/app.js`)

**`app.js` — settings object:**
```javascript
const settings = {
    honorifics: true,
    image: true,
    tag: true,
    description: true,
    traits: true,
    spoilers: true,
    seiyuu: true,  // NEW
};
```

**`app.js` — `updatePreviewCard()`:** Add `seiyuu` to the sections map:
```javascript
const sections = {
    // ... existing ...
    seiyuu: document.getElementById('preview-seiyuu'),
};
```

**`app.js` — `settingsParams()`:**
```javascript
if (!settings.seiyuu) parts.push('seiyuu=false');
```

**`index.html` — preview card:** Add a new toggleable section after `preview-traits`:

```html
<!-- Seiyuu [toggleable] -->
<div id="preview-seiyuu" class="toggle-section">
    <span class="toggle-btn" onclick="toggleSetting('seiyuu')" title="Click to disable">❌</span>
    <div class="yomitan-details">
        <div class="yomitan-details-summary">&#9660; Seiyuu</div>
        <div class="yomitan-details-content">
            島島博士 (Shimashima Hakase)
        </div>
    </div>
</div>
```

### 8. Media Cache Compatibility (`media_cache.rs`)

The media cache stores serialized `CharacterData`. Adding `seiyuu: Option<String>` to `Character` is backward-compatible with `serde_json` deserialization — cached entries without the field will deserialize `seiyuu` as `None`. No cache migration needed, but cached entries won't have VA data until they expire and are re-fetched.

For VNDB specifically, the VA data comes from the VN endpoint (not cached with character data), so it's applied after cache retrieval. This means even cache hits will get VA data as long as the VN info fetch includes it.

## Implementation Steps

1. Add `seiyuu: Option<String>` to `Character` in `models.rs`, defaulting to `None` in all existing constructors.
2. Create `VnInfo` struct in `vndb_client.rs`. Refactor `fetch_vn_title` → `fetch_vn_info` to also request `va.staff.name,va.staff.original,va.character.id` and return `VnInfo`.
3. In `main.rs`, after fetching VNDB characters, apply the VA map from `VnInfo` to populate `seiyuu` on each character.
4. In `anilist_client.rs`, add `voiceActors(language: JAPANESE) { name { full native } }` to `CHARACTERS_QUERY` and extract `seiyuu` in `process_character`.
5. Add `show_seiyuu: bool` to `DictSettings` (default `true`).
6. Add `seiyuu: bool` to `DictQuery` and `GenerateStreamQuery`, wire through `to_settings()` and `append_settings_params()`.
7. Add the "Seiyuu" `<details>` section to `build_content()` in `content_builder.rs`, gated by `show_seiyuu`.
8. Add the toggle to the frontend: `settings.seiyuu`, `preview-seiyuu` section in HTML, `settingsParams()` update.
9. Update all tests that construct `Character` or `DictSettings` to include the new fields.
10. Test against live VNDB API to verify `va` field structure on the VN endpoint.
11. Test against live AniList API to verify `voiceActors` response on character edges.

## Compatibility Notes

- `seiyuu` defaults to `true`, so existing URLs are unaffected — VA data simply appears for the first time.
- Users can hide it with `&seiyuu=false`, which survives the Yomitan update cycle.
- The `seiyuu` section is completely independent from `traits` — `traits=false` does NOT hide seiyuu, and `seiyuu=false` does NOT hide traits.
- AniList characters that have no Japanese VA (e.g. some manga-only entries) will simply not show the section.
- VNDB characters whose VN has no voice acting will not show the section.

## Open Questions

1. **Multiple VAs per character:** Some characters are voiced by different actors in different editions/releases. The current design takes the first VA found. Should we show all of them (comma-separated)? Recommendation: start with first-only, revisit if users request it.
2. **VNDB `va` field pagination:** If a VN has hundreds of VA entries, does the `va` array get truncated? The VNDB API docs don't mention pagination for nested fields. Needs testing with a large VN (e.g. a long-running series). If truncated, we may need to handle `more` on the VN response.
3. **Display format:** Currently just the name string. Could later be enhanced with a link to the VA's VNDB/AniList page using the `<a>` tag in structured content, but that adds complexity and the URLs would need to be stored alongside the name.
