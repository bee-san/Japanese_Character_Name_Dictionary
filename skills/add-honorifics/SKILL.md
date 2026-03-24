---
name: add-honorifics
description: Add or update Japanese honorific suffix support in the Yomitan Character Dictionary Builder. Use when a request involves adding a new honorific, changing an honorific reading or gloss, regrouping honorific categories, or verifying that honorific-generated dictionary entries and banners still behave correctly.
---

# Add Honorifics

## Overview

Implement honorific suffix changes in the Rust dictionary builder without breaking the generated Yomitan entries. Prefer the smallest change that satisfies the request: most additions only require editing the honorific data table and adding a focused regression test.

## Workflow

1. Read `agents.md` and `docs/plans/honorific_design.md` when the request changes honorific behavior or presentation, not just a single suffix entry.
2. Edit `yomitan-dict-builder/src/name_parser.rs`.
   - Update `HONORIFIC_SUFFIXES`, which uses `(display, hiragana_reading, english_description)` tuples.
   - Keep the surrounding category comments and nearby ordering consistent.
   - Add kana duplicates only when the project already treats them as separate searchable forms.
3. Decide whether other Rust files need changes.
   - Simple suffix additions usually need no logic changes because `yomitan-dict-builder/src/dict_builder.rs` expands honorific variants automatically.
   - Touch `yomitan-dict-builder/src/content_builder.rs` only when the honorific banner or structured-content presentation changes.
   - Touch `yomitan-dict-builder/src/dict_builder.rs` only when generation, deduplication, or export rules change.
4. Add or update tests close to the behavior you changed.
   - Put suffix-data assertions in `yomitan-dict-builder/src/name_parser.rs`.
   - Put generated-entry assertions in `yomitan-dict-builder/src/dict_builder.rs` when verifying emitted terms or banner propagation.
   - Keep tests targeted: assert the new suffix exists, has the expected reading/description, and is emitted in at least one generated term when behavior changes.
5. Update docs only when user-facing behavior or maintained counts change.
   - Reconcile any manually stated totals in `README.md`, `agents.md`, or docs if the request changes them.

## Checklist

- Use the exact reading users should search.
- Use a short English gloss that explains tone or context, not just a literal translation.
- Preserve the `HONORIFIC_SUFFIXES` tuple shape.
- Avoid duplicating entries that already exist under the same display and reading combination.
- Remember that adding data is enough for most honorific requests; broader code changes are the exception.

## Verification

Run targeted tests first:

```bash
cd /Users/skerraut/Documents/character_name_dict/yomitan-dict-builder
cargo test honorific
```

If you changed parsing logic more broadly, also run:

```bash
cd /Users/skerraut/Documents/character_name_dict/yomitan-dict-builder
cargo test name_parser
cargo test dict_builder
```
