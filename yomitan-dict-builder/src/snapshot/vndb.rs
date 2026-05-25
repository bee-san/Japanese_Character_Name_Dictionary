use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    models::{Character, CharacterData, CharacterTrait},
    vndb_client::{VnInfo, VndbClient, VoiceActorInfo},
};

use super::{
    model::EntityKind,
    source::{RawExternalId, RawNameValue, RawRelationship, RawSourceBundle, RawSourceRecord},
};

pub fn build_vndb_raw_bundle(
    vn_id: &str,
    vn_info: &VnInfo,
    char_data: &CharacterData,
    retrieved_at: &str,
) -> RawSourceBundle {
    let vn_id = VndbClient::normalize_id(vn_id);
    let context_key = format!("vndb:{vn_id}");
    let mut records = Vec::new();

    records.push(build_work_record(
        &vn_id,
        vn_info,
        retrieved_at,
        &context_key,
    ));

    let mut seen_staff = BTreeSet::new();
    for character in char_data.all_characters() {
        records.push(build_character_record(
            &vn_id,
            character,
            vn_info.va_map.get(&character.id),
            retrieved_at,
            &context_key,
        ));

        if let Some(va_info) = vn_info.va_map.get(&character.id) {
            if seen_staff.insert(va_info.staff_id.clone()) {
                records.push(build_staff_record(va_info, retrieved_at));
            }
        }
    }

    RawSourceBundle { records }
}

fn build_work_record(
    vn_id: &str,
    vn_info: &VnInfo,
    retrieved_at: &str,
    context_key: &str,
) -> RawSourceRecord {
    let primary_is_native = !vn_info.alttitle.trim().is_empty();
    let primary_value = if primary_is_native {
        vn_info.alttitle.trim()
    } else {
        vn_info.title.trim()
    };

    let mut aliases = Vec::new();
    let mut seen_names = BTreeSet::new();
    seen_names.insert(primary_value.to_string());
    if primary_is_native
        && !vn_info.title.trim().is_empty()
        && seen_names.insert(vn_info.title.trim().to_string())
    {
        aliases.push(RawNameValue {
            value: vn_info.title.trim().to_string(),
            locale: Some("en".to_string()),
            name_type: Some("romanized_title".to_string()),
            script_hint: None,
        });
    } else if !primary_is_native
        && !vn_info.alttitle.trim().is_empty()
        && seen_names.insert(vn_info.alttitle.trim().to_string())
    {
        aliases.push(RawNameValue {
            value: vn_info.alttitle.trim().to_string(),
            locale: Some("ja".to_string()),
            name_type: Some("native_title".to_string()),
            script_hint: None,
        });
    }

    let mut fields = BTreeMap::new();
    fields.insert("source_title".to_string(), json!(vn_info.title));
    fields.insert("source_alttitle".to_string(), json!(vn_info.alttitle));

    RawSourceRecord {
        record_id: format!("vn:{vn_id}"),
        record_uri: Some(format!("https://vndb.org/{vn_id}")),
        retrieved_at: Some(retrieved_at.to_string()),
        entity_kind: EntityKind::Work,
        domain: "visual_novel".to_string(),
        context_key: Some(context_key.to_string()),
        primary_name: RawNameValue {
            value: primary_value.to_string(),
            locale: Some(if primary_is_native { "ja" } else { "en" }.to_string()),
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
        images: Vec::new(),
        fields,
    }
}

fn build_character_record(
    vn_id: &str,
    character: &Character,
    voice_actor: Option<&VoiceActorInfo>,
    retrieved_at: &str,
    context_key: &str,
) -> RawSourceRecord {
    let primary_is_native = !character.name_original.trim().is_empty();
    let primary_value = if primary_is_native {
        character.name_original.trim()
    } else {
        character.name.trim()
    };

    let mut aliases = Vec::new();
    let mut seen_names = BTreeSet::new();
    seen_names.insert(primary_value.to_string());

    if primary_is_native
        && !character.name.trim().is_empty()
        && seen_names.insert(character.name.trim().to_string())
    {
        aliases.push(RawNameValue {
            value: character.name.trim().to_string(),
            locale: Some("en".to_string()),
            name_type: Some("romanized".to_string()),
            script_hint: None,
        });
    }

    for alias in &character.aliases {
        if !alias.trim().is_empty() && seen_names.insert(alias.trim().to_string()) {
            aliases.push(RawNameValue {
                value: alias.trim().to_string(),
                locale: None,
                name_type: Some("alias".to_string()),
                script_hint: None,
            });
        }
    }

    let mut relationships = vec![RawRelationship {
        predicate: "appears_in".to_string(),
        target_source_name: "vndb".to_string(),
        target_external_id: vn_id.to_string(),
        context_source_name: None,
        context_external_id: None,
        confidence: Some(1.0),
    }];

    if let Some(voice_actor) = voice_actor {
        relationships.push(RawRelationship {
            predicate: "voiced_by".to_string(),
            target_source_name: "vndb".to_string(),
            target_external_id: voice_actor.staff_id.clone(),
            context_source_name: Some("vndb".to_string()),
            context_external_id: Some(vn_id.to_string()),
            confidence: Some(1.0),
        });
    }

    let mut fields = BTreeMap::new();
    fields.insert("role".to_string(), json!(character.role));
    fields.insert("sex".to_string(), json!(character.sex));
    fields.insert("age".to_string(), json!(character.age));
    fields.insert("height_cm".to_string(), json!(character.height));
    fields.insert("weight_kg".to_string(), json!(character.weight));
    fields.insert("blood_type".to_string(), json!(character.blood_type));
    fields.insert("birthday".to_string(), json!(character.birthday));
    fields.insert("description".to_string(), json!(character.description));
    fields.insert(
        "spoiler_aliases".to_string(),
        json!(character.spoiler_aliases),
    );
    fields.insert(
        "traits".to_string(),
        json!({
            "personality": encode_traits(&character.personality),
            "roles": encode_traits(&character.roles),
            "engages_in": encode_traits(&character.engages_in),
            "subject_of": encode_traits(&character.subject_of),
        }),
    );

    RawSourceRecord {
        record_id: format!("character:{}", character.id),
        record_uri: Some(format!("https://vndb.org/{}", character.id)),
        retrieved_at: Some(retrieved_at.to_string()),
        entity_kind: EntityKind::FictionalCharacter,
        domain: "visual_novel".to_string(),
        context_key: Some(context_key.to_string()),
        primary_name: RawNameValue {
            value: primary_value.to_string(),
            locale: Some(if primary_is_native { "ja" } else { "en" }.to_string()),
            name_type: Some("primary".to_string()),
            script_hint: None,
        },
        aliases,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "vndb".to_string(),
            value: character.id.clone(),
            uri: Some(format!("https://vndb.org/{}", character.id)),
        }],
        relationships,
        images: Vec::new(),
        fields,
    }
}

