use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};

use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::jiten_client::JitenFrequencyEntry;

const FREQUENCY_BANK_LIMIT: usize = 20_000;
pub const FREQUENCY_DICTIONARY_TITLE: &str = "Bee's Frequency Dictionary";
pub const JITEN_SOURCE_URL: &str = "https://jiten.moe/";
pub const JITEN_LICENSE_LABEL: &str = "CC BY-SA 4.0";
pub const JITEN_LICENSE_URL: &str = "https://creativecommons.org/licenses/by-sa/4.0/";
pub const JITEN_ATTRIBUTION: &str = "Data from jiten.moe licensed under CC BY-SA 4.0.";
const JITEN_ATTRIBUTION_FILE: &str = "attribution.txt";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrequencyKey {
    pub term: String,
    pub reading: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FrequencyAggregate {
    pub total_occurrences: u64,
    pub source_deck_ids: BTreeSet<i32>,
}

pub struct FrequencyDictBuilder {
    entries: BTreeMap<FrequencyKey, FrequencyAggregate>,
    download_url: Option<String>,
    index_url: Option<String>,
    revision: String,
}

impl FrequencyDictBuilder {
    pub fn new(download_url: Option<String>, index_url: Option<String>) -> Self {
        let revision: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            entries: BTreeMap::new(),
            download_url,
            index_url,
            revision: format!("{:012}", revision),
        }
    }

    pub fn add_entries_for_deck(&mut self, deck_id: i32, entries: &[JitenFrequencyEntry]) {
        for entry in entries {
            if entry.term.trim().is_empty() || entry.value == 0 {
                continue;
            }
            let key = FrequencyKey {
                term: entry.term.clone(),
                reading: entry.reading.clone(),
            };
            let aggregate = self.entries.entry(key).or_default();
            aggregate.total_occurrences = aggregate.total_occurrences.saturating_add(entry.value);
            aggregate.source_deck_ids.insert(deck_id);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn filtered_entry_count(
        &self,
        min_occurrences: Option<u64>,
        max_terms: Option<usize>,
    ) -> usize {
        self.sorted_entries(min_occurrences, max_terms).len()
    }

    pub fn create_index(&self) -> serde_json::Value {
        let mut index = json!({
            "title": FREQUENCY_DICTIONARY_TITLE,
            "format": 3,
            "revision": &self.revision,
            "sequenced": false,
            "frequencyMode": "occurrence-based",
            "author": "Bee / Jiten",
            "url": "https://characterdictionary.tokyo/frequency",
            "description": "Combined occurrence counts from jiten.moe for the user's VNDB/AniList media. Data from jiten.moe licensed under CC BY-SA 4.0.",
            "attribution": JITEN_ATTRIBUTION,
            "sourceUrl": JITEN_SOURCE_URL,
            "license": JITEN_LICENSE_LABEL,
            "licenseUrl": JITEN_LICENSE_URL,
        });

        if let Some(download_url) = &self.download_url {
            index["downloadUrl"] = json!(download_url);
        }
        if let Some(index_url) = &self.index_url {
            index["indexUrl"] = json!(index_url);
        }
        if self.download_url.is_some() || self.index_url.is_some() {
            index["isUpdatable"] = json!(true);
        }

        index
    }

    pub fn export_bytes(
        &self,
        min_occurrences: Option<u64>,
        max_terms: Option<usize>,
    ) -> Result<Vec<u8>, String> {
        let sorted_entries = self.sorted_entries(min_occurrences, max_terms);
        if sorted_entries.is_empty() {
            return Err("No frequency entries matched the requested filters".to_string());
        }

        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("index.json", options)
            .map_err(|e| format!("Failed to create index.json in frequency ZIP: {}", e))?;
        let index_json = serde_json::to_string_pretty(&self.create_index())
            .map_err(|e| format!("Failed to serialize frequency index.json: {}", e))?;
        zip.write_all(index_json.as_bytes())
            .map_err(|e| format!("Failed to write frequency index.json: {}", e))?;

        zip.start_file(JITEN_ATTRIBUTION_FILE, options)
            .map_err(|e| {
                format!(
                    "Failed to create {} in frequency ZIP: {}",
                    JITEN_ATTRIBUTION_FILE, e
                )
            })?;
        zip.write_all(attribution_text().as_bytes()).map_err(|e| {
            format!(
                "Failed to write {} in frequency ZIP: {}",
                JITEN_ATTRIBUTION_FILE, e
            )
        })?;

        for (bank_idx, chunk) in sorted_entries.chunks(FREQUENCY_BANK_LIMIT).enumerate() {
            if chunk.is_empty() {
                continue;
            }
            let bank_name = format!("term_meta_bank_{}.json", bank_idx + 1);
            zip.start_file(&bank_name, options)
                .map_err(|e| format!("Failed to create {} in frequency ZIP: {}", bank_name, e))?;
            let bank_entries: Vec<serde_json::Value> = chunk
                .iter()
                .map(|(key, aggregate)| frequency_entry_value(key, aggregate.total_occurrences))
                .collect();
            let bank_json = serde_json::to_string(&bank_entries)
                .map_err(|e| format!("Failed to serialize {}: {}", bank_name, e))?;
            zip.write_all(bank_json.as_bytes())
                .map_err(|e| format!("Failed to write {}: {}", bank_name, e))?;
        }

        let cursor = zip
            .finish()
            .map_err(|e| format!("Failed to finalize frequency ZIP: {}", e))?;
        Ok(cursor.into_inner())
    }

    fn sorted_entries(
        &self,
        min_occurrences: Option<u64>,
        max_terms: Option<usize>,
    ) -> Vec<(&FrequencyKey, &FrequencyAggregate)> {
        let min_occurrences = min_occurrences.unwrap_or(0);
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, aggregate)| aggregate.total_occurrences >= min_occurrences)
            .collect();

        entries.sort_by(|(left_key, left), (right_key, right)| {
            right
                .total_occurrences
                .cmp(&left.total_occurrences)
                .then_with(|| left_key.term.cmp(&right_key.term))
                .then_with(|| left_key.reading.cmp(&right_key.reading))
        });

        if let Some(max_terms) = max_terms {
            entries.truncate(max_terms);
        }

        entries
    }
}

