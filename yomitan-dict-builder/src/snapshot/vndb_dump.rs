use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Cursor, Read},
    path::Path,
};
use tar::Archive;

use super::{
    model::{EntityKind, RightsStatus},
    source::{RawExternalId, RawImage, RawNameValue, RawRelationship, RawSourceRecord},
};

const REQUIRED_TABLES: &[&str] = &[
    "db/chars",
    "db/chars_names",
    "db/chars_alias",
    "db/chars_vns",
    "db/vn",
    "db/vn_titles",
    "db/vn_seiyuu",
    "db/staff",
    "db/staff_alias",
    "db/images",
];

#[derive(Debug, Clone, Default)]
struct CharBase {
    image_id: Option<String>,
    blood_type: Option<String>,
    sex: Option<String>,
    birthday: Option<u32>,
    height: Option<u32>,
    weight: Option<u32>,
    age: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone)]
struct CharNameRow {
    lang: String,
    name: String,
    latin: Option<String>,
}

#[derive(Debug, Clone)]
struct CharAliasRow {
    spoil: u8,
    name: String,
    latin: Option<String>,
}

#[derive(Debug, Clone)]
struct CharAppearance {
    vn_id: String,
    role: String,
    spoil: u8,
}

#[derive(Debug, Clone, Default)]
struct VnBase {
    image_id: Option<String>,
    olang: Option<String>,
    aliases: Vec<String>,
    description: Option<String>,
}

#[derive(Debug, Clone)]
struct VnTitleRow {
    lang: String,
    official: bool,
    title: String,
    latin: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StaffBase {
    main_alias_id: Option<String>,
    gender: Option<String>,
    lang: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone)]
struct StaffAliasRow {
    staff_id: String,
    alias_id: String,
    name: String,
    latin: Option<String>,
}

#[derive(Debug, Clone)]
struct ImageMeta {
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Clone)]
struct SeiyuuLink {
    vn_id: String,
    char_id: String,
    alias_id: String,
}

pub fn load_vndb_dump_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let tables = load_required_tables(path)?;

    let chars = parse_chars(require_table(&tables, "db/chars")?)?;
    let char_names = parse_char_names(require_table(&tables, "db/chars_names")?)?;
    let char_aliases = parse_char_aliases(require_table(&tables, "db/chars_alias")?)?;
    let char_appearances = parse_char_appearances(require_table(&tables, "db/chars_vns")?)?;
    let vn_base = parse_vn(require_table(&tables, "db/vn")?)?;
    let vn_titles = parse_vn_titles(require_table(&tables, "db/vn_titles")?)?;
    let seiyuu_links = parse_vn_seiyuu(require_table(&tables, "db/vn_seiyuu")?)?;
    let staff_base = parse_staff(require_table(&tables, "db/staff")?)?;
    let staff_aliases = parse_staff_aliases(require_table(&tables, "db/staff_alias")?)?;
    let images = parse_images(require_table(&tables, "db/images")?)?;

    build_raw_records(
        chars,
        char_names,
        char_aliases,
        char_appearances,
        vn_base,
        vn_titles,
        seiyuu_links,
        staff_base,
        staff_aliases,
        images,
    )
}

fn load_required_tables(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    if path.is_dir() {
        let mut tables = BTreeMap::new();
        for table in REQUIRED_TABLES {
            let full_path = path.join(table);
            let bytes = std::fs::read(&full_path)
                .with_context(|| format!("failed to read {}", full_path.display()))?;
            tables.insert((*table).to_string(), bytes);
        }
        return Ok(tables);
    }

    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("failed to open zstd decoder for {}", path.display()))?;
    let mut archive = Archive::new(decoder);
    let mut tables = BTreeMap::new();

    for entry in archive.entries().context("failed to iterate tar entries")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let entry_path = entry.path().context("failed to read tar entry path")?;
        let entry_path = entry_path.to_string_lossy().into_owned();
        if !REQUIRED_TABLES
            .iter()
            .any(|required| *required == entry_path)
        {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read tar member {entry_path}"))?;
        tables.insert(entry_path, bytes);
    }

    for table in REQUIRED_TABLES {
        if !tables.contains_key(*table) {
            bail!(
                "VNDB dump input {} is missing required table {}",
                path.display(),
                table
            );
        }
    }

    Ok(tables)
}

