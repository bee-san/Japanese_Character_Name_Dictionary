use std::io::{Cursor, Read};

use reqwest::Client;
use serde_json::Value;
use zip::ZipArchive;

const JITEN_DECK_FORMAT_YOMITAN: u8 = 5;
const JITEN_DOWNLOAD_TYPE_FULL: u8 = 1;
const JITEN_DECK_ORDER_FREQUENCY: u8 = 3;

#[derive(Clone)]
pub struct JitenClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitenFrequencyEntry {
    pub term: String,
    pub reading: Option<String>,
    pub value: u64,
}

impl JitenClient {
    pub fn new(client: Client) -> Self {
        Self::with_base_url(
            client,
            std::env::var("JITEN_BASE_URL").unwrap_or_else(|_| "https://api.jiten.moe".to_string()),
        )
    }

    #[cfg(test)]
    fn with_base_url(client: Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: std::env::var("JITEN_API_KEY").ok(),
        }
    }

    #[cfg(not(test))]
    fn with_base_url(client: Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: std::env::var("JITEN_API_KEY").ok(),
        }
    }

    pub async fn deck_ids_by_external_link(
        &self,
        source: &str,
        id: &str,
    ) -> Result<Vec<i32>, String> {
        let link_type = source_to_link_type(source).ok_or_else(|| {
            format!(
                "INVALID_INPUT: Unsupported Jiten media source '{}'",
                source.trim()
            )
        })?;
        let normalized_id = normalize_external_id_for_jiten(source, id);
        let url = format!(
            "{}{}",
            self.base_url,
            deck_lookup_path(link_type, &normalized_id)
        );

        let mut request = self.client.get(url);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to contact Jiten: {}", e))?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_jiten_status(
                status.as_u16(),
                "resolve Jiten media deck",
                &body,
            ));
        }

        response
            .json::<Vec<i32>>()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to parse Jiten deck ID response: {}", e))
    }

    pub async fn download_yomitan_frequency_zip(&self, deck_id: i32) -> Result<Vec<u8>, String> {
        let url = format!("{}/api/media-deck/{}/download", self.base_url, deck_id);
        let payload = yomitan_frequency_download_payload();

        let mut request = self.client.post(url).json(&payload);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to download Jiten deck {}: {}", deck_id, e))?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_jiten_status(
                status.as_u16(),
                &format!("download Jiten deck {}", deck_id),
                &body,
            ));
        }

        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| {
                format!(
                    "UPSTREAM: Failed to read Jiten deck {} bytes: {}",
                    deck_id, e
                )
            })
    }

    pub fn parse_yomitan_frequency_zip(bytes: &[u8]) -> Result<Vec<JitenFrequencyEntry>, String> {
        let cursor = Cursor::new(bytes);
        let mut archive =
            ZipArchive::new(cursor).map_err(|e| format!("Invalid Jiten frequency ZIP: {}", e))?;

        let mut bank_names = Vec::new();
        for idx in 0..archive.len() {
            let file = archive
                .by_index(idx)
                .map_err(|e| format!("Failed to read Jiten ZIP entry: {}", e))?;
            let name = file.name().to_string();
            if is_term_meta_bank_name(&name) {
                bank_names.push(name);
            }
        }
        bank_names.sort();

        let mut entries = Vec::new();
        for bank_name in bank_names {
            let mut file = archive
                .by_name(&bank_name)
                .map_err(|e| format!("Failed to open {} in Jiten ZIP: {}", bank_name, e))?;
            let mut raw = String::new();
            file.read_to_string(&mut raw)
                .map_err(|e| format!("Failed to read {} in Jiten ZIP: {}", bank_name, e))?;
            let value: Value = serde_json::from_str(&raw)
                .map_err(|e| format!("Failed to parse {} in Jiten ZIP: {}", bank_name, e))?;
            let Some(bank_entries) = value.as_array() else {
                return Err(format!("{} in Jiten ZIP is not a JSON array", bank_name));
            };

            for entry in bank_entries {
                if let Some(parsed) = parse_term_meta_entry(entry) {
                    entries.push(parsed);
                }
            }
        }

        Ok(entries)
    }
}

