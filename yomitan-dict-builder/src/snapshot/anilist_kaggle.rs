use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::{
    model::RightsStatus,
    source::{RawExternalId, RawImage, RawNameValue, RawRelationship, RawSourceRecord},
};

#[derive(Debug, Default)]
struct NameFields {
    full: Option<String>,
    native: Option<String>,
    first: Option<String>,
    last: Option<String>,
    user_preferred: Option<String>,
    alternative: Vec<String>,
    alternative_spoiler: Vec<String>,
}

struct NameSelection {
    primary: RawNameValue,
    aliases: Vec<RawNameValue>,
}

struct MediaContext {
    media_id: String,
    domain: String,
    media_uri: Option<String>,
    retrieved_at: Option<String>,
}

pub fn load_kaggle_anilist_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("json")
        .to_ascii_lowercase()
        .as_str()
    {
        "csv" => load_csv_rows(path),
        "jsonl" | "ndjson" => load_jsonl_rows(path),
        "json" => load_json_rows(path),
        other => Err(anyhow!(
            "unsupported AniList Kaggle input format {other} for {}",
            path.display()
        )),
    }
}

fn load_csv_rows(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("failed to open AniList Kaggle CSV {}", path.display()))?;
    let headers = reader
        .headers()
        .with_context(|| format!("failed to read headers from {}", path.display()))?
        .clone();

    let mut records = Vec::new();
    for (row_idx, row) in reader.records().enumerate() {
        let row = row.with_context(|| {
            format!(
                "failed to read AniList Kaggle CSV row {} from {}",
                row_idx + 2,
                path.display()
            )
        })?;
        let mut object = Map::new();
        for (header, value) in headers.iter().zip(row.iter()) {
            object.insert(header.to_string(), csv_cell_to_value(value));
        }
        records.extend(media_row_to_records(&object).with_context(|| {
            format!(
                "failed to parse AniList Kaggle CSV row {} from {}",
                row_idx + 2,
                path.display()
            )
        })?);
    }
    Ok(records)
}

fn load_jsonl_rows(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read AniList Kaggle JSONL {}", path.display()))?;
    let mut records = Vec::new();
    for (line_idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).with_context(|| {
            format!(
                "invalid AniList Kaggle JSONL row {} in {}",
                line_idx + 1,
                path.display()
            )
        })?;
        if let Some(raw_record) = maybe_raw_record(&value) {
            records.push(raw_record);
            continue;
        }
        let Some(object) = value.as_object() else {
            return Err(anyhow!(
                "AniList Kaggle JSONL row {} in {} is not an object",
                line_idx + 1,
                path.display()
            ));
        };
        records.extend(media_row_to_records(object).with_context(|| {
            format!(
                "failed to parse AniList Kaggle JSONL row {} in {}",
                line_idx + 1,
                path.display()
            )
        })?);
    }
    Ok(records)
}

fn load_json_rows(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read AniList Kaggle JSON {}", path.display()))?;
    if let Ok(raw_records) = serde_json::from_str::<Vec<RawSourceRecord>>(&raw) {
        return Ok(raw_records);
    }
    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        if let Some(bundle_records) = value.get("records").and_then(|records| {
            serde_json::from_value::<Vec<RawSourceRecord>>(records.clone()).ok()
        }) {
            return Ok(bundle_records);
        }

        let mut out = Vec::new();
        for value in media_values_from_json(&value)? {
            if let Some(raw_record) = maybe_raw_record(&value) {
                out.push(raw_record);
                continue;
            }
            let object = value
                .as_object()
                .ok_or_else(|| anyhow!("AniList Kaggle JSON rows must be objects"))?;
            out.extend(media_row_to_records(object)?);
        }
        return Ok(out);
    }
    Err(anyhow!(
        "failed to parse AniList Kaggle JSON input {}",
        path.display()
    ))
}