fn attribution_text() -> String {
    format!("{JITEN_ATTRIBUTION}\n\nSource: {JITEN_SOURCE_URL}\nLicense: {JITEN_LICENSE_URL}\n")
}

fn frequency_entry_value(key: &FrequencyKey, occurrences: u64) -> serde_json::Value {
    let display_value = occurrences.to_string();
    match &key.reading {
        Some(reading) => json!([
            key.term.as_str(),
            "freq",
            {
                "reading": reading.as_str(),
                "frequency": {
                    "value": occurrences,
                    "displayValue": display_value
                }
            }
        ]),
        None => json!([
            key.term.as_str(),
            "freq",
            {
                "value": occurrences,
                "displayValue": display_value
            }
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    fn entry(term: &str, reading: Option<&str>, value: u64) -> JitenFrequencyEntry {
        JitenFrequencyEntry {
            term: term.to_string(),
            reading: reading.map(ToOwned::to_owned),
            value,
        }
    }

    fn read_zip_entry(archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> String {
        let mut file = archive.by_name(name).unwrap();
        let mut raw = String::new();
        file.read_to_string(&mut raw).unwrap();
        raw
    }

    #[test]
    fn same_term_reading_across_two_decks_sums_occurrences() {
        let mut builder = FrequencyDictBuilder::new(None, None);

        builder.add_entries_for_deck(1, &[entry("岡部", Some("おかべ"), 10)]);
        builder.add_entries_for_deck(2, &[entry("岡部", Some("おかべ"), 15)]);

        let key = FrequencyKey {
            term: "岡部".to_string(),
            reading: Some("おかべ".to_string()),
        };
        let aggregate = builder.entries.get(&key).unwrap();
        assert_eq!(aggregate.total_occurrences, 25);
        assert_eq!(aggregate.source_deck_ids, BTreeSet::from([1, 2]));
    }

    #[test]
    fn duplicate_key_within_same_deck_sums_but_deck_count_remains_one() {
        let mut builder = FrequencyDictBuilder::new(None, None);

        builder.add_entries_for_deck(1, &[entry("の", None, 10), entry("の", None, 15)]);

        let key = FrequencyKey {
            term: "の".to_string(),
            reading: None,
        };
        let aggregate = builder.entries.get(&key).unwrap();
        assert_eq!(aggregate.total_occurrences, 25);
        assert_eq!(aggregate.source_deck_ids, BTreeSet::from([1]));
    }

    #[test]
    fn different_readings_stay_separate() {
        let mut builder = FrequencyDictBuilder::new(None, None);

        builder.add_entries_for_deck(
            1,
            &[
                entry("今日", Some("きょう"), 10),
                entry("今日", Some("こんにち"), 5),
            ],
        );

        assert_eq!(builder.filtered_entry_count(None, None), 2);
    }

    #[test]
    fn optional_min_occurrences_filters_low_count() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("a", None, 1), entry("b", None, 10)]);

        let sorted = builder.sorted_entries(Some(5), None);

        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].0.term, "b");
    }

    #[test]
    fn optional_max_terms_truncates() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(
            1,
            &[
                entry("a", None, 1),
                entry("b", None, 10),
                entry("c", None, 5),
            ],
        );

        let sorted = builder.sorted_entries(None, Some(2));

        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0].0.term, "b");
        assert_eq!(sorted[1].0.term, "c");
    }

    #[test]
    fn filtered_entry_count_matches_export_filters() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(
            1,
            &[
                entry("a", None, 1),
                entry("b", None, 10),
                entry("c", None, 5),
            ],
        );

        assert_eq!(builder.filtered_entry_count(Some(5), Some(1)), 1);
    }

    #[test]
    fn export_bytes_rejects_empty_filtered_output() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("a", None, 1)]);

        let error = builder.export_bytes(Some(5), None).unwrap_err();

        assert!(error.contains("No frequency entries"));
    }

    #[test]
    fn export_bytes_produces_valid_frequency_zip() {
        let mut builder = FrequencyDictBuilder::new(
            Some("https://example.com/api/yomitan-frequency-dict".to_string()),
            Some("https://example.com/api/yomitan-frequency-index".to_string()),
        );
        builder.add_entries_for_deck(
            1,
            &[entry("の", None, 2410), entry("岡部", Some("おかべ"), 1645)],
        );

        let bytes = builder.export_bytes(None, None).unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let names: Vec<String> = archive.file_names().map(ToOwned::to_owned).collect();

        assert!(names.contains(&"index.json".to_string()));
        assert!(names.contains(&JITEN_ATTRIBUTION_FILE.to_string()));
        assert!(names.contains(&"term_meta_bank_1.json".to_string()));
        assert!(!names.contains(&"term_bank_1.json".to_string()));

        let index: serde_json::Value =
            serde_json::from_str(&read_zip_entry(&mut archive, "index.json")).unwrap();
        assert_eq!(index["title"], FREQUENCY_DICTIONARY_TITLE);
        assert_eq!(index["format"], 3);
        assert_eq!(index["frequencyMode"], "occurrence-based");
        assert_eq!(index["isUpdatable"], true);
        assert_eq!(index["attribution"], JITEN_ATTRIBUTION);
        assert_eq!(index["sourceUrl"], JITEN_SOURCE_URL);
        assert_eq!(index["license"], JITEN_LICENSE_LABEL);
        assert_eq!(index["licenseUrl"], JITEN_LICENSE_URL);

        let attribution = read_zip_entry(&mut archive, JITEN_ATTRIBUTION_FILE);
        assert!(attribution.contains(JITEN_ATTRIBUTION));
        assert!(attribution.contains(JITEN_SOURCE_URL));
        assert!(attribution.contains(JITEN_LICENSE_URL));

        let entries: serde_json::Value =
            serde_json::from_str(&read_zip_entry(&mut archive, "term_meta_bank_1.json")).unwrap();
        assert_eq!(entries[0][0], "の");
        assert_eq!(entries[0][1], "freq");
        assert_eq!(entries[0][2]["value"], 2410);
        assert_eq!(entries[1][0], "岡部");
        assert_eq!(entries[1][2]["reading"], "おかべ");
        assert_eq!(entries[1][2]["frequency"]["value"], 1645);
    }

    #[test]
    fn sorted_entries_use_occurrence_desc_then_term_then_reading_none_first() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(
            1,
            &[
                entry("b", None, 10),
                entry("a", Some("あ"), 10),
                entry("a", None, 10),
                entry("z", None, 20),
            ],
        );

        let sorted = builder.sorted_entries(None, None);

        assert_eq!(sorted[0].0.term, "z");
        assert_eq!(sorted[1].0.term, "a");
        assert_eq!(sorted[1].0.reading, None);
        assert_eq!(sorted[2].0.term, "a");
        assert_eq!(sorted[2].0.reading.as_deref(), Some("あ"));
        assert_eq!(sorted[3].0.term, "b");
    }
}
