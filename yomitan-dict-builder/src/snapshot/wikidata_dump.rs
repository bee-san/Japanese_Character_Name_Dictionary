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

const LANG_PRIORITY: &[&str] = &["ja", "ja-hira", "ja-kana", "ja-latn", "en"];

#[derive(Debug, Clone)]
struct LocalizedValue {
    lang: String,
    value: String,
}

pub fn load_wikidata_dump_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
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
        if trimmed.is_empty() || trimmed == "[" || trimmed == "]" || trimmed == "," {
            continue;
        }

        let candidate = trimmed.trim_end_matches(',').trim();
        if candidate.is_empty() {
            continue;
        }

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
                    "invalid Wikidata JSON in {} line {}: {error}",
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
        .with_context(|| format!("failed to parse Wikidata JSON root {}", path.display()))?;
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

            for key in ["entities", "items", "rows", "data"] {
                if let Some(value) = object.get(key) {
                    return expand_root_value(value.clone());
                }
            }
            Ok(vec![Value::Object(object)])
        }
        _ => Err(anyhow!(
            "Wikidata input must be line-oriented JSON objects, an array, or an object root"
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
                if looks_like_wikidata_entity(&object) {
                    out.push(entity_to_record(&object)?);
                } else {
                    for key in ["entity", "item", "row"] {
                        if let Some(value) = object.get(key) {
                            out.extend(values_to_records(vec![value.clone()])?);
                            break;
                        }
                    }
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

fn looks_like_wikidata_entity(object: &Map<String, Value>) -> bool {
    object.get("id").and_then(Value::as_str).is_some()
        && (object.contains_key("labels")
            || object.contains_key("aliases")
            || object.contains_key("claims")
            || object.contains_key("sitelinks")
            || object.contains_key("descriptions"))
}

fn entity_to_record(object: &Map<String, Value>) -> Result<RawSourceRecord> {
    let qid = string_field(object, &["id"]).ok_or_else(|| anyhow!("Wikidata entity missing id"))?;
    let labels = localized_values(object.get("labels"));
    let aliases_by_lang = localized_alias_values(object.get("aliases"));
    let primary = choose_primary_label(&labels)
        .or_else(|| localized_field_value(object, &["label", "name", "title"], "und"))
        .ok_or_else(|| anyhow!("Wikidata entity {qid} has no usable labels"))?;

    let entity_kind = classify_entity_kind(object);
    let domain = match entity_kind {
        EntityKind::RealPerson => "knowledge_graph_person",
        EntityKind::FictionalCharacter => "knowledge_graph_character",
        EntityKind::Organization => "knowledge_graph_organization",
        EntityKind::Work => "knowledge_graph_work",
        EntityKind::Product => "knowledge_graph_product",
        EntityKind::Place => "knowledge_graph_place",
    };

    let primary_name = RawNameValue {
        value: primary.value.clone(),
        locale: Some(locale_for_lang(&primary.lang)),
        name_type: Some("primary".to_string()),
        script_hint: script_hint_for_lang(&primary.lang).or_else(|| {
            let script = detect_script(&primary.value);
            (script != ScriptKind::Unknown).then_some(script)
        }),
    };

    let mut aliases = Vec::new();
    let mut seen_aliases = BTreeSet::new();
    for label in labels {
        if label.value == primary.value {
            continue;
        }
        push_alias(
            &mut aliases,
            &mut seen_aliases,
            RawNameValue {
                value: label.value,
                locale: Some(locale_for_lang(&label.lang)),
                name_type: Some("label".to_string()),
                script_hint: script_hint_for_lang(&label.lang),
            },
        );
    }
    for alias in aliases_by_lang {
        push_alias(
            &mut aliases,
            &mut seen_aliases,
            RawNameValue {
                value: alias.value,
                locale: Some(locale_for_lang(&alias.lang)),
                name_type: Some("alias".to_string()),
                script_hint: script_hint_for_lang(&alias.lang),
            },
        );
    }

    let sitelinks = object
        .get("sitelinks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (site, value) in &sitelinks {
        let Some(title) = value
            .get("title")
            .and_then(Value::as_str)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        push_alias(
            &mut aliases,
            &mut seen_aliases,
            RawNameValue {
                value: title.replace('_', " "),
                locale: site_locale(site),
                name_type: Some("sitelink_title".to_string()),
                script_hint: None,
            },
        );
    }

    for (property, kind) in [
        ("P1448", "official_name"),
        ("P1559", "native_name"),
        ("P1813", "short_name"),
    ] {
        for value in claim_text_values(object, property) {
            push_alias(
                &mut aliases,
                &mut seen_aliases,
                RawNameValue {
                    value,
                    locale: Some("und".to_string()),
                    name_type: Some(kind.to_string()),
                    script_hint: None,
                },
            );
        }
    }

    let mut readings = Vec::new();
    let mut seen_readings = BTreeSet::new();
    for label in localized_values(object.get("labels"))
        .into_iter()
        .filter(|label| matches!(label.lang.as_str(), "ja-hira" | "ja-kana"))
    {
        let reading_key = format!("{}|{}", primary.value, label.value);
        if seen_readings.insert(reading_key) {
            readings.push(RawReading {
                value: label.value,
                for_name: Some(primary.value.clone()),
                reading_type: Some("kana_label".to_string()),
            });
        }
    }

    let mut external_ids = vec![RawExternalId {
        source_name: "wikidata".to_string(),
        value: qid.clone(),
        uri: Some(format!("https://www.wikidata.org/wiki/{qid}")),
    }];
    push_external_ids(
        &mut external_ids,
        "ndl",
        claim_string_values(object, "P349"),
        |value| Some(format!("https://id.ndl.go.jp/auth/ndlna/{value}")),
    );
    push_external_ids(
        &mut external_ids,
        "viaf",
        claim_string_values(object, "P214"),
        |value| Some(format!("https://viaf.org/viaf/{value}")),
    );
    for (site, value) in &sitelinks {
        let Some(source_name) = site_external_source(site) else {
            continue;
        };
        let Some(title) = value
            .get("title")
            .and_then(Value::as_str)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        external_ids.push(RawExternalId {
            source_name: source_name.to_string(),
            value: title.replace('_', " "),
            uri: article_url_for_site(site, title),
        });
    }

    let mut fields = BTreeMap::new();
    fields.insert("wikidata_id".to_string(), Value::String(qid.clone()));
    let instance_of = claim_entity_ids(object, "P31");
    if !instance_of.is_empty() {
        fields.insert(
            "instance_of".to_string(),
            Value::Array(instance_of.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(description) = description_value(object, "ja") {
        fields.insert("description_ja".to_string(), Value::String(description));
    }
    if let Some(description) = description_value(object, "en") {
        fields.insert("description_en".to_string(), Value::String(description));
    }
    if !sitelinks.is_empty() {
        let sitelink_titles: BTreeMap<String, Value> = sitelinks
            .iter()
            .filter_map(|(site, value)| {
                value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|title| (site.clone(), Value::String(title.replace('_', " "))))
            })
            .collect();
        fields.insert(
            "sitelinks".to_string(),
            serde_json::to_value(sitelink_titles)?,
        );
    }

    Ok(RawSourceRecord {
        record_id: format!("wikidata-{qid}"),
        record_uri: Some(format!("https://www.wikidata.org/wiki/{qid}")),
        retrieved_at: None,
        entity_kind,
        domain: domain.to_string(),
        context_key: Some(format!("wikidata:{qid}")),
        primary_name,
        aliases,
        readings,
        external_ids,
        relationships: Vec::new(),
        images: Vec::new(),
        fields,
    })
}

fn localized_values(value: Option<&Value>) -> Vec<LocalizedValue> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut entries = object
        .iter()
        .filter_map(|(lang, value)| localized_scalar(value).map(|text| (lang.clone(), text)))
        .map(|(lang, value)| LocalizedValue { lang, value })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| language_priority(&entry.lang));
    entries
}

fn localized_alias_values(value: Option<&Value>) -> Vec<LocalizedValue> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for (lang, values) in object {
        let Some(array) = values.as_array() else {
            continue;
        };
        for value in array {
            if let Some(text) = localized_scalar(value) {
                entries.push(LocalizedValue {
                    lang: lang.clone(),
                    value: text,
                });
            }
        }
    }
    entries.sort_by_key(|entry| language_priority(&entry.lang));
    entries
}

fn choose_primary_label(labels: &[LocalizedValue]) -> Option<LocalizedValue> {
    labels.first().cloned()
}

fn localized_field_value(
    object: &Map<String, Value>,
    keys: &[&str],
    locale: &str,
) -> Option<LocalizedValue> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(localized_scalar)
            .map(|value| LocalizedValue {
                lang: locale.to_string(),
                value,
            })
    })
}

