use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    RealPerson,
    FictionalCharacter,
    Organization,
    Work,
    Product,
    Place,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::RealPerson => "real_person",
            EntityKind::FictionalCharacter => "fictional_character",
            EntityKind::Organization => "organization",
            EntityKind::Work => "work",
            EntityKind::Product => "product",
            EntityKind::Place => "place",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptKind {
    Kanji,
    Hiragana,
    Katakana,
    MixedJapanese,
    Latin,
    Mixed,
    Other,
    Unknown,
}

impl ScriptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScriptKind::Kanji => "kanji",
            ScriptKind::Hiragana => "hiragana",
            ScriptKind::Katakana => "katakana",
            ScriptKind::MixedJapanese => "mixed_japanese",
            ScriptKind::Latin => "latin",
            ScriptKind::Mixed => "mixed",
            ScriptKind::Other => "other",
            ScriptKind::Unknown => "unknown",
        }
    }
    pub fn is_japanese(self) -> bool {
        matches!(
            self,
            ScriptKind::Kanji
                | ScriptKind::Hiragana
                | ScriptKind::Katakana
                | ScriptKind::MixedJapanese
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RightsStatus {
    Unknown,
    Restricted,
    Licensed,
    PublicDomain,
    Shareable,
}

impl RightsStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RightsStatus::Unknown => "unknown",
            RightsStatus::Restricted => "restricted",
            RightsStatus::Licensed => "licensed",
            RightsStatus::PublicDomain => "public_domain",
            RightsStatus::Shareable => "shareable",
        }
    }
    pub fn shareable(self) -> bool {
        matches!(self, RightsStatus::PublicDomain | RightsStatus::Shareable)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseRecord {
    pub id: String,
    pub source_id: String,
    pub license_class: String,
    pub label: String,
    pub url: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: String,
    pub source_id: String,
    pub record_key: String,
    pub record_uri: Option<String>,
    pub retrieved_at: String,
    pub payload_json: String,
    pub license_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRecord {
    pub id: String,
    pub entity_kind: String,
    pub display_name_raw: String,
    pub display_name_normalized: String,
    pub script: String,
    pub domain: String,
    pub context_key: Option<String>,
    pub merge_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameVariantRecord {
    pub id: String,
    pub entity_id: String,
    pub value_raw: String,
    pub value_normalized: String,
    pub script: String,
    pub locale: Option<String>,
    pub name_type: String,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingRecord {
    pub id: String,
    pub name_variant_id: String,
    pub value_raw: String,
    pub value_normalized: String,
    pub script: String,
    pub reading_type: String,
    pub is_derived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalIdRecord {
    pub id: String,
    pub entity_id: String,
    pub source_name: String,
    pub external_id: String,
    pub external_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipRecord {
    pub id: String,
    pub subject_entity_id: String,
    pub predicate: String,
    pub object_entity_id: String,
    pub context_entity_id: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAssetRecord {
    pub id: String,
    pub entity_id: String,
    pub source_record_id: Option<String>,
    pub source_id: String,
    pub sha256: String,
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub relative_path: String,
    pub rights_status: String,
    pub source_url: Option<String>,
    pub local_full_allowed: bool,
    pub shareable_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAssertionRecord {
    pub id: String,
    pub source_id: String,
    pub source_record_id: Option<String>,
    pub target_table: String,
    pub target_row_id: String,
    pub entity_id: Option<String>,
    pub field_path: String,
    pub value_json: String,
    pub retrieval_date: String,
    pub license_class: String,
    pub is_derived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedSourceRow {
    pub id: String,
    pub source_id: String,
    pub record_key: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotArtifacts {
    pub staged_rows: Vec<StagedSourceRow>,
    pub licenses: Vec<LicenseRecord>,
    pub source_records: Vec<SourceRecord>,
    pub entities: Vec<EntityRecord>,
    pub name_variants: Vec<NameVariantRecord>,
    pub readings: Vec<ReadingRecord>,
    pub external_ids: Vec<ExternalIdRecord>,
    pub relationships: Vec<RelationshipRecord>,
    pub image_assets: Vec<ImageAssetRecord>,
    pub source_assertions: Vec<SourceAssertionRecord>,
}

impl SnapshotArtifacts {
    pub fn new() -> Self {
        Self {
            staged_rows: Vec::new(),
            licenses: Vec::new(),
            source_records: Vec::new(),
            entities: Vec::new(),
            name_variants: Vec::new(),
            readings: Vec::new(),
            external_ids: Vec::new(),
            relationships: Vec::new(),
            image_assets: Vec::new(),
            source_assertions: Vec::new(),
        }
    }
}
