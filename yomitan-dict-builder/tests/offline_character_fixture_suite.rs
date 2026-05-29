use std::collections::HashSet;
use std::io::{Cursor, Read};

use serde_json::Value;
use yomitan_dict_builder::content_builder::DictSettings;
use yomitan_dict_builder::dict_builder::DictBuilder;
use yomitan_dict_builder::models::{Character, CharacterTrait};

#[test]
fn offline_character_fixture_exports_valid_yomitan_zip() {
    let mut builder = build_offline_character_fixture_dictionary();
    let mut archive = zip::ZipArchive::new(Cursor::new(builder.export_bytes().unwrap())).unwrap();

    let names = zip_filenames(&mut archive);
    assert!(names.contains(&"index.json".to_string()));
    assert!(names.contains(&"tag_bank_1.json".to_string()));
    assert!(names.contains(&"term_bank_1.json".to_string()));
    assert!(names.contains(&"term_bank_2.json".to_string()));
    assert!(names.iter().any(|name| name == "img/cvndb-crossover.jpg"));
    let first_bank: Vec<Value> =
        serde_json::from_str(&read_zip_entry(&mut archive, "term_bank_1.json")).unwrap();
    let second_bank: Vec<Value> =
        serde_json::from_str(&read_zip_entry(&mut archive, "term_bank_2.json")).unwrap();
    assert_eq!(first_bank.len(), 2_000);
    assert!(!second_bank.is_empty());

    let index: Value = serde_json::from_str(&read_zip_entry(&mut archive, "index.json")).unwrap();
    assert_eq!(index["format"], 3);
    assert_eq!(index["title"], "Bee's Character Dictionary");

    let tag_bank: Value =
        serde_json::from_str(&read_zip_entry(&mut archive, "tag_bank_1.json")).unwrap();
    assert!(tag_bank
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag[0] == "main"));

    let term_entries = all_term_entries(&mut archive, &names);
    assert!(!term_entries.is_empty());
    assert_no_duplicate_terms(&term_entries);
    assert_term_entry_field_types(&term_entries);
    assert_image_paths_resolve(&term_entries, &names);
}

#[test]
fn offline_character_fixture_covers_merge_names_honorifics_and_skips() {
    let mut builder = build_offline_character_fixture_dictionary();
    let mut archive = zip::ZipArchive::new(Cursor::new(builder.export_bytes().unwrap())).unwrap();
    let names = zip_filenames(&mut archive);
    let term_entries = all_term_entries(&mut archive, &names);

    assert_eq!(
        builder.skipped_no_japanese_summary(),
        vec![("English Only Show".to_string(), 1)]
    );

    assert_term(&term_entries, "春日野 穹", "かすがのそら");
    assert_term(&term_entries, "春日野穹", "かすがのそら");
    assert_term(&term_entries, "春日野", "かすがの");
    assert_term(&term_entries, "穹", "そら");
    assert_term(&term_entries, "セイバー", "せいばー");
    assert_term(&term_entries, "アイリ", "あいり");
    assert_term(&term_entries, "藍り", "あいり");
    assert_term(&term_entries, "佐藤 花", "さとうはな");
    assert_term(&term_entries, "碧井海", "あおきうみ");
    assert_term(&term_entries, "春日野さん", "かすがのさん");
    assert_term(&term_entries, "穹ちゃん", "そらちゃん");

    let merged_entry = term_entries
        .iter()
        .find(|entry| entry[0].as_str() == Some("春日野 穹"))
        .expect("merged base term should exist");
    let card = serde_json::to_string(&merged_entry[5]).unwrap();
    assert!(card.contains("VN Fixture"));
    assert!(card.contains("VN Fixture Encore"));
    assert!(card.contains("AniList Fixture"));
    assert!(merged_entry[2]
        .as_str()
        .unwrap()
        .split_whitespace()
        .any(|tag| tag == "main"));
    assert_eq!(merged_entry[4].as_i64(), Some(100));
    assert!(card.contains("Protagonist"));
    assert!(card.contains("Female"));
    assert!(card.contains("16"));
    assert!(card.contains("160"));
    assert!(card.contains("45"));
    assert!(card.contains("AB"));
    assert!(card.contains("March 3"));
    assert!(card.contains("Gentle"));
    assert!(card.contains("Student"));
    assert!(card.contains("Cooking"));
    assert!(card.contains("Mascot"));
    assert!(card.contains("Public description"));
    assert!(!card.contains("hidden spoiler"));
}