fn require_table<'a>(tables: &'a BTreeMap<String, Vec<u8>>, key: &str) -> Result<&'a [u8]> {
    tables
        .get(key)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow!("missing required VNDB table {key}"))
}

fn parse_chars(bytes: &[u8]) -> Result<BTreeMap<String, CharBase>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 18)? {
        out.insert(
            fields[0].clone(),
            CharBase {
                image_id: non_empty(&fields[1]),
                blood_type: normalized_optional_field(&fields[2]),
                sex: normalized_optional_field(&fields[4]),
                birthday: parse_u32_opt(&fields[13])?,
                height: parse_u32_opt(&fields[14])?,
                weight: parse_u32_opt(&fields[15])?,
                age: normalized_optional_field(&fields[16]),
                description: non_empty(&fields[17]),
            },
        );
    }
    Ok(out)
}

fn parse_char_names(bytes: &[u8]) -> Result<BTreeMap<String, Vec<CharNameRow>>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 4)? {
        out.entry(fields[0].clone())
            .or_insert_with(Vec::new)
            .push(CharNameRow {
                lang: fields[1].clone(),
                name: fields[2].clone(),
                latin: non_empty(&fields[3]),
            });
    }
    Ok(out)
}

fn parse_char_aliases(bytes: &[u8]) -> Result<BTreeMap<String, Vec<CharAliasRow>>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 4)? {
        out.entry(fields[0].clone())
            .or_insert_with(Vec::new)
            .push(CharAliasRow {
                spoil: parse_u8(&fields[1])?,
                name: fields[2].clone(),
                latin: non_empty(&fields[3]),
            });
    }
    Ok(out)
}

fn parse_char_appearances(bytes: &[u8]) -> Result<BTreeMap<String, Vec<CharAppearance>>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 5)? {
        out.entry(fields[0].clone())
            .or_insert_with(Vec::new)
            .push(CharAppearance {
                vn_id: fields[1].clone(),
                role: fields[3].clone(),
                spoil: parse_u8(&fields[4])?,
            });
    }
    Ok(out)
}

fn parse_vn(bytes: &[u8]) -> Result<BTreeMap<String, VnBase>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 13)? {
        let aliases = split_alias_blob(&fields[11]);
        out.insert(
            fields[0].clone(),
            VnBase {
                image_id: non_empty(&fields[1]),
                olang: normalized_optional_field(&fields[3]),
                aliases,
                description: non_empty(&fields[12]),
            },
        );
    }
    Ok(out)
}

fn parse_vn_titles(bytes: &[u8]) -> Result<BTreeMap<String, Vec<VnTitleRow>>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 5)? {
        out.entry(fields[0].clone())
            .or_insert_with(Vec::new)
            .push(VnTitleRow {
                lang: fields[1].clone(),
                official: parse_bool(&fields[2])?,
                title: fields[3].clone(),
                latin: non_empty(&fields[4]),
            });
    }
    Ok(out)
}

fn parse_vn_seiyuu(bytes: &[u8]) -> Result<Vec<SeiyuuLink>> {
    let mut out = Vec::new();
    for fields in parse_copy_lines(bytes, 4)? {
        out.push(SeiyuuLink {
            vn_id: fields[0].clone(),
            char_id: fields[1].clone(),
            alias_id: fields[2].clone(),
        });
    }
    Ok(out)
}

fn parse_staff(bytes: &[u8]) -> Result<BTreeMap<String, StaffBase>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 6)? {
        out.insert(
            fields[0].clone(),
            StaffBase {
                gender: normalized_optional_field(&fields[1]),
                lang: normalized_optional_field(&fields[2]),
                main_alias_id: normalized_optional_field(&fields[3]),
                description: non_empty(&fields[4]),
            },
        );
    }
    Ok(out)
}

fn parse_staff_aliases(bytes: &[u8]) -> Result<BTreeMap<String, Vec<StaffAliasRow>>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 4)? {
        out.entry(fields[0].clone())
            .or_insert_with(Vec::new)
            .push(StaffAliasRow {
                staff_id: fields[0].clone(),
                alias_id: fields[1].clone(),
                name: fields[2].clone(),
                latin: non_empty(&fields[3]),
            });
    }
    Ok(out)
}

