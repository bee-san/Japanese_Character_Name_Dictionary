use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use quick_xml::{events::Event, Reader};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use super::{
    model::{EntityKind, ScriptKind},
    source::{RawExternalId, RawNameValue, RawReading, RawSourceRecord},
};

const JMNE_DICT_DOMAIN: &str = "proper_name_lexicon";

#[derive(Debug, Default)]
struct EntryBuilder {
    ent_seq: Option<String>,
    kanji_forms: Vec<String>,
    readings: Vec<EntryReading>,
    translations: Vec<EntryTranslation>,
    current_keb: Option<String>,
    current_reading: Option<EntryReading>,
    current_translation: Option<EntryTranslation>,
}

#[derive(Debug, Clone, Default)]
struct EntryReading {
    value: Option<String>,
    restrictions: Vec<String>,
    no_kanji: bool,
}

#[derive(Debug, Clone, Default)]
struct EntryTranslation {
    name_types: Vec<String>,
    trans_dets: Vec<String>,
    xrefs: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum TextTarget {
    EntSeq,
    Keb,
    Reb,
    ReRestr,
    NameType,
    TransDet,
    Xref,
}

pub fn load_jmnedict_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let xml = load_jmnedict_xml(path)?;
    parse_jmnedict_xml(&xml)
}

fn load_jmnedict_xml(path: &Path) -> Result<String> {
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
        bytes =
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    }

    let raw = String::from_utf8(bytes)
        .with_context(|| format!("{} is not valid UTF-8 XML", path.display()))?;
    Ok(expand_internal_entities(&raw))
}

fn expand_internal_entities(raw: &str) -> String {
    let raw = raw.trim_start_matches('\u{feff}');
    let (without_doctype, entity_map) = strip_doctype_and_collect_entities(raw);
    if entity_map.is_empty() {
        return without_doctype;
    }

    let mut expanded = without_doctype;
    for (name, value) in entity_map {
        let escaped = escape_xml_text(&value);
        expanded = expanded.replace(&format!("&{name};"), &escaped);
    }
    expanded
}

fn strip_doctype_and_collect_entities(raw: &str) -> (String, BTreeMap<String, String>) {
    let Some(start) = raw.find("<!DOCTYPE") else {
        return (raw.to_string(), BTreeMap::new());
    };
    let Some(end_rel) = raw[start..].find("]>") else {
        return (raw.to_string(), BTreeMap::new());
    };
    let end = start + end_rel + 2;
    let doctype = &raw[start..end];
    let mut entity_map = BTreeMap::new();

    for line in doctype.lines() {
        if let Some((name, value)) = parse_entity_definition(line) {
            entity_map.insert(name, value);
        }
    }

    let mut stripped = String::with_capacity(raw.len() - doctype.len());
    stripped.push_str(&raw[..start]);
    stripped.push_str(&raw[end..]);
    (stripped, entity_map)
}

