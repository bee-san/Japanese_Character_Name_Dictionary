use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    content_builder::{ContentBuilder, DictSettings},
    dict_builder::DictBuilder,
    kana,
    models::{Character, CharacterTrait},
    snapshot::source::{RawExternalId, RawImage, RawSourceRecord},
};

#[derive(Debug, Clone)]
pub struct SnapshotYomitanOptions {
    pub settings: DictSettings,
    pub title: Option<String>,
}

impl Default for SnapshotYomitanOptions {
    fn default() -> Self {
        Self {
            settings: DictSettings::default(),
            title: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotYomitanResult {
    pub zip_path: PathBuf,
    pub character_source_records: usize,
    pub character_entries_added: usize,
    pub generic_entries_added: usize,
    pub skipped_no_japanese: usize,
}

#[derive(Debug, Clone)]
struct SourceRecordRow {
    id: String,
    raw: RawSourceRecord,
}

#[derive(Debug, Clone, Default)]
struct ImageInfo {
    path: Option<PathBuf>,
    ext: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    source_url: Option<String>,
}

pub fn export_yomitan_from_snapshot(
    snapshot_input: &Path,
    out_zip: &Path,
    options: SnapshotYomitanOptions,
) -> Result<SnapshotYomitanResult> {
    let (snapshot_path, snapshot_root) = resolve_snapshot_paths(snapshot_input)?;
    let conn = Connection::open(&snapshot_path)
        .with_context(|| format!("failed to open {}", snapshot_path.display()))?;

    let source_records = load_source_records(&conn)?;
    let image_by_source_record = load_image_map(&conn, &snapshot_root)?;

    let mut work_titles_by_external = HashMap::new();
    let mut person_by_external = HashMap::new();
    let mut character_rows = Vec::new();
    let mut generic_rows = Vec::new();

    for row in source_records {
        match row.raw.entity_kind.as_str() {
            "work" => {
                let title = preferred_display_name(&row.raw);
                for external in &row.raw.external_ids {
                    work_titles_by_external.insert(external_key(external), title.clone());
                }
                generic_rows.push(row);
            }
            "real_person" => {
                let image = image_by_source_record
                    .get(&row.id)
                    .cloned()
                    .or(first_image_from_raw(&row.raw, &snapshot_root)?)
                    .unwrap_or_default();
                let person = build_person_card(&row.raw, image);
                for external in &row.raw.external_ids {
                    person_by_external.insert(external_key(external), person.clone());
                }
                generic_rows.push(row);
            }
            "organization" | "place" | "product" => generic_rows.push(row),
            "fictional_character" => character_rows.push(row),
            _ => {}
        }
    }

    let default_title = options
        .title
        .unwrap_or_else(|| "Offline character snapshot".to_string());
    let settings = options.settings;
    let mut builder = DictBuilder::new(settings.clone(), None, default_title);
    let mut character_entries_added = 0usize;

    for row in &character_rows {
        let image = image_by_source_record
            .get(&row.id)
            .cloned()
            .or(first_image_from_raw(&row.raw, &snapshot_root)?)
            .unwrap_or_default();

        let appearances = character_appearances(&row.raw, &work_titles_by_external);
        if appearances.is_empty() {
            let character = build_character(&row.raw, &person_by_external, &image, None)?;
            builder.add_character(&character, "Unknown");
            character_entries_added += 1;
            continue;
        }

        for appearance in appearances {
            let character =
                build_character(&row.raw, &person_by_external, &image, Some(&appearance))?;
            builder.add_character(&character, &appearance.title);
            character_entries_added += 1;
        }
    }

    let generic_entries_added = add_generic_name_rows(&mut builder, &generic_rows, &settings);

    if let Some(parent) = out_zip.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    builder
        .export_file(out_zip)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to write {}", out_zip.display()))?;

    Ok(SnapshotYomitanResult {
        zip_path: out_zip.to_path_buf(),
        character_source_records: character_rows.len(),
        character_entries_added,
        generic_entries_added,
        skipped_no_japanese: builder.skipped_no_japanese_count(),
    })
}

fn resolve_snapshot_paths(input: &Path) -> Result<(PathBuf, PathBuf)> {
    if input.is_dir() {
        let sqlite_path = input.join("snapshot.sqlite");
        if !sqlite_path.exists() {
            bail!(
                "snapshot directory {} is missing snapshot.sqlite",
                input.display()
            );
        }
        return Ok((sqlite_path, input.to_path_buf()));
    }
    let file_name = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if file_name != "snapshot.sqlite" {
        bail!(
            "snapshot input must be a snapshot output directory or snapshot.sqlite, got {}",
            input.display()
        );
    }
    let root = input
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("snapshot path {} has no parent directory", input.display()))?;
    Ok((input.to_path_buf(), root))
}

fn load_source_records(conn: &Connection) -> Result<Vec<SourceRecordRow>> {
    let mut stmt = conn.prepare("SELECT id, payload_json FROM source_record ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let payload_json: String = row.get(1)?;
        Ok((id, payload_json))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, payload_json) = row?;
        let raw = serde_json::from_str::<RawSourceRecord>(&payload_json)
            .with_context(|| format!("failed to parse source_record payload for {id}"))?;
        out.push(SourceRecordRow { id, raw });
    }
    Ok(out)
}

fn load_image_map(conn: &Connection, snapshot_root: &Path) -> Result<HashMap<String, ImageInfo>> {
    let mut stmt = conn.prepare(
        "SELECT source_record_id, relative_path, ext, width, height, source_url
         FROM image_asset
         WHERE source_record_id IS NOT NULL
         ORDER BY source_record_id, id",
    )?;
    let rows = stmt.query_map([], |row| {
        let source_record_id: String = row.get(0)?;
        let relative_path: String = row.get(1)?;
        let ext: String = row.get(2)?;
        let width: Option<i64> = row.get(3)?;
        let height: Option<i64> = row.get(4)?;
        let source_url: Option<String> = row.get(5)?;
        Ok((
            source_record_id,
            relative_path,
            ext,
            width.map(|value| value as u32),
            height.map(|value| value as u32),
            source_url,
        ))
    })?;

    let mut out = HashMap::new();
    for row in rows {
        let (source_record_id, relative_path, ext, width, height, source_url) = row?;
        let entry = out
            .entry(source_record_id)
            .or_insert_with(ImageInfo::default);
        if entry.path.is_some() {
            continue;
        }
        let full_path = snapshot_root.join(&relative_path);
        if full_path.exists() {
            entry.path = Some(full_path);
            entry.ext = Some(ext);
            entry.width = width;
            entry.height = height;
            entry.source_url = source_url;
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct PersonCard {
    display_name: String,
    image: ImageInfo,
}

fn build_person_card(raw: &RawSourceRecord, image: ImageInfo) -> PersonCard {
    PersonCard {
        display_name: preferred_display_name(raw),
        image,
    }
}

#[derive(Debug, Clone)]
struct Appearance {
    title: String,
    role: String,
    context_external_key: String,
}

fn character_appearances(
    raw: &RawSourceRecord,
    work_titles_by_external: &HashMap<String, String>,
) -> Vec<Appearance> {
    let mut appearances = Vec::new();
    for relationship in raw
        .relationships
        .iter()
        .filter(|relationship| relationship.predicate == "appears_in")
    {
        let key = format!(
            "{}:{}",
            relationship.target_source_name, relationship.target_external_id
        );
        let title = work_titles_by_external
            .get(&key)
            .cloned()
            .unwrap_or_else(|| relationship.target_external_id.clone());
        let role = role_for_context(raw, &key);
        appearances.push(Appearance {
            title,
            role,
            context_external_key: key,
        });
    }
    appearances
}

fn build_character(
    raw: &RawSourceRecord,
    people_by_external: &HashMap<String, PersonCard>,
    image: &ImageInfo,
    appearance: Option<&Appearance>,
) -> Result<Character> {
    let (name, name_original, first_name_hint, last_name_hint) = choose_character_names(raw);

    let mut character = Character {
        id: primary_external_value(raw).unwrap_or_else(|| raw.record_id.clone()),
        name,
        name_original,
        role: appearance
            .map(|appearance| appearance.role.clone())
            .unwrap_or_else(|| default_character_role(raw)),
        source: primary_external_source(raw).unwrap_or_else(|| "snapshot".to_string()),
        sex: normalize_sex(field_string(&raw.fields, &["sex", "gender"])),
        age: field_string(&raw.fields, &["age"]),
        height: field_u32(&raw.fields, &["height_cm", "height"]),
        weight: field_u32(&raw.fields, &["weight_kg", "weight"]),
        blood_type: field_string(&raw.fields, &["blood_type"]),
        birthday: field_birthday(&raw.fields),
        description: field_string(&raw.fields, &["description"]),
        aliases: raw
            .aliases
            .iter()
            .filter(|alias| alias.name_type.as_deref() != Some("spoiler_alias"))
            .map(|alias| alias.value.clone())
            .collect(),
        spoiler_aliases: raw
            .aliases
            .iter()
            .filter(|alias| alias.name_type.as_deref() == Some("spoiler_alias"))
            .map(|alias| alias.value.clone())
            .collect(),
        personality: field_traits(&raw.fields, "personality"),
        roles: field_traits(&raw.fields, "roles"),
        engages_in: field_traits(&raw.fields, "engages_in"),
        subject_of: field_traits(&raw.fields, "subject_of"),
        image_url: image.source_url.clone(),
        image_bytes: load_image_bytes_from_info(image)?,
        image_ext: image.ext.clone(),
        image_width: image.width,
        image_height: image.height,
        first_name_hint,
        last_name_hint,
        seiyuu: None,
        seiyuu_image_url: None,
        seiyuu_image_bytes: None,
        seiyuu_image_ext: None,
        seiyuu_image_width: None,
        seiyuu_image_height: None,
    };

    if let Some((name, voice_image)) = choose_seiyuu(raw, people_by_external, appearance) {
        character.seiyuu = Some(name);
        character.seiyuu_image_url = voice_image.source_url.clone();
        character.seiyuu_image_bytes = load_image_bytes_from_info(&voice_image)?;
        character.seiyuu_image_ext = voice_image.ext.clone();
        character.seiyuu_image_width = voice_image.width;
        character.seiyuu_image_height = voice_image.height;
    }

    Ok(character)
}

fn choose_character_names(
    raw: &RawSourceRecord,
) -> (String, String, Option<String>, Option<String>) {
    let mut japanese_names = Vec::new();
    let mut latin_names = Vec::new();

    push_name_bucket(
        &mut japanese_names,
        &mut latin_names,
        &raw.primary_name.value,
    );
    for alias in &raw.aliases {
        push_name_bucket(&mut japanese_names, &mut latin_names, &alias.value);
    }

    let name_original = japanese_names
        .first()
        .cloned()
        .unwrap_or_else(|| raw.primary_name.value.clone());
    let name = latin_names
        .first()
        .cloned()
        .or_else(|| {
            (!looks_japanese(&raw.primary_name.value)).then(|| raw.primary_name.value.clone())
        })
        .unwrap_or_else(|| name_original.clone());

    (
        name,
        name_original,
        field_string(&raw.fields, &["first_name_hint"]),
        field_string(&raw.fields, &["last_name_hint"]),
    )
}

fn push_name_bucket(japanese: &mut Vec<String>, latin: &mut Vec<String>, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    if looks_japanese(value) {
        if !japanese.iter().any(|existing| existing == value) {
            japanese.push(value.to_string());
        }
    } else if !latin.iter().any(|existing| existing == value) {
        latin.push(value.to_string());
    }
}

fn default_character_role(raw: &RawSourceRecord) -> String {
    normalize_role(
        field_string(&raw.fields, &["role"])
            .unwrap_or_else(|| "appears".to_string())
            .as_str(),
    )
}

fn role_for_context(raw: &RawSourceRecord, context_external_key: &str) -> String {
    if let Some(appearances) = raw.fields.get("appearances").and_then(Value::as_array) {
        for appearance in appearances {
            let Some(vn_id) = appearance.get("vn_id").and_then(value_as_string) else {
                continue;
            };
            let source_name = primary_external_source(raw).unwrap_or_else(|| "vndb".to_string());
            let candidate_key = format!("{source_name}:{vn_id}");
            if candidate_key == context_external_key {
                if let Some(role) = appearance.get("role").and_then(value_as_string) {
                    return normalize_role(&role);
                }
            }
        }
    }
    default_character_role(raw)
}

fn choose_seiyuu(
    raw: &RawSourceRecord,
    people_by_external: &HashMap<String, PersonCard>,
    appearance: Option<&Appearance>,
) -> Option<(String, ImageInfo)> {
    let preferred_context = appearance.map(|appearance| appearance.context_external_key.clone());
    for relationship in raw
        .relationships
        .iter()
        .filter(|relationship| relationship.predicate == "voiced_by")
    {
        let key = format!(
            "{}:{}",
            relationship.target_source_name, relationship.target_external_id
        );
        let person = people_by_external.get(&key)?;
        if let Some(preferred_context) = &preferred_context {
            let relationship_context = relationship
                .context_source_name
                .as_ref()
                .zip(relationship.context_external_id.as_ref())
                .map(|(source_name, external_id)| format!("{source_name}:{external_id}"));
            if relationship_context.as_ref() != Some(preferred_context) {
                continue;
            }
        }
        return Some((person.display_name.clone(), person.image.clone()));
    }

    raw.relationships
        .iter()
        .filter(|relationship| relationship.predicate == "voiced_by")
        .find_map(|relationship| {
            let key = format!(
                "{}:{}",
                relationship.target_source_name, relationship.target_external_id
            );
            people_by_external
                .get(&key)
                .map(|person| (person.display_name.clone(), person.image.clone()))
        })
}

fn primary_external_source(raw: &RawSourceRecord) -> Option<String> {
    raw.external_ids
        .first()
        .map(|external| external.source_name.clone())
}

fn primary_external_value(raw: &RawSourceRecord) -> Option<String> {
    raw.external_ids
        .first()
        .map(|external| external.value.clone())
}

fn preferred_display_name(raw: &RawSourceRecord) -> String {
    if looks_japanese(&raw.primary_name.value) {
        return raw.primary_name.value.clone();
    }
    raw.aliases
        .iter()
        .find(|alias| looks_japanese(&alias.value))
        .map(|alias| alias.value.clone())
        .unwrap_or_else(|| raw.primary_name.value.clone())
}

fn external_key(external: &RawExternalId) -> String {
    format!("{}:{}", external.source_name, external.value)
}

fn normalize_role(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "main" | "protagonist" => "main".to_string(),
        "primary" | "main_character" => "primary".to_string(),
        "side" | "supporting" => "side".to_string(),
        _ => "appears".to_string(),
    }
}

fn normalize_sex(value: Option<String>) -> Option<String> {
    value.and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
        "m" | "male" => Some("m".to_string()),
        "f" | "female" => Some("f".to_string()),
        _ => None,
    })
}

fn field_string(fields: &BTreeMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        fields
            .get(*key)
            .and_then(value_as_string)
            .and_then(|value| {
                let trimmed = value.trim().to_string();
                (!trimmed.is_empty() && trimmed != "null").then_some(trimmed)
            })
    })
}