pub fn source_to_link_type(source: &str) -> Option<i32> {
    match source.trim().to_lowercase().as_str() {
        "vndb" => Some(2),
        "anilist" => Some(4),
        _ => None,
    }
}

pub fn normalize_external_id_for_jiten(source: &str, id: &str) -> String {
    let trimmed = id.trim();
    match source.trim().to_lowercase().as_str() {
        "vndb" => normalize_vndb_id_for_jiten(trimmed),
        "anilist" => normalize_anilist_id_for_jiten(trimmed),
        _ => trimmed.to_string(),
    }
}

pub fn deck_lookup_path(link_type: i32, normalized_id: &str) -> String {
    format!(
        "/api/media-deck/by-link-id/{}/{}",
        link_type,
        urlencoding::encode(normalized_id)
    )
}

pub fn yomitan_frequency_download_payload() -> serde_json::Value {
    serde_json::json!({
        "format": JITEN_DECK_FORMAT_YOMITAN,
        "downloadType": JITEN_DOWNLOAD_TYPE_FULL,
        "order": JITEN_DECK_ORDER_FREQUENCY,
        "excludeKana": false,
        "excludeExampleSentences": true,
    })
}

fn normalize_vndb_id_for_jiten(id: &str) -> String {
    if let Some(pos) = id.find("vndb.org/v") {
        let after = &id[pos + "vndb.org/".len()..];
        let id_part = after
            .split(&['/', '?', '#'][..])
            .next()
            .unwrap_or(after)
            .trim();
        if !id_part.is_empty() {
            return id_part.to_string();
        }
    }

    if id.starts_with('v') || id.starts_with('V') {
        id.to_lowercase()
    } else if id.chars().all(|ch| ch.is_ascii_digit()) {
        format!("v{}", id)
    } else {
        id.to_string()
    }
}

fn normalize_anilist_id_for_jiten(id: &str) -> String {
    if let Some(pos) = id.find("anilist.co/") {
        let after = &id[pos + "anilist.co/".len()..];
        let segments: Vec<&str> = after.split('/').collect();
        if segments.len() >= 2 {
            let id_part = segments[1]
                .split(&['?', '#'][..])
                .next()
                .unwrap_or("")
                .trim();
            if id_part.chars().all(|ch| ch.is_ascii_digit()) {
                return id_part.to_string();
            }
        }
    }

    id.to_string()
}

fn classify_jiten_status(status: u16, action: &str, body: &str) -> String {
    let suffix = if body.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", body.trim())
    };

    match status {
        400 | 404 => format!(
            "INVALID_INPUT: Jiten could not {} (HTTP {}){}",
            action, status, suffix
        ),
        429 => format!(
            "RATE_LIMIT: Jiten is rate limiting requests while trying to {}",
            action
        ),
        500..=599 => format!(
            "UPSTREAM: Jiten failed while trying to {} (HTTP {}){}",
            action, status, suffix
        ),
        _ => format!(
            "UPSTREAM: Jiten returned HTTP {} while trying to {}{}",
            status, action, suffix
        ),
    }
}

fn is_term_meta_bank_name(name: &str) -> bool {
    name.starts_with("term_meta_bank_") && name.ends_with(".json")
}

