use regex::Regex;
use serde_json::json;

use crate::models::{Character, CharacterTrait};

/// Role badge colors
const ROLE_COLORS: &[(&str, &str)] = &[
    ("main", "#4CAF50"),     // green
    ("primary", "#2196F3"),  // blue
    ("side", "#FF9800"),     // orange
    ("appears", "#9E9E9E"),  // gray
];

/// Role display labels
const ROLE_LABELS: &[(&str, &str)] = &[
    ("main", "Protagonist"),
    ("primary", "Main Character"),
    ("side", "Side Character"),
    ("appears", "Minor Role"),
];

/// Month names for birthday formatting
const MONTH_NAMES: &[(u32, &str)] = &[
    (1, "January"),
    (2, "February"),
    (3, "March"),
    (4, "April"),
    (5, "May"),
    (6, "June"),
    (7, "July"),
    (8, "August"),
    (9, "September"),
    (10, "October"),
    (11, "November"),
    (12, "December"),
];

/// Sex display mapping — must handle both "m"/"f" and "male"/"female" inputs
const SEX_DISPLAY: &[(&str, &str)] = &[
    ("m", "♂ Male"),
    ("f", "♀ Female"),
    ("male", "♂ Male"),
    ("female", "♀ Female"),
];

pub struct ContentBuilder {
    spoiler_level: u8,
}

impl ContentBuilder {
    pub fn new(spoiler_level: u8) -> Self {
        Self { spoiler_level }
    }

    /// Remove spoiler content from text. Both VNDB and AniList formats.
    pub fn strip_spoilers(text: &str) -> String {
        // VNDB: [spoiler]...[/spoiler]
        let re_vndb = Regex::new(r"(?is)\[spoiler\].*?\[/spoiler\]").unwrap();
        let text = re_vndb.replace_all(text, "");
        // AniList: ~!...!~
        let re_anilist = Regex::new(r"(?s)~!.*?!~").unwrap();
        re_anilist.replace_all(&text, "").trim().to_string()
    }

    /// Check if text contains spoiler tags (either format).
    pub fn has_spoiler_tags(text: &str) -> bool {
        let re_vndb = Regex::new(r"(?i)\[spoiler\]").unwrap();
        let re_anilist = Regex::new(r"(?s)~!.*?!~").unwrap();
        re_vndb.is_match(text) || re_anilist.is_match(text)
    }

    /// Parse VNDB markup: [url=https://...]text[/url] → just the text
    pub fn parse_vndb_markup(text: &str) -> String {
        let re = Regex::new(r"(?i)\[url=[^\]]+\]([^\[]*)\[/url\]").unwrap();
        re.replace_all(text, "$1").to_string()
    }

    /// Parse BBCode [b] and [i] tags into Yomitan structured content nodes.
    /// Returns a serde_json::Value that is either a plain string (no tags found)
    /// or an array of mixed strings and {"tag":"b"/"i","content":...} objects.
    pub fn parse_bbcode_to_structured(text: &str) -> serde_json::Value {
        let re = Regex::new(r"(?is)\[(b|i)\](.*?)\[/\1\]").unwrap();
        if !re.is_match(text) {
            return json!(text);
        }

        let mut result: Vec<serde_json::Value> = Vec::new();
        let mut last_end = 0;

        for cap in re.captures_iter(text) {
            let full_match = cap.get(0).unwrap();
            // Push any text before this match
            if full_match.start() > last_end {
                let before = &text[last_end..full_match.start()];
                if !before.is_empty() {
                    result.push(json!(before));
                }
            }
            let tag = cap[1].to_lowercase();
            // Recursively parse inner content for nested tags
            let inner = Self::parse_bbcode_to_structured(&cap[2]);
            result.push(json!({ "tag": tag, "content": inner }));
            last_end = full_match.end();
        }

        // Push any remaining text after the last match
        if last_end < text.len() {
            let after = &text[last_end..];
            if !after.is_empty() {
                result.push(json!(after));
            }
        }

        if result.len() == 1 {
            result.into_iter().next().unwrap()
        } else {
            json!(result)
        }
    }

    /// Format birthday [month, day] → "September 1"
    pub fn format_birthday(birthday: &[u32]) -> String {
        if birthday.len() < 2 {
            return String::new();
        }
        let month = birthday[0];
        let day = birthday[1];
        let month_name = MONTH_NAMES
            .iter()
            .find(|(m, _)| *m == month)
            .map(|(_, name)| *name)
            .unwrap_or("Unknown");
        format!("{} {}", month_name, day)
    }