fn field_u32(fields: &BTreeMap<String, Value>, keys: &[&str]) -> Option<u32> {
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(value_as_u32))
}

fn field_birthday(fields: &BTreeMap<String, Value>) -> Option<Vec<u32>> {
    if let Some(date) = fields.get("date_of_birth").and_then(Value::as_object) {
        let month = date.get("month").and_then(value_as_u32)?;
        let day = date.get("day").and_then(value_as_u32)?;
        return Some(vec![month, day]);
    }
    None
}

fn field_traits(fields: &BTreeMap<String, Value>, category: &str) -> Vec<CharacterTrait> {
    let Some(traits) = fields.get("traits").and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(values) = traits.get(category).and_then(Value::as_array) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            let name = object.get("name").and_then(value_as_string)?;
            let spoiler = object.get("spoiler").and_then(value_as_u32).unwrap_or(0) as u8;
            Some(CharacterTrait { name, spoiler })
        })
        .collect()
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_as_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(value) => value.as_u64().map(|value| value as u32),
        Value::String(value) => value.parse::<u32>().ok(),
        _ => None,
    }
}

fn load_image_bytes_from_info(image: &ImageInfo) -> Result<Option<Vec<u8>>> {
    let Some(path) = &image.path else {
        return Ok(None);
    };
    let bytes =
        fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    Ok(Some(bytes))
}

