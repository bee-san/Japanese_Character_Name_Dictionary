use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Client;

use crate::models::*;

pub struct VndbClient {
    client: Client,
}

impl VndbClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Resolve a VNDB username to a user ID (e.g. "yorhel" → "u2").
    /// Uses GET /user?q=USERNAME endpoint. Case-insensitive.
    pub async fn resolve_user(&self, username: &str) -> Result<String, String> {
        let response = self
            .client
            .get("https://api.vndb.org/kana/user")
            .query(&[("q", username)])
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if response.status() != 200 {
            return Err(format!("VNDB user API returned status {}", response.status()));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        // The response has the query as key, value is null or {id, username}
        let user_data = data
            .get(username)
            .or_else(|| {
                // Try case-insensitive: the API returns with the original casing of the query
                data.as_object().and_then(|obj| {
                    obj.values().next()
                })
            });

        match user_data {
            Some(val) if !val.is_null() => {
                val["id"]
                    .as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| "User ID not found in response".to_string())
            }
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

            let response = self
                .client
                .post("https://api.vndb.org/kana/ulist")
                .json(&payload)
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            if response.status() != 200 {
                return Err(format!("VNDB ulist API returned status {}", response.status()));
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

                let title_romaji = item["vn"]["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let title_japanese = item["vn"]["alttitle"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

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

    /// Fetch the VN's title. Returns (romaji_title, original_japanese_title).
    pub async fn fetch_vn_title(&self, vn_id: &str) -> Result<(String, String), String> {
        let vn_id = Self::normalize_id(vn_id);
        let payload = serde_json::json!({
            "filters": ["id", "=", &vn_id],
            "fields": "title, alttitle"
        });

        let response = self
            .client
            .post("https://api.vndb.org/kana/vn")
            .json(&payload)
            .send()
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
        let title = vn["title"].as_str().unwrap_or("").to_string(); // Romanized
        let alttitle = vn["alttitle"].as_str().unwrap_or("").to_string(); // Japanese original
        Ok((title, alttitle))
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

            let response = self
                .client
                .post("https://api.vndb.org/kana/character")
                .json(&payload)
                .send()
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
            image_base64: None, // Populated later in a separate pass
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
}
