# Yomitan Character Dictionary Builder — Complete Implementation Plan

**Project:** Standalone Website for Generating Yomitan Character Dictionaries
**Architecture:** Rust (Axum) Server + Simple HTML/CSS/JS Frontend
**Target:** A third-party developer with no access to the original Python codebase

---

## TABLE OF CONTENTS

1. Project Overview & User Flow
2. Architecture & File Structure
3. Cargo.toml (Complete)
4. Shared Data Model — `models.rs`
5. VNDB API Client — `vndb_client.rs`
6. AniList API Client — `anilist_client.rs`
7. Name Parser — `name_parser.rs` (romaji→kana, name splitting, honorifics)
8. Content Builder — `content_builder.rs` (Yomitan structured content JSON)
9. Image Handler — `image_handler.rs`
10. Dictionary Builder — `dict_builder.rs` (ZIP assembly orchestrator)
11. Server — `main.rs` (Axum routes)
12. Frontend — `static/index.html`
13. Yomitan ZIP Format Specification
14. Test Expectations & Verification
15. Critical Implementation Notes
16. Deployment

---

## 1. PROJECT OVERVIEW & USER FLOW

### What This Builds

A web application that generates **Yomitan-compatible character dictionaries** from VNDB (Visual Novel Database) and AniList (Anime/Manga). When installed in Yomitan (a browser dictionary extension for Japanese), looking up a character's Japanese name shows a rich popup card with their portrait, role, stats, and description.

### End-to-End User Flow

```
1. User opens http://localhost:3000 in browser
2. User selects source (VNDB or AniList), enters media ID (e.g., "v17" or "9253"),
   selects media type (for AniList: Anime or Manga), and selects spoiler level (0/1/2)
3. User clicks "Generate Dictionary"
4. Browser sends GET /api/yomitan-dict?source=vndb&id=v17&spoiler_level=0
5. Server:
   a. Calls VNDB/AniList API to fetch all characters (with automatic pagination)
   b. Fetches VN/media title from the same API
   c. Downloads each character's portrait image, resizes to 80×100 thumbnail, encodes as base64
   d. For each character: parses Japanese name → hiragana readings, builds structured content card
   e. For each character: creates multiple term entries (family, given, combined, with-space,
      honorific suffixes, aliases, alias honorific suffixes)
   f. Assembles all entries + images into a ZIP file in memory
   g. Returns ZIP as application/zip response
6. Browser triggers file download
7. User imports ZIP into Yomitan
8. When reading Japanese text, hovering over a character name shows the card popup
```

### Key Feature: Auto-Update URLs

The ZIP's `index.json` contains a `downloadUrl` pointing back to this server with the same query parameters. Yomitan periodically re-fetches that URL to get an updated dictionary. There is also an `indexUrl` endpoint that returns lightweight JSON metadata for Yomitan to check if an update is available (compares revision strings).

### What Each Entry Looks Like in Yomitan

For a character named "須々木 心一" (Suzuki Shinichi), the dictionary produces these term entries:

| Term | Reading | Scenario |
|---|---|---|
| `須々木 心一` | `すずきしんいち` | Full name with space |
| `須々木心一` | `すずきしんいち` | Combined without space |
| `須々木` | `すずき` | Family name only |
| `心一` | `しんいち` | Given name only |
| `須々木さん` | `すずきさん` | Family + honorific (×15 honorifics) |
| `心一さん` | `しんいちさん` | Given + honorific (×15 honorifics) |
| `須々木心一さん` | `すずきしんいちさん` | Combined + honorific (×15 honorifics) |
| `須々木 心一さん` | `すずきしんいちさん` | Original + honorific (×15 honorifics) |
| `しんいち` | `しんいち` | Alias entry (if "しんいち" is in aliases) |
| `しんいちさん` | `しんいちさん` | Alias + honorific |

All entries share the same structured content card — only the term and reading differ.

---

## 2. ARCHITECTURE & FILE STRUCTURE

```
yomitan-dict-builder/
├── Cargo.toml
├── src/
│   ├── main.rs              # Axum server, routes, CLI
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

---

## 3. COMPLETE Cargo.toml

```toml
[package]
name = "yomitan-dict-builder"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
zip = { version = "2", default-features = false, features = ["deflate"] }
base64 = "0.22"
tower-http = { version = "0.5", features = ["fs", "cors"] }
rand = "0.8"
regex = "1"
```

**Note on ZIP writing:** The `zip` crate's `ZipWriter` requires `Write + Seek`. A plain `Vec<u8>` does not implement `Seek`. You must use `std::io::Cursor<Vec<u8>>` as the backing buffer.

---

## 4. SHARED DATA MODEL — `models.rs`

```rust
use serde::{Deserialize, Serialize};

/// A trait with spoiler metadata.
/// Represents entries like: {"name": "Kind", "spoiler": 0}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterTrait {
    pub name: String,
    pub spoiler: u8, // 0=none, 1=minor, 2=major
}

/// Normalized character data. Both VNDB and AniList clients produce this format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub name: String,              // Romanized (Western order for VNDB: "Given Family")
    pub name_original: String,     // Japanese (Japanese order: "Family Given")
    pub role: String,              // "main", "primary", "side", "appears"
    pub sex: Option<String>,       // "m" or "f"
    pub age: Option<String>,       // String because AniList may return "17-18"
    pub height: Option<u32>,       // cm (VNDB only; None for AniList)
    pub weight: Option<u32>,       // kg (VNDB only; None for AniList)
    pub blood_type: Option<String>,
    pub birthday: Option<Vec<u32>>, // [month, day]
    pub description: Option<String>,
    pub aliases: Vec<String>,
    pub personality: Vec<CharacterTrait>,
    pub roles: Vec<CharacterTrait>,
    pub engages_in: Vec<CharacterTrait>,
    pub subject_of: Vec<CharacterTrait>,
    pub image_url: Option<String>,     // Raw URL from API (used for downloading)
    pub image_base64: Option<String>,  // "data:image/jpeg;base64,..." after download
}

/// Categorized characters for a single game/media.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterData {
    pub main: Vec<Character>,
    pub primary: Vec<Character>,
    pub side: Vec<Character>,
    pub appears: Vec<Character>,
}

impl CharacterData {
    pub fn new() -> Self {
        Self {
            main: Vec::new(),
            primary: Vec::new(),
            side: Vec::new(),
            appears: Vec::new(),
        }
    }

    /// Iterate over all characters across all role categories.
    pub fn all_characters(&self) -> impl Iterator<Item = &Character> {
        self.main
            .iter()
            .chain(self.primary.iter())
            .chain(self.side.iter())
            .chain(self.appears.iter())
    }

    /// Mutable iterator (used for populating image_base64 after download).
    pub fn all_characters_mut(&mut self) -> impl Iterator<Item = &mut Character> {
        self.main
            .iter_mut()
            .chain(self.primary.iter_mut())
            .chain(self.side.iter_mut())
            .chain(self.appears.iter_mut())
    }
}
```

---

## 5. VNDB API CLIENT — `vndb_client.rs`

### API Details

- **Base URL:** `https://api.vndb.org/kana`
- **No authentication required.**
- **Rate limit:** 200 requests per 5 minutes. Add ~200ms delay between paginated requests.
- **All requests are POST with JSON body.**

### 5.1 VN ID Normalization

Accepts "17", "v17", "V17" → always returns "v17".

```rust
pub fn normalize_id(id: &str) -> String {
    let id = id.trim();
    if id.to_lowercase().starts_with('v') {
        format!("v{}", &id[1..])
    } else {
        format!("v{}", id)
    }
}
```

### 5.2 VN Title Lookup

**Endpoint:** `POST https://api.vndb.org/kana/vn`

```rust
/// Fetch the VN's title. Returns (romaji_title, original_japanese_title).
pub async fn fetch_vn_title(&self, vn_id: &str) -> Result<(String, String), String> {
    let vn_id = Self::normalize_id(vn_id);
    let payload = serde_json::json!({
        "filters": ["id", "=", &vn_id],
        "fields": "title, alttitle"
    });

    let response = self.client
        .post("https://api.vndb.org/kana/vn")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if response.status() != 200 {
        return Err(format!("VNDB VN API returned status {}", response.status()));
    }

    let data: serde_json::Value = response.json().await
        .map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let results = data["results"].as_array().ok_or("No results")?;
    if results.is_empty() {
        return Err("VN not found".to_string());
    }

    let vn = &results[0];
    let title = vn["title"].as_str().unwrap_or("").to_string();     // Romanized
    let alttitle = vn["alttitle"].as_str().unwrap_or("").to_string(); // Japanese original
    Ok((title, alttitle))
}
```

### 5.3 Character Fetching with Pagination

**Endpoint:** `POST https://api.vndb.org/kana/character`

**Request body:**
```json
{
    "filters": ["vn", "=", ["id", "=", "v17"]],
    "fields": "id,name,original,image.url,sex,birthday,age,blood_type,height,weight,description,aliases,vns.role,vns.id,traits.name,traits.group_name,traits.spoiler",
    "results": 100,
    "page": 1
}
```

**Response shape:**
```json
{
    "results": [
        {
            "id": "c123",
            "name": "Shinichi Suzuki",
            "original": "須々木 心一",
            "image": { "url": "https://t.vndb.org/ch/12/34567.jpg" },
            "sex": ["m"],
            "birthday": [9, 1],
            "age": 17,
            "blood_type": "A",
            "height": 165,
            "weight": 50,
            "description": "The protagonist.\n[spoiler]Secret info[/spoiler]",
            "aliases": ["しんいち"],
            "vns": [{ "id": "v17", "role": "main" }],
            "traits": [
                { "name": "Kind", "group_name": "Personality", "spoiler": 0 },
                { "name": "Student", "group_name": "Role", "spoiler": 0 }
            ]
        }
    ],
    "more": true
}
```

**Pagination:** Loop while `"more": true`, incrementing `page`. Add 200ms sleep between pages.

