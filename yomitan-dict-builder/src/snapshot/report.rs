use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceManifestEntry {
    pub source_id: String,
    pub kind: String,
    pub input: Option<String>,
    pub record_count: usize,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseManifest {
    pub licenses: Vec<LicenseManifestEntry>,
    pub image_rights_summary: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseManifestEntry {
    pub source_id: String,
    pub license_class: String,
    pub label: String,
    pub url: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageManifestEntry {
    pub image_asset_id: String,
    pub entity_id: String,
    pub sha256: String,
    pub relative_path: String,
    pub rights_status: String,
    pub shareable_allowed: bool,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildReportContext {
    pub built_at: String,
    pub out_dir: String,
    pub row_counts: Vec<(String, i64)>,
    pub enabled_sources: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn write_source_manifest(path: &Path, entries: &[SourceManifestEntry]) -> Result<()> {
    write_json_pretty(path, entries)
}

pub fn write_license_manifest(path: &Path, manifest: &LicenseManifest) -> Result<()> {
    write_json_pretty(path, manifest)
}

pub fn write_image_manifest(path: &Path, entries: &[ImageManifestEntry]) -> Result<()> {
    write_json_pretty(path, entries)
}

pub fn write_build_report(path: &Path, context: &BuildReportContext) -> Result<()> {
    let mut body = String::new();
    body.push_str("# Ultimate Snapshot Build Report\n\n");
    body.push_str(&format!("Built at: {}\n\n", context.built_at));
    body.push_str(&format!("Output directory: `{}`\n\n", context.out_dir));
    body.push_str("## Enabled Sources\n\n");
    if context.enabled_sources.is_empty() {
        body.push_str("- none\n");
    } else {
        for source in &context.enabled_sources {
            body.push_str(&format!("- {}\n", source));
        }
    }
    body.push_str("\n## Row Counts\n\n");
    for (table, count) in &context.row_counts {
        body.push_str(&format!("- {}: {}\n", table, count));
    }
    body.push_str("\n## Warnings\n\n");
    if context.warnings.is_empty() {
        body.push_str("- none\n");
    } else {
        for warning in &context.warnings {
            body.push_str(&format!("- {}\n", warning));
        }
    }
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn write_shareable_report(
    path: &Path,
    built_at: &str,
    shareable_assets: &[ImageManifestEntry],
) -> Result<()> {
    let mut body = String::new();
    body.push_str("# Shareable Export Report\n\n");
    body.push_str(&format!("Built at: {}\n\n", built_at));
    body.push_str(&format!(
        "Shareable image assets: {}\n\n",
        shareable_assets.len()
    ));
    for asset in shareable_assets {
        body.push_str(&format!(
            "- `{}` (`{}`) {}\n",
            asset.image_asset_id, asset.entity_id, asset.relative_path
        ));
    }
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn write_json_pretty<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