fn parse_images(bytes: &[u8]) -> Result<BTreeMap<String, ImageMeta>> {
    let mut out = BTreeMap::new();
    for fields in parse_copy_lines(bytes, 9)? {
        out.insert(
            fields[0].clone(),
            ImageMeta {
                width: parse_u32_opt(&fields[1])?,
                height: parse_u32_opt(&fields[2])?,
            },
        );
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn build_raw_records(
    chars: BTreeMap<String, CharBase>,
    char_names: BTreeMap<String, Vec<CharNameRow>>,
    char_aliases: BTreeMap<String, Vec<CharAliasRow>>,
    char_appearances: BTreeMap<String, Vec<CharAppearance>>,
    vn_base: BTreeMap<String, VnBase>,
    vn_titles: BTreeMap<String, Vec<VnTitleRow>>,
    seiyuu_links: Vec<SeiyuuLink>,
    staff_base: BTreeMap<String, StaffBase>,
    staff_aliases: BTreeMap<String, Vec<StaffAliasRow>>,
    images: BTreeMap<String, ImageMeta>,
) -> Result<Vec<RawSourceRecord>> {
    let mut records = Vec::new();
    let staff_alias_by_id = index_staff_alias_rows(&staff_aliases);
    let seiyuu_by_char_vn = index_seiyuu_links(&seiyuu_links, &staff_alias_by_id);
    let used_vn_ids: BTreeSet<String> = char_appearances
        .values()
        .flat_map(|rows| rows.iter().map(|row| row.vn_id.clone()))
        .collect();
    let used_staff_ids: BTreeSet<String> = seiyuu_by_char_vn
        .values()
        .flat_map(|rows| rows.iter().map(|row| row.staff_id.clone()))
        .collect();

    for vn_id in &used_vn_ids {
        let Some(base) = vn_base.get(vn_id) else {
            continue;
        };
        let titles = vn_titles.get(vn_id).cloned().unwrap_or_default();
        if let Some(record) = build_vn_record(
            vn_id,
            base,
            &titles,
            base.image_id.as_ref().and_then(|id| images.get(id)),
        ) {
            records.push(record);
        }
    }

    for (char_id, base) in chars {
        let names = char_names.get(&char_id).cloned().unwrap_or_default();
        let aliases = char_aliases.get(&char_id).cloned().unwrap_or_default();
        let appearances = char_appearances.get(&char_id).cloned().unwrap_or_default();
        if names.is_empty() || appearances.is_empty() {
            continue;
        }
        records.push(build_char_record(
            &char_id,
            &base,
            &names,
            &aliases,
            &appearances,
            &seiyuu_by_char_vn,
            base.image_id.as_ref().and_then(|id| images.get(id)),
        )?);
    }

    for staff_id in &used_staff_ids {
        let aliases = staff_aliases.get(staff_id).cloned().unwrap_or_default();
        if aliases.is_empty() {
            continue;
        }
        let base = staff_base.get(staff_id).cloned().unwrap_or_default();
        if let Some(record) = build_staff_record(staff_id, &base, &aliases) {
            records.push(record);
        }
    }

    Ok(records)
}

fn build_vn_record(
    vn_id: &str,
    base: &VnBase,
    titles: &[VnTitleRow],
    image_meta: Option<&ImageMeta>,
) -> Option<RawSourceRecord> {
    let primary = choose_vn_primary_title(base, titles)?;
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    seen.insert(primary.value.clone());

    for title in titles {
        push_name_variant(
            &mut aliases,
            &mut seen,
            &title.title,
            Some(title.lang.as_str()),
            Some(if title.official {
                "official_title"
            } else {
                "title"
            }),
        );
        if let Some(latin) = &title.latin {
            push_name_variant(
                &mut aliases,
                &mut seen,
                latin,
                Some(title.lang.as_str()),
                Some("romanized_title"),
            );
        }
    }

    for alias in &base.aliases {
        push_name_variant(&mut aliases, &mut seen, alias, None, Some("alias"));
    }

    let images = base
        .image_id
        .as_ref()
        .and_then(|image_id| build_vndb_image(image_id, image_meta))
        .into_iter()
        .collect();

    Some(RawSourceRecord {
        record_id: format!("vn:{vn_id}"),
        record_uri: Some(format!("https://vndb.org/{vn_id}")),
        retrieved_at: None,
        entity_kind: EntityKind::Work,
        domain: "visual_novel".to_string(),
        context_key: Some(format!("vndb:{vn_id}")),
        primary_name: RawNameValue {
            value: primary.value,
            locale: primary.locale,
            name_type: Some("primary".to_string()),
            script_hint: None,
        },
        aliases,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "vndb".to_string(),
            value: vn_id.to_string(),
            uri: Some(format!("https://vndb.org/{vn_id}")),
        }],
        relationships: Vec::new(),
        images,
        fields: {
            let mut fields = BTreeMap::new();
            fields.insert(
                "olang".to_string(),
                serde_json::json!(base.olang.clone().unwrap_or_default()),
            );
            fields.insert(
                "description".to_string(),
                serde_json::json!(base.description),
            );
            fields
        },
    })
}