fn media_values_from_json(value: &Value) -> Result<Vec<Value>> {
    match value {
        Value::Array(rows) => Ok(rows.clone()),
        Value::Object(object) => {
            for key in ["data", "rows", "items"] {
                if let Some(value) = object.get(key) {
                    return media_values_from_json(value);
                }
            }
            Ok(vec![value.clone()])
        }
        _ => Err(anyhow!(
            "AniList Kaggle JSON root must be an object or array"
        )),
    }
}

fn media_row_to_records(row: &Map<String, Value>) -> Result<Vec<RawSourceRecord>> {
    let media = parse_media_context(row)?;
    let work_id = format!("work-anilist-{}", media.media_id);
    let work_name = select_work_names(row)?;

    let mut work_record = RawSourceRecord {
        record_id: work_id,
        record_uri: media.media_uri.clone(),
        retrieved_at: media.retrieved_at.clone(),
        entity_kind: super::model::EntityKind::Work,
        domain: media.domain.clone(),
        context_key: Some(format!("anilist:{}", media.media_id)),
        primary_name: work_name.primary,
        aliases: work_name.aliases,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "anilist".to_string(),
            value: media.media_id.clone(),
            uri: media.media_uri.clone(),
        }],
        relationships: Vec::new(),
        images: Vec::new(),
        fields: BTreeMap::new(),
    };

    if let Some(mal_id) = string_field(row, &["idMal", "id_mal", "mal_id", "malId"]) {
        work_record.external_ids.push(RawExternalId {
            source_name: "mal".to_string(),
            value: mal_id.clone(),
            uri: Some(format!(
                "https://myanimelist.net/{}/{}",
                media.domain, mal_id
            )),
        });
    }
    push_image_url(
        &mut work_record.images,
        image_url_from_value(field_value(row, &["coverImage", "cover_image"]))
            .or_else(|| string_field(row, &["coverImage_large", "cover_image_large"]))
            .or_else(|| string_field(row, &["coverImage_extraLarge", "cover_image_extra_large"])),
    );
    push_image_url(
        &mut work_record.images,
        string_field(row, &["bannerImage", "banner_image"]),
    );
    copy_scalar_field(
        row,
        &mut work_record.fields,
        "media_type",
        &["type", "media_type"],
    );
    copy_scalar_field(row, &mut work_record.fields, "format", &["format"]);
    copy_scalar_field(row, &mut work_record.fields, "status", &["status"]);
    copy_scalar_field(
        row,
        &mut work_record.fields,
        "updated_at",
        &["updatedAt", "updated_at"],
    );

    let mut character_records = Vec::new();
    let mut person_records: BTreeMap<String, RawSourceRecord> = BTreeMap::new();

    for edge in value_array_field(row, &["characters", "character_edges", "character_list"]) {
        let Some(edge_object) = edge.as_object() else {
            continue;
        };
        let node = edge_object
            .get("node")
            .and_then(Value::as_object)
            .or_else(|| edge_object.get("character").and_then(Value::as_object))
            .unwrap_or(edge_object);

        let Some(character_id) =
            string_field(node, &["id", "characterId", "character_id", "id_anilist"])
        else {
            continue;
        };

        let Some(character_name) = select_person_names(&extract_name_fields(node), "primary")
        else {
            continue;
        };

        let role = normalize_character_role(
            string_field(edge_object, &["role", "characterRole", "character_role"])
                .unwrap_or_else(|| "appears".to_string()),
        );
        let char_uri = string_field(node, &["siteUrl", "site_url"])
            .or_else(|| Some(format!("https://anilist.co/character/{character_id}")));
        let mut character_record = RawSourceRecord {
            record_id: format!("char-anilist-{character_id}-media-{}", media.media_id),
            record_uri: char_uri.clone(),
            retrieved_at: media.retrieved_at.clone(),
            entity_kind: super::model::EntityKind::FictionalCharacter,
            domain: media.domain.clone(),
            context_key: Some(format!("anilist:{}", media.media_id)),
            primary_name: character_name.primary,
            aliases: character_name.aliases,
            readings: Vec::new(),
            external_ids: vec![RawExternalId {
                source_name: "anilist".to_string(),
                value: character_id.clone(),
                uri: char_uri,
            }],
            relationships: vec![RawRelationship {
                predicate: "appears_in".to_string(),
                target_source_name: "anilist".to_string(),
                target_external_id: media.media_id.clone(),
                context_source_name: None,
                context_external_id: None,
                confidence: Some(1.0),
            }],
            images: Vec::new(),
            fields: BTreeMap::new(),
        };

        if let Some(mal_id) = string_field(node, &["idMal", "id_mal", "mal_id", "malId"]) {
            character_record.external_ids.push(RawExternalId {
                source_name: "mal".to_string(),
                value: mal_id.clone(),
                uri: Some(format!("https://myanimelist.net/character/{mal_id}")),
            });
        }
        push_image_url(
            &mut character_record.images,
            image_url_from_value(node.get("image"))
                .or_else(|| string_field(node, &["image", "image_url"])),
        );
        if !role.is_empty() {
            character_record
                .fields
                .insert("role".to_string(), Value::String(role));
        }
        copy_scalar_field(
            node,
            &mut character_record.fields,
            "description",
            &["description"],
        );
        copy_scalar_field(node, &mut character_record.fields, "gender", &["gender"]);
        copy_scalar_field(node, &mut character_record.fields, "age", &["age"]);
        copy_scalar_field(
            node,
            &mut character_record.fields,
            "blood_type",
            &["bloodType", "blood_type"],
        );
        copy_scalar_field(
            node,
            &mut character_record.fields,
            "first_name_hint",
            &["name_first", "first_name"],
        );
        copy_scalar_field(
            node,
            &mut character_record.fields,
            "last_name_hint",
            &["name_last", "last_name"],
        );
        if let Some(date_value) = field_value(node, &["dateOfBirth", "date_of_birth"]).cloned() {
            character_record
                .fields
                .insert("date_of_birth".to_string(), date_value);
        }

        for actor in value_array_field(edge_object, &["voiceActors", "voice_actors"]) {
            let Some(actor_object) = actor.as_object() else {
                continue;
            };
            if !voice_actor_is_japanese(actor_object) {
                continue;
            }
            let Some(actor_id) =
                string_field(actor_object, &["id", "staffId", "staff_id", "id_anilist"])
            else {
                continue;
            };

            upsert_person_record(
                &mut person_records,
                &media,
                &actor_id,
                string_field(actor_object, &["siteUrl", "site_url"])
                    .or_else(|| Some(format!("https://anilist.co/staff/{actor_id}"))),
                "voice_actor",
                &extract_name_fields(actor_object),
                image_url_from_value(actor_object.get("image"))
                    .or_else(|| string_field(actor_object, &["image", "image_url"])),
                string_field(actor_object, &["idMal", "id_mal", "mal_id", "malId"]).map(|mal_id| {
                    RawExternalId {
                        source_name: "mal".to_string(),
                        value: mal_id.clone(),
                        uri: Some(format!("https://myanimelist.net/people/{mal_id}")),
                    }
                }),
                None,
            );

            push_unique_relationship(
                &mut character_record.relationships,
                RawRelationship {
                    predicate: "voiced_by".to_string(),
                    target_source_name: "anilist".to_string(),
                    target_external_id: actor_id,
                    context_source_name: Some("anilist".to_string()),
                    context_external_id: Some(media.media_id.clone()),
                    confidence: Some(1.0),
                },
            );
        }

        character_records.push(character_record);
    }

    for staff_entry in value_array_field(row, &["staff", "staff_edges", "staff_list"]) {
        let Some(staff_object) = staff_entry.as_object() else {
            continue;
        };
        let node = staff_object
            .get("node")
            .and_then(Value::as_object)
            .or_else(|| staff_object.get("staff").and_then(Value::as_object))
            .unwrap_or(staff_object);
        let Some(staff_id) = string_field(node, &["id", "staffId", "staff_id", "id_anilist"])
        else {
            continue;
        };
        upsert_person_record(
            &mut person_records,
            &media,
            &staff_id,
            string_field(node, &["siteUrl", "site_url"])
                .or_else(|| Some(format!("https://anilist.co/staff/{staff_id}"))),
            "staff",
            &extract_name_fields(node),
            image_url_from_value(node.get("image"))
                .or_else(|| string_field(node, &["image", "image_url"])),
            string_field(node, &["idMal", "id_mal", "mal_id", "malId"]).map(|mal_id| {
                RawExternalId {
                    source_name: "mal".to_string(),
                    value: mal_id.clone(),
                    uri: Some(format!("https://myanimelist.net/people/{mal_id}")),
                }
            }),
            Some(RawRelationship {
                predicate: "worked_on".to_string(),
                target_source_name: "anilist".to_string(),
                target_external_id: media.media_id.clone(),
                context_source_name: None,
                context_external_id: None,
                confidence: Some(1.0),
            }),
        );
    }

    let mut records = vec![work_record];
    records.extend(character_records);
    records.extend(person_records.into_values());
    Ok(records)
}

