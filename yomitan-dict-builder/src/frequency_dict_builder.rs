use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};

use serde::Deserialize;
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
    pub per_deck_occurrences: BTreeMap<i32, u64>,
}

pub struct FrequencyDictBuilder {
    entries: BTreeMap<FrequencyKey, FrequencyAggregate>,
    deck_totals: BTreeMap<i32, u64>,
    download_url: Option<String>,
    index_url: Option<String>,
    revision: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyDisplayMode {
    #[default]
    Occurrence,
    PerMillion,
    Percent,
    Rank,
}

impl FrequencyDisplayMode {
    pub fn as_query_value(self) -> &'static str {
        match self {
            Self::Occurrence => "occurrence",
            Self::PerMillion => "per_million",
            Self::Percent => "percent",
            Self::Rank => "rank",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyCombineMode {
    #[default]
    Average,
    Sum,
}

impl FrequencyCombineMode {
    pub fn as_query_value(self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::Sum => "sum",
        }
    }
}

struct PreparedFrequencyEntry<'a> {
    key: &'a FrequencyKey,
    aggregate: &'a FrequencyAggregate,
    rank: Option<usize>,
}

impl FrequencyDictBuilder {
    pub fn new(download_url: Option<String>, index_url: Option<String>) -> Self {
        let revision: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            entries: BTreeMap::new(),
            deck_totals: BTreeMap::new(),
            download_url,
            index_url,
            revision: format!("{:012}", revision),
        }
    }