fn build_char_record(
    char_id: &str,
    base: &CharBase,
    names: &[CharNameRow],
    alias_rows: &[CharAliasRow],
    appearances: &[CharAppearance],
    seiyuu_by_char_vn: &BTreeMap<(String, String), Vec<ResolvedSeiyuu>>,
    image_meta: Option<&ImageMeta>,
) -> Result<RawSourceRecord> {
    let primary = choose_char_primary_name(names)
        .ok_or_else(|| anyhow!("character {char_id} has no usable name rows"))?;
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    seen.insert(primary.value.clone());

    for row in names {
        push_name_variant(
            &mut aliases,
            &mut seen,
            &row.name,
            Some(row.lang.as_str()),
            Some("name"),
        );
        if let Some(latin) = &row.latin {
            push_name_variant(
                &mut aliases,
                &mut seen,
                latin,
                Some(row.lang.as_str()),
                Some("romanized"),
            );
        }
    }

    for row in alias_rows {
        let alias_type = if row.spoil > 0 {
            "spoiler_alias"
        } else {
            "alias"
        };
        push_name_variant(&mut aliases, &mut seen, &row.name, None, Some(alias_type));
        if let Some(latin) = &row.latin {
            push_name_variant(
                &mut aliases,
                &mut seen,
                latin,
                None,
                Some("romanized_alias"),
            );
        }
    }

    let mut relationships = Vec::new();
    for appearance in appearances {
        relationships.push(RawRelationship {
            predicate: "appears_in".to_string(),
            target_source_name: "vndb".to_string(),
            target_external_id: appearance.vn_id.clone(),
            context_source_name: None,
            context_external_id: None,
            confidence: Some(if appearance.spoil > 0 { 0.8 } else { 1.0 }),
        });
        if let Some(seiyuu_rows) =
            seiyuu_by_char_vn.get(&(char_id.to_string(), appearance.vn_id.clone()))
        {
            for seiyuu in seiyuu_rows {
                relationships.push(RawRelationship {
                    predicate: "voiced_by".to_string(),
                    target_source_name: "vndb".to_string(),
                    target_external_id: seiyuu.staff_id.clone(),
                    context_source_name: Some("vndb".to_string()),
                    context_external_id: Some(appearance.vn_id.clone()),
                    confidence: Some(1.0),
                });
            }
        }
    }

    let images = base
        .image_id
        .as_ref()
        .and_then(|image_id| build_vndb_image(image_id, image_meta))
        .into_iter()
        .collect();

    Ok(RawSourceRecord {
        record_id: format!("character:{char_id}"),
        record_uri: Some(format!("https://vndb.org/{char_id}")),
        retrieved_at: None,
        entity_kind: EntityKind::FictionalCharacter,
        domain: "visual_novel".to_string(),
        context_key: None,
        primary_name: RawNameValue {
            value: primary.value,
            locale: primary.locale,
            name_type: Some("primary".to_string()),
            script_hint: None,
        },
        aliases,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "vndb".to_string(),
            value: char_id.to_string(),
            uri: Some(format!("https://vndb.org/{char_id}")),
        }],
        relationships,
        images,
        fields: {
            let mut fields = BTreeMap::new();
            fields.insert("blood_type".to_string(), serde_json::json!(base.blood_type));
            fields.insert("sex".to_string(), serde_json::json!(base.sex));
            fields.insert("birthday".to_string(), serde_json::json!(base.birthday));
            fields.insert("height_cm".to_string(), serde_json::json!(base.height));
            fields.insert("weight_kg".to_string(), serde_json::json!(base.weight));
            fields.insert("age".to_string(), serde_json::json!(base.age));
            fields.insert(
                "description".to_string(),
                serde_json::json!(base.description.clone()),
            );
            fields.insert(
                "appearances".to_string(),
                serde_json::json!(appearances
                    .iter()
                    .map(|appearance| {
                        serde_json::json!({
                            "vn_id": appearance.vn_id,
                            "role": appearance.role,
                            "spoiler": appearance.spoil,
                        })
                    })
                    .collect::<Vec<_>>()),
            );
            fields
        },
    })
}