fn parse_media_context(row: &Map<String, Value>) -> Result<MediaContext> {
    let media_id = string_field(row, &["id", "media_id", "anilist_id"])
        .ok_or_else(|| anyhow!("AniList Kaggle row is missing media id"))?;
    let media_type = string_field(row, &["type", "media_type", "mediaType"])
        .unwrap_or_else(|| "ANIME".to_string());
    let domain = match media_type.trim().to_ascii_uppercase().as_str() {
        "MANGA" => "manga".to_string(),
        "ANIME" => "anime".to_string(),
        other => other.to_ascii_lowercase(),
    };
    let media_uri = string_field(row, &["siteUrl", "site_url"])
        .or_else(|| Some(format!("https://anilist.co/{domain}/{media_id}")));

    Ok(MediaContext {
        media_id,
        domain,
        media_uri,
        retrieved_at: normalized_timestamp(
            field_value(row, &["updatedAt", "updated_at"])
                .or_else(|| field_value(row, &["createdAt", "created_at"])),
        ),
    })
}

fn select_work_names(row: &Map<String, Value>) -> Result<NameSelection> {
    let title = field_value(row, &["title"])
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let native = string_field(row, &["title_native", "native_title"])
        .or_else(|| string_field(&title, &["native"]));
    let romaji = string_field(row, &["title_romaji", "romaji_title", "title"])
        .or_else(|| string_field(&title, &["romaji"]));
    let english = string_field(row, &["title_english", "english_title"])
        .or_else(|| string_field(&title, &["english"]));
    let user_preferred = string_field(row, &["title_user_preferred", "user_preferred_title"])
        .or_else(|| string_field(&title, &["userPreferred"]));
    let synonyms = string_array_field(row, &["synonyms"]);

    let primary = if let Some(native) = native.clone().filter(|value| !value.is_empty()) {
        RawNameValue {
            value: native,
            locale: Some("ja".to_string()),
            name_type: Some("primary".to_string()),
            script_hint: None,
        }
    } else if let Some(romaji) = romaji.clone().filter(|value| !value.is_empty()) {
        RawNameValue {
            value: romaji,
            locale: Some("en".to_string()),
            name_type: Some("primary".to_string()),
            script_hint: None,
        }
    } else if let Some(english) = english.clone().filter(|value| !value.is_empty()) {
        RawNameValue {
            value: english,
            locale: Some("en".to_string()),
            name_type: Some("primary".to_string()),
            script_hint: None,
        }
    } else if let Some(user_preferred) = user_preferred.clone().filter(|value| !value.is_empty()) {
        RawNameValue {
            value: user_preferred,
            locale: None,
            name_type: Some("primary".to_string()),
            script_hint: None,
        }
    } else {
        return Err(anyhow!("AniList Kaggle row is missing usable titles"));
    };

    let mut aliases = Vec::new();
    push_unique_name(
        &mut aliases,
        Some(RawNameValue {
            value: native.unwrap_or_default(),
            locale: Some("ja".to_string()),
            name_type: Some("native".to_string()),
            script_hint: None,
        }),
        &primary.value,
    );
    push_unique_name(
        &mut aliases,
        Some(RawNameValue {
            value: romaji.unwrap_or_default(),
            locale: Some("en".to_string()),
            name_type: Some("romaji".to_string()),
            script_hint: None,
        }),
        &primary.value,
    );
    push_unique_name(
        &mut aliases,
        Some(RawNameValue {
            value: english.unwrap_or_default(),
            locale: Some("en".to_string()),
            name_type: Some("english".to_string()),
            script_hint: None,
        }),
        &primary.value,
    );
    push_unique_name(
        &mut aliases,
        user_preferred.map(|value| RawNameValue {
            value,
            locale: None,
            name_type: Some("preferred".to_string()),
            script_hint: None,
        }),
        &primary.value,
    );
    for synonym in synonyms {
        push_unique_name(
            &mut aliases,
            Some(RawNameValue {
                value: synonym,
                locale: None,
                name_type: Some("synonym".to_string()),
                script_hint: None,
            }),
            &primary.value,
        );
    }

    Ok(NameSelection { primary, aliases })
}

