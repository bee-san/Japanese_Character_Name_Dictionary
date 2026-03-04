# Toggleable Character Information Categories

## Problem

Currently, the `show_traits` toggle is all-or-nothing: it controls the entire "Character Information" section as a single block. Users cannot selectively show/hide individual trait categories (e.g. keep "Personality" and "Voiced by" but hide "Hair" and "Body").

Additionally, VNDB returns many trait groups beyond the 4 currently captured (Personality, Role, Engages in, Subject of). Groups like Hair, Eyes, Body, and Clothes are silently discarded by the `_ => {}` match arm in `vndb_client.rs::process_character()`. The "Voiced by" field is a separate VNDB API field (`voiced`) not currently fetched at all.

AniList has no trait categories (its `process_character` sets all trait vecs to empty), but it does have voice actor data available via the `voiceActors` field on character edges in the GraphQL API.

## Goal

1. Capture ALL VNDB trait groups (Hair, Eyes, Body, Clothes, etc.) instead of discarding them.
2. Add "Voiced by" data from both VNDB and AniList.
3. Make each trait category independently toggleable via query parameters.
4. Preserve the URL-as-settings pattern (all toggles survive Yomitan update cycles).

## Current Architecture

### Data Model (`models.rs`)
```rust
pub struct Character {
    // ...
    pub personality: Vec<CharacterTrait>,  // VNDB "Personality" group
    pub roles: Vec<CharacterTrait>,        // VNDB "Role" group
    pub engages_in: Vec<CharacterTrait>,   // VNDB "Engages in" group
    pub subject_of: Vec<CharacterTrait>,   // VNDB "Subject of" group
    // No fields for Hair, Eyes, Body, Clothes, Voiced by, etc.
}
```

### VNDB Client (`vndb_client.rs`)
- Fetches `traits.name,traits.group_name,traits.spoiler` from VNDB API.
- Only maps 4 groups; all others hit `_ => {}` and are dropped.
- Does NOT request `voiced` field from VNDB API.