fn build_staff_record(
    staff_id: &str,
    base: &StaffBase,
    aliases: &[StaffAliasRow],
) -> Option<RawSourceRecord> {
    let primary = choose_staff_primary_alias(base, aliases)?;
    let mut seen = BTreeSet::new();
    let mut name_variants = Vec::new();
    seen.insert(primary.value.clone());

    for row in aliases {
        push_name_variant(
            &mut name_variants,
            &mut seen,
            &row.name,
            base.lang.as_deref(),
            Some("name"),
        );
        if let Some(latin) = &row.latin {
            push_name_variant(
                &mut name_variants,
                &mut seen,
                latin,
                base.lang.as_deref(),
                Some("romanized"),
            );
        }
    }

    Some(RawSourceRecord {
        record_id: format!("staff:{staff_id}"),
        record_uri: Some(format!("https://vndb.org/{staff_id}")),
        retrieved_at: None,
        entity_kind: EntityKind::RealPerson,
        domain: "voice_actor".to_string(),
        context_key: None,
        primary_name: RawNameValue {
            value: primary.value,
            locale: primary.locale,
            name_type: Some("primary".to_string()),
            script_hint: None,
        },
        aliases: name_variants,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "vndb".to_string(),
            value: staff_id.to_string(),
            uri: Some(format!("https://vndb.org/{staff_id}")),
        }],
        relationships: Vec::new(),
        images: Vec::new(),
        fields: {
            let mut fields = BTreeMap::new();
            fields.insert("gender".to_string(), serde_json::json!(base.gender));
            fields.insert(
                "description".to_string(),
                serde_json::json!(base.description),
            );
            fields
        },
    })
}

fn choose_vn_primary_title(base: &VnBase, titles: &[VnTitleRow]) -> Option<ChosenName> {
    let preferred_lang = base.olang.as_deref().unwrap_or("ja");
    let primary = titles
        .iter()
        .find(|row| row.lang == preferred_lang && row.official && !row.title.is_empty())
        .or_else(|| {
            titles
                .iter()
                .find(|row| row.lang == preferred_lang && !row.title.is_empty())
        })
        .or_else(|| {
            titles
                .iter()
                .find(|row| row.official && !row.title.is_empty())
        })
        .or_else(|| titles.iter().find(|row| !row.title.is_empty()))?;
    Some(ChosenName {
        value: primary.title.clone(),
        locale: Some(primary.lang.clone()),
    })
}

fn choose_char_primary_name(rows: &[CharNameRow]) -> Option<ChosenName> {
    let primary = rows
        .iter()
        .find(|row| row.lang == "ja" && !row.name.is_empty())
        .or_else(|| rows.iter().find(|row| !row.name.is_empty()))
        .or_else(|| rows.iter().find(|row| row.latin.as_deref().is_some()))?;
    Some(ChosenName {
        value: if !primary.name.is_empty() {
            primary.name.clone()
        } else {
            primary.latin.clone().unwrap_or_default()
        },
        locale: Some(primary.lang.clone()),
    })
}

fn choose_staff_primary_alias(base: &StaffBase, aliases: &[StaffAliasRow]) -> Option<ChosenName> {
    let primary = base
        .main_alias_id
        .as_ref()
        .and_then(|alias_id| aliases.iter().find(|row| &row.alias_id == alias_id))
        .or_else(|| aliases.first())?;
    Some(ChosenName {
        value: primary.name.clone(),
        locale: base.lang.clone(),
    })
}

fn push_name_variant(
    out: &mut Vec<RawNameValue>,
    seen: &mut BTreeSet<String>,
    value: &str,
    locale: Option<&str>,
    name_type: Option<&str>,
) {
    let trimmed = value.trim();
    if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
        return;
    }
    out.push(RawNameValue {
        value: trimmed.to_string(),
        locale: locale.map(str::to_string),
        name_type: name_type.map(str::to_string),
        script_hint: None,
    });
}

fn build_vndb_image(image_id: &str, image_meta: Option<&ImageMeta>) -> Option<RawImage> {
    Some(RawImage {
        url: Some(vndb_image_url(image_id)?),
        local_path: None,
        bytes_base64: None,
        ext: Some("jpg".to_string()),
        rights_status: Some(RightsStatus::Restricted),
        width: image_meta.and_then(|meta| meta.width),
        height: image_meta.and_then(|meta| meta.height),
    })
}

