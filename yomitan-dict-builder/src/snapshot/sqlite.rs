use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

use super::model::SnapshotArtifacts;

pub fn write_snapshot(path: &Path, artifacts: &SnapshotArtifacts) -> Result<()> {
    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    create_schema(&conn)?;
    let tx = conn.unchecked_transaction()?;

    for row in &artifacts.staged_rows {
        tx.execute(
            "INSERT INTO staged_source_row (id, source_id, record_key, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![row.id, row.source_id, row.record_key, row.payload_json],
        )?;
    }
    for row in &artifacts.licenses {
        tx.execute(
            "INSERT INTO \"license\" (id, source_id, license_class, label, url, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![row.id, row.source_id, row.license_class, row.label, row.url, row.notes],
        )?;
    }
    for row in &artifacts.source_records {
        tx.execute(
            "INSERT INTO source_record (id, source_id, record_key, record_uri, retrieved_at, payload_json, license_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.id,
                row.source_id,
                row.record_key,
                row.record_uri,
                row.retrieved_at,
                row.payload_json,
                row.license_id
            ],
        )?;
    }
    for row in &artifacts.entities {
        tx.execute(
            "INSERT INTO entity (id, entity_kind, display_name_raw, display_name_normalized, script, domain, context_key, merge_strategy) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.id,
                row.entity_kind,
                row.display_name_raw,
                row.display_name_normalized,
                row.script,
                row.domain,
                row.context_key,
                row.merge_strategy
            ],
        )?;
    }
    for row in &artifacts.name_variants {
        tx.execute(
            "INSERT INTO name_variant (id, entity_id, value_raw, value_normalized, script, locale, name_type, is_primary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.id,
                row.entity_id,
                row.value_raw,
                row.value_normalized,
                row.script,
                row.locale,
                row.name_type,
                row.is_primary
            ],
        )?;
    }
    for row in &artifacts.readings {
        tx.execute(
            "INSERT INTO reading (id, name_variant_id, value_raw, value_normalized, script, reading_type, is_derived) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.id,
                row.name_variant_id,
                row.value_raw,
                row.value_normalized,
                row.script,
                row.reading_type,
                row.is_derived
            ],
        )?;
    }
    for row in &artifacts.external_ids {
        tx.execute(
            "INSERT INTO external_id (id, entity_id, source_name, external_id, external_uri) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row.id,
                row.entity_id,
                row.source_name,
                row.external_id,
                row.external_uri
            ],
        )?;
    }
    for row in &artifacts.relationships {
        tx.execute(
            "INSERT INTO \"relationship\" (id, subject_entity_id, predicate, object_entity_id, context_entity_id, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.id,
                row.subject_entity_id,
                row.predicate,
                row.object_entity_id,
                row.context_entity_id,
                row.confidence
            ],
        )?;
    }
    for row in &artifacts.image_assets {
        tx.execute(
            "INSERT INTO image_asset (id, entity_id, source_record_id, source_id, sha256, ext, width, height, relative_path, rights_status, source_url, local_full_allowed, shareable_allowed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.id,
                row.entity_id,
                row.source_record_id,
                row.source_id,
                row.sha256,
                row.ext,
                row.width.map(i64::from),
                row.height.map(i64::from),
                row.relative_path,
                row.rights_status,
                row.source_url,
                row.local_full_allowed,
                row.shareable_allowed
            ],
        )?;
    }
    for row in &artifacts.source_assertions {
        tx.execute(
            "INSERT INTO source_assertion (id, source_id, source_record_id, target_table, target_row_id, entity_id, field_path, value_json, retrieval_date, license_class, is_derived) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                row.id,
                row.source_id,
                row.source_record_id,
                row.target_table,
                row.target_row_id,
                row.entity_id,
                row.field_path,
                row.value_json,
                row.retrieval_date,
                row.license_class,
                row.is_derived
            ],
        )?;
    }

    tx.commit()?;
    Ok(())
}