fn first_image_from_raw(raw: &RawSourceRecord, snapshot_root: &Path) -> Result<Option<ImageInfo>> {
    for image in &raw.images {
        if let Some(info) = image_info_from_raw(image, snapshot_root)? {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

fn add_generic_name_rows(
    builder: &mut DictBuilder,
    rows: &[SourceRecordRow],
    settings: &DictSettings,
) -> usize {
    rows.iter()
        .map(|row| add_generic_name_row(builder, &row.raw, settings))
        .sum()
}

fn add_generic_name_row(
    builder: &mut DictBuilder,
    raw: &RawSourceRecord,
    settings: &DictSettings,
) -> usize {
    let content = build_generic_content(raw, settings);
    let score = generic_entry_score(raw);
    builder.add_prebuilt_entries_with_optional_honorifics(
        &generic_term_readings(raw),
        "",
        score,
        &content,
    )
}

fn generic_term_readings(raw: &RawSourceRecord) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let primary_name_key = compact_name_key(&raw.primary_name.value);
    let global_readings: Vec<String> = raw
        .readings
        .iter()
        .map(|reading| normalize_term_reading(&reading.value))
        .filter(|reading| !reading.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut japanese_names = Vec::new();
    push_generic_name(&mut japanese_names, &raw.primary_name.value);
    for alias in &raw.aliases {
        push_generic_name(&mut japanese_names, &alias.value);
    }

    for term in japanese_names {
        let mut readings = raw
            .readings
            .iter()
            .filter(|reading| {
                reading
                    .for_name
                    .as_ref()
                    .map(|for_name| compact_name_key(for_name) == compact_name_key(&term))
                    .unwrap_or(false)
            })
            .map(|reading| normalize_term_reading(&reading.value))
            .filter(|reading| !reading.is_empty())
            .collect::<BTreeSet<_>>();

        if readings.is_empty() && compact_name_key(&term) == primary_name_key {
            for reading in raw
                .readings
                .iter()
                .filter(|reading| reading.for_name.is_none())
            {
                let normalized = normalize_term_reading(&reading.value);
                if !normalized.is_empty() {
                    readings.insert(normalized);
                }
            }
        }

        if readings.is_empty() && is_self_reading_japanese(&term) {
            readings.insert(normalize_term_reading(&term));
        }

        if readings.is_empty() && global_readings.len() == 1 {
            readings.insert(global_readings[0].clone());
        }

        if readings.is_empty() {
            out.push((term.clone(), String::new()));
            continue;
        }

        for reading in readings {
            out.push((term.clone(), reading));
        }
    }

    out
}

fn push_generic_name(names: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() || !looks_japanese(trimmed) {
        return;
    }
    if !names.iter().any(|existing| existing == trimmed) {
        names.push(trimmed.to_string());
    }
}

fn compact_name_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
}

fn normalize_term_reading(value: &str) -> String {
    kana::kata_to_hira(value)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn is_self_reading_japanese(value: &str) -> bool {
    looks_japanese(value) && !kana::contains_kanji(value)
}

fn generic_entry_score(raw: &RawSourceRecord) -> i32 {
    match raw.entity_kind.as_str() {
        "real_person" => 80,
        "work" => 60,
        "organization" => 55,
        "place" => 50,
        "product" => 45,
        _ => 40,
    }
}

fn build_generic_content(raw: &RawSourceRecord, settings: &DictSettings) -> Value {
    let display_name = preferred_display_name(raw);
    let mut blocks = vec![json!({
        "tag": "div",
        "style": { "fontWeight": "bold", "fontSize": "1.2em" },
        "content": display_name
    })];

    if let Some(latin_name) = preferred_latin_name(raw) {
        if latin_name != raw.primary_name.value && latin_name != preferred_display_name(raw) {
            blocks.push(json!({
                "tag": "div",
                "style": { "color": "#666666" },
                "content": latin_name
            }));
        }
    }

    let mut meta = vec![format!(
        "Type: {}",
        entity_kind_label(raw.entity_kind.as_str())
    )];
    if !raw.domain.trim().is_empty() {
        meta.push(format!("Domain: {}", raw.domain));
    }
    if let Some(source_name) = primary_external_source(raw) {
        meta.push(format!("Source: {}", source_name));
    }
    blocks.push(json!({
        "tag": "div",
        "style": { "marginTop": "0.35em" },
        "content": meta.join(" | ")
    }));

    if let Some(readings_line) = generic_display_readings(raw) {
        blocks.push(json!({
            "tag": "div",
            "style": { "marginTop": "0.35em" },
            "content": format!("Reading: {}", readings_line)
        }));
    }

    if let Some(name_types) = field_string_list(&raw.fields, "name_types") {
        blocks.push(json!({
            "tag": "div",
            "style": { "marginTop": "0.35em" },
            "content": format!("Kinds: {}", name_types.join(", "))
        }));
    }

    if let Some(glosses) = field_string_list(&raw.fields, "translations") {
        blocks.push(json!({
            "tag": "div",
            "style": { "marginTop": "0.35em" },
            "content": format!("Gloss: {}", glosses.join("; "))
        }));
    }

    let non_japanese_aliases = raw
        .aliases
        .iter()
        .filter(|alias| !looks_japanese(&alias.value))
        .map(|alias| alias.value.clone())
        .collect::<Vec<_>>();
    if !non_japanese_aliases.is_empty() {
        blocks.push(json!({
            "tag": "div",
            "style": { "marginTop": "0.35em" },
            "content": format!("Aliases: {}", non_japanese_aliases.join(", "))
        }));
    }

    if settings.show_description {
        if let Some(description) = field_string(&raw.fields, &["description"]) {
            let description = if settings.show_spoilers {
                description
            } else {
                ContentBuilder::strip_spoilers(&description)
            };
            let description = ContentBuilder::parse_vndb_markup(&description);
            if !description.trim().is_empty() {
                blocks.push(json!({
                    "tag": "div",
                    "style": { "marginTop": "0.5em" },
                    "content": description
                }));
            }
        }
    }

    json!({
        "tag": "div",
        "content": blocks
    })
}

fn preferred_latin_name(raw: &RawSourceRecord) -> Option<String> {
    if !looks_japanese(&raw.primary_name.value) {
        return Some(raw.primary_name.value.clone());
    }
    raw.aliases
        .iter()
        .find(|alias| !looks_japanese(&alias.value))
        .map(|alias| alias.value.clone())
}

fn field_string_list(fields: &BTreeMap<String, Value>, key: &str) -> Option<Vec<String>> {
    let values = fields.get(key)?.as_array()?;
    let values = values
        .iter()
        .filter_map(value_as_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn generic_display_readings(raw: &RawSourceRecord) -> Option<String> {
    let readings = raw
        .readings
        .iter()
        .map(|reading| normalize_term_reading(&reading.value))
        .filter(|reading| !reading.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !readings.is_empty() {
        return Some(readings.join(", "));
    }
    if is_self_reading_japanese(&preferred_display_name(raw)) {
        return Some(normalize_term_reading(&preferred_display_name(raw)));
    }
    None
}

fn entity_kind_label(entity_kind: &str) -> &'static str {
    match entity_kind {
        "real_person" => "Person",
        "fictional_character" => "Character",
        "organization" => "Organization",
        "work" => "Work",
        "product" => "Product",
        "place" => "Place",
        _ => "Name",
    }
}

fn image_info_from_raw(image: &RawImage, snapshot_root: &Path) -> Result<Option<ImageInfo>> {
    if let Some(local_path) = &image.local_path {
        let path = if Path::new(local_path).is_absolute() {
            PathBuf::from(local_path)
        } else {
            snapshot_root.join(local_path)
        };
        if path.exists() {
            return Ok(Some(ImageInfo {
                path: Some(path),
                ext: image.ext.clone(),
                width: image.width,
                height: image.height,
                source_url: image.url.clone(),
            }));
        }
    }
    if let Some(bytes_base64) = &image.bytes_base64 {
        let bytes = STANDARD
            .decode(bytes_base64)
            .context("invalid image bytes_base64 payload")?;
        let ext = image.ext.clone().unwrap_or_else(|| "bin".to_string());
        let image_dir = snapshot_root.join(".snapshot_yomitan_cache");
        fs::create_dir_all(&image_dir)
            .with_context(|| format!("failed to create {}", image_dir.display()))?;
        let path = image_dir.join(format!("embedded-{}.{}", fxhash(bytes_base64), ext));
        if !path.exists() {
            fs::write(&path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        return Ok(Some(ImageInfo {
            path: Some(path),
            ext: image.ext.clone(),
            width: image.width,
            height: image.height,
            source_url: image.url.clone(),
        }));
    }
    Ok(None)
}

fn fxhash(value: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn looks_japanese(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff66..=0xff9f
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;
    use zip::ZipArchive;

    use crate::snapshot::pipeline::build_snapshot;

    #[test]
    fn exports_yomitan_zip_from_snapshot_fixture() {
        let fixture_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ultimate_snapshot/config.toml");
        let temp = tempdir().unwrap();
        let snapshot_out = temp.path().join("snapshot");
        build_snapshot(&fixture_config, &snapshot_out).unwrap();

        let zip_path = temp.path().join("dict.zip");
        let result = export_yomitan_from_snapshot(
            &snapshot_out,
            &zip_path,
            SnapshotYomitanOptions::default(),
        )
        .unwrap();
        assert!(result.zip_path.exists());
        assert!(result.character_source_records >= 1);
        assert!(result.generic_entries_added >= 1);

        let bytes = fs::read(&zip_path).unwrap();
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut index = String::new();
        archive
            .by_name("index.json")
            .unwrap()
            .read_to_string(&mut index)
            .unwrap();
        assert!(index.contains("Bee's Character Dictionary"));

        let mut term_bank = String::new();
        archive
            .by_name("term_bank_1.json")
            .unwrap()
            .read_to_string(&mut term_bank)
            .unwrap();
        assert!(term_bank.contains("岡部倫太郎") || term_bank.contains("岡部 倫太郎"));
        assert!(term_bank.contains("岡部さん"));
        assert!(term_bank.contains("花澤香菜"));
    }
}