fn parse_entity_definition(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if !line.starts_with("<!ENTITY ") {
        return None;
    }
    let rest = line.trim_start_matches("<!ENTITY ").trim();
    let mut split = rest.splitn(2, char::is_whitespace);
    let name = split.next()?.trim();
    if name.starts_with('%') {
        return None;
    }
    let value_part = split.next()?.trim();
    let quote = value_part.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = value_part[1..].find(quote)?;
    let value = &value_part[1..1 + end];
    Some((name.to_string(), value.to_string()))
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_jmnedict_xml(xml: &str) -> Result<Vec<RawSourceRecord>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut current_entry: Option<EntryBuilder> = None;
    let mut current_text_target: Option<TextTarget> = None;
    let mut current_text = String::new();
    let mut records = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .context("failed to read JMnedict XML event")?
        {
            Event::Start(event) => match event.name().as_ref() {
                b"entry" => current_entry = Some(EntryBuilder::default()),
                b"r_ele" => {
                    if let Some(entry) = current_entry.as_mut() {
                        entry.current_reading = Some(EntryReading::default());
                    }
                }
                b"trans" => {
                    if let Some(entry) = current_entry.as_mut() {
                        entry.current_translation = Some(EntryTranslation::default());
                    }
                }
                b"ent_seq" => {
                    current_text_target = Some(TextTarget::EntSeq);
                    current_text.clear();
                }
                b"keb" => {
                    current_text_target = Some(TextTarget::Keb);
                    current_text.clear();
                }
                b"reb" => {
                    current_text_target = Some(TextTarget::Reb);
                    current_text.clear();
                }
                b"re_restr" => {
                    current_text_target = Some(TextTarget::ReRestr);
                    current_text.clear();
                }
                b"name_type" => {
                    current_text_target = Some(TextTarget::NameType);
                    current_text.clear();
                }
                b"trans_det" => {
                    current_text_target = Some(TextTarget::TransDet);
                    current_text.clear();
                }
                b"xref" => {
                    current_text_target = Some(TextTarget::Xref);
                    current_text.clear();
                }
                _ => {}
            },
            Event::Empty(event) => {
                if event.name().as_ref() == b"re_nokanji" {
                    if let Some(entry) = current_entry.as_mut() {
                        if let Some(reading) = entry.current_reading.as_mut() {
                            reading.no_kanji = true;
                        }
                    }
                }
            }
            Event::Text(event) => {
                if current_text_target.is_some() {
                    current_text.push_str(
                        &event
                            .decode()
                            .context("failed to decode JMnedict text node")?,
                    );
                }
            }
            Event::CData(event) => {
                if current_text_target.is_some() {
                    current_text.push_str(
                        &event
                            .decode()
                            .context("failed to decode JMnedict CDATA node")?,
                    );
                }
            }
            Event::End(event) => {
                let tag = event.name();
                match tag.as_ref() {
                    b"ent_seq" => {
                        if let Some(entry) = current_entry.as_mut() {
                            entry.ent_seq = normalized_non_empty(&current_text);
                        }
                        current_text_target = None;
                    }
                    b"keb" => {
                        if let Some(entry) = current_entry.as_mut() {
                            entry.current_keb = normalized_non_empty(&current_text);
                        }
                        current_text_target = None;
                    }
                    b"k_ele" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(keb) = entry.current_keb.take() {
                                push_unique_string(&mut entry.kanji_forms, keb);
                            }
                        }
                    }
                    b"reb" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(reading) = entry.current_reading.as_mut() {
                                reading.value = normalized_non_empty(&current_text);
                            }
                        }
                        current_text_target = None;
                    }
                    b"re_restr" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(reading) = entry.current_reading.as_mut() {
                                if let Some(value) = normalized_non_empty(&current_text) {
                                    push_unique_string(&mut reading.restrictions, value);
                                }
                            }
                        }
                        current_text_target = None;
                    }
                    b"r_ele" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(reading) = entry.current_reading.take() {
                                if reading.value.is_some() {
                                    entry.readings.push(reading);
                                }
                            }
                        }
                    }
                    b"name_type" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(translation) = entry.current_translation.as_mut() {
                                if let Some(value) = normalized_non_empty(&current_text) {
                                    push_unique_string(&mut translation.name_types, value);
                                }
                            }
                        }
                        current_text_target = None;
                    }
                    b"trans_det" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(translation) = entry.current_translation.as_mut() {
                                if let Some(value) = normalized_non_empty(&current_text) {
                                    push_unique_string(&mut translation.trans_dets, value);
                                }
                            }
                        }
                        current_text_target = None;
                    }
                    b"xref" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(translation) = entry.current_translation.as_mut() {
                                if let Some(value) = normalized_non_empty(&current_text) {
                                    push_unique_string(&mut translation.xrefs, value);
                                }
                            }
                        }
                        current_text_target = None;
                    }
                    b"trans" => {
                        if let Some(entry) = current_entry.as_mut() {
                            if let Some(translation) = entry.current_translation.take() {
                                entry.translations.push(translation);
                            }
                        }
                    }
                    b"entry" => {
                        if let Some(entry) = current_entry.take() {
                            records.extend(entry_to_records(entry)?);
                        }
                    }
                    _ => {}
                }
                current_text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(records)
}