fn extract_name_fields(object: &Map<String, Value>) -> NameFields {
    let name = object
        .get("name")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    NameFields {
        full: string_field(object, &["name_full", "full_name"])
            .or_else(|| string_field(&name, &["full"])),
        native: string_field(object, &["name_native", "native_name"])
            .or_else(|| string_field(&name, &["native"])),
        first: string_field(object, &["name_first", "first_name"])
            .or_else(|| string_field(&name, &["first"])),
        last: string_field(object, &["name_last", "last_name"])
            .or_else(|| string_field(&name, &["last"])),
        user_preferred: string_field(object, &["name_user_preferred", "user_preferred_name"])
            .or_else(|| string_field(&name, &["userPreferred"])),
        alternative: string_array_field(object, &["alternative", "aliases", "alternative_names"])
            .into_iter()
            .chain(string_array_field(
                &name,
                &["alternative", "aliases", "alternative_names"],
            ))
            .collect(),
        alternative_spoiler: string_array_field(
            object,
            &["alternativeSpoiler", "spoilerAliases", "spoiler_aliases"],
        )
        .into_iter()
        .chain(string_array_field(
            &name,
            &["alternativeSpoiler", "spoilerAliases", "spoiler_aliases"],
        ))
        .collect(),
    }
}

fn select_person_names(fields: &NameFields, primary_name_type: &str) -> Option<NameSelection> {
    let combined = combine_western_name(fields.first.as_deref(), fields.last.as_deref());
    let primary_value = fields
        .native
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fields
                .full
                .clone()
                .filter(|value| !value.is_empty())
                .or(combined.clone())
                .or_else(|| fields.user_preferred.clone())
        })?;

    let primary_locale = if looks_japanese(&primary_value) {
        Some("ja".to_string())
    } else {
        Some("en".to_string())
    };
    let primary = RawNameValue {
        value: primary_value.clone(),
        locale: primary_locale,
        name_type: Some(primary_name_type.to_string()),
        script_hint: None,
    };

    let mut aliases = Vec::new();
    push_unique_name(
        &mut aliases,
        fields.native.clone().map(|value| RawNameValue {
            value,
            locale: Some("ja".to_string()),
            name_type: Some("native".to_string()),
            script_hint: None,
        }),
        &primary_value,
    );
    push_unique_name(
        &mut aliases,
        fields.full.clone().map(|value| RawNameValue {
            value,
            locale: Some("en".to_string()),
            name_type: Some("romanized".to_string()),
            script_hint: None,
        }),
        &primary_value,
    );
    push_unique_name(
        &mut aliases,
        combined.map(|value| RawNameValue {
            value,
            locale: Some("en".to_string()),
            name_type: Some("romanized".to_string()),
            script_hint: None,
        }),
        &primary_value,
    );
    push_unique_name(
        &mut aliases,
        fields.user_preferred.clone().map(|value| RawNameValue {
            value,
            locale: None,
            name_type: Some("preferred".to_string()),
            script_hint: None,
        }),
        &primary_value,
    );

    for alias in &fields.alternative {
        push_unique_name(
            &mut aliases,
            Some(RawNameValue {
                value: alias.clone(),
                locale: None,
                name_type: Some("alias".to_string()),
                script_hint: None,
            }),
            &primary_value,
        );
    }
    for alias in &fields.alternative_spoiler {
        push_unique_name(
            &mut aliases,
            Some(RawNameValue {
                value: alias.clone(),
                locale: None,
                name_type: Some("spoiler_alias".to_string()),
                script_hint: None,
            }),
            &primary_value,
        );
    }

    Some(NameSelection { primary, aliases })
}

