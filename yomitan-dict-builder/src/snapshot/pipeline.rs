use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use image::GenericImageView;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    config::{ImagePolicyConfig, SnapshotConfig, SourceConfig},
    model::{
        EntityRecord, ExternalIdRecord, ImageAssetRecord, LicenseRecord, NameVariantRecord,
        ReadingRecord, RelationshipRecord, RightsStatus, ScriptKind, SnapshotArtifacts,
        SourceAssertionRecord, SourceRecord, StagedSourceRow,
    },
    normalize::{derive_romaji_from_reading, normalize_name, normalize_reading, NormalizedText},
    parquet_export::write_parquet_exports,
    report::{
        write_build_report, write_image_manifest, write_license_manifest, write_shareable_report,
        write_source_manifest, BuildReportContext, ImageManifestEntry, LicenseManifest,
        LicenseManifestEntry, SourceManifestEntry,
    },
    source::{
        enabled_source_ids, load_raw_records, source_policy, RawImage, RawNameValue, RawReading,
        RawSourceRecord,
    },
    sqlite::{count_table_rows, verify_snapshot, write_snapshot},
};

pub struct BuildResult {
    pub sqlite_path: PathBuf,
    pub parquet_dir: PathBuf,
    pub image_store_dir: PathBuf,
    pub source_manifest_path: PathBuf,
    pub license_manifest_path: PathBuf,
    pub local_full_image_manifest_path: PathBuf,
    pub shareable_image_manifest_path: PathBuf,
    pub build_report_path: PathBuf,
    pub shareable_report_path: PathBuf,
    pub warnings: Vec<String>,
    pub row_counts: Vec<(String, i64)>,
}

#[derive(Clone)]
struct CandidateRecord {
    source_id: String,
    source_record_id: String,
    license_class: String,
    input_base_dir: PathBuf,
    raw: RawSourceRecord,
    primary_name: NormalizedText,
    aliases: Vec<(RawNameValue, NormalizedText)>,
    readings: Vec<(RawReading, NormalizedText)>,
    external_key_set: BTreeSet<String>,
}

pub fn build_snapshot(config_path: &Path, out_dir: &Path) -> Result<BuildResult> {
    let config = SnapshotConfig::load(config_path)?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    let built_at = Utc::now().to_rfc3339();
    let retrieval_date = Utc::now().format("%Y-%m-%d").to_string();
    let config_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let enabled_sources: Vec<String> = enabled_source_ids(&config.sources).into_iter().collect();
    if enabled_sources.is_empty() && config.build.fail_on_no_enabled_sources {
        bail!("no sources are enabled in {}", config_path.display());
    }

    let mut artifacts = SnapshotArtifacts::new();
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    let mut source_manifest = Vec::new();

    for (source_id, source_cfg) in &config.sources {
        if !source_cfg.enabled {
            continue;
        }
        load_source_candidates(
            source_id,
            source_cfg,
            &config,
            &config_dir,
            &retrieval_date,
            &mut artifacts,
            &mut candidates,
            &mut warnings,
            &mut source_manifest,
        )?;
    }

    let resolution = resolve_candidates(
        &candidates,
        &config.image_policy,
        &retrieval_date,
        out_dir,
        &mut artifacts,
        &mut warnings,
    )?;

    let sqlite_path = out_dir.join("snapshot.sqlite");
    write_snapshot(&sqlite_path, &artifacts)?;

    let parquet_dir = out_dir.join("parquet");
    write_parquet_exports(&parquet_dir, &artifacts)?;

    let source_manifest_path = out_dir.join("source_manifest.json");
    write_source_manifest(&source_manifest_path, &source_manifest)?;

    let license_manifest = LicenseManifest {
        licenses: artifacts
            .licenses
            .iter()
            .map(|license| LicenseManifestEntry {
                source_id: license.source_id.clone(),
                license_class: license.license_class.clone(),
                label: license.label.clone(),
                url: license.url.clone(),
                notes: license.notes.clone(),
            })
            .collect(),
        image_rights_summary: summarize_image_rights(&artifacts.image_assets),
    };
    let license_manifest_path = out_dir.join("license_manifest.json");
    write_license_manifest(&license_manifest_path, &license_manifest)?;

    let local_full_image_manifest_path = out_dir.join("local_full_image_manifest.json");
    write_image_manifest(
        &local_full_image_manifest_path,
        &resolution.local_full_image_manifest,
    )?;

    let shareable_image_manifest_path = out_dir.join("shareable_image_manifest.json");
    write_image_manifest(
        &shareable_image_manifest_path,
        &resolution.shareable_image_manifest,
    )?;

    let row_counts = count_table_rows(&sqlite_path)?;
    let build_report_path = out_dir.join("build_report.md");
    write_build_report(
        &build_report_path,
        &BuildReportContext {
            built_at: built_at.clone(),
            out_dir: out_dir.display().to_string(),
            row_counts: row_counts.clone(),
            enabled_sources: enabled_sources.clone(),
            warnings: warnings.clone(),
        },
    )?;

    let shareable_report_path = out_dir.join("shareable_export_report.md");
    write_shareable_report(
        &shareable_report_path,
        &built_at,
        &resolution.shareable_image_manifest,
    )?;

    Ok(BuildResult {
        sqlite_path,
        parquet_dir,
        image_store_dir: out_dir.join(&config.image_policy.image_store_subdir),
        source_manifest_path,
        license_manifest_path,
        local_full_image_manifest_path,
        shareable_image_manifest_path,
        build_report_path,
        shareable_report_path,
        warnings,
        row_counts,
    })
}

