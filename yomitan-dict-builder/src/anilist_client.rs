use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Client;

use crate::models::*;

pub struct AnilistClient {
    client: Client,
}

impl AnilistClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    const USER_LIST_QUERY: &'static str = r#"
    query ($username: String, $type: MediaType) {
        MediaListCollection(userName: $username, type: $type, status: CURRENT) {
            lists {
                name
                status
                entries {
                    media {
                        id
                        title {
                            romaji
                            english
                            native
                        }
                    }
                }
            }
        }
    }
    "#;

    /// Fetch a user's currently watching/reading media from AniList.
    /// Queries both ANIME and MANGA with status CURRENT.
    pub async fn fetch_user_current_list(
        &self,
        username: &str,
    ) -> Result<Vec<UserMediaEntry>, String> {
        let mut entries = Vec::new();

        for (media_type_gql, media_type_label) in &[("ANIME", "anime"), ("MANGA", "manga")] {
            let variables = serde_json::json!({
                "username": username,
                "type": media_type_gql
            });

            let response = self
                .client
                .post("https://graphql.anilist.co")
                .json(&serde_json::json!({
                    "query": Self::USER_LIST_QUERY,
                    "variables": variables
                }))
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            if response.status() != 200 {
                // AniList returns 404 for non-existent users
                if response.status() == 404 {
                    return Err(format!("AniList user '{}' not found", username));
                }
                return Err(format!(
                    "AniList API returned status {}",
                    response.status()
                ));
            }

            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse JSON: {}", e))?;

            if data["errors"].is_array() {
                let errors = &data["errors"];
                // Check if it's a "user not found" error
                if let Some(first_err) = errors.as_array().and_then(|a| a.first()) {
                    let msg = first_err["message"].as_str().unwrap_or("");
                    if msg.contains("not found") || msg.contains("Private") {
                        return Err(format!("AniList user '{}' not found or private", username));
                    }
                }
                return Err(format!("GraphQL error: {:?}", errors));
            }

            let lists = data["data"]["MediaListCollection"]["lists"]
                .as_array();

            if let Some(lists) = lists {
                for list in lists {
                    let list_entries = list["entries"].as_array();
                    if let Some(list_entries) = list_entries {
                        for entry in list_entries {
                            let media = &entry["media"];
                            let id = media["id"].as_u64().unwrap_or(0);
                            if id == 0 {
                                continue;
                            }

                            let title_data = &media["title"];
                            let title_native = title_data["native"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            let title_romaji = title_data["romaji"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            let title_english = title_data["english"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();

                            // Prefer native (Japanese), fall back to romaji, then english
                            let title = if !title_native.is_empty() {
                                title_native
                            } else if !title_romaji.is_empty() {
                                title_romaji.clone()
                            } else {
                                title_english
                            };

                            entries.push(UserMediaEntry {
                                id: id.to_string(),
                                title,
                                title_romaji,
                                source: "anilist".to_string(),
                                media_type: media_type_label.to_string(),
                            });
                        }
                    }
                }
            }

            // Rate limit delay between ANIME and MANGA queries
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        }

        Ok(entries)
    }

    const CHARACTERS_QUERY: &'static str = r#"
    query ($id: Int!, $type: MediaType, $page: Int, $perPage: Int) {
        Media(id: $id, type: $type) {
            id
            title {
                romaji
                english
                native
            }
            characters(page: $page, perPage: $perPage, sort: [ROLE, RELEVANCE, ID]) {
                pageInfo {
                    hasNextPage
                    currentPage
                }
                edges {
                    role
                    node {
                        id
                        name {
                            full
                            native
                            alternative
                        }
                        image {
                            large
                        }
                        description
                        gender
                        age
                        dateOfBirth {
                            month
                            day
                        }
                        bloodType
                    }
                }
            }
        }
    }
    "#;

    /// Fetch all characters and the media title.
    /// media_type must be "ANIME" or "MANGA".
    pub async fn fetch_characters(
        &self,
        media_id: i32,
        media_type: &str,
    ) -> Result<(CharacterData, String), String> {
        let mut char_data = CharacterData::new();
        let mut page = 1;
        let mut media_title = String::new();

        loop {
            let variables = serde_json::json!({
                "id": media_id,
                "type": media_type.to_uppercase(),
                "page": page,
                "perPage": 25
            });

            let response = self
                .client
                .post("https://graphql.anilist.co")
                .json(&serde_json::json!({
                    "query": Self::CHARACTERS_QUERY,
                    "variables": variables
                }))
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            if response.status() != 200 {
                return Err(format!(
                    "AniList API returned status {}",
                    response.status()
                ));
            }

            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse JSON: {}", e))?;

            if data["errors"].is_array() {
                return Err(format!("GraphQL error: {:?}", data["errors"]));
            }

            let media = &data["data"]["Media"];

            // Extract title on first page
            if page == 1 {
                let title_data = &media["title"];
                media_title = title_data["native"]
                    .as_str()
                    .or_else(|| title_data["romaji"].as_str())
                    .or_else(|| title_data["english"].as_str())
                    .unwrap_or("")
                    .to_string();
            }

            let edges = media["characters"]["edges"]
                .as_array()
                .ok_or("Invalid response format")?;

            for edge in edges {
                if let Some(character) = self.process_character(edge) {
                    match character.role.as_str() {
                        "main" => char_data.main.push(character),
                        "primary" => char_data.primary.push(character),
                        "side" => char_data.side.push(character),
                        _ => char_data.side.push(character),
                    }
                }
            }

            let has_next = media["characters"]["pageInfo"]["hasNextPage"]
                .as_bool()
                .unwrap_or(false);

            if !has_next {
                break;
            }

            page += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        }

        Ok((char_data, media_title))
    }

    /// Process a single AniList character edge into our Character struct.
    fn process_character(&self, edge: &serde_json::Value) -> Option<Character> {
        let node = edge.get("node")?;
        let role_raw = edge["role"].as_str().unwrap_or("BACKGROUND");

        let role = match role_raw {
            "MAIN" => "main",
            "SUPPORTING" => "primary",
            "BACKGROUND" => "side",
            _ => "side",
        }
        .to_string();

        let name_data = node.get("name")?;
        let name_full = name_data["full"].as_str().unwrap_or("").to_string();
        let name_native = name_data["native"].as_str().unwrap_or("").to_string();

        let alternatives: Vec<String> = name_data["alternative"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Gender: "Male" → "m", "Female" → "f"
        let sex = node
            .get("gender")
            .and_then(|g| g.as_str())
            .and_then(|g| match g.to_lowercase().chars().next() {
                Some('m') => Some("m".to_string()),
                Some('f') => Some("f".to_string()),
                _ => None,
            });

        // Birthday: {"month": 9, "day": 1} → [9, 1]
        let birthday = node.get("dateOfBirth").and_then(|dob| {
            let month = dob["month"].as_u64()? as u32;
            let day = dob["day"].as_u64()? as u32;
            Some(vec![month, day])
        });

        // Image URL
        let image_url = node
            .get("image")
            .and_then(|img| img["large"].as_str())
            .map(|s| s.to_string());

        // Age — AniList returns as string, may be "17-18" or similar
        let age = node.get("age").and_then(|v| {
            // Try string first, then integer
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        });

        Some(Character {
            id: node
                .get("id")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .to_string(),
            name: name_full,
            name_original: name_native,
            role,
            sex,
            age,
            height: None,  // AniList doesn't provide
            weight: None,  // AniList doesn't provide
            blood_type: node
                .get("bloodType")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            birthday,
            description: node
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            aliases: alternatives,
            personality: Vec::new(), // AniList has no trait categories
            roles: Vec::new(),
            engages_in: Vec::new(),
            subject_of: Vec::new(),
            image_url,
            image_base64: None,
        })
    }

    /// Download an image and return as base64 data URI string.
    /// Returns None on any failure (network, non-200 status, etc.).
    pub async fn fetch_image_as_base64(&self, url: &str) -> Option<String> {
        let response = self.client.get(url).send().await.ok()?;

        if response.status() != 200 {
            return None;
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = response.bytes().await.ok()?;
        let b64 = STANDARD.encode(&bytes);
        Some(format!("data:{};base64,{}", content_type, b64))
    }
}