### AniList Client (`anilist_client.rs`)
- No trait categories at all (AniList doesn't have VNDB-style traits).
- GraphQL query does NOT request `voiceActors` from character edges.

### Content Builder (`content_builder.rs`)
- `DictSettings` has a single `show_traits: bool` toggle.
- `build_traits_by_category()` iterates over the 4 hardcoded category fields.
- `build_content()` gates the entire "Character Information" `<details>` block on `show_traits`.

### Frontend (`static/app.js`)
- Single `traits` toggle in the settings object.
- Single `preview-traits` element in the preview card.

### Query Params (`main.rs`)
- `DictQuery` and `GenerateStreamQuery` have `traits: bool`.
- `to_settings()` maps it to `DictSettings.show_traits`.
- `append_settings_params()` emits `traits=false` when disabled.

## Design

### Approach: Generic Trait Storage + Per-Category Toggles

Rather than adding a separate `Vec<CharacterTrait>` field for every possible VNDB group (fragile, requires model changes for each new group), use a single generic map that preserves whatever groups the API returns.

### 1. Model Changes (`models.rs`)

Replace the 4 separate trait vecs with a single ordered map, and add a voiced-by field:

```rust
pub struct Character {
    // ... existing fields unchanged ...

    // Replace personality, roles, engages_in, subject_of with:
    pub traits: IndexMap<String, Vec<CharacterTrait>>,
    // Keys are group names: "Personality", "Role", "Engages in", "Subject of",
    // "Hair", "Eyes", "Body", "Clothes", etc.
    // IndexMap preserves insertion order for consistent rendering.

    // Voice actor (new)
    pub voiced_by: Option<String>,  // e.g. "Shimashima Hakase" or "Hanazawa Kana"

    // ... image fields, hint fields unchanged ...
}
```

**Why IndexMap?** We want deterministic category ordering in the popup card. `IndexMap` (from the `indexmap` crate) preserves insertion order while still allowing O(1) lookups. The VNDB client inserts groups in the order they appear in the API response, which is consistent.

**Migration note:** All existing code that references `char.personality`, `char.roles`, `char.engages_in`, `char.subject_of` must be updated to use `char.traits.get("Personality")`, etc. This is a mechanical refactor.

### 2. VNDB Client Changes (`vndb_client.rs`)

**API fields:** Add `voiced` to the fields string:
```
"fields": "id,name,original,image.url,sex,birthday,age,blood_type,height,weight,description,aliases,vns.role,vns.id,traits.name,traits.group_name,traits.spoiler,voiced.name"
```

**Trait processing:** Replace the match-based routing with generic insertion:
```rust
// Before:
match group {
    "Personality" => personality.push(trait_obj),
    "Role" => roles.push(trait_obj),
    "Engages in" => engages_in.push(trait_obj),
    "Subject of" => subject_of.push(trait_obj),
    _ => {} // ← This drops Hair, Eyes, Body, Clothes, etc.
}

// After:
traits.entry(group.to_string()).or_default().push(trait_obj);
```

**Voiced by:** Extract from the `voiced` array in the VNDB response. VNDB returns voice actors per-VN, so filter for the target VN's voice actor:
```rust
let voiced_by = data["voiced"]
    .as_array()
    .and_then(|arr| {
        // voiced entries are per-VN; find the one for our target VN if available
        // If no VN filter, just take the first
        arr.iter()
            .find(|v| v["id"].as_str() == Some(target_vn))
            .or_else(|| arr.first())
    })
    .and_then(|v| v["name"].as_str())
    .map(|s| s.to_string());
```

Note: The exact VNDB `voiced` response structure needs verification. The VNDB API docs at https://api.vndb.org/kana#post-character show `voiced` as a nested field. The fields string should be `voiced.id,voiced.name` or similar. This should be tested against the live API during implementation.

### 3. AniList Client Changes (`anilist_client.rs`)

**GraphQL query:** Add `voiceActors` to the character edges:
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

**Process character:** Extract voice actor name:
```rust
let voiced_by = edge["voiceActors"]
    .as_array()
    .and_then(|arr| arr.first())
    .and_then(|va| {
        va["name"]["native"].as_str()
            .or_else(|| va["name"]["full"].as_str())
    })
    .map(|s| s.to_string());
```

AniList still won't have trait categories (Hair, Eyes, etc.) — those remain VNDB-only. The `traits` map will simply be empty for AniList characters. But voice actors work for both sources.

### 4. DictSettings & Toggle Design (`content_builder.rs`, `main.rs`)

#### Option A: Individual bool per category (rejected)
Adding `show_hair: bool`, `show_eyes: bool`, etc. is fragile — new VNDB groups would require code changes everywhere.

#### Option B: Exclusion set (chosen)
Use a set of category names to exclude. Default = show everything. Users opt OUT of specific categories.

```rust
pub struct DictSettings {
    pub show_image: bool,
    pub show_tag: bool,
    pub show_description: bool,
    pub show_traits: bool,        // Master toggle — if false, hides ALL traits
    pub show_spoilers: bool,
    pub honorifics: bool,
    pub show_voiced_by: bool,     // Toggle for "Voiced by" line
    pub hidden_trait_groups: HashSet<String>,  // Category names to hide, e.g. {"Hair", "Body"}
}
```

**Query parameter encoding:**

```
# Hide Hair and Body categories:
&hide_traits=Hair,Body

# Hide voiced by:
&voiced_by=false

# Master traits toggle still works:
&traits=false    (hides entire Character Information section)
```

The `hide_traits` param is a comma-separated list of group names to exclude. This is extensible — if VNDB adds new groups, they automatically appear without code changes, and users can hide them by name.

**Parsing in `DictQuery`:**
```rust
struct DictQuery {
    // ... existing fields ...
    #[serde(default = "default_true")]
    voiced_by: bool,
    #[serde(default)]
    hide_traits: Option<String>,  // Comma-separated group names to hide
}

impl DictQuery {
    fn to_settings(&self) -> DictSettings {
        let hidden = self.hide_traits
            .as_deref()
            .unwrap_or("")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();

        DictSettings {
            // ... existing mappings ...
            show_voiced_by: self.voiced_by,
            hidden_trait_groups: hidden,
        }
    }
}
```

**URL roundtrip:** `append_settings_params()` emits `hide_traits=Hair,Body` and `voiced_by=false` when non-default. This survives the Yomitan update cycle since it's just another query param.

### 5. Content Builder Changes (`content_builder.rs`)

**`build_traits_by_category()`:** Iterate over the `traits` IndexMap instead of hardcoded fields:

```rust
pub fn build_traits_by_category(&self, char: &Character) -> Vec<serde_json::Value> {
    let mut items = Vec::new();

    for (group_name, traits) in &char.traits {
        // Skip hidden groups
        if self.settings.hidden_trait_groups.contains(group_name) {
            continue;
        }

        let filtered: Vec<&str> = traits
            .iter()
            .filter(|t| !t.name.is_empty() && (self.settings.show_spoilers || t.spoiler == 0))
            .map(|t| t.name.as_str())
            .collect();

        if !filtered.is_empty() {
            items.push(json!({
                "tag": "li",
                "content": format!("{}: {}", group_name, filtered.join(", "))
            }));
        }
    }

    items
}
```

**`build_content()`:** Add "Voiced by" line inside the Character Information section, gated by `show_voiced_by`:

```rust
// Inside the show_traits block, after trait_items:
if self.settings.show_voiced_by {
    if let Some(ref va) = char.voiced_by {
        if !va.is_empty() {
            info_items.push(json!({
                "tag": "li",
                "content": format!("Voiced by: {}", va)
            }));
        }
    }
}
```

### 6. Frontend Changes (`static/index.html`, `static/app.js`)

The preview card should show the new categories and allow toggling. Two approaches:

**Simple approach (recommended for v1):** Keep the existing single "traits" toggle in the preview card. Add a "Voiced by" toggle. The per-category `hide_traits` param is an advanced/URL-only feature — power users can manually add `&hide_traits=Hair,Body` to their URL.

**Full approach (v2):** Dynamically render toggle buttons for each trait category in the preview card. This requires knowing which categories exist before generating, which means either:
- Hardcoding the known VNDB categories in the frontend
- Adding an API endpoint that returns available categories for a given character set

For v1, the frontend changes are minimal:
- Add a "Voiced by" toggle checkbox/button alongside the existing toggles
- Add `voiced_by` to the settings object and `settingsParams()` function
- Document `hide_traits` as an advanced URL parameter

### 7. Rendering Order

VNDB trait groups should render in a sensible order. Since `IndexMap` preserves insertion order, we can control this by inserting in a defined order during `process_character()`:

```rust
// Define preferred display order
const TRAIT_GROUP_ORDER: &[&str] = &[
    "Hair", "Eyes", "Body", "Clothes",
    "Personality", "Role", "Engages in", "Subject of",
];

// After collecting all traits into a temporary HashMap,
// re-insert into IndexMap in preferred order:
let mut ordered_traits = IndexMap::new();
for &group in TRAIT_GROUP_ORDER {
    if let Some(traits) = raw_traits.remove(group) {
        ordered_traits.insert(group.to_string(), traits);
    }
}
// Append any remaining groups not in the predefined order
for (group, traits) in raw_traits {
    ordered_traits.insert(group, traits);
}
```

This ensures consistent ordering while still being extensible.

## Implementation Steps

1. Add `indexmap` to `Cargo.toml` dependencies.
2. Refactor `Character` in `models.rs`: replace 4 trait vecs with `traits: IndexMap<String, Vec<CharacterTrait>>`, add `voiced_by: Option<String>`.
3. Update `vndb_client.rs`: generic trait insertion, add `voiced` to API fields, extract voice actor.
4. Update `anilist_client.rs`: add `voiceActors` to GraphQL query, extract voice actor name.
5. Update `DictSettings` in `content_builder.rs`: add `show_voiced_by` and `hidden_trait_groups`.
6. Update `build_traits_by_category()` to iterate the IndexMap and respect `hidden_trait_groups`.
7. Update `build_content()` to render "Voiced by" line.
8. Update `DictQuery` / `GenerateStreamQuery` in `main.rs`: add `voiced_by` and `hide_traits` params, update `to_settings()` and `append_settings_params()`.
9. Update frontend: add "Voiced by" toggle, update `settingsParams()`.
10. Update all tests that construct `Character` or `DictSettings` (mechanical — replace field names).
11. Test against live VNDB API to verify `voiced` field structure.
12. Test against live AniList API to verify `voiceActors` response.

## Compatibility Notes

- All new query params default to their current behavior (everything shown). Existing URLs continue to work unchanged.
- The `traits=false` master toggle still hides the entire section, overriding per-category settings.
- AniList characters will have an empty `traits` map (no change in rendered output) but may now show "Voiced by" if available.
- The `hide_traits` param uses group names as they come from VNDB (case-sensitive: "Hair" not "hair"). This matches what users see in the rendered card.

## Open Questions

1. **VNDB `voiced` field structure:** Need to verify the exact JSON shape returned by the VNDB API for `voiced.name`. The VNDB API docs should be checked, or a test request made. The field may be `voiced.staff.name` or similar.
2. **AniList voice actor language preference:** Currently hardcoded to `JAPANESE`. Should this be configurable? Most users of a Japanese dictionary builder want Japanese VAs, so probably fine as default.
3. **Frontend v2 timeline:** Should per-category toggles in the UI be part of this change, or deferred? Recommendation: defer to keep scope manageable. The URL-based `hide_traits` param covers power users immediately.
