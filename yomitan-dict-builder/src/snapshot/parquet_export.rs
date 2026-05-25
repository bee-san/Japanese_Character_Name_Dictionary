use anyhow::{Context, Result};
use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use serde::Serialize;
use std::{fs::File, path::Path, sync::Arc};

use super::model::SnapshotArtifacts;

pub fn write_parquet_exports(dir: &Path, artifacts: &SnapshotArtifacts) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    write_json_rows(
        &dir.join("entity.parquet"),
        artifacts
            .entities
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("name_variant.parquet"),
        artifacts
            .name_variants
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("reading.parquet"),
        artifacts
            .readings
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("external_id.parquet"),
        artifacts
            .external_ids
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("relationship.parquet"),
        artifacts
            .relationships
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("source_record.parquet"),
        artifacts
            .source_records
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("source_assertion.parquet"),
        artifacts
            .source_assertions
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("license.parquet"),
        artifacts
            .licenses
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    write_json_rows(
        &dir.join("image_asset.parquet"),
        artifacts
            .image_assets
            .iter()
            .map(|row| (&row.id, row))
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}

fn write_json_rows<T: Serialize>(path: &Path, rows: Vec<(&String, &T)>) -> Result<()> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("row_id", DataType::Utf8, false),
        Field::new("row_json", DataType::Utf8, false),
    ]));

    let ids: Vec<String> = rows.iter().map(|(id, _)| (*id).clone()).collect();
    let json_rows: Vec<String> = rows
        .iter()
        .map(|(_, row)| serde_json::to_string(row).context("failed to serialize row"))
        .collect::<Result<_>>()?;

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(json_rows)) as ArrayRef,
        ],
    )?;

    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut writer = ArrowWriter::try_new(file, schema, None)?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(())
}
