use std::collections::HashSet;
use std::io::{Cursor, Write};

use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::content_builder::ContentBuilder;
use crate::image_handler::ImageHandler;
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

        let mut base_names_with_readings: Vec<(String, String)> = Vec::new();
        if name_parts.has_space {
            if let Some(ref family) = name_parts.family {
                if !family.is_empty() {
                    base_names_with_readings
                        .push((family.clone(), readings.family.clone()));
                }
            }
            if let Some(ref given) = name_parts.given {
                if !given.is_empty() {
                    base_names_with_readings
                        .push((given.clone(), readings.given.clone()));
                }
            }
            if !name_parts.combined.is_empty() {
                base_names_with_readings
                    .push((name_parts.combined.clone(), readings.full.clone()));
            }
            if !name_parts.original.is_empty() {
                base_names_with_readings
                    .push((name_parts.original.clone(), readings.full.clone()));
            }
        } else if !name_original.is_empty() {
            base_names_with_readings
                .push((name_original.clone(), readings.full.clone()));
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

    /// Create index.json metadata.
    fn create_index(&self) -> serde_json::Value {
        let description = if self.game_title.is_empty() {
            "Character names dictionary".to_string()
        } else {
            format!("Character names from {}", self.game_title)
        };

        let mut index = json!({
            "title": "GSM Character Dictionary",
            "revision": &self.revision,
            "format": 3,
            "author": "GameSentenceMiner",
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
        json!([
            ["name", "partOfSpeech", 0, "Character name", 0],
            ["main", "name", 0, "Protagonist", 0],
            ["primary", "name", 0, "Main character", 0],
            ["side", "name", 0, "Side character", 0],
            ["appears", "name", 0, "Minor appearance", 0]
        ])
    }

    /// Export the dictionary as in-memory ZIP bytes.
    pub fn export_bytes(&self) -> Vec<u8> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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
            zip.start_file(format!("img/{}", filename), options)
                .unwrap();
            zip.write_all(bytes).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Character, CharacterTrait};
    use std::io::Read;

    fn make_test_character(
        id: &str,
        name: &str,
        name_original: &str,
        role: &str,
    ) -> Character {
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
            image_base64: None,
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
        let char = make_test_character("c1", "Name", "", "main");
        builder.add_character(&char, "Test Game");
        assert_eq!(builder.entries.len(), 0, "Empty name_original should produce no entries");
    }

    #[test]
    fn test_add_character_creates_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");
        assert!(
            builder.entries.len() > 0,
            "Should create at least one entry"
        );
    }

    #[test]
    fn test_add_character_two_part_name_creates_base_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
        let char = make_test_character("c1", "Shinichi Suzuki", "須々木 心一", "main");
        builder.add_character(&char, "Test Game");

        // Collect all terms
        let terms: Vec<String> = builder
            .entries
            .iter()
            .filter_map(|e| e[0].as_str().map(|s| s.to_string()))
            .collect();

        // Should have: original with space, combined, family, given
        assert!(terms.contains(&"須々木 心一".to_string()), "Should have original with space");
        assert!(terms.contains(&"須々木心一".to_string()), "Should have combined");
        assert!(terms.contains(&"須々木".to_string()), "Should have family name");
        assert!(terms.contains(&"心一".to_string()), "Should have given name");
    }

    #[test]
    fn test_add_character_honorific_variants() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
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
    fn test_add_character_alias_entries() {
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
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
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
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
        );
        let index = builder.create_index_public();

        assert_eq!(index["title"], "GSM Character Dictionary");
        assert_eq!(index["format"], 3);
        assert_eq!(index["author"], "GameSentenceMiner");
        assert!(index["description"].as_str().unwrap().contains("Test Game"));
        assert!(index["downloadUrl"].as_str().is_some());
        assert!(index["indexUrl"].as_str().unwrap().contains("/api/yomitan-index"));
        assert_eq!(index["isUpdatable"], true);
    }

    #[test]
    fn test_index_metadata_no_download_url() {
        let builder = DictBuilder::new(0, None, "Test".to_string());
        let index = builder.create_index_public();

        assert_eq!(index["title"], "GSM Character Dictionary");
        assert!(index.get("downloadUrl").is_none() || index["downloadUrl"].is_null());
    }

    #[test]
    fn test_index_metadata_empty_title() {
        let builder = DictBuilder::new(0, None, String::new());
        let index = builder.create_index_public();
        assert_eq!(
            index["description"].as_str().unwrap(),
            "Character names dictionary"
        );
    }

    // === ZIP export tests ===

    #[test]
    fn test_export_bytes_produces_valid_zip() {
        let mut builder = DictBuilder::new(0, None, "Test Game".to_string());
        let char = make_test_character("c1", "Test Name", "テスト", "main");
        builder.add_character(&char, "Test Game");

        let zip_bytes = builder.export_bytes();
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
        let mut builder = DictBuilder::new(0, None, "Test Game".to_string());
        let char = make_test_character("c1", "Test", "テスト", "main");
        builder.add_character(&char, "Test Game");

        let zip_bytes = builder.export_bytes();
        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut index_file = archive.by_name("index.json").unwrap();
        let mut contents = String::new();
        index_file.read_to_string(&mut contents).unwrap();

        let index: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(index["title"], "GSM Character Dictionary");
        assert_eq!(index["format"], 3);
    }

    #[test]
    fn test_export_bytes_with_image() {
        use base64::Engine;
        let mut builder = DictBuilder::new(0, None, "Test".to_string());
        let mut char = make_test_character("c1", "Test", "テスト", "main");
        // Provide a base64 image
        let raw = vec![0xFF, 0xD8, 0xFF];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        char.image_base64 = Some(format!("data:image/jpeg;base64,{}", b64));
        builder.add_character(&char, "Test");

        let zip_bytes = builder.export_bytes();
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
        let mut builder = DictBuilder::new(0, None, "Multi-title dict".to_string());

        let char1 = make_test_character("c1", "Name1", "名前一", "main");
        let char2 = make_test_character("c2", "Name2", "名前二", "side");

        builder.add_character(&char1, "Game A");
        builder.add_character(&char2, "Game B");

        // Both characters should have entries
        assert!(builder.entries.len() > 2, "Should have entries from both characters");

        // Verify different game titles in structured content
        let entry1_content = &builder.entries[0][5][0];
        let entry1_str = serde_json::to_string(entry1_content).unwrap();
        assert!(
            entry1_str.contains("Game A"),
            "First character should reference Game A"
        );
    }
}
