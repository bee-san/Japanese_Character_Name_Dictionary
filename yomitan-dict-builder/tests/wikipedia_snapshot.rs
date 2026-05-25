use rusqlite::Connection;
use tempfile::tempdir;
use yomitan_dict_builder::snapshot::pipeline::build_snapshot;

#[test]
fn builds_wikipedia_snapshot_fixture() {
    let fixture_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wikipedia_snapshot/config.toml");
    let temp = tempdir().unwrap();
    let out_dir = temp.path().join("out");

    let result = build_snapshot(&fixture_config, &out_dir).unwrap();
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
    let alias_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM name_variant WHERE name_type = 'redirect'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(entity_count, 2);
    assert_eq!(character_count, 1);
    assert_eq!(alias_count, 2);
}
