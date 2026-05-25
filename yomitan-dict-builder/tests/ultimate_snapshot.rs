use tempfile::tempdir;
use yomitan_dict_builder::snapshot::pipeline::{build_snapshot, verify_output_dir};

#[test]
fn builds_and_verifies_snapshot_fixture() {
    let fixture_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ultimate_snapshot/config.toml");
    let temp = tempdir().unwrap();
    let out_dir = temp.path().join("out");

    let result = build_snapshot(&fixture_config, &out_dir).unwrap();
    assert!(result.sqlite_path.exists());
    assert!(result.parquet_dir.join("entity.parquet").exists());
    assert!(result.image_store_dir.exists());
    assert!(result.source_manifest_path.exists());
    assert!(result.license_manifest_path.exists());
    assert!(result.local_full_image_manifest_path.exists());
    assert!(result.shareable_image_manifest_path.exists());
    assert!(result.build_report_path.exists());
    assert!(result.shareable_report_path.exists());

    let checks = verify_output_dir(&out_dir).unwrap();
    assert!(checks.iter().any(|line| line.contains("entity")));
}
