use serde::{Deserialize, Serialize};

/// An entry from a user's media list (VNDB or AniList).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMediaEntry {
    pub id: String,           // "v17" for VNDB, "9253" for AniList
    pub title: String,        // Display title (prefer Japanese/native)
    pub title_romaji: String, // Romanized title
    pub source: String,       // "vndb" or "anilist"
    pub media_type: String,   // "vn", "anime", "manga"
}

/// A trait with spoiler metadata.
/// Represents entries like: {"name": "Kind", "spoiler": 0}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterTrait {
    pub name: String,
    pub spoiler: u8, // 0=none, 1=minor, 2=major
}

/// Normalized character data. Both VNDB and AniList clients produce this format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub name: String,          // Romanized (Western order for VNDB: "Given Family")
    pub name_original: String, // Japanese (Japanese order: "Family Given")
    pub role: String,          // "main", "primary", "side", "appears"
    pub sex: Option<String>,   // "m" or "f"
    pub age: Option<String>,   // String because AniList may return "17-18"
    pub height: Option<u32>,   // cm (VNDB only; None for AniList)
    pub weight: Option<u32>,   // kg (VNDB only; None for AniList)
    pub blood_type: Option<String>,
    pub birthday: Option<Vec<u32>>, // [month, day]
    pub description: Option<String>,
    pub aliases: Vec<String>,
    pub personality: Vec<CharacterTrait>,
    pub roles: Vec<CharacterTrait>,
    pub engages_in: Vec<CharacterTrait>,
    pub subject_of: Vec<CharacterTrait>,
    pub image_url: Option<String>, // Raw URL from API (used for downloading)
    pub image_bytes: Option<Vec<u8>>, // Raw image bytes (after download + resize)
    pub image_ext: Option<String>, // File extension: "jpg", "png", "webp", etc.
    pub image_width: Option<u32>,  // Actual pixel width after resize
    pub image_height: Option<u32>, // Actual pixel height after resize
    pub first_name_hint: Option<String>, // Given name romaji hint (AniList "first")
    pub last_name_hint: Option<String>, // Family name romaji hint (AniList "last")
}

/// Categorized characters for a single game/media.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterData {
    pub main: Vec<Character>,
    pub primary: Vec<Character>,
    pub side: Vec<Character>,
    pub appears: Vec<Character>,
}

impl CharacterData {
    pub fn new() -> Self {
        Self {
            main: Vec::new(),
            primary: Vec::new(),
            side: Vec::new(),
            appears: Vec::new(),
        }
    }

    /// Iterate over all characters across all role categories.
    pub fn all_characters(&self) -> impl Iterator<Item = &Character> {
        self.main
            .iter()
            .chain(self.primary.iter())
            .chain(self.side.iter())
            .chain(self.appears.iter())
    }

    /// Mutable iterator (used for populating image_bytes after download).
    pub fn all_characters_mut(&mut self) -> impl Iterator<Item = &mut Character> {
        self.main
            .iter_mut()
            .chain(self.primary.iter_mut())
            .chain(self.side.iter_mut())
            .chain(self.appears.iter_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_media_entry_serialization() {
        let entry = UserMediaEntry {
            id: "v17".to_string(),
            title: "Steins;Gate".to_string(),
            title_romaji: "Steins;Gate".to_string(),
            source: "vndb".to_string(),
            media_type: "vn".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("v17"));
        assert!(json.contains("Steins;Gate"));
        assert!(json.contains("vndb"));

        // Test deserialization roundtrip
        let deserialized: UserMediaEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "v17");
        assert_eq!(deserialized.source, "vndb");
    }

    #[test]
    fn test_character_data_new_empty() {
        let cd = CharacterData::new();
        assert!(cd.main.is_empty());
        assert!(cd.primary.is_empty());
        assert!(cd.side.is_empty());
        assert!(cd.appears.is_empty());
        assert_eq!(cd.all_characters().count(), 0);
    }

    #[test]
    fn test_character_data_all_characters() {
        let mut cd = CharacterData::new();
        cd.main.push(Character {
            id: "c1".to_string(),
            name: "A".to_string(),
            name_original: "A".to_string(),
            role: "main".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
        });
        cd.side.push(Character {
            id: "c2".to_string(),
            name: "B".to_string(),
            name_original: "B".to_string(),
            role: "side".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
        });

        assert_eq!(cd.all_characters().count(), 2);
        let ids: Vec<&str> = cd.all_characters().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"c1"));
        assert!(ids.contains(&"c2"));
    }

    #[test]
    fn test_character_data_all_characters_mut() {
        let mut cd = CharacterData::new();
        cd.main.push(Character {
            id: "c1".to_string(),
            name: "A".to_string(),
            name_original: "A".to_string(),
            role: "main".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
        });

        for c in cd.all_characters_mut() {
            c.image_bytes = Some(vec![1, 2, 3]);
            c.image_ext = Some("jpg".to_string());
        }

        assert_eq!(cd.main[0].image_bytes.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(cd.main[0].image_ext.as_deref(), Some("jpg"));
    }
}