fn upsert_person_record(
    people: &mut BTreeMap<String, RawSourceRecord>,
    media: &MediaContext,
    person_id: &str,
    record_uri: Option<String>,
    domain_hint: &str,
    names: &NameFields,
    image_url: Option<String>,
    extra_external_id: Option<RawExternalId>,
    relationship: Option<RawRelationship>,
) {
    let Some(name_selection) = select_person_names(names, "primary") else {
        return;
    };

    let record_id = format!("person-anilist-{person_id}-media-{}", media.media_id);
    let record = people
        .entry(record_id.clone())
        .or_insert_with(|| RawSourceRecord {
            record_id,
            record_uri: record_uri.clone(),
            retrieved_at: media.retrieved_at.clone(),
            entity_kind: super::model::EntityKind::RealPerson,
            domain: domain_hint.to_string(),
            context_key: None,
            primary_name: name_selection.primary.clone(),
            aliases: name_selection.aliases.clone(),
            readings: Vec::new(),
            external_ids: vec![RawExternalId {
                source_name: "anilist".to_string(),
                value: person_id.to_string(),
                uri: record_uri.clone(),
            }],
            relationships: Vec::new(),
            images: Vec::new(),
            fields: BTreeMap::new(),
        });

    if domain_hint == "voice_actor" {
        record.domain = "voice_actor".to_string();
    }
    if record.record_uri.is_none() {
        record.record_uri = record_uri.clone();
    }
    if record.retrieved_at.is_none() {
        record.retrieved_at = media.retrieved_at.clone();
    }

    maybe_promote_primary_name(record, name_selection.primary);
    for alias in name_selection.aliases {
        push_unique_name(&mut record.aliases, Some(alias), &record.primary_name.value);
    }

    if let Some(external_id) = extra_external_id {
        push_unique_external_id(&mut record.external_ids, external_id);
    }
    if let Some(relationship) = relationship {
        push_unique_relationship(&mut record.relationships, relationship);
    }
    push_image_url(&mut record.images, image_url);
    copy_scalar_field(
        &names_to_fields_map(names),
        &mut record.fields,
        "first_name_hint",
        &["first_name_hint"],
    );
    copy_scalar_field(
        &names_to_fields_map(names),
        &mut record.fields,
        "last_name_hint",
        &["last_name_hint"],
    );
}

