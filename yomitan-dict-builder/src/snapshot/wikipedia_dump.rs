use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use super::{
    model::{EntityKind, ScriptKind},
    normalize::detect_script,
    source::{RawExternalId, RawNameValue, RawReading, RawSourceRecord},
};

pub fn load_wikipedia_dump_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let values = load_values(path)?;
    values_to_records(values)
}

fn load_values(path: &Path) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    let mut parsed_any = false;
    let mut fallback_to_full_parse = false;

    let reader = open_reader(path)?;
    for (line_idx, line) in reader.lines().enumerate() {
        let line = line
            .with_context(|| format!("failed to read {} line {}", path.display(), line_idx + 1))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let candidate = trimmed.trim_end_matches(',').trim();
        if !(candidate.starts_with('{') || candidate.starts_with('[')) {
            continue;
        }

        match serde_json::from_str::<Value>(candidate) {
            Ok(value) => {
                parsed_any = true;
                values.extend(expand_root_value(value)?);
            }
            Err(_) if !parsed_any => {
                fallback_to_full_parse = true;
                break;
            }
            Err(error) => {
                return Err(anyhow!(
                    "invalid Wikipedia JSON in {} line {}: {error}",
                    path.display(),
                    line_idx + 1
                ));
            }
        }
    }

    if parsed_any && !fallback_to_full_parse {
        return Ok(values);
    }

    let text = read_text(path)?;
    let root: Value = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse Wikipedia JSON root {}", path.display()))?;
    expand_root_value(root)
}

