use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use tracing::warn;

use crate::models::*;

/// Maximum number of retries on HTTP 429 (rate limited).
const MAX_RETRIES: u32 = 3;

#[derive(Debug)]
enum RequestError {
    Transport(reqwest::Error),
    RateLimited { attempts: u32, last_wait_ms: u64 },
}

impl From<reqwest::Error> for RequestError {
    fn from(value: reqwest::Error) -> Self {
        Self::Transport(value)
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "{}", err),
            Self::RateLimited {
                attempts,
                last_wait_ms,
            } => write!(
                f,
                "VNDB rate limited after {} attempts (last wait {}ms)",
                attempts, last_wait_ms
            ),
        }
    }
}

fn map_request_error(err: RequestError) -> String {
    match err {
        RequestError::Transport(err) => format!("UPSTREAM: VNDB request failed: {}", err),
        RequestError::RateLimited {
            attempts,
            last_wait_ms,
        } => format!(
            "RATE_LIMIT: VNDB temporarily rate limited requests after {} attempts (last wait {}ms)",
            attempts, last_wait_ms
        ),
    }
}

fn non_empty_json_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Send a request with automatic retry on HTTP 429 (Too Many Requests).
/// Uses exponential backoff: 1s, 2s, 4s.
async fn send_with_retry(
    request_builder: reqwest::RequestBuilder,
    client: &Client,
) -> Result<reqwest::Response, RequestError> {
    // We need to clone the request for retries, so build it first
    let request = request_builder.build()?;
    let mut delay_ms = 1000u64;
    let mut last_wait_ms = 0u64;

    for attempt in 0..=MAX_RETRIES {
        let req_clone = request.try_clone().expect("Request body must be cloneable");
        let response = client.execute(req_clone).await?;

        if response.status() == 429 {
            if attempt < MAX_RETRIES {
                // Check for Retry-After header
                let wait_ms = if let Some(retry_after) = response.headers().get("retry-after") {
                    if let Ok(secs) = retry_after.to_str().unwrap_or("").parse::<u64>() {
                        secs.min(10) * 1000
                    } else {
                        delay_ms
                    }
                } else {
                    delay_ms
                };
                last_wait_ms = wait_ms;
                warn!(
                    attempt = attempt + 1,
                    max_retries = MAX_RETRIES,
                    wait_ms = wait_ms,
                    "VNDB rate limited request, retrying"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
                delay_ms *= 2;
                continue;
            }

            warn!(
                attempts = attempt + 1,
                last_wait_ms = last_wait_ms,
                "VNDB rate limit retries exhausted"
            );
            return Err(RequestError::RateLimited {
                attempts: attempt + 1,
                last_wait_ms,
            });
        }

        return Ok(response);
    }

    Err(RequestError::RateLimited {
        attempts: MAX_RETRIES + 1,
        last_wait_ms,
    })
}

/// Parsed result from user input: either a direct user ID or a username to resolve.
enum ParsedUserInput {
    UserId(String),
    Username(String),
}

pub struct VndbClient {
    client: Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VndbShelfStatus {
    Playing,
    Finished,
    Wishlist,
}

impl VndbShelfStatus {
    pub fn default_statuses() -> Vec<Self> {
        vec![Self::Playing]
    }

    pub fn parse_list(raw: Option<&str>) -> Result<Vec<Self>, String> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::default_statuses());
        };

        let mut statuses = Vec::new();
        for token in raw
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let status = match token.to_ascii_lowercase().as_str() {
                "playing" | "current" => Self::Playing,
                "finished" | "completed" => Self::Finished,
                "wishlist" | "planning" | "planned" => Self::Wishlist,
                other => {
                    return Err(format!(
                        "vndb_status contains unsupported value '{}'",
                        other
                    ));
                }
            };
            if !statuses.contains(&status) {
                statuses.push(status);
            }
        }

        if statuses.is_empty() {
            Ok(Self::default_statuses())
        } else {
            Ok(statuses)
        }
    }

    pub fn is_default_list(statuses: &[Self]) -> bool {
        matches!(statuses, [Self::Playing])
    }

    pub fn query_value(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Finished => "finished",
            Self::Wishlist => "wishlist",
        }
    }

    pub fn label_id(self) -> u8 {
        match self {
            Self::Playing => 1,
            Self::Finished => 2,
            Self::Wishlist => 5,
        }
    }
}