    pub fn add_entries_for_deck(&mut self, deck_id: i32, entries: &[JitenFrequencyEntry]) {
        if self.deck_totals.contains_key(&deck_id) {
            return;
        }

        let mut deck_entries: BTreeMap<FrequencyKey, u64> = BTreeMap::new();
        let mut deck_total = 0u64;

        for entry in entries {
            if entry.term.trim().is_empty() || entry.value == 0 {
                continue;
            }
            let key = FrequencyKey {
                term: entry.term.clone(),
                reading: entry.reading.clone(),
            };
            deck_total = deck_total.saturating_add(entry.value);
            let deck_occurrences = deck_entries.entry(key).or_default();
            *deck_occurrences = deck_occurrences.saturating_add(entry.value);
        }

        if deck_total == 0 {
            return;
        }

        self.deck_totals.insert(deck_id, deck_total);

        for (key, occurrences) in deck_entries {
            let aggregate = self.entries.entry(key).or_default();
            aggregate.total_occurrences = aggregate.total_occurrences.saturating_add(occurrences);
            aggregate.source_deck_ids.insert(deck_id);
            aggregate.per_deck_occurrences.insert(deck_id, occurrences);
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

    pub fn export_bytes_with_options(
        &self,
        min_occurrences: Option<u64>,
        max_terms: Option<usize>,
        display_mode: FrequencyDisplayMode,
        combine_mode: FrequencyCombineMode,
    ) -> Result<Vec<u8>, String> {
        let sorted_entries = self.sorted_entries_with_options(
            min_occurrences,
            max_terms,
            display_mode,
            combine_mode,
        );
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
                .map(|entry| {
                    frequency_entry_value(
                        entry.key,
                        entry.aggregate.total_occurrences,
                        self.display_value(entry.aggregate, display_mode, combine_mode, entry.rank),
                    )
                })
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

    fn sorted_entries_with_options(
        &self,
        min_occurrences: Option<u64>,
        max_terms: Option<usize>,
        display_mode: FrequencyDisplayMode,
        combine_mode: FrequencyCombineMode,
    ) -> Vec<PreparedFrequencyEntry<'_>> {
        let candidates = self.sorted_entries(min_occurrences, max_terms);
        let mut entries: Vec<_> = candidates
            .into_iter()
            .map(|(key, aggregate)| PreparedFrequencyEntry {
                key,
                aggregate,
                rank: None,
            })
            .collect();

        if display_mode == FrequencyDisplayMode::Rank {
            entries.sort_by(|left, right| {
                self.rank_score(right.aggregate, combine_mode)
                    .total_cmp(&self.rank_score(left.aggregate, combine_mode))
                    .then_with(|| {
                        right
                            .aggregate
                            .total_occurrences
                            .cmp(&left.aggregate.total_occurrences)
                    })
                    .then_with(|| left.key.term.cmp(&right.key.term))
                    .then_with(|| left.key.reading.cmp(&right.key.reading))
            });
            for (idx, entry) in entries.iter_mut().enumerate() {
                entry.rank = Some(idx + 1);
            }
        }

        entries
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

    fn display_value(
        &self,
        aggregate: &FrequencyAggregate,
        display_mode: FrequencyDisplayMode,
        combine_mode: FrequencyCombineMode,
        rank: Option<usize>,
    ) -> String {
        match display_mode {
            FrequencyDisplayMode::Occurrence => format!(
                "{} total occurrence{}",
                aggregate.total_occurrences,
                if aggregate.total_occurrences == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            FrequencyDisplayMode::PerMillion => {
                let rate = self.frequency_rate(aggregate, combine_mode) * 1_000_000.0;
                format!(
                    "{} per million ({})",
                    format_decimal(rate),
                    combine_mode.display_label()
                )
            }
            FrequencyDisplayMode::Percent => {
                let percent = self.frequency_rate(aggregate, combine_mode) * 100.0;
                format!(
                    "{}% ({})",
                    format_decimal(percent),
                    combine_mode.display_label()
                )
            }
            FrequencyDisplayMode::Rank => {
                let rank = rank.unwrap_or(0);
                format!("#{} ({})", rank, combine_mode.display_label())
            }
        }
    }

    fn frequency_rate(
        &self,
        aggregate: &FrequencyAggregate,
        combine_mode: FrequencyCombineMode,
    ) -> f64 {
        match combine_mode {
            FrequencyCombineMode::Average => {
                if self.deck_totals.is_empty() {
                    return 0.0;
                }
                let sum_rates: f64 = self
                    .deck_totals
                    .iter()
                    .map(|(deck_id, deck_total)| {
                        if *deck_total == 0 {
                            0.0
                        } else {
                            let occurrences = aggregate
                                .per_deck_occurrences
                                .get(deck_id)
                                .copied()
                                .unwrap_or_default();
                            occurrences as f64 / *deck_total as f64
                        }
                    })
                    .sum();
                sum_rates / self.deck_totals.len() as f64
            }
            FrequencyCombineMode::Sum => {
                let total: u64 = self.deck_totals.values().copied().sum();
                if total == 0 {
                    0.0
                } else {
                    aggregate.total_occurrences as f64 / total as f64
                }
            }
        }
    }

    fn rank_score(
        &self,
        aggregate: &FrequencyAggregate,
        combine_mode: FrequencyCombineMode,
    ) -> f64 {
        match combine_mode {
            FrequencyCombineMode::Average => self.frequency_rate(aggregate, combine_mode),
            FrequencyCombineMode::Sum => aggregate.total_occurrences as f64,
        }
    }
}

fn attribution_text() -> String {
    format!("{JITEN_ATTRIBUTION}\n\nSource: {JITEN_SOURCE_URL}\nLicense: {JITEN_LICENSE_URL}\n")
}

impl FrequencyCombineMode {
    fn display_label(self) -> &'static str {
        match self {
            Self::Average => "average per title",
            Self::Sum => "combined corpus",
        }
    }
}

fn format_decimal(value: f64) -> String {
    let mut formatted = format!("{:.2}", value);
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    formatted
}

fn frequency_entry_value(
    key: &FrequencyKey,
    occurrences: u64,
    display_value: String,
) -> serde_json::Value {
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

    fn exported_term_meta(
        builder: &FrequencyDictBuilder,
        display_mode: FrequencyDisplayMode,
        combine_mode: FrequencyCombineMode,
    ) -> Vec<serde_json::Value> {
        let bytes = builder
            .export_bytes_with_options(None, None, display_mode, combine_mode)
            .unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor).unwrap();
        serde_json::from_str(&read_zip_entry(&mut archive, "term_meta_bank_1.json")).unwrap()
    }

    fn display_value_for<'a>(entries: &'a [serde_json::Value], term: &str) -> &'a str {
        entries
            .iter()
            .find(|entry| entry[0] == term)
            .and_then(|entry| {
                entry[2]["displayValue"]
                    .as_str()
                    .or_else(|| entry[2]["frequency"]["displayValue"].as_str())
            })
            .unwrap()
    }