fn open_reader(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

fn read_text(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        GzDecoder::new(file)
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to decompress {}", path.display()))?;
    } else {
        File::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read {}", path.display()))?;
    }
    String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn expand_root_value(value: Value) -> Result<Vec<Value>> {
    match value {
        Value::Array(rows) => Ok(rows),
        Value::Object(object) => {
            if let Some(records) = try_raw_source_records(&Value::Object(object.clone())) {
                return Ok(records
                    .into_iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?);
            }
            for key in ["pages", "rows", "data", "items"] {
                if let Some(value) = object.get(key) {
                    return expand_root_value(value.clone());
                }
            }
            Ok(vec![Value::Object(object)])
        }
        _ => Err(anyhow!(
            "Wikipedia input must be JSON rows, arrays, or an object root"
        )),
    }
}

fn values_to_records(values: Vec<Value>) -> Result<Vec<RawSourceRecord>> {
    let mut out = Vec::new();
    for value in values {
        if let Some(records) = try_raw_source_records(&value) {
            out.extend(records);
            continue;
        }

        match value {
            Value::Array(rows) => out.extend(values_to_records(rows)?),
            Value::Object(object) => {
                if let Some(record) = row_to_record(&object)? {
                    out.push(record);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn try_raw_source_records(value: &Value) -> Option<Vec<RawSourceRecord>> {
    if let Ok(records) = serde_json::from_value::<Vec<RawSourceRecord>>(value.clone()) {
        return Some(records);
    }
    value
        .get("records")
        .and_then(|records| serde_json::from_value::<Vec<RawSourceRecord>>(records.clone()).ok())
}

fn row_to_record(object: &Map<String, Value>) -> Result<Option<RawSourceRecord>> {
    if bool_field(object, &["is_disambiguation", "disambiguation"])
        || category_matches(object, &["曖昧さ回避", "disambiguation pages"])
    {
        return Ok(None);
    }

    let namespace = integer_field(object, &["namespace", "ns"]);
    if namespace.is_some_and(|namespace| namespace != 0) {
        return Ok(None);
    }

    let raw_title = string_field(
        object,
        &[
            "display_title",
            "displaytitle",
            "title",
            "page_title",
            "name",
        ],
    )
    .ok_or_else(|| anyhow!("Wikipedia row missing title"))?;
    let primary_title = normalize_title(&raw_title);
    if primary_title.is_empty() {
        return Ok(None);
    }

    let language = string_field(object, &["language", "lang"])
        .or_else(|| {
            string_field(object, &["wiki"]).map(|wiki| {
                wiki.trim_end_matches("wiki")
                    .trim_end_matches("wikiquote")
                    .to_string()
            })
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ja".to_string());
    let locale = Some(language.clone());
    let entity_kind = classify_entity_kind(object);
    let source_name = format!("wikipedia_{language}");
    let record_id = string_field(object, &["page_id", "id"])
        .map(|value| format!("wikipedia-{language}-{value}"))
        .unwrap_or_else(|| format!("wikipedia-{language}-{}", primary_title.replace(' ', "_")));
    let page_url = string_field(object, &["url", "uri"]);

    let primary_name = RawNameValue {
        value: primary_title.clone(),
        locale: locale.clone(),
        name_type: Some("primary".to_string()),
        script_hint: Some(detect_script_or_none(&primary_title)),
    };

    let mut aliases = Vec::new();
    let mut seen_aliases = BTreeSet::new();

    if raw_title != primary_title {
        push_alias(
            &mut aliases,
            &mut seen_aliases,
            RawNameValue {
                value: raw_title,
                locale: locale.clone(),
                name_type: Some("raw_title".to_string()),
                script_hint: None,
            },
        );
    }

    for key in [
        "redirects",
        "redirect_titles",
        "aliases",
        "other_titles",
        "synonyms",
    ] {
        for alias in string_list_field(object, key) {
            push_alias(
                &mut aliases,
                &mut seen_aliases,
                RawNameValue {
                    value: normalize_title(&alias),
                    locale: locale.clone(),
                    name_type: Some("redirect".to_string()),
                    script_hint: None,
                },
            );
        }
    }

    for alias in string_list_field_from_object(object, "titles") {
        push_alias(
            &mut aliases,
            &mut seen_aliases,
            RawNameValue {
                value: normalize_title(&alias),
                locale: locale.clone(),
                name_type: Some("alternate_title".to_string()),
                script_hint: None,
            },
        );
    }

    let mut readings = Vec::new();
    let mut seen_readings = BTreeSet::new();
    for reading in reading_values(object) {
        let key = format!("{}|{}", primary_title, reading.value);
        if seen_readings.insert(key) {
            readings.push(reading);
        }
    }

    let mut external_ids = vec![RawExternalId {
        source_name,
        value: primary_title.clone(),
        uri: page_url.clone(),
    }];
    if let Some(wikidata_id) = string_field(object, &["wikidata_id", "wikibase_item", "wikidata"]) {
        external_ids.push(RawExternalId {
            source_name: "wikidata".to_string(),
            value: wikidata_id,
            uri: None,
        });
    }

    let mut fields = BTreeMap::new();
    fields.insert("language".to_string(), Value::String(language));
    if let Some(description) = string_field(object, &["description", "extract", "summary"]) {
        fields.insert("description".to_string(), Value::String(description));
    }
    let categories = categories(object);
    if !categories.is_empty() {
        fields.insert(
            "categories".to_string(),
            Value::Array(categories.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(page_id) = integer_field(object, &["page_id", "id"]) {
        fields.insert(
            "page_id".to_string(),
            Value::Number(serde_json::Number::from(page_id)),
        );
    }

    Ok(Some(RawSourceRecord {
        record_id,
        record_uri: page_url,
        retrieved_at: None,
        entity_kind,
        domain: format!(
            "wikipedia_{locale}",
            locale = locale.clone().unwrap_or_else(|| "und".to_string())
        ),
        context_key: Some(format!(
            "wikipedia:{}:{}",
            locale.clone().unwrap_or_else(|| "und".to_string()),
            primary_title
        )),
        primary_name,
        aliases,
        readings,
        external_ids,
        relationships: Vec::new(),
        images: Vec::new(),
        fields,
    }))
}

fn classify_entity_kind(object: &Map<String, Value>) -> EntityKind {
    if let Some(kind) = string_field(object, &["entity_kind", "kind", "page_type"]) {
        if let Some(kind) = parse_entity_kind_string(&kind) {
            return kind;
        }
    }

    let categories = categories(object).join(" ");
    let description =
        string_field(object, &["description", "extract", "summary"]).unwrap_or_default();
    let combined = format!("{categories} {description}");
    if combined.contains("架空")
        || combined.contains("登場人物")
        || combined.to_ascii_lowercase().contains("fictional")
        || combined.contains("キャラクター")
    {
        EntityKind::FictionalCharacter
    } else if combined.contains("人物")
        || combined.contains("声優")
        || combined.contains("作家")
        || combined.contains("監督")
        || combined.to_ascii_lowercase().contains("writer")
        || combined.to_ascii_lowercase().contains("actor")
        || combined.to_ascii_lowercase().contains("director")
    {
        EntityKind::RealPerson
    } else if combined.contains("企業")
        || combined.contains("団体")
        || combined.contains("法人")
        || combined.contains("放送局")
        || combined.to_ascii_lowercase().contains("organization")
        || combined.to_ascii_lowercase().contains("company")
    {
        EntityKind::Organization
    } else if combined.contains("都道府県")
        || combined.contains("市")
        || combined.contains("町")
        || combined.contains("村")
        || combined.contains("島")
        || combined.to_ascii_lowercase().contains("city")
        || combined.to_ascii_lowercase().contains("island")
        || combined.to_ascii_lowercase().contains("prefecture")
    {
        EntityKind::Place
    } else if combined.contains("製品")
        || combined.contains("ソフトウェア")
        || combined.contains("ブランド")
        || combined.to_ascii_lowercase().contains("product")
        || combined.to_ascii_lowercase().contains("software")
    {
        EntityKind::Product
    } else {
        EntityKind::Work
    }
}

fn parse_entity_kind_string(value: &str) -> Option<EntityKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "person" | "real_person" | "human" => Some(EntityKind::RealPerson),
        "fictional_character" | "character" => Some(EntityKind::FictionalCharacter),
        "organization" | "org" | "company" => Some(EntityKind::Organization),
        "place" | "location" => Some(EntityKind::Place),
        "product" | "software" | "brand" => Some(EntityKind::Product),
        "work" | "media" => Some(EntityKind::Work),
        _ => None,
    }
}

fn categories(object: &Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["categories", "category_titles"] {
        out.extend(string_list_field(object, key));
    }
    out
}

fn category_matches(object: &Map<String, Value>, needles: &[&str]) -> bool {
    let categories = categories(object);
    categories.iter().any(|category| {
        let lowered = category.to_ascii_lowercase();
        needles
            .iter()
            .any(|needle| lowered.contains(&needle.to_ascii_lowercase()))
    })
}

fn reading_values(object: &Map<String, Value>) -> Vec<RawReading> {
    let primary = string_field(
        object,
        &[
            "display_title",
            "displaytitle",
            "title",
            "page_title",
            "name",
        ],
    )
    .map(|value| normalize_title(&value));
    let for_name = primary.filter(|value| !value.is_empty());
    let mut out = Vec::new();

    if let Some(kana) = string_field(object, &["kana", "reading"]) {
        out.push(RawReading {
            value: kana,
            for_name: for_name.clone(),
            reading_type: Some("kana".to_string()),
        });
    }

    if let Some(readings) = object.get("readings").and_then(Value::as_array) {
        for value in readings {
            match value {
                Value::String(value) => out.push(RawReading {
                    value: value.trim().to_string(),
                    for_name: for_name.clone(),
                    reading_type: Some("kana".to_string()),
                }),
                Value::Object(object) => {
                    let Some(value) = object
                        .get("value")
                        .and_then(Value::as_str)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                    else {
                        continue;
                    };
                    out.push(RawReading {
                        value,
                        for_name: object
                            .get("for_name")
                            .and_then(Value::as_str)
                            .map(|value| value.to_string())
                            .or_else(|| for_name.clone()),
                        reading_type: object
                            .get("reading_type")
                            .and_then(Value::as_str)
                            .map(|value| value.to_string())
                            .or_else(|| Some("kana".to_string())),
                    });
                }
                _ => {}
            }
        }
    }

    out
}

fn string_list_field(object: &Map<String, Value>, key: &str) -> Vec<String> {
    let Some(values) = object.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match value {
            Value::String(value) => {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_string())
            }
            _ => None,
        })
        .collect()
}

fn string_list_field_from_object(object: &Map<String, Value>, key: &str) -> Vec<String> {
    let Some(values) = object.get(key).and_then(Value::as_object) else {
        return Vec::new();
    };

    values
        .values()
        .flat_map(|value| match value {
            Value::String(value) => vec![value.clone()],
            Value::Array(values) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::String(value) => {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_string())
            }
            _ => None,
        })
    })
}

fn integer_field(object: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::Number(number) => number.as_i64(),
            Value::String(value) => value.parse().ok(),
            _ => None,
        })
    })
}