fn localized_scalar(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        _ => None,
    }
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

fn description_value(object: &Map<String, Value>, lang: &str) -> Option<String> {
    object
        .get("descriptions")
        .and_then(Value::as_object)
        .and_then(|descriptions| descriptions.get(lang))
        .and_then(localized_scalar)
}

fn classify_entity_kind(object: &Map<String, Value>) -> EntityKind {
    if let Some(kind) = string_field(object, &["entity_kind", "kind", "page_type"]) {
        if let Some(kind) = parse_entity_kind_string(&kind) {
            return kind;
        }
    }

    let instance_of = claim_entity_ids(object, "P31");
    if instance_of
        .iter()
        .any(|value| matches!(value.as_str(), "Q95074" | "Q15632617" | "Q15773347"))
    {
        return EntityKind::FictionalCharacter;
    }
    if instance_of
        .iter()
        .any(|value| matches!(value.as_str(), "Q5" | "Q215627"))
    {
        return EntityKind::RealPerson;
    }
    if instance_of.iter().any(|value| {
        matches!(
            value.as_str(),
            "Q43229" | "Q4830453" | "Q783794" | "Q163740" | "Q31855" | "Q7278" | "Q891723"
        )
    }) {
        return EntityKind::Organization;
    }
    if instance_of.iter().any(|value| {
        matches!(
            value.as_str(),
            "Q386724"
                | "Q7725634"
                | "Q17537576"
                | "Q2431196"
                | "Q11424"
                | "Q5398426"
                | "Q1107"
                | "Q8274"
                | "Q7889"
                | "Q571"
        )
    }) {
        return EntityKind::Work;
    }
    if instance_of
        .iter()
        .any(|value| matches!(value.as_str(), "Q2424752" | "Q7397" | "Q431289"))
    {
        return EntityKind::Product;
    }
    if instance_of.iter().any(|value| {
        matches!(
            value.as_str(),
            "Q618123" | "Q2221906" | "Q486972" | "Q515" | "Q6256" | "Q5107" | "Q23442"
        )
    }) {
        return EntityKind::Place;
    }

    let description = [
        description_value(object, "ja"),
        description_value(object, "en"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    classify_from_text(&description)
}

fn claim_entity_ids(object: &Map<String, Value>, property: &str) -> Vec<String> {
    claim_values(object, property)
        .into_iter()
        .filter_map(|value| match value {
            Value::Object(object) => object
                .get("id")
                .and_then(Value::as_str)
                .map(|value| value.to_string()),
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn claim_string_values(object: &Map<String, Value>, property: &str) -> Vec<String> {
    claim_values(object, property)
        .into_iter()
        .filter_map(claim_value_to_string)
        .collect()
}

fn claim_text_values(object: &Map<String, Value>, property: &str) -> Vec<String> {
    claim_values(object, property)
        .into_iter()
        .filter_map(claim_value_to_text)
        .collect()
}

fn claim_values(object: &Map<String, Value>, property: &str) -> Vec<Value> {
    let Some(claims) = object.get("claims").and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(values) = claims.get(property).and_then(Value::as_array) else {
        return Vec::new();
    };

    values
        .iter()
        .filter_map(|claim| {
            claim
                .get("mainsnak")
                .and_then(Value::as_object)
                .filter(|snak| snak.get("snaktype").and_then(Value::as_str) == Some("value"))
                .and_then(|snak| snak.get("datavalue"))
                .and_then(|datavalue| datavalue.get("value"))
                .cloned()
        })
        .collect()
}

fn claim_value_to_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| object.get("id").and_then(Value::as_str))
            .map(|value| value.to_string()),
        _ => None,
    }
}

fn claim_value_to_text(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(|value| value.to_string()),
        _ => None,
    }
}

fn push_external_ids<F>(
    external_ids: &mut Vec<RawExternalId>,
    source_name: &str,
    values: Vec<String>,
    uri_builder: F,
) where
    F: Fn(&str) -> Option<String>,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value.clone()) {
            continue;
        }
        external_ids.push(RawExternalId {
            source_name: source_name.to_string(),
            uri: uri_builder(&value),
            value,
        });
    }
}