**Field notes:**
- `name`: Romanized, **Western order** ("Given Family")
- `original`: Japanese, **Japanese order** ("Family Given")
- `sex`: Array like `["m"]` or `["f"]` — take first element. Can be null.
- `birthday`: Array `[month, day]` or null
- `age`: Integer or null
- `height`, `weight`: Integer or null
- `traits[].group_name`: One of `"Personality"`, `"Role"`, `"Engages in"`, `"Subject of"`, or other values to ignore
- `traits[].spoiler`: Integer 0, 1, or 2

### 5.4 Complete Single Character Processing

```rust
/// Process a single raw VNDB character JSON value into our Character struct.
fn process_character(&self, data: &serde_json::Value, target_vn: &str) -> Option<Character> {
    // Find role for this specific VN
    let role = data["vns"]
        .as_array()?
        .iter()
        .find(|v| v["id"].as_str() == Some(target_vn))
        .and_then(|v| v["role"].as_str())
        .unwrap_or("side")
        .to_string();

    // Extract sex from array format: ["m"] → "m"
    let sex = data["sex"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Process traits by group_name
    let empty_vec = vec![];
    let traits = data["traits"].as_array().unwrap_or(&empty_vec);
    let mut personality = Vec::new();
    let mut roles = Vec::new();
    let mut engages_in = Vec::new();
    let mut subject_of = Vec::new();

    for trait_data in traits {
        let name = trait_data["name"].as_str().unwrap_or("").to_string();
        let spoiler = trait_data["spoiler"].as_u64().unwrap_or(0) as u8;
        let group = trait_data["group_name"].as_str().unwrap_or("");

        if name.is_empty() {
            continue;
        }

        let trait_obj = CharacterTrait { name, spoiler };

        match group {
            "Personality" => personality.push(trait_obj),
            "Role" => roles.push(trait_obj),
            "Engages in" => engages_in.push(trait_obj),
            "Subject of" => subject_of.push(trait_obj),
            _ => {} // Ignore other groups
        }
    }

    // Image URL (nested: {"image": {"url": "..."}})
    let image_url = data["image"]["url"].as_str().map(|s| s.to_string());

    // Birthday: [month, day] array
    let birthday = data["birthday"].as_array().and_then(|arr| {
        if arr.len() >= 2 {
            Some(vec![arr[0].as_u64()? as u32, arr[1].as_u64()? as u32])
        } else {
            None
        }
    });

    // Aliases: array of strings
    let aliases = data["aliases"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Some(Character {
        id: data["id"].as_str().unwrap_or("").to_string(),
        name: data["name"].as_str().unwrap_or("").to_string(),
        name_original: data["original"].as_str().unwrap_or("").to_string(),
        role,
        sex,
        age: data["age"].as_u64().map(|a| a.to_string()),
        height: data["height"].as_u64().map(|h| h as u32),
        weight: data["weight"].as_u64().map(|w| w as u32),
        blood_type: data["blood_type"].as_str().map(|s| s.to_string()),
        birthday,
        description: data["description"].as_str().map(|s| s.to_string()),
        aliases,
        personality,
        roles,
        engages_in,
        subject_of,
        image_url,
        image_base64: None, // Populated later in a separate pass
    })
}
```

### 5.5 Complete fetch_characters (with pagination)

```rust
pub async fn fetch_characters(&self, vn_id: &str) -> Result<CharacterData, String> {
    let vn_id = Self::normalize_id(vn_id);
    let mut char_data = CharacterData::new();
    let mut page = 1;

    loop {
        let payload = serde_json::json!({
            "filters": ["vn", "=", ["id", "=", &vn_id]],
            "fields": "id,name,original,image.url,sex,birthday,age,blood_type,height,weight,description,aliases,vns.role,vns.id,traits.name,traits.group_name,traits.spoiler",
            "results": 100,
            "page": page
        });

        let response = self.client
            .post("https://api.vndb.org/kana/character")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != 200 {
            return Err(format!("VNDB API returned status {}", response.status()));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        let results = data["results"].as_array().ok_or("Invalid response format")?;

        for char_json in results {
            if let Some(character) = self.process_character(char_json, &vn_id) {
                match character.role.as_str() {
                    "main" => char_data.main.push(character),
                    "primary" => char_data.primary.push(character),
                    "side" => char_data.side.push(character),
                    "appears" => char_data.appears.push(character),
                    _ => char_data.side.push(character),
                }
            }
        }

        if !data["more"].as_bool().unwrap_or(false) {
            break;
        }

        page += 1;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    Ok(char_data)
}
```

### 5.6 Image Download + Base64 Encoding

Downloads the image as-is and encodes to base64 with a data URI prefix. No resizing is done server-side (the images from VNDB CDN are already small thumbnails). If you want to add resizing, use the `image` crate.

```rust
/// Download an image and return as base64 data URI string.
/// Returns None on any failure (network, non-200 status, etc.).
pub async fn fetch_image_as_base64(&self, url: &str) -> Option<String> {
    let response = self.client.get(url).send().await.ok()?;

    if response.status() != 200 {
        return None;
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let bytes = response.bytes().await.ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{};base64,{}", content_type, b64))
}
```

### 5.7 Full VndbClient struct

```rust
use reqwest::Client;
use crate::models::*;

pub struct VndbClient {
    client: Client,
}

impl VndbClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    // All methods from sections 5.1–5.6 go here
}
```

---

## 6. ANILIST API CLIENT — `anilist_client.rs`

### API Details

- **Endpoint:** `POST https://graphql.anilist.co`
- **No authentication required.**
- **Rate limit:** 90 requests per minute. Add ~300ms delay between paginated requests.
- **Uses GraphQL.**

### 6.1 GraphQL Query

```graphql
query ($id: Int!, $type: MediaType, $page: Int, $perPage: Int) {
    Media(id: $id, type: $type) {
        id
        title {
            romaji
            english
            native
        }
        characters(page: $page, perPage: $perPage, sort: [ROLE, RELEVANCE, ID]) {
            pageInfo {
                hasNextPage
                currentPage
            }
            edges {
                role
                node {
                    id
                    name {
                        full
                        native
                        alternative
                    }
                    image {
                        large
                    }
                    description
                    gender
                    age
                    dateOfBirth {
                        month
                        day
                    }
                    bloodType
                }
            }
        }
    }
}
```

**Variables:**
```json
{ "id": 9253, "type": "ANIME", "page": 1, "perPage": 25 }
```

**`type` must be `"ANIME"` or `"MANGA"` (uppercase string).**

### 6.2 Role Mapping

| AniList Role | Our Role |
|---|---|
| `MAIN` | `"main"` |
| `SUPPORTING` | `"primary"` |
| `BACKGROUND` | `"side"` |
| (anything else) | `"side"` |

### 6.3 Differences from VNDB