fn copy_scalar_field(
    row: &Map<String, Value>,
    fields: &mut BTreeMap<String, Value>,
    output_key: &str,
    input_keys: &[&str],
) {
    if let Some(value) = field_value(row, input_keys).cloned() {
        fields.insert(output_key.to_string(), value);
    }
}

fn names_to_fields_map(names: &NameFields) -> Map<String, Value> {
    let mut map = Map::new();
    if let Some(first) = &names.first {
        map.insert("first_name_hint".to_string(), Value::String(first.clone()));
    }
    if let Some(last) = &names.last {
        map.insert("last_name_hint".to_string(), Value::String(last.clone()));
    }
    map
}

fn maybe_promote_primary_name(record: &mut RawSourceRecord, candidate: RawNameValue) {
    if prefers_name(&candidate, &record.primary_name) {
        let previous = std::mem::replace(&mut record.primary_name, candidate);
        push_unique_name(
            &mut record.aliases,
            Some(previous),
            &record.primary_name.value,
        );
    } else {
        push_unique_name(
            &mut record.aliases,
            Some(candidate),
            &record.primary_name.value,
        );
    }
}

fn prefers_name(candidate: &RawNameValue, current: &RawNameValue) -> bool {
    let candidate_score = (
        looks_japanese(&candidate.value),
        candidate.name_type.as_deref() == Some("primary"),
        candidate.value.len(),
    );
    let current_score = (
        looks_japanese(&current.value),
        current.name_type.as_deref() == Some("primary"),
        current.value.len(),
    );
    candidate_score > current_score
}