fn build_staff_record(voice_actor: &VoiceActorInfo, retrieved_at: &str) -> RawSourceRecord {
    let primary_is_native = !voice_actor.original.trim().is_empty();
    let primary_value = if primary_is_native {
        voice_actor.original.trim()
    } else {
        voice_actor.name.trim()
    };

    let mut aliases = Vec::new();
    if primary_is_native
        && !voice_actor.name.trim().is_empty()
        && voice_actor.name.trim() != primary_value
    {
        aliases.push(RawNameValue {
            value: voice_actor.name.trim().to_string(),
            locale: Some("en".to_string()),
            name_type: Some("romanized".to_string()),
            script_hint: None,
        });
    }

    let mut fields = BTreeMap::new();
    fields.insert("role".to_string(), json!("voice_actor"));

    RawSourceRecord {
        record_id: format!("staff:{}", voice_actor.staff_id),
        record_uri: Some(format!("https://vndb.org/{}", voice_actor.staff_id)),
        retrieved_at: Some(retrieved_at.to_string()),
        entity_kind: EntityKind::RealPerson,
        domain: "voice_actor".to_string(),
        context_key: None,
        primary_name: RawNameValue {
            value: primary_value.to_string(),
            locale: Some(if primary_is_native { "ja" } else { "en" }.to_string()),
            name_type: Some("primary".to_string()),
            script_hint: None,
        },
        aliases,
        readings: Vec::new(),
        external_ids: vec![RawExternalId {
            source_name: "vndb".to_string(),
            value: voice_actor.staff_id.clone(),
            uri: Some(format!("https://vndb.org/{}", voice_actor.staff_id)),
        }],
        relationships: Vec::new(),
        images: Vec::new(),
        fields,
    }
}

fn encode_traits(traits: &[CharacterTrait]) -> Vec<BTreeMap<&'static str, serde_json::Value>> {
    traits
        .iter()
        .map(|trait_item| {
            let mut row = BTreeMap::new();
            row.insert("name", json!(trait_item.name));
            row.insert("spoiler", json!(trait_item.spoiler));
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_character_data() -> CharacterData {
        let mut data = CharacterData::new();
        data.main.push(Character {
            id: "c11".to_string(),
            name: "Okabe Rintarou".to_string(),
            name_original: "岡部 倫太郎".to_string(),
            role: "main".to_string(),
            description: Some("Self-proclaimed mad scientist.".to_string()),
            aliases: vec!["Okarin".to_string()],
            personality: vec![CharacterTrait {
                name: "Eccentric".to_string(),
                spoiler: 0,
            }],
            seiyuu: Some("宮野真守".to_string()),
            ..Character::default()
        });
        data
    }

    #[test]
    fn builds_vndb_bundle_with_work_character_and_staff_records() {
        let mut va_map = BTreeMap::new();
        va_map.insert(
            "c11".to_string(),
            VoiceActorInfo {
                staff_id: "s44".to_string(),
                name: "Mamoru Miyano".to_string(),
                original: "宮野真守".to_string(),
                display_name: "宮野真守".to_string(),
            },
        );

        let vn_info = VnInfo {
            title: "Steins;Gate".to_string(),
            alttitle: "シュタインズ・ゲート".to_string(),
            va_map: va_map.into_iter().collect(),
        };

        let bundle = build_vndb_raw_bundle("v17", &vn_info, &sample_character_data(), "2026-04-08");
        assert_eq!(bundle.records.len(), 3);
        assert!(bundle
            .records
            .iter()
            .any(|record| record.record_id == "vn:v17"));
        assert!(bundle
            .records
            .iter()
            .any(|record| record.record_id == "character:c11"));
        assert!(bundle
            .records
            .iter()
            .any(|record| record.record_id == "staff:s44"));

        let character = bundle
            .records
            .iter()
            .find(|record| record.record_id == "character:c11")
            .unwrap();
        assert_eq!(character.relationships.len(), 2);
        assert!(character
            .relationships
            .iter()
            .any(|rel| rel.predicate == "appears_in" && rel.target_external_id == "v17"));
        assert!(character
            .relationships
            .iter()
            .any(|rel| rel.predicate == "voiced_by" && rel.target_external_id == "s44"));
    }
}
