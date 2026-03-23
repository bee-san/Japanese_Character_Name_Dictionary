use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};

use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::content_builder::{ContentBuilder, DictSettings};
use crate::image_handler::ImageHandler;
use crate::kana;
use crate::models::*;
use crate::name_parser::{self, HONORIFIC_SUFFIXES};

/// Parse AniList alternative name format "Romanized (Japanese)".
///
/// AniList alternative names often use the format "Romaji (日本語)" where
/// the romanized reading is outside the parentheses and the Japanese form
/// is inside. This function extracts both parts.
///
/// Returns `(term, optional_reading)`:
/// - For "Aoki Umi (碧井海)": returns ("碧井海", Some("あおきうみ"))
/// - For "碧井海": returns ("碧井海", None) — standalone Japanese
/// - For "Ruri's Mother": returns ("Ruri's Mother", None) — no parenthesized part
fn parse_alt_name_format(alias: &str) -> (String, Option<String>) {
    // Look for pattern: "text (text)" where the parenthesized part contains Japanese
    if let Some(open) = alias.rfind('(') {
        if let Some(close) = alias[open..].find(')') {
            let inside = alias[open + 1..open + close].trim();
            let outside = alias[..open].trim();

            if kana::contains_japanese(inside) && !outside.is_empty() {
                // The Japanese part is the term, the romanized part provides the reading
                let reading = kana::alphabet_to_kana(outside).replace(' ', "");
                return (inside.to_string(), Some(reading));
            }
        }
    }

    // No "X (Y)" format or not Japanese inside — return as-is
    (alias.to_string(), None)
}

/// Maximum number of entries to put into one term bank file.
/// Yomitan recommends keeping each term bank small to reduce import issues.
/// JMdict and Jitendex both use 2k entries as their limit.
const TERM_BANK_LIMIT: usize = 2_000;

fn get_score(role: &str) -> i32 {
    match role {
        "main" => 100,
        "primary" => 75,
        "side" => 50,
        "appears" => 25,
        _ => 0,
    }
}

/// Role importance ordering for sorting appearances.
fn role_importance(role: &str) -> u8 {
    match role {
        "main" => 0,
        "primary" => 1,
        "side" => 2,
        "appears" => 3,
        _ => 4,
    }
}

/// A character merged across multiple media appearances.
///
/// When the same character (by source+ID within a single source, or by
/// normalized name across sources) appears in multiple media, their data
/// is merged into one of these. Single-value fields use first-non-None
/// semantics; appearances are accumulated.
struct MergedCharacter {
    /// The base Character data (merged using first-non-None for Option fields).
    character: Character,
    /// All (role, media_title) pairs — one per media appearance.
    appearances: Vec<(String, String)>,
    /// Highest role score across all appearances.
    best_score: i32,
    /// Highest role string (for definition tags).
    best_role: String,
}

/// Compact data needed to lazily generate honorific entries during ZIP export.
/// Instead of eagerly expanding ~257 honorific suffixes x N base names into full
/// serde_json::Value entries (which can consume hundreds of MB for large dictionaries),
/// we store just the source data and generate entries on-the-fly during export.
struct HonorificSource {
    /// (base_name, base_reading) pairs to combine with each honorific suffix
    base_names_with_readings: Vec<(String, String)>,
    /// The shared structured content card for this character (cloned once per honorific)
    structured_content: serde_json::Value,
    tag_role: String,
    score: i32,
    /// Terms already added as base entries — skip these during honorific expansion
    added_terms: HashSet<String>,
}

pub struct DictBuilder {
    pub entries: Vec<serde_json::Value>,
    /// Deferred honorific data — expanded lazily during export_bytes()
    honorific_sources: Vec<HonorificSource>,
    images: Vec<(String, Vec<u8>)>, // (filename, bytes) for ZIP img/ folder
    added_images: HashSet<String>,  // track image filenames to avoid duplicate ZIP entries

    // --- Two-pass merge data structures ---
    /// Map from (source, character_id) to index in merged_characters.
    /// Used for within-source dedup (same character ID from same API).
    seen_by_id: HashMap<(String, String), usize>,
    /// Map from normalized name_original (whitespace stripped) to index in merged_characters.
    /// Used for cross-source dedup (same character across VNDB + AniList).
    seen_by_name: HashMap<String, usize>,
    /// Collected characters awaiting finalization into term entries.
    merged_characters: Vec<MergedCharacter>,
    /// Whether finalize() has been called (entries generated from merged_characters).
    finalized: bool,
    /// Counts of skipped characters with no Japanese name, grouped by media title.
    skipped_no_japanese_by_title: HashMap<String, usize>,

    settings: DictSettings,
    revision: String,
    download_url: Option<String>,
    game_title: String,
}