fn voice_actor_is_japanese(actor: &Map<String, Value>) -> bool {
    string_field(actor, &["languageV2", "language", "language_name"])
        .map(|language| {
            let normalized = language.trim().to_ascii_uppercase();
            normalized == "JAPANESE" || normalized == "日本語"
        })
        .unwrap_or(true)
}

fn normalize_character_role(role: String) -> String {
    match role.trim().to_ascii_uppercase().as_str() {
        "MAIN" => "primary".to_string(),
        "SUPPORTING" => "side".to_string(),
        "BACKGROUND" => "appears".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

fn combine_western_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    match (first.map(str::trim), last.map(str::trim)) {
        (Some(first), Some(last)) if !first.is_empty() && !last.is_empty() => {
            Some(format!("{first} {last}"))
        }
        (Some(first), None) if !first.is_empty() => Some(first.to_string()),
        (None, Some(last)) if !last.is_empty() => Some(last.to_string()),
        _ => None,
    }
}

fn field_value<'a>(row: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| row.get(*key))
        .filter(|value| !value_is_blank(value))
}

fn string_field(row: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    field_value(row, keys).and_then(string_from_value)
}

fn string_array_field(row: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    field_value(row, keys)
        .map(string_array_from_value)
        .unwrap_or_default()
}

fn value_array_field(row: &Map<String, Value>, keys: &[&str]) -> Vec<Value> {
    field_value(row, keys)
        .map(value_array_from_value)
        .unwrap_or_default()
}

fn string_array_from_value(value: &Value) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    for item in value_array_from_value(value) {
        if let Some(value) = string_from_value(&item).filter(|value| !value.is_empty()) {
            if seen.insert(value.clone()) {
                values.push(value);
            }
        }
    }
    if values.is_empty() {
        if let Some(value) = string_from_value(value) {
            let trimmed = value.trim();
            if trimmed.starts_with('[') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return string_array_from_value(&parsed);
                }
            } else if !trimmed.is_empty() {
                return vec![trimmed.to_string()];
            }
        }
    }
    values
}

fn value_array_from_value(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values.clone(),
        Value::Object(object) => {
            if let Some(edges) = object.get("edges").and_then(Value::as_array) {
                return edges.clone();
            }
            if let Some(nodes) = object.get("nodes").and_then(Value::as_array) {
                return nodes.clone();
            }
            vec![value.clone()]
        }
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') || trimmed.starts_with('{') {
                serde_json::from_str::<Value>(trimmed)
                    .ok()
                    .map(|parsed| value_array_from_value(&parsed))
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn image_url_from_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(url) = string_from_value(value) {
        return Some(url);
    }
    let Some(object) = value.as_object() else {
        return None;
    };
    string_field(
        object,
        &[
            "extraLarge",
            "large",
            "medium",
            "bannerImage",
            "url",
            "source",
        ],
    )
}

fn normalized_timestamp(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Number(number) => {
            let timestamp = number.as_i64()?;
            Utc.timestamp_opt(timestamp, 0)
                .single()
                .map(|dt| dt.format("%Y-%m-%d").to_string())
        }
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                None
            } else if let Ok(timestamp) = trimmed.parse::<i64>() {
                Utc.timestamp_opt(timestamp, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .or_else(|| Some(trimmed.to_string()))
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

fn string_from_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}

fn csv_cell_to_value(value: &str) -> Value {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Value::Null
    } else {
        Value::String(trimmed.to_string())
    }
}

fn value_is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(raw) => {
            let trimmed = raw.trim();
            trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null")
        }
        _ => false,
    }
}