    /// Build physical stats line.
    pub fn format_stats(&self, char: &Character) -> String {
        let mut parts = Vec::new();

        if let Some(ref sex) = char.sex {
            let sex_lower = sex.to_lowercase();
            if let Some((_, display)) =
                SEX_DISPLAY.iter().find(|(k, _)| *k == sex_lower.as_str())
            {
                parts.push(display.to_string());
            }
        }

        if let Some(ref age) = char.age {
            parts.push(format!("{} years", age));
        }

        if let Some(height) = char.height {
            parts.push(format!("{}cm", height));
        }

        if let Some(weight) = char.weight {
            parts.push(format!("{}kg", weight));
        }

        if let Some(ref blood_type) = char.blood_type {
            parts.push(format!("Blood Type {}", blood_type));
        }

        if let Some(ref birthday) = char.birthday {
            let formatted = Self::format_birthday(birthday);
            if !formatted.is_empty() {
                parts.push(format!("Birthday: {}", formatted));
            }
        }

        parts.join(" • ")
    }

    /// Build trait items grouped by category, filtered by spoiler_level.
    /// Returns a Vec of Yomitan `li` content items.
    pub fn build_traits_by_category(&self, char: &Character) -> Vec<serde_json::Value> {
        let mut items = Vec::new();

        let categories: &[(&[CharacterTrait], &str)] = &[
            (&char.personality, "Personality"),
            (&char.roles, "Role"),
            (&char.engages_in, "Activities"),
            (&char.subject_of, "Subject of"),
        ];

        for (traits, label) in categories {
            if traits.is_empty() {
                continue;
            }

            // Filter traits by spoiler level
            let filtered: Vec<&str> = traits
                .iter()
                .filter(|t| t.spoiler <= self.spoiler_level && !t.name.is_empty())
                .map(|t| t.name.as_str())
                .collect();

            if !filtered.is_empty() {
                items.push(json!({
                    "tag": "li",
                    "content": format!("{}: {}", label, filtered.join(", "))
                }));
            }
        }

        items
    }

