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
    #[serde(default)]
    pub seiyuu: Option<String>,        // Voice actor name (e.g. "花澤香菜")
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
            last_name_hint: None, seiyuu: None,
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
            last_name_hint: None, seiyuu: None,
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
            last_name_hint: None, seiyuu: None,
        });

        for c in cd.all_characters_mut() {
            c.image_bytes = Some(vec![1, 2, 3]);
            c.image_ext = Some("jpg".to_string());
        }

        assert_eq!(cd.main[0].image_bytes.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(cd.main[0].image_ext.as_deref(), Some("jpg"));
    }

    // ===== Additional comprehensive tests =====

    #[test]
    fn test_character_serialization_roundtrip() {
        let char = Character {
            id: "c42".to_string(),
            name: "Okabe Rintarou".to_string(),
            name_original: "岡部 倫太郎".to_string(),
            role: "main".to_string(),
            sex: Some("m".to_string()),
            age: Some("18".to_string()),
            height: Some(177),
            weight: Some(59),
            blood_type: Some("A".to_string()),
            birthday: Some(vec![12, 14]),
            description: Some("Mad scientist".to_string()),
            aliases: vec!["Hououin Kyouma".to_string(), "Okarin".to_string()],
            personality: vec![CharacterTrait { name: "Eccentric".to_string(), spoiler: 0 }],
            roles: vec![CharacterTrait { name: "Protagonist".to_string(), spoiler: 0 }],
            engages_in: vec![],
            subject_of: vec![],
            image_url: Some("https://example.com/img.jpg".to_string()),
            image_bytes: None,
            image_ext: Some("jpg".to_string()),
            image_width: Some(100),
            image_height: Some(150),
            first_name_hint: Some("Rintarou".to_string()),
            last_name_hint: Some("Okabe".to_string()),
            seiyuu: None,
        };

        let json = serde_json::to_string(&char).unwrap();
        let deserialized: Character = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "c42");
        assert_eq!(deserialized.name, "Okabe Rintarou");
        assert_eq!(deserialized.name_original, "岡部 倫太郎");
        assert_eq!(deserialized.role, "main");
        assert_eq!(deserialized.sex, Some("m".to_string()));
        assert_eq!(deserialized.age, Some("18".to_string()));
        assert_eq!(deserialized.height, Some(177));
        assert_eq!(deserialized.weight, Some(59));
        assert_eq!(deserialized.blood_type, Some("A".to_string()));
        assert_eq!(deserialized.birthday, Some(vec![12, 14]));
        assert_eq!(deserialized.description, Some("Mad scientist".to_string()));
        assert_eq!(deserialized.aliases.len(), 2);
        assert_eq!(deserialized.personality.len(), 1);
        assert_eq!(deserialized.personality[0].name, "Eccentric");
        assert_eq!(deserialized.personality[0].spoiler, 0);
        assert_eq!(deserialized.first_name_hint, Some("Rintarou".to_string()));
        assert_eq!(deserialized.last_name_hint, Some("Okabe".to_string()));
    }

    #[test]
    fn test_character_with_all_none_fields() {
        let char = Character {
            id: "c1".to_string(),
            name: "".to_string(),
            name_original: "".to_string(),
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
            last_name_hint: None, seiyuu: None,
        };

        let json = serde_json::to_string(&char).unwrap();
        let deserialized: Character = serde_json::from_str(&json).unwrap();
        assert!(deserialized.sex.is_none());
        assert!(deserialized.age.is_none());
        assert!(deserialized.height.is_none());
        assert!(deserialized.birthday.is_none());
        assert!(deserialized.description.is_none());
        assert!(deserialized.image_url.is_none());
        assert!(deserialized.first_name_hint.is_none());
        assert!(deserialized.last_name_hint.is_none());
    }

    #[test]
    fn test_character_trait_serialization() {
        let trait_ = CharacterTrait {
            name: "Kind".to_string(),
            spoiler: 1,
        };
        let json = serde_json::to_string(&trait_).unwrap();
        let deserialized: CharacterTrait = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Kind");
        assert_eq!(deserialized.spoiler, 1);
    }

    #[test]
    fn test_character_data_ordering() {
        let mut cd = CharacterData::new();
        cd.main.push(Character {
            id: "c1".to_string(), name: "Main".to_string(), name_original: "".to_string(),
            role: "main".to_string(), sex: None, age: None, height: None, weight: None,
            blood_type: None, birthday: None, description: None, aliases: vec![],
            personality: vec![], roles: vec![], engages_in: vec![], subject_of: vec![],
            image_url: None, image_bytes: None, image_ext: None, image_width: None,
            image_height: None, first_name_hint: None, last_name_hint: None, seiyuu: None,
        });
        cd.primary.push(Character {
            id: "c2".to_string(), name: "Primary".to_string(), name_original: "".to_string(),
            role: "primary".to_string(), sex: None, age: None, height: None, weight: None,
            blood_type: None, birthday: None, description: None, aliases: vec![],
            personality: vec![], roles: vec![], engages_in: vec![], subject_of: vec![],
            image_url: None, image_bytes: None, image_ext: None, image_width: None,
            image_height: None, first_name_hint: None, last_name_hint: None, seiyuu: None,
        });
        cd.side.push(Character {
            id: "c3".to_string(), name: "Side".to_string(), name_original: "".to_string(),
            role: "side".to_string(), sex: None, age: None, height: None, weight: None,
            blood_type: None, birthday: None, description: None, aliases: vec![],
            personality: vec![], roles: vec![], engages_in: vec![], subject_of: vec![],
            image_url: None, image_bytes: None, image_ext: None, image_width: None,
            image_height: None, first_name_hint: None, last_name_hint: None, seiyuu: None,
        });
        cd.appears.push(Character {
            id: "c4".to_string(), name: "Appears".to_string(), name_original: "".to_string(),
            role: "appears".to_string(), sex: None, age: None, height: None, weight: None,
            blood_type: None, birthday: None, description: None, aliases: vec![],
            personality: vec![], roles: vec![], engages_in: vec![], subject_of: vec![],
            image_url: None, image_bytes: None, image_ext: None, image_width: None,
            image_height: None, first_name_hint: None, last_name_hint: None, seiyuu: None,
        });

        // all_characters should iterate in order: main, primary, side, appears
        let ids: Vec<&str> = cd.all_characters().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["c1", "c2", "c3", "c4"]);
    }

    #[test]
    fn test_character_data_serialization_roundtrip() {
        let mut cd = CharacterData::new();
        cd.main.push(Character {
            id: "c1".to_string(), name: "Test".to_string(), name_original: "テスト".to_string(),
            role: "main".to_string(), sex: Some("f".to_string()), age: None, height: None,
            weight: None, blood_type: None, birthday: None, description: None, aliases: vec![],
            personality: vec![], roles: vec![], engages_in: vec![], subject_of: vec![],
            image_url: None, image_bytes: None, image_ext: None, image_width: None,
            image_height: None, first_name_hint: None, last_name_hint: None, seiyuu: None,
        });

        let json = serde_json::to_string(&cd).unwrap();
        let deserialized: CharacterData = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.main.len(), 1);
        assert_eq!(deserialized.main[0].id, "c1");
        assert_eq!(deserialized.main[0].name_original, "テスト");
        assert!(deserialized.primary.is_empty());
        assert!(deserialized.side.is_empty());
        assert!(deserialized.appears.is_empty());
    }

    #[test]
    fn test_user_media_entry_all_sources() {
        for (source, media_type) in &[("vndb", "vn"), ("anilist", "anime"), ("anilist", "manga")] {
            let entry = UserMediaEntry {
                id: "123".to_string(),
                title: "Test".to_string(),
                title_romaji: "Test".to_string(),
                source: source.to_string(),
                media_type: media_type.to_string(),
            };
            let json = serde_json::to_string(&entry).unwrap();
            let de: UserMediaEntry = serde_json::from_str(&json).unwrap();
            assert_eq!(de.source, *source);
            assert_eq!(de.media_type, *media_type);
        }
    }
}