    fn rank_test_builder() -> FrequencyDictBuilder {
        let mut builder = FrequencyDictBuilder::new(None, None);

        let mut deck_one = vec![
            entry("average_winner", None, 50),
            entry("sum_winner", None, 1),
        ];
        deck_one.extend((0..49).map(|idx| entry(&format!("deck1_filler_{idx}"), None, 1)));
        builder.add_entries_for_deck(1, &deck_one);

        let mut deck_two = vec![entry("sum_winner", None, 100)];
        deck_two.extend((0..900).map(|idx| entry(&format!("deck2_filler_{idx}"), None, 1)));
        builder.add_entries_for_deck(2, &deck_two);

        builder
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
        assert_eq!(aggregate.per_deck_occurrences.get(&1), Some(&10));
        assert_eq!(aggregate.per_deck_occurrences.get(&2), Some(&15));
    }

    #[test]
    fn same_term_in_three_decks_keeps_summed_occurrences() {
        let mut builder = FrequencyDictBuilder::new(None, None);

        builder.add_entries_for_deck(1, &[entry("岡部", Some("おかべ"), 10)]);
        builder.add_entries_for_deck(2, &[entry("岡部", Some("おかべ"), 15)]);
        builder.add_entries_for_deck(3, &[entry("岡部", Some("おかべ"), 10)]);

        let key = FrequencyKey {
            term: "岡部".to_string(),
            reading: Some("おかべ".to_string()),
        };
        let aggregate = builder.entries.get(&key).unwrap();
        assert_eq!(aggregate.total_occurrences, 35);
        assert_eq!(aggregate.source_deck_ids, BTreeSet::from([1, 2, 3]));
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
        assert_eq!(aggregate.per_deck_occurrences.get(&1), Some(&25));
    }

    #[test]
    fn duplicate_deck_ids_are_counted_once() {
        let mut builder = FrequencyDictBuilder::new(None, None);

        builder.add_entries_for_deck(1, &[entry("の", None, 10), entry("filler", None, 90)]);
        builder.add_entries_for_deck(1, &[entry("の", None, 10), entry("filler", None, 90)]);

        let key = FrequencyKey {
            term: "の".to_string(),
            reading: None,
        };
        let aggregate = builder.entries.get(&key).unwrap();
        assert_eq!(builder.deck_totals, BTreeMap::from([(1, 100)]));
        assert_eq!(aggregate.total_occurrences, 10);
        assert_eq!(aggregate.per_deck_occurrences, BTreeMap::from([(1, 10)]));
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

        let error = builder
            .export_bytes_with_options(
                Some(5),
                None,
                FrequencyDisplayMode::Occurrence,
                FrequencyCombineMode::Average,
            )
            .unwrap_err();

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

        let bytes = builder
            .export_bytes_with_options(
                None,
                None,
                FrequencyDisplayMode::Occurrence,
                FrequencyCombineMode::Average,
            )
            .unwrap();
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
        assert_eq!(entries[0][2]["displayValue"], "2410 total occurrences");
        assert_eq!(entries[1][0], "岡部");
        assert_eq!(entries[1][2]["reading"], "おかべ");
        assert_eq!(entries[1][2]["frequency"]["value"], 1645);
        assert_eq!(
            entries[1][2]["frequency"]["displayValue"],
            "1645 total occurrences"
        );
    }