fn bool_field(object: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        object.get(*key).is_some_and(|value| match value {
            Value::Bool(value) => *value,
            Value::String(value) => matches!(value.trim(), "true" | "1" | "yes"),
            _ => false,
        })
    })
}

fn normalize_title(value: &str) -> String {
    value.trim().replace('_', " ")
}

fn detect_script_or_none(value: &str) -> ScriptKind {
    let script = detect_script(value);
    if script == ScriptKind::Unknown {
        ScriptKind::Mixed
    } else {
        script
    }
}

fn push_alias(aliases: &mut Vec<RawNameValue>, seen: &mut BTreeSet<String>, alias: RawNameValue) {
    if alias.value.is_empty() {
        return;
    }
    let key = format!(
        "{}|{}",
        alias.name_type.clone().unwrap_or_default(),
        alias.value
    );
    if seen.insert(key) {
        aliases.push(alias);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wikipedia_rows_into_snapshot_records() {
        let value = serde_json::json!([
            {
                "title": "吾輩は猫である",
                "language": "ja",
                "page_id": 100,
                "page_type": "work",
                "redirects": ["我輩は猫である"],
                "wikidata_id": "Q333",
                "categories": ["日本の小説", "1905年の小説"],
                "url": "https://ja.wikipedia.org/wiki/%E5%90%BE%E8%BC%A9%E3%81%AF%E7%8C%AB%E3%81%A7%E3%81%82%E3%82%8B"
            },
            {
                "title": "ドラえもん",
                "language": "ja",
                "page_id": 101,
                "page_type": "fictional_character",
                "redirects": ["ドラエもん"],
                "readings": ["ドラえもん"],
                "categories": ["架空のネコ", "漫画の登場人物"]
            }
        ]);

        let rows = expand_root_value(value).unwrap();
        let records = values_to_records(rows).unwrap();
        assert_eq!(records.len(), 2);

        let work = records
            .iter()
            .find(|record| record.primary_name.value == "吾輩は猫である")
            .unwrap();
        assert_eq!(work.entity_kind, EntityKind::Work);
        assert!(work
            .aliases
            .iter()
            .any(|alias| alias.value == "我輩は猫である"));
        assert!(work
            .external_ids
            .iter()
            .any(|external_id| external_id.source_name == "wikidata"));

        let character = records
            .iter()
            .find(|record| record.primary_name.value == "ドラえもん")
            .unwrap();
        assert_eq!(character.entity_kind, EntityKind::FictionalCharacter);
        assert!(character
            .readings
            .iter()
            .any(|reading| reading.value == "ドラえもん"));
    }
}
