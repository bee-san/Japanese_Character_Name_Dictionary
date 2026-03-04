# Bug Report: `primary` tag appears in Yomitan when `tag=false`

## Symptom

When the "Main Character" tag toggle is disabled in the frontend UI, the generated Yomitan dictionary still shows the `primary` tag pill in entry headers:

```
name | primary | Bee's Character Dictionary
```

The role badge inside the popup card ("Main Character") is also expected to be hidden.

## Root Cause

The fix for this bug (`tag_role` variable in `dict_builder.rs:76`) exists as an **uncommitted local change** in the working tree. The committed code passes `role` (the character's actual role, e.g. `"primary"`) directly to `create_term_entry`, ignoring the `show_tag` setting entirely.

The diff shows 18 call sites in `dict_builder.rs` where `role` was replaced with `tag_role`:

```rust
// Line 76 (new, uncommitted):
let tag_role: &str = if self.settings.show_tag { role } else { "" };

// All create_term_entry calls changed from:
ContentBuilder::create_term_entry(..., role, ...)
// To:
ContentBuilder::create_term_entry(..., tag_role, ...)
```

Additionally, the running server process (PID 6262, started 11:11 AM) predates both the source modification (11:17 AM) and the binary rebuild (11:36 AM), so even the local fix isn't active.

## How definitionTags work in Yomitan

Each term entry in `term_bank_N.json` has a `definitionTags` field (element `[2]`), a space-separated string of tag names. Yomitan looks up each tag name in `tag_bank_1.json` by exact name match and displays them as colored pills. There is no category-based auto-application -- only tags explicitly listed in the `definitionTags` string are shown.

When `show_tag=true`: `definitionTags = "name primary"` -> pills: `name`, `primary`
When `show_tag=false`: `definitionTags = "name"` -> pill: `name` only

## Files affected

| File | Issue |
|---|---|
| `src/dict_builder.rs:76` | Core fix: `tag_role` replaces `role` in all `create_term_entry` calls (uncommitted) |
| `src/dict_builder.rs:461` (`create_tags`) | Always emits role tag definitions in `tag_bank_1.json` even when unused |
| `static/app.js:51` | Preview dims `primary` pill (opacity 0.3) instead of hiding it entirely |
| `src/content_builder.rs:382` | Role badge in structured content correctly gated by `show_tag` (no change needed) |

## Fix plan

1. **Commit the `tag_role` fix** already in the working tree (all 18 `role` -> `tag_role` substitutions)
2. **Conditionally exclude role tags from `tag_bank_1.json`** when `show_tag=false` (remove `main`, `primary`, `side`, `appears` definitions)
3. **Fix frontend preview**: change `app.js:51` from `opacity: 0.3` to `display: none` for the `primary` header pill
4. **Add test**: verify `entry[2] == "name"` (not `"name primary"`) when `show_tag=false`
5. **Restart the server** after building to pick up the new binary

## Verification

Confirmed via Yomitan source code (`ext/js/language/translator.js`, `_expandTagGroups` method) that tag resolution is strictly name-based: the `definitionTags` string is split by space, each name is looked up in the tag_bank's IndexedDB store by exact `name` match. No category inheritance or auto-application exists.