fn push_alias(aliases: &mut Vec<RawNameValue>, seen: &mut BTreeSet<String>, alias: RawNameValue) {
    let key = format!(
        "{}|{}",
        alias.name_type.clone().unwrap_or_default(),
        alias.value
    );
    if seen.insert(key) {
        aliases.push(alias);
    }
}

fn site_external_source(site: &str) -> Option<&'static str> {
    match site {
        "jawiki" => Some("wikipedia_ja"),
        "enwiki" => Some("wikipedia_en"),
        _ => None,
    }
}

fn article_url_for_site(site: &str, title: &str) -> Option<String> {
    let language = site.strip_suffix("wiki")?;
    Some(format!(
        "https://{}.wikipedia.org/wiki/{}",
        language,
        title.replace(' ', "_")
    ))
}

fn site_locale(site: &str) -> Option<String> {
    site.strip_suffix("wiki")
        .map(|language| language.to_string())
}

fn locale_for_lang(lang: &str) -> String {
    match lang.to_ascii_lowercase().as_str() {
        "ja" | "ja-hira" | "ja-kana" | "ja-latn" => "ja".to_string(),
        "en" => "en".to_string(),
        other => other.to_string(),
    }
}

fn script_hint_for_lang(lang: &str) -> Option<ScriptKind> {
    match lang.to_ascii_lowercase().as_str() {
        "ja-hira" => Some(ScriptKind::Hiragana),
        "ja-kana" => Some(ScriptKind::Katakana),
        "ja-latn" | "en" => Some(ScriptKind::Latin),
        _ => None,
    }
}