    /// Build the complete Yomitan structured content for a character card.
    pub fn build_content(
        &self,
        char: &Character,
        image_path: Option<&str>,
        game_title: &str,
    ) -> serde_json::Value {
        let mut content: Vec<serde_json::Value> = Vec::new();

        // ===== LEVEL 0: Always shown =====

        // Japanese name (large, bold)
        if !char.name_original.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontWeight": "bold", "fontSize": "1.2em" },
                "content": &char.name_original
            }));
        }

        // Romanized name (italic, gray)
        if !char.name.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontStyle": "italic", "color": "#666", "marginBottom": "8px" },
                "content": &char.name
            }));
        }

        // Character portrait image
        if let Some(path) = image_path {
            content.push(json!({
                "tag": "img",
                "path": path,
                "width": 80,
                "height": 100,
                "sizeUnits": "px",
                "collapsible": false,
                "collapsed": false,
                "background": false
            }));
        }

        // Game/media title
        if !game_title.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontSize": "0.9em", "color": "#888", "marginTop": "4px" },
                "content": format!("From: {}", game_title)
            }));
        }

        // Role badge with color
        let role = char.role.as_str();
        let role_color = ROLE_COLORS
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, c)| *c)
            .unwrap_or("#9E9E9E");
        let role_label = ROLE_LABELS
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, l)| *l)
            .unwrap_or("Unknown");

        content.push(json!({
            "tag": "span",
            "style": {
                "background": role_color,
                "color": "white",
                "padding": "2px 6px",
                "borderRadius": "3px",
                "fontSize": "0.85em",
                "marginTop": "4px"
            },
            "content": role_label
        }));

        // ===== LEVEL 1+: Description and Character Information =====

        if self.spoiler_level >= 1 {
            // Description section (collapsible <details>)
            if let Some(ref desc) = char.description {
                if !desc.trim().is_empty() {
                    let display_desc = if self.spoiler_level == 1 {
                        Self::strip_spoilers(desc)
                    } else {
                        desc.clone() // Level 2: full description
                    };

                    if !display_desc.is_empty() {
                        let parsed = Self::parse_vndb_markup(&display_desc);
                        let structured = Self::parse_bbcode_to_structured(&parsed);
                        content.push(json!({
                            "tag": "details",
                            "content": [
                                { "tag": "summary", "content": "Description" },
                                {
                                    "tag": "div",
                                    "style": { "fontSize": "0.9em", "marginTop": "4px" },
                                    "content": structured
                                }
                            ]
                        }));
                    }
                }
            }

            // Character Information section (collapsible <details>)
            let mut info_items: Vec<serde_json::Value> = Vec::new();

            // Physical stats as compact line
            let stats = self.format_stats(char);
            if !stats.is_empty() {
                info_items.push(json!({
                    "tag": "li",
                    "style": { "fontWeight": "bold" },
                    "content": stats
                }));
            }

            // Traits organized by category (filtered by spoiler level)
            let trait_items = self.build_traits_by_category(char);
            info_items.extend(trait_items);

            if !info_items.is_empty() {
                content.push(json!({
                    "tag": "details",
                    "content": [
                        { "tag": "summary", "content": "Character Information" },
                        {
                            "tag": "ul",
                            "style": { "marginTop": "4px", "paddingLeft": "20px" },
                            "content": info_items
                        }
                    ]
                }));
            }
        }

        json!({
            "type": "structured-content",
            "content": content
        })
    }

    /// Build a structured content card with an honorific description banner.
    /// Clones the base content and prepends a styled honorific note.
    pub fn build_honorific_content(
        base_content: &serde_json::Value,
        suffix_display: &str,
        suffix_description: &str,
    ) -> serde_json::Value {
        let mut content_array = match base_content.get("content") {
            Some(serde_json::Value::Array(arr)) => arr.clone(),
            _ => return base_content.clone(),
        };

        // Honorific banner: styled div at the top of the card
        // Note: Yomitan's structuredContentStyle only allows borderColor/borderStyle/borderWidth
        // (applied to all sides), not borderLeft. We use a thin border + background tint instead.
        let banner = json!({
            "tag": "div",
            "style": {
                "fontSize": "0.85em",
                "color": "#4A90D9",
                "borderColor": "#4A90D9",
                "borderStyle": "solid",
                "borderWidth": "0 0 0 3px",
                "paddingLeft": "6px",
                "marginBottom": "6px"
            },
            "content": [
                {
                    "tag": "span",
                    "style": { "fontWeight": "bold" },
                    "content": suffix_display
                },
                {
                    "tag": "span",
                    "content": format!(" — {}", suffix_description)
                }
            ]
        });

        content_array.insert(0, banner);

        json!({
            "type": "structured-content",
            "content": content_array
        })
    }

    /// Create a single Yomitan term entry.
    pub fn create_term_entry(
        term: &str,
        reading: &str,
        role: &str,
        score: i32,
        structured_content: &serde_json::Value,
    ) -> serde_json::Value {
        json!([
            term,
            reading,
            if role.is_empty() { "name".to_string() } else { format!("name {}", role) },
            "",
            score,
            [structured_content],
            0,
            ""
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Character, CharacterTrait};

    fn make_test_character() -> Character {
        Character {
            id: "c123".to_string(),
            name: "Shinichi Suzuki".to_string(),
            name_original: "須々木 心一".to_string(),
            role: "main".to_string(),
            sex: Some("m".to_string()),
            age: Some("17".to_string()),
            height: Some(165),
            weight: Some(50),
            blood_type: Some("A".to_string()),
            birthday: Some(vec![9, 1]),
            description: Some("The protagonist.\n[spoiler]Secret info[/spoiler]".to_string()),
            aliases: vec!["しんいち".to_string()],
            personality: vec![
                CharacterTrait { name: "Kind".to_string(), spoiler: 0 },
                CharacterTrait { name: "Secret trait".to_string(), spoiler: 2 },
            ],
            roles: vec![CharacterTrait { name: "Student".to_string(), spoiler: 0 }],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_base64: None,
        }
    }

    // === Spoiler stripping tests ===

    #[test]
    fn test_strip_spoilers_vndb() {
        let result = ContentBuilder::strip_spoilers("before [spoiler]hidden[/spoiler] after");
        assert_eq!(result, "before  after");
    }

    #[test]
    fn test_strip_spoilers_anilist() {
        let result = ContentBuilder::strip_spoilers("before ~!hidden!~ after");
        assert_eq!(result, "before  after");
    }

    #[test]
    fn test_strip_spoilers_both_formats() {
        let result = ContentBuilder::strip_spoilers("a [spoiler]x[/spoiler] b ~!y!~ c");
        assert_eq!(result, "a  b  c");
    }

    #[test]
    fn test_strip_spoilers_no_spoilers() {
        let result = ContentBuilder::strip_spoilers("clean text");
        assert_eq!(result, "clean text");
    }

    // === Spoiler detection tests ===

    #[test]
    fn test_has_spoiler_tags_vndb() {
        assert!(ContentBuilder::has_spoiler_tags("x [spoiler]y[/spoiler]"));
    }

    #[test]
    fn test_has_spoiler_tags_anilist() {
        assert!(ContentBuilder::has_spoiler_tags("x ~!y!~"));
    }

    #[test]
    fn test_has_spoiler_tags_none() {
        assert!(!ContentBuilder::has_spoiler_tags("plain text"));
    }

    // === VNDB markup tests ===

    #[test]
    fn test_parse_vndb_markup_url() {
        let result = ContentBuilder::parse_vndb_markup(
            "see [url=https://example.com]this link[/url] here",
        );
        assert_eq!(result, "see this link here");
    }

    #[test]
    fn test_parse_vndb_markup_no_markup() {
        let result = ContentBuilder::parse_vndb_markup("plain text");
        assert_eq!(result, "plain text");
    }

    // === BBCode to structured content tests ===

    #[test]
    fn test_parse_bbcode_bold() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b]bold text[/b]");
        assert_eq!(result, json!({"tag": "b", "content": "bold text"}));
    }

    #[test]
    fn test_parse_bbcode_italic() {
        let result = ContentBuilder::parse_bbcode_to_structured("[i]italic text[/i]");
        assert_eq!(result, json!({"tag": "i", "content": "italic text"}));
    }

    #[test]
    fn test_parse_bbcode_nested() {
        let result = ContentBuilder::parse_bbcode_to_structured(
            "[b]SNS Manager of [i]Lemonade Factory[/i][/b]",
        );
        assert_eq!(
            result,
            json!({"tag": "b", "content": ["SNS Manager of ", {"tag": "i", "content": "Lemonade Factory"}]})
        );
    }

    #[test]
    fn test_parse_bbcode_mixed_with_plain() {
        let result = ContentBuilder::parse_bbcode_to_structured(
            "[b]SNS Manager[/b]  The protagonist's friend.",
        );
        assert_eq!(
            result,
            json!([{"tag": "b", "content": "SNS Manager"}, "  The protagonist's friend."])
        );
    }

    #[test]
    fn test_parse_bbcode_no_tags() {
        let result = ContentBuilder::parse_bbcode_to_structured("plain text");
        assert_eq!(result, json!("plain text"));
    }

    #[test]
    fn test_parse_bbcode_preserves_brackets_not_bbcode() {
        let result = ContentBuilder::parse_bbcode_to_structured(
            "[Translated from official website]",
        );
        assert_eq!(result, json!("[Translated from official website]"));
    }

    // === Birthday formatting tests ===

    #[test]
    fn test_format_birthday() {
        assert_eq!(ContentBuilder::format_birthday(&[9, 1]), "September 1");
        assert_eq!(ContentBuilder::format_birthday(&[1, 15]), "January 15");
        assert_eq!(ContentBuilder::format_birthday(&[12, 25]), "December 25");
    }

    #[test]
    fn test_format_birthday_short_array() {
        assert_eq!(ContentBuilder::format_birthday(&[9]), "");
        assert_eq!(ContentBuilder::format_birthday(&[]), "");
    }

    // === Stats formatting tests ===

    #[test]
    fn test_format_stats_full() {
        let cb = ContentBuilder::new(2);
        let char = make_test_character();
        let stats = cb.format_stats(&char);
        assert!(stats.contains("Male"));
        assert!(stats.contains("17 years"));
        assert!(stats.contains("165cm"));
        assert!(stats.contains("50kg"));
        assert!(stats.contains("Blood Type A"));
        assert!(stats.contains("September 1"));
    }

    #[test]
    fn test_format_stats_partial() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert!(stats.contains("Male"));
        assert!(stats.contains("17 years"));
        assert!(!stats.contains("cm"));
        assert!(!stats.contains("kg"));
    }

    #[test]
    fn test_format_stats_empty() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.sex = None;
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert_eq!(stats, "");
    }

    // === Trait filtering tests ===

    #[test]
    fn test_traits_spoiler_level_0() {
        let cb = ContentBuilder::new(0);
        let char = make_test_character();
        let items = cb.build_traits_by_category(&char);
        // At level 0, only traits with spoiler=0 pass
        // But level 0 means the content section isn't shown anyway
        // The function itself should still filter correctly
        for item in &items {
            let content = item["content"].as_str().unwrap();
            assert!(!content.contains("Secret trait"));
        }
    }

    #[test]
    fn test_traits_spoiler_level_1() {
        let cb = ContentBuilder::new(1);
        let char = make_test_character();
        let items = cb.build_traits_by_category(&char);
        // spoiler=0 traits included, spoiler=2 excluded
        let all_text: String = items
            .iter()
            .filter_map(|i| i["content"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_text.contains("Kind"));
        assert!(all_text.contains("Student"));
        assert!(!all_text.contains("Secret trait"));
    }

    #[test]
    fn test_traits_spoiler_level_2() {
        let cb = ContentBuilder::new(2);
        let char = make_test_character();
        let items = cb.build_traits_by_category(&char);
        let all_text: String = items
            .iter()
            .filter_map(|i| i["content"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_text.contains("Kind"));
        assert!(all_text.contains("Secret trait"));
    }

    // === Structured content tests ===

    #[test]
    fn test_build_content_level_0() {
        let cb = ContentBuilder::new(0);
        let char = make_test_character();
        let content = cb.build_content(&char, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        // Level 0: should NOT contain <details> tags
        let has_details = items.iter().any(|v| v["tag"].as_str() == Some("details"));
        assert!(!has_details, "Level 0 should not contain details sections");
        // Should contain name and role
        let has_span = items.iter().any(|v| v["tag"].as_str() == Some("span"));
        assert!(has_span, "Should contain role badge span");
    }

    #[test]
    fn test_build_content_level_1() {
        let cb = ContentBuilder::new(1);
        let char = make_test_character();
        let content = cb.build_content(&char, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        // Level 1: should contain <details> tags (Description + Character Information)
        let details_count = items
            .iter()
            .filter(|v| v["tag"].as_str() == Some("details"))
            .count();
        assert!(details_count >= 1, "Level 1 should have details sections");
    }

    #[test]
    fn test_build_content_level_2() {
        let cb = ContentBuilder::new(2);
        let char = make_test_character();
        let content = cb.build_content(&char, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        let details_count = items
            .iter()
            .filter(|v| v["tag"].as_str() == Some("details"))
            .count();
        assert!(details_count >= 1, "Level 2 should have details sections");
    }

    #[test]
    fn test_build_content_with_image() {
        let cb = ContentBuilder::new(0);
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c123.jpg"), "Test Game");
        let items = content["content"].as_array().unwrap();
        let has_img = items.iter().any(|v| v["tag"].as_str() == Some("img"));
        assert!(has_img, "Should contain image tag");
    }

    // === Term entry format tests ===

    #[test]
    fn test_create_term_entry_format() {
        let sc = json!({"type": "structured-content", "content": []});
        let entry = ContentBuilder::create_term_entry("須々木", "すずき", "main", 100, &sc);
        let arr = entry.as_array().unwrap();
        assert_eq!(arr.len(), 8);
        assert_eq!(arr[0], "須々木");           // term
        assert_eq!(arr[1], "すずき");           // reading
        assert_eq!(arr[2], "name main");         // tags
        assert_eq!(arr[3], "");                  // rules
        assert_eq!(arr[4], 100);                 // score
        assert!(arr[5].is_array());              // definitions array
        assert_eq!(arr[6], 0);                   // sequence
        assert_eq!(arr[7], "");                  // termTags
    }

    #[test]
    fn test_create_term_entry_empty_role() {
        let sc = json!({"type": "structured-content"});
        let entry = ContentBuilder::create_term_entry("test", "test", "", 50, &sc);
        let arr = entry.as_array().unwrap();
        assert_eq!(arr[2], "name");
    }

    // === Honorific content tests ===

    #[test]
    fn test_build_honorific_content_prepends_banner() {
        let base = json!({
            "type": "structured-content",
            "content": [
                { "tag": "div", "content": "original" }
            ]
        });
        let result = ContentBuilder::build_honorific_content(&base, "さん", "Generic polite suffix (Mr./Ms./Mrs.)");
        let items = result["content"].as_array().unwrap();
        // Banner should be first element
        assert_eq!(items[0]["tag"], "div");
        let banner_content = items[0]["content"].as_array().unwrap();
        assert_eq!(banner_content[0]["content"], "さん");
        assert!(banner_content[1]["content"].as_str().unwrap().contains("Generic polite"));
        // Original content should follow
        assert_eq!(items[1]["content"], "original");
    }

    #[test]
    fn test_build_honorific_content_preserves_base() {
        let base = json!({
            "type": "structured-content",
            "content": [
                { "tag": "div", "content": "first" },
                { "tag": "div", "content": "second" }
            ]
        });
        let result = ContentBuilder::build_honorific_content(&base, "様", "Very formal/respectful");
        let items = result["content"].as_array().unwrap();
        // Banner + 2 original items = 3 total
        assert_eq!(items.len(), 3);
        assert_eq!(items[1]["content"], "first");
        assert_eq!(items[2]["content"], "second");
    }

    #[test]
    fn test_build_honorific_content_no_content_array() {
        // If base content doesn't have a content array, return base unchanged
        let base = json!({"type": "structured-content"});
        let result = ContentBuilder::build_honorific_content(&base, "さん", "test");
        assert_eq!(result, base);
    }
}

