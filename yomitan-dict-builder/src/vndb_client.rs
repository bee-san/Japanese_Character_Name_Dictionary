use reqwest::Client;
use std::collections::HashMap;

use crate::models::*;

/// Maximum number of retries on HTTP 429 (rate limited).
const MAX_RETRIES: u32 = 3;

/// Send a request with automatic retry on HTTP 429 (Too Many Requests).
/// Uses exponential backoff: 1s, 2s, 4s.
async fn send_with_retry(
    request_builder: reqwest::RequestBuilder,
    client: &Client,
) -> Result<reqwest::Response, reqwest::Error> {
    // We need to clone the request for retries, so build it first
    let request = request_builder.build()?;
    let mut delay_ms = 1000u64;

    for attempt in 0..=MAX_RETRIES {
        let req_clone = request.try_clone().expect("Request body must be cloneable");
        let response = client.execute(req_clone).await?;

        if response.status() == 429 && attempt < MAX_RETRIES {
            // Check for Retry-After header
            if let Some(retry_after) = response.headers().get("retry-after") {
                if let Ok(secs) = retry_after.to_str().unwrap_or("").parse::<u64>() {
                    tokio::time::sleep(tokio::time::Duration::from_secs(secs.min(10))).await;
                    continue;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            delay_ms *= 2;
            continue;
        }

        return Ok(response);
    }

    // Shouldn't reach here, but just in case
    client.execute(request).await
}

/// Parsed result from user input: either a direct user ID or a username to resolve.
enum ParsedUserInput {
    UserId(String),
    Username(String),
}

pub struct VndbClient {
    client: Client,
}

/// Information returned from the VNDB VN endpoint, including title and voice actor mapping.
pub struct VnInfo {
    pub title: String,                    // romanized
    pub alttitle: String,                 // Japanese
    pub va_map: HashMap<String, String>,  // character_id → VA display name
}

impl VndbClient {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// Parse a VNDB user input which may be a URL, user ID, or username.
    /// Supports formats like:
    ///   - "https://vndb.org/u306587"
    ///   - "vndb.org/u306587"
    ///   - "u306587"
    ///   - "yorhel" (plain username)
    ///     Returns either a resolved user ID or the cleaned username for API lookup.
    fn parse_user_input(input: &str) -> ParsedUserInput {
        let input = input.trim();

        // Try to parse as URL or URL-like path containing /uNNNN
        // Match patterns like https://vndb.org/u306587 or vndb.org/u306587
        if input.contains("vndb.org/") {
            if let Some(pos) = input.rfind("vndb.org/") {
                let after_slash = &input[pos + "vndb.org/".len()..];
                // Extract the path segment (stop at '/' or '?' or '#' or end)
                let segment = after_slash
                    .split(&['/', '?', '#'][..])
                    .next()
                    .unwrap_or("")
                    .trim();
                if !segment.is_empty() {
                    // Check if it's a user ID like "u306587"
                    if segment.starts_with('u')
                        && segment.len() > 1
                        && segment[1..].chars().all(|c| c.is_ascii_digit())
                    {
                        return ParsedUserInput::UserId(segment.to_string());
                    }
                }
            }
        }

        // Check if input is directly a user ID like "u306587"
        if input.starts_with('u')
            && input.len() > 1
            && input[1..].chars().all(|c| c.is_ascii_digit())
        {
            return ParsedUserInput::UserId(input.to_string());
        }

        // Otherwise treat as a username to resolve
        ParsedUserInput::Username(input.to_string())
    }

    /// Resolve a VNDB username to a user ID (e.g. "yorhel" → "u2").
    /// Uses GET /user?q=USERNAME endpoint. Case-insensitive.
    pub async fn resolve_user(&self, username: &str) -> Result<String, String> {
        // First, parse the input to handle URLs and direct user IDs
        match Self::parse_user_input(username) {
            ParsedUserInput::UserId(id) => Ok(id),
            ParsedUserInput::Username(name) => self.resolve_username(&name).await,
        }
    }

    /// Internal: resolve a plain username string via the VNDB API.
    async fn resolve_username(&self, username: &str) -> Result<String, String> {
        let response = send_with_retry(
            self.client
                .get("https://api.vndb.org/kana/user")
                .query(&[("q", username)]),
            &self.client,
        )
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != 200 {
            return Err(format!(
                "VNDB user API returned status {}",
                response.status()
            ));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        // The response has the query as key, value is null or {id, username}
        let user_data = data.get(username).or_else(|| {
            // Try case-insensitive: the API returns with the original casing of the query
            data.as_object().and_then(|obj| obj.values().next())
        });

        match user_data {
            Some(val) if !val.is_null() => val["id"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "User ID not found in response".to_string()),
            _ => Err(format!("VNDB user '{}' not found", username)),
        }
    }

    /// Fetch a user's "Playing" VN list (label ID 1).
    /// Returns a list of VNs the user is currently playing.
    pub async fn fetch_user_playing_list(
        &self,
        username: &str,
    ) -> Result<Vec<UserMediaEntry>, String> {
        // Step 1: Resolve username → user ID
        let user_id = self.resolve_user(username).await?;

        let mut entries = Vec::new();
        let mut page = 1;

        loop {
            let payload = serde_json::json!({
                "user": &user_id,
                "fields": "id, labels{id,label}, vn{title,alttitle}",
                "filters": ["label", "=", 1],
                "sort": "lastmod",
                "reverse": true,
                "results": 100,
                "page": page
            });

            let response = send_with_retry(
                self.client
                    .post("https://api.vndb.org/kana/ulist")
                    .json(&payload),
                &self.client,
            )
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

            if response.status() != 200 {
                return Err(format!(
                    "VNDB ulist API returned status {}",
                    response.status()
                ));
            }

            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse JSON: {}", e))?;

            let results = data["results"]
                .as_array()
                .ok_or("Invalid ulist response format")?;

            for item in results {
                let id = item["id"].as_str().unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }

                let title_romaji = item["vn"]["title"].as_str().unwrap_or("").to_string();
                let title_japanese = item["vn"]["alttitle"].as_str().unwrap_or("").to_string();

                // Prefer Japanese title, fall back to romaji
                let title = if !title_japanese.is_empty() {
                    title_japanese
                } else {
                    title_romaji.clone()
                };

                entries.push(UserMediaEntry {
                    id,
                    title,
                    title_romaji,
                    source: "vndb".to_string(),
                    media_type: "vn".to_string(),
                });
            }

            if !data["more"].as_bool().unwrap_or(false) {
                break;
            }

            page += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        Ok(entries)
    }

    /// Normalize VN ID: accepts "17", "v17", "V17" → always returns "v17".
    pub fn normalize_id(id: &str) -> String {
        let id = id.trim();
        if id.to_lowercase().starts_with('v') {
            format!("v{}", &id[1..])
        } else {
            format!("v{}", id)
        }
    }

    /// Fetch VN info including title and voice actor mapping.
    pub async fn fetch_vn_info(&self, vn_id: &str) -> Result<VnInfo, String> {
        let vn_id = Self::normalize_id(vn_id);
        let payload = serde_json::json!({
            "filters": ["id", "=", &vn_id],
            "fields": "title, alttitle, va.staff.name, va.staff.original, va.character.id"
        });

        let response = send_with_retry(
            self.client
                .post("https://api.vndb.org/kana/vn")
                .json(&payload),
            &self.client,
        )
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != 200 {
            return Err(format!("VNDB VN API returned status {}", response.status()));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        let results = data["results"].as_array().ok_or("No results")?;
        if results.is_empty() {
            return Err("VN not found".to_string());
        }

        let vn = &results[0];
        let title = vn["title"].as_str().unwrap_or("").to_string();
        let alttitle = vn["alttitle"].as_str().unwrap_or("").to_string();

        // Build VA map: character_id → voice actor display name
        let mut va_map = HashMap::new();
        if let Some(va_arr) = vn["va"].as_array() {
            for entry in va_arr {
                let char_id = entry["character"]["id"].as_str().unwrap_or("");
                let va_name = entry["staff"]["original"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| entry["staff"]["name"].as_str())
                    .unwrap_or("")
                    .to_string();
                if !char_id.is_empty() && !va_name.is_empty() {
                    // First VA wins (don't overwrite if character has multiple VAs)
                    va_map.entry(char_id.to_string()).or_insert(va_name);
                }
            }
        }

        Ok(VnInfo {
            title,
            alttitle,
            va_map,
        })
    }

    /// Fetch all characters for a VN, with automatic pagination.
    pub async fn fetch_characters(&self, vn_id: &str) -> Result<CharacterData, String> {
        let vn_id = Self::normalize_id(vn_id);
        let mut char_data = CharacterData::new();
        let mut page = 1;

        loop {
            let payload = serde_json::json!({
                "filters": ["vn", "=", ["id", "=", &vn_id]],
                "fields": "id,name,original,image.url,sex,birthday,age,blood_type,height,weight,description,aliases,vns.role,vns.id,traits.name,traits.group_name,traits.spoiler",
                "results": 100,
                "page": page
            });

            let response = send_with_retry(
                self.client
                    .post("https://api.vndb.org/kana/character")
                    .json(&payload),
                &self.client,
            )
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

            if response.status() != 200 {
                return Err(format!("VNDB API returned status {}", response.status()));
            }

            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse JSON: {}", e))?;

            let results = data["results"]
                .as_array()
                .ok_or("Invalid response format")?;

            for char_json in results {
                if let Some(character) = self.process_character(char_json, &vn_id) {
                    match character.role.as_str() {
                        "main" => char_data.main.push(character),
                        "primary" => char_data.primary.push(character),
                        "side" => char_data.side.push(character),
                        "appears" => char_data.appears.push(character),
                        _ => char_data.side.push(character),
                    }
                }
            }

            if !data["more"].as_bool().unwrap_or(false) {
                break;
            }

            page += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        Ok(char_data)
    }

    /// Process a single raw VNDB character JSON value into our Character struct.
    fn process_character(&self, data: &serde_json::Value, target_vn: &str) -> Option<Character> {
        // Find role for this specific VN
        let role = data["vns"]
            .as_array()?
            .iter()
            .find(|v| v["id"].as_str() == Some(target_vn))
            .and_then(|v| v["role"].as_str())
            .unwrap_or("side")
            .to_string();

        // Extract sex from array format: ["m"] → "m"
        let sex = data["sex"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Process traits by group_name
        let empty_vec = vec![];
        let traits = data["traits"].as_array().unwrap_or(&empty_vec);
        let mut personality = Vec::new();
        let mut roles = Vec::new();
        let mut engages_in = Vec::new();
        let mut subject_of = Vec::new();

        for trait_data in traits {
            let name = trait_data["name"].as_str().unwrap_or("").to_string();
            let spoiler = trait_data["spoiler"].as_u64().unwrap_or(0) as u8;
            let group = trait_data["group_name"].as_str().unwrap_or("");

            if name.is_empty() {
                continue;
            }

            let trait_obj = CharacterTrait { name, spoiler };

            match group {
                "Personality" => personality.push(trait_obj),
                "Role" => roles.push(trait_obj),
                "Engages in" => engages_in.push(trait_obj),
                "Subject of" => subject_of.push(trait_obj),
                _ => {} // Ignore other groups
            }
        }

        // Image URL (nested: {"image": {"url": "..."}})
        let image_url = data["image"]["url"].as_str().map(|s| s.to_string());

        // Birthday: [month, day] array
        let birthday = data["birthday"].as_array().and_then(|arr| {
            if arr.len() >= 2 {
                Some(vec![arr[0].as_u64()? as u32, arr[1].as_u64()? as u32])
            } else {
                None
            }
        });

        // Aliases: array of strings
        let aliases = data["aliases"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Some(Character {
            id: data["id"].as_str().unwrap_or("").to_string(),
            name: data["name"].as_str().unwrap_or("").to_string(),
            name_original: data["original"].as_str().unwrap_or("").to_string(),
            role,
            sex,
            age: data["age"].as_u64().map(|a| a.to_string()),
            height: data["height"].as_u64().map(|h| h as u32),
            weight: data["weight"].as_u64().map(|w| w as u32),
            blood_type: data["blood_type"].as_str().map(|s| s.to_string()),
            birthday,
            description: data["description"].as_str().map(|s| s.to_string()),
            aliases,
            personality,
            roles,
            engages_in,
            subject_of,
            image_url,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_id_bare_number() {
        assert_eq!(VndbClient::normalize_id("17"), "v17");
    }

    #[test]
    fn test_normalize_id_lowercase_v() {
        assert_eq!(VndbClient::normalize_id("v17"), "v17");
    }

    #[test]
    fn test_normalize_id_uppercase_v() {
        assert_eq!(VndbClient::normalize_id("V17"), "v17");
    }

    #[test]
    fn test_normalize_id_with_whitespace() {
        assert_eq!(VndbClient::normalize_id("  v17  "), "v17");
    }

    #[test]
    fn test_normalize_id_large_number() {
        assert_eq!(VndbClient::normalize_id("58641"), "v58641");
    }

    // Helper to assert parse_user_input results
    fn assert_user_id(input: &str, expected_id: &str) {
        match VndbClient::parse_user_input(input) {
            ParsedUserInput::UserId(id) => assert_eq!(id, expected_id, "input: {}", input),
            ParsedUserInput::Username(name) => {
                panic!(
                    "Expected UserId('{}') but got Username('{}') for input: {}",
                    expected_id, name, input
                )
            }
        }
    }

    fn assert_username(input: &str, expected_name: &str) {
        match VndbClient::parse_user_input(input) {
            ParsedUserInput::Username(name) => assert_eq!(name, expected_name, "input: {}", input),
            ParsedUserInput::UserId(id) => {
                panic!(
                    "Expected Username('{}') but got UserId('{}') for input: {}",
                    expected_name, id, input
                )
            }
        }
    }

    #[test]
    fn test_parse_user_input_https_url() {
        assert_user_id("https://vndb.org/u306587", "u306587");
    }

    #[test]
    fn test_parse_user_input_http_url() {
        assert_user_id("http://vndb.org/u306587", "u306587");
    }

    #[test]
    fn test_parse_user_input_bare_domain_url() {
        assert_user_id("vndb.org/u306587", "u306587");
    }

    #[test]
    fn test_parse_user_input_url_with_trailing_slash() {
        assert_user_id("https://vndb.org/u306587/", "u306587");
    }

    #[test]
    fn test_parse_user_input_url_with_query_string() {
        assert_user_id("https://vndb.org/u306587?tab=list", "u306587");
    }

    #[test]
    fn test_parse_user_input_url_with_fragment() {
        assert_user_id("https://vndb.org/u306587#top", "u306587");
    }

    #[test]
    fn test_parse_user_input_direct_user_id() {
        assert_user_id("u306587", "u306587");
    }

    #[test]
    fn test_parse_user_input_direct_user_id_small() {
        assert_user_id("u2", "u2");
    }

    #[test]
    fn test_parse_user_input_plain_username() {
        assert_username("yorhel", "yorhel");
    }

    #[test]
    fn test_parse_user_input_plain_username_with_whitespace() {
        assert_username("  yorhel  ", "yorhel");
    }

    #[test]
    fn test_parse_user_input_url_with_whitespace() {
        assert_user_id("  https://vndb.org/u306587  ", "u306587");
    }

    // === Edge case: parse_user_input boundary inputs ===

    #[test]
    fn test_parse_user_input_bare_u() {
        // "u" alone — length is 1, so the `len() > 1` check fails
        assert_username("u", "u");
    }

    #[test]
    fn test_parse_user_input_u_with_non_numeric() {
        // "u123abc" — not all digits after 'u', treated as username
        assert_username("u123abc", "u123abc");
    }

    #[test]
    fn test_parse_user_input_empty() {
        assert_username("", "");
    }

    #[test]
    fn test_parse_user_input_url_with_non_user_path() {
        // vndb.org/v17 — not a user ID (starts with 'v', not 'u')
        assert_username("https://vndb.org/v17", "https://vndb.org/v17");
    }

    #[test]
    fn test_parse_user_input_url_with_username_path() {
        // vndb.org/yorhel — not a uNNN pattern, treated as username
        assert_username("https://vndb.org/yorhel", "https://vndb.org/yorhel");
    }

    // === Edge case: normalize_id boundary inputs ===

    #[test]
    fn test_normalize_id_empty() {
        // Empty string → "v"
        assert_eq!(VndbClient::normalize_id(""), "v");
    }

    #[test]
    fn test_normalize_id_just_v() {
        // "v" alone → "v" (slices &id[1..] which is empty)
        assert_eq!(VndbClient::normalize_id("v"), "v");
    }

    #[test]
    fn test_normalize_id_zero() {
        assert_eq!(VndbClient::normalize_id("0"), "v0");
    }
}