pub fn verify_output_dir(out_dir: &Path) -> Result<Vec<String>> {
    let required_files = [
        out_dir.join("snapshot.sqlite"),
        out_dir.join("source_manifest.json"),
        out_dir.join("license_manifest.json"),
        out_dir.join("local_full_image_manifest.json"),
        out_dir.join("shareable_image_manifest.json"),
        out_dir.join("build_report.md"),
        out_dir.join("shareable_export_report.md"),
    ];
    for path in required_files {
        if !path.exists() {
            bail!("missing required artifact {}", path.display());
        }
    }
    verify_snapshot(&out_dir.join("snapshot.sqlite"))
}

pub fn regenerate_reports(out_dir: &Path) -> Result<()> {
    let source_manifest: Vec<SourceManifestEntry> = serde_json::from_str(
        &fs::read_to_string(out_dir.join("source_manifest.json")).with_context(|| {
            format!(
                "failed to read {}",
                out_dir.join("source_manifest.json").display()
            )
        })?,
    )?;
    let shareable_image_manifest: Vec<ImageManifestEntry> = serde_json::from_str(
        &fs::read_to_string(out_dir.join("shareable_image_manifest.json")).with_context(|| {
            format!(
                "failed to read {}",
                out_dir.join("shareable_image_manifest.json").display()
            )
        })?,
    )?;
    let warnings = verify_output_dir(out_dir)?;
    let row_counts = count_table_rows(&out_dir.join("snapshot.sqlite"))?;
    write_build_report(
        &out_dir.join("build_report.md"),
        &BuildReportContext {
            built_at: Utc::now().to_rfc3339(),
            out_dir: out_dir.display().to_string(),
            row_counts,
            enabled_sources: source_manifest
                .into_iter()
                .map(|entry| entry.source_id)
                .collect(),
            warnings,
        },
    )?;
    write_shareable_report(
        &out_dir.join("shareable_export_report.md"),
        &Utc::now().to_rfc3339(),
        &shareable_image_manifest,
    )?;
    Ok(())
}

struct ResolutionResult {
    local_full_image_manifest: Vec<ImageManifestEntry>,
    shareable_image_manifest: Vec<ImageManifestEntry>,
}