pub fn verify_snapshot(path: &Path) -> Result<Vec<String>> {
    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let required_tables = [
        "entity",
        "name_variant",
        "reading",
        "external_id",
        "relationship",
        "source_record",
        "source_assertion",
        "license",
        "image_asset",
    ];

    for table in required_tables {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )?;
        if count != 1 {
            anyhow::bail!("missing required table {table}");
        }
    }

    let tables_to_check = [
        ("entity", "id"),
        ("name_variant", "id"),
        ("reading", "id"),
        ("external_id", "id"),
        ("relationship", "id"),
        ("image_asset", "id"),
    ];
    let mut warnings = Vec::new();

    for (table, id_column) in tables_to_check {
        let sql = format!(
            "SELECT COUNT(*) FROM \"{table}\" t WHERE NOT EXISTS (SELECT 1 FROM source_assertion sa WHERE sa.target_table = ?1 AND sa.target_row_id = t.{id_column})"
        );
        let missing: i64 = conn.query_row(&sql, [table], |row| row.get(0))?;
        if missing > 0 {
            anyhow::bail!("{missing} rows in {table} are missing source assertions");
        }
        let row_count: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })?;
        warnings.push(format!("{table}: {row_count} rows"));
    }

    Ok(warnings)
}

pub fn count_table_rows(path: &Path) -> Result<Vec<(String, i64)>> {
    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let tables = [
        "staged_source_row",
        "license",
        "source_record",
        "entity",
        "name_variant",
        "reading",
        "external_id",
        "relationship",
        "image_asset",
        "source_assertion",
    ];
    tables
        .iter()
        .map(|table| {
            let count =
                conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                    row.get(0)
                })?;
            Ok(((*table).to_string(), count))
        })
        .collect()
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS staged_source_row (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            record_key TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS "license" (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            license_class TEXT NOT NULL,
            label TEXT NOT NULL,
            url TEXT,
            notes TEXT
        );
        CREATE TABLE IF NOT EXISTS source_record (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            record_key TEXT NOT NULL,
            record_uri TEXT,
            retrieved_at TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            license_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS entity (
            id TEXT PRIMARY KEY,
            entity_kind TEXT NOT NULL,
            display_name_raw TEXT NOT NULL,
            display_name_normalized TEXT NOT NULL,
            script TEXT NOT NULL,
            domain TEXT NOT NULL,
            context_key TEXT,
            merge_strategy TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS name_variant (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            value_raw TEXT NOT NULL,
            value_normalized TEXT NOT NULL,
            script TEXT NOT NULL,
            locale TEXT,
            name_type TEXT NOT NULL,
            is_primary INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS reading (
            id TEXT PRIMARY KEY,
            name_variant_id TEXT NOT NULL,
            value_raw TEXT NOT NULL,
            value_normalized TEXT NOT NULL,
            script TEXT NOT NULL,
            reading_type TEXT NOT NULL,
            is_derived INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS external_id (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            source_name TEXT NOT NULL,
            external_id TEXT NOT NULL,
            external_uri TEXT
        );
        CREATE TABLE IF NOT EXISTS "relationship" (
            id TEXT PRIMARY KEY,
            subject_entity_id TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object_entity_id TEXT NOT NULL,
            context_entity_id TEXT,
            confidence REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS image_asset (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            source_record_id TEXT,
            source_id TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            ext TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            relative_path TEXT NOT NULL,
            rights_status TEXT NOT NULL,
            source_url TEXT,
            local_full_allowed INTEGER NOT NULL,
            shareable_allowed INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS source_assertion (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            source_record_id TEXT,
            target_table TEXT NOT NULL,
            target_row_id TEXT NOT NULL,
            entity_id TEXT,
            field_path TEXT NOT NULL,
            value_json TEXT NOT NULL,
            retrieval_date TEXT NOT NULL,
            license_class TEXT NOT NULL,
            is_derived INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_source_assertion_target ON source_assertion(target_table, target_row_id);
        CREATE INDEX IF NOT EXISTS idx_external_id_lookup ON external_id(source_name, external_id);
        "#,
    )?;
    Ok(())
}
