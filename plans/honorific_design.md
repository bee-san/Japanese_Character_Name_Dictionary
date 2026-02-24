# Honorific Description Design — Self-Describing Honorific Entries

## Problem

Currently, when a user looks up `須々木さん` in Yomitan, they see the exact same popup card as `須々木` — the character's portrait, name, role, stats, etc. There's no indication of what `さん` means or why it's appended. The honorific is invisible in the result.

For learners, this is a missed opportunity. The dictionary already knows the honorific was matched — it generated the entry — but it doesn't communicate that knowledge to the user.

## Goal

When an honorific variant is looked up, the popup card should include a brief description of the honorific suffix that was matched. The user sees the normal character card plus a contextual note like:

> **さん** — Generic polite suffix (Mr./Ms./Mrs.)

This turns every honorific lookup into a mini-lesson without cluttering the base name entries.

## Design

### Approach: Honorific-Specific Structured Content

Instead of sharing the same `structured_content` between base entries and honorific entries, honorific entries get a modified version with an extra element prepended to the card content.

The honorific banner is a small styled `div` inserted at the top of the structured content, before the character's Japanese name. It shows the suffix and its English gloss.

### Data Model Change

Extend `HONORIFIC_SUFFIXES` in `name_parser.rs` from a 2-tuple to a 3-tuple:

```rust
// Before
pub const HONORIFIC_SUFFIXES: &[(&str, &str)] = &[
    ("さん", "さん"),           // Generic polite (Mr./Ms./Mrs.)
    ("様", "さま"),             // Very formal/respectful (Lord/Lady/Dear)
    ...
];

// After
pub const HONORIFIC_SUFFIXES: &[(&str, &str, &str)] = &[
    ("さん", "さん", "Generic polite suffix (Mr./Ms./Mrs.)"),
    ("様", "さま", "Very formal/respectful (Lord/Lady/Dear)"),
    ("さま", "さま", "Kana form of 様 — very formal/respectful"),
    ...
];
```

The third field is the English gloss. This is already present as comments in the current code — it just needs to be promoted to data.

### Structured Content Modification

Add a new function to `content_builder.rs`:

```rust
/// Build a structured content card with an honorific description banner.
/// Clones the base content and prepends a styled honorific note.
pub fn build_honorific_content(
    base_content: &serde_json::Value,
    suffix_display: &str,
    suffix_description: &str,
) -> serde_json::Value {
    let mut content_array = match base_content.get("content") {
        Some(serde_json::Value::Array(arr)) => arr.clone(),
        _ => return base_content.clone(),
    };

    // Honorific banner: styled div at the top of the card
    let banner = json!({
        "tag": "div",
        "style": {
            "fontSize": "0.85em",
            "color": "#4A90D9",
            "borderColor": "#4A90D9",
            "borderStyle": "solid",
            "borderWidth": "0 0 0 3px",
            "paddingLeft": "6px",
            "marginBottom": "6px"
        },
        "content": [
            {
                "tag": "span",
                "style": { "fontWeight": "bold" },
                "content": suffix_display
            },
            {
                "tag": "span",
                "content": format!(" — {}", suffix_description)
            }
        ]
    });

    content_array.insert(0, banner);

    json!({
        "type": "structured-content",
        "content": content_array
    })
}
```

### Dict Builder Changes

In `dict_builder.rs::add_character()`, the honorific loop currently does:

```rust
for (suffix, suffix_reading) in HONORIFIC_SUFFIXES {
    // ... creates entry with shared structured_content
}
```

Change to:

```rust
for (suffix, suffix_reading, description) in HONORIFIC_SUFFIXES {
    let term_with_suffix = format!("{}{}", base_name, suffix);
    let reading_with_suffix = format!("{}{}", base_reading, suffix_reading);

    if added_terms.insert(term_with_suffix.clone()) {
        let honorific_content = ContentBuilder::build_honorific_content(
            &structured_content,
            suffix,
            description,
        );
        self.entries.push(ContentBuilder::create_term_entry(
            &term_with_suffix,
            &reading_with_suffix,
            role,
            score,
            &honorific_content,
        ));
    }
}
```

Same change applies to the alias honorific loop.

### Visual Result

Looking up `須々木さん` in Yomitan would show:

```
┌─────────────────────────────────┐
│ ▎ さん — Generic polite suffix  │  ← NEW: honorific banner
│   (Mr./Ms./Mrs.)               │
│                                 │
│ 須々木 心一                      │  ← existing card content
│ Shinichi Suzuki                 │
│ [portrait]                      │
│ From: Ever17                    │
│ [Main Character]                │
│ ▸ Description                   │
│ ▸ Character Information         │
└─────────────────────────────────┘
```

Looking up `須々木` (no honorific) shows the card without the banner — unchanged from current behavior.

## Files to Modify

| File | Change |
|---|---|
| `name_parser.rs` | Change `HONORIFIC_SUFFIXES` from `(&str, &str)` to `(&str, &str, &str)`, add English gloss as third element to every entry |
| `content_builder.rs` | Add `build_honorific_content()` function |
| `dict_builder.rs` | Update both honorific loops (base names + aliases) to call `build_honorific_content()` and destructure the 3-tuple |
| Tests in all three files | Update to match new tuple shape and verify banner presence |

## Impact on ZIP Size

Each honorific entry now has its own structured content clone instead of sharing a reference. This increases the JSON size per honorific entry by ~150-200 bytes (the banner div). With ~200 suffixes × ~10 base names per character × ~50 characters per dictionary, that's roughly 10-20 MB additional uncompressed JSON. ZIP compression should reduce this significantly since the banner structures are highly repetitive.

If this becomes a concern, an alternative is to use Yomitan's `definitionTags` field (index 2 of the term entry) to encode the honorific info as a tag instead. Tags are defined once in `tag_bank_1.json` and referenced by name, avoiding content duplication. However, tags display differently in Yomitan (small colored pills above the definition) and offer less formatting control than structured content. The structured content approach is recommended for richer presentation.

## Testing

New unit tests:

1. `test_honorific_suffixes_have_descriptions()` — verify every entry in `HONORIFIC_SUFFIXES` has a non-empty third field
2. `test_build_honorific_content_prepends_banner()` — verify the returned content has the banner as first element
3. `test_build_honorific_content_preserves_base()` — verify the rest of the card is unchanged
4. `test_honorific_entry_uses_honorific_content()` — verify `add_character` produces entries with banners for honorific terms

## Migration

This is a non-breaking change. The ZIP format is identical — only the JSON content within term entries changes. Existing dictionaries continue to work. Users get the enhanced cards on their next dictionary regeneration.
