use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;

use crate::models::{Character, CharacterTrait};

// === Cached regex patterns (compiled once, reused across all calls) ===

static RE_VNDB_SPOILER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[spoiler\].*?\[/spoiler\]").unwrap());
static RE_ANILIST_SPOILER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)~!.*?!~").unwrap());
static RE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[url=[^\]]+\]([^\[]*)\[/url\]").unwrap());
static RE_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[quote\](.*?)\[/quote\]").unwrap());
static RE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[code\](.*?)\[/code\]").unwrap());
static RE_RAW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[raw\](.*?)\[/raw\]").unwrap());
static RE_UNDERLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[u\](.*?)\[/u\]").unwrap());
static RE_STRIKETHROUGH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[s\](.*?)\[/s\]").unwrap());
static RE_BBCODE_INNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[(b|i)\]([^\[]*?)\[/(b|i)\]").unwrap());
static RE_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x00NODE(\d+)\x00").unwrap());

/// Role badge colors
const ROLE_COLORS: &[(&str, &str)] = &[
    ("main", "#4CAF50"),    // green
    ("primary", "#2196F3"), // blue
    ("side", "#FF9800"),    // orange
    ("appears", "#9E9E9E"), // gray
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
        let text = RE_VNDB_SPOILER.replace_all(text, "");
        // AniList: ~!...!~
        RE_ANILIST_SPOILER.replace_all(&text, "").trim().to_string()
    }

    /// Parse VNDB markup: strip [url=...], [quote], [code], [raw] tags down to inner text.
    pub fn parse_vndb_markup(text: &str) -> String {
        // [url=https://...]text[/url] → text
        let text = RE_URL.replace_all(text, "$1");
        // [quote]...[/quote] → inner text
        let text = RE_QUOTE.replace_all(&text, "$1");
        // [code]...[/code] → inner text
        let text = RE_CODE.replace_all(&text, "$1");
        // [raw]...[/raw] → inner text (raw means "don't format", so we just unwrap)
        let text = RE_RAW.replace_all(&text, "$1");
        // [u]...[/u] → inner text (Yomitan doesn't support textDecoration style)
        let text = RE_UNDERLINE.replace_all(&text, "$1");
        // [s]...[/s] → inner text (Yomitan doesn't support textDecoration style)
        RE_STRIKETHROUGH.replace_all(&text, "$1").to_string()
    }

    /// Parse BBCode [b] and [i] tags into Yomitan structured content nodes.
    /// Returns a serde_json::Value that is either a plain string (no tags found)
    /// or an array of mixed strings and {"tag":"b"/"i","content":...} objects.
    pub fn parse_bbcode_to_structured(text: &str) -> serde_json::Value {
        // Process innermost BBCode tags first, then work outward.
        // Each pass finds tags whose content has no nested `[x]` tags (no `[` inside).
        let mut nodes: Vec<serde_json::Value> = Vec::new();
        let mut working = text.to_string();
        let placeholder_prefix = "\x00NODE";

        while let Some(cap) = RE_BBCODE_INNER.captures(&working) {
                let open_tag = cap[1].to_lowercase();
                let close_tag = cap[3].to_lowercase();

                // Mismatched tags — strip the tags, keep content
                if open_tag != close_tag {
                    let full = cap.get(0).unwrap();
                    working = format!(
                        "{}{}{}",
                        &working[..full.start()],
                        &cap[2],
                        &working[full.end()..]
                    );
                    continue;
                }

                let inner_text = &cap[2];
                // Build the inner content: resolve any placeholders in it
                let inner_content = Self::resolve_placeholders(inner_text, &nodes);

                let node = match open_tag.as_str() {
                    "b" => json!({
                        "tag": "span",
                        "style": { "fontWeight": "bold" },
                        "content": inner_content
                    }),
                    "i" => json!({
                        "tag": "span",
                        "style": { "fontStyle": "italic" },
                        "content": inner_content
                    }),
                    _ => json!({ "tag": "span", "content": inner_content }),
                };

                let idx = nodes.len();
                nodes.push(node);
                let placeholder = format!("{}{}\x00", placeholder_prefix, idx);
                let full = cap.get(0).unwrap();
                working = format!(
                    "{}{}{}",
                    &working[..full.start()],
                    placeholder,
                    &working[full.end()..]
                );
        }

        // Now resolve the final working string with all placeholders
        Self::resolve_placeholders(&working, &nodes)
    }

    /// Resolve placeholder markers back into structured content nodes.
    /// Returns a single Value (string, object, or array of mixed).
    fn resolve_placeholders(text: &str, nodes: &[serde_json::Value]) -> serde_json::Value {
        if !RE_PLACEHOLDER.is_match(text) {
            return json!(text);
        }

        let mut result: Vec<serde_json::Value> = Vec::new();
        let mut last_end = 0;

        for cap in RE_PLACEHOLDER.captures_iter(text) {
            let full = cap.get(0).unwrap();
            if full.start() > last_end {
                let before = &text[last_end..full.start()];
                if !before.is_empty() {
                    result.push(json!(before));
                }
            }
            let idx: usize = cap[1].parse().unwrap();
            if idx < nodes.len() {
                result.push(nodes[idx].clone());
            }
            last_end = full.end();
        }

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
            if let Some((_, display)) = SEX_DISPLAY.iter().find(|(k, _)| *k == sex_lower.as_str()) {
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
            if role.is_empty() {
                "name".to_string()
            } else {
                format!("name {}", role)
            },
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
                CharacterTrait {
                    name: "Kind".to_string(),
                    spoiler: 0,
                },
                CharacterTrait {
                    name: "Secret trait".to_string(),
                    spoiler: 2,
                },
            ],
            roles: vec![CharacterTrait {
                name: "Student".to_string(),
                spoiler: 0,
            }],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
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

    // === VNDB markup tests ===

    #[test]
    fn test_parse_vndb_markup_url() {
        let result =
            ContentBuilder::parse_vndb_markup("see [url=https://example.com]this link[/url] here");
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
        assert_eq!(
            result,
            json!({"tag": "span", "style": {"fontWeight": "bold"}, "content": "bold text"})
        );
    }

    #[test]
    fn test_parse_bbcode_italic() {
        let result = ContentBuilder::parse_bbcode_to_structured("[i]italic text[/i]");
        assert_eq!(
            result,
            json!({"tag": "span", "style": {"fontStyle": "italic"}, "content": "italic text"})
        );
    }

    #[test]
    fn test_parse_bbcode_nested() {
        let result = ContentBuilder::parse_bbcode_to_structured(
            "[b]SNS Manager of [i]Lemonade Factory[/i][/b]",
        );
        assert_eq!(
            result,
            json!({"tag": "span", "style": {"fontWeight": "bold"}, "content": ["SNS Manager of ", {"tag": "span", "style": {"fontStyle": "italic"}, "content": "Lemonade Factory"}]})
        );
    }

    #[test]
    fn test_parse_bbcode_mixed_with_plain() {
        let result = ContentBuilder::parse_bbcode_to_structured(
            "[b]SNS Manager[/b]  The protagonist's friend.",
        );
        assert_eq!(
            result,
            json!([{"tag": "span", "style": {"fontWeight": "bold"}, "content": "SNS Manager"}, "  The protagonist's friend."])
        );
    }

    #[test]
    fn test_parse_bbcode_no_tags() {
        let result = ContentBuilder::parse_bbcode_to_structured("plain text");
        assert_eq!(result, json!("plain text"));
    }

    #[test]
    fn test_parse_bbcode_preserves_brackets_not_bbcode() {
        let result =
            ContentBuilder::parse_bbcode_to_structured("[Translated from official website]");
        assert_eq!(result, json!("[Translated from official website]"));
    }

    // === Underline [u] and strikethrough [s] are stripped in parse_vndb_markup ===

    #[test]
    fn test_parse_vndb_markup_underline() {
        let result = ContentBuilder::parse_vndb_markup("text [u]underlined[/u] here");
        assert_eq!(result, "text underlined here");
    }

    #[test]
    fn test_parse_vndb_markup_strikethrough() {
        let result = ContentBuilder::parse_vndb_markup("text [s]struck[/s] here");
        assert_eq!(result, "text struck here");
    }

    // === VNDB markup stripping: [quote], [code], [raw] ===

    #[test]
    fn test_parse_vndb_markup_quote() {
        let result = ContentBuilder::parse_vndb_markup("before [quote]quoted text[/quote] after");
        assert_eq!(result, "before quoted text after");
    }

    #[test]
    fn test_parse_vndb_markup_code() {
        let result = ContentBuilder::parse_vndb_markup("see [code]some code[/code] here");
        assert_eq!(result, "see some code here");
    }

    #[test]
    fn test_parse_vndb_markup_raw() {
        let result = ContentBuilder::parse_vndb_markup("text [raw][b]not bold[/b][/raw] end");
        assert_eq!(result, "text [b]not bold[/b] end");
    }

    #[test]
    fn test_parse_vndb_markup_multiple_tags() {
        let result = ContentBuilder::parse_vndb_markup(
            "[url=https://example.com]link[/url] and [quote]quoted[/quote]",
        );
        assert_eq!(result, "link and quoted");
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
        assert_eq!(arr[0], "須々木"); // term
        assert_eq!(arr[1], "すずき"); // reading
        assert_eq!(arr[2], "name main"); // tags
        assert_eq!(arr[3], ""); // rules
        assert_eq!(arr[4], 100); // score
        assert!(arr[5].is_array()); // definitions array
        assert_eq!(arr[6], 0); // sequence
        assert_eq!(arr[7], ""); // termTags
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
        let result = ContentBuilder::build_honorific_content(
            &base,
            "さん",
            "Generic polite suffix (Mr./Ms./Mrs.)",
        );
        let items = result["content"].as_array().unwrap();
        // Banner should be first element
        assert_eq!(items[0]["tag"], "div");
        let banner_content = items[0]["content"].as_array().unwrap();
        assert_eq!(banner_content[0]["content"], "さん");
        assert!(banner_content[1]["content"]
            .as_str()
            .unwrap()
            .contains("Generic polite"));
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

    // === Edge case: nested spoiler tags ===

    #[test]
    fn test_strip_spoilers_nested_vndb() {
        // Non-greedy regex matches first [spoiler] to first [/spoiler],
        // leaving the outer closing tag as visible text
        let result = ContentBuilder::strip_spoilers(
            "[spoiler]outer [spoiler]inner[/spoiler] still hidden[/spoiler]",
        );
        // First match: "[spoiler]outer [spoiler]inner[/spoiler]" is removed
        // Remaining: " still hidden[/spoiler]"
        assert!(
            result.contains("still hidden"),
            "Nested spoiler leaves partial text: '{}'",
            result
        );
    }

    #[test]
    fn test_strip_spoilers_multiple_separate() {
        let result =
            ContentBuilder::strip_spoilers("a [spoiler]x[/spoiler] b [spoiler]y[/spoiler] c");
        assert_eq!(result, "a  b  c");
    }

    #[test]
    fn test_strip_spoilers_anilist_multiline() {
        let result = ContentBuilder::strip_spoilers("before ~!line1\nline2!~ after");
        assert_eq!(result, "before  after");
    }

    #[test]
    fn test_strip_spoilers_only_spoiler_content() {
        let result = ContentBuilder::strip_spoilers("[spoiler]everything[/spoiler]");
        assert_eq!(result, "");
    }

    // === Edge case: BBCode mismatched tags ===

    #[test]
    fn test_parse_bbcode_mismatched_tags() {
        // [b]...[/i] — mismatched, should strip tags and keep content
        let result = ContentBuilder::parse_bbcode_to_structured("[b]text[/i]");
        // The regex matches [b]text[/i] as a capture, detects mismatch,
        // strips the tags and keeps "text"
        assert_eq!(result, json!("text"));
    }

    #[test]
    fn test_parse_bbcode_empty_tags() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b][/b]");
        assert_eq!(
            result,
            json!({"tag": "span", "style": {"fontWeight": "bold"}, "content": ""})
        );
    }

    #[test]
    fn test_parse_bbcode_unclosed_tag() {
        // No closing tag — regex doesn't match, passes through as-is
        let result = ContentBuilder::parse_bbcode_to_structured("[b]no close");
        assert_eq!(result, json!("[b]no close"));
    }

    // === Edge case: birthday with invalid month ===

    #[test]
    fn test_format_birthday_invalid_month() {
        assert_eq!(ContentBuilder::format_birthday(&[13, 1]), "Unknown 1");
        assert_eq!(ContentBuilder::format_birthday(&[0, 15]), "Unknown 15");
    }

    #[test]
    fn test_format_birthday_zero_day() {
        // Day 0 is technically invalid but we just format it
        assert_eq!(ContentBuilder::format_birthday(&[1, 0]), "January 0");
    }

    // === Edge case: format_stats with unusual values ===

    #[test]
    fn test_format_stats_unknown_sex() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.sex = Some("X".to_string());
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        // "X" is not in SEX_DISPLAY, so it's silently skipped
        assert_eq!(stats, "");
    }

    #[test]
    fn test_format_stats_zero_height_weight() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.sex = None;
        char.age = None;
        char.height = Some(0);
        char.weight = Some(0);
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert!(stats.contains("0cm"));
        assert!(stats.contains("0kg"));
    }

    #[test]
    fn test_format_stats_female() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.sex = Some("f".to_string());
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert!(stats.contains("Female"));
    }

    #[test]
    fn test_format_stats_female_full_word() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.sex = Some("female".to_string());
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert!(stats.contains("Female"));
    }

    // === Edge case: build_content with unknown role ===

    #[test]
    fn test_build_content_unknown_role() {
        let cb = ContentBuilder::new(0);
        let mut char = make_test_character();
        char.role = "custom_role".to_string();
        let content = cb.build_content(&char, None, "Test");
        let items = content["content"].as_array().unwrap();
        // Should use fallback color and "Unknown" label
        let role_span = items
            .iter()
            .find(|v| v["style"]["background"].as_str() == Some("#9E9E9E"));
        assert!(role_span.is_some(), "Unknown role should use gray fallback");
        assert_eq!(role_span.unwrap()["content"], "Unknown");
    }

    // === Edge case: build_content with empty game title ===

    #[test]
    fn test_build_content_empty_game_title() {
        let cb = ContentBuilder::new(0);
        let char = make_test_character();
        let content = cb.build_content(&char, None, "");
        let content_str = serde_json::to_string(&content).unwrap();
        // Empty game title should not produce a "From: " div
        assert!(!content_str.contains("From: "));
    }

    // === Edge case: description becomes empty after spoiler stripping ===

    #[test]
    fn test_build_content_description_only_spoilers() {
        let cb = ContentBuilder::new(1);
        let mut char = make_test_character();
        char.description = Some("[spoiler]everything is hidden[/spoiler]".to_string());
        let content = cb.build_content(&char, None, "Test");
        let items = content["content"].as_array().unwrap();
        // After stripping, description is empty → no Description details section
        let desc_details = items.iter().find(|v| {
            if let Some(arr) = v["content"].as_array() {
                arr.iter()
                    .any(|c| c["content"].as_str() == Some("Description"))
            } else {
                false
            }
        });
        assert!(
            desc_details.is_none(),
            "Empty description after stripping should not produce a details section"
        );
    }

    // === Edge case: traits with empty names filtered out ===

    #[test]
    fn test_traits_empty_name_filtered() {
        let cb = ContentBuilder::new(2);
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            },
        ];
        char.roles = vec![];
        let items = cb.build_traits_by_category(&char);
        let all_text: String = items
            .iter()
            .filter_map(|i| i["content"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_text.contains("Kind"));
        // Empty name should not appear
        assert!(!all_text.contains("Personality: , "));
    }

    // === Edge case: description with mixed spoiler formats ===

    #[test]
    fn test_strip_spoilers_mixed_formats() {
        let result = ContentBuilder::strip_spoilers(
            "visible [spoiler]vndb hidden[/spoiler] middle ~!anilist hidden!~ end",
        );
        assert_eq!(result, "visible  middle  end");
    }

    // === Edge case: VNDB markup with BBCode inside spoiler ===

    #[test]
    fn test_spoiler_then_bbcode() {
        // Spoiler stripping happens before BBCode parsing in build_content
        let stripped =
            ContentBuilder::strip_spoilers("[spoiler][b]hidden bold[/b][/spoiler] visible");
        assert_eq!(stripped, "visible");
    }
}