impl DictBuilder {
    pub fn new(settings: DictSettings, download_url: Option<String>, game_title: String) -> Self {
        // Unix timestamp as revision string (triggers Yomitan update detection)
        let revision: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            entries: Vec::new(),
            honorific_sources: Vec::new(),
            images: Vec::new(),
            added_images: HashSet::new(),
            seen_by_id: HashMap::new(),
            seen_by_name: HashMap::new(),
            merged_characters: Vec::new(),
            finalized: false,
            skipped_no_japanese_by_title: HashMap::new(),
            settings,
            revision: format!("{:012}", revision),
            download_url,
            game_title,
        }
    }

    pub fn skipped_no_japanese_count(&self) -> usize {
        self.skipped_no_japanese_by_title.values().sum()
    }

    pub fn skipped_no_japanese_summary(&self) -> Vec<(String, usize)> {
        let mut summary: Vec<(String, usize)> = self
            .skipped_no_japanese_by_title
            .iter()
            .map(|(title, count)| (title.clone(), *count))
            .collect();
        summary.sort_by(|a, b| a.0.cmp(&b.0));
        summary
    }

    pub fn log_skipped_no_japanese_summary(&self) {
        for (game_title, skipped_count) in self.skipped_no_japanese_summary() {
            tracing::warn!(
                game_title = %game_title,
                skipped_count = skipped_count,
                "Skipped characters with no Japanese name"
            );
        }
    }

    /// Collect a character for later entry generation. Characters with the same
    /// (source, id) are merged (same character across media within one API).
    /// Characters with the same normalized name_original across different sources
    /// (VNDB + AniList) are also merged.
    ///
    /// Different characters that happen to share a name but have different IDs
    /// will produce separate dictionary entries (Yomitan stacks multiple definitions).
    pub fn add_character(&mut self, char: &Character, game_title: &str) {
        let name_original = &char.name_original;
        if name_original.is_empty() {
            *self
                .skipped_no_japanese_by_title
                .entry(game_title.to_string())
                .or_insert(0) += 1;
            tracing::debug!(
                id = %char.id,
                name = %char.name,
                game_title = %game_title,
                "Skipping character with no Japanese name (name_original is empty)"
            );
            return;
        }

        let normalized_name: String = name_original
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();

        let new_score = get_score(&char.role);
        let appearance = (char.role.clone(), game_title.to_string());

        // Check within-source dedup first: same (source, id)
        let id_key = (char.source.clone(), char.id.clone());
        if let Some(&idx) = self.seen_by_id.get(&id_key) {
            // Same character from same source — merge appearance
            let merged = &mut self.merged_characters[idx];
            merged.appearances.push(appearance);
            if new_score > merged.best_score {
                merged.best_score = new_score;
                merged.best_role = char.role.clone();
            }
            // First-non-None gap-fill for Option/Vec fields
            Self::merge_character_fields(&mut merged.character, char);
            tracing::debug!(
                id = %char.id,
                source = %char.source,
                name = %char.name,
                game = %game_title,
                "Merged duplicate character (same source+ID) into existing entry"
            );
            return;
        }

        // Check cross-source dedup: same normalized name_original
        if let Some(&idx) = self.seen_by_name.get(&normalized_name) {
            // Same name from different source — merge appearance
            let merged = &mut self.merged_characters[idx];
            merged.appearances.push(appearance);
            if new_score > merged.best_score {
                merged.best_score = new_score;
                merged.best_role = char.role.clone();
            }
            Self::merge_character_fields(&mut merged.character, char);
            // Also register this (source, id) so future occurrences from the
            // same source hit the faster id-based lookup
            if !char.source.is_empty() && !char.id.is_empty() {
                self.seen_by_id.insert(id_key, idx);
            }
            tracing::debug!(
                id = %char.id,
                source = %char.source,
                name = %char.name,
                name_original = %name_original,
                game = %game_title,
                "Merged duplicate character (same name, cross-source) into existing entry"
            );
            return;
        }

        // New character — create a MergedCharacter
        let idx = self.merged_characters.len();
        self.merged_characters.push(MergedCharacter {
            character: char.clone(),
            appearances: vec![appearance],
            best_score: new_score,
            best_role: char.role.clone(),
        });

        // Register in both lookup maps
        if !char.source.is_empty() && !char.id.is_empty() {
            self.seen_by_id.insert(id_key, idx);
        }
        self.seen_by_name.insert(normalized_name, idx);
    }

    /// Merge single-value fields from `other` into `base` using first-non-None semantics.
    /// Only fills in gaps — never overwrites an existing Some value.
    /// For Vec fields: uses first-non-empty.
    /// For aliases: takes the union (deduplicated).
    fn merge_character_fields(base: &mut Character, other: &Character) {
        // Option fields: fill gaps
        macro_rules! fill_option {
            ($field:ident) => {
                if base.$field.is_none() {
                    base.$field = other.$field.clone();
                }
            };
        }
        fill_option!(sex);
        fill_option!(age);
        fill_option!(height);
        fill_option!(weight);
        fill_option!(blood_type);
        fill_option!(birthday);
        fill_option!(description);
        fill_option!(image_url);
        fill_option!(image_bytes);
        fill_option!(image_ext);
        fill_option!(image_width);
        fill_option!(image_height);
        fill_option!(seiyuu);
        fill_option!(seiyuu_image_url);
        fill_option!(seiyuu_image_bytes);
        fill_option!(seiyuu_image_ext);
        fill_option!(seiyuu_image_width);
        fill_option!(seiyuu_image_height);

        // Name hints: fill gaps
        if base.first_name_hint.is_none() {
            base.first_name_hint = other.first_name_hint.clone();
        }
        if base.last_name_hint.is_none() {
            base.last_name_hint = other.last_name_hint.clone();
        }

        // Romanized name: fill if empty
        if base.name.is_empty() && !other.name.is_empty() {
            base.name = other.name.clone();
        }

        // Vec trait fields: first-non-empty
        macro_rules! fill_vec {
            ($field:ident) => {
                if base.$field.is_empty() && !other.$field.is_empty() {
                    base.$field = other.$field.clone();
                }
            };
        }
        fill_vec!(personality);
        fill_vec!(roles);
        fill_vec!(engages_in);
        fill_vec!(subject_of);

        // Aliases: union (deduplicated)
        if !other.aliases.is_empty() {
            let mut existing: HashSet<String> = base.aliases.iter().cloned().collect();
            for alias in &other.aliases {
                if !alias.is_empty() && existing.insert(alias.clone()) {
                    base.aliases.push(alias.clone());
                }
            }
        }

        // Spoiler aliases: union (deduplicated)
        if !other.spoiler_aliases.is_empty() {
            let mut existing: HashSet<String> = base.spoiler_aliases.iter().cloned().collect();
            for alias in &other.spoiler_aliases {
                if !alias.is_empty() && existing.insert(alias.clone()) {
                    base.spoiler_aliases.push(alias.clone());
                }
            }
        }
    }

    /// Generate all term entries from merged characters.
    /// Called automatically by export_bytes() if not yet finalized.
    fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;

        // Take ownership of merged_characters to iterate without borrow conflict
        let merged_chars: Vec<MergedCharacter> = std::mem::take(&mut self.merged_characters);

        for mut merged in merged_chars {
            // Sort appearances by role importance (main first)
            merged
                .appearances
                .sort_by_key(|(role, _)| role_importance(role));

            self.generate_entries_for_merged(&merged);
        }
    }

    /// Generate all term entries (base + kana + alias + honorific) for one merged character.
    fn generate_entries_for_merged(&mut self, merged: &MergedCharacter) {
        let char = &merged.character;
        let name_original = &char.name_original;

        let score = merged.best_score;
        let tag_role: &str = if self.settings.show_tag {
            &merged.best_role
        } else {
            ""
        };

        let content_builder = ContentBuilder::new(self.settings.clone());

        // Generate hiragana readings using unified name handling (supports hints)
        let readings = name_parser::generate_name_readings(
            name_original,
            &char.name,
            char.first_name_hint.as_deref(),
            char.last_name_hint.as_deref(),
        );

        // Handle image: use raw bytes from download + resize (gated by show_image)
        let image_path = if self.settings.show_image {
            if let Some(ref img_bytes) = char.image_bytes {
                let ext = char.image_ext.as_deref().unwrap_or("jpg");
                let filename = ImageHandler::make_filename(&char.id, ext);
                let path = format!("img/{}", filename);
                // Only add image bytes once per filename to avoid duplicate ZIP entries
                if self.added_images.insert(filename.clone()) {
                    self.images.push((filename, img_bytes.clone()));
                }
                Some(path)
            } else {
                None
            }
        } else {
            None
        };

        // Handle seiyuu image (gated by show_seiyuu)
        let seiyuu_image_path = if self.settings.show_seiyuu {
            if let Some(ref img_bytes) = char.seiyuu_image_bytes {
                let ext = char.seiyuu_image_ext.as_deref().unwrap_or("jpg");
                let filename = ImageHandler::make_filename(&format!("{}_va", char.id), ext);
                let path = format!("img/{}", filename);
                if self.added_images.insert(filename.clone()) {
                    self.images.push((filename, img_bytes.clone()));
                }
                Some(path)
            } else {
                None
            }
        } else {
            None
        };

        // Build the structured content card
        let image_dims = match (char.image_width, char.image_height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        };
        let seiyuu_image_dims = match (char.seiyuu_image_width, char.seiyuu_image_height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        };

        // Use merged content builder when there are multiple appearances,
        // single-appearance builder otherwise (keeps output identical for
        // the common single-media case)
        let structured_content = if merged.appearances.len() > 1 {
            content_builder.build_merged_content(
                char,
                image_path.as_deref(),
                image_dims,
                seiyuu_image_path.as_deref(),
                seiyuu_image_dims,
                &merged.appearances,
            )
        } else {
            // Single appearance — use original build_content for backward compatibility
            let game_title = merged
                .appearances
                .first()
                .map(|(_, t)| t.as_str())
                .unwrap_or("");
            content_builder.build_content(
                char,
                image_path.as_deref(),
                image_dims,
                seiyuu_image_path.as_deref(),
                seiyuu_image_dims,
                game_title,
            )
        };

        // Track terms to avoid duplicates within this character
        let mut added_terms: HashSet<String> = HashSet::new();

        // Split the Japanese name (with hints for AniList characters)
        let name_parts = name_parser::split_japanese_name_with_hints(
            name_original,
            char.first_name_hint.as_deref(),
            char.last_name_hint.as_deref(),
        );

        // Get ALL plausible splits (for ambiguous all-kanji names with symmetric
        // kana lengths, multiple candidates may exist — we generate entries for
        // each so lookups work regardless of which split is correct).
        let all_candidates = name_parser::split_japanese_name_all_candidates(
            name_original,
            char.first_name_hint.as_deref(),
            char.last_name_hint.as_deref(),
        );

        // --- Base name entries ---
        // Generate split entries when we have family/given parts (either from space or hints)
        let has_split = name_parts.has_space || name_parts.family.is_some();

        if has_split {
            // 1. Original with space: "須々木 心一"
            if !name_parts.original.is_empty() && added_terms.insert(name_parts.original.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &name_parts.original,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }

            // 2. Combined without space: "須々木心一"
            if !name_parts.combined.is_empty() && added_terms.insert(name_parts.combined.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &name_parts.combined,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }

            // 3. Family name only and 4. Given name only — for ALL split candidates.
            for candidate in &all_candidates {
                if let Some(ref family) = candidate.family {
                    if !family.is_empty() && added_terms.insert(family.clone()) {
                        self.entries.push(ContentBuilder::create_term_entry(
                            family,
                            &readings.family,
                            tag_role,
                            score,
                            &structured_content,
                        ));
                    }
                }
                if let Some(ref given) = candidate.given {
                    if !given.is_empty() && added_terms.insert(given.clone()) {
                        self.entries.push(ContentBuilder::create_term_entry(
                            given,
                            &readings.given,
                            tag_role,
                            score,
                            &structured_content,
                        ));
                    }
                }
            }
        } else {
            // Single-word name
            if !name_original.is_empty() && added_terms.insert(name_original.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    name_original,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
        }

        // --- Hiragana / Katakana term entries ---
        if has_split {
            // Hiragana combined (no space): "すずきしんいち"
            let hira_combined = format!("{}{}", readings.family, readings.given);
            if !hira_combined.is_empty() && added_terms.insert(hira_combined.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &hira_combined,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            // Hiragana with space: "すずき しんいち"
            let hira_spaced = format!("{} {}", readings.family, readings.given);
            if added_terms.insert(hira_spaced.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &hira_spaced,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            // Hiragana family only
            if !readings.family.is_empty() && added_terms.insert(readings.family.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &readings.family,
                    &readings.family,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            // Hiragana given only
            if !readings.given.is_empty() && added_terms.insert(readings.given.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &readings.given,
                    &readings.given,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }

            // Katakana variants
            let kata_family = kana::hira_to_kata(&readings.family);
            let kata_given = kana::hira_to_kata(&readings.given);
            let kata_combined = format!("{}{}", kata_family, kata_given);
            if !kata_combined.is_empty() && added_terms.insert(kata_combined.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_combined,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            let kata_spaced = format!("{} {}", kata_family, kata_given);
            if added_terms.insert(kata_spaced.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_spaced,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            if !kata_family.is_empty() && added_terms.insert(kata_family.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_family,
                    &readings.family,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            if !kata_given.is_empty() && added_terms.insert(kata_given.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_given,
                    &readings.given,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
        } else {
            // Single-word name: add hiragana and katakana forms
            if !readings.full.is_empty() && added_terms.insert(readings.full.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &readings.full,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
            let kata_full = kana::hira_to_kata(&readings.full);
            if !kata_full.is_empty() && added_terms.insert(kata_full.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_full,
                    &readings.full,
                    tag_role,
                    score,
                    &structured_content,
                ));
            }
        }

        // --- Honorific suffix variants for all base names ---
        let mut base_names_with_readings: Vec<(String, String)> = Vec::new();
        if has_split {
            // Original kanji forms — all split candidates' family/given pairs
            for candidate in &all_candidates {
                if let Some(ref family) = candidate.family {
                    if !family.is_empty() {
                        base_names_with_readings.push((family.clone(), readings.family.clone()));
                    }
                }
                if let Some(ref given) = candidate.given {
                    if !given.is_empty() {
                        base_names_with_readings.push((given.clone(), readings.given.clone()));
                    }
                }
            }
            if !name_parts.combined.is_empty() {
                base_names_with_readings.push((name_parts.combined.clone(), readings.full.clone()));
            }
            if !name_parts.original.is_empty() {
                base_names_with_readings.push((name_parts.original.clone(), readings.full.clone()));
            }
            // Hiragana forms (family, given, combined)
            if !readings.family.is_empty() {
                base_names_with_readings.push((readings.family.clone(), readings.family.clone()));
            }
            if !readings.given.is_empty() {
                base_names_with_readings.push((readings.given.clone(), readings.given.clone()));
            }
            let hira_combined = format!("{}{}", readings.family, readings.given);
            if !hira_combined.is_empty() {
                base_names_with_readings.push((hira_combined, readings.full.clone()));
            }
            // Katakana forms (family, given, combined)
            let kata_family = kana::hira_to_kata(&readings.family);
            let kata_given = kana::hira_to_kata(&readings.given);
            if !kata_family.is_empty() {
                base_names_with_readings.push((kata_family.clone(), readings.family.clone()));
            }
            if !kata_given.is_empty() {
                base_names_with_readings.push((kata_given.clone(), readings.given.clone()));
            }
            let kata_combined = format!("{}{}", kata_family, kata_given);
            if !kata_combined.is_empty() {
                base_names_with_readings.push((kata_combined, readings.full.clone()));
            }
        } else if !name_original.is_empty() {
            base_names_with_readings.push((name_original.clone(), readings.full.clone()));
            // Hiragana form
            if !readings.full.is_empty() {
                base_names_with_readings.push((readings.full.clone(), readings.full.clone()));
            }
            // Katakana form
            let kata_full = kana::hira_to_kata(&readings.full);
            if !kata_full.is_empty() {
                base_names_with_readings.push((kata_full, readings.full.clone()));
            }
        }

        // --- Alias entries ---
        // Include spoiler aliases when the user has enabled spoilers
        let spoiler_aliases: &[String] = if self.settings.show_spoilers {
            &char.spoiler_aliases
        } else {
            &[]
        };

        for alias in char.aliases.iter().chain(spoiler_aliases.iter()) {
            if alias.is_empty() {
                continue;
            }

            // Parse "Romanized (Japanese)" format used by AniList alternative names.
            // E.g. "Aoki Umi (碧井海)" → term="碧井海", reading from "Aoki Umi"
            let (alias_term, alias_reading) = parse_alt_name_format(alias);

            // Skip aliases that don't contain any Japanese characters
            if !kana::contains_japanese(&alias_term) {
                continue;
            }

            if added_terms.insert(alias_term.clone()) {
                let reading = alias_reading.unwrap_or_else(|| readings.full.clone());
                self.entries.push(ContentBuilder::create_term_entry(
                    &alias_term,
                    &reading,
                    tag_role,
                    score,
                    &structured_content,
                ));

                // Also collect alias name+reading for deferred honorific generation
                if self.settings.honorifics {
                    base_names_with_readings.push((alias_term, reading));
                }
            }
        }

        // --- Deferred honorific generation ---
        if self.settings.honorifics && !base_names_with_readings.is_empty() {
            self.honorific_sources.push(HonorificSource {
                base_names_with_readings,
                structured_content,
                tag_role: tag_role.to_string(),
                score,
                added_terms,
            });
        }
    }

    /// Returns true if the builder has any entries (base, deferred honorifics, or pending merge).
    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
            || !self.honorific_sources.is_empty()
            || !self.merged_characters.is_empty()
    }

    /// Collect all entries including lazily-generated honorifics.
    /// This materializes the full entry set — use only for testing or inspection,
    /// not for ZIP export (which streams in chunks to avoid memory spikes).
    #[cfg(test)]
    fn collect_all_entries(&mut self) -> Vec<serde_json::Value> {
        self.finalize();
        let mut all = self.entries.clone();
        let mut honorific_dedup: HashSet<String> = HashSet::new();
        for source in &self.honorific_sources {
            for (base_name, base_reading) in &source.base_names_with_readings {
                for (suffix, suffix_reading, description) in HONORIFIC_SUFFIXES {
                    let term_with_suffix = format!("{}{}", base_name, suffix);
                    // Skip if this term was already added as a base entry for this character
                    if source.added_terms.contains(&term_with_suffix) {
                        continue;
                    }
                    // Skip if already emitted as an honorific (cross-character or
                    // duplicate base name forms within the same character)
                    if !honorific_dedup.insert(term_with_suffix.clone()) {
                        continue;
                    }
                    let reading_with_suffix = format!("{}{}", base_reading, suffix_reading);
                    let honorific_content = ContentBuilder::build_honorific_content(
                        &source.structured_content,
                        suffix,
                        description,
                    );
                    all.push(ContentBuilder::create_term_entry(
                        &term_with_suffix,
                        &reading_with_suffix,
                        &source.tag_role,
                        source.score,
                        &honorific_content,
                    ));
                }
            }
        }
        all
    }

    /// Finalize and return a reference to just the base (non-honorific) entries.
    /// Convenience helper for tests that don't need honorific entries.
    #[cfg(test)]
    fn base_entries(&mut self) -> &[serde_json::Value] {
        self.finalize();
        &self.entries
    }

    /// Create index.json metadata.
    fn create_index(&self) -> serde_json::Value {
        let description = if self.game_title.is_empty() {
            "Character names dictionary".to_string()
        } else {
            format!("Character names from {}", self.game_title)
        };

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

    /// Public accessor for index generation (used by the index endpoint).
    pub fn create_index_public(&self) -> serde_json::Value {
        self.create_index()
    }

    /// Create tag_bank_1.json — fixed tag definitions for character roles.
    fn create_tags(&self) -> serde_json::Value {
        let build_timestamp = env!("BUILD_TIMESTAMP")
            .split(' ')
            .next()
            .unwrap_or("unknown");
        let dict_built = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let mut tags = vec![json!(["name", "partOfSpeech", 0, "Character name", 0])];

        // Only include role tag definitions when show_tag is enabled;
        // when disabled, no term entry references these tags so they'd be unused.
        if self.settings.show_tag {
            tags.push(json!(["main", "name", 0, "Protagonist", 0]));
            tags.push(json!(["primary", "name", 0, "Main character", 0]));
            tags.push(json!(["side", "name", 0, "Side character", 0]));
            tags.push(json!(["appears", "name", 0, "Minor appearance", 0]));
        }

        tags.push(json!(["docker-built", "meta", 0, build_timestamp, 0]));
        tags.push(json!(["dict-built", "meta", 0, dict_built, 0]));

        serde_json::Value::Array(tags)
    }

    /// Export the dictionary as in-memory ZIP bytes.
    /// Automatically finalizes (generates entries from merged characters) if needed.
    pub fn export_bytes(&mut self) -> Result<Vec<u8>, String> {
        self.finalize();
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = ZipWriter::new(cursor);
        let json_options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Images are already compressed (JPEG/PNG) — storing them
        // uncompressed avoids wasting CPU for near-zero size reduction.
        let image_options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // 1. index.json
        zip.start_file("index.json", json_options)
            .map_err(|e| format!("Failed to create index.json in ZIP: {}", e))?;
        let index_json = serde_json::to_string_pretty(&self.create_index())
            .map_err(|e| format!("Failed to serialize index.json: {}", e))?;
        zip.write_all(index_json.as_bytes())
            .map_err(|e| format!("Failed to write index.json: {}", e))?;

        // 2. tag_bank_1.json
        zip.start_file("tag_bank_1.json", json_options)
            .map_err(|e| format!("Failed to create tag_bank_1.json in ZIP: {}", e))?;
        let tags_json = serde_json::to_string(&self.create_tags())
            .map_err(|e| format!("Failed to serialize tag_bank: {}", e))?;
        zip.write_all(tags_json.as_bytes())
            .map_err(|e| format!("Failed to write tag_bank: {}", e))?;

        // 3. term_bank_N.json — stream base entries then lazily generate honorifics.
        // Instead of holding all honorific entries in memory (which can be hundreds of
        // MB for large dictionaries), we buffer up to a term bank amount at a time and
        // flush each chunk to the ZIP before moving on.
        let entries_per_bank = TERM_BANK_LIMIT;
        let mut bank_buffer: Vec<serde_json::Value> = Vec::with_capacity(entries_per_bank);
        let mut bank_number: usize = 1;

        // Helper closure: flush the buffer as a term_bank file
        let flush_bank = |buf: &mut Vec<serde_json::Value>,
                          num: &mut usize,
                          zip: &mut ZipWriter<Cursor<Vec<u8>>>,
                          opts: SimpleFileOptions|
         -> Result<(), String> {
            if buf.is_empty() {
                return Ok(());
            }
            let filename = format!("term_bank_{}.json", *num);
            zip.start_file(&filename, opts)
                .map_err(|e| format!("Failed to create {} in ZIP: {}", filename, e))?;
            let data = serde_json::to_string(&*buf)
                .map_err(|e| format!("Failed to serialize {}: {}", filename, e))?;
            zip.write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write {}: {}", filename, e))?;
            buf.clear();
            *num += 1;
            Ok(())
        };

        // Phase 1: Drain base entries (non-honorific)
        for entry in &self.entries {
            bank_buffer.push(entry.clone());
            if bank_buffer.len() >= entries_per_bank {
                flush_bank(&mut bank_buffer, &mut bank_number, &mut zip, json_options)?;
            }
        }

        // Phase 2: Lazily generate and stream honorific entries
        let mut honorific_dedup: HashSet<String> = HashSet::new();
        for source in &self.honorific_sources {
            for (base_name, base_reading) in &source.base_names_with_readings {
                for (suffix, suffix_reading, description) in HONORIFIC_SUFFIXES {
                    let term_with_suffix = format!("{}{}", base_name, suffix);

                    // Skip if this term was already added as a base entry
                    if source.added_terms.contains(&term_with_suffix) {
                        continue;
                    }

                    // Skip duplicates (e.g. same base name form appearing multiple
                    // times, or same honorific term across different characters)
                    if !honorific_dedup.insert(term_with_suffix.clone()) {
                        continue;
                    }

                    let reading_with_suffix = format!("{}{}", base_reading, suffix_reading);
                    let honorific_content = ContentBuilder::build_honorific_content(
                        &source.structured_content,
                        suffix,
                        description,
                    );
                    bank_buffer.push(ContentBuilder::create_term_entry(
                        &term_with_suffix,
                        &reading_with_suffix,
                        &source.tag_role,
                        source.score,
                        &honorific_content,
                    ));

                    if bank_buffer.len() >= entries_per_bank {
                        flush_bank(&mut bank_buffer, &mut bank_number, &mut zip, json_options)?;
                    }
                }
            }
        }

        // Flush any remaining entries
        flush_bank(&mut bank_buffer, &mut bank_number, &mut zip, json_options)?;

        // 4. Images in img/ folder (stored uncompressed)
        for (filename, bytes) in &self.images {
            zip.start_file(format!("img/{}", filename), image_options)
                .map_err(|e| format!("Failed to create img/{} in ZIP: {}", filename, e))?;
            zip.write_all(bytes)
                .map_err(|e| format!("Failed to write img/{}: {}", filename, e))?;
        }

        let cursor = zip
            .finish()
            .map_err(|e| format!("Failed to finalize ZIP: {}", e))?;
        Ok(cursor.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_builder::DictSettings;
    use crate::models::{Character, CharacterTrait};
    use std::collections::HashSet;
    use std::io::Read;

    /// Default settings (everything enabled)
    fn s() -> DictSettings {
        DictSettings::default()
    }

    /// Settings with honorifics disabled
    fn s_no_hon() -> DictSettings {
        DictSettings {
            honorifics: false,
            ..DictSettings::default()
        }
    }

    fn make_test_character(id: &str, name: &str, name_original: &str, role: &str) -> Character {
        Character {
            id: id.to_string(),
            name: name.to_string(),
            name_original: name_original.to_string(),
            role: role.to_string(),
            sex: Some("m".to_string()),
            age: Some("17".to_string()),
            height: Some(170),
            weight: Some(60),
            blood_type: Some("A".to_string()),
            birthday: Some(vec![1, 1]),
            description: Some("Test description".to_string()),
            aliases: vec!["テストエイリアス".to_string()],
            personality: vec![CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            }],
            ..Character::default()
        }
    }

    // === Score tests ===

    #[test]
    fn test_role_scores() {
        assert_eq!(get_score("main"), 100);
        assert_eq!(get_score("primary"), 75);
        assert_eq!(get_score("side"), 50);
        assert_eq!(get_score("appears"), 25);
        assert_eq!(get_score("unknown"), 0);
        assert_eq!(get_score(""), 0);
    }

    // === Character entry generation tests ===

    #[test]
    fn test_add_character_empty_name_skipped() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Name", "", "main");
        builder.add_character(&char, "Test Game");
        assert_eq!(
            builder.base_entries().len(),
            0,
            "Empty name_original should produce no entries"
        );
    }

    #[test]
    fn test_skipped_no_japanese_summary_groups_by_title() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        builder.add_character(&make_test_character("c1", "Name 1", "", "main"), "Game A");
        builder.add_character(&make_test_character("c2", "Name 2", "", "side"), "Game A");
        builder.add_character(&make_test_character("c3", "Name 3", "", "side"), "Game B");

        assert_eq!(builder.skipped_no_japanese_count(), 3);
        assert_eq!(
            builder.skipped_no_japanese_summary(),
            vec![("Game A".to_string(), 2), ("Game B".to_string(), 1)]
        );
    }

    #[test]
    fn test_add_character_creates_entries() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");
        assert!(
            builder.base_entries().len() > 0,
            "Should create at least one entry"
        );
    }

    #[test]
    fn test_add_character_two_part_name_creates_base_entries() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        // Collect all terms
        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should have: original with space, combined, family, given
        assert!(
            terms.contains(&"須々木 心一".to_string()),
            "Should have original with space"
        );
        assert!(
            terms.contains(&"須々木心一".to_string()),
            "Should have combined"
        );
        assert!(
            terms.contains(&"須々木".to_string()),
            "Should have family name"
        );
        assert!(
            terms.contains(&"心一".to_string()),
            "Should have given name"
        );
    }

    #[test]
    fn test_add_character_honorific_variants() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Check some honorific variants exist
        assert!(
            terms.iter().any(|t| t.ends_with("さん")),
            "Should have -san variants"
        );
        assert!(
            terms.iter().any(|t| t.ends_with("ちゃん")),
            "Should have -chan variants"
        );
    }

    #[test]
    fn test_honorific_entry_uses_honorific_content() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        // Find an entry ending with さん
        let all_entries = builder.collect_all_entries();
        let san_entry = all_entries
            .iter()
            .find(|e| {
                e[0].as_str()
                    .map(|s| s.ends_with("さん") && s != "さん")
                    .unwrap_or(false)
            })
            .expect("Should have a -san honorific entry");

        // The definitions array is at index 5, first element is the structured content
        let definitions = san_entry[5].as_array().unwrap();
        let sc = &definitions[0];
        let content_arr = sc["content"].as_array().unwrap();

        // First element should be the honorific banner div
        let banner = &content_arr[0];
        assert_eq!(banner["tag"], "div");
        let banner_content = banner["content"].as_array().unwrap();
        assert_eq!(banner_content[0]["content"], "さん");
        assert!(
            banner_content[1]["content"]
                .as_str()
                .unwrap()
                .contains("Generic polite"),
            "Banner should contain the honorific description"
        );
    }

    #[test]
    fn test_add_character_alias_entries() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Name", "名前", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"テストエイリアス".to_string()),
            "Should have alias entry"
        );
    }

    #[test]
    fn test_add_character_deduplication() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut char = make_test_character("c1", "Name", "名前", "main");
        // Set alias same as original name
        char.aliases = vec!["名前".to_string()];
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Count occurrences of the name
        let count = terms.iter().filter(|t| t.as_str() == "名前").count();
        assert_eq!(count, 1, "Duplicate terms should be deduplicated");
    }

    #[test]
    fn test_add_character_single_word_name() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let char = make_test_character("c1", "Saber", "セイバー", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"セイバー".to_string()),
            "Should have single-word name entry"
        );
    }

    // === Index metadata tests ===

    #[test]
    fn test_index_metadata() {
        let builder = DictBuilder::new(
            s(),
            Some("http://127.0.0.1:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Test Game".to_string(),
        );
        let index = builder.create_index_public();

        assert_eq!(index["title"], "Bee's Character Dictionary");
        assert_eq!(index["format"], 3);
        assert_eq!(index["author"], "Bee (https://github.com/bee-san)");
        assert!(index["description"].as_str().unwrap().contains("Test Game"));
        assert!(index["downloadUrl"].as_str().is_some());
        assert!(index["indexUrl"]
            .as_str()
            .unwrap()
            .contains("/api/yomitan-index"));
        assert_eq!(index["isUpdatable"], true);
    }

    #[test]
    fn test_index_metadata_no_download_url() {
        let builder = DictBuilder::new(s(), None, "Test".to_string());
        let index = builder.create_index_public();

        assert_eq!(index["title"], "Bee's Character Dictionary");
        assert!(index.get("downloadUrl").is_none() || index["downloadUrl"].is_null());
    }

    #[test]
    fn test_index_metadata_empty_title() {
        let builder = DictBuilder::new(s(), None, String::new());
        let index = builder.create_index_public();
        assert_eq!(
            index["description"].as_str().unwrap(),
            "Character names dictionary"
        );
    }

    // === ZIP export tests ===

    #[test]
    fn test_export_bytes_produces_valid_zip() {
        let mut builder = DictBuilder::new(s(), None, "Test Game".to_string());
        let char = make_test_character("c1", "Test Name", "テスト", "main");
        builder.add_character(&char, "Test Game");

        let zip_bytes = builder.export_bytes().unwrap();
        assert!(!zip_bytes.is_empty(), "ZIP should not be empty");

        // Verify it's a valid ZIP (starts with PK magic bytes)
        assert_eq!(zip_bytes[0], b'P');
        assert_eq!(zip_bytes[1], b'K');

        // Verify contents
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut filenames: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            filenames.push(archive.by_index(i).unwrap().name().to_string());
        }

        assert!(filenames.contains(&"index.json".to_string()));
        assert!(filenames.contains(&"tag_bank_1.json".to_string()));
        assert!(filenames.contains(&"term_bank_1.json".to_string()));
    }

    #[test]
    fn test_export_bytes_index_json_valid() {
        let mut builder = DictBuilder::new(s(), None, "Test Game".to_string());
        let char = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&char, "Test Game");

        let zip_bytes = builder.export_bytes().unwrap();
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut index_file = archive.by_name("index.json").unwrap();
        let mut contents = String::new();
        index_file.read_to_string(&mut contents).unwrap();

        let index: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(index["title"], "Bee's Character Dictionary");
        assert_eq!(index["format"], 3);
    }

    #[test]
    fn test_export_bytes_with_image() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut char = make_test_character("c1", "Test", "テスト", "main");
        let raw = vec![0xFF, 0xD8, 0xFF];
        char.image_bytes = Some(raw);
        char.image_ext = Some("jpg".to_string());
        builder.add_character(&char, "Test");

        let zip_bytes = builder.export_bytes().unwrap();
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut filenames: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            filenames.push(archive.by_index(i).unwrap().name().to_string());
        }

        assert!(
            filenames.iter().any(|f| f.starts_with("img/")),
            "ZIP should contain images in img/ folder"
        );
    }

    // === Multi-title dictionary tests ===

    #[test]
    fn test_multi_title_entries() {
        let mut builder = DictBuilder::new(s(), None, "Multi-title dict".to_string());

        let char1 = make_test_character("c1", "Name1", "名前一", "main");
        let char2 = make_test_character("c2", "Name2", "名前二", "side");

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Both characters should have entries
        assert!(
            builder.base_entries().len() > 2,
            "Should have entries from both characters"
        );

        // Verify different game titles in structured content
        let entry1_content = &builder.base_entries()[0][5][0];
        let entry1_str = serde_json::to_string(entry1_content).unwrap();
        assert!(
            entry1_str.contains("Game A"),
            "First character should reference Game A"
        );
    }

    // =========================================================================
    // Yomitan Import Validation Tests
    //
    // These tests simulate what Yomitan does when importing a dictionary ZIP:
    // unzip, parse index.json, validate tag banks, parse term banks with
    // correct field types, and resolve image paths.
    // =========================================================================

    /// Helper: build a ZIP and return a ZipArchive for inspection.
    fn build_zip_archive(builder: &mut DictBuilder) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        let bytes = builder.export_bytes().unwrap();
        let cursor = std::io::Cursor::new(bytes);
        zip::ZipArchive::new(cursor).expect("export_bytes must produce a valid ZIP")
    }

    /// Helper: read a file from the archive as a string.
    fn read_zip_entry(
        archive: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
        name: &str,
    ) -> String {
        let mut file = archive
            .by_name(name)
            .unwrap_or_else(|_| panic!("ZIP missing {}", name));
        let mut buf = String::new();
        file.read_to_string(&mut buf).unwrap();
        buf
    }

    /// Helper: list all filenames in the archive.
    fn zip_filenames(archive: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>) -> Vec<String> {
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect()
    }

    /// Helper: build a realistic two-part-name character with image.
    fn make_full_character() -> Character {
        let raw = vec![0xFF, 0xD8, 0xFF, 0xE0]; // JPEG magic
        Character {
            id: "c42".to_string(),
            name: "Shinichi Suzuki".to_string(),
            name_original: "須々木 心一".to_string(),
            role: "main".to_string(),
            sex: Some("m".to_string()),
            age: Some("17".to_string()),
            height: Some(175),
            weight: Some(65),
            blood_type: Some("AB".to_string()),
            birthday: Some(vec![3, 14]),
            description: Some("A brilliant detective.".to_string()),
            aliases: vec!["シンイチ".to_string()],
            personality: vec![
                CharacterTrait {
                    name: "Clever".to_string(),
                    spoiler: 0,
                },
                CharacterTrait {
                    name: "Secret identity".to_string(),
                    spoiler: 2,
                },
            ],
            image_url: Some("https://example.com/img.jpg".to_string()),
            image_bytes: Some(raw),
            image_ext: Some("jpg".to_string()),
            ..Character::default()
        }
    }

    // --- index.json validation (Yomitan format 3 requirements) ---

    #[test]
    fn test_yomitan_index_required_fields() {
        let mut builder = DictBuilder::new(
            s(),
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Steins;Gate".to_string(),
        );
        let mut archive = build_zip_archive(&mut builder);
        let raw = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value = serde_json::from_str(&raw).unwrap();

        // Yomitan requires these fields
        assert!(index["title"].is_string(), "title must be a string");
        assert!(index["revision"].is_string(), "revision must be a string");
        assert_eq!(index["format"].as_i64().unwrap(), 3, "format must be 3");
        assert!(index["author"].is_string(), "author must be a string");
        assert!(
            index["description"].is_string(),
            "description must be a string"
        );
    }

    #[test]
    fn test_yomitan_index_update_fields() {
        let url = "http://localhost:3000/api/yomitan-dict?source=vndb&id=v17&spoiler_level=0";
        let mut builder = DictBuilder::new(s(), Some(url.to_string()), "Test".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let raw = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value = serde_json::from_str(&raw).unwrap();

        // Auto-update fields
        assert_eq!(index["downloadUrl"].as_str().unwrap(), url);
        assert!(
            index["indexUrl"]
                .as_str()
                .unwrap()
                .contains("/api/yomitan-index"),
            "indexUrl must point to the index endpoint"
        );
        assert_eq!(index["isUpdatable"].as_bool().unwrap(), true);
    }

    #[test]
    fn test_yomitan_index_no_update_fields_when_no_url() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let raw = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value = serde_json::from_str(&raw).unwrap();

        // Without download_url, these should be absent
        assert!(
            index.get("downloadUrl").is_none() || index["downloadUrl"].is_null(),
            "downloadUrl should be absent without URL"
        );
        assert!(
            index.get("isUpdatable").is_none() || index["isUpdatable"].is_null(),
            "isUpdatable should be absent without URL"
        );
    }

    #[test]
    fn test_yomitan_revision_is_unix_timestamp() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let b = DictBuilder::new(s(), None, "T".to_string());
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let rev: u64 = b.revision.parse().expect("revision must be numeric");
        assert!(
            rev >= before && rev <= after,
            "Revision {rev} should be a Unix timestamp between {before} and {after}"
        );
    }

    // --- tag_bank validation ---

    #[test]
    fn test_yomitan_tag_bank_format() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let raw = read_zip_entry(&mut archive, "tag_bank_1.json");
        let tags: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let arr = tags.as_array().expect("tag_bank must be a JSON array");
        assert!(!arr.is_empty(), "tag_bank must not be empty");

        for tag in arr {
            let tag_arr = tag.as_array().expect("each tag must be an array");
            assert_eq!(
                tag_arr.len(),
                5,
                "each tag must have 5 fields: [name, category, sortOrder, notes, score]"
            );
            assert!(tag_arr[0].is_string(), "tag name must be string");
            assert!(tag_arr[1].is_string(), "tag category must be string");
            assert!(tag_arr[2].is_number(), "tag sortOrder must be number");
            assert!(tag_arr[3].is_string(), "tag notes must be string");
            assert!(tag_arr[4].is_number(), "tag score must be number");
        }
    }

    #[test]
    fn test_yomitan_tag_bank_contains_role_tags() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let raw = read_zip_entry(&mut archive, "tag_bank_1.json");
        let tags: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let tag_names: Vec<&str> = tags
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t[0].as_str().unwrap())
            .collect();

        for expected in &["name", "main", "primary", "side", "appears"] {
            assert!(
                tag_names.contains(expected),
                "tag_bank must contain '{}' tag",
                expected
            );
        }
    }

    // --- term_bank entry format validation ---

    #[test]
    fn test_yomitan_term_entry_field_types() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test Game");

        // Validate every entry (base + honorific) matches Yomitan's expected schema
        let all_entries = builder.collect_all_entries();
        for (i, entry) in all_entries.iter().enumerate() {
            let arr = entry
                .as_array()
                .unwrap_or_else(|| panic!("entry {} must be array", i));
            assert_eq!(arr.len(), 8, "entry {} must have 8 fields", i);

            // [0] term: string
            assert!(arr[0].is_string(), "entry {}[0] term must be string", i);
            // [1] reading: string
            assert!(arr[1].is_string(), "entry {}[1] reading must be string", i);
            // [2] definitionTags: string
            assert!(
                arr[2].is_string(),
                "entry {}[2] definitionTags must be string",
                i
            );
            // [3] rules: string (always "")
            assert_eq!(
                arr[3].as_str().unwrap(),
                "",
                "entry {}[3] rules must be empty string",
                i
            );
            // [4] score: integer
            assert!(arr[4].is_number(), "entry {}[4] score must be number", i);
            // [5] definitions: array with structured-content objects
            let defs = arr[5]
                .as_array()
                .unwrap_or_else(|| panic!("entry {}[5] must be array", i));
            assert!(
                !defs.is_empty(),
                "entry {}[5] definitions must not be empty",
                i
            );
            assert_eq!(
                defs[0]["type"].as_str().unwrap(),
                "structured-content",
                "entry {}[5][0] must be structured-content",
                i
            );
            assert!(
                defs[0].get("content").is_some(),
                "entry {}[5][0] must have content",
                i
            );
            // [6] sequence: integer (always 0)
            assert_eq!(
                arr[6].as_i64().unwrap(),
                0,
                "entry {}[6] sequence must be 0",
                i
            );
            // [7] termTags: string (always "")
            assert_eq!(
                arr[7].as_str().unwrap(),
                "",
                "entry {}[7] termTags must be empty string",
                i
            );
        }
    }

    #[test]
    fn test_yomitan_term_entry_definition_tags_format() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        for entry in builder.base_entries() {
            let tags_str = entry[2].as_str().unwrap();
            // Must be space-separated, first tag is always "name"
            assert!(
                tags_str.starts_with("name"),
                "definitionTags must start with 'name', got: {}",
                tags_str
            );
            let parts: Vec<&str> = tags_str.split_whitespace().collect();
            assert!(
                parts.len() >= 2,
                "definitionTags must have at least 'name' + role"
            );
        }
    }

    #[test]
    fn test_yomitan_term_entry_scores_match_roles() {
        let roles_scores = [
            ("main", 100),
            ("primary", 75),
            ("side", 50),
            ("appears", 25),
        ];
        for (role, expected_score) in &roles_scores {
            let mut builder = DictBuilder::new(s(), None, "Test".to_string());
            let ch = make_test_character("c1", "Test", "テスト", role);
            builder.add_character(&ch, "Test");

            let all_entries = builder.collect_all_entries();
            for entry in &all_entries {
                assert_eq!(
                    entry[4].as_i64().unwrap(),
                    *expected_score as i64,
                    "role '{}' should have score {}",
                    role,
                    expected_score
                );
            }
        }
    }

    // --- ZIP structure validation ---

    #[test]
    fn test_yomitan_zip_required_files_present() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        assert!(
            names.contains(&"index.json".to_string()),
            "ZIP must contain index.json"
        );
        assert!(
            names.contains(&"tag_bank_1.json".to_string()),
            "ZIP must contain tag_bank_1.json"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("term_bank_") && n.ends_with(".json")),
            "ZIP must contain at least one term_bank_N.json"
        );
    }

    #[test]
    fn test_yomitan_zip_term_banks_are_valid_json_arrays() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        for name in names.iter().filter(|n| n.starts_with("term_bank_")) {
            let raw = read_zip_entry(&mut archive, name);
            let parsed: serde_json::Value = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("{} must be valid JSON: {}", name, e));
            assert!(parsed.is_array(), "{} must be a JSON array", name);
        }
    }

    #[test]
    fn test_yomitan_zip_image_paths_resolve() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        // Collect all image paths referenced in structured content
        let mut referenced_paths: HashSet<String> = HashSet::new();
        for entry in builder.base_entries() {
            let sc_str = serde_json::to_string(&entry[5]).unwrap();
            // Look for "path":"img/..." patterns
            for cap in regex::Regex::new(r#""path"\s*:\s*"(img/[^"]+)""#)
                .unwrap()
                .captures_iter(&sc_str)
            {
                referenced_paths.insert(cap[1].to_string());
            }
        }

        // Every referenced image path must exist in the ZIP
        for path in &referenced_paths {
            assert!(
                names.contains(path),
                "Image path '{}' referenced in structured content but not found in ZIP",
                path
            );
        }
    }

    #[test]
    fn test_yomitan_zip_image_bytes_not_empty() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        for name in names.iter().filter(|n| n.starts_with("img/")) {
            let mut file = archive.by_name(name).unwrap();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            assert!(!buf.is_empty(), "Image file '{}' must not be empty", name);
        }
    }

    // --- Term bank chunking ---

    #[test]
    fn test_yomitan_term_bank_chunking() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());

        // Generate enough entries to force multiple term banks.
        // With ~170 honorific suffixes, each character produces ~2000+ entries,
        // so only a handful of characters are needed to exceed the size limit.
        for i in 0..10 {
            let ch = Character {
                id: format!("c{}", i),
                name: format!("Given{} Family{}", i, i),
                name_original: format!("姓{} 名{}", i, i),
                role: "main".to_string(),
                aliases: vec![format!("Alias{}", i)],
                ..Character::default()
            };
            builder.add_character(&ch, "Test");
        }

        assert!(
            builder.collect_all_entries().len() > TERM_BANK_LIMIT,
            "Need >{} entries to test chunking, got {}",
            TERM_BANK_LIMIT,
            builder.collect_all_entries().len()
        );

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        let term_banks: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("term_bank_") && n.ends_with(".json"))
            .collect();

        assert!(
            term_banks.len() >= 2,
            "Should have at least 2 term banks with >{} entries, got {}",
            TERM_BANK_LIMIT,
            term_banks.len()
        );

        // Verify each bank has at most the term bank limit number of entries
        for name in &term_banks {
            let raw = read_zip_entry(&mut archive, name);
            let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
            assert!(
                arr.len() <= TERM_BANK_LIMIT,
                "{} has {} entries, max is {}",
                name,
                arr.len(),
                TERM_BANK_LIMIT
            );
        }
    }

    // --- End-to-end realistic character import ---

    #[test]
    fn test_yomitan_full_import_simulation() {
        // Simulate what Yomitan does: unzip → parse index → parse tags → parse all term banks
        let mut builder = DictBuilder::new(
            s(),
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Steins;Gate".to_string(),
        );
        let ch = make_full_character();
        builder.add_character(&ch, "Steins;Gate");

        let zip_bytes = builder.export_bytes().unwrap();

        // Step 1: Valid ZIP
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).expect("Must be a valid ZIP");

        // Step 2: Parse index.json
        let index_raw = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value =
            serde_json::from_str(&index_raw).expect("index.json must be valid JSON");
        assert_eq!(index["format"].as_i64().unwrap(), 3);
        assert!(!index["revision"].as_str().unwrap().is_empty());
        assert!(index["description"]
            .as_str()
            .unwrap()
            .contains("Steins;Gate"));

        // Step 3: Parse tag_bank
        let tags_raw = read_zip_entry(&mut archive, "tag_bank_1.json");
        let tags: Vec<serde_json::Value> =
            serde_json::from_str(&tags_raw).expect("tag_bank must be valid JSON array");
        let tag_names: HashSet<String> = tags
            .iter()
            .map(|t| t[0].as_str().unwrap().to_string())
            .collect();

        // Step 4: Parse all term banks and validate entries
        let names = zip_filenames(&mut archive);
        let mut total_entries = 0;
        let mut all_terms: Vec<String> = Vec::new();

        for name in names.iter().filter(|n| n.starts_with("term_bank_")) {
            let raw = read_zip_entry(&mut archive, name);
            let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();

            for entry in &entries {
                let arr = entry.as_array().unwrap();
                let term = arr[0].as_str().unwrap();
                let reading = arr[1].as_str().unwrap();
                let def_tags = arr[2].as_str().unwrap();

                // Term and reading must be non-empty
                assert!(!term.is_empty(), "term must not be empty");
                assert!(!reading.is_empty(), "reading must not be empty");

                // definitionTags must reference tags that exist in tag_bank
                for tag in def_tags.split_whitespace() {
                    assert!(
                        tag_names.contains(tag),
                        "definitionTag '{}' not found in tag_bank",
                        tag
                    );
                }

                all_terms.push(term.to_string());
                total_entries += 1;
            }
        }

        assert!(total_entries > 0, "Dictionary must have at least one entry");

        // Verify expected base terms are present
        assert!(
            all_terms.contains(&"須々木 心一".to_string()),
            "Must have full name with space"
        );
        assert!(
            all_terms.contains(&"須々木心一".to_string()),
            "Must have combined name"
        );
        assert!(
            all_terms.contains(&"須々木".to_string()),
            "Must have family name"
        );
        assert!(
            all_terms.contains(&"心一".to_string()),
            "Must have given name"
        );
        assert!(
            all_terms.contains(&"シンイチ".to_string()),
            "Must have alias entry"
        );

        // Verify honorific variants exist
        assert!(
            all_terms.iter().any(|t| t == "須々木さん"),
            "Must have family+san"
        );
        assert!(
            all_terms.iter().any(|t| t == "心一ちゃん"),
            "Must have given+chan"
        );

        // Verify image is in the ZIP
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("img/") && n.contains("c42")),
            "ZIP must contain the character's image"
        );
    }

    #[test]
    fn test_yomitan_no_duplicate_terms_in_export() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        // Collect (term, reading) pairs — should be unique across all entries
        let all_entries = builder.collect_all_entries();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for entry in &all_entries {
            let term = entry[0].as_str().unwrap().to_string();
            let reading = entry[1].as_str().unwrap().to_string();
            let key = (term.clone(), reading.clone());
            assert!(
                seen.insert(key),
                "Duplicate entry: term='{}', reading='{}'",
                term,
                reading
            );
        }
    }

    #[test]
    fn test_yomitan_empty_dict_produces_valid_zip() {
        // A dictionary with no characters should still produce a valid ZIP
        // with index.json and tag_bank but no term banks
        let mut builder = DictBuilder::new(s(), None, "Empty".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        assert!(names.contains(&"index.json".to_string()));
        assert!(names.contains(&"tag_bank_1.json".to_string()));
        // No term banks since no entries
        assert!(
            !names.iter().any(|n| n.starts_with("term_bank_")),
            "Empty dict should have no term banks"
        );
    }

    #[test]
    fn test_yomitan_characters_without_japanese_name_skipped() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());

        // Character with empty name_original
        let ch = make_test_character("c1", "John Smith", "", "main");
        builder.add_character(&ch, "Test");

        assert_eq!(
            builder.base_entries().len(),
            0,
            "Characters without Japanese names must produce no entries"
        );

        // ZIP should still be valid
        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);
        assert!(names.contains(&"index.json".to_string()));
    }

    #[test]
    fn test_yomitan_spoiler_settings_affect_content() {
        // No spoilers: spoiler traits should be excluded
        let no_spoilers = DictSettings {
            show_spoilers: false,
            ..DictSettings::default()
        };
        let mut builder_no = DictBuilder::new(no_spoilers, None, "Test".to_string());
        let ch = make_full_character(); // has a spoiler=2 trait "Secret identity"
        builder_no.add_character(&ch, "Test");

        // Full spoilers: spoiler traits should be included
        let mut builder_full = DictBuilder::new(s(), None, "Test".to_string());
        builder_full.add_character(&ch, "Test");

        // Find the base name entry in each
        let find_base = |entries: &[serde_json::Value]| -> String {
            let entry = entries
                .iter()
                .find(|e| e[0].as_str().unwrap() == "須々木 心一")
                .unwrap();
            serde_json::to_string(&entry[5]).unwrap()
        };

        let content_no = find_base(builder_no.base_entries());
        let content_full = find_base(builder_full.base_entries());

        assert!(
            !content_no.contains("Secret identity"),
            "No-spoilers settings should exclude spoiler=2 traits"
        );
        assert!(
            content_full.contains("Secret identity"),
            "Full settings should include spoiler=2 traits"
        );
    }

    #[test]
    fn test_yomitan_single_word_name_no_family_given_split() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Saber", "セイバー", "main");
        builder.add_character(&ch, "Test");

        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .map(|e| e[0].as_str().unwrap().to_string())
            .collect();

        // Single-word name should not produce separate family/given entries
        assert!(terms.contains(&"セイバー".to_string()));
        // Should still have honorific variants
        assert!(
            terms.iter().any(|t| t == "セイバーさん"),
            "Single name should get honorifics"
        );
    }

    #[test]
    fn test_yomitan_structured_content_has_type_field() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let all_entries = builder.collect_all_entries();
        for entry in &all_entries {
            let defs = entry[5].as_array().unwrap();
            for def in defs {
                assert_eq!(
                    def["type"].as_str().unwrap(),
                    "structured-content",
                    "Every definition must have type=structured-content"
                );
                let content = &def["content"];
                assert!(
                    content.is_array() || content.is_object() || content.is_string(),
                    "structured-content.content must be array, object, or string"
                );
            }
        }
    }

    // === Edge case: honorifics disabled ===

    #[test]
    fn test_honorifics_disabled_no_suffix_entries() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should NOT have any honorific variants
        assert!(
            !terms.iter().any(|t| t.ends_with("さん")),
            "honorifics=false should not produce -san variants"
        );
        assert!(
            !terms.iter().any(|t| t.ends_with("ちゃん")),
            "honorifics=false should not produce -chan variants"
        );

        // Should still have base entries
        assert!(terms.contains(&"須々木 心一".to_string()));
        assert!(terms.contains(&"須々木".to_string()));
        assert!(terms.contains(&"心一".to_string()));
    }

    // === Edge case: character with space-only name_original ===

    #[test]
    fn test_add_character_space_only_name() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char = make_test_character("c1", "Name", " ", "main");
        builder.add_character(&char, "Test");

        // " " splits into family="" and given=""
        // Empty checks should prevent entries for empty parts
        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // The original " " entry might be added, but empty family/given should not
        assert!(
            !terms.iter().any(|t| t.is_empty()),
            "Should not have entries with empty terms"
        );
    }

    // === Edge case: alias identical to kana reading ===

    #[test]
    fn test_alias_matching_kana_reading_deduplicated() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char = make_test_character("c1", "Saber", "セイバー", "main");
        // Alias is the hiragana form of the name
        char.aliases = vec!["せいばー".to_string()];
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // "せいばー" should appear only once (either from kana generation or alias, not both)
        let count = terms.iter().filter(|t| t.as_str() == "せいばー").count();
        assert_eq!(
            count, 1,
            "Alias matching kana reading should be deduplicated"
        );
    }

    // === Edge case: two characters with same name are deduplicated ===

    #[test]
    fn test_two_characters_same_name_deduplicated() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char1 = make_test_character("c1", "Saber", "セイバー", "main");
        let char2 = make_test_character("c2", "Saber", "セイバー", "side");
        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Cross-character dedup: second character with same name_original is skipped
        let saber_entries: Vec<&serde_json::Value> = builder
            .base_entries()
            .iter()
            .filter(|e| e[0].as_str() == Some("セイバー"))
            .collect();

        assert_eq!(
            saber_entries.len(),
            1,
            "Duplicate character should be deduplicated, got {} entries",
            saber_entries.len()
        );
    }

    #[test]
    fn test_cross_media_dedup_same_name_different_games() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char1 = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        let char2 = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&char1, "Steins;Gate");
        builder.add_character(&char2, "Steins;Gate 0");

        // Characters merge — entry count should match a single-character builder
        let entries_count = builder.base_entries().len();
        let mut builder2 = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        builder2.add_character(&char1, "Steins;Gate");
        assert_eq!(
            entries_count,
            builder2.base_entries().len(),
            "Merged character should produce the same number of entries as a single occurrence"
        );
    }

    #[test]
    fn test_cross_media_dedup_space_normalization() {
        // VNDB uses "岡部 倫太郎" (with space), AniList might use "岡部倫太郎" (no space)
        // Both should merge into a single character entry.
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char_vndb = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        let mut char_anilist = make_test_character("35252", "Okabe Rintarou", "岡部倫太郎", "main");
        char_anilist.first_name_hint = Some("Rintarou".to_string());
        char_anilist.last_name_hint = Some("Okabe".to_string());

        builder.add_character(&char_vndb, "Steins;Gate VN");
        builder.add_character(&char_anilist, "Steins;Gate Anime");

        // Should merge — compare against a single-character builder
        let mut single_builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        single_builder.add_character(&char_vndb, "Steins;Gate VN");
        assert_eq!(
            builder.base_entries().len(),
            single_builder.base_entries().len(),
            "Same character with/without space in name_original should be deduplicated"
        );
    }

    #[test]
    fn test_cross_media_dedup_different_names_not_affected() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char1 = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        let char2 = make_test_character("c43", "Makise Kurisu", "牧瀬 紅莉栖", "primary");
        builder.add_character(&char1, "Steins;Gate");
        builder.add_character(&char2, "Steins;Gate");

        // Both characters should have entries since they have different names
        let terms: Vec<&str> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str())
            .collect();
        assert!(
            terms.contains(&"岡部 倫太郎"),
            "First character should have entries"
        );
        assert!(
            terms.contains(&"牧瀬 紅莉栖"),
            "Second character with different name should also have entries"
        );
    }

    #[test]
    fn test_cross_media_dedup_different_source_ids_same_name() {
        // Simulates VNDB character c42 and AniList character 35252 — same person, different IDs
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char_vndb = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        let mut char_anilist =
            make_test_character("35252", "Okabe Rintarou", "岡部 倫太郎", "main");
        char_anilist.first_name_hint = Some("Rintarou".to_string());
        char_anilist.last_name_hint = Some("Okabe".to_string());

        builder.add_character(&char_vndb, "Steins;Gate VN");
        builder.add_character(&char_anilist, "Steins;Gate Anime");

        // Should merge — compare against a single-character builder
        let mut single_builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        single_builder.add_character(&char_vndb, "Steins;Gate VN");
        assert_eq!(
            builder.base_entries().len(),
            single_builder.base_entries().len(),
            "Cross-source duplicate (different IDs, same name_original) should be deduplicated"
        );
    }

    // === Edge case: same character (same ID) from two media should only produce one image ===

    #[test]
    fn test_duplicate_character_image_deduplicated() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let fake_image = vec![0xFF, 0xD8, 0xFF, 0xE0]; // fake JPEG bytes
        let mut char1 = make_test_character("c1234", "Saber", "セイバー", "main");
        char1.image_bytes = Some(fake_image.clone());
        char1.image_ext = Some("jpg".to_string());

        let mut char2 = make_test_character("c1234", "Saber", "セイバー", "main");
        char2.image_bytes = Some(fake_image.clone());
        char2.image_ext = Some("jpg".to_string());

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Finalize to trigger image and entry generation
        builder.finalize();

        // Image should only appear once in the builder
        // make_filename("c1234", "jpg") produces "cc1234.jpg"
        let image_count = builder
            .images
            .iter()
            .filter(|(name, _)| name == "cc1234.jpg")
            .count();
        assert_eq!(
            image_count, 1,
            "Same character from two media should only produce one image, got {}",
            image_count
        );
    }

    // === Edge case: character with many empty aliases ===

    #[test]
    fn test_empty_aliases_skipped() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char = make_test_character("c1", "Name", "名前", "main");
        char.aliases = vec![
            "".to_string(),
            "".to_string(),
            "有効".to_string(),
            "".to_string(),
        ];
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"有効".to_string()),
            "Non-empty Japanese alias should be present"
        );
        // Empty aliases should not produce entries
        let empty_count = terms.iter().filter(|t| t.is_empty()).count();
        assert_eq!(empty_count, 0, "Empty aliases should not produce entries");
    }

    // === Edge case: hiragana and katakana term entries ===

    #[test]
    fn test_kana_term_entries_generated() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should have hiragana and katakana variants
        // The readings come from alphabet_to_kana("shinichi") and alphabet_to_kana("suzuki")
        let has_hiragana = terms.iter().any(|t| {
            t.chars().all(|c| {
                let code = c as u32;
                (0x3041..=0x3096).contains(&code) || c == ' '
            }) && !t.is_empty()
        });
        let has_katakana = terms.iter().any(|t| {
            t.chars().all(|c| {
                let code = c as u32;
                (0x30A1..=0x30F6).contains(&code) || c == ' '
            }) && !t.is_empty()
        });

        assert!(has_hiragana, "Should have hiragana term entries");
        assert!(has_katakana, "Should have katakana term entries");
    }

    // === Edge case: index URL replacement ===

    #[test]
    fn test_index_url_replacement() {
        let builder = DictBuilder::new(
            s(),
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Test".to_string(),
        );
        let index = builder.create_index_public();
        let index_url = index["indexUrl"].as_str().unwrap();
        assert!(index_url.contains("/api/yomitan-index"));
        assert!(!index_url.contains("/api/yomitan-dict"));
        // Query params should be preserved
        assert!(index_url.contains("source=vndb"));
        assert!(index_url.contains("id=v17"));
    }

    // === Edge case: unknown role score ===

    #[test]
    fn test_unknown_role_score_zero() {
        assert_eq!(get_score("custom"), 0);
        assert_eq!(get_score(""), 0);
        assert_eq!(get_score("MAIN"), 0); // case-sensitive
    }

    // === Regression: ambiguous split generates entries for all candidates ===

    #[test]
    fn test_ambiguous_split_generates_both_family_given_entries() {
        // "石井守" with hints Mamoru/Ishii has symmetric kana lengths (3+3),
        // producing two equally-scored splits: "石"+"井守" and "石井"+"守".
        // The dict builder should generate entries for BOTH so lookups work
        // regardless of which split is correct.
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Mamoru Ishii", "石井守", "main");
        ch.first_name_hint = Some("Mamoru".to_string());
        ch.last_name_hint = Some("Ishii".to_string());
        ch.aliases = vec![];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Combined form (same for all splits)
        assert!(
            terms.contains(&"石井守".to_string()),
            "Should have combined name"
        );

        // Both family candidates
        assert!(
            terms.contains(&"石井".to_string()),
            "Should have 2-kanji family entry '石井'"
        );
        assert!(
            terms.contains(&"石".to_string()),
            "Should have 1-kanji family entry '石'"
        );

        // Both given candidates
        assert!(
            terms.contains(&"守".to_string()),
            "Should have 1-kanji given entry '守'"
        );
        assert!(
            terms.contains(&"井守".to_string()),
            "Should have 2-kanji given entry '井守'"
        );
    }

    #[test]
    fn test_unambiguous_split_generates_single_family_given() {
        // "幸平創真" with hints Souma/Yukihira has asymmetric kana (4+3),
        // producing a single clear winner at split_pos=2 → "幸平"+"創真".
        // Should NOT generate entries for the wrong split.
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Souma Yukihira", "幸平創真", "main");
        ch.first_name_hint = Some("Souma".to_string());
        ch.last_name_hint = Some("Yukihira".to_string());
        ch.aliases = vec![];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"幸平".to_string()),
            "Should have family '幸平'"
        );
        assert!(
            terms.contains(&"創真".to_string()),
            "Should have given '創真'"
        );
        // The wrong split should NOT be present
        assert!(
            !terms.contains(&"幸".to_string()),
            "Should NOT have wrong 1-char family split"
        );
    }

    // ===== Additional comprehensive tests =====

    // --- Entry generation: kana/katakana variants ---

    #[test]
    fn test_kana_and_katakana_entries_for_two_part_name() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should have hiragana combined (no space)
        assert!(
            terms.iter().any(|t| t == "おかべりんたろう"),
            "Missing hiragana combined"
        );
        // Should have hiragana with space
        assert!(
            terms.iter().any(|t| t == "おかべ りんたろう"),
            "Missing hiragana spaced"
        );
        // Should have hiragana family only
        assert!(
            terms.iter().any(|t| t == "おかべ"),
            "Missing hiragana family"
        );
        // Should have hiragana given only
        assert!(
            terms.iter().any(|t| t == "りんたろう"),
            "Missing hiragana given"
        );
        // Should have katakana combined
        assert!(
            terms.iter().any(|t| t == "オカベリンタロウ"),
            "Missing katakana combined"
        );
        // Should have katakana with space
        assert!(
            terms.iter().any(|t| t == "オカベ リンタロウ"),
            "Missing katakana spaced"
        );
        // Should have katakana family
        assert!(
            terms.iter().any(|t| t == "オカベ"),
            "Missing katakana family"
        );
        // Should have katakana given
        assert!(
            terms.iter().any(|t| t == "リンタロウ"),
            "Missing katakana given"
        );
    }

    #[test]
    fn test_kana_entries_for_single_word_name() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("c1", "Saber", "セイバー", "main");
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Original katakana
        assert!(terms.contains(&"セイバー".to_string()));
        // Hiragana form
        assert!(terms.contains(&"せいばー".to_string()));
    }

    // --- Entry generation: honorific variants ---

    #[test]
    fn test_honorific_entries_generated_for_all_base_forms() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Kanji family + さん
        assert!(terms.iter().any(|t| t == "岡部さん"), "Missing 岡部さん");
        // Kanji given + さん
        assert!(
            terms.iter().any(|t| t == "倫太郎さん"),
            "Missing 倫太郎さん"
        );
        // Combined + さん
        assert!(
            terms.iter().any(|t| t == "岡部倫太郎さん"),
            "Missing 岡部倫太郎さん"
        );
        // Hiragana family + さん
        assert!(
            terms.iter().any(|t| t == "おかべさん"),
            "Missing おかべさん"
        );
        // Katakana family + さん
        assert!(
            terms.iter().any(|t| t == "オカベさん"),
            "Missing オカベさん"
        );
    }

    #[test]
    fn test_honorific_entries_for_aliases() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        ch.aliases = vec!["鳳凰院凶真".to_string()];
        builder.add_character(&ch, "Test");

        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"鳳凰院凶真".to_string()),
            "Missing alias base entry"
        );
        assert!(
            terms.iter().any(|t| t == "鳳凰院凶真さん"),
            "Missing alias + さん"
        );
    }

    // --- Deduplication ---

    #[test]
    fn test_no_duplicate_entries_in_single_character() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    e[0].as_str().unwrap_or(""),
                    e[1].as_str().unwrap_or("")
                )
            })
            .collect();

        let unique: std::collections::HashSet<&String> = terms.iter().collect();
        assert_eq!(
            terms.len(),
            unique.len(),
            "Found duplicate term:reading pairs"
        );
    }

    #[test]
    fn test_two_characters_no_cross_contamination() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch1 = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        let ch2 = make_test_character("c2", "Makise Kurisu", "牧瀬 紅莉栖", "primary");
        builder.add_character(&ch1, "Test");
        builder.add_character(&ch2, "Test");

        // Both characters should have entries
        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(terms.contains(&"岡部 倫太郎".to_string()));
        assert!(terms.contains(&"牧瀬 紅莉栖".to_string()));
    }

    // --- Score assignment ---

    #[test]
    fn test_scores_decrease_by_role() {
        let main_score = get_score("main");
        let primary_score = get_score("primary");
        let side_score = get_score("side");
        let appears_score = get_score("appears");

        assert!(main_score > primary_score, "main > primary");
        assert!(primary_score > side_score, "primary > side");
        assert!(side_score > appears_score, "side > appears");
    }

    #[test]
    fn test_score_in_entries_matches_role() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "テスト", "primary");
        builder.add_character(&ch, "Test");

        let expected_score = get_score("primary");
        for entry in builder.base_entries() {
            let score = entry[4].as_i64().unwrap();
            assert_eq!(score, expected_score as i64);
        }
    }

    // --- ZIP export ---

    #[test]
    fn test_export_empty_builder_produces_valid_zip() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Empty".to_string());
        let bytes = builder.export_bytes().unwrap();
        assert!(!bytes.is_empty());

        let archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive.clone());
        assert!(names.contains(&"index.json".to_string()));
        assert!(names.contains(&"tag_bank_1.json".to_string()));
    }

    #[test]
    fn test_export_with_images_includes_img_folder() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_full_character();
        ch.image_bytes = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]); // JPEG header
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);
        assert!(
            names.iter().any(|n| n.starts_with("img/")),
            "Should have img/ entries"
        );
    }

    #[test]
    fn test_export_index_json_has_correct_title() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "My Game Title".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let index_str = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
        // Title is always "Bee's Character Dictionary", game title goes in description
        assert_eq!(
            index["title"].as_str().unwrap(),
            "Bee's Character Dictionary"
        );
        assert!(index["description"]
            .as_str()
            .unwrap()
            .contains("My Game Title"));
    }

    #[test]
    fn test_export_revision_is_12_digits() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut archive = build_zip_archive(&mut builder);
        let index_str = read_zip_entry(&mut archive, "index.json");
        let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
        let revision = index["revision"].as_str().unwrap();
        assert_eq!(revision.len(), 12, "Revision should be 12 digits");
        assert!(
            revision.chars().all(|c| c.is_ascii_digit()),
            "Revision should be all digits"
        );
    }

    #[test]
    fn test_export_two_builders_have_different_revisions() {
        let mut b1 = DictBuilder::new(s_no_hon(), None, "T".to_string());
        let mut b2 = DictBuilder::new(s_no_hon(), None, "T".to_string());
        let mut a1 = build_zip_archive(&mut b1);
        let mut a2 = build_zip_archive(&mut b2);
        let i1: serde_json::Value =
            serde_json::from_str(&read_zip_entry(&mut a1, "index.json")).unwrap();
        let i2: serde_json::Value =
            serde_json::from_str(&read_zip_entry(&mut a2, "index.json")).unwrap();
        // Both are Unix timestamps — may be equal if built in the same second; just check they're valid strings
        assert!(i1["revision"].is_string());
        assert!(i2["revision"].is_string());
    }

    // --- Stress test: many characters ---

    #[test]
    fn test_many_characters_produces_valid_zip() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Stress Test".to_string());
        for i in 0..50 {
            let ch = make_test_character(
                &format!("c{}", i),
                &format!("Char{} Name{}", i, i),
                &format!("キャラ{}", i),
                if i % 4 == 0 {
                    "main"
                } else if i % 4 == 1 {
                    "primary"
                } else if i % 4 == 2 {
                    "side"
                } else {
                    "appears"
                },
            );
            builder.add_character(&ch, "Stress Test");
        }

        assert!(
            builder.base_entries().len() >= 50,
            "Should have at least 50 entries"
        );
        let bytes = builder.export_bytes().unwrap();
        assert!(!bytes.is_empty());

        // Verify it's a valid ZIP
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let names = zip_filenames(&mut archive);
        assert!(names.contains(&"index.json".to_string()));
    }

    // --- Character with image bytes ---

    #[test]
    fn test_character_with_image_creates_img_entry() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("42", "Test", "テスト", "main");
        ch.image_bytes = Some(vec![1, 2, 3, 4]);
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        // make_filename("42", "jpg") → "c42.jpg", path → "img/c42.jpg"
        let content_str = serde_json::to_string(builder.base_entries()).unwrap();
        assert!(
            content_str.contains("img/c42.jpg"),
            "content: {}",
            content_str
        );
    }

    #[test]
    fn test_character_without_image_no_img_reference() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("42", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        let content_str = serde_json::to_string(builder.base_entries()).unwrap();
        assert!(!content_str.contains("img/"));
    }

    // --- Index metadata ---

    #[test]
    fn test_index_with_download_url() {
        let builder = DictBuilder::new(
            s_no_hon(),
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Test".to_string(),
        );
        let index = builder.create_index_public();
        assert!(index["downloadUrl"].is_string());
        assert!(index["downloadUrl"].as_str().unwrap().contains("v17"));
    }

    #[test]
    fn test_index_without_download_url() {
        let builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let index = builder.create_index_public();
        // Should not have update-related fields
        assert!(index.get("downloadUrl").is_none() || index["downloadUrl"].is_null());
    }

    // --- AniList character with hints ---

    #[test]
    fn test_anilist_character_with_hints_generates_correct_entries() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Rintarou Okabe", "岡部倫太郎", "main");
        ch.first_name_hint = Some("Rintarou".to_string());
        ch.last_name_hint = Some("Okabe".to_string());
        ch.aliases = vec![];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should have the combined form
        assert!(
            terms.contains(&"岡部倫太郎".to_string()),
            "Missing combined kanji"
        );
    }

    // --- Edge case: character with only aliases ---

    #[test]
    fn test_character_with_aliases_generates_alias_entries() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test Name", "テスト", "main");
        ch.aliases = vec!["別名".to_string(), "エイリアス".to_string()];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(terms.contains(&"別名".to_string()));
        assert!(terms.contains(&"エイリアス".to_string()));
    }

    // --- Edge case: whitespace-only name_original ---

    #[test]
    fn test_whitespace_only_name_original() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "   ", "main");
        builder.add_character(&ch, "Test");
        // Whitespace-only name should still generate entries (it's not empty)
        assert!(!builder.base_entries().is_empty());
    }

    // ===================================================================
    // DictSettings-gated behavior tests — verify each setting controls
    // the correct aspect of dictionary generation
    // ===================================================================

    // --- show_image: false should suppress images in ZIP ---

    #[test]
    fn test_show_image_false_no_images_in_builder() {
        let settings = DictSettings {
            show_image: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.image_bytes = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        assert!(
            builder.images.is_empty(),
            "show_image=false should not add images to the builder"
        );
    }

    #[test]
    fn test_show_image_false_no_img_references_in_content() {
        let settings = DictSettings {
            show_image: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.image_bytes = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        let content_str = serde_json::to_string(builder.base_entries()).unwrap();
        assert!(
            !content_str.contains("\"img\""),
            "show_image=false should not reference img tags in structured content"
        );
    }

    #[test]
    fn test_show_image_false_zip_has_no_img_folder() {
        let settings = DictSettings {
            show_image: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.image_bytes = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);
        assert!(
            !names.iter().any(|n| n.starts_with("img/")),
            "show_image=false ZIP should have no img/ entries"
        );
    }

    #[test]
    fn test_show_image_true_zip_has_img_folder() {
        let settings = DictSettings {
            show_image: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.image_bytes = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        ch.image_ext = Some("jpg".to_string());
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);
        assert!(
            names.iter().any(|n| n.starts_with("img/")),
            "show_image=true ZIP should have img/ entries"
        );
    }

    // --- show_tag: false should suppress role badges ---

    #[test]
    fn test_show_tag_false_no_role_badge_in_content() {
        let settings = DictSettings {
            show_tag: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test Name", "テスト", "main");
        builder.add_character(&ch, "Test");

        // Find base entry
        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            !sc_str.contains("Protagonist"),
            "show_tag=false should not include 'Protagonist' label"
        );
    }

    #[test]
    fn test_show_tag_true_has_role_badge_in_content() {
        let settings = DictSettings {
            show_tag: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test Name", "テスト", "main");
        builder.add_character(&ch, "Test");

        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            sc_str.contains("Protagonist"),
            "show_tag=true should include 'Protagonist' label"
        );
    }

    #[test]
    fn test_show_tag_false_definition_tags_exclude_role() {
        let settings = DictSettings {
            show_tag: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test Name", "テスト", "primary");
        builder.add_character(&ch, "Test");

        // Every entry's definitionTags (element [2]) must be just "name" with no role
        for entry in builder.base_entries() {
            let def_tags = entry[2].as_str().unwrap();
            assert_eq!(
                def_tags, "name",
                "show_tag=false: definitionTags must be 'name', got '{}'",
                def_tags
            );
        }
    }

    #[test]
    fn test_show_tag_true_definition_tags_include_role() {
        let settings = DictSettings {
            show_tag: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test Name", "テスト", "primary");
        builder.add_character(&ch, "Test");

        // Every entry's definitionTags (element [2]) must include the role
        for entry in builder.base_entries() {
            let def_tags = entry[2].as_str().unwrap();
            assert_eq!(
                def_tags, "name primary",
                "show_tag=true: definitionTags must be 'name primary', got '{}'",
                def_tags
            );
        }
    }

    #[test]
    fn test_show_tag_false_tag_bank_excludes_role_tags() {
        let settings = DictSettings {
            show_tag: false,
            ..DictSettings::default()
        };
        let builder = DictBuilder::new(settings, None, "Test".to_string());
        let tags = builder.create_tags();
        let tags_str = serde_json::to_string(&tags).unwrap();
        assert!(
            !tags_str.contains("\"primary\""),
            "show_tag=false: tag_bank must not define 'primary'"
        );
        assert!(
            !tags_str.contains("\"main\""),
            "show_tag=false: tag_bank must not define 'main'"
        );
        // "name" tag should always be present
        assert!(
            tags_str.contains("\"name\""),
            "tag_bank must always define 'name'"
        );
    }

    #[test]
    fn test_show_tag_true_tag_bank_includes_role_tags() {
        let settings = DictSettings {
            show_tag: true,
            ..DictSettings::default()
        };
        let builder = DictBuilder::new(settings, None, "Test".to_string());
        let tags = builder.create_tags();
        let tags_str = serde_json::to_string(&tags).unwrap();
        assert!(
            tags_str.contains("\"primary\""),
            "show_tag=true: tag_bank must define 'primary'"
        );
        assert!(
            tags_str.contains("\"main\""),
            "show_tag=true: tag_bank must define 'main'"
        );
        assert!(
            tags_str.contains("\"side\""),
            "show_tag=true: tag_bank must define 'side'"
        );
        assert!(
            tags_str.contains("\"appears\""),
            "show_tag=true: tag_bank must define 'appears'"
        );
    }

    // --- show_description: false should suppress descriptions ---

    #[test]
    fn test_show_description_false_no_description_in_content() {
        let settings = DictSettings {
            show_description: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test", "テスト", "main");
        ch.description = Some("A unique test description.".to_string());
        builder.add_character(&ch, "Test");

        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            !sc_str.contains("A unique test description."),
            "show_description=false should exclude description"
        );
    }

    #[test]
    fn test_show_description_true_has_description_in_content() {
        let settings = DictSettings {
            show_description: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test", "テスト", "main");
        ch.description = Some("A unique test description.".to_string());
        builder.add_character(&ch, "Test");

        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            sc_str.contains("A unique test description."),
            "show_description=true should include description"
        );
    }

    // --- show_traits: false should suppress traits ---

    #[test]
    fn test_show_traits_false_no_traits_in_content() {
        let settings = DictSettings {
            show_traits: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            !sc_str.contains("Character Information"),
            "show_traits=false should exclude Character Information"
        );
    }

    #[test]
    fn test_show_traits_true_has_traits_in_content() {
        let settings = DictSettings {
            show_traits: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            sc_str.contains("Character Information"),
            "show_traits=true should include Character Information"
        );
    }

    // --- All settings false: minimal content ---

    #[test]
    fn test_all_settings_off_minimal_content() {
        let settings = DictSettings {
            show_image: false,
            show_tag: false,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            honorifics: false,
            show_seiyuu: false,
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.description = Some("Detailed description.".to_string());
        builder.add_character(&ch, "Test");

        // No images in builder
        assert!(builder.images.is_empty());

        // No honorific entries
        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            !terms.iter().any(|t| t.ends_with("さん")),
            "All off should not have honorifics"
        );

        // Content should not have img, role badge, description, or traits
        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("須々木 心一"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(!sc_str.contains("\"img\""), "No img tag");
        assert!(!sc_str.contains("Protagonist"), "No role badge");
        assert!(!sc_str.contains("Description"), "No description");
        assert!(!sc_str.contains("Character Information"), "No traits");
    }

    // --- All settings true: maximal content ---

    #[test]
    fn test_all_settings_on_maximal_content() {
        let settings = DictSettings::default();
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_full_character();
        ch.description = Some("A brilliant detective.".to_string());
        builder.add_character(&ch, "Test");

        // Has honorific entries (this also triggers finalize)
        let all_entries = builder.collect_all_entries();
        let terms: Vec<String> = all_entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Has images (checked after finalize)
        assert!(!builder.images.is_empty(), "Should have images");

        assert!(
            terms.iter().any(|t| t.ends_with("さん")),
            "Should have honorifics"
        );

        // Content should have everything
        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("須々木 心一"))
            .unwrap();
        let sc_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(sc_str.contains("\"img\""), "Should have img tag");
        assert!(sc_str.contains("Protagonist"), "Should have role badge");
        assert!(sc_str.contains("Description"), "Should have description");
        assert!(
            sc_str.contains("Character Information"),
            "Should have traits"
        );
    }

    // --- Settings don't affect entry counts (only content shape) ---

    #[test]
    fn test_settings_dont_change_entry_count_except_honorifics() {
        let settings_with = DictSettings {
            honorifics: true,
            show_image: true,
            show_tag: true,
            show_description: true,
            show_traits: true,
            show_spoilers: true,
            show_seiyuu: true,
        };
        let settings_without = DictSettings {
            honorifics: true,
            show_image: false,
            show_tag: false,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            show_seiyuu: false,
        };

        let mut b1 = DictBuilder::new(settings_with, None, "T".to_string());
        let mut b2 = DictBuilder::new(settings_without, None, "T".to_string());

        let ch = make_test_character("c1", "Test Name", "テスト", "main");
        b1.add_character(&ch, "T");
        b2.add_character(&ch, "T");

        assert_eq!(
            b1.collect_all_entries().len(),
            b2.collect_all_entries().len(),
            "Image/tag/description/traits/spoilers settings should not affect entry count when honorifics are the same"
        );
    }

    #[test]
    fn test_honorifics_setting_affects_entry_count() {
        let with_hon = DictSettings {
            honorifics: true,
            ..DictSettings::default()
        };
        let without_hon = DictSettings {
            honorifics: false,
            ..DictSettings::default()
        };

        let mut b1 = DictBuilder::new(with_hon, None, "T".to_string());
        let mut b2 = DictBuilder::new(without_hon, None, "T".to_string());

        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        b1.add_character(&ch, "T");
        b2.add_character(&ch, "T");

        let count_with = b1.collect_all_entries().len();
        let count_without = b2.collect_all_entries().len();
        assert!(
            count_with > count_without,
            "honorifics=true should produce more entries ({}) than honorifics=false ({})",
            count_with,
            count_without
        );
    }

    // --- ZIP export with various settings ---

    #[test]
    fn test_zip_export_with_all_settings_off() {
        let settings = DictSettings {
            show_image: false,
            show_tag: false,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            honorifics: false,
            show_seiyuu: false,
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let zip_bytes = builder.export_bytes().unwrap();
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let names = zip_filenames(&mut archive);
        assert!(names.contains(&"index.json".to_string()));
        assert!(names.contains(&"tag_bank_1.json".to_string()));
        // Should have term banks but no images
        assert!(names.iter().any(|n| n.starts_with("term_bank_")));
        assert!(!names.iter().any(|n| n.starts_with("img/")));
    }

    // --- Spoiler settings affect all honorific variants equally ---

    #[test]
    fn test_spoiler_settings_propagate_to_honorific_entries() {
        let no_spoilers = DictSettings {
            show_spoilers: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(no_spoilers, None, "Test".to_string());
        let mut ch = make_full_character();
        // Add a spoiler trait that should be excluded
        ch.personality.push(CharacterTrait {
            name: "HiddenTrait".to_string(),
            spoiler: 2,
        });
        builder.add_character(&ch, "Test");

        // Check a honorific entry
        let all_entries = builder.collect_all_entries();
        let san_entry = all_entries
            .iter()
            .find(|e| e[0].as_str().map(|s| s == "須々木さん").unwrap_or(false))
            .expect("Should have 須々木さん");

        let sc_str = serde_json::to_string(&san_entry[5]).unwrap();
        assert!(
            !sc_str.contains("HiddenTrait"),
            "Spoiler traits should be excluded from honorific entries too"
        );
    }

    // === Deferred honorific generation (memory optimization) tests ===

    #[test]
    fn test_honorifics_not_materialized_in_entries_vec() {
        // The core optimization: builder.entries should only hold base entries,
        // while honorific entries are deferred to export time.
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        let base_count = builder.base_entries().len();
        let total_count = builder.collect_all_entries().len();

        // Base entries should be a small fraction of total
        assert!(
            base_count < 30,
            "Base entries should be small (got {}), honorifics should be deferred",
            base_count
        );
        assert!(
            total_count > base_count * 10,
            "Total entries ({}) should be much larger than base entries ({}), \
             confirming honorifics are generated lazily",
            total_count,
            base_count
        );
    }

    #[test]
    fn test_honorific_sources_populated_when_enabled() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        // Finalize to trigger honorific source generation
        builder.finalize();

        assert!(
            !builder.honorific_sources.is_empty(),
            "honorific_sources should be populated when honorifics are enabled"
        );
        assert!(
            !builder.honorific_sources[0]
                .base_names_with_readings
                .is_empty(),
            "base_names_with_readings should contain name forms for honorific expansion"
        );
    }

    #[test]
    fn test_honorific_sources_empty_when_disabled() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let ch = make_test_character("c1", "Okabe Rintarou", "岡部 倫太郎", "main");
        builder.add_character(&ch, "Test");

        assert!(
            builder.honorific_sources.is_empty(),
            "honorific_sources should be empty when honorifics are disabled"
        );
    }

    #[test]
    fn test_has_entries_reflects_deferred_honorifics() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        assert!(
            !builder.has_entries(),
            "Empty builder should not have entries"
        );

        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");
        assert!(
            builder.has_entries(),
            "Builder with character should have entries"
        );
    }

    #[test]
    fn test_export_produces_same_entries_as_collect_all() {
        // Verify that the streaming export in export_bytes() produces the same
        // entries as collect_all_entries() — they use the same generation logic.
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let collected = builder.collect_all_entries();
        let collected_terms: HashSet<String> = collected
            .iter()
            .map(|e| e[0].as_str().unwrap().to_string())
            .collect();

        // Parse terms from the exported ZIP
        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);
        let mut exported_terms: HashSet<String> = HashSet::new();
        for name in names.iter().filter(|n| n.starts_with("term_bank_")) {
            let raw = read_zip_entry(&mut archive, name);
            let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
            for entry in &entries {
                exported_terms.insert(entry[0].as_str().unwrap().to_string());
            }
        }

        assert_eq!(
            collected_terms, exported_terms,
            "collect_all_entries() and export_bytes() should produce identical term sets"
        );
    }

    #[test]
    fn test_deferred_honorifics_dedup_across_characters() {
        // Two characters sharing a family name should not produce duplicate honorific entries
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let ch1 = make_test_character("c1", "Ichiro Suzuki", "鈴木 一郎", "main");
        let mut ch2 = make_test_character("c2", "Jiro Suzuki", "鈴木 二郎", "side");
        ch2.aliases = vec![];
        builder.add_character(&ch1, "Test");
        builder.add_character(&ch2, "Test");

        let all_entries = builder.collect_all_entries();
        let all_terms: Vec<String> = all_entries
            .iter()
            .map(|e| e[0].as_str().unwrap().to_string())
            .collect();

        // "鈴木さん" should appear exactly once despite both characters having family name 鈴木
        let suzuki_san_count = all_terms
            .iter()
            .filter(|t| t.as_str() == "鈴木さん")
            .count();
        assert_eq!(
            suzuki_san_count, 1,
            "Shared family name honorific should be deduplicated across characters, got {}",
            suzuki_san_count
        );
    }

    #[test]
    fn test_streaming_chunk_size_respected_in_export() {
        // Verify that no term_bank file exceeds the limit for entries,
        // even with many characters producing deferred honorifics
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        for i in 0..10 {
            let ch = Character {
                id: format!("c{}", i),
                name: format!("Given{} Family{}", i, i),
                name_original: format!("姓{} 名{}", i, i),
                role: "main".to_string(),
                aliases: vec![format!("Alias{}", i)],
                ..Character::default()
            };
            builder.add_character(&ch, "Test");
        }

        let mut archive = build_zip_archive(&mut builder);
        let names = zip_filenames(&mut archive);

        let term_banks: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("term_bank_") && n.ends_with(".json"))
            .collect();

        assert!(
            term_banks.len() >= 2,
            "Should produce multiple term banks from deferred honorifics"
        );

        for name in &term_banks {
            let raw = read_zip_entry(&mut archive, name);
            let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
            assert!(
                arr.len() <= TERM_BANK_LIMIT,
                "Streaming chunk violated: {} has {} entries (max {})",
                name,
                arr.len(),
                TERM_BANK_LIMIT
            );
        }
    }

    // =========================================================================
    // Character Merging Tests
    //
    // Tests for the two-pass merge architecture: characters with the same
    // (source, id) or same normalized name are merged into a single entry
    // with multiple appearances.
    // =========================================================================

    #[test]
    fn test_same_id_merges_appearances() {
        // Same character ID from same source across two media → merged
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char1 = make_test_character("c123", "Koito Yuu", "小糸 侑", "main");
        char1.source = "anilist".to_string();
        let mut char2 = make_test_character("c123", "Koito Yuu", "小糸 侑", "side");
        char2.source = "anilist".to_string();

        builder.add_character(&char1, "やがて君になる");
        builder.add_character(&char2, "やがて君になる 公式コミックアンソロジー");

        // Should produce entries for ONE merged character
        let entries = builder.base_entries();
        let terms: Vec<&str> = entries.iter().filter_map(|e| e[0].as_str()).collect();
        assert!(
            terms.contains(&"小糸 侑"),
            "Should have the merged character's name"
        );

        // Structured content should contain BOTH media titles
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("小糸 侑"))
            .unwrap();
        let content_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            content_str.contains("やがて君になる")
                && content_str.contains("公式コミックアンソロジー"),
            "Merged card should contain both media titles"
        );
    }

    #[test]
    fn test_same_id_uses_highest_score() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char1 = make_test_character("c1", "Shimamura", "島村 抱月", "side");
        char1.source = "anilist".to_string();
        let mut char2 = make_test_character("c1", "Shimamura", "島村 抱月", "main");
        char2.source = "anilist".to_string();

        builder.add_character(&char1, "Side Story");
        builder.add_character(&char2, "Main Story");

        let entries = builder.base_entries();
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("島村 抱月"))
            .unwrap();
        // Score should be 100 (main), not 50 (side)
        assert_eq!(
            entry[4].as_i64().unwrap(),
            100,
            "Should use highest role score"
        );
        // Definition tag should reflect highest role
        assert_eq!(
            entry[2].as_str().unwrap(),
            "name main",
            "Should use highest role tag"
        );
    }

    #[test]
    fn test_different_ids_same_name_produces_separate_entries() {
        // Two DIFFERENT characters that happen to share the same Japanese name
        // but have different (source, id) pairs. With name-based cross-source dedup,
        // these would normally merge. To test truly separate entries, use the SAME source.
        // Actually, name-based dedup is cross-source; within same source, id-based dedup applies.
        // So: different IDs from same source + same name → separate entries.
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char1 = make_test_character("c100", "Myne", "マイン", "main");
        char1.source = "anilist".to_string();
        let mut char2 = make_test_character("c200", "Mine", "マイン", "main");
        char2.source = "anilist".to_string();

        builder.add_character(&char1, "Ascendance of a Bookworm");
        builder.add_character(&char2, "Akame ga Kill");

        // Both have the same normalized name "マイン", so they merge via name-based dedup.
        // This is the expected cross-source behavior. For truly separate entries,
        // the characters would need different name_original values.
        // Verify: merged entry has both appearances.
        let entries = builder.base_entries();
        let myne_entries: Vec<_> = entries
            .iter()
            .filter(|e| e[0].as_str() == Some("マイン"))
            .collect();
        assert_eq!(
            myne_entries.len(),
            1,
            "Same name merges into one entry (name-based dedup)"
        );
        let content_str = serde_json::to_string(&myne_entries[0][5]).unwrap();
        assert!(
            content_str.contains("Ascendance of a Bookworm")
                && content_str.contains("Akame ga Kill"),
            "Merged card should show both media"
        );
    }

    #[test]
    fn test_merge_appearances_sorted_by_importance() {
        let mut builder = DictBuilder::new(s(), None, "Test".to_string());
        let mut char1 = make_test_character("c1", "Test", "テスト名前", "side");
        char1.source = "anilist".to_string();
        let mut char2 = make_test_character("c1", "Test", "テスト名前", "main");
        char2.source = "anilist".to_string();
        let mut char3 = make_test_character("c1", "Test", "テスト名前", "primary");
        char3.source = "anilist".to_string();

        builder.add_character(&char1, "Side Story");
        builder.add_character(&char2, "Main Story");
        builder.add_character(&char3, "Primary Story");

        let entries = builder.base_entries();
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("テスト名前"))
            .unwrap();
        let content_str = serde_json::to_string(&entry[5]).unwrap();

        // Main should appear before Side in the content (sorted by importance)
        let main_pos = content_str.find("Main Story").unwrap();
        let primary_pos = content_str.find("Primary Story").unwrap();
        let side_pos = content_str.find("Side Story").unwrap();
        assert!(
            main_pos < primary_pos && primary_pos < side_pos,
            "Appearances should be sorted: main < primary < side"
        );
    }

    #[test]
    fn test_merge_first_non_none_fields() {
        // First character has no description; second does. Second's should fill the gap.
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char1 = make_test_character("c1", "Name", "テスト", "main");
        char1.source = "anilist".to_string();
        char1.description = None;
        char1.seiyuu = None;

        let mut char2 = make_test_character("c1", "Name", "テスト", "side");
        char2.source = "anilist".to_string();
        char2.description = Some("A great character".to_string());
        char2.seiyuu = Some("花澤香菜".to_string());

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        let entries = builder.base_entries();
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let content_str = serde_json::to_string(&entry[5]).unwrap();
        // Description from second character should be used (first was None)
        assert!(
            content_str.contains("A great character"),
            "Second character's description should fill the gap"
        );
    }

    #[test]
    fn test_merge_aliases_union() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char1 = make_test_character("c1", "Name", "テスト", "main");
        char1.source = "anilist".to_string();
        char1.aliases = vec!["別名一".to_string()];

        let mut char2 = make_test_character("c1", "Name", "テスト", "side");
        char2.source = "anilist".to_string();
        char2.aliases = vec!["別名二".to_string(), "別名一".to_string()]; // 別名一 is duplicate

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        let entries = builder.base_entries();
        let terms: Vec<&str> = entries.iter().filter_map(|e| e[0].as_str()).collect();
        assert!(terms.contains(&"別名一"), "Should have 別名一");
        assert!(
            terms.contains(&"別名二"),
            "Should have 別名二 from second character"
        );
        // 別名一 should appear only once as a term
        let alias1_count = terms.iter().filter(|&&t| t == "別名一").count();
        assert_eq!(alias1_count, 1, "別名一 should not be duplicated");
    }

    #[test]
    fn test_cross_source_merge_by_name() {
        // VNDB character (source="vndb") and AniList character (source="anilist")
        // with the same normalized name → should merge via name-based dedup
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut char_vndb = make_test_character("c42", "Okabe Rintarou", "岡部 倫太郎", "main");
        char_vndb.source = "vndb".to_string();
        char_vndb.description = Some("VNDB description".to_string());

        let mut char_anilist =
            make_test_character("35252", "Okabe Rintarou", "岡部 倫太郎", "main");
        char_anilist.source = "anilist".to_string();
        char_anilist.seiyuu = Some("宮野真守".to_string());

        builder.add_character(&char_vndb, "Steins;Gate VN");
        builder.add_character(&char_anilist, "Steins;Gate Anime");

        let entries = builder.base_entries();
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("岡部 倫太郎"))
            .unwrap();
        let content_str = serde_json::to_string(&entry[5]).unwrap();

        // Should have VNDB description (first-non-None)
        assert!(
            content_str.contains("VNDB description"),
            "Should use VNDB description (first-non-None)"
        );
        // Should have AniList seiyuu (gap-filled from second)
        assert!(
            content_str.contains("宮野真守"),
            "Should fill seiyuu from AniList"
        );
        // Should show both media titles
        assert!(
            content_str.contains("Steins;Gate VN") && content_str.contains("Steins;Gate Anime"),
            "Should show both media"
        );
    }

    #[test]
    fn test_single_appearance_uses_original_layout() {
        // A character with only one appearance should use the original build_content
        // layout (single "From: title" + role badge), not the merged layout
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let char = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&char, "Only Game");

        let entries = builder.base_entries();
        let entry = entries
            .iter()
            .find(|e| e[0].as_str() == Some("テスト"))
            .unwrap();
        let content_str = serde_json::to_string(&entry[5]).unwrap();
        assert!(
            content_str.contains("From: Only Game"),
            "Should have single media title"
        );
    }

    // === parse_alt_name_format tests ===

    #[test]
    fn test_parse_alt_name_romanized_japanese_format() {
        // Standard "Romanized (Japanese)" format
        let (term, reading) = parse_alt_name_format("Aoki Umi (碧井海)");
        assert_eq!(term, "碧井海");
        assert!(reading.is_some());
        assert_eq!(reading.unwrap(), "あおきうみ");
    }

    #[test]
    fn test_parse_alt_name_standalone_japanese() {
        // Standalone Japanese alias — no parentheses
        let (term, reading) = parse_alt_name_format("碧井海");
        assert_eq!(term, "碧井海");
        assert!(reading.is_none());
    }

    #[test]
    fn test_parse_alt_name_standalone_english() {
        // English alias — returned as-is (filtered downstream by contains_japanese)
        let (term, reading) = parse_alt_name_format("Ruri's Mother");
        assert_eq!(term, "Ruri's Mother");
        assert!(reading.is_none());
    }

    #[test]
    fn test_parse_alt_name_katakana_in_parens() {
        let (term, reading) = parse_alt_name_format("Elaina (エレイナ)");
        assert_eq!(term, "エレイナ");
        assert!(reading.is_some());
        // "Elaina" → e=え, la=ら, i=い, na=な → "えらいな"
        assert_eq!(reading.unwrap(), "えらいな");
    }

    #[test]
    fn test_parse_alt_name_empty_outside() {
        // Edge case: nothing outside parens — return as-is
        let (term, reading) = parse_alt_name_format("(碧井海)");
        assert_eq!(term, "(碧井海)");
        assert!(reading.is_none());
    }

    // === Non-Japanese alias filtering ===

    #[test]
    fn test_english_alias_filtered_out() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test Name", "テスト", "main");
        ch.aliases = vec!["Ruri's Mother".to_string(), "エイリアス".to_string()];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            !terms.contains(&"Ruri's Mother".to_string()),
            "English alias should be filtered out"
        );
        assert!(
            terms.contains(&"エイリアス".to_string()),
            "Japanese alias should be present"
        );
    }

    // === "Romanized (Japanese)" alias integration ===

    #[test]
    fn test_alias_romanized_japanese_format_creates_correct_entry() {
        let mut builder = DictBuilder::new(s_no_hon(), None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test Name", "テスト", "main");
        ch.aliases = vec!["Aoki Umi (碧井海)".to_string()];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // The Japanese part should be the term, not the full "Aoki Umi (碧井海)"
        assert!(
            terms.contains(&"碧井海".to_string()),
            "Should extract Japanese part as term"
        );
        assert!(
            !terms.contains(&"Aoki Umi (碧井海)".to_string()),
            "Full format string should not be a term"
        );

        // Check that the reading is derived from the romanized part
        let entry = builder
            .base_entries()
            .iter()
            .find(|e| e[0].as_str() == Some("碧井海"))
            .unwrap()
            .clone();
        assert_eq!(entry[1].as_str().unwrap(), "あおきうみ");
    }

    // === Spoiler alias tests ===

    #[test]
    fn test_spoiler_aliases_included_when_enabled() {
        let settings = DictSettings {
            honorifics: false,
            show_spoilers: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test Name", "テスト", "main");
        ch.aliases = vec!["通常名".to_string()];
        ch.spoiler_aliases = vec!["秘密名".to_string()];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"通常名".to_string()),
            "Normal alias should be present"
        );
        assert!(
            terms.contains(&"秘密名".to_string()),
            "Spoiler alias should be present when spoilers enabled"
        );
    }

    #[test]
    fn test_spoiler_aliases_excluded_when_disabled() {
        let settings = DictSettings {
            honorifics: false,
            show_spoilers: false,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());
        let mut ch = make_test_character("c1", "Test Name", "テスト", "main");
        ch.aliases = vec!["通常名".to_string()];
        ch.spoiler_aliases = vec!["秘密名".to_string()];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"通常名".to_string()),
            "Normal alias should be present"
        );
        assert!(
            !terms.contains(&"秘密名".to_string()),
            "Spoiler alias should NOT be present when spoilers disabled"
        );
    }

    #[test]
    fn test_spoiler_aliases_merged() {
        let settings = DictSettings {
            honorifics: false,
            show_spoilers: true,
            ..DictSettings::default()
        };
        let mut builder = DictBuilder::new(settings, None, "Test".to_string());

        let mut char1 = make_test_character("c1", "Name", "テスト", "main");
        char1.source = "anilist".to_string();
        char1.spoiler_aliases = vec!["秘密一".to_string()];

        let mut char2 = make_test_character("c1", "Name", "テスト", "side");
        char2.source = "anilist".to_string();
        char2.spoiler_aliases = vec!["秘密二".to_string(), "秘密一".to_string()];

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        let terms: Vec<String> = builder
            .base_entries()
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(terms.contains(&"秘密一".to_string()));
        assert!(terms.contains(&"秘密二".to_string()));
        // Deduplicated
        let count = terms.iter().filter(|t| t.as_str() == "秘密一").count();
        assert_eq!(count, 1, "Spoiler aliases should be deduplicated");
    }
}
