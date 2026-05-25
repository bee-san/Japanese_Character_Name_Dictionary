use rusqlite::Connection;
use serde_json::Value;
use std::io::Read;
use tempfile::tempdir;
use yomitan_dict_builder::{
    content_builder::DictSettings,
    snapshot::pipeline::build_snapshot,
    snapshot_yomitan::{export_yomitan_from_snapshot, SnapshotYomitanOptions},
};
use zip::ZipArchive;

#[test]
fn builds_combined_vndb_and_anilist_snapshot_and_exports_yomitan() {
    let fixture_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/all_names_snapshot/config.toml");
    let temp = tempdir().unwrap();
    let snapshot_out = temp.path().join("snapshot");

    let result = build_snapshot(&fixture_config, &snapshot_out).unwrap();
    assert!(result.sqlite_path.exists());

    let conn = Connection::open(&result.sqlite_path).unwrap();
    let entity_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entity", [], |row| row.get(0))
        .unwrap();
    let character_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entity WHERE entity_kind = 'fictional_character'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let person_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entity WHERE entity_kind = 'real_person'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let relationship_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM relationship", [], |row| row.get(0))
        .unwrap();
    let organization_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entity WHERE entity_kind = 'organization'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let place_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entity WHERE entity_kind = 'place'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(entity_count, 19);
    assert_eq!(character_count, 4);
    assert_eq!(person_count, 8);
    assert_eq!(organization_count, 3);
    assert_eq!(place_count, 1);
    assert_eq!(relationship_count, 7);

    let zip_path = temp.path().join("all-names.zip");
    let export_result = export_yomitan_from_snapshot(
        &snapshot_out,
        &zip_path,
        SnapshotYomitanOptions {
            settings: DictSettings {
                honorifics: false,
                ..DictSettings::default()
            },
            title: Some("All Names Fixture".to_string()),
        },
    )
    .unwrap();
    assert!(export_result.zip_path.exists());
    assert_eq!(export_result.character_source_records, 4);

    let bytes = std::fs::read(&zip_path).unwrap();
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).unwrap();
    let mut term_entries = Vec::new();

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if !file.name().starts_with("term_bank_") {
            continue;
        }

        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        let bank_entries: Vec<Value> = serde_json::from_str(&body).unwrap();
        term_entries.extend(bank_entries);
    }

    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term.contains("牧瀬") && term.contains("紅莉栖"))
        }),
        "expected AniList-only character to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "花澤香菜")
        }),
        "expected JMnedict real-person entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "南島")
        }),
        "expected JMnedict place entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "夏目漱石")
        }),
        "expected Web NDL person entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "日本放送協会")
        }),
        "expected Web NDL organization entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "宮崎駿")
        }),
        "expected Wikidata real-person entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "スタジオジブリ")
        }),
        "expected Wikidata organization entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "吾輩は猫である")
        }),
        "expected Wikipedia work entry to be exported"
    );
    assert!(
        term_entries.iter().any(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term == "ドラえもん")
        }),
        "expected Wikipedia character entry to be exported"
    );

    let okabe_max_appearance_mentions = term_entries
        .iter()
        .filter(|entry| {
            entry
                .get(0)
                .and_then(Value::as_str)
                .is_some_and(|term| term.contains("岡部") && term.contains("倫太郎"))
        })
        .map(|entry| serde_json::to_string(&entry[5]).unwrap())
        .map(|content| content.matches("From: シュタインズ・ゲート").count())
        .max()
        .unwrap_or(0);

    assert!(
        okabe_max_appearance_mentions >= 2,
        "expected merged Okabe entry to keep both VNDB and AniList appearances"
    );
}