/// Information returned from the VNDB VN endpoint, including title and voice actor mapping.
pub struct VnInfo {
    pub title: String,                           // romanized
    pub alttitle: String,                        // Japanese
    pub va_map: HashMap<String, VoiceActorInfo>, // character_id → VA info
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceActorInfo {
    pub staff_id: String,
    pub name: String,
    pub original: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VndbMediaTitles {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
    pub user_preferred: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VndbMediaSearchResult {
    pub id: String,
    pub url: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub titles: VndbMediaTitles,
    pub synonyms: Vec<String>,
    pub format: Option<String>,
    pub year: Option<i32>,
    pub popularity: Option<i32>,
}

impl VndbClient {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    pub fn normalize_user_input(input: &str) -> String {
        match Self::parse_user_input(input) {
            ParsedUserInput::UserId(id) => id,
            ParsedUserInput::Username(name) => name,
        }
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
        .map_err(map_request_error)?;

        if response.status() != 200 {
            return Err(format!(
                "UPSTREAM: VNDB user API returned status {}",
                response.status()
            ));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to parse VNDB JSON: {}", e))?;

        // The response has the query as key, value is null or {id, username}
        let user_data = data.get(username).or_else(|| {
            // Try case-insensitive: the API returns with the original casing of the query
            data.as_object().and_then(|obj| obj.values().next())
        });

        match user_data {
            Some(val) if !val.is_null() => val["id"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "UPSTREAM: User ID not found in VNDB response".to_string()),
            _ => Err(format!("INVALID_INPUT: VNDB user '{}' not found", username)),
        }
    }

    /// Fetch a user's VN list for the selected VNDB shelf labels.
    pub async fn fetch_user_list(
        &self,
        username: &str,
        statuses: &[VndbShelfStatus],
    ) -> Result<Vec<UserMediaEntry>, String> {
        // Step 1: Resolve username → user ID
        let user_id = self.resolve_user(username).await?;
        let statuses = if statuses.is_empty() {
            VndbShelfStatus::default_statuses()
        } else {
            statuses.to_vec()
        };

        let mut entries = Vec::new();
        for status in statuses {
            let mut page = 1;

            loop {
                let payload = serde_json::json!({
                    "user": &user_id,
                    "fields": "id, labels{id,label}, vn{title,alttitle}",
                    "filters": ["label", "=", status.label_id()],
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
                .map_err(map_request_error)?;

                if response.status() != 200 {
                    return Err(format!(
                        "UPSTREAM: VNDB ulist API returned status {}",
                        response.status()
                    ));
                }

                let data: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("UPSTREAM: Failed to parse VNDB JSON: {}", e))?;

                let results = data["results"]
                    .as_array()
                    .ok_or("UPSTREAM: Invalid VNDB ulist response format")?;

                entries.extend(Self::parse_user_list_results(results, status));

                if !data["more"].as_bool().unwrap_or(false) {
                    break;
                }

                page += 1;
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        Ok(entries)
    }

    fn parse_user_list_results(
        results: &[serde_json::Value],
        status: VndbShelfStatus,
    ) -> Vec<UserMediaEntry> {
        let mut entries = Vec::new();

        for item in results {
            let raw_id = item["id"].as_str().unwrap_or("").trim();
            if raw_id.is_empty() {
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

            let id = match Self::parse_vn_id(raw_id) {
                Ok(id) => id,
                Err(error) => {
                    warn!(
                        raw_id = raw_id,
                        title = %title,
                        error = %error,
                        "Skipping VNDB list entry with invalid media ID"
                    );
                    continue;
                }
            };

            entries.push(UserMediaEntry {
                id,
                title,
                title_romaji,
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
                status: status.query_value().to_string(),
            });
        }

        entries
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

    /// Parse a VN ID from "17", "v17", or a full VNDB URL like
    /// "https://vndb.org/v17/..." and return a normalized "v17" form.
    pub fn parse_vn_id(input: &str) -> Result<String, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("Invalid VNDB ID: value is empty".to_string());
        }

        if input.contains("vndb.org/") {
            if let Some(pos) = input.rfind("vndb.org/") {
                let after = &input[pos + "vndb.org/".len()..];
                let segment = after
                    .split(&['/', '?', '#'][..])
                    .next()
                    .unwrap_or("")
                    .trim();
                if segment.len() > 1
                    && segment.to_lowercase().starts_with('v')
                    && segment[1..].chars().all(|c| c.is_ascii_digit())
                {
                    return Ok(format!("v{}", &segment[1..]));
                }
            }

            return Err(format!(
                "Invalid VNDB ID '{}': could not extract a vNNN ID from the URL",
                input
            ));
        }

        if input.len() > 1
            && input.to_lowercase().starts_with('v')
            && input[1..].chars().all(|c| c.is_ascii_digit())
        {
            return Ok(format!("v{}", &input[1..]));
        }

        if input.chars().all(|c| c.is_ascii_digit()) {
            return Ok(format!("v{}", input));
        }

        Err(format!(
            "Invalid VNDB ID '{}': must be a number, vNNN, or VNDB URL",
            input
        ))
    }

    /// Fetch VN info including title and voice actor mapping.
    pub async fn fetch_vn_info(&self, vn_id: &str) -> Result<VnInfo, String> {
        let vn_id = Self::normalize_id(vn_id);
        let payload = serde_json::json!({
            "filters": ["id", "=", &vn_id],
            "fields": "title, alttitle, va.staff.id, va.staff.name, va.staff.original, va.character.id"
        });

        let response = send_with_retry(
            self.client
                .post("https://api.vndb.org/kana/vn")
                .json(&payload),
            &self.client,
        )
        .await
        .map_err(map_request_error)?;

        if response.status() != 200 {
            return Err(format!(
                "UPSTREAM: VNDB VN API returned status {}",
                response.status()
            ));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to parse VNDB JSON: {}", e))?;

        let results = data["results"]
            .as_array()
            .ok_or("UPSTREAM: VNDB VN response did not include results")?;
        if results.is_empty() {
            return Err(format!(
                "INVALID_INPUT: VNDB media '{}' was not found",
                vn_id
            ));
        }

        let vn = &results[0];
        let title = vn["title"].as_str().unwrap_or("").to_string();
        let alttitle = vn["alttitle"].as_str().unwrap_or("").to_string();

        // Build VA map: character_id → voice actor display name
        let mut va_map = HashMap::new();
        if let Some(va_arr) = vn["va"].as_array() {
            for entry in va_arr {
                let char_id = entry["character"]["id"].as_str().unwrap_or("");
                let staff_id = entry["staff"]["id"].as_str().unwrap_or("");
                let name = entry["staff"]["name"].as_str().unwrap_or("").to_string();
                let original = entry["staff"]["original"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let display_name = entry["staff"]["original"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| entry["staff"]["name"].as_str())
                    .unwrap_or("")
                    .to_string();
                if !char_id.is_empty() && !display_name.is_empty() {
                    // First VA wins (don't overwrite if character has multiple VAs)
                    va_map
                        .entry(char_id.to_string())
                        .or_insert_with(|| VoiceActorInfo {
                            staff_id: staff_id.to_string(),
                            name,
                            original,
                            display_name,
                        });
                }
            }
        }

        Ok(VnInfo {
            title,
            alttitle,
            va_map,
        })
    }

    /// Search VNDB visual novels by title for manual media autocomplete.
    pub async fn search_vns(
        &self,
        search: &str,
        limit: usize,
    ) -> Result<Vec<VndbMediaSearchResult>, String> {
        let payload = serde_json::json!({
            "filters": ["search", "=", search.trim()],
            "fields": "id,title,alttitle,released,votecount",
            "sort": "searchrank",
            "results": limit.clamp(1, 8)
        });

        let response = send_with_retry(
            self.client
                .post("https://api.vndb.org/kana/vn")
                .json(&payload),
            &self.client,
        )
        .await
        .map_err(map_request_error)?;

        if response.status() != 200 {
            return Err(format!(
                "UPSTREAM: VNDB VN search API returned status {}",
                response.status()
            ));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("UPSTREAM: Failed to parse VNDB JSON: {}", e))?;

        Self::parse_vn_search_results(&data)
    }

    fn parse_vn_search_results(
        data: &serde_json::Value,
    ) -> Result<Vec<VndbMediaSearchResult>, String> {
        let results = data["results"]
            .as_array()
            .ok_or_else(|| "UPSTREAM: Invalid VNDB VN search response".to_string())?;

        let mut parsed = Vec::new();
        for item in results {
            let id = item["id"].as_str().unwrap_or("").trim();
            if id.is_empty() {
                continue;
            }

            let title = non_empty_json_string(&item["title"]);
            let alttitle = non_empty_json_string(&item["alttitle"]);
            if title.is_none() && alttitle.is_none() {
                continue;
            }

            let user_preferred = alttitle.clone().or_else(|| title.clone());
            let year = item["released"]
                .as_str()
                .and_then(|released| released.get(0..4))
                .and_then(|year| year.parse::<i32>().ok())
                .filter(|year| *year > 0);
            let popularity = item["votecount"]
                .as_i64()
                .and_then(|value| i32::try_from(value).ok());

            parsed.push(VndbMediaSearchResult {
                id: id.to_string(),
                url: format!("https://vndb.org/{id}"),
                media_type: "VN".to_string(),
                titles: VndbMediaTitles {
                    romaji: title,
                    english: None,
                    native: alttitle,
                    user_preferred,
                },
                synonyms: Vec::new(),
                format: Some("VN".to_string()),
                year,
                popularity,
            });
        }

        Ok(parsed)
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
            .map_err(map_request_error)?;

            if response.status() != 200 {
                return Err(format!(
                    "UPSTREAM: VNDB API returned status {}",
                    response.status()
                ));
            }

            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("UPSTREAM: Failed to parse VNDB JSON: {}", e))?;

            let results = data["results"]
                .as_array()
                .ok_or("UPSTREAM: Invalid VNDB character response format")?;

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

        // Explicitly annotate the vector type so Rust can infer the collection target.
        let aliases: Vec<String> = data["aliases"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // If the original name is empty or romanized, prefer a Japanese alias when available.
        let mut name_original = data["original"].as_str().unwrap_or("").to_string();
        if !is_japanese(&name_original) {
            if let Some(jp_alias) = aliases.iter().find(|a| is_japanese(a)) {
                name_original = jp_alias.clone();
            }
        }

        Some(Character {
            id: data["id"].as_str().unwrap_or("").to_string(),
            name: data["name"].as_str().unwrap_or("").to_string(),
            name_original,
            role,
            source: "vndb".to_string(),
            sex,
            age: data["age"].as_u64().map(|a| a.to_string()),
            height: data["height"].as_u64().map(|h| h as u32),
            weight: data["weight"].as_u64().map(|w| w as u32),
            blood_type: data["blood_type"].as_str().map(|s| s.to_string()),
            birthday,
            description: data["description"].as_str().map(|s| s.to_string()),
            aliases,
            spoiler_aliases: Vec::new(), // VNDB has no spoiler alternative names
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
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
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

    #[test]
    fn test_parse_vn_id_bare_number() {
        assert_eq!(VndbClient::parse_vn_id("17").unwrap(), "v17");
    }

    #[test]
    fn test_parse_vn_id_prefixed_value() {
        assert_eq!(VndbClient::parse_vn_id("v17").unwrap(), "v17");
    }

    #[test]
    fn test_parse_vn_id_full_url() {
        assert_eq!(
            VndbClient::parse_vn_id("https://vndb.org/v17").unwrap(),
            "v17"
        );
    }

    #[test]
    fn test_parse_vn_id_full_url_with_slug_and_query() {
        assert_eq!(
            VndbClient::parse_vn_id("https://vndb.org/v17/steins-gate?view=chars").unwrap(),
            "v17"
        );
    }

    #[test]
    fn test_parse_vn_id_invalid_string() {
        assert!(VndbClient::parse_vn_id("hello").is_err());
    }

    #[test]
    fn test_parse_user_list_results_normalizes_url_ids() {
        let results = vec![serde_json::json!({
            "id": "https://vndb.org/v17763",
            "vn": {
                "title": "Muv-Luv Alternative",
                "alttitle": "マブラヴ オルタネイティヴ"
            }
        })];

        let entries = VndbClient::parse_user_list_results(&results, VndbShelfStatus::Playing);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "v17763");
        assert_eq!(entries[0].title, "マブラヴ オルタネイティヴ");
        assert_eq!(entries[0].status, "playing");
    }

    #[test]
    fn test_vndb_shelf_status_parsing_and_label_mapping() {
        let statuses = VndbShelfStatus::parse_list(Some("playing,finished,wishlist")).unwrap();
        let labels: Vec<u8> = statuses.iter().map(|status| status.label_id()).collect();
        let query_values: Vec<&str> = statuses.iter().map(|status| status.query_value()).collect();

        assert_eq!(labels, vec![1, 2, 5]);
        assert_eq!(query_values, vec!["playing", "finished", "wishlist"]);
    }

    #[test]
    fn test_vndb_shelf_status_default_is_playing() {
        let statuses = VndbShelfStatus::parse_list(None).unwrap();

        assert_eq!(statuses, vec![VndbShelfStatus::Playing]);
        assert!(VndbShelfStatus::is_default_list(&statuses));
    }

    #[test]
    fn test_non_empty_json_string_trims_and_rejects_blank_values() {
        assert_eq!(
            non_empty_json_string(&serde_json::json!("  Steins;Gate  ")).as_deref(),
            Some("Steins;Gate")
        );
        assert_eq!(non_empty_json_string(&serde_json::json!("   ")), None);
        assert_eq!(non_empty_json_string(&serde_json::json!(17)), None);
    }

    #[test]
    fn test_parse_vn_search_results_maps_vndb_shape() {
        let data = serde_json::json!({
            "results": [
                {
                    "id": "v17",
                    "title": "Ever17 -The Out of Infinity-",
                    "alttitle": "Ever17 -the out of infinity-",
                    "released": "2002-08-29",
                    "votecount": 14823
                }
            ]
        });

        let results = VndbClient::parse_vn_search_results(&data).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "v17");
        assert_eq!(results[0].url, "https://vndb.org/v17");
        assert_eq!(results[0].media_type, "VN");
        assert_eq!(
            results[0].titles.romaji.as_deref(),
            Some("Ever17 -The Out of Infinity-")
        );
        assert_eq!(
            results[0].titles.native.as_deref(),
            Some("Ever17 -the out of infinity-")
        );
        assert_eq!(results[0].year, Some(2002));
        assert_eq!(results[0].popularity, Some(14823));
    }

    #[test]
    fn test_parse_vn_search_results_handles_alt_title_only_and_invalid_metrics() {
        let data = serde_json::json!({
            "results": [
                {
                    "id": "v42",
                    "title": " ",
                    "alttitle": "終のステラ",
                    "released": "TBA",
                    "votecount": 999999999999i64
                }
            ]
        });

        let results = VndbClient::parse_vn_search_results(&data).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "v42");
        assert_eq!(results[0].titles.romaji, None);
        assert_eq!(results[0].titles.native.as_deref(), Some("終のステラ"));
        assert_eq!(
            results[0].titles.user_preferred.as_deref(),
            Some("終のステラ")
        );
        assert_eq!(results[0].year, None);
        assert_eq!(results[0].popularity, None);
    }

    #[test]
    fn test_parse_vn_search_results_skips_entries_without_titles() {
        let data = serde_json::json!({
            "results": [
                {"id": "v1", "title": "", "alttitle": null},
                {"id": "", "title": "Missing ID", "alttitle": null},
                {"id": "v2", "title": "Valid VN", "alttitle": ""}
            ]
        });

        let results = VndbClient::parse_vn_search_results(&data).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "v2");
        assert_eq!(
            results[0].titles.user_preferred.as_deref(),
            Some("Valid VN")
        );
    }

    #[test]
    fn test_parse_vn_search_results_rejects_missing_results_array() {
        let data = serde_json::json!({"items": []});

        let error = VndbClient::parse_vn_search_results(&data).unwrap_err();

        assert!(error.contains("Invalid VNDB VN search response"));
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

fn is_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        let cp = c as u32;
        (0x3040..=0x309F).contains(&cp)
            || (0x30A0..=0x30FF).contains(&cp)
            || (0x4E00..=0x9FFF).contains(&cp)
    })
}