fn looks_japanese(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff66..=0xff9f
        )
    })
}

fn maybe_raw_record(value: &Value) -> Option<RawSourceRecord> {
    value
        .get("record_id")
        .is_some()
        .then(|| serde_json::from_value::<RawSourceRecord>(value.clone()).ok())
        .flatten()
}

fn push_unique_name(
    target: &mut Vec<RawNameValue>,
    value: Option<RawNameValue>,
    primary_value: &str,
) {
    let Some(value) = value.filter(|value| !value.value.trim().is_empty()) else {
        return;
    };
    if value.value == primary_value {
        return;
    }
    if target
        .iter()
        .any(|existing| existing.value == value.value && existing.name_type == value.name_type)
    {
        return;
    }
    target.push(value);
}

fn push_unique_external_id(target: &mut Vec<RawExternalId>, value: RawExternalId) {
    if target
        .iter()
        .any(|existing| existing.source_name == value.source_name && existing.value == value.value)
    {
        return;
    }
    target.push(value);
}

fn push_unique_relationship(target: &mut Vec<RawRelationship>, value: RawRelationship) {
    if target.iter().any(|existing| {
        existing.predicate == value.predicate
            && existing.target_source_name == value.target_source_name
            && existing.target_external_id == value.target_external_id
            && existing.context_source_name == value.context_source_name
            && existing.context_external_id == value.context_external_id
    }) {
        return;
    }
    target.push(value);
}

fn push_image_url(target: &mut Vec<RawImage>, url: Option<String>) {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
        return;
    };
    if target
        .iter()
        .any(|image| image.url.as_deref() == Some(url.as_str()))
    {
        return;
    }
    target.push(RawImage {
        url: Some(url),
        local_path: None,
        bytes_base64: None,
        ext: None,
        rights_status: Some(RightsStatus::Unknown),
        width: None,
        height: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_embedded_character_and_staff_rows() {
        let row = serde_json::json!({
            "id": 9253,
            "type": "ANIME",
            "updatedAt": 1609459200,
            "title_native": "シュタインズ・ゲート",
            "title_romaji": "Steins;Gate",
            "synonyms": ["シュタゲ"],
            "characters": [{
                "role": "MAIN",
                "voiceActors": [{
                    "id": 9526,
                    "name": {"full": "Kana Hanazawa", "native": "花澤香菜"}
                }],
                "node": {
                    "id": 35252,
                    "name": {
                        "full": "Rintarou Okabe",
                        "native": "岡部倫太郎",
                        "alternative": ["オカリン"]
                    }
                }
            }],
            "staff": [{
                "role": "Director",
                "node": {
                    "id": 1001,
                    "name": {"full": "Hiroshi Hamasaki", "native": "浜崎博嗣"}
                }
            }]
        });

        let records = media_row_to_records(row.as_object().unwrap()).unwrap();
        assert_eq!(records.len(), 4);
        assert!(records
            .iter()
            .any(|record| record.record_id == "work-anilist-9253"));
        assert!(records
            .iter()
            .any(|record| record.record_id == "char-anilist-35252-media-9253"));
        assert!(records.iter().any(|record| {
            record.record_id == "person-anilist-9526-media-9253" && record.domain == "voice_actor"
        }));
        assert!(records.iter().any(|record| {
            record.record_id == "person-anilist-1001-media-9253"
                && record
                    .relationships
                    .iter()
                    .any(|rel| rel.predicate == "worked_on")
        }));
    }
}