fn load_source_candidates(
    source_id: &str,
    source_cfg: &SourceConfig,
    config: &SnapshotConfig,
    config_dir: &Path,
    retrieval_date: &str,
    artifacts: &mut SnapshotArtifacts,
    candidates: &mut Vec<CandidateRecord>,
    warnings: &mut Vec<String>,
    source_manifest: &mut Vec<SourceManifestEntry>,
) -> Result<()> {
    let policy = source_policy(&source_cfg.kind)?;
    if let Some(reason) = policy.disabled_reason {
        bail!("source {} is disabled: {}", source_id, reason);
    }

    let input_path = source_cfg
        .input
        .as_ref()
        .map(|input| config_dir.join(input))
        .ok_or_else(|| anyhow!("source {} is missing an input path", source_id))?;

    if !input_path.exists() {
        if config.build.fail_on_missing_input {
            bail!(
                "source {} input does not exist: {}",
                source_id,
                input_path.display()
            );
        }
        warnings.push(format!(
            "source {} skipped because {} does not exist",
            source_id,
            input_path.display()
        ));
        return Ok(());
    }

    let records = load_raw_records(&input_path, source_cfg.format.as_deref(), &source_cfg.kind)?;
    let license_id = stable_id("license", &[source_id, &source_cfg.kind]);
    artifacts.licenses.push(LicenseRecord {
        id: license_id.clone(),
        source_id: source_id.to_string(),
        license_class: source_cfg
            .license_class
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        label: source_cfg
            .license_label
            .clone()
            .unwrap_or_else(|| format!("{} license", source_id)),
        url: source_cfg.license_url.clone(),
        notes: source_cfg.notes.clone(),
    });

    source_manifest.push(SourceManifestEntry {
        source_id: source_id.to_string(),
        kind: source_cfg.kind.clone(),
        input: source_cfg.input.clone(),
        record_count: records.len(),
        notes: source_cfg.notes.clone(),
    });

    for raw in records {
        if let Err(err) = policy.validate_record(&raw) {
            if config.build.fail_on_policy_error {
                return Err(err);
            }
            warnings.push(format!(
                "source {} record {} skipped: {}",
                source_id, raw.record_id, err
            ));
            continue;
        }

        let payload_json = serde_json::to_string(&raw)?;
        artifacts.staged_rows.push(StagedSourceRow {
            id: stable_id("staged", &[source_id, &raw.record_id]),
            source_id: source_id.to_string(),
            record_key: raw.record_id.clone(),
            payload_json: payload_json.clone(),
        });

        let retrieved_at = raw
            .retrieved_at
            .clone()
            .unwrap_or_else(|| retrieval_date.to_string());
        let source_record_id = stable_id("source_record", &[source_id, &raw.record_id]);
        artifacts.source_records.push(SourceRecord {
            id: source_record_id.clone(),
            source_id: source_id.to_string(),
            record_key: raw.record_id.clone(),
            record_uri: raw.record_uri.clone(),
            retrieved_at: retrieved_at.clone(),
            payload_json,
            license_id: license_id.clone(),
        });

        let primary_name = normalize_name(&raw.primary_name.value);
        let aliases = raw
            .aliases
            .iter()
            .cloned()
            .map(|alias| {
                let normalized = normalize_name(&alias.value);
                (alias, normalized)
            })
            .collect();
        let readings = raw
            .readings
            .iter()
            .cloned()
            .map(|reading| {
                let normalized = normalize_reading(&reading.value);
                (reading, normalized)
            })
            .collect();
        let external_key_set = raw
            .external_ids
            .iter()
            .map(|external_id| external_key(&external_id.source_name, &external_id.value))
            .collect();

        candidates.push(CandidateRecord {
            source_id: source_id.to_string(),
            source_record_id,
            license_class: source_cfg
                .license_class
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            input_base_dir: input_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| config_dir.to_path_buf()),
            raw,
            primary_name,
            aliases,
            readings,
            external_key_set,
        });
    }

    Ok(())
}