- **Spoiler format:** AniList uses `~!spoiler text!~` instead of VNDB's `[spoiler]...[/spoiler]`
- **Missing fields:** AniList has NO height, weight, or trait categories (personality/roles/engages_in/subject_of are all empty `Vec`)
- **Gender:** AniList returns a string like `"Male"` or `"Female"` — map first char: `'m' → "m"`, `'f' → "f"`
- **Age:** AniList returns age as a **string** (can be `"17"`, `"17-18"`, `null`) — store as `Option<String>` directly
- **Name order:** AniList `name.full` is already the romanized full name; `name.native` is the Japanese name. No order swapping issues specific to AniList (the name_parser handles VNDB's Western-order romaji).

### 6.4 Title Extraction

The title comes from the first page's response at `data.Media.title`. Prefer `native` (Japanese), fall back to `romaji`, then `english`.

```rust
// Extract title from first page response
let title_data = &data["data"]["Media"]["title"];
let media_title = title_data["native"].as_str()
    .or_else(|| title_data["romaji"].as_str())
    .or_else(|| title_data["english"].as_str())
    .unwrap_or("")
    .to_string();
```

### 6.5 Complete AniList Character Processing

```rust
fn process_character(&self, edge: &serde_json::Value) -> Option<Character> {
    let node = edge.get("node")?;
    let role_raw = edge["role"].as_str().unwrap_or("BACKGROUND");

    let role = match role_raw {
        "MAIN" => "main",
        "SUPPORTING" => "primary",
        "BACKGROUND" => "side",
        _ => "side",
    }
    .to_string();

    let name_data = node.get("name")?;
    let name_full = name_data["full"].as_str().unwrap_or("").to_string();
    let name_native = name_data["native"].as_str().unwrap_or("").to_string();

    let alternatives: Vec<String> = name_data["alternative"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Gender: "Male" → "m", "Female" → "f"
    let sex = node
        .get("gender")
        .and_then(|g| g.as_str())
        .and_then(|g| match g.to_lowercase().chars().next() {
            Some('m') => Some("m".to_string()),
            Some('f') => Some("f".to_string()),
            _ => None,
        });

    // Birthday: {"month": 9, "day": 1} → [9, 1]
    let birthday = node.get("dateOfBirth").and_then(|dob| {
        let month = dob["month"].as_u64()? as u32;
        let day = dob["day"].as_u64()? as u32;
        Some(vec![month, day])
    });

    // Image URL
    let image_url = node
        .get("image")
        .and_then(|img| img["large"].as_str())
        .map(|s| s.to_string());

    // Age — AniList returns as string, may be "17-18" or similar
    let age = node
        .get("age")
        .and_then(|v| {
            // Try string first, then integer
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        });

    Some(Character {
        id: node
            .get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .to_string(),
        name: name_full,
        name_original: name_native,
        role,
        sex,
        age,
        height: None,      // AniList doesn't provide
        weight: None,      // AniList doesn't provide
        blood_type: node
            .get("bloodType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        birthday,
        description: node
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        aliases: alternatives,
        personality: Vec::new(),  // AniList has no trait categories
        roles: Vec::new(),
        engages_in: Vec::new(),
        subject_of: Vec::new(),
        image_url,
        image_base64: None,
    })
}
```

### 6.6 Complete fetch_characters (with pagination + title)

```rust
/// Fetch all characters and the media title.
/// media_type must be "ANIME" or "MANGA".
pub async fn fetch_characters(
    &self,
    media_id: i32,
    media_type: &str,
) -> Result<(CharacterData, String), String> {
    let mut char_data = CharacterData::new();
    let mut page = 1;
    let mut media_title = String::new();

    loop {
        let variables = serde_json::json!({
            "id": media_id,
            "type": media_type.to_uppercase(),
            "page": page,
            "perPage": 25
        });

        let response = self.client
            .post("https://graphql.anilist.co")
            .json(&serde_json::json!({
                "query": Self::CHARACTERS_QUERY,
                "variables": variables
            }))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != 200 {
            return Err(format!("AniList API returned status {}", response.status()));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        if data["errors"].is_array() {
            return Err(format!("GraphQL error: {:?}", data["errors"]));
        }

        let media = &data["data"]["Media"];

        // Extract title on first page
        if page == 1 {
            let title_data = &media["title"];
            media_title = title_data["native"]
                .as_str()
                .or_else(|| title_data["romaji"].as_str())
                .or_else(|| title_data["english"].as_str())
                .unwrap_or("")
                .to_string();
        }

        let edges = media["characters"]["edges"]
            .as_array()
            .ok_or("Invalid response format")?;

        for edge in edges {
            if let Some(character) = self.process_character(edge) {
                match character.role.as_str() {
                    "main" => char_data.main.push(character),
                    "primary" => char_data.primary.push(character),
                    "side" => char_data.side.push(character),
                    _ => char_data.side.push(character),
                }
            }
        }

        let has_next = media["characters"]["pageInfo"]["hasNextPage"]
            .as_bool()
            .unwrap_or(false);

        if !has_next {
            break;
        }

        page += 1;
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    }

    Ok((char_data, media_title))
}
```

### 6.7 Complete AnilistClient struct

```rust
use reqwest::Client;
use crate::models::*;

pub struct AnilistClient {
    client: Client,
}

impl AnilistClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    const CHARACTERS_QUERY: &'static str = r#"
    query ($id: Int!, $type: MediaType, $page: Int, $perPage: Int) {
        Media(id: $id, type: $type) {
            id
            title {
                romaji
                english
                native
            }
            characters(page: $page, perPage: $perPage, sort: [ROLE, RELEVANCE, ID]) {
                pageInfo {
                    hasNextPage
                    currentPage
                }
                edges {
                    role
                    node {
                        id
                        name {
                            full
                            native
                            alternative
                        }
                        image {
                            large
                        }
                        description
                        gender
                        age
                        dateOfBirth {
                            month
                            day
                        }
                        bloodType
                    }
                }
            }
        }
    }
    "#;

    // fetch_image_as_base64: same implementation as VndbClient (section 5.6)
    // process_character: section 6.5
    // fetch_characters: section 6.6
}
```

---

## 7. NAME PARSER — `name_parser.rs`

This is the most complex module. It handles:

1. Detecting if text contains kanji
2. Splitting Japanese names by space (family/given)
3. Converting romanized text to hiragana (romaji → kana)
4. Converting katakana to hiragana
5. Mixed name handling (per-part kanji/kana detection)
6. Honorific suffix constants

### 7.1 Kanji Detection

```rust
/// Check if text contains kanji characters.
/// Unicode ranges: CJK Unified Ideographs (0x4E00–0x9FFF) + Extension A (0x3400–0x4DBF).
pub fn contains_kanji(text: &str) -> bool {
    text.chars().any(|c| {
        let code = c as u32;
        (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code)
    })
}
```

### 7.2 Split Japanese Name

Japanese names from VNDB are stored as "FamilyName GivenName" with a space. Split on the **first** space only.

```rust
/// Returns (family, given, combined, original, has_space)
pub fn split_japanese_name(name_original: &str) -> JapaneseNameParts {
    if name_original.is_empty() || !name_original.contains(' ') {
        return JapaneseNameParts {
            has_space: false,
            original: name_original.to_string(),
            combined: name_original.to_string(),
            family: None,
            given: None,
        };
    }

    // Split on first space only
    let pos = name_original.find(' ').unwrap();
    let family = name_original[..pos].to_string();
    let given = name_original[pos + 1..].to_string();
    let combined = format!("{}{}", family, given);

    JapaneseNameParts {
        has_space: true,
        original: name_original.to_string(),
        combined,
        family: Some(family),
        given: Some(given),
    }
}

/// Name parts result.
#[derive(Debug, Clone)]
pub struct JapaneseNameParts {
    pub has_space: bool,
    pub original: String,
    pub combined: String,
    pub family: Option<String>,
    pub given: Option<String>,
}
```

### 7.3 Katakana → Hiragana

Katakana range: U+30A1 (ァ) to U+30F6 (ヶ). Subtract 0x60 to get hiragana equivalent.

```rust
pub fn kata_to_hira(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (0x30A1..=0x30F6).contains(&code) {
                char::from_u32(code - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}
```

### 7.4 Romaji → Hiragana Conversion

This is the Rust port of Python's `jaconv.alphabet2kana()`. Match longest sequences first. Handle double consonants (っ) and the special `n` rule.

```rust
pub fn alphabet_to_kana(input: &str) -> String {
    let text = input.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < chars.len() {
        // 1. Double consonant check: if chars[i] == chars[i+1] and both are consonants → っ
        if i + 1 < chars.len()
            && chars[i] == chars[i + 1]
            && is_consonant(chars[i])
        {
            result.push('っ');
            i += 1; // Skip one; the second consonant starts the next match
            continue;
        }

        // 2. Try 3-character sequence
        if i + 3 <= chars.len() {
            let three: String = chars[i..i + 3].iter().collect();
            if let Some(kana) = lookup_romaji(&three) {
                result.push_str(kana);
                i += 3;
                continue;
            }
        }

        // 3. Try 2-character sequence
        if i + 2 <= chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            if let Some(kana) = lookup_romaji(&two) {
                result.push_str(kana);
                i += 2;
                continue;
            }
        }

        // 4. Special 'n' handling: ん only when NOT followed by a vowel or 'y'
        if chars[i] == 'n' {
            let next = chars.get(i + 1).copied();
            if next.is_none() || !is_vowel_or_y(next.unwrap()) {
                result.push('ん');
                i += 1;
                continue;
            }
        }

        // 5. Try 1-character sequence (vowels)
        let one = chars[i].to_string();
        if let Some(kana) = lookup_romaji(&one) {
            result.push_str(kana);
        } else {
            // Unknown character — pass through unchanged
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

fn is_consonant(c: char) -> bool {
    matches!(
        c,
        'b' | 'c' | 'd' | 'f' | 'g' | 'h' | 'j' | 'k' | 'l' | 'm' | 'n' | 'p' | 'q'
            | 'r' | 's' | 't' | 'v' | 'w' | 'x' | 'y' | 'z'
    )
}

fn is_vowel_or_y(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o' | 'y')
}
```

### 7.5 Romaji Lookup Table

Build this as a function returning a HashMap, or as a static lookup. **Order does not matter** — the algorithm tries 3-char, then 2-char, then 1-char.

```rust
fn lookup_romaji(key: &str) -> Option<&'static str> {
    // Using a match for clarity. You can also use a lazy_static HashMap.
    match key {
        // === 3-character sequences ===
        "sha" => Some("しゃ"), "shi" => Some("し"),  "shu" => Some("しゅ"), "sho" => Some("しょ"),
        "chi" => Some("ち"),   "tsu" => Some("つ"),
        "cha" => Some("ちゃ"), "chu" => Some("ちゅ"), "cho" => Some("ちょ"),
        "nya" => Some("にゃ"), "nyu" => Some("にゅ"), "nyo" => Some("にょ"),
        "hya" => Some("ひゃ"), "hyu" => Some("ひゅ"), "hyo" => Some("ひょ"),
        "mya" => Some("みゃ"), "myu" => Some("みゅ"), "myo" => Some("みょ"),
        "rya" => Some("りゃ"), "ryu" => Some("りゅ"), "ryo" => Some("りょ"),
        "gya" => Some("ぎゃ"), "gyu" => Some("ぎゅ"), "gyo" => Some("ぎょ"),
        "bya" => Some("びゃ"), "byu" => Some("びゅ"), "byo" => Some("びょ"),
        "pya" => Some("ぴゃ"), "pyu" => Some("ぴゅ"), "pyo" => Some("ぴょ"),
        "kya" => Some("きゃ"), "kyu" => Some("きゅ"), "kyo" => Some("きょ"),
        "jya" => Some("じゃ"), "jyu" => Some("じゅ"), "jyo" => Some("じょ"),

        // === 2-character sequences ===
        "ka" => Some("か"), "ki" => Some("き"), "ku" => Some("く"), "ke" => Some("け"), "ko" => Some("こ"),
        "sa" => Some("さ"), "si" => Some("し"), "su" => Some("す"), "se" => Some("せ"), "so" => Some("そ"),
        "ta" => Some("た"), "ti" => Some("ち"), "tu" => Some("つ"), "te" => Some("て"), "to" => Some("と"),
        "na" => Some("な"), "ni" => Some("に"), "nu" => Some("ぬ"), "ne" => Some("ね"), "no" => Some("の"),
        "ha" => Some("は"), "hi" => Some("ひ"), "hu" => Some("ふ"), "fu" => Some("ふ"), "he" => Some("へ"), "ho" => Some("ほ"),
        "ma" => Some("ま"), "mi" => Some("み"), "mu" => Some("む"), "me" => Some("め"), "mo" => Some("も"),
        "ra" => Some("ら"), "ri" => Some("り"), "ru" => Some("る"), "re" => Some("れ"), "ro" => Some("ろ"),
        "ya" => Some("や"), "yu" => Some("ゆ"), "yo" => Some("よ"),
        "wa" => Some("わ"), "wi" => Some("ゐ"), "we" => Some("ゑ"), "wo" => Some("を"),
        "ga" => Some("が"), "gi" => Some("ぎ"), "gu" => Some("ぐ"), "ge" => Some("げ"), "go" => Some("ご"),
        "za" => Some("ざ"), "zi" => Some("じ"), "zu" => Some("ず"), "ze" => Some("ぜ"), "zo" => Some("ぞ"),
        "da" => Some("だ"), "di" => Some("ぢ"), "du" => Some("づ"), "de" => Some("で"), "do" => Some("ど"),
        "ba" => Some("ば"), "bi" => Some("び"), "bu" => Some("ぶ"), "be" => Some("べ"), "bo" => Some("ぼ"),
        "pa" => Some("ぱ"), "pi" => Some("ぴ"), "pu" => Some("ぷ"), "pe" => Some("ぺ"), "po" => Some("ぽ"),
        "ja" => Some("じゃ"), "ju" => Some("じゅ"), "jo" => Some("じょ"),

        // === 1-character sequences (vowels only; 'n' handled separately) ===
        "a" => Some("あ"), "i" => Some("い"), "u" => Some("う"), "e" => Some("え"), "o" => Some("お"),

        _ => None,
    }
}
```

### 7.6 Mixed Name Reading Generation (CRITICAL — most complex function)

This function handles the core challenge: generating hiragana readings for names that may have mixed kanji/kana parts.

**The key insight:** VNDB romanized names are in **Western order** ("Given Family"), but Japanese names are in **Japanese order** ("Family Given"). When a name has two parts separated by a space, we must **swap** the romanized parts to match:

```
Japanese:  "須々木 心一"      → parts: [Family="須々木", Given="心一"]
Romanized: "Shinichi Suzuki"  → parts: [Given="Shinichi", Family="Suzuki"]

Mapping (with swap):
  Japanese Family "須々木" (has kanji) → use romaji "Shinichi" → hiragana "しんいち"
  Wait, that's wrong! Let me re-read the Python...
```

**ACTUALLY — read the Python carefully.** The mapping is:

```
Romanized parts[0] = "Shinichi" (Western given = first word)
Romanized parts[1] = "Suzuki"   (Western family = second word)

Japanese parts[0] = "須々木" (Japanese family = first word)
Japanese parts[1] = "心一"   (Japanese given = second word)

The Python code does:
  given_romaji = romanized_parts[0]   ("Shinichi")
  family_romaji = romanized_parts[1]  ("Suzuki")

  If Japanese family ("須々木") has kanji → use given_romaji ("Shinichi") via alphabet2kana
  If Japanese given ("心一") has kanji → use family_romaji ("Suzuki") via alphabet2kana
```

Wait, this seems wrong at first glance, but it IS correct. The Python comment says:
- `given_romaji = romanized_parts[0]` — "Western given" corresponds to "Japanese family"
- `family_romaji = romanized_parts[1]` — "Western family" corresponds to "Japanese given"

**But that produces: family_reading = kana("shinichi") = "しんいち" for 須々木(Suzuki)**

Let me re-read the Python more carefully...

Actually, the Python variable naming is confusing. Let me trace through with "Shinichi Suzuki" / "須々木 心一":

```python
romanized_parts = ["Shinichi", "Suzuki"]  # split "Shinichi Suzuki"
given_romaji = romanized_parts[0]   # "Shinichi"
family_romaji = romanized_parts[1]  # "Suzuki"

# family_has_kanji = True (須々木 has kanji)
# given_has_kanji = True (心一 has kanji)

# Family reading: uses given_romaji ("Shinichi") — WAIT THIS IS WRONG

# Actually looking at the code again:
# Line 296: family_reading = jaconv.alphabet2kana(given_romaji.lower())
# This means: family_reading = kana("shinichi") = "しんいち"
# But 須々木 = Suzuki = すずき !!
```

OK so the Python code has the variable naming as `given_romaji` and `family_romaji` but the mapping is:

```python
# Lines 290-291:
given_romaji = romanized_parts[0]   # First word of Western name
family_romaji = romanized_parts[1]  # Second word of Western name

# Line 296 (for family reading when family has kanji):
family_reading = jaconv.alphabet2kana(given_romaji.lower())
```

This means: Japanese family name uses the **first word** of the romanized name. In VNDB's Western order, the first word IS the given name. So `family_reading = kana(given_romaji)`.

For "Shinichi Suzuki" / "須々木 心一":
- `family_reading = kana("shinichi")` = "しんいち" — this is WRONG for 須々木

**This means the Python variable naming is misleading, OR VNDB's "Western order" is not consistently "Given Family".** Let me check: VNDB says `name` field is "romanized" in "Western order". For Japanese VNs, this typically means the name is listed as "Given Family" — e.g., "Shinichi Suzuki" where Shinichi is the given name.

But Japanese order is "Family Given" — "須々木 心一" where 須々木 is family.

So the correct mapping should be:
- Romanized[0] ("Shinichi") = Given → maps to Japanese Given ("心一")
- Romanized[1] ("Suzuki") = Family → maps to Japanese Family ("須々木")

But the Python does the OPPOSITE! Let me re-check by looking at test_name_parser.py line 82:

```python
result = parser.generate_mixed_name_readings("\u6f22 kana", "Given Family")
assert result["family"] == "kana(given)"  # Japanese family = kana(romanized[0])
assert result["given"] == "hira(kana)"    # Japanese given = hira(Japanese given directly)
```

So the test confirms: **Japanese family reading** uses `kana(romanized[0])` which is the Western Given name.

This seems backwards, but it's what the Python code does and tests verify. The explanation is likely that VNDB's "Western order" actually puts family name first for some entries, or the Python was written to match a specific VNDB convention. **Regardless, we must match the Python behavior exactly.**

Here is the correct implementation matching the Python:

```rust
/// Name reading results.
#[derive(Debug, Clone)]
pub struct NameReadings {
    pub has_space: bool,
    pub original: String,
    pub full: String,    // Full hiragana reading (family + given)
    pub family: String,  // Family name hiragana reading
    pub given: String,   // Given name hiragana reading
}

/// Generate hiragana readings for a name that may have mixed kanji/kana parts.
///
/// For each name part (family, given) independently:
/// - If part contains kanji → convert corresponding romanized part via alphabet_to_kana
/// - If part is kana only → use kata_to_hira directly on the Japanese text
///
/// IMPORTANT: Romanized names from VNDB are Western order ("Given Family").
/// Japanese names are Japanese order ("Family Given").
/// romanized_parts[0] maps to Japanese family; romanized_parts[1] maps to Japanese given.
pub fn generate_mixed_name_readings(
    name_original: &str,
    romanized_name: &str,
) -> NameReadings {
    // Handle empty names
    if name_original.is_empty() {
        return NameReadings {
            has_space: false,
            original: String::new(),
            full: String::new(),
            family: String::new(),
            given: String::new(),
        };
    }

    // For single-word names (no space)
    if !name_original.contains(' ') {
        if contains_kanji(name_original) {
            // Has kanji — use romanized reading
            let full = alphabet_to_kana(romanized_name);
            return NameReadings {
                has_space: false,
                original: name_original.to_string(),
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        } else {
            // Pure kana — use kata_to_hira on the Japanese text itself
            let full = kata_to_hira(&name_original.replace(' ', ""));
            return NameReadings {
                has_space: false,
                original: name_original.to_string(),
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        }
    }

    // Two-part name: split Japanese (Family Given order)
    let jp_parts = split_japanese_name(name_original);
    let family_jp = jp_parts.family.as_deref().unwrap_or("");
    let given_jp = jp_parts.given.as_deref().unwrap_or("");

    let family_has_kanji = contains_kanji(family_jp);
    let given_has_kanji = contains_kanji(given_jp);

    // Split romanized name (Western order: first_word second_word)
    let rom_parts: Vec<&str> = romanized_name.splitn(2, ' ').collect();
    let rom_first = rom_parts.first().copied().unwrap_or("");   // romanized_parts[0]
    let rom_second = rom_parts.get(1).copied().unwrap_or("");   // romanized_parts[1]

    // Family reading: if kanji, use rom_first (romanized_parts[0]) via alphabet_to_kana
    //                 if kana, use Japanese family text via kata_to_hira
    let family_reading = if family_has_kanji {
        alphabet_to_kana(rom_first)
    } else {
        kata_to_hira(family_jp)
    };

    // Given reading: if kanji, use rom_second (romanized_parts[1]) via alphabet_to_kana
    //                if kana, use Japanese given text via kata_to_hira
    let given_reading = if given_has_kanji {
        alphabet_to_kana(rom_second)
    } else {
        kata_to_hira(given_jp)
    };

    let full_reading = format!("{}{}", family_reading, given_reading);

    NameReadings {
        has_space: true,
        original: name_original.to_string(),
        full: full_reading,
        family: family_reading,
        given: given_reading,
    }
}
```

### 7.7 Honorific Suffix Constants

These are appended to character names to create additional searchable entries. Each tuple is `(kanji_or_kana_form, hiragana_reading)`.

```rust
/// Honorific suffixes: (display form appended to term, hiragana appended to reading)
pub const HONORIFIC_SUFFIXES: &[(&str, &str)] = &[
    // Respectful/Formal
    ("さん", "さん"),
    ("様", "さま"),
    ("先生", "せんせい"),
    ("先輩", "せんぱい"),
    ("後輩", "こうはい"),
    ("氏", "し"),
    // Casual/Friendly
    ("君", "くん"),
    ("くん", "くん"),
    ("ちゃん", "ちゃん"),
    ("たん", "たん"),
    ("坊", "ぼう"),
    // Old-fashioned/Archaic
    ("殿", "どの"),
    ("博士", "はかせ"),
    // Occupational/Specific
    ("社長", "しゃちょう"),
    ("部長", "ぶちょう"),
];
```

---

## 8. CONTENT BUILDER — `content_builder.rs`

Builds the Yomitan "structured-content" JSON that renders the character card popup.

### 8.1 Constants

```rust
use serde_json::json;
use regex::Regex;
use crate::models::Character;

pub struct ContentBuilder {
    spoiler_level: u8,
}

/// Role badge colors
const ROLE_COLORS: &[(&str, &str)] = &[
    ("main", "#4CAF50"),     // green
    ("primary", "#2196F3"),  // blue
    ("side", "#FF9800"),     // orange
    ("appears", "#9E9E9E"),  // gray
];

/// Role display labels
const ROLE_LABELS: &[(&str, &str)] = &[
    ("main", "Protagonist"),
    ("primary", "Main Character"),
    ("side", "Side Character"),
    ("appears", "Minor Role"),
];

/// Month names for birthday formatting
const MONTH_NAMES: &[(u32, &str)] = &[
    (1, "January"), (2, "February"), (3, "March"), (4, "April"),
    (5, "May"), (6, "June"), (7, "July"), (8, "August"),
    (9, "September"), (10, "October"), (11, "November"), (12, "December"),
];

/// Sex display mapping — must handle both "m"/"f" and "male"/"female" inputs
const SEX_DISPLAY: &[(&str, &str)] = &[
    ("m", "♂ Male"),
    ("f", "♀ Female"),
    ("male", "♂ Male"),
    ("female", "♀ Female"),
];
```

### 8.2 Spoiler Stripping

Handles both VNDB format `[spoiler]...[/spoiler]` and AniList format `~!...!~`.

```rust
impl ContentBuilder {
    pub fn new(spoiler_level: u8) -> Self {
        Self { spoiler_level }
    }

    /// Remove spoiler content from text. Both VNDB and AniList formats.
    pub fn strip_spoilers(text: &str) -> String {
        // VNDB: [spoiler]...[/spoiler]
        let re_vndb = Regex::new(r"(?is)\[spoiler\].*?\[/spoiler\]").unwrap();
        let text = re_vndb.replace_all(text, "");
        // AniList: ~!...!~
        let re_anilist = Regex::new(r"(?s)~!.*?!~").unwrap();
        re_anilist.replace_all(&text, "").trim().to_string()
    }

    /// Check if text contains spoiler tags (either format).
    pub fn has_spoiler_tags(text: &str) -> bool {
        let re_vndb = Regex::new(r"(?i)\[spoiler\]").unwrap();
        let re_anilist = Regex::new(r"(?s)~!.*?!~").unwrap();
        re_vndb.is_match(text) || re_anilist.is_match(text)
    }

    /// Parse VNDB markup: [url=https://...]text[/url] → just the text
    pub fn parse_vndb_markup(text: &str) -> String {
        let re = Regex::new(r"(?i)\[url=[^\]]+\]([^\[]*)\[/url\]").unwrap();
        re.replace_all(text, "$1").to_string()
    }
```

### 8.3 Birthday Formatting

```rust
    /// Format birthday [month, day] → "September 1"
    pub fn format_birthday(birthday: &[u32]) -> String {
        if birthday.len() < 2 {
            return String::new();
        }
        let month = birthday[0];
        let day = birthday[1];
        let month_name = MONTH_NAMES
            .iter()
            .find(|(m, _)| *m == month)
            .map(|(_, name)| *name)
            .unwrap_or("Unknown");
        format!("{} {}", month_name, day)
    }
```

### 8.4 Physical Stats Line

Builds a compact string like: `"♀ Female • 17 years • 165cm • 50kg • Blood Type A • Birthday: September 1"`

```rust
    /// Build physical stats line.
    pub fn format_stats(&self, char: &Character) -> String {
        let mut parts = Vec::new();

        if let Some(ref sex) = char.sex {
            let sex_lower = sex.to_lowercase();
            if let Some((_, display)) = SEX_DISPLAY.iter().find(|(k, _)| *k == sex_lower.as_str()) {
                parts.push(display.to_string());
            }
        }

        if let Some(ref age) = char.age {
            parts.push(format!("{} years", age));
        }

        if let Some(height) = char.height {
            parts.push(format!("{}cm", height));
        }

        if let Some(weight) = char.weight {
            parts.push(format!("{}kg", weight));
        }

        if let Some(ref blood_type) = char.blood_type {
            parts.push(format!("Blood Type {}", blood_type));
        }

        if let Some(ref birthday) = char.birthday {
            let formatted = Self::format_birthday(birthday);
            if !formatted.is_empty() {
                parts.push(format!("Birthday: {}", formatted));
            }
        }

        parts.join(" • ")
    }
```

### 8.5 Trait Categorization with Spoiler Filtering

```rust
    /// Build trait items grouped by category, filtered by spoiler_level.
    /// Returns a Vec of Yomitan `li` content items.
    pub fn build_traits_by_category(&self, char: &Character) -> Vec<serde_json::Value> {
        let mut items = Vec::new();

        let categories: &[(&[CharacterTrait], &str)] = &[
            (&char.personality, "Personality"),
            (&char.roles, "Role"),
            (&char.engages_in, "Activities"),
            (&char.subject_of, "Subject of"),
        ];

        for (traits, label) in categories {
            if traits.is_empty() {
                continue;
            }

            // Filter traits by spoiler level
            let filtered: Vec<&str> = traits
                .iter()
                .filter(|t| t.spoiler <= self.spoiler_level && !t.name.is_empty())
                .map(|t| t.name.as_str())
                .collect();

            if !filtered.is_empty() {
                items.push(json!({
                    "tag": "li",
                    "content": format!("{}: {}", label, filtered.join(", "))
                }));
            }
        }

        items
    }
```

### 8.6 Build Structured Content (CORE — the character card)

Three-tier spoiler system:
- **Level 0 (No Spoilers):** Name, image, game title, role badge ONLY
- **Level 1 (Minor Spoilers):** + Description (with spoiler tags stripped) + Character info (stats + traits filtered to spoiler≤1)
- **Level 2 (Full Spoilers):** + Full unfiltered description + All traits

```rust
    /// Build the complete Yomitan structured content for a character card.
    pub fn build_content(
        &self,
        char: &Character,
        image_path: Option<&str>,
        game_title: &str,
    ) -> serde_json::Value {
        let mut content: Vec<serde_json::Value> = Vec::new();

        // ===== LEVEL 0: Always shown =====

        // Japanese name (large, bold)
        if !char.name_original.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontWeight": "bold", "fontSize": "1.2em" },
                "content": &char.name_original
            }));
        }

        // Romanized name (italic, gray)
        if !char.name.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontStyle": "italic", "color": "#666", "marginBottom": "8px" },
                "content": &char.name
            }));
        }

        // Character portrait image
        if let Some(path) = image_path {
            content.push(json!({
                "tag": "img",
                "path": path,
                "width": 80,
                "height": 100,
                "sizeUnits": "px",
                "collapsible": false,
                "collapsed": false,
                "background": false
            }));
        }

        // Game/media title
        if !game_title.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontSize": "0.9em", "color": "#888", "marginTop": "4px" },
                "content": format!("From: {}", game_title)
            }));
        }

        // Role badge with color
        let role = char.role.as_str();
        let role_color = ROLE_COLORS
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, c)| *c)
            .unwrap_or("#9E9E9E");
        let role_label = ROLE_LABELS
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, l)| *l)
            .unwrap_or("Unknown");

        content.push(json!({
            "tag": "span",
            "style": {
                "background": role_color,
                "color": "white",
                "padding": "2px 6px",
                "borderRadius": "3px",
                "fontSize": "0.85em",
                "marginTop": "4px"
            },
            "content": role_label
        }));

        // ===== LEVEL 1+: Description and Character Information =====

        if self.spoiler_level >= 1 {
            // Description section (collapsible <details>)
            if let Some(ref desc) = char.description {
                if !desc.trim().is_empty() {
                    let display_desc = if self.spoiler_level == 1 {
                        Self::strip_spoilers(desc)
                    } else {
                        desc.clone() // Level 2: full description
                    };

                    if !display_desc.is_empty() {
                        let parsed = Self::parse_vndb_markup(&display_desc);
                        content.push(json!({
                            "tag": "details",
                            "content": [
                                { "tag": "summary", "content": "Description" },
                                {
                                    "tag": "div",
                                    "style": { "fontSize": "0.9em", "marginTop": "4px" },
                                    "content": parsed
                                }
                            ]
                        }));
                    }
                }
            }

            // Character Information section (collapsible <details>)
            let mut info_items: Vec<serde_json::Value> = Vec::new();

            // Physical stats as compact line
            let stats = self.format_stats(char);
            if !stats.is_empty() {
                info_items.push(json!({
                    "tag": "li",
                    "style": { "fontWeight": "bold" },
                    "content": stats
                }));
            }

            // Traits organized by category (filtered by spoiler level)
            let trait_items = self.build_traits_by_category(char);
            info_items.extend(trait_items);

            if !info_items.is_empty() {
                content.push(json!({
                    "tag": "details",
                    "content": [
                        { "tag": "summary", "content": "Character Information" },
                        {
                            "tag": "ul",
                            "style": { "marginTop": "4px", "paddingLeft": "20px" },
                            "content": info_items
                        }
                    ]
                }));
            }
        }

        json!({
            "type": "structured-content",
            "content": content
        })
    }
```

### 8.7 Create Term Entry

Each Yomitan term entry is an 8-element array:

```
[term, reading, definitionTags, rules, score, [definitions], sequence, termTags]
```

```rust
    /// Create a single Yomitan term entry.
    pub fn create_term_entry(
        term: &str,
        reading: &str,
        role: &str,
        score: i32,
        structured_content: &serde_json::Value,
    ) -> serde_json::Value {
        json!([
            term,
            reading,
            if role.is_empty() { "name".to_string() } else { format!("name {}", role) },
            "",
            score,
            [structured_content],
            0,
            ""
        ])
    }
}
```

---

## 9. IMAGE HANDLER — `image_handler.rs`

Decodes base64 images (which come with a data URI prefix from the API clients) into raw bytes + determines the file extension.

```rust
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

pub struct ImageHandler;

impl ImageHandler {
    /// Decode a base64-encoded image string.
    /// Input may have data URI prefix: "data:image/jpeg;base64,..."
    /// Returns (filename, raw_image_bytes).
    pub fn decode_image(base64_data: &str, char_id: &str) -> (String, Vec<u8>) {
        let (ext, data_part) = if let Some(comma_pos) = base64_data.find(',') {
            let header = &base64_data[..comma_pos];
            let data = &base64_data[comma_pos + 1..];

            let ext = if header.contains("png") {
                "png"
            } else if header.contains("gif") {
                "gif"
            } else if header.contains("webp") {
                "webp"
            } else {
                "jpg"
            };

            (ext, data)
        } else {
            ("jpg", base64_data) // No prefix — assume JPEG
        };

        let image_bytes = STANDARD.decode(data_part).unwrap_or_default();
        let filename = format!("c{}.{}", char_id, ext);

        (filename, image_bytes)
    }
}
```

---

## 10. DICTIONARY BUILDER — `dict_builder.rs`

The orchestrator: takes characters, generates all term entries (with name variants, honorifics, aliases), and assembles the ZIP.

### 10.1 Role Scores

```rust
fn get_score(role: &str) -> i32 {
    match role {
        "main" => 100,
        "primary" => 75,
        "side" => 50,
        "appears" => 25,
        _ => 0,
    }
}
```

### 10.2 add_character — Complete Entry Generation

This is the core function. For a single character it generates:
1. Up to 4 base name entries (original with space, combined, family only, given only)
2. Honorific suffix variants for ALL base names (4 × 15 = up to 60 entries)
3. Alias entries
4. Honorific suffix variants for all aliases
5. All deduplicated via a HashSet

```rust
use std::collections::HashSet;
use std::io::{Cursor, Write};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;
use serde_json::json;
use crate::models::*;
use crate::name_parser::{self, NameReadings, HONORIFIC_SUFFIXES};
use crate::content_builder::ContentBuilder;
use crate::image_handler::ImageHandler;

pub struct DictBuilder {
    entries: Vec<serde_json::Value>,
    images: Vec<(String, Vec<u8>)>, // (filename, bytes) for ZIP img/ folder
    spoiler_level: u8,
    revision: String,
    download_url: Option<String>,
    game_title: String, // Title of the media for index.json description
}

impl DictBuilder {
    pub fn new(spoiler_level: u8, download_url: Option<String>, game_title: String) -> Self {
        // Random 12-digit revision string
        let revision: u64 = rand::random::<u64>() % 1_000_000_000_000;
        Self {
            entries: Vec::new(),
            images: Vec::new(),
            spoiler_level,
            revision: format!("{:012}", revision),
            download_url,
            game_title,
        }
    }

    /// Process a single character and create all term entries.
    pub fn add_character(&mut self, char: &Character, game_title: &str) {
        let name_original = &char.name_original;
        if name_original.is_empty() {
            return; // Skip characters with no Japanese name
        }

        // Generate hiragana readings using mixed name handling
        let readings = name_parser::generate_mixed_name_readings(name_original, &char.name);

        let role = &char.role;
        let score = get_score(role);

        let content_builder = ContentBuilder::new(self.spoiler_level);

        // Handle image: decode base64 → raw bytes for ZIP
        let image_path = if let Some(ref img_base64) = char.image_base64 {
            let (filename, image_bytes) = ImageHandler::decode_image(img_base64, &char.id);
            let path = format!("img/{}", filename);
            self.images.push((filename, image_bytes));
            Some(path)
        } else {
            None
        };

        // Build the structured content card (shared across all entries for this character)
        let structured_content =
            content_builder.build_content(char, image_path.as_deref(), game_title);

        // Track terms to avoid duplicates
        let mut added_terms: HashSet<String> = HashSet::new();

        // Split the Japanese name
        let name_parts = name_parser::split_japanese_name(name_original);

        // --- Base name entries ---

        if name_parts.has_space {
            // 1. Original with space: "須々木 心一"
            if !name_parts.original.is_empty() && added_terms.insert(name_parts.original.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &name_parts.original,
                    &readings.full,
                    role,
                    score,
                    &structured_content,
                ));
            }

            // 2. Combined without space: "須々木心一"
            if !name_parts.combined.is_empty() && added_terms.insert(name_parts.combined.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &name_parts.combined,
                    &readings.full,
                    role,
                    score,
                    &structured_content,
                ));
            }

            // 3. Family name only: "須々木"
            if let Some(ref family) = name_parts.family {
                if !family.is_empty() && added_terms.insert(family.clone()) {
                    self.entries.push(ContentBuilder::create_term_entry(
                        family,
                        &readings.family,
                        role,
                        score,
                        &structured_content,
                    ));
                }
            }

            // 4. Given name only: "心一"
            if let Some(ref given) = name_parts.given {
                if !given.is_empty() && added_terms.insert(given.clone()) {
                    self.entries.push(ContentBuilder::create_term_entry(
                        given,
                        &readings.given,
                        role,
                        score,
                        &structured_content,
                    ));
                }
            }
        } else {
            // Single-word name
            if !name_original.is_empty() && added_terms.insert(name_original.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    name_original,
                    &readings.full,
                    role,
                    score,
                    &structured_content,
                ));
            }
        }

        // --- Honorific suffix variants for all base names ---

        let mut base_names_with_readings: Vec<(&str, &str)> = Vec::new();
        if name_parts.has_space {
            if let Some(ref family) = name_parts.family {
                if !family.is_empty() {
                    base_names_with_readings.push((family, &readings.family));
                }
            }
            if let Some(ref given) = name_parts.given {
                if !given.is_empty() {
                    base_names_with_readings.push((given, &readings.given));
                }
            }
            if !name_parts.combined.is_empty() {
                base_names_with_readings.push((&name_parts.combined, &readings.full));
            }
            if !name_parts.original.is_empty() {
                base_names_with_readings.push((&name_parts.original, &readings.full));
            }
        } else {
            if !name_original.is_empty() {
                base_names_with_readings.push((name_original, &readings.full));
            }
        }

        for (base_name, base_reading) in &base_names_with_readings {
            for (suffix, suffix_reading) in HONORIFIC_SUFFIXES {
                let term_with_suffix = format!("{}{}", base_name, suffix);
                let reading_with_suffix = format!("{}{}", base_reading, suffix_reading);

                if added_terms.insert(term_with_suffix.clone()) {
                    self.entries.push(ContentBuilder::create_term_entry(
                        &term_with_suffix,
                        &reading_with_suffix,
                        role,
                        score,
                        &structured_content,
                    ));
                }
            }
        }

        // --- Alias entries ---

        for alias in &char.aliases {
            if !alias.is_empty() && added_terms.insert(alias.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    alias,
                    &readings.full, // Use full reading for aliases
                    role,
                    score,
                    &structured_content,
                ));

                // Also add honorific variants for each alias
                for (suffix, suffix_reading) in HONORIFIC_SUFFIXES {
                    let alias_with_suffix = format!("{}{}", alias, suffix);
                    let reading_with_suffix = format!("{}{}", readings.full, suffix_reading);

                    if added_terms.insert(alias_with_suffix.clone()) {
                        self.entries.push(ContentBuilder::create_term_entry(
                            &alias_with_suffix,
                            &reading_with_suffix,
                            role,
                            score,
                            &structured_content,
                        ));
                    }
                }
            }
        }
    }
```

### 10.3 Index and Tag Bank

```rust
    /// Create index.json metadata.
    fn create_index(&self) -> serde_json::Value {
        let description = format!("Character names from {}", self.game_title);

        let mut index = json!({
            "title": "Bee's Character Dictionary",
            "revision": &self.revision,
            "format": 3,
            "author": "Bee (https://github.com/bee-san)",
            "description": description
        });

        // Add auto-update URLs if download_url is set
        if let Some(ref url) = self.download_url {
            index["downloadUrl"] = json!(url);
            // indexUrl is the same URL but with /api/yomitan-index instead of /api/yomitan-dict
            index["indexUrl"] = json!(url.replace("/api/yomitan-dict", "/api/yomitan-index"));
            index["isUpdatable"] = json!(true);
        }

        index
    }

    /// Create tag_bank_1.json — fixed tag definitions for character roles.
    fn create_tags(&self) -> serde_json::Value {
        json!([
            ["name", "partOfSpeech", 0, "Character name", 0],
            ["main", "name", 0, "Protagonist", 0],
            ["primary", "name", 0, "Main character", 0],
            ["side", "name", 0, "Side character", 0],
            ["appears", "name", 0, "Minor appearance", 0]
        ])
    }
```

### 10.4 ZIP Export

**CRITICAL:** Use `Cursor<Vec<u8>>` because `ZipWriter` needs `Write + Seek`. Plain `Vec<u8>` doesn't implement `Seek`.

```rust
    /// Export the dictionary as in-memory ZIP bytes.
    pub fn export_bytes(&self) -> Vec<u8> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        // 1. index.json
        zip.start_file("index.json", options).unwrap();
        let index_json = serde_json::to_string_pretty(&self.create_index()).unwrap();
        zip.write_all(index_json.as_bytes()).unwrap();

        // 2. tag_bank_1.json
        zip.start_file("tag_bank_1.json", options).unwrap();
        let tags_json = serde_json::to_string(&self.create_tags()).unwrap();
        zip.write_all(tags_json.as_bytes()).unwrap();

        // 3. term_bank_N.json (chunked at 10,000 entries per file)
        let entries_per_bank = 10_000;
        for (i, chunk) in self.entries.chunks(entries_per_bank).enumerate() {
            let filename = format!("term_bank_{}.json", i + 1);
            zip.start_file(&filename, options).unwrap();
            let data = serde_json::to_string(chunk).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }

        // 4. Images in img/ folder
        for (filename, bytes) in &self.images {
            zip.start_file(format!("img/{}", filename), options).unwrap();
            zip.write_all(bytes).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }
}
```

---

## 11. SERVER — `main.rs`

### 11.1 Complete Server Implementation

```rust
use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;
use tower_http::services::ServeDir;

mod models;
mod name_parser;
mod content_builder;
mod image_handler;
mod dict_builder;
mod vndb_client;
mod anilist_client;

use dict_builder::DictBuilder;
use vndb_client::VndbClient;
use anilist_client::AnilistClient;

#[derive(Deserialize)]
struct DictQuery {
    source: String,                // "vndb" or "anilist"
    id: String,                    // VN ID like "v17" or AniList media ID like "9253"
    #[serde(default)]
    spoiler_level: u8,             // 0, 1, or 2 (default 0)
    #[serde(default = "default_media_type")]
    media_type: String,            // "ANIME" or "MANGA" (for AniList only, default "ANIME")
}

fn default_media_type() -> String {
    "ANIME".to_string()
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/yomitan-dict", get(generate_dict))
        .route("/api/yomitan-index", get(generate_index))
        .nest_service("/static", ServeDir::new("static"));

    let addr = "0.0.0.0:3000";
    println!("Server running on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn serve_index() -> impl IntoResponse {
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(html) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn generate_dict(Query(params): Query<DictQuery>) -> impl IntoResponse {
    // Clamp spoiler level
    let spoiler_level = params.spoiler_level.min(2);

    // Build the download URL for auto-update
    let download_url = format!(
        "http://127.0.0.1:3000/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}",
        params.source, params.id, spoiler_level, params.media_type
    );

    match params.source.to_lowercase().as_str() {
        "vndb" => match generate_vndb_dict(&params.id, spoiler_level, &download_url).await {
            Ok(bytes) => (
                StatusCode::OK,
                [
                    ("content-type", "application/zip"),
                    ("content-disposition", "attachment; filename=bee_characters.zip"),
                    ("access-control-allow-origin", "*"),
                ],
                bytes,
            )
                .into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        "anilist" => {
            let media_id: i32 = match params.id.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid AniList ID: must be a number")
                        .into_response()
                }
            };
            let media_type = params.media_type.to_uppercase();
            if media_type != "ANIME" && media_type != "MANGA" {
                return (
                    StatusCode::BAD_REQUEST,
                    "media_type must be ANIME or MANGA",
                )
                    .into_response();
            }
            match generate_anilist_dict(media_id, &media_type, spoiler_level, &download_url).await
            {
                Ok(bytes) => (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        ("access-control-allow-origin", "*"),
                    ],
                    bytes,
                )
                    .into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            }
        }
        _ => (StatusCode::BAD_REQUEST, "source must be 'vndb' or 'anilist'").into_response(),
    }
}

/// Lightweight endpoint: returns just the index.json metadata as JSON.
/// Yomitan checks this to decide if an update is available.
async fn generate_index(Query(params): Query<DictQuery>) -> impl IntoResponse {
    let spoiler_level = params.spoiler_level.min(2);
    let download_url = format!(
        "http://127.0.0.1:3000/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}",
        params.source, params.id, spoiler_level, params.media_type
    );
    let builder = DictBuilder::new(
        spoiler_level,
        Some(download_url),
        "".to_string(), // Title not needed for index-only
    );
    let index = builder.create_index_public(); // Need a public accessor

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        serde_json::to_string(&index).unwrap(),
    )
        .into_response()
}

// Note: DictBuilder needs a public method for index generation:
// pub fn create_index_public(&self) -> serde_json::Value { self.create_index() }

async fn generate_vndb_dict(
    vn_id: &str,
    spoiler_level: u8,
    download_url: &str,
) -> Result<Vec<u8>, String> {
    let client = VndbClient::new();

    // 1. Fetch VN title
    let (romaji_title, original_title) = client
        .fetch_vn_title(vn_id)
        .await
        .unwrap_or_else(|_| ("Unknown VN".to_string(), String::new()));
    let game_title = if !original_title.is_empty() {
        original_title
    } else {
        romaji_title
    };

    // 2. Fetch all characters (categorized by role)
    let mut char_data = client.fetch_characters(vn_id).await?;

    // 3. Download images for all characters
    for char in char_data.all_characters_mut() {
        if let Some(ref url) = char.image_url {
            char.image_base64 = client.fetch_image_as_base64(url).await;
        }
    }

    // 4. Build dictionary
    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
    );

    for char in char_data.all_characters() {
        builder.add_character(char, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    Ok(builder.export_bytes())
}

async fn generate_anilist_dict(
    media_id: i32,
    media_type: &str,
    spoiler_level: u8,
    download_url: &str,
) -> Result<Vec<u8>, String> {
    let client = AnilistClient::new();

    // 1. Fetch characters + media title (title comes from first page of character query)
    let (mut char_data, media_title) = client.fetch_characters(media_id, media_type).await?;

    let game_title = if !media_title.is_empty() {
        media_title
    } else {
        format!("AniList {}", media_id)
    };

    // 2. Download images for all characters
    for char in char_data.all_characters_mut() {
        if let Some(ref url) = char.image_url {
            char.image_base64 = client.fetch_image_as_base64(url).await;
        }
    }

    // 3. Build dictionary
    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
    );

    for char in char_data.all_characters() {
        builder.add_character(char, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    Ok(builder.export_bytes())
}
```

Note: `AnilistClient::fetch_image_as_base64` uses the same implementation as `VndbClient::fetch_image_as_base64` (section 5.6). You can extract it into a shared function or duplicate it.

---

## 12. FRONTEND — `static/index.html`

```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Yomitan Dictionary Builder</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
        }
        .container {
            background: white;
            border-radius: 12px;
            box-shadow: 0 20px 60px rgba(0,0,0,0.3);
            max-width: 600px;
            width: 100%;
            padding: 40px;
        }
        h1 { color: #333; margin-bottom: 30px; text-align: center; }
        .form-group { margin-bottom: 20px; }
        label { display: block; margin-bottom: 8px; color: #555; font-weight: 500; }
        input, select {
            width: 100%;
            padding: 12px;
            border: 2px solid #ddd;
            border-radius: 6px;
            font-size: 16px;
            transition: border-color 0.3s;
        }
        input:focus, select:focus { outline: none; border-color: #667eea; }
        button {
            width: 100%;
            padding: 12px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border: none;
            border-radius: 6px;
            font-size: 16px;
            font-weight: 600;
            cursor: pointer;
            transition: transform 0.2s;
        }
        button:hover { transform: translateY(-2px); }
        button:disabled { opacity: 0.6; cursor: not-allowed; transform: none; }
        .status { margin-top: 20px; padding: 12px; border-radius: 6px; display: none; }
        .status.show { display: block; }
        .status.success { background: #d4edda; color: #155724; }
        .status.error { background: #f8d7da; color: #721c24; }
        .status.loading { background: #cce5ff; color: #004085; }
        /* Hide media type selector when VNDB is selected */
        #mediaTypeGroup { display: none; }
    </style>
</head>
<body>
    <div class="container">
        <h1>Yomitan Dictionary Builder</h1>
        <form id="dictForm">
            <div class="form-group">
                <label for="source">Source:</label>
                <select id="source" name="source" required>
                    <option value="vndb">VNDB (Visual Novel Database)</option>
                    <option value="anilist">AniList (Anime/Manga)</option>
                </select>
            </div>

            <div class="form-group" id="mediaTypeGroup">
                <label for="mediaType">Media Type:</label>
                <select id="mediaType" name="mediaType">
                    <option value="ANIME">Anime</option>
                    <option value="MANGA">Manga</option>
                </select>
            </div>

            <div class="form-group">
                <label for="id">Media ID:</label>
                <input type="text" id="id" name="id" placeholder="e.g., v17 or 9253" required>
            </div>

            <div class="form-group">
                <label for="spoiler">Spoiler Level:</label>
                <select id="spoiler" name="spoiler">
                    <option value="0">No Spoilers (name, image, role only)</option>
                    <option value="1">Minor Spoilers (+ description, stats)</option>
                    <option value="2">Full Spoilers (+ all traits, unspoilered description)</option>
                </select>
            </div>

            <button type="submit" id="submitBtn">Generate Dictionary</button>
        </form>

        <div class="status" id="status"></div>
    </div>

    <script>
        // Show/hide media type selector based on source
        document.getElementById('source').addEventListener('change', function() {
            const mediaTypeGroup = document.getElementById('mediaTypeGroup');
            mediaTypeGroup.style.display = this.value === 'anilist' ? 'block' : 'none';
        });

        document.getElementById('dictForm').addEventListener('submit', async (e) => {
            e.preventDefault();

            const source = document.getElementById('source').value;
            const id = document.getElementById('id').value.trim();
            const spoiler = document.getElementById('spoiler').value;
            const mediaType = document.getElementById('mediaType').value;
            const submitBtn = document.getElementById('submitBtn');
            const status = document.getElementById('status');

            if (!id) {
                status.textContent = 'Please enter a media ID';
                status.className = 'status show error';
                return;
            }

            submitBtn.disabled = true;
            submitBtn.textContent = 'Generating...';
            status.textContent = 'Fetching characters and building dictionary... This may take a minute.';
            status.className = 'status show loading';

            try {
                let url = `/api/yomitan-dict?source=${source}&id=${encodeURIComponent(id)}&spoiler_level=${spoiler}`;
                if (source === 'anilist') {
                    url += `&media_type=${mediaType}`;
                }

                const response = await fetch(url);

                if (!response.ok) {
                    const text = await response.text();
                    throw new Error(text || `HTTP ${response.status}`);
                }

                const blob = await response.blob();
                const downloadUrl = window.URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = downloadUrl;
                a.download = `yomitan_${source}_${id}.zip`;
                document.body.appendChild(a);
                a.click();
                a.remove();
                window.URL.revokeObjectURL(downloadUrl);

                status.textContent = 'Dictionary downloaded! Import the ZIP into Yomitan.';
                status.className = 'status show success';
            } catch (err) {
                status.textContent = `Error: ${err.message}`;
                status.className = 'status show error';
            } finally {
                submitBtn.disabled = false;
                submitBtn.textContent = 'Generate Dictionary';
            }
        });
    </script>
</body>
</html>
```

---

## 13. YOMITAN ZIP FORMAT SPECIFICATION

The ZIP file must contain these files:

```
dictionary.zip
├── index.json           # Dictionary metadata
├── tag_bank_1.json      # Tag definitions (fixed content)
├── term_bank_1.json     # Up to 10,000 term entries
├── term_bank_2.json     # (if > 10,000 entries)
├── term_bank_3.json     # (if > 20,000 entries)
└── img/
    ├── c123.jpg         # Character portrait images
    ├── c456.jpg
    └── ...
```

### index.json

```json
{
    "title": "Bee's Character Dictionary",
    "revision": "384729104856",
    "format": 3,
    "author": "Bee (https://github.com/bee-san)",
    "description": "Character names from Steins;Gate",
    "downloadUrl": "http://127.0.0.1:3000/api/yomitan-dict?source=vndb&id=v17&spoiler_level=0&media_type=ANIME",
    "indexUrl": "http://127.0.0.1:3000/api/yomitan-index?source=vndb&id=v17&spoiler_level=0&media_type=ANIME",
    "isUpdatable": true
}
```

- `format`: Must be `3` (Yomitan format version)
- `revision`: Unique string; Yomitan compares this to detect updates
- `downloadUrl`: Full URL returning the ZIP (for auto-update)
- `indexUrl`: Full URL returning just the index.json as JSON (for lightweight update checking)
- `isUpdatable`: Must be `true` for Yomitan to check for updates

### tag_bank_1.json

Array of tag definitions. Each tag: `[name, category, sortOrder, notes, score]`

```json
[
    ["name", "partOfSpeech", 0, "Character name", 0],
    ["main", "name", 0, "Protagonist", 0],
    ["primary", "name", 0, "Main character", 0],
    ["side", "name", 0, "Side character", 0],
    ["appears", "name", 0, "Minor appearance", 0]
]
```

### term_bank_N.json

Array of term entries. Each entry: `[term, reading, definitionTags, rules, score, definitions, sequence, termTags]`

```json
[
    ["須々木 心一", "すずきしんいち", "name main", "", 100, [{"type":"structured-content","content":[...]}], 0, ""],
    ["須々木", "すずき", "name main", "", 100, [{"type":"structured-content","content":[...]}], 0, ""],
    ["須々木さん", "すずきさん", "name main", "", 100, [{"type":"structured-content","content":[...]}], 0, ""]
]
```

Field breakdown:
| Index | Field | Type | Notes |
|---|---|---|---|
| 0 | term | string | The searchable text (Japanese) |
| 1 | reading | string | Hiragana reading |
| 2 | definitionTags | string | Space-separated tags, e.g. `"name main"` |
| 3 | rules | string | Always empty `""` for names |
| 4 | score | integer | Priority: main=100, primary=75, side=50, appears=25 |
| 5 | definitions | array | Array containing ONE structured-content object |
| 6 | sequence | integer | Always `0` |
| 7 | termTags | string | Always empty `""` |

### Image Tag in Structured Content

When an image is included in the ZIP, it's referenced in structured content like this:

```json
{
    "tag": "img",
    "path": "img/c123.jpg",
    "width": 80,
    "height": 100,
    "sizeUnits": "px",
    "collapsible": false,
    "collapsed": false,
    "background": false
}
```

- `path`: Relative to ZIP root
- `width`/`height`: Fixed 80×100 pixels
- `collapsible`/`collapsed`: Must be `false` (image always visible)
- `background`: Must be `false`

---

## 14. TEST EXPECTATIONS & VERIFICATION

These describe the expected behaviors based on the Python test suite. Use them to verify your implementation.

### Name Parser Tests

1. **Kanji detection:** `contains_kanji("漢a")` → `true`; `contains_kanji("kana")` → `false`; `contains_kanji("")` → `false`
2. **Name splitting:** `split_japanese_name("family given")` → `has_space=true, family="family", given="given", combined="familygiven"`; `split_japanese_name("single")` → `has_space=false, family=None, combined="single"`
3. **Mixed readings (kanji two-part):** For `generate_mixed_name_readings("漢 kana", "Given Family")`:
   - `family` = `alphabet_to_kana("given")` (because 漢 has kanji, uses romanized_parts[0])
   - `given` = `kata_to_hira("kana")` (because "kana" has no kanji, uses Japanese text directly)
4. **Mixed readings (single kanji word):** `generate_mixed_name_readings("漢", "Kan")` → `full` = `alphabet_to_kana("kan")`
5. **Mixed readings (single kana word):** `generate_mixed_name_readings("kana", "unused")` → delegates to kana path (kata_to_hira)
6. **Mixed readings (empty):** `generate_mixed_name_readings("", "")` → all fields empty

### Content Builder Tests

1. **Spoiler stripping:** `strip_spoilers("a [spoiler]x[/spoiler] b ~!y!~ c")` → `"a  b  c"`
2. **Spoiler detection:** `has_spoiler_tags("x [spoiler]y[/spoiler]")` → `true`; `has_spoiler_tags("x ~!y!~")` → `true`; `has_spoiler_tags("plain")` → `false`
3. **VNDB markup:** `parse_vndb_markup("see [url=https://example.com]this link[/url]")` → `"see this link"`
4. **Birthday:** `format_birthday([9, 1])` → `"September 1"`
5. **Physical stats:** Include sex, age, height, weight, blood type, birthday — separated by ` • `
6. **Traits filtering:** With `spoiler_level=1`, traits with `spoiler=2` are excluded, traits with `spoiler=0` or `spoiler=1` are included. Plain string traits (old format) are always included.
7. **Structured content level 0:** Contains `img` and `span` tags but NO `details` tags
8. **Structured content level 1:** Has 2 `details` sections (Description + Character Information); description has spoilers stripped
9. **Structured content level 2:** Description retains original spoiler tags unmodified

### Dict Builder Tests

1. **Role scores:** main=100, primary=75, unknown=0
2. **Empty names skipped:** Character with `name_original=""` and `name=""` produces no entries
3. **Full character processing:** Creates entries for: original, combined, family, given, honorific variants, alias, alias honorific variants. Deduplicates (e.g., if family name equals an alias, only one entry).
4. **Term entry format:** `entry[0]` = term, `entry[1]` = reading, `entry[2]` = role string like `"name main"`, `entry[4]` = score, `entry[5]` = `[structured_content]`
5. **Index metadata:** Contains `title`, `revision`, `description` (includes game titles), `downloadUrl`, `indexUrl` (derived by replacing `/api/yomitan-dict` with `/api/yomitan-index`), `isUpdatable=true`
6. **ZIP export:** Contains files: `index.json`, `tag_bank_1.json`, `term_bank_1.json`, `img/c1.jpg` (for images)

---

## 15. CRITICAL IMPLEMENTATION NOTES

### Name Order Swap (MOST IMPORTANT)

VNDB romanized names are **Western order** ("Given Family"), but Japanese names are **Japanese order** ("Family Given"). When generating readings:

```
Romanized: "Shinichi Suzuki"  → rom_parts[0]="Shinichi", rom_parts[1]="Suzuki"
Japanese:  "須々木 心一"       → jp_family="須々木", jp_given="心一"

Mapping (matching the Python):
  JP family reading ← alphabet_to_kana(rom_parts[0])  // "Shinichi" → "しんいち"
  JP given reading  ← alphabet_to_kana(rom_parts[1])  // "Suzuki" → "すずき"
```

This mapping is what the Python source code and tests verify. Implement it exactly as shown.

### Spoiler Filtering

- **Level 0:** Card shows ONLY: Japanese name, romanized name, image, game title, role badge. No description, no stats, no traits.
- **Level 1:** Adds description (with `[spoiler]...[/spoiler]` and `~!...!~` content REMOVED), adds physical stats line, adds traits filtered to `trait.spoiler <= 1`.
- **Level 2:** Adds full unmodified description (spoiler tags still present in text), adds ALL traits regardless of spoiler level.

### Image Download Flow

The correct sequence is:
1. Fetch all characters from API → `CharacterData` (images not yet downloaded; `image_base64` is `None`)
2. Loop over all characters, download each `image_url` → set `image_base64` to the data URI string
3. Pass characters to `DictBuilder::add_character()` which reads `image_base64`

**Do NOT call `add_character()` before images are downloaded.** The structured content is built once per character and shared across all term entries.

### Double Consonant → っ

When two identical consonant characters appear consecutively (e.g., "kk" in "Nakka"), emit `っ` and advance by 1 (so the second consonant starts the next romaji match). Only applies to consonants (bcdfghjklmnpqrstvwxyz).

Example: "kappa" → k+a = か, p+p = っ, then p+a = ぱ → "かっぱ"

### `n` Before Vowel/y

The character `n` maps to `ん` ONLY when it is NOT followed by a vowel (a/i/u/e/o) or 'y'. If followed by a vowel, it starts a two-character sequence (na→な, ni→に, etc.).

Example: "anna" → a=あ, n+n=っ (double consonant), a=あ → "あっあ"... wait, actually "nn" → っ then n+a=な? Let me think again.

Actually: "anna" → a=あ, then chars[1]='n', chars[2]='n' — double consonant → っ, advance to i=2. Then chars[2]='n', chars[3]='a' → "na" matches → な. Result: "あっな". This is correct.

"kana" → k+a=か, n followed by a (vowel) → NOT ん, instead n+a=な → "かな". Correct.

"kantan" → k+a=か, n+? → 'n' followed by 't' (not vowel/y) → ん, then t+a=た, n at end (nothing follows) → ん → "かんたん". Correct.

### Axum Version Compatibility

Axum 0.7 removed `axum::Server`. Use `tokio::net::TcpListener` + `axum::serve()`:

```rust
let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
axum::serve(listener, app).await.unwrap();
```

### ZIP Writer Requires Seek

`zip::ZipWriter::new()` requires `Write + Seek`. Use `Cursor<Vec<u8>>`, NOT bare `Vec<u8>`:

```rust
let buffer = Vec::new();
let cursor = std::io::Cursor::new(buffer);
let mut zip = ZipWriter::new(cursor);
// ... write files ...
let cursor = zip.finish().unwrap();
let bytes: Vec<u8> = cursor.into_inner();
```

### AniList Media Type

The frontend must let users select "Anime" or "Manga" when using AniList. This value is passed as the `media_type` query parameter (`"ANIME"` or `"MANGA"`) and forwarded to the GraphQL query's `$type` variable.

### Entry Deduplication

Use a `HashSet<String>` tracking terms already added. Before adding any entry (base name, honorific variant, or alias), check `added_terms.insert(term)` — if it returns `false`, skip the entry. This prevents duplicates when, e.g., a family name happens to equal an alias.

---

## 16. DEPLOYMENT

### Local Development

```bash
cargo build
cargo run
# Visit http://localhost:3000
```

### Production

```bash
cargo build --release
./target/release/yomitan-dict-builder
# Put behind nginx/reverse proxy for HTTPS
```

The server binds to `0.0.0.0:3000`. For production, configure the `downloadUrl` base to use your public domain instead of `127.0.0.1:3000`. You may want to make the bind address and port configurable via environment variables.

---

**End of Plan**
