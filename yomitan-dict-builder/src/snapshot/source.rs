use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::{
    anilist_kaggle::load_kaggle_anilist_records,
    config::SourceConfig,
    jmnedict::load_jmnedict_records,
    model::{EntityKind, RightsStatus, ScriptKind},
    vndb_dump::load_vndb_dump_records,
    web_ndl_authorities::load_web_ndl_authority_records,
    wikidata_dump::load_wikidata_dump_records,
    wikipedia_dump::load_wikipedia_dump_records,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSourceBundle {
    #[serde(default)]
    pub records: Vec<RawSourceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSourceRecord {
    pub record_id: String,
    #[serde(default)]
    pub record_uri: Option<String>,
    #[serde(default)]
    pub retrieved_at: Option<String>,
    pub entity_kind: EntityKind,
    pub domain: String,
    #[serde(default)]
    pub context_key: Option<String>,
    pub primary_name: RawNameValue,
    #[serde(default)]
    pub aliases: Vec<RawNameValue>,
    #[serde(default)]
    pub readings: Vec<RawReading>,
    #[serde(default)]
    pub external_ids: Vec<RawExternalId>,
    #[serde(default)]
    pub relationships: Vec<RawRelationship>,
    #[serde(default)]
    pub images: Vec<RawImage>,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawNameValue {
    pub value: String,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub name_type: Option<String>,
    #[serde(default)]
    pub script_hint: Option<ScriptKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawReading {
    pub value: String,
    #[serde(default)]
    pub for_name: Option<String>,
    #[serde(default)]
    pub reading_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawExternalId {
    pub source_name: String,
    pub value: String,
    #[serde(default)]
    pub uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRelationship {
    pub predicate: String,
    pub target_source_name: String,
    pub target_external_id: String,
    #[serde(default)]
    pub context_source_name: Option<String>,
    #[serde(default)]
    pub context_external_id: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawImage {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    #[serde(default)]
    pub bytes_base64: Option<String>,
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default)]
    pub rights_status: Option<RightsStatus>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct SourcePolicy {
    pub kind: &'static str,
    pub allow_images: bool,
    pub allowed_entity_kinds: &'static [EntityKind],
    pub disabled_reason: Option<&'static str>,
}

impl SourcePolicy {
    pub fn validate_record(&self, record: &RawSourceRecord) -> Result<()> {
        if !self.allowed_entity_kinds.contains(&record.entity_kind) {
            bail!(
                "source kind {} cannot emit entity kind {}",
                self.kind,
                record.entity_kind.as_str()
            );
        }
        if !self.allow_images && !record.images.is_empty() {
            bail!("source kind {} cannot emit image records", self.kind);
        }
        Ok(())
    }
}

pub fn source_policy(kind: &str) -> Result<SourcePolicy> {
    let policy = match kind {
        "fixture_bundle" => SourcePolicy {
            kind: "fixture_bundle",
            allow_images: true,
            allowed_entity_kinds: &[
                EntityKind::RealPerson,
                EntityKind::FictionalCharacter,
                EntityKind::Organization,
                EntityKind::Work,
                EntityKind::Product,
                EntityKind::Place,
            ],
            disabled_reason: None,
        },
        "anime_offline_database" => SourcePolicy {
            kind: "anime_offline_database",
            allow_images: true,
            allowed_entity_kinds: &[EntityKind::Work],
            disabled_reason: None,
        },
        "kaggle_mal_catalog" => SourcePolicy {
            kind: "kaggle_mal_catalog",
            allow_images: false,
            allowed_entity_kinds: &[EntityKind::Work, EntityKind::Organization],
            disabled_reason: None,
        },
        "kaggle_anilist_snapshot" => SourcePolicy {
            kind: "kaggle_anilist_snapshot",
            allow_images: true,
            allowed_entity_kinds: &[
                EntityKind::FictionalCharacter,
                EntityKind::RealPerson,
                EntityKind::Work,
            ],
            disabled_reason: None,
        },
        "kaggle_mal_character_snapshot" => SourcePolicy {
            kind: "kaggle_mal_character_snapshot",
            allow_images: true,
            allowed_entity_kinds: &[EntityKind::FictionalCharacter, EntityKind::RealPerson],
            disabled_reason: None,
        },
        "jmnedict" => SourcePolicy {
            kind: "jmnedict",
            allow_images: false,
            allowed_entity_kinds: &[
                EntityKind::RealPerson,
                EntityKind::Organization,
                EntityKind::Work,
                EntityKind::Product,
                EntityKind::Place,
            ],
            disabled_reason: None,
        },
        "web_ndl_authorities" => SourcePolicy {
            kind: "web_ndl_authorities",
            allow_images: false,
            allowed_entity_kinds: &[EntityKind::RealPerson, EntityKind::Organization],
            disabled_reason: None,
        },
        "wikidata_dump" | "wikipedia_dump" => SourcePolicy {
            kind: if kind == "wikidata_dump" {
                "wikidata_dump"
            } else {
                "wikipedia_dump"
            },
            allow_images: false,
            allowed_entity_kinds: &[
                EntityKind::RealPerson,
                EntityKind::FictionalCharacter,
                EntityKind::Organization,
                EntityKind::Work,
                EntityKind::Product,
                EntityKind::Place,
            ],
            disabled_reason: None,
        },
        "bangumi_snapshot" => SourcePolicy {
            kind: "bangumi_snapshot",
            allow_images: true,
            allowed_entity_kinds: &[
                EntityKind::FictionalCharacter,
                EntityKind::RealPerson,
                EntityKind::Work,
            ],
            disabled_reason: None,
        },
        "musicbrainz_dump" => SourcePolicy {
            kind: "musicbrainz_dump",
            allow_images: false,
            allowed_entity_kinds: &[
                EntityKind::RealPerson,
                EntityKind::Organization,
                EntityKind::Work,
                EntityKind::Product,
            ],
            disabled_reason: None,
        },
        "viaf_dump" => SourcePolicy {
            kind: "viaf_dump",
            allow_images: false,
            allowed_entity_kinds: &[EntityKind::RealPerson, EntityKind::Organization],
            disabled_reason: None,
        },
        "vndb_dump" => SourcePolicy {
            kind: "vndb_dump",
            allow_images: true,
            allowed_entity_kinds: &[
                EntityKind::FictionalCharacter,
                EntityKind::RealPerson,
                EntityKind::Work,
            ],
            disabled_reason: None,
        },
        "direct_anilist_api" => SourcePolicy {
            kind: "direct_anilist_api",
            allow_images: false,
            allowed_entity_kinds: &[],
            disabled_reason: Some(
                "direct AniList API full-crawl ingestion is intentionally disabled in v1",
            ),
        },
        _ => return Err(anyhow!("unknown source kind {kind}")),
    };
    Ok(policy)
}

pub fn enabled_source_ids(sources: &BTreeMap<String, SourceConfig>) -> BTreeSet<String> {
    sources
        .iter()
        .filter(|(_, source)| source.enabled)
        .map(|(id, _)| id.clone())
        .collect()
}

pub fn load_raw_records(
    path: &Path,
    format_hint: Option<&str>,
    source_kind: &str,
) -> Result<Vec<RawSourceRecord>> {
    if source_kind == "vndb_dump"
        || format_hint == Some("vndb_dump")
        || path.is_dir()
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("vndb-db-") && name.ends_with(".tar.zst"))
    {
        return load_vndb_dump_records(path);
    }
    if source_kind == "kaggle_anilist_snapshot" || format_hint == Some("kaggle_anilist_snapshot") {
        return load_kaggle_anilist_records(path);
    }
    if source_kind == "jmnedict"
        || format_hint == Some("jmnedict")
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("JMnedict.xml")
                    || name.eq_ignore_ascii_case("JMnedict.xml.gz")
            })
    {
        return load_jmnedict_records(path);
    }
    if source_kind == "web_ndl_authorities" || format_hint == Some("web_ndl_authorities") {
        return load_web_ndl_authority_records(path);
    }
    if source_kind == "wikidata_dump" || format_hint == Some("wikidata_dump") {
        return load_wikidata_dump_records(path);
    }
    if source_kind == "wikipedia_dump" || format_hint == Some("wikipedia_dump") {
        return load_wikipedia_dump_records(path);
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read source input {}", path.display()))?;
    let format = format_hint
        .map(str::to_string)
        .or_else(|| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "json".to_string());

    match format.as_str() {
        "jsonl" => raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<RawSourceRecord>(line).context("invalid JSONL row"))
            .collect(),
        "json" => {
            if let Ok(bundle) = serde_json::from_str::<RawSourceBundle>(&raw) {
                return Ok(bundle.records);
            }
            if let Ok(records) = serde_json::from_str::<Vec<RawSourceRecord>>(&raw) {
                return Ok(records);
            }
            Err(anyhow!(
                "JSON input {} must be an object with records or an array of records",
                path.display()
            ))
        }
        other => Err(anyhow!("unsupported source format {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_policies_block_title_only_character_rows() {
        let record = RawSourceRecord {
            record_id: "r1".to_string(),
            record_uri: None,
            retrieved_at: None,
            entity_kind: EntityKind::FictionalCharacter,
            domain: "anime".to_string(),
            context_key: None,
            primary_name: RawNameValue {
                value: "岡部倫太郎".to_string(),
                locale: None,
                name_type: None,
                script_hint: None,
            },
            aliases: Vec::new(),
            readings: Vec::new(),
            external_ids: Vec::new(),
            relationships: Vec::new(),
            images: Vec::new(),
            fields: BTreeMap::new(),
        };

        let err = source_policy("anime_offline_database")
            .unwrap()
            .validate_record(&record)
            .unwrap_err();
        assert!(err.to_string().contains("cannot emit entity kind"));
    }

    #[test]
    fn source_policies_block_images_for_mal_catalog() {
        let record = RawSourceRecord {
            record_id: "r1".to_string(),
            record_uri: None,
            retrieved_at: None,
            entity_kind: EntityKind::Work,
            domain: "anime".to_string(),
            context_key: None,
            primary_name: RawNameValue {
                value: "Steins;Gate".to_string(),
                locale: None,
                name_type: None,
                script_hint: None,
            },
            aliases: Vec::new(),
            readings: Vec::new(),
            external_ids: Vec::new(),
            relationships: Vec::new(),
            images: vec![RawImage {
                url: Some("https://example.com/image.jpg".to_string()),
                local_path: None,
                bytes_base64: None,
                ext: Some("jpg".to_string()),
                rights_status: None,
                width: None,
                height: None,
            }],
            fields: BTreeMap::new(),
        };

        let err = source_policy("kaggle_mal_catalog")
            .unwrap()
            .validate_record(&record)
            .unwrap_err();
        assert!(err.to_string().contains("cannot emit image records"));
    }

    #[test]
    fn direct_anilist_is_disabled() {
        let policy = source_policy("direct_anilist_api").unwrap();
        assert!(policy.disabled_reason.is_some());
    }
}
