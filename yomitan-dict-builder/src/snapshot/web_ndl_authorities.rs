use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::{
    model::EntityKind,
    source::{RawExternalId, RawNameValue, RawReading, RawSourceRecord},
};

#[derive(Debug, Default)]
struct AuthorityBuilder {
    uri: String,
    primary_name: Option<String>,
    primary_yomi: Option<String>,
    entity_kind: Option<EntityKind>,
    birth: Option<String>,
    death: Option<String>,
    aliases: Vec<AliasClaim>,
}

#[derive(Debug, Clone)]
struct AliasClaim {
    value: String,
    yomi: Option<String>,
    kind: String,
}

pub fn load_web_ndl_authority_records(path: &Path) -> Result<Vec<RawSourceRecord>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read Web NDL authority input {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse Web NDL authority input {}", path.display()))?;

    if let Some(raw_records) = try_raw_source_records(&value) {
        return Ok(raw_records);
    }

    let rows = sparql_rows_from_json(&value)?;
    build_records(rows)
}

fn try_raw_source_records(value: &Value) -> Option<Vec<RawSourceRecord>> {
    if let Ok(records) = serde_json::from_value::<Vec<RawSourceRecord>>(value.clone()) {
        return Some(records);
    }
    value
        .get("records")
        .and_then(|records| serde_json::from_value::<Vec<RawSourceRecord>>(records.clone()).ok())
}

fn sparql_rows_from_json(value: &Value) -> Result<Vec<Map<String, Value>>> {
    if let Some(bindings) = value
        .get("results")
        .and_then(|results| results.get("bindings"))
        .and_then(Value::as_array)
    {
        return bindings
            .iter()
            .map(|row| {
                row.as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("SPARQL binding row must be an object"))
            })
            .collect();
    }

    if let Some(rows) = value.as_array() {
        return rows
            .iter()
            .map(|row| {
                row.as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("authority row must be an object"))
            })
            .collect();
    }

    if let Some(rows) = value.get("rows").and_then(Value::as_array) {
        return rows
            .iter()
            .map(|row| {
                row.as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("authority row must be an object"))
            })
            .collect();
    }

    Err(anyhow!(
        "Web NDL Authorities input must be SPARQL JSON, an array of rows, or raw snapshot records"
    ))
}

