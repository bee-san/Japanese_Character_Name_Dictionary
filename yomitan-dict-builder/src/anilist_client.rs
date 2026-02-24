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


#[cfg(test)]
mod tests {
    use super::*;

    fn make_client() -> AnilistClient {
        AnilistClient::new()
    }

    // ── process_character tests ──

    fn make_edge(role: &str, id: u64, full: &str, native: &str, gender: Option<&str>, age: Option<serde_json::Value>, dob: Option<(u64, u64)>, blood: Option<&str>, desc: Option<&str>, alts: Vec<&str>, image: Option<&str>) -> serde_json::Value {
        let mut node = serde_json::json!({
            "id": id,
            "name": {
                "full": full,
                "native": native,
                "alternative": alts
            },
            "description": desc,
            "gender": gender,
            "age": age,
            "bloodType": blood,
        });
        if let Some((m, d)) = dob {
            node["dateOfBirth"] = serde_json::json!({"month": m, "day": d});
        } else {
            node["dateOfBirth"] = serde_json::json!(null);
        }
        if let Some(url) = image {
            node["image"] = serde_json::json!({"large": url});
        } else {
            node["image"] = serde_json::json!({"large": null});
        }
        serde_json::json!({
            "role": role,
            "node": node
        })
    }

    #[test]
    fn test_process_character_main_role() {
        let client = make_client();
        let edge = make_edge("MAIN", 12345, "Lelouch Lamperouge", "ルルーシュ・ランペルージ", Some("Male"), Some(serde_json::json!("17")), Some((12, 5)), Some("A"), Some("The protagonist."), vec!["Zero"], Some("https://example.com/img.jpg"));
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.id, "12345");
        assert_eq!(ch.name, "Lelouch Lamperouge");
        assert_eq!(ch.name_original, "ルルーシュ・ランペルージ");
        assert_eq!(ch.role, "main");
        assert_eq!(ch.sex, Some("m".to_string()));
        assert_eq!(ch.age, Some("17".to_string()));
        assert_eq!(ch.birthday, Some(vec![12, 5]));
        assert_eq!(ch.blood_type, Some("A".to_string()));
        assert_eq!(ch.description, Some("The protagonist.".to_string()));
        assert_eq!(ch.aliases, vec!["Zero".to_string()]);
        assert_eq!(ch.image_url, Some("https://example.com/img.jpg".to_string()));
        assert!(ch.image_base64.is_none());
        assert!(ch.height.is_none());
        assert!(ch.weight.is_none());
        assert!(ch.personality.is_empty());
        assert!(ch.roles.is_empty());
        assert!(ch.engages_in.is_empty());
        assert!(ch.subject_of.is_empty());
    }

    #[test]
    fn test_process_character_supporting_maps_to_primary() {
        let client = make_client();
        let edge = make_edge("SUPPORTING", 99, "Kallen Stadtfeld", "紅月カレン", Some("Female"), None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.role, "primary");
        assert_eq!(ch.sex, Some("f".to_string()));
        assert!(ch.age.is_none());
        assert!(ch.birthday.is_none());
        assert!(ch.blood_type.is_none());
        assert!(ch.description.is_none());
        assert!(ch.aliases.is_empty());
        assert!(ch.image_url.is_none());
    }

    #[test]
    fn test_process_character_background_maps_to_side() {
        let client = make_client();
        let edge = make_edge("BACKGROUND", 50, "Extra", "", None, None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.role, "side");
        assert_eq!(ch.name_original, "");
    }

    #[test]
    fn test_process_character_unknown_role_maps_to_side() {
        let client = make_client();
        let edge = make_edge("UNKNOWN_ROLE", 50, "Extra", "", None, None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.role, "side");
    }

    #[test]
    fn test_process_character_age_as_string() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, Some(serde_json::json!("17-18")), None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.age, Some("17-18".to_string()));
    }

    #[test]
    fn test_process_character_age_as_integer() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, Some(serde_json::json!(25)), None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.age, Some("25".to_string()));
    }

    #[test]
    fn test_process_character_age_null() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert!(ch.age.is_none());
    }

    #[test]
    fn test_process_character_gender_nonbinary_returns_none() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", Some("Non-binary"), None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        // "Non-binary" starts with 'n', which is neither 'm' nor 'f'
        assert!(ch.sex.is_none());
    }

    #[test]
    fn test_process_character_gender_null() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec![], None);
        let ch = client.process_character(&edge).unwrap();
        assert!(ch.sex.is_none());
    }

    #[test]
    fn test_process_character_multiple_aliases() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec!["Alias1", "Alias2", "Alias3"], None);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.aliases, vec!["Alias1", "Alias2", "Alias3"]);
    }

    #[test]
    fn test_process_character_empty_aliases_filtered() {
        let client = make_client();
        // Build edge with empty string alias mixed in
        let mut edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec![], None);
        edge["node"]["name"]["alternative"] = serde_json::json!(["Good", "", "Also Good", ""]);
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.aliases, vec!["Good", "Also Good"]);
    }

    #[test]
    fn test_process_character_missing_node_returns_none() {
        let client = make_client();
        let edge = serde_json::json!({"role": "MAIN"});
        assert!(client.process_character(&edge).is_none());
    }

    #[test]
    fn test_process_character_missing_name_returns_none() {
        let client = make_client();
        let edge = serde_json::json!({"role": "MAIN", "node": {"id": 1}});
        assert!(client.process_character(&edge).is_none());
    }

    #[test]
    fn test_process_character_birthday_partial_null() {
        // AniList can return {"month": 5, "day": null} for unknown day
        let client = make_client();
        let mut edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec![], None);
        edge["node"]["dateOfBirth"] = serde_json::json!({"month": 5, "day": null});
        let ch = client.process_character(&edge).unwrap();
        // day is null → as_u64() returns None → whole birthday is None
        assert!(ch.birthday.is_none());
    }

    #[test]
    fn test_process_character_id_zero_when_missing() {
        let client = make_client();
        let mut edge = make_edge("MAIN", 0, "A", "あ", None, None, None, None, None, vec![], None);
        edge["node"].as_object_mut().unwrap().remove("id");
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.id, "0");
    }

    #[test]
    fn test_process_character_no_role_defaults_to_side() {
        let client = make_client();
        let mut edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, None, vec![], None);
        edge.as_object_mut().unwrap().remove("role");
        let ch = client.process_character(&edge).unwrap();
        // role_raw defaults to "BACKGROUND" when missing → maps to "side"
        assert_eq!(ch.role, "side");
    }

    #[test]
    fn test_process_character_description_with_anilist_spoilers() {
        let client = make_client();
        let edge = make_edge("MAIN", 1, "A", "あ", None, None, None, None, Some("Visible text ~!hidden spoiler!~ more text"), vec![], None);
        let ch = client.process_character(&edge).unwrap();
        // process_character stores raw description; spoiler stripping happens in content_builder
        assert_eq!(ch.description.unwrap(), "Visible text ~!hidden spoiler!~ more text");
    }

    // ── GraphQL query structure tests ──

    #[test]
    fn test_user_list_query_is_valid_graphql_shape() {
        let query = AnilistClient::USER_LIST_QUERY;
        assert!(query.contains("MediaListCollection"));
        assert!(query.contains("userName"));
        assert!(query.contains("status: CURRENT"));
        assert!(query.contains("$type: MediaType"));
        assert!(query.contains("title"));
        assert!(query.contains("romaji"));
        assert!(query.contains("native"));
    }

    #[test]
    fn test_characters_query_is_valid_graphql_shape() {
        let query = AnilistClient::CHARACTERS_QUERY;
        assert!(query.contains("Media(id: $id, type: $type)"));
        assert!(query.contains("characters(page: $page"));
        assert!(query.contains("hasNextPage"));
        assert!(query.contains("name {"));
        assert!(query.contains("full"));
        assert!(query.contains("native"));
        assert!(query.contains("alternative"));
        assert!(query.contains("dateOfBirth"));
        assert!(query.contains("bloodType"));
        assert!(query.contains("gender"));
        assert!(query.contains("age"));
        assert!(query.contains("description"));
        assert!(query.contains("sort: [ROLE, RELEVANCE, ID]"));
    }

    // ── Role categorization in fetch_characters response simulation ──

    #[test]
    fn test_role_categorization_all_types() {
        let client = make_client();
        let edges = vec![
            make_edge("MAIN", 1, "Main Char", "主人公", None, None, None, None, None, vec![], None),
            make_edge("SUPPORTING", 2, "Support Char", "サポート", None, None, None, None, None, vec![], None),
            make_edge("BACKGROUND", 3, "BG Char", "背景", None, None, None, None, None, vec![], None),
        ];

        let mut char_data = CharacterData::new();
        for edge in &edges {
            if let Some(character) = client.process_character(edge) {
                match character.role.as_str() {
                    "main" => char_data.main.push(character),
                    "primary" => char_data.primary.push(character),
                    "side" => char_data.side.push(character),
                    _ => char_data.side.push(character),
                }
            }
        }
        assert_eq!(char_data.main.len(), 1);
        assert_eq!(char_data.main[0].name, "Main Char");
        assert_eq!(char_data.primary.len(), 1);
        assert_eq!(char_data.primary[0].name, "Support Char");
        assert_eq!(char_data.side.len(), 1);
        assert_eq!(char_data.side[0].name, "BG Char");
        assert!(char_data.appears.is_empty());
    }

    // ── User list response parsing simulation ──

    #[test]
    fn test_user_list_response_parsing() {
        // Simulate the JSON structure AniList returns for MediaListCollection
        let response_json = serde_json::json!({
            "data": {
                "MediaListCollection": {
                    "lists": [{
                        "name": "Watching",
                        "status": "CURRENT",
                        "entries": [

                            {
                                "media": {
                                    "id": 9253,
                                    "title": {
                                        "romaji": "Steins;Gate",
                                        "english": "Steins;Gate",
                                        "native": "シュタインズ・ゲート"
                                    }
                                }
                            },
                            {
                                "media": {
                                    "id": 1535,
                                    "title": {
                                        "romaji": "Death Note",
                                        "english": "Death Note",
                                        "native": null
                                    }
                                }
                            }
                        ]
                    }]
                }
            }
        });

        // Parse entries the same way the client does
        let mut entries = Vec::new();
        let lists = response_json["data"]["MediaListCollection"]["lists"].as_array().unwrap();
        for list in lists {
            let list_entries = list["entries"].as_array().unwrap();
            for entry in list_entries {
                let media = &entry["media"];
                let id = media["id"].as_u64().unwrap_or(0);
                if id == 0 { continue; }

                let title_data = &media["title"];
                let title_native = title_data["native"].as_str().unwrap_or("").to_string();
                let title_romaji = title_data["romaji"].as_str().unwrap_or("").to_string();
                let title_english = title_data["english"].as_str().unwrap_or("").to_string();
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
                    media_type: "anime".to_string(),
                });
            }
        }

        assert_eq!(entries.len(), 2);
        // First entry has native title → should prefer it
        assert_eq!(entries[0].id, "9253");
        assert_eq!(entries[0].title, "シュタインズ・ゲート");
        assert_eq!(entries[0].title_romaji, "Steins;Gate");
        // Second entry has null native → falls back to romaji
        assert_eq!(entries[1].id, "1535");
        assert_eq!(entries[1].title, "Death Note");
    }

    #[test]
    fn test_user_list_response_empty_lists() {
        // When user has no current anime/manga, lists can be empty or null
        let response_json = serde_json::json!({
            "data": {
                "MediaListCollection": {
                    "lists": []
                }
            }
        });
        let lists = response_json["data"]["MediaListCollection"]["lists"].as_array();
        assert!(lists.unwrap().is_empty());
    }

    #[test]
    fn test_user_list_response_null_collection() {
        // When user doesn't exist or has private list, collection can be null
        let response_json = serde_json::json!({
            "data": {
                "MediaListCollection": null
            }
        });
        let lists = response_json["data"]["MediaListCollection"]["lists"].as_array();
        assert!(lists.is_none());
    }

    #[test]
    fn test_user_list_skips_zero_id() {
        let response_json = serde_json::json!({
            "data": {
                "MediaListCollection": {
                    "lists": [{
                        "entries": [{
                            "media": {
                                "id": 0,
                                "title": {"romaji": "Bad", "english": null, "native": null}
                            }
                        }]
                    }]
                }
            }
        });
        let lists = response_json["data"]["MediaListCollection"]["lists"].as_array().unwrap();
        let mut entries = Vec::new();
        for list in lists {
            for entry in list["entries"].as_array().unwrap() {
                let id = entry["media"]["id"].as_u64().unwrap_or(0);
                if id == 0 { continue; }
                entries.push(id);
            }
        }
        assert!(entries.is_empty());
    }

    // ── Error response parsing simulation ──

    #[test]
    fn test_graphql_error_detection() {
        let response = serde_json::json!({
            "data": null,
            "errors": [{
                "message": "User not found",
                "status": 404
            }]
        });
        assert!(response["errors"].is_array());
        let msg = response["errors"][0]["message"].as_str().unwrap();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_graphql_private_user_error() {
        let response = serde_json::json!({
            "data": null,
            "errors": [{
                "message": "Private User",
                "status": 403
            }]
        });
        let msg = response["errors"][0]["message"].as_str().unwrap();
        assert!(msg.contains("Private"));
    }

    #[test]
    fn test_no_errors_field_is_not_array() {
        let response = serde_json::json!({"data": {"Media": {}}});
        assert!(!response["errors"].is_array());
    }

    // ── Characters response parsing simulation ──

    #[test]
    fn test_characters_response_title_extraction() {
        let response = serde_json::json!({
            "data": {
                "Media": {
                    "title": {
                        "native": "シュタインズ・ゲート",
                        "romaji": "Steins;Gate",
                        "english": "Steins;Gate"
                    },
                    "characters": {
                        "pageInfo": {"hasNextPage": false, "currentPage": 1},
                        "edges": []
                    }
                }
            }
        });
        let media = &response["data"]["Media"];
        let title_data = &media["title"];
        let title = title_data["native"].as_str()
            .or_else(|| title_data["romaji"].as_str())
            .or_else(|| title_data["english"].as_str())
            .unwrap_or("");
        assert_eq!(title, "シュタインズ・ゲート");
    }

    #[test]
    fn test_characters_response_title_fallback_to_romaji() {
        let response = serde_json::json!({
            "data": {
                "Media": {
                    "title": {
                        "native": null,
                        "romaji": "Steins;Gate",
                        "english": "Steins;Gate"
                    },
                    "characters": {
                        "pageInfo": {"hasNextPage": false},
                        "edges": []
                    }
                }
            }
        });
        let title_data = &response["data"]["Media"]["title"];
        let title = title_data["native"].as_str()
            .or_else(|| title_data["romaji"].as_str())
            .or_else(|| title_data["english"].as_str())
            .unwrap_or("");
        assert_eq!(title, "Steins;Gate");
    }

    #[test]
    fn test_characters_response_pagination_detection() {
        let page1 = serde_json::json!({
            "data": {"Media": {"characters": {"pageInfo": {"hasNextPage": true, "currentPage": 1}, "edges": []}}}
        });
        let page2 = serde_json::json!({
            "data": {"Media": {"characters": {"pageInfo": {"hasNextPage": false, "currentPage": 2}, "edges": []}}}
        });
        assert!(page1["data"]["Media"]["characters"]["pageInfo"]["hasNextPage"].as_bool().unwrap());
        assert!(!page2["data"]["Media"]["characters"]["pageInfo"]["hasNextPage"].as_bool().unwrap());
    }

    // ── Full character edge processing from realistic API response ──

    #[test]
    fn test_realistic_anilist_character_edge() {
        let client = make_client();
        let edge = serde_json::json!({
            "role": "MAIN",
            "node": {
                "id": 35252,
                "name": {
                    "full": "Okabe Rintarou",
                    "native": "岡部 倫太郎",
                    "alternative": ["Hououin Kyouma", "Okarin"]
                },
                "image": {
                    "large": "https://s4.anilist.co/file/anilistcdn/character/large/b35252-Z1j0Uf60wSfL.png"
                },
                "description": "Okabe is the founder of the Future Gadget Laboratory. ~!He discovers time travel!~",
                "gender": "Male",
                "age": "18",
                "dateOfBirth": {"month": 12, "day": 14},
                "bloodType": "A"
            }
        });
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.id, "35252");
        assert_eq!(ch.name, "Okabe Rintarou");
        assert_eq!(ch.name_original, "岡部 倫太郎");
        assert_eq!(ch.role, "main");
        assert_eq!(ch.sex, Some("m".to_string()));
        assert_eq!(ch.age, Some("18".to_string()));
        assert_eq!(ch.birthday, Some(vec![12, 14]));
        assert_eq!(ch.blood_type, Some("A".to_string()));
        assert!(ch.description.unwrap().contains("~!He discovers time travel!~"));
        assert_eq!(ch.aliases, vec!["Hououin Kyouma", "Okarin"]);
        assert!(ch.image_url.unwrap().contains("anilist"));
    }

    #[test]
    fn test_realistic_anilist_character_minimal_data() {
        // Some AniList characters have very sparse data
        let client = make_client();
        let edge = serde_json::json!({
            "role": "BACKGROUND",
            "node": {
                "id": 99999,
                "name": {
                    "full": "Student A",
                    "native": null,
                    "alternative": []
                },
                "image": {"large": null},
                "description": null,
                "gender": null,
                "age": null,
                "dateOfBirth": {"month": null, "day": null},
                "bloodType": null
            }
        });
        let ch = client.process_character(&edge).unwrap();
        assert_eq!(ch.id, "99999");
        assert_eq!(ch.name, "Student A");
        assert_eq!(ch.name_original, "");
        assert_eq!(ch.role, "side");
        assert!(ch.sex.is_none());
        assert!(ch.age.is_none());
        assert!(ch.birthday.is_none());
        assert!(ch.blood_type.is_none());
        assert!(ch.description.is_none());
        assert!(ch.aliases.is_empty());
        assert!(ch.image_url.is_none());
    }

    // ── Title preference in user list entries ──

    #[test]
    fn test_title_preference_english_only() {
        let title_data = serde_json::json!({"native": null, "romaji": "", "english": "Attack on Titan"});
        let native = title_data["native"].as_str().unwrap_or("");
        let romaji = title_data["romaji"].as_str().unwrap_or("");
        let english = title_data["english"].as_str().unwrap_or("");
        let title = if !native.is_empty() { native.to_string() }
            else if !romaji.is_empty() { romaji.to_string() }
            else { english.to_string() };
        assert_eq!(title, "Attack on Titan");
    }
}