fn entry_to_records(entry: EntryBuilder) -> Result<Vec<RawSourceRecord>> {
    let ent_seq = entry
        .ent_seq
        .ok_or_else(|| anyhow!("JMnedict entry is missing ent_seq"))?;
    let primary_name = entry
        .kanji_forms
        .first()
        .cloned()
        .or_else(|| {
            entry
                .readings
                .iter()
                .find_map(|reading| reading.value.clone())
        })
        .ok_or_else(|| anyhow!("JMnedict entry {ent_seq} has no names"))?;

    let translations = if entry.translations.is_empty() {
        vec![EntryTranslation::default()]
    } else {
        entry.translations
    };
    let kana_only = entry.kanji_forms.is_empty();

    let mut records = Vec::new();
    for (index, translation) in translations.into_iter().enumerate() {
        let trans_index = index + 1;
        let record_id = format!("jmnedict-{ent_seq}-{trans_index}");
        let external_id = format!("{ent_seq}:{trans_index}");
        let mut fields = BTreeMap::new();
        fields.insert("ent_seq".to_string(), Value::String(ent_seq.clone()));
        fields.insert(
            "trans_index".to_string(),
            Value::Number(serde_json::Number::from(trans_index as u64)),
        );
        fields.insert(
            "name_types".to_string(),
            Value::Array(
                translation
                    .name_types
                    .iter()
                    .map(|value| Value::String(value.clone()))
                    .collect(),
            ),
        );
        fields.insert(
            "translations".to_string(),
            Value::Array(
                translation
                    .trans_dets
                    .iter()
                    .map(|value| Value::String(value.clone()))
                    .collect(),
            ),
        );
        if !translation.xrefs.is_empty() {
            fields.insert(
                "xrefs".to_string(),
                Value::Array(
                    translation
                        .xrefs
                        .iter()
                        .map(|value| Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        fields.insert("kana_only".to_string(), Value::Bool(kana_only));

        records.push(RawSourceRecord {
            record_id,
            record_uri: None,
            retrieved_at: None,
            entity_kind: classify_entity_kind(&translation.name_types),
            domain: JMNE_DICT_DOMAIN.to_string(),
            context_key: Some(format!("jmnedict:{ent_seq}")),
            primary_name: RawNameValue {
                value: primary_name.clone(),
                locale: Some("ja".to_string()),
                name_type: Some("primary".to_string()),
                script_hint: None,
            },
            aliases: build_aliases(
                &primary_name,
                &entry.kanji_forms,
                &entry.readings,
                &translation.trans_dets,
            ),
            readings: build_readings(&entry.kanji_forms, &entry.readings),
            external_ids: vec![RawExternalId {
                source_name: "jmnedict".to_string(),
                value: external_id,
                uri: None,
            }],
            relationships: Vec::new(),
            images: Vec::new(),
            fields,
        });
    }

    Ok(records)
}

fn build_aliases(
    primary_name: &str,
    kanji_forms: &[String],
    readings: &[EntryReading],
    translations: &[String],
) -> Vec<RawNameValue> {
    let mut aliases = Vec::new();
    let mut seen = BTreeSet::new();

    for kanji in kanji_forms {
        if kanji == primary_name || !seen.insert(kanji.clone()) {
            continue;
        }
        aliases.push(RawNameValue {
            value: kanji.clone(),
            locale: Some("ja".to_string()),
            name_type: Some("alternate_writing".to_string()),
            script_hint: None,
        });
    }

    if kanji_forms.is_empty() {
        for reading in readings.iter().filter_map(|reading| reading.value.as_ref()) {
            if reading == primary_name || !seen.insert(reading.clone()) {
                continue;
            }
            aliases.push(RawNameValue {
                value: reading.clone(),
                locale: Some("ja".to_string()),
                name_type: Some("alternate_kana".to_string()),
                script_hint: Some(ScriptKind::Hiragana),
            });
        }
    }

    for translation in translations {
        let translation = translation.trim();
        if translation.is_empty() || !seen.insert(translation.to_string()) {
            continue;
        }
        aliases.push(RawNameValue {
            value: translation.to_string(),
            locale: Some("en".to_string()),
            name_type: Some("transliteration".to_string()),
            script_hint: Some(ScriptKind::Latin),
        });
    }

    aliases
}

fn build_readings(kanji_forms: &[String], readings: &[EntryReading]) -> Vec<RawReading> {
    if kanji_forms.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut dedupe = BTreeSet::new();

    for reading in readings {
        let Some(value) = reading.value.as_ref() else {
            continue;
        };

        if reading.no_kanji {
            let key = format!("{value}|");
            if dedupe.insert(key) {
                out.push(RawReading {
                    value: value.clone(),
                    for_name: None,
                    reading_type: Some("kana".to_string()),
                });
            }
            continue;
        }

        let targets: Vec<Option<String>> = if !reading.restrictions.is_empty() {
            reading.restrictions.iter().cloned().map(Some).collect()
        } else {
            kanji_forms.iter().cloned().map(Some).collect()
        };

        for target in targets {
            let key = format!("{value}|{}", target.clone().unwrap_or_default());
            if dedupe.insert(key) {
                out.push(RawReading {
                    value: value.clone(),
                    for_name: target,
                    reading_type: Some("kana".to_string()),
                });
            }
        }
    }

    out
}

fn classify_entity_kind(name_types: &[String]) -> EntityKind {
    let normalized = name_types
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();

    if normalized
        .iter()
        .any(|value| value.contains("organization") || value.contains("company"))
    {
        EntityKind::Organization
    } else if normalized.iter().any(|value| {
        value.contains("work") || value.contains("film") || value.contains("literature")
    }) {
        EntityKind::Work
    } else if normalized.iter().any(|value| value.contains("product")) {
        EntityKind::Product
    } else if normalized
        .iter()
        .any(|value| value.contains("place") || value.contains("station"))
    {
        EntityKind::Place
    } else {
        EntityKind::RealPerson
    }
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn normalized_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_jmnedict_entities_and_splits_multitype_entries() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE JMnedict [
<!ENTITY person "full name of a particular person">
<!ENTITY fem "female given name">
<!ENTITY organization "organization name">
<!ENTITY place "place name">
<!ENTITY surname "surname">
]>
<JMnedict>
  <entry>
    <ent_seq>1000001</ent_seq>
    <k_ele><keb>花澤香菜</keb></k_ele>
    <r_ele><reb>はなざわかな</reb></r_ele>
    <trans>
      <name_type>&person;</name_type>
      <name_type>&fem;</name_type>
      <trans_det>Kana Hanazawa</trans_det>
    </trans>
  </entry>
  <entry>
    <ent_seq>1000002</ent_seq>
    <k_ele><keb>南島</keb></k_ele>
    <r_ele><reb>みなみじま</reb></r_ele>
    <trans>
      <name_type>&place;</name_type>
      <trans_det>Minamijima</trans_det>
    </trans>
    <trans>
      <name_type>&surname;</name_type>
      <trans_det>Minamijima</trans_det>
    </trans>
  </entry>
  <entry>
    <ent_seq>1000003</ent_seq>
    <r_ele><reb>マイア</reb><re_nokanji/></r_ele>
    <trans>
      <name_type>&organization;</name_type>
      <trans_det>Maia Foundation</trans_det>
    </trans>
  </entry>
</JMnedict>"#;

        let expanded = expand_internal_entities(xml);
        assert!(!expanded.contains("&person;"));

        let records = parse_jmnedict_xml(&expanded).unwrap();
        assert_eq!(records.len(), 4);

        let hanazawa = records
            .iter()
            .find(|record| record.primary_name.value == "花澤香菜")
            .unwrap();
        assert_eq!(hanazawa.entity_kind, EntityKind::RealPerson);
        assert_eq!(hanazawa.domain, JMNE_DICT_DOMAIN);
        assert_eq!(
            hanazawa
                .fields
                .get("translations")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(hanazawa.readings.len(), 1);
        assert_eq!(hanazawa.readings[0].for_name.as_deref(), Some("花澤香菜"));

        let minamijima_records = records
            .iter()
            .filter(|record| record.primary_name.value == "南島")
            .collect::<Vec<_>>();
        assert_eq!(minamijima_records.len(), 2);
        assert!(minamijima_records
            .iter()
            .any(|record| record.entity_kind == EntityKind::Place));
        assert!(minamijima_records
            .iter()
            .any(|record| record.entity_kind == EntityKind::RealPerson));

        let maia = records
            .iter()
            .find(|record| record.primary_name.value == "マイア")
            .unwrap();
        assert_eq!(maia.entity_kind, EntityKind::Organization);
        assert!(maia.readings.is_empty());
        assert!(maia
            .aliases
            .iter()
            .any(|alias| alias.value == "Maia Foundation"));
    }
}