    #[test]
    fn occurrence_display_stays_summed_total() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("target", None, 30), entry("filler1", None, 70)]);
        builder.add_entries_for_deck(2, &[entry("target", None, 5), entry("filler2", None, 95)]);
        builder.add_entries_for_deck(3, &[entry("other", None, 1000)]);

        let entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::Occurrence,
            FrequencyCombineMode::Average,
        );

        let target = entries.iter().find(|entry| entry[0] == "target").unwrap();
        assert_eq!(target[2]["value"], 35);
        assert_eq!(target[2]["displayValue"], "35 total occurrences");
    }

    #[test]
    fn average_per_million_includes_zeroes_for_missing_decks() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("target", None, 30), entry("filler1", None, 70)]);
        builder.add_entries_for_deck(2, &[entry("target", None, 5), entry("filler2", None, 95)]);
        builder.add_entries_for_deck(3, &[entry("other", None, 1000)]);

        let entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::PerMillion,
            FrequencyCombineMode::Average,
        );

        assert_eq!(
            display_value_for(&entries, "target"),
            "116666.67 per million (average per title)"
        );
    }

    #[test]
    fn sum_mode_per_million_uses_combined_corpus_total() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("target", None, 30), entry("filler1", None, 70)]);
        builder.add_entries_for_deck(2, &[entry("target", None, 5), entry("filler2", None, 95)]);
        builder.add_entries_for_deck(3, &[entry("other", None, 1000)]);

        let entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::PerMillion,
            FrequencyCombineMode::Sum,
        );

        assert_eq!(
            display_value_for(&entries, "target"),
            "29166.67 per million (combined corpus)"
        );
    }

    #[test]
    fn percent_display_changes_between_average_and_sum_modes() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("target", None, 30), entry("filler1", None, 70)]);
        builder.add_entries_for_deck(2, &[entry("target", None, 5), entry("filler2", None, 95)]);
        builder.add_entries_for_deck(3, &[entry("other", None, 1000)]);

        let average_entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::Percent,
            FrequencyCombineMode::Average,
        );
        let sum_entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::Percent,
            FrequencyCombineMode::Sum,
        );

        assert_eq!(
            display_value_for(&average_entries, "target"),
            "11.67% (average per title)"
        );
        assert_eq!(
            display_value_for(&sum_entries, "target"),
            "2.92% (combined corpus)"
        );
    }

    #[test]
    fn frequency_style_modes_keep_numeric_value_as_summed_occurrences() {
        let mut builder = FrequencyDictBuilder::new(None, None);
        builder.add_entries_for_deck(1, &[entry("target", None, 30), entry("filler1", None, 70)]);
        builder.add_entries_for_deck(2, &[entry("target", None, 5), entry("filler2", None, 95)]);
        builder.add_entries_for_deck(3, &[entry("other", None, 1000)]);

        for display_mode in [
            FrequencyDisplayMode::PerMillion,
            FrequencyDisplayMode::Percent,
            FrequencyDisplayMode::Rank,
        ] {
            for combine_mode in [FrequencyCombineMode::Average, FrequencyCombineMode::Sum] {
                let entries = exported_term_meta(&builder, display_mode, combine_mode);
                let target = entries.iter().find(|entry| entry[0] == "target").unwrap();

                assert_eq!(target[2]["value"], 35);
            }
        }
    }

    #[test]
    fn rank_mode_orders_by_average_or_summed_count() {
        let builder = rank_test_builder();

        let average_entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::Rank,
            FrequencyCombineMode::Average,
        );
        let sum_entries = exported_term_meta(
            &builder,
            FrequencyDisplayMode::Rank,
            FrequencyCombineMode::Sum,
        );

        assert_eq!(average_entries[0][0], "average_winner");
        assert_eq!(
            display_value_for(&average_entries, "average_winner"),
            "#1 (average per title)"
        );
        assert_eq!(sum_entries[0][0], "sum_winner");
        assert_eq!(
            display_value_for(&sum_entries, "sum_winner"),
            "#1 (combined corpus)"
        );
    }

    #[test]
    fn max_terms_still_truncates_by_summed_occurrences() {
        let builder = rank_test_builder();

        let bytes = builder
            .export_bytes_with_options(
                None,
                Some(1),
                FrequencyDisplayMode::Rank,
                FrequencyCombineMode::Average,
            )
            .unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let entries: Vec<serde_json::Value> =
            serde_json::from_str(&read_zip_entry(&mut archive, "term_meta_bank_1.json")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0][0], "sum_winner");
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