pub fn vndb_image_url(image_id: &str) -> Option<String> {
    let prefix: String = image_id
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    let digits: String = image_id
        .chars()
        .skip_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    if prefix.is_empty() || digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let shard = if digits.len() >= 2 {
        &digits[digits.len() - 2..]
    } else {
        &digits[..]
    };
    Some(format!("https://t.vndb.org/{prefix}/{shard}/{digits}.jpg"))
}

fn parse_copy_lines(bytes: &[u8], expected_fields: usize) -> Result<Vec<Vec<String>>> {
    let reader = BufReader::new(Cursor::new(bytes));
    let mut rows = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("failed to read COPY line {}", idx + 1))?;
        if line.is_empty() {
            continue;
        }
        let raw_fields: Vec<&str> = line.split('\t').collect();
        if raw_fields.len() != expected_fields {
            bail!(
                "COPY row {} had {} fields, expected {}",
                idx + 1,
                raw_fields.len(),
                expected_fields
            );
        }
        rows.push(
            raw_fields
                .into_iter()
                .map(decode_copy_field)
                .collect::<Result<Vec<_>>>()?,
        );
    }
    Ok(rows)
}

fn decode_copy_field(raw: &str) -> Result<String> {
    if raw == "\\N" {
        return Ok(String::new());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            bail!("dangling backslash in COPY field");
        };
        match escaped {
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{000B}'),
            '\\' => out.push('\\'),
            other => out.push(other),
        }
    }
    Ok(out)
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "t" => Ok(true),
        "f" => Ok(false),
        other => bail!("invalid boolean value {other}"),
    }
}

fn parse_u8(value: &str) -> Result<u8> {
    value
        .parse::<u8>()
        .with_context(|| format!("invalid u8 value {value}"))
}

fn parse_u32_opt(value: &str) -> Result<Option<u32>> {
    if value.is_empty() || value == "0" {
        return Ok(None);
    }
    let parsed = value
        .parse::<u32>()
        .with_context(|| format!("invalid u32 value {value}"))?;
    Ok(Some(parsed))
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalized_optional_field(value: &str) -> Option<String> {
    non_empty(value).and_then(|value| match value.as_str() {
        "unknown" => None,
        other => Some(other.to_string()),
    })
}

fn split_alias_blob(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone)]
struct ResolvedSeiyuu {
    staff_id: String,
}

fn index_staff_alias_rows(
    rows: &BTreeMap<String, Vec<StaffAliasRow>>,
) -> BTreeMap<String, StaffAliasRow> {
    let mut out = BTreeMap::new();
    for aliases in rows.values() {
        for alias in aliases {
            out.insert(alias.alias_id.clone(), alias.clone());
        }
    }
    out
}

fn index_seiyuu_links(
    links: &[SeiyuuLink],
    staff_alias_by_id: &BTreeMap<String, StaffAliasRow>,
) -> BTreeMap<(String, String), Vec<ResolvedSeiyuu>> {
    let mut out: BTreeMap<(String, String), Vec<ResolvedSeiyuu>> = BTreeMap::new();
    for link in links {
        let Some(alias) = staff_alias_by_id.get(&link.alias_id) else {
            continue;
        };
        out.entry((link.char_id.clone(), link.vn_id.clone()))
            .or_default()
            .push(ResolvedSeiyuu {
                staff_id: alias.staff_id.clone(),
            });
    }
    out
}

#[derive(Debug, Clone)]
struct ChosenName {
    value: String,
    locale: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_vndb_image_urls() {
        assert_eq!(
            vndb_image_url("ch175652").as_deref(),
            Some("https://t.vndb.org/ch/52/175652.jpg")
        );
        assert_eq!(
            vndb_image_url("cv20339").as_deref(),
            Some("https://t.vndb.org/cv/39/20339.jpg")
        );
    }

    #[test]
    fn loads_fixture_directory_into_raw_records() {
        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vndb_dump_archive");
        let records = load_vndb_dump_records(&fixture_dir).unwrap();
        assert!(records.iter().any(|row| row.record_id == "vn:v17"));
        assert!(records.iter().any(|row| row.record_id == "character:c11"));
        assert!(records.iter().any(|row| row.record_id == "staff:s44"));
    }
}
