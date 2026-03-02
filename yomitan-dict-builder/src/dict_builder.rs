use std::collections::HashSet;
use std::io::{Cursor, Write};

use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::content_builder::ContentBuilder;
use crate::image_handler::ImageHandler;
use crate::kana;
use crate::models::*;
use crate::name_parser::{self, HONORIFIC_SUFFIXES};

fn get_score(role: &str) -> i32 {
    match role {
        "main" => 100,
        "primary" => 75,
        "side" => 50,
        "appears" => 25,
        _ => 0,
    }
}

pub struct DictBuilder {
    pub entries: Vec<serde_json::Value>,
    images: Vec<(String, Vec<u8>)>, // (filename, bytes) for ZIP img/ folder
    spoiler_level: u8,
    revision: String,
    download_url: Option<String>,
    game_title: String,
    honorifics: bool,
}

impl DictBuilder {
    pub fn new(
        spoiler_level: u8,
        download_url: Option<String>,
        game_title: String,
        honorifics: bool,
    ) -> Self {
        // Random 12-digit revision string
        let revision: u64 = rand::random::<u64>() % 1_000_000_000_000;
        Self {
            entries: Vec::new(),
            images: Vec::new(),
            spoiler_level,
            revision: format!("{:012}", revision),
            download_url,
            game_title,
            honorifics,
        }
    }