fn build_records(rows: Vec<Map<String, Value>>) -> Result<Vec<RawSourceRecord>> {
    let mut builders: BTreeMap<String, AuthorityBuilder> = BTreeMap::new();

    for row in rows {
        let Some(uri) = binding_string(&row, &["uri1", "uri", "authority_uri", "record_uri"])
        else {
            continue;
        };
        let builder = builders
            .entry(uri.clone())
            .or_insert_with(|| AuthorityBuilder {
                uri,
                ..AuthorityBuilder::default()
            });

        if builder.primary_name.is_none() {
            builder.primary_name = binding_string(&row, &["heading", "prefLabel", "label"]);
        }
        if builder.primary_yomi.is_none() {
            builder.primary_yomi = binding_string(
                &row,
                &["yomi", "transcription", "prefLabelKana", "heading_yomi"],
            );
        }
        if builder.entity_kind.is_none() {
            builder.entity_kind = binding_string(&row, &["entity_type", "kind", "type"])
                .and_then(|value| classify_kind(&value));
        }
        if builder.birth.is_none() {
            builder.birth = binding_string(&row, &["birth", "dateOfBirth", "birth_year"]);
        }
        if builder.death.is_none() {
            builder.death = binding_string(&row, &["death", "dateOfDeath", "death_year"]);
        }

        push_alias_claim(
            builder,
            binding_string(&row, &["variant", "altLabel", "variant_name"]),
            binding_string(
                &row,
                &["variant_yomi", "altLabelKana", "variant_transcription"],
            ),
            "variant_name",
        );
        push_alias_claim(
            builder,
            binding_string(&row, &["real_name", "realName"]),
            binding_string(&row, &["real_name_yomi", "realNameKana"]),
            "real_name",
        );
        push_alias_claim(
            builder,
            binding_string(&row, &["pseudonym", "pen_name", "stage_name"]),
            binding_string(
                &row,
                &["pseudonym_yomi", "pen_name_yomi", "stage_name_yomi"],
            ),
            "pseudonym",
        );
        push_alias_claim(
            builder,
            binding_string(&row, &["later_name", "laterName"]),
            binding_string(&row, &["later_name_yomi", "laterNameKana"]),
            "later_name",
        );
        push_alias_claim(
            builder,
            binding_string(&row, &["earlier_name", "earlierName"]),
            binding_string(&row, &["earlier_name_yomi", "earlierNameKana"]),
            "earlier_name",
        );
        push_alias_claim(
            builder,
            binding_string(&row, &["abbreviation", "short_name"]),
            binding_string(&row, &["abbreviation_yomi", "short_name_yomi"]),
            "abbreviation",
        );
    }

    let mut records = Vec::new();
    for builder in builders.into_values() {
        let primary_name = builder
            .primary_name
            .clone()
            .ok_or_else(|| anyhow!("Web NDL authority row missing heading for {}", builder.uri))?;
        let entity_kind = builder.entity_kind.unwrap_or(EntityKind::RealPerson);
        let authority_id = builder
            .uri
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("unknown");

        let mut aliases = Vec::new();
        let mut readings = Vec::new();
        let mut seen_aliases = BTreeSet::new();
        let mut seen_readings = BTreeSet::new();

        if let Some(primary_yomi) = builder.primary_yomi.clone() {
            let reading_key = format!("{primary_name}|{primary_yomi}");
            if seen_readings.insert(reading_key) {
                readings.push(RawReading {
                    value: primary_yomi,
                    for_name: Some(primary_name.clone()),
                    reading_type: Some("kana".to_string()),
                });
            }
        }

        for alias in builder.aliases {
            if alias.value == primary_name {
                if let Some(yomi) = alias.yomi {
                    let reading_key = format!("{}|{}", alias.value, yomi);
                    if seen_readings.insert(reading_key) {
                        readings.push(RawReading {
                            value: yomi,
                            for_name: Some(alias.value),
                            reading_type: Some("kana".to_string()),
                        });
                    }
                }
                continue;
            }

            if seen_aliases.insert(alias.value.clone()) {
                aliases.push(RawNameValue {
                    value: alias.value.clone(),
                    locale: guess_locale(&alias.value),
                    name_type: Some(alias.kind.clone()),
                    script_hint: None,
                });
            }
            if let Some(yomi) = alias.yomi {
                let reading_key = format!("{}|{}", alias.value, yomi);
                if seen_readings.insert(reading_key) {
                    readings.push(RawReading {
                        value: yomi,
                        for_name: Some(alias.value),
                        reading_type: Some("kana".to_string()),
                    });
                }
            }
        }

        let domain = match entity_kind {
            EntityKind::Organization => "authority_organization",
            _ => "authority_person",
        };

        let mut fields = BTreeMap::new();
        if let Some(birth) = builder.birth {
            fields.insert("birth_year".to_string(), Value::String(birth));
        }
        if let Some(death) = builder.death {
            fields.insert("death_year".to_string(), Value::String(death));
        }
        fields.insert(
            "authority_source".to_string(),
            Value::String("web_ndl_authorities".to_string()),
        );

        records.push(RawSourceRecord {
            record_id: format!("web-ndl-{authority_id}"),
            record_uri: Some(builder.uri.clone()),
            retrieved_at: None,
            entity_kind,
            domain: domain.to_string(),
            context_key: Some(format!("ndl:{authority_id}")),
            primary_name: RawNameValue {
                value: primary_name,
                locale: Some("ja".to_string()),
                name_type: Some("primary".to_string()),
                script_hint: None,
            },
            aliases,
            readings,
            external_ids: vec![RawExternalId {
                source_name: "ndl".to_string(),
                value: authority_id.to_string(),
                uri: Some(builder.uri),
            }],
            relationships: Vec::new(),
            images: Vec::new(),
            fields,
        });
    }

    Ok(records)
}