fn language_priority(lang: &str) -> usize {
    let normalized = lang.to_ascii_lowercase();
    LANG_PRIORITY
        .iter()
        .position(|candidate| *candidate == normalized)
        .unwrap_or(LANG_PRIORITY.len() + 1)
}

fn parse_entity_kind_string(value: &str) -> Option<EntityKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "person" | "real_person" | "human" => Some(EntityKind::RealPerson),
        "fictional_character" | "character" => Some(EntityKind::FictionalCharacter),
        "organization" | "org" | "company" => Some(EntityKind::Organization),
        "work" | "media" => Some(EntityKind::Work),
        "product" | "software" | "brand" => Some(EntityKind::Product),
        "place" | "location" => Some(EntityKind::Place),
        _ => None,
    }
}

fn classify_from_text(value: &str) -> EntityKind {
    let normalized = value.to_ascii_lowercase();
    if normalized.contains("fictional")
        || normalized.contains("架空")
        || normalized.contains("登場人物")
        || normalized.contains("キャラクター")
    {
        EntityKind::FictionalCharacter
    } else if normalized.contains("company")
        || normalized.contains("organization")
        || normalized.contains("corporation")
        || normalized.contains("法人")
        || normalized.contains("団体")
        || normalized.contains("企業")
        || normalized.contains("放送局")
    {
        EntityKind::Organization
    } else if normalized.contains("city")
        || normalized.contains("prefecture")
        || normalized.contains("country")
        || normalized.contains("island")
        || normalized.contains("location")
        || normalized.contains("地理")
        || normalized.contains("都道府県")
        || normalized.contains("市")
        || normalized.contains("町")
        || normalized.contains("村")
        || normalized.contains("島")
    {
        EntityKind::Place
    } else if normalized.contains("software")
        || normalized.contains("product")
        || normalized.contains("brand")
        || normalized.contains("製品")
        || normalized.contains("ソフトウェア")
        || normalized.contains("ブランド")
    {
        EntityKind::Product
    } else if normalized.contains("novel")
        || normalized.contains("manga")
        || normalized.contains("anime")
        || normalized.contains("film")
        || normalized.contains("game")
        || normalized.contains("book")
        || normalized.contains("series")
        || normalized.contains("作品")
        || normalized.contains("漫画")
        || normalized.contains("小説")
        || normalized.contains("映画")
        || normalized.contains("ゲーム")
        || normalized.contains("アニメ")
    {
        EntityKind::Work
    } else if normalized.contains("person")
        || normalized.contains("writer")
        || normalized.contains("author")
        || normalized.contains("actor")
        || normalized.contains("director")
        || normalized.contains("人物")
        || normalized.contains("作家")
        || normalized.contains("声優")
    {
        EntityKind::RealPerson
    } else {
        EntityKind::Work
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wikidata_entities_from_line_dump() {
        let raw = r#"
[
{"id":"Q111","type":"item","labels":{"ja":{"language":"ja","value":"宮崎駿"},"ja-kana":{"language":"ja-kana","value":"ミヤザキ ハヤオ"},"en":{"language":"en","value":"Hayao Miyazaki"}},"descriptions":{"ja":{"language":"ja","value":"日本のアニメ映画監督"},"en":{"language":"en","value":"Japanese film director"}},"aliases":{"ja":[{"language":"ja","value":"みやざき はやお"}],"en":[{"language":"en","value":"Miyazaki Hayao"}]},"claims":{"P31":[{"mainsnak":{"snaktype":"value","property":"P31","datavalue":{"value":{"id":"Q5"},"type":"wikibase-entityid"}}}],"P214":[{"mainsnak":{"snaktype":"value","property":"P214","datavalue":{"value":"987654","type":"string"}}}]},"sitelinks":{"jawiki":{"site":"jawiki","title":"宮崎駿"},"enwiki":{"site":"enwiki","title":"Hayao_Miyazaki"}}},
{"id":"Q222","type":"item","labels":{"ja":{"language":"ja","value":"スタジオジブリ"},"en":{"language":"en","value":"Studio Ghibli"}},"descriptions":{"ja":{"language":"ja","value":"日本のアニメ制作会社"}},"claims":{"P31":[{"mainsnak":{"snaktype":"value","property":"P31","datavalue":{"value":{"id":"Q783794"},"type":"wikibase-entityid"}}}]}}
]
"#;
        let values = expand_root_value(serde_json::from_str(raw).unwrap()).unwrap();
        let records = values_to_records(values).unwrap();
        assert_eq!(records.len(), 2);

        let person = records
            .iter()
            .find(|record| record.primary_name.value == "宮崎駿")
            .unwrap();
        assert_eq!(person.entity_kind, EntityKind::RealPerson);
        assert!(person
            .readings
            .iter()
            .any(|reading| reading.value == "ミヤザキ ハヤオ"));
        assert!(person
            .external_ids
            .iter()
            .any(|external_id| external_id.source_name == "viaf"));

        let org = records
            .iter()
            .find(|record| record.primary_name.value == "スタジオジブリ")
            .unwrap();
        assert_eq!(org.entity_kind, EntityKind::Organization);
    }
}