fn build_offline_character_fixture_dictionary() -> DictBuilder {
    let settings = DictSettings {
        show_spoilers: false,
        ..DictSettings::default()
    };
    let mut builder = DictBuilder::new(settings, None, "Offline character fixtures".to_string());

    let mut vndb_crossover = Character {
        id: "vndb-crossover".to_string(),
        name: "Kasugano Sora".to_string(),
        name_original: "春日野 穹".to_string(),
        role: "side".to_string(),
        source: "vndb".to_string(),
        sex: Some("f".to_string()),
        age: Some("16".to_string()),
        height: Some(160),
        weight: Some(45),
        blood_type: Some("AB".to_string()),
        birthday: Some(vec![3, 3]),
        description: Some("Public description. [spoiler]hidden spoiler[/spoiler]".to_string()),
        aliases: vec!["穹".to_string(), "Kasugano Sora (春日野穹)".to_string()],
        personality: vec![
            CharacterTrait {
                name: "Gentle".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "HiddenTrait".to_string(),
                spoiler: 2,
            },
        ],
        roles: vec![CharacterTrait {
            name: "Student".to_string(),
            spoiler: 0,
        }],
        engages_in: vec![CharacterTrait {
            name: "Cooking".to_string(),
            spoiler: 0,
        }],
        subject_of: vec![CharacterTrait {
            name: "Mascot".to_string(),
            spoiler: 0,
        }],
        image_bytes: Some(vec![0xFF, 0xD8, 0xFF, 0xE0]),
        image_ext: Some("jpg".to_string()),
        image_width: Some(96),
        image_height: Some(144),
        first_name_hint: Some("Sora".to_string()),
        last_name_hint: Some("Kasugano".to_string()),
        ..Character::default()
    };
    builder.add_character(&vndb_crossover, "VN Fixture");
    builder.add_character(&vndb_crossover, "VN Fixture Encore");

    vndb_crossover.id = "anilist-crossover".to_string();
    vndb_crossover.source = "anilist".to_string();
    vndb_crossover.role = "main".to_string();
    vndb_crossover.description = None;
    vndb_crossover.seiyuu = Some("田村ゆかり".to_string());
    vndb_crossover.image_bytes = None;
    vndb_crossover.image_ext = None;
    builder.add_character(&vndb_crossover, "AniList Fixture");

    builder.add_character(
        &Character {
            id: "anilist-katakana".to_string(),
            name: "Saber".to_string(),
            name_original: "セイバー".to_string(),
            role: "primary".to_string(),
            source: "anilist".to_string(),
            aliases: vec!["騎士王".to_string()],
            ..Character::default()
        },
        "Katakana Fixture",
    );

    builder.add_character(
        &Character {
            id: "vndb-mixed".to_string(),
            name: "Airi".to_string(),
            name_original: "藍り".to_string(),
            role: "appears".to_string(),
            source: "vndb".to_string(),
            aliases: vec!["アイリ".to_string()],
            ..Character::default()
        },
        "Mixed Kana Fixture",
    );

    builder.add_character(
        &Character {
            id: "anilist-spaced".to_string(),
            name: "Satou Hana".to_string(),
            name_original: "佐藤 花".to_string(),
            role: "side".to_string(),
            source: "anilist".to_string(),
            first_name_hint: Some("Hana".to_string()),
            last_name_hint: Some("Satou".to_string()),
            aliases: vec!["Aoki Umi (碧井海)".to_string()],
            ..Character::default()
        },
        "Spaced Name Fixture",
    );

    builder.add_character(
        &Character {
            id: "english-only".to_string(),
            name: "English Only".to_string(),
            name_original: "".to_string(),
            role: "main".to_string(),
            source: "anilist".to_string(),
            ..Character::default()
        },
        "English Only Show",
    );

    builder
}

fn zip_filenames(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>) -> Vec<String> {
    (0..archive.len())
        .map(|idx| archive.by_index(idx).unwrap().name().to_string())
        .collect()
}

fn read_zip_entry(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> String {
    let mut file = archive.by_name(name).unwrap();
    let mut buf = String::new();
    file.read_to_string(&mut buf).unwrap();
    buf
}

fn all_term_entries(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    names: &[String],
) -> Vec<Value> {
    let mut entries = Vec::new();
    for name in names
        .iter()
        .filter(|name| name.starts_with("term_bank_") && name.ends_with(".json"))
    {
        let raw = read_zip_entry(archive, name);
        let mut bank_entries: Vec<Value> = serde_json::from_str(&raw).unwrap();
        entries.append(&mut bank_entries);
    }
    entries
}

fn assert_no_duplicate_terms(entries: &[Value]) {
    let mut seen = HashSet::new();
    for entry in entries {
        let term = entry[0].as_str().unwrap();
        assert!(
            seen.insert(term.to_string()),
            "duplicate term exported: {term}"
        );
    }
}

fn assert_term_entry_field_types(entries: &[Value]) {
    for entry in entries {
        let fields = entry.as_array().expect("term entry must be an array");
        assert_eq!(fields.len(), 8, "term entry must have eight fields");
        assert!(fields[0].is_string(), "term must be a string: {entry:?}");
        assert!(fields[1].is_string(), "reading must be a string: {entry:?}");
        assert!(fields[2].is_string(), "tags must be a string: {entry:?}");
        assert!(fields[3].is_string(), "rules must be a string: {entry:?}");
        assert!(fields[4].is_number(), "score must be numeric: {entry:?}");
        assert!(
            fields[5].is_array(),
            "definitions must be an array: {entry:?}"
        );
        assert!(fields[6].is_number(), "sequence must be numeric: {entry:?}");
        assert!(
            fields[7].is_string(),
            "term tags must be a string: {entry:?}"
        );
    }
}

fn assert_image_paths_resolve(entries: &[Value], zip_names: &[String]) {
    let zip_names: HashSet<&str> = zip_names.iter().map(String::as_str).collect();
    for entry in entries {
        let serialized = serde_json::to_string(&entry[5]).unwrap();
        for image_path in serialized
            .match_indices("img/")
            .map(|(start, _)| &serialized[start..])
        {
            let end = image_path.find('"').unwrap_or(image_path.len());
            let path = &image_path[..end];
            assert!(
                zip_names.contains(path),
                "structured content references missing image: {path}"
            );
        }
    }
}

fn assert_term(entries: &[Value], expected_term: &str, expected_reading: &str) {
    let entry = entries
        .iter()
        .find(|entry| entry[0].as_str() == Some(expected_term))
        .unwrap_or_else(|| panic!("missing term {expected_term}"));
    assert_eq!(entry[1].as_str(), Some(expected_reading));
}