    /// Process a single character and create all term entries.
    pub fn add_character(&mut self, char: &Character, game_title: &str) {
        let name_original = &char.name_original;
        if name_original.is_empty() {
            tracing::warn!(
                id = %char.id,
                name = %char.name,
                "Skipping character with no Japanese name (name_original is empty)"
            );
            return;
        }

        // Generate hiragana readings using unified name handling (supports hints)
        let readings = name_parser::generate_name_readings(
            name_original,
            &char.name,
            char.first_name_hint.as_deref(),
            char.last_name_hint.as_deref(),
        );

        let role = &char.role;
        let score = get_score(role);

        let content_builder = ContentBuilder::new(self.spoiler_level);

        // Handle image: use raw bytes from download + resize
        let image_path = if let Some(ref img_bytes) = char.image_bytes {
            let ext = char.image_ext.as_deref().unwrap_or("jpg");
            let filename = ImageHandler::make_filename(&char.id, ext);
            let path = format!("img/{}", filename);
            self.images.push((filename, img_bytes.clone()));
            Some(path)
        } else {
            None
        };

        // Build the structured content card (shared across all entries for this character)
        let image_dims = match (char.image_width, char.image_height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        };
        let structured_content =
            content_builder.build_content(char, image_path.as_deref(), image_dims, game_title);

        // Track terms to avoid duplicates
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

            // 3. Family name only and 4. Given name only — for ALL split candidates.
            // When the split is unambiguous, all_candidates has one entry (same as
            // name_parts). When ambiguous, this adds entries for every plausible
            // family/given pair (e.g., both "石井"/"守" and "石"/"井守").
            for candidate in &all_candidates {
                if let Some(ref family) = candidate.family {
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
                if let Some(ref given) = candidate.given {
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

        // --- Hiragana / Katakana term entries ---
        // When the original name contains kanji, also add entries where the term
        // itself is the hiragana or katakana form so lookups work on kana text too.

        if has_split {
            // Hiragana combined (no space): "すずきしんいち"
            let hira_combined = format!("{}{}", readings.family, readings.given);
            if !hira_combined.is_empty() && added_terms.insert(hira_combined.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &hira_combined,
                    &readings.full,
                    role,
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
                    role,
                    score,
                    &structured_content,
                ));
            }
            // Hiragana family only
            if !readings.family.is_empty() && added_terms.insert(readings.family.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &readings.family,
                    &readings.family,
                    role,
                    score,
                    &structured_content,
                ));
            }
            // Hiragana given only
            if !readings.given.is_empty() && added_terms.insert(readings.given.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &readings.given,
                    &readings.given,
                    role,
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
                    role,
                    score,
                    &structured_content,
                ));
            }
            let kata_spaced = format!("{} {}", kata_family, kata_given);
            if added_terms.insert(kata_spaced.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_spaced,
                    &readings.full,
                    role,
                    score,
                    &structured_content,
                ));
            }
            if !kata_family.is_empty() && added_terms.insert(kata_family.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_family,
                    &readings.family,
                    role,
                    score,
                    &structured_content,
                ));
            }
            if !kata_given.is_empty() && added_terms.insert(kata_given.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_given,
                    &readings.given,
                    role,
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
                    role,
                    score,
                    &structured_content,
                ));
            }
            let kata_full = kana::hira_to_kata(&readings.full);
            if !kata_full.is_empty() && added_terms.insert(kata_full.clone()) {
                self.entries.push(ContentBuilder::create_term_entry(
                    &kata_full,
                    &readings.full,
                    role,
                    score,
                    &structured_content,
                ));
            }
        }

        // --- Honorific suffix variants for all base names ---
        // Includes original kanji forms + hiragana/katakana forms

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

        if self.honorifics {
            for (base_name, base_reading) in &base_names_with_readings {
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
                if self.honorifics {
                    for (suffix, suffix_reading, description) in HONORIFIC_SUFFIXES {
                        let alias_with_suffix = format!("{}{}", alias, suffix);
                        let reading_with_suffix = format!("{}{}", readings.full, suffix_reading);

                        if added_terms.insert(alias_with_suffix.clone()) {
                            let honorific_content = ContentBuilder::build_honorific_content(
                                &structured_content,
                                suffix,
                                description,
                            );
                            self.entries.push(ContentBuilder::create_term_entry(
                                &alias_with_suffix,
                                &reading_with_suffix,
                                role,
                                score,
                                &honorific_content,
                            ));
                        }
                    }
                }
            }
        }
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

        json!([
            ["name", "partOfSpeech", 0, "Character name", 0],
            ["main", "name", 0, "Protagonist", 0],
            ["primary", "name", 0, "Main character", 0],
            ["side", "name", 0, "Side character", 0],
            ["appears", "name", 0, "Minor appearance", 0],
            ["docker-built", "meta", 0, build_timestamp, 0],
            ["dict-built", "meta", 0, dict_built, 0]
        ])
    }

    /// Export the dictionary as in-memory ZIP bytes.
    pub fn export_bytes(&self) -> Result<Vec<u8>, String> {
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

        // 3. term_bank_N.json (chunked at 10,000 entries per file)
        let entries_per_bank = 10_000;
        for (i, chunk) in self.entries.chunks(entries_per_bank).enumerate() {
            let filename = format!("term_bank_{}.json", i + 1);
            zip.start_file(&filename, json_options)
                .map_err(|e| format!("Failed to create {} in ZIP: {}", filename, e))?;
            let data = serde_json::to_string(chunk)
                .map_err(|e| format!("Failed to serialize {}: {}", filename, e))?;
            zip.write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write {}: {}", filename, e))?;
        }

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
    use crate::models::{Character, CharacterTrait};
    use std::collections::HashSet;
    use std::io::Read;

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
            aliases: vec!["TestAlias".to_string()],
            personality: vec![CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            }],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Name", "", "main");
        builder.add_character(&char, "Test Game");
        assert_eq!(
            builder.entries.len(),
            0,
            "Empty name_original should produce no entries"
        );
    }

    #[test]
    fn test_add_character_creates_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");
        assert!(
            builder.entries.len() > 0,
            "Should create at least one entry"
        );
    }

    #[test]
    fn test_add_character_two_part_name_creates_base_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        // Collect all terms
        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        // Find an entry ending with さん
        let san_entry = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Name", "名前", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"TestAlias".to_string()),
            "Should have alias entry"
        );
    }

    #[test]
    fn test_add_character_deduplication() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let mut char = make_test_character("c1", "Name", "名前", "main");
        // Set alias same as original name
        char.aliases = vec!["名前".to_string()];
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Count occurrences of the name
        let count = terms.iter().filter(|t| t.as_str() == "名前").count();
        assert_eq!(count, 1, "Duplicate terms should be deduplicated");
    }

    #[test]
    fn test_add_character_single_word_name() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let char = make_test_character("c1", "Saber", "セイバー", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .entries
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
            0,
            Some("http://127.0.0.1:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Test Game".to_string(),
            true,
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
        let builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let index = builder.create_index_public();

        assert_eq!(index["title"], "Bee's Character Dictionary");
        assert!(index.get("downloadUrl").is_none() || index["downloadUrl"].is_null());
    }

    #[test]
    fn test_index_metadata_empty_title() {
        let builder = DictBuilder::new(0, None, String::new(), true);
        let index = builder.create_index_public();
        assert_eq!(
            index["description"].as_str().unwrap(),
            "Character names dictionary"
        );
    }

    // === ZIP export tests ===

    #[test]
    fn test_export_bytes_produces_valid_zip() {
        let mut builder = DictBuilder::new(0, None, "Test Game".to_string(), true);
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
        let mut builder = DictBuilder::new(0, None, "Test Game".to_string(), true);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
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
        let mut builder = DictBuilder::new(0, None, "Multi-title dict".to_string(), true);

        let char1 = make_test_character("c1", "Name1", "名前一", "main");
        let char2 = make_test_character("c2", "Name2", "名前二", "side");

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Both characters should have entries
        assert!(
            builder.entries.len() > 2,
            "Should have entries from both characters"
        );

        // Verify different game titles in structured content
        let entry1_content = &builder.entries[0][5][0];
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
    fn build_zip_archive(builder: &DictBuilder) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
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
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: Some("https://example.com/img.jpg".to_string()),
            image_bytes: Some(raw),
            image_ext: Some("jpg".to_string()),
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
        }
    }

    // --- index.json validation (Yomitan format 3 requirements) ---

    #[test]
    fn test_yomitan_index_required_fields() {
        let builder = DictBuilder::new(
            0,
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Steins;Gate".to_string(),
            true,
        );
        let mut archive = build_zip_archive(&builder);
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
        let builder = DictBuilder::new(0, Some(url.to_string()), "Test".to_string(), true);
        let mut archive = build_zip_archive(&builder);
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
        let builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let mut archive = build_zip_archive(&builder);
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
    fn test_yomitan_revision_is_unique_per_build() {
        let b1 = DictBuilder::new(0, None, "T".to_string(), true);
        let b2 = DictBuilder::new(0, None, "T".to_string(), true);
        // Revisions should differ (random). Extremely unlikely to collide.
        assert_ne!(
            b1.revision, b2.revision,
            "Each build must have a unique revision"
        );
    }

    // --- tag_bank validation ---

    #[test]
    fn test_yomitan_tag_bank_format() {
        let builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let mut archive = build_zip_archive(&builder);
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
        let builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let mut archive = build_zip_archive(&builder);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test Game");

        // Validate every entry matches Yomitan's expected schema
        for (i, entry) in builder.entries.iter().enumerate() {
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        for entry in &builder.entries {
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);

        let roles_scores = [
            ("main", 100),
            ("primary", 75),
            ("side", 50),
            ("appears", 25),
        ];
        for (role, expected_score) in &roles_scores {
            let ch = make_test_character("c1", "Test", "テスト", role);
            builder.entries.clear();
            builder.add_character(&ch, "Test");

            for entry in &builder.entries {
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&builder);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&builder);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&builder);
        let names = zip_filenames(&mut archive);

        // Collect all image paths referenced in structured content
        let mut referenced_paths: HashSet<String> = HashSet::new();
        for entry in &builder.entries {
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        let mut archive = build_zip_archive(&builder);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);

        // Generate enough entries to force multiple term banks.
        // With ~170 honorific suffixes, each character produces ~2000+ entries,
        // so only a handful of characters are needed to exceed 10,000.
        for i in 0..10 {
            let ch = Character {
                id: format!("c{}", i),
                name: format!("Given{} Family{}", i, i),
                name_original: format!("姓{} 名{}", i, i),
                role: "main".to_string(),
                sex: None,
                age: None,
                height: None,
                weight: None,
                blood_type: None,
                birthday: None,
                description: None,
                aliases: vec![format!("Alias{}", i)],
                personality: vec![],
                roles: vec![],
                engages_in: vec![],
                subject_of: vec![],
                image_url: None,
                image_bytes: None,
                image_ext: None,
                image_width: None,
                image_height: None,
                first_name_hint: None,
                last_name_hint: None,
            };
            builder.add_character(&ch, "Test");
        }

        assert!(
            builder.entries.len() > 10_000,
            "Need >10k entries to test chunking, got {}",
            builder.entries.len()
        );

        let mut archive = build_zip_archive(&builder);
        let names = zip_filenames(&mut archive);

        let term_banks: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("term_bank_") && n.ends_with(".json"))
            .collect();

        assert!(
            term_banks.len() >= 2,
            "Should have at least 2 term banks with >10k entries, got {}",
            term_banks.len()
        );

        // Verify each bank has at most 10,000 entries
        for name in &term_banks {
            let raw = read_zip_entry(&mut archive, name);
            let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
            assert!(
                arr.len() <= 10_000,
                "{} has {} entries, max is 10,000",
                name,
                arr.len()
            );
        }
    }

    // --- End-to-end realistic character import ---

    #[test]
    fn test_yomitan_full_import_simulation() {
        // Simulate what Yomitan does: unzip → parse index → parse tags → parse all term banks
        let mut builder = DictBuilder::new(
            0,
            Some("http://localhost:3000/api/yomitan-dict?source=vndb&id=v17".to_string()),
            "Steins;Gate".to_string(),
            true,
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        // Collect (term, reading) pairs — should be unique
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for entry in &builder.entries {
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
        let builder = DictBuilder::new(0, None, "Empty".to_string(), true);
        let mut archive = build_zip_archive(&builder);
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);

        // Character with empty name_original
        let ch = make_test_character("c1", "John Smith", "", "main");
        builder.add_character(&ch, "Test");

        assert_eq!(
            builder.entries.len(),
            0,
            "Characters without Japanese names must produce no entries"
        );

        // ZIP should still be valid
        let mut archive = build_zip_archive(&builder);
        let names = zip_filenames(&mut archive);
        assert!(names.contains(&"index.json".to_string()));
    }

    #[test]
    fn test_yomitan_spoiler_level_affects_content() {
        // Level 0: spoiler traits should be excluded
        let mut builder_l0 = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character(); // has a spoiler=2 trait "Secret identity"
        builder_l0.add_character(&ch, "Test");

        // Level 2: spoiler traits should be included
        let mut builder_l2 = DictBuilder::new(2, None, "Test".to_string(), true);
        builder_l2.add_character(&ch, "Test");

        // Find the base name entry in each
        let find_base = |entries: &[serde_json::Value]| -> String {
            let entry = entries
                .iter()
                .find(|e| e[0].as_str().unwrap() == "須々木 心一")
                .unwrap();
            serde_json::to_string(&entry[5]).unwrap()
        };

        let content_l0 = find_base(&builder_l0.entries);
        let content_l2 = find_base(&builder_l2.entries);

        assert!(
            !content_l0.contains("Secret identity"),
            "Spoiler level 0 should exclude spoiler=2 traits"
        );
        assert!(
            content_l2.contains("Secret identity"),
            "Spoiler level 2 should include spoiler=2 traits"
        );
    }

    #[test]
    fn test_yomitan_single_word_name_no_family_given_split() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_test_character("c1", "Saber", "セイバー", "main");
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), true);
        let ch = make_full_character();
        builder.add_character(&ch, "Test");

        for entry in &builder.entries {
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let char = make_test_character("c1", "Name", " ", "main");
        builder.add_character(&char, "Test");

        // " " splits into family="" and given=""
        // Empty checks should prevent entries for empty parts
        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let mut char = make_test_character("c1", "Saber", "セイバー", "main");
        // Alias is the hiragana form of the name
        char.aliases = vec!["せいばー".to_string()];
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .entries
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

    // === Edge case: two characters with same name ===

    #[test]
    fn test_two_characters_same_name_both_get_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let char1 = make_test_character("c1", "Saber", "セイバー", "main");
        let char2 = make_test_character("c2", "Saber", "セイバー", "side");
        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Both characters should produce entries (no cross-character dedup)
        // The terms will be the same but the structured content differs
        let saber_entries: Vec<&serde_json::Value> = builder
            .entries
            .iter()
            .filter(|e| e[0].as_str() == Some("セイバー"))
            .collect();

        assert!(
            saber_entries.len() >= 2,
            "Two characters with same name should both produce entries, got {}",
            saber_entries.len()
        );
    }

    // === Edge case: character with many empty aliases ===

    #[test]
    fn test_empty_aliases_skipped() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let mut char = make_test_character("c1", "Name", "名前", "main");
        char.aliases = vec![
            "".to_string(),
            "".to_string(),
            "Valid".to_string(),
            "".to_string(),
        ];
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            terms.contains(&"Valid".to_string()),
            "Non-empty alias should be present"
        );
        // Empty aliases should not produce entries
        let empty_count = terms.iter().filter(|t| t.is_empty()).count();
        assert_eq!(empty_count, 0, "Empty aliases should not produce entries");
    }

    // === Edge case: hiragana and katakana term entries ===

    #[test]
    fn test_kana_term_entries_generated() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test");

        let terms: Vec<String> = builder
            .entries
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
            0,
            Some(
                "http://localhost:3000/api/yomitan-dict?source=vndb&id=v17&spoiler_level=0"
                    .to_string(),
            ),
            "Test".to_string(),
            true,
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let mut ch = make_test_character("c1", "Mamoru Ishii", "石井守", "main");
        ch.first_name_hint = Some("Mamoru".to_string());
        ch.last_name_hint = Some("Ishii".to_string());
        ch.aliases = vec![];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .entries
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string(), false);
        let mut ch = make_test_character("c1", "Souma Yukihira", "幸平創真", "main");
        ch.first_name_hint = Some("Souma".to_string());
        ch.last_name_hint = Some("Yukihira".to_string());
        ch.aliases = vec![];
        builder.add_character(&ch, "Test");

        let terms: Vec<String> = builder
            .entries
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
}
