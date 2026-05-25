use rusqlite::Connection;
use tempfile::tempdir;
use yomitan_dict_builder::snapshot::pipeline::build_snapshot;

#[test]
fn builds_vndb_only_snapshot_fixture() {
    let fixture_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vndb_dump_archive/config.toml");
    let temp = tempdir().unwrap();
    let out_dir = temp.path().join("out");

    let result = build_snapshot(&fixture_config, &out_dir).unwrap();
    assert!(result.sqlite_path.exists());
    assert!(result.parquet_dir.join("entity.parquet").exists());

    let conn = Connection::open(&result.sqlite_path).unwrap();
    let entity_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entity", [], |row| row.get(0))
        .unwrap();
    let relationship_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM relationship", [], |row| row.get(0))
        .unwrap();
    let person_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entity WHERE entity_kind = 'real_person'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(entity_count, 3);
    assert_eq!(relationship_count, 2);
    assert_eq!(person_count, 1);
}