fn push_alias_claim(
    builder: &mut AuthorityBuilder,
    value: Option<String>,
    yomi: Option<String>,
    kind: &str,
) {
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    if builder
        .aliases
        .iter()
        .any(|alias| alias.value == value && alias.kind == kind)
    {
        return;
    }
    builder.aliases.push(AliasClaim {
        value,
        yomi,
        kind: kind.to_string(),
    });
}

fn binding_string(row: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        row.get(*key).and_then(|value| match value {
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
        })
    })
}

fn classify_kind(value: &str) -> Option<EntityKind> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.contains("organization")
        || normalized.contains("corporate")
        || normalized.contains("団体")
        || normalized.contains("法人")
    {
        Some(EntityKind::Organization)
    } else if normalized.contains("person")
        || normalized.contains("personal")
        || normalized.contains("個人")
        || normalized.contains("人物")
        || normalized.contains("name")
    {
        Some(EntityKind::RealPerson)
    } else {
        None
    }
}

fn guess_locale(value: &str) -> Option<String> {
    if value.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff66..=0xff9f
        )
    }) {
        Some("ja".to_string())
    } else {
        Some("und".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sparql_json_rows_into_people_and_organizations() {
        let value = serde_json::json!({
            "head": { "vars": ["uri1", "heading", "yomi", "variant", "variant_yomi", "entity_type", "birth", "death"] },
            "results": {
                "bindings": [
                    {
                        "uri1": { "type": "uri", "value": "https://id.ndl.go.jp/auth/ndlna/00000001" },
                        "heading": { "type": "literal", "xml:lang": "ja", "value": "夏目漱石" },
                        "yomi": { "type": "literal", "xml:lang": "ja-Kana", "value": "ナツメ ソウセキ" },
                        "variant": { "type": "literal", "xml:lang": "ja", "value": "夏目金之助" },
                        "variant_yomi": { "type": "literal", "xml:lang": "ja-Kana", "value": "ナツメ キンノスケ" },
                        "entity_type": { "type": "literal", "value": "person" },
                        "birth": { "type": "literal", "value": "1867" },
                        "death": { "type": "literal", "value": "1916" }
                    },
                    {
                        "uri1": { "type": "uri", "value": "https://id.ndl.go.jp/auth/ndlna/00600065" },
                        "heading": { "type": "literal", "xml:lang": "ja", "value": "日本放送協会" },
                        "yomi": { "type": "literal", "xml:lang": "ja-Kana", "value": "ニッポン ホウソウ キョウカイ" },
                        "variant": { "type": "literal", "value": "NHK" },
                        "entity_type": { "type": "literal", "value": "organization" }
                    }
                ]
            }
        });

        let rows = sparql_rows_from_json(&value).unwrap();
        let records = build_records(rows).unwrap();
        assert_eq!(records.len(), 2);

        let person = records
            .iter()
            .find(|record| record.primary_name.value == "夏目漱石")
            .unwrap();
        assert_eq!(person.entity_kind, EntityKind::RealPerson);
        assert_eq!(person.domain, "authority_person");
        assert!(person
            .aliases
            .iter()
            .any(|alias| alias.value == "夏目金之助"));
        assert!(person.readings.iter().any(|reading| {
            reading.for_name.as_deref() == Some("夏目漱石") && reading.value == "ナツメ ソウセキ"
        }));

        let org = records
            .iter()
            .find(|record| record.primary_name.value == "日本放送協会")
            .unwrap();
        assert_eq!(org.entity_kind, EntityKind::Organization);
        assert_eq!(org.domain, "authority_organization");
        assert!(org.aliases.iter().any(|alias| alias.value == "NHK"));
    }
}