fn resolve_candidates(
    candidates: &[CandidateRecord],
    image_policy: &ImagePolicyConfig,
    retrieval_date: &str,
    out_dir: &Path,
    artifacts: &mut SnapshotArtifacts,
    warnings: &mut Vec<String>,
) -> Result<ResolutionResult> {
    let mut dsu = UnionFind::new(candidates.len());
    let mut external_map = HashMap::new();
    let mut japanese_name_map = HashMap::new();
    let mut reading_map = HashMap::new();

    for (idx, candidate) in candidates.iter().enumerate() {
        for external in &candidate.external_key_set {
            if let Some(previous) = external_map.insert(external.clone(), idx) {
                dsu.union(previous, idx);
            }
        }

        if candidate.primary_name.script.is_japanese() {
            let key = format!(
                "{}|{}|{}|{}",
                candidate.raw.entity_kind.as_str(),
                candidate.raw.domain,
                candidate.raw.context_key.clone().unwrap_or_default(),
                candidate.primary_name.normalized
            );
            if let Some(previous) = japanese_name_map.insert(key, idx) {
                dsu.union(previous, idx);
            }
        }

        if let Some(reading) = primary_reading(candidate) {
            let key = format!(
                "{}|{}|{}|{}|{}",
                candidate.raw.entity_kind.as_str(),
                candidate.raw.domain,
                candidate.raw.context_key.clone().unwrap_or_default(),
                candidate.primary_name.normalized,
                reading
            );
            if let Some(previous) = reading_map.get(&key).copied() {
                if can_probabilistically_merge(candidate, &candidates[previous]) {
                    dsu.union(previous, idx);
                }
            } else {
                reading_map.insert(key, idx);
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for idx in 0..candidates.len() {
        groups.entry(dsu.find(idx)).or_default().push(idx);
    }

    let mut candidate_to_entity = HashMap::new();
    let mut entity_external_lookup = HashMap::new();
    let mut local_full_image_manifest = Vec::new();
    let mut shareable_image_manifest = Vec::new();
    let image_http_client = if image_policy.fetch_remote {
        Some(
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .context("failed to build blocking image HTTP client")?,
        )
    } else {
        None
    };

    for group in groups.values_mut() {
        group.sort_by_key(|idx| {
            format!(
                "{}:{}",
                candidates[*idx].source_id, candidates[*idx].raw.record_id
            )
        });
        let canonical_idx = choose_canonical_candidate(group, candidates);
        let canonical = &candidates[canonical_idx];
        let entity_id = stable_id(
            "entity",
            &group
                .iter()
                .map(|idx| {
                    format!(
                        "{}:{}",
                        candidates[*idx].source_id, candidates[*idx].raw.record_id
                    )
                })
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let merge_strategy = determine_merge_strategy(group, candidates);

        artifacts.entities.push(EntityRecord {
            id: entity_id.clone(),
            entity_kind: canonical.raw.entity_kind.as_str().to_string(),
            display_name_raw: canonical.primary_name.raw.clone(),
            display_name_normalized: canonical.primary_name.normalized.clone(),
            script: canonical.primary_name.script.as_str().to_string(),
            domain: canonical.raw.domain.clone(),
            context_key: canonical.raw.context_key.clone(),
            merge_strategy: merge_strategy.to_string(),
        });
        push_assertion(
            artifacts,
            &canonical.source_id,
            Some(&canonical.source_record_id),
            "entity",
            &entity_id,
            Some(&entity_id),
            "entity.display_name_raw",
            &canonical.primary_name.raw,
            retrieval_date,
            &canonical.license_class,
            false,
        )?;
        push_assertion(
            artifacts,
            &canonical.source_id,
            Some(&canonical.source_record_id),
            "entity",
            &entity_id,
            Some(&entity_id),
            "entity.display_name_normalized",
            &canonical.primary_name.normalized,
            retrieval_date,
            &canonical.license_class,
            true,
        )?;

        let mut name_variant_lookup = HashMap::new();
        let mut name_variant_by_normalized = HashMap::new();
        let mut reading_dedupe = HashSet::new();
        let mut external_dedupe = HashSet::new();
        let mut image_dedupe = HashSet::new();

        for idx in group.iter().copied() {
            let candidate = &candidates[idx];
            candidate_to_entity.insert(idx, entity_id.clone());

            let primary_variant_id = upsert_name_variant(
                artifacts,
                &mut name_variant_lookup,
                &mut name_variant_by_normalized,
                &entity_id,
                candidate,
                &candidate.raw.primary_name,
                &candidate.primary_name,
                true,
                "primary",
                retrieval_date,
            )?;

            for (alias, normalized) in &candidate.aliases {
                upsert_name_variant(
                    artifacts,
                    &mut name_variant_lookup,
                    &mut name_variant_by_normalized,
                    &entity_id,
                    candidate,
                    alias,
                    normalized,
                    false,
                    alias.name_type.as_deref().unwrap_or("alias"),
                    retrieval_date,
                )?;
            }

            for external_id in &candidate.raw.external_ids {
                let key = external_key(&external_id.source_name, &external_id.value);
                if external_dedupe.insert(key.clone()) {
                    let row_id = stable_id("external_id", &[&entity_id, &key]);
                    artifacts.external_ids.push(ExternalIdRecord {
                        id: row_id.clone(),
                        entity_id: entity_id.clone(),
                        source_name: external_id.source_name.clone(),
                        external_id: external_id.value.clone(),
                        external_uri: external_id.uri.clone(),
                    });
                    entity_external_lookup.insert(key, entity_id.clone());
                    push_assertion(
                        artifacts,
                        &candidate.source_id,
                        Some(&candidate.source_record_id),
                        "external_id",
                        &row_id,
                        Some(&entity_id),
                        "external_id.external_id",
                        &external_id.value,
                        retrieval_date,
                        &candidate.license_class,
                        false,
                    )?;
                }
            }

            for (reading, normalized) in &candidate.readings {
                let target_name = reading
                    .for_name
                    .as_ref()
                    .map(|value| normalize_name(value).normalized)
                    .unwrap_or_else(|| candidate.primary_name.normalized.clone());
                let name_variant_id = name_variant_by_normalized
                    .get(&target_name)
                    .cloned()
                    .unwrap_or_else(|| primary_variant_id.clone());
                let key = format!(
                    "{}|{}|{}",
                    name_variant_id,
                    normalized.normalized,
                    reading.reading_type.as_deref().unwrap_or("source")
                );
                if reading_dedupe.insert(key) {
                    let row_id = stable_id(
                        "reading",
                        &[
                            &name_variant_id,
                            &normalized.normalized,
                            reading.reading_type.as_deref().unwrap_or("source"),
                        ],
                    );
                    artifacts.readings.push(ReadingRecord {
                        id: row_id.clone(),
                        name_variant_id: name_variant_id.clone(),
                        value_raw: normalized.raw.clone(),
                        value_normalized: normalized.normalized.clone(),
                        script: normalized.script.as_str().to_string(),
                        reading_type: reading
                            .reading_type
                            .clone()
                            .unwrap_or_else(|| "source".to_string()),
                        is_derived: false,
                    });
                    push_assertion(
                        artifacts,
                        &candidate.source_id,
                        Some(&candidate.source_record_id),
                        "reading",
                        &row_id,
                        Some(&entity_id),
                        "reading.value_raw",
                        &normalized.raw,
                        retrieval_date,
                        &candidate.license_class,
                        false,
                    )?;

                    if let Some(romaji) = derive_romaji_from_reading(&normalized.normalized) {
                        let derived_key = format!("{}|{}|derived_romaji", name_variant_id, romaji);
                        if reading_dedupe.insert(derived_key) {
                            let derived_id = stable_id(
                                "reading",
                                &[&name_variant_id, &romaji, "derived_romaji"],
                            );
                            artifacts.readings.push(ReadingRecord {
                                id: derived_id.clone(),
                                name_variant_id: name_variant_id.clone(),
                                value_raw: romaji.clone(),
                                value_normalized: romaji.clone(),
                                script: ScriptKind::Latin.as_str().to_string(),
                                reading_type: "derived_romaji".to_string(),
                                is_derived: true,
                            });
                            push_assertion(
                                artifacts,
                                &candidate.source_id,
                                Some(&candidate.source_record_id),
                                "reading",
                                &derived_id,
                                Some(&entity_id),
                                "reading.value_normalized",
                                &romaji,
                                retrieval_date,
                                &candidate.license_class,
                                true,
                            )?;
                        }
                    }
                }
            }

            if matches!(
                candidate.primary_name.script,
                ScriptKind::Hiragana | ScriptKind::Katakana
            ) {
                let key = format!(
                    "{}|{}|self_kana",
                    primary_variant_id, candidate.primary_name.normalized
                );
                if reading_dedupe.insert(key) {
                    let row_id = stable_id(
                        "reading",
                        &[
                            &primary_variant_id,
                            &candidate.primary_name.normalized,
                            "self_kana",
                        ],
                    );
                    artifacts.readings.push(ReadingRecord {
                        id: row_id.clone(),
                        name_variant_id: primary_variant_id.clone(),
                        value_raw: candidate.primary_name.raw.clone(),
                        value_normalized: candidate.primary_name.normalized.clone(),
                        script: ScriptKind::Hiragana.as_str().to_string(),
                        reading_type: "self_kana".to_string(),
                        is_derived: true,
                    });
                    push_assertion(
                        artifacts,
                        &candidate.source_id,
                        Some(&candidate.source_record_id),
                        "reading",
                        &row_id,
                        Some(&entity_id),
                        "reading.value_normalized",
                        &candidate.primary_name.normalized,
                        retrieval_date,
                        &candidate.license_class,
                        true,
                    )?;
                }
            }

            for image in &candidate.raw.images {
                if let Some((
                    sha256,
                    ext,
                    relative_path,
                    width,
                    height,
                    rights_status,
                    source_url,
                )) = mirror_image(
                    image,
                    candidate,
                    &entity_id,
                    image_policy,
                    image_http_client.as_ref(),
                    out_dir,
                    warnings,
                )? {
                    let dedupe_key = format!("{}|{}", entity_id, sha256);
                    if image_dedupe.insert(dedupe_key) {
                        let image_id = stable_id("image_asset", &[&entity_id, &sha256]);
                        let shareable = rights_status.shareable();
                        artifacts.image_assets.push(ImageAssetRecord {
                            id: image_id.clone(),
                            entity_id: entity_id.clone(),
                            source_record_id: Some(candidate.source_record_id.clone()),
                            source_id: candidate.source_id.clone(),
                            sha256: sha256.clone(),
                            ext,
                            width,
                            height,
                            relative_path: relative_path.clone(),
                            rights_status: rights_status.as_str().to_string(),
                            source_url: source_url.clone(),
                            local_full_allowed: image_policy.mirror_locally,
                            shareable_allowed: shareable,
                        });
                        push_assertion(
                            artifacts,
                            &candidate.source_id,
                            Some(&candidate.source_record_id),
                            "image_asset",
                            &image_id,
                            Some(&entity_id),
                            "image_asset.sha256",
                            &sha256,
                            retrieval_date,
                            &candidate.license_class,
                            true,
                        )?;
                        let manifest_entry = ImageManifestEntry {
                            image_asset_id: image_id,
                            entity_id: entity_id.clone(),
                            sha256,
                            relative_path,
                            rights_status: rights_status.as_str().to_string(),
                            shareable_allowed: shareable,
                            source_url,
                        };
                        local_full_image_manifest.push(manifest_entry.clone());
                        if shareable {
                            shareable_image_manifest.push(manifest_entry);
                        }
                    }
                }
            }
        }
    }

    let mut relationship_dedupe = HashSet::new();
    for (idx, candidate) in candidates.iter().enumerate() {
        let Some(subject_entity_id) = candidate_to_entity.get(&idx) else {
            continue;
        };
        for relationship in &candidate.raw.relationships {
            let target_key = external_key(
                &relationship.target_source_name,
                &relationship.target_external_id,
            );
            let Some(object_entity_id) = entity_external_lookup.get(&target_key) else {
                warnings.push(format!(
                    "relationship {} from {}:{} could not resolve target {}",
                    relationship.predicate,
                    candidate.source_id,
                    candidate.raw.record_id,
                    target_key
                ));
                continue;
            };
            let context_entity_id = relationship
                .context_source_name
                .as_ref()
                .zip(relationship.context_external_id.as_ref())
                .and_then(|(source_name, external_id)| {
                    entity_external_lookup
                        .get(&external_key(source_name, external_id))
                        .cloned()
                });
            let key = format!(
                "{}|{}|{}|{}",
                subject_entity_id,
                relationship.predicate,
                object_entity_id,
                context_entity_id.clone().unwrap_or_default()
            );
            if relationship_dedupe.insert(key) {
                let row_id = stable_id(
                    "relationship",
                    &[
                        subject_entity_id,
                        &relationship.predicate,
                        object_entity_id,
                        context_entity_id.as_deref().unwrap_or(""),
                    ],
                );
                artifacts.relationships.push(RelationshipRecord {
                    id: row_id.clone(),
                    subject_entity_id: subject_entity_id.clone(),
                    predicate: relationship.predicate.clone(),
                    object_entity_id: object_entity_id.clone(),
                    context_entity_id: context_entity_id.clone(),
                    confidence: relationship.confidence.unwrap_or(1.0),
                });
                push_assertion(
                    artifacts,
                    &candidate.source_id,
                    Some(&candidate.source_record_id),
                    "relationship",
                    &row_id,
                    Some(subject_entity_id),
                    "relationship.predicate",
                    &relationship.predicate,
                    retrieval_date,
                    &candidate.license_class,
                    false,
                )?;
            }
        }
    }

    Ok(ResolutionResult {
        local_full_image_manifest,
        shareable_image_manifest,
    })
}

fn upsert_name_variant(
    artifacts: &mut SnapshotArtifacts,
    name_variant_lookup: &mut HashMap<String, String>,
    name_variant_by_normalized: &mut HashMap<String, String>,
    entity_id: &str,
    candidate: &CandidateRecord,
    raw_name: &RawNameValue,
    normalized: &NormalizedText,
    is_primary: bool,
    fallback_name_type: &str,
    retrieval_date: &str,
) -> Result<String> {
    let key = format!(
        "{}|{}|{}|{}",
        entity_id,
        normalized.normalized,
        raw_name.locale.clone().unwrap_or_default(),
        raw_name
            .name_type
            .clone()
            .unwrap_or_else(|| fallback_name_type.to_string())
    );
    if let Some(existing) = name_variant_lookup.get(&key) {
        return Ok(existing.clone());
    }

    let row_id = stable_id("name_variant", &[entity_id, &normalized.normalized, &key]);
    artifacts.name_variants.push(NameVariantRecord {
        id: row_id.clone(),
        entity_id: entity_id.to_string(),
        value_raw: normalized.raw.clone(),
        value_normalized: normalized.normalized.clone(),
        script: normalized.script.as_str().to_string(),
        locale: raw_name.locale.clone(),
        name_type: raw_name
            .name_type
            .clone()
            .unwrap_or_else(|| fallback_name_type.to_string()),
        is_primary,
    });
    name_variant_lookup.insert(key, row_id.clone());
    name_variant_by_normalized
        .entry(normalized.normalized.clone())
        .or_insert_with(|| row_id.clone());

    push_assertion(
        artifacts,
        &candidate.source_id,
        Some(&candidate.source_record_id),
        "name_variant",
        &row_id,
        Some(entity_id),
        "name_variant.value_raw",
        &normalized.raw,
        retrieval_date,
        &candidate.license_class,
        false,
    )?;
    Ok(row_id)
}

fn mirror_image(
    image: &RawImage,
    candidate: &CandidateRecord,
    entity_id: &str,
    image_policy: &ImagePolicyConfig,
    image_http_client: Option<&reqwest::blocking::Client>,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<
    Option<(
        String,
        String,
        String,
        Option<u32>,
        Option<u32>,
        RightsStatus,
        Option<String>,
    )>,
> {
    let Some(bytes) = load_image_bytes(image, candidate, image_http_client)? else {
        let intentionally_skipped_remote_url = image.bytes_base64.is_none()
            && image.local_path.is_none()
            && image.url.is_some()
            && !image_policy.fetch_remote;
        if !intentionally_skipped_remote_url {
            warnings.push(format!(
                "image for entity {} from {}:{} could not be mirrored because no bytes were provided",
                entity_id, candidate.source_id, candidate.raw.record_id
            ));
        }
        return Ok(None);
    };
    let sha256 = sha256_hex(&bytes);
    let ext = infer_image_ext(image).unwrap_or_else(|| "bin".to_string());
    let relative_path = format!(
        "{}/{}/{}.{}",
        image_policy.image_store_subdir,
        &sha256[..2],
        sha256,
        ext
    );
    let full_path = out_dir.join(&relative_path);
    if image_policy.mirror_locally {
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if !full_path.exists() {
            fs::write(&full_path, &bytes)
                .with_context(|| format!("failed to write {}", full_path.display()))?;
        }
    }
    let (width, height) = if image.width.is_some() || image.height.is_some() {
        (image.width, image.height)
    } else {
        image::load_from_memory(&bytes)
            .map(|decoded| {
                let (width, height) = decoded.dimensions();
                (Some(width), Some(height))
            })
            .unwrap_or((None, None))
    };
    Ok(Some((
        sha256,
        ext,
        relative_path,
        width,
        height,
        image.rights_status.unwrap_or(RightsStatus::Unknown),
        image.url.clone(),
    )))
}

fn load_image_bytes(
    image: &RawImage,
    candidate: &CandidateRecord,
    image_http_client: Option<&reqwest::blocking::Client>,
) -> Result<Option<Vec<u8>>> {
    if let Some(bytes_base64) = &image.bytes_base64 {
        let bytes = STANDARD
            .decode(bytes_base64)
            .context("invalid image bytes_base64 payload")?;
        return Ok(Some(bytes));
    }
    if let Some(local_path) = &image.local_path {
        let full_path = if Path::new(local_path).is_absolute() {
            PathBuf::from(local_path)
        } else {
            candidate.input_base_dir.join(local_path)
        };
        let bytes = fs::read(&full_path)
            .with_context(|| format!("failed to read image file {}", full_path.display()))?;
        return Ok(Some(bytes));
    }
    if let (Some(url), Some(client)) = (&image.url, image_http_client) {
        let response = client
            .get(url)
            .send()
            .with_context(|| format!("failed to fetch image URL {url}"))?;
        if !response.status().is_success() {
            bail!("image URL {url} returned {}", response.status());
        }
        let bytes = response
            .bytes()
            .with_context(|| format!("failed to read image body from {url}"))?;
        return Ok(Some(bytes.to_vec()));
    }
    Ok(None)
}

fn infer_image_ext(image: &RawImage) -> Option<String> {
    image
        .ext
        .clone()
        .or_else(|| {
            image
                .local_path
                .as_ref()
                .and_then(|path| Path::new(path).extension().and_then(|ext| ext.to_str()))
                .map(str::to_string)
        })
        .or_else(|| {
            image.url.as_ref().and_then(|url| {
                Path::new(url.split('?').next().unwrap_or(url))
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_string)
            })
        })
}

fn determine_merge_strategy(group: &[usize], candidates: &[CandidateRecord]) -> &'static str {
    if group.len() <= 1 {
        return "singleton";
    }
    let total_external_ids: usize = group
        .iter()
        .map(|idx| candidates[*idx].external_key_set.len())
        .sum();
    let unique_external_ids: HashSet<String> = group
        .iter()
        .flat_map(|idx| candidates[*idx].external_key_set.iter().cloned())
        .collect();
    if unique_external_ids.len() < total_external_ids {
        return "external_id";
    }
    let japanese_name_count: HashSet<String> = group
        .iter()
        .filter_map(|idx| {
            let candidate = &candidates[*idx];
            candidate
                .primary_name
                .script
                .is_japanese()
                .then(|| candidate.primary_name.normalized.clone())
        })
        .collect();
    if japanese_name_count.len() == 1 {
        return "exact_japanese_name";
    }
    "conservative_probabilistic"
}

fn choose_canonical_candidate(group: &[usize], candidates: &[CandidateRecord]) -> usize {
    group
        .iter()
        .copied()
        .max_by_key(|idx| {
            let candidate = &candidates[*idx];
            (
                candidate.primary_name.script.is_japanese(),
                candidate.primary_name.raw.len(),
                std::cmp::Reverse(candidate.raw.record_id.clone()),
            )
        })
        .unwrap_or(group[0])
}

fn can_probabilistically_merge(left: &CandidateRecord, right: &CandidateRecord) -> bool {
    left.raw.entity_kind == right.raw.entity_kind
        && left.raw.domain == right.raw.domain
        && left.raw.context_key == right.raw.context_key
        && (left.external_key_set.is_empty()
            || right.external_key_set.is_empty()
            || !left.external_key_set.is_disjoint(&right.external_key_set))
}

fn primary_reading(candidate: &CandidateRecord) -> Option<String> {
    candidate
        .readings
        .first()
        .map(|(_, normalized)| normalized.normalized.clone())
        .or_else(|| {
            matches!(
                candidate.primary_name.script,
                ScriptKind::Hiragana | ScriptKind::Katakana
            )
            .then(|| candidate.primary_name.normalized.clone())
        })
}

fn external_key(source_name: &str, value: &str) -> String {
    format!("{}:{}", source_name, value)
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    for part in parts {
        hasher.update([0xff]);
        hasher.update(part.as_bytes());
    }
    format!("{}_{}", prefix, sha256_from_digest(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    sha256_from_digest(hasher.finalize())
}

fn sha256_from_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn summarize_image_rights(image_assets: &[ImageAssetRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for asset in image_assets {
        *counts.entry(asset.rights_status.clone()).or_insert(0) += 1;
    }
    counts
}

fn push_assertion<T: Serialize>(
    artifacts: &mut SnapshotArtifacts,
    source_id: &str,
    source_record_id: Option<&str>,
    target_table: &str,
    target_row_id: &str,
    entity_id: Option<&str>,
    field_path: &str,
    value: &T,
    retrieval_date: &str,
    license_class: &str,
    is_derived: bool,
) -> Result<()> {
    artifacts.source_assertions.push(SourceAssertionRecord {
        id: stable_id(
            "source_assertion",
            &[
                source_id,
                source_record_id.unwrap_or(""),
                target_table,
                target_row_id,
                field_path,
            ],
        ),
        source_id: source_id.to_string(),
        source_record_id: source_record_id.map(str::to_string),
        target_table: target_table.to_string(),
        target_row_id: target_row_id.to_string(),
        entity_id: entity_id.map(str::to_string),
        field_path: field_path.to_string(),
        value_json: serde_json::to_string(value)?,
        retrieval_date: retrieval_date.to_string(),
        license_class: license_class.to_string(),
        is_derived,
    });
    Ok(())
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, idx: usize) -> usize {
        if self.parent[idx] != idx {
            let root = self.find(self.parent[idx]);
            self.parent[idx] = root;
        }
        self.parent[idx]
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root == right_root {
            return;
        }
        if self.rank[left_root] < self.rank[right_root] {
            self.parent[left_root] = right_root;
        } else if self.rank[left_root] > self.rank[right_root] {
            self.parent[right_root] = left_root;
        } else {
            self.parent[right_root] = left_root;
            self.rank[left_root] += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::model::EntityKind;
    use crate::snapshot::source::{RawExternalId, RawSourceRecord};

    fn candidate(
        source_id: &str,
        record_id: &str,
        primary_name: &str,
        entity_kind: EntityKind,
        domain: &str,
        context_key: Option<&str>,
        external_ids: Vec<(&str, &str)>,
    ) -> CandidateRecord {
        let raw = RawSourceRecord {
            record_id: record_id.to_string(),
            record_uri: None,
            retrieved_at: None,
            entity_kind,
            domain: domain.to_string(),
            context_key: context_key.map(str::to_string),
            primary_name: RawNameValue {
                value: primary_name.to_string(),
                locale: None,
                name_type: None,
                script_hint: None,
            },
            aliases: Vec::new(),
            readings: Vec::new(),
            external_ids: external_ids
                .into_iter()
                .map(|(source_name, value)| RawExternalId {
                    source_name: source_name.to_string(),
                    value: value.to_string(),
                    uri: None,
                })
                .collect(),
            relationships: Vec::new(),
            images: Vec::new(),
            fields: BTreeMap::new(),
        };
        CandidateRecord {
            source_id: source_id.to_string(),
            source_record_id: stable_id("source_record", &[source_id, record_id]),
            license_class: "fixture".to_string(),
            input_base_dir: PathBuf::from("."),
            primary_name: normalize_name(primary_name),
            aliases: Vec::new(),
            readings: Vec::new(),
            external_key_set: raw
                .external_ids
                .iter()
                .map(|id| external_key(&id.source_name, &id.value))
                .collect(),
            raw,
        }
    }

    #[test]
    fn merge_strategy_prefers_external_ids() {
        let left = candidate(
            "a",
            "1",
            "岡部倫太郎",
            EntityKind::FictionalCharacter,
            "anime",
            Some("work:1"),
            vec![("mal", "1")],
        );
        let right = candidate(
            "b",
            "2",
            "岡部倫太郎",
            EntityKind::FictionalCharacter,
            "anime",
            Some("work:1"),
            vec![("mal", "1")],
        );
        assert_eq!(
            determine_merge_strategy(&[0, 1], &[left, right]),
            "external_id"
        );
    }
}