fn parse_term_meta_entry(entry: &Value) -> Option<JitenFrequencyEntry> {
    let parts = entry.as_array()?;
    if parts.len() < 3 || parts.get(1)?.as_str()? != "freq" {
        return None;
    }

    let term = parts.first()?.as_str()?.trim();
    if term.is_empty() {
        return None;
    }

    let data = parts.get(2)?.as_object()?;
    let reading = data
        .get("reading")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reading| !reading.is_empty())
        .map(ToOwned::to_owned);
    let value = data
        .get("frequency")
        .and_then(|frequency| frequency.get("value"))
        .and_then(Value::as_u64)
        .or_else(|| data.get("value").and_then(Value::as_u64))?;

    Some(JitenFrequencyEntry {
        term: term.to_string(),
        reading,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn make_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn maps_sources_to_jiten_link_types() {
        assert_eq!(source_to_link_type("vndb"), Some(2));
        assert_eq!(source_to_link_type("anilist"), Some(4));
        assert_eq!(source_to_link_type("unknown"), None);
    }

    #[test]
    fn keeps_vndb_v_prefix_for_jiten() {
        assert_eq!(normalize_external_id_for_jiten("vndb", "v17"), "v17");
        assert_eq!(
            normalize_external_id_for_jiten("vndb", "https://vndb.org/v17"),
            "v17"
        );
    }

    #[test]
    fn parses_deck_id_array() {
        let ids: Vec<i32> = serde_json::from_str("[22541,118931]").unwrap();
        assert_eq!(ids, vec![22541, 118931]);
    }

    #[test]
    fn parse_yomitan_frequency_zip_handles_no_reading_entry() {
        let zip = make_zip(&[(
            "term_meta_bank_1.json",
            r#"[["の","freq",{"value":2410,"displayValue":"2410㋕"}]]"#,
        )]);

        let entries = JitenClient::parse_yomitan_frequency_zip(&zip).unwrap();

        assert_eq!(
            entries,
            vec![JitenFrequencyEntry {
                term: "の".to_string(),
                reading: None,
                value: 2410
            }]
        );
    }

    #[test]
    fn parse_yomitan_frequency_zip_handles_reading_entry() {
        let zip = make_zip(&[(
            "term_meta_bank_1.json",
            r#"[["岡部","freq",{"reading":"おかべ","frequency":{"value":1645,"displayValue":"1645"}}]]"#,
        )]);

        let entries = JitenClient::parse_yomitan_frequency_zip(&zip).unwrap();

        assert_eq!(
            entries,
            vec![JitenFrequencyEntry {
                term: "岡部".to_string(),
                reading: Some("おかべ".to_string()),
                value: 1645
            }]
        );
    }

    #[test]
    fn parse_yomitan_frequency_zip_ignores_non_freq_metadata() {
        let zip = make_zip(&[(
            "term_meta_bank_1.json",
            r#"[["岡部","pitch",{"reading":"おかべ"}],["の","freq",{"value":1}]]"#,
        )]);

        let entries = JitenClient::parse_yomitan_frequency_zip(&zip).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "の");
    }

    #[test]
    fn parse_yomitan_frequency_zip_rejects_invalid_zip() {
        let error = JitenClient::parse_yomitan_frequency_zip(b"not zip").unwrap_err();
        assert!(error.contains("Invalid Jiten frequency ZIP"));
    }

    #[test]
    fn parse_yomitan_frequency_zip_reads_multiple_term_meta_banks() {
        let zip = make_zip(&[
            (
                "term_meta_bank_2.json",
                r#"[["岡部","freq",{"reading":"おかべ","frequency":{"value":2}}]]"#,
            ),
            ("index.json", r#"{"title":"ignored"}"#),
            ("term_meta_bank_1.json", r#"[["の","freq",{"value":1}]]"#),
        ]);

        let entries = JitenClient::parse_yomitan_frequency_zip(&zip).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.term == "の"));
        assert!(entries.iter().any(|entry| entry.term == "岡部"));
    }

    #[test]
    fn builds_anilist_deck_lookup_path() {
        let link_type = source_to_link_type("anilist").unwrap();
        let id = normalize_external_id_for_jiten("anilist", "9253");

        assert_eq!(
            deck_lookup_path(link_type, &id),
            "/api/media-deck/by-link-id/4/9253"
        );
    }

    #[test]
    fn builds_vndb_deck_lookup_path_with_v_prefix() {
        let link_type = source_to_link_type("vndb").unwrap();
        let id = normalize_external_id_for_jiten("vndb", "https://vndb.org/v17");

        assert_eq!(
            deck_lookup_path(link_type, &id),
            "/api/media-deck/by-link-id/2/v17"
        );
    }

    #[test]
    fn builds_jiten_yomitan_frequency_download_payload() {
        let payload = yomitan_frequency_download_payload();

        assert_eq!(payload["format"], 5);
        assert_eq!(payload["downloadType"], 1);
        assert_eq!(payload["order"], 3);
        assert_eq!(payload["excludeKana"], false);
        assert_eq!(payload["excludeExampleSentences"], true);
    }
}
