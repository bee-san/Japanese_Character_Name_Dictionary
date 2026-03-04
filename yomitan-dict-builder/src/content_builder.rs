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

/// Per-dictionary display settings that control which sections appear in
/// the generated Yomitan structured-content cards.
#[derive(Clone, Debug)]
pub struct DictSettings {
    pub show_image: bool,
    pub show_tag: bool,
    pub show_description: bool,
    pub show_traits: bool,
    pub show_spoilers: bool,
    pub honorifics: bool,
}

impl Default for DictSettings {
    fn default() -> Self {
        Self {
            show_image: true,
            show_tag: true,
            show_description: true,
            show_traits: true,
            show_spoilers: true,
            honorifics: true,
        }
    }
}

pub struct ContentBuilder {
    settings: DictSettings,
}

impl ContentBuilder {
    pub fn new(settings: DictSettings) -> Self {
        Self { settings }
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

            // Filter traits: when spoilers are hidden, only include non-spoiler traits (spoiler == 0)
            let filtered: Vec<&str> = traits
                .iter()
                .filter(|t| !t.name.is_empty() && (self.settings.show_spoilers || t.spoiler == 0))
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

    /// Maximum display height for character portrait images (in CSS pixels).
    const MAX_DISPLAY_HEIGHT: u32 = 100;
    /// Fallback display width when actual dimensions are unknown.
    const FALLBACK_DISPLAY_WIDTH: u32 = 67;

    /// Build the complete Yomitan structured content for a character card.
    pub fn build_content(
        &self,
        char: &Character,
        image_path: Option<&str>,
        image_dims: Option<(u32, u32)>,
        game_title: &str,
    ) -> serde_json::Value {
        let mut content: Vec<serde_json::Value> = Vec::new();

        // ===== Always shown: Name block =====

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

        // ===== Character portrait image (gated by show_image) =====
        if self.settings.show_image {
            if let Some(path) = image_path {
                let (display_w, display_h) = match image_dims {
                    Some((w, h)) if w > 0 && h > 0 => {
                        let dh = Self::MAX_DISPLAY_HEIGHT;
                        let dw = (w * dh + h / 2) / h;
                        (dw, dh)
                    }
                    _ => (Self::FALLBACK_DISPLAY_WIDTH, Self::MAX_DISPLAY_HEIGHT),
                };
                content.push(json!({
                    "tag": "img",
                    "path": path,
                    "width": display_w,
                    "height": display_h,
                    "sizeUnits": "px",
                    "collapsible": false,
                    "collapsed": false,
                    "background": false
                }));
            }
        }

        // Game/media title (always shown)
        if !game_title.is_empty() {
            content.push(json!({
                "tag": "div",
                "style": { "fontSize": "0.9em", "color": "#888", "marginTop": "4px" },
                "content": format!("From: {}", game_title)
            }));
        }

        // ===== Role badge (gated by show_tag) =====
        if self.settings.show_tag {
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
        }

        // ===== Description section (gated by show_description) =====
        if self.settings.show_description {
            if let Some(ref desc) = char.description {
                if !desc.trim().is_empty() {
                    let display_desc = if !self.settings.show_spoilers {
                        Self::strip_spoilers(desc)
                    } else {
                        desc.clone()
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
        }

        // ===== Character Information / Traits section (gated by show_traits) =====
        if self.settings.show_traits {
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

            // Traits organized by category (filtered by spoiler setting)
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

    /// Helper: settings with nothing shown (equivalent to old spoiler_level 0)
    fn settings_minimal() -> DictSettings {
        DictSettings {
            show_image: true,
            show_tag: true,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            honorifics: true,
        }
    }

    /// Helper: settings with description+traits but spoilers stripped (old level 1)
    fn settings_no_spoilers() -> DictSettings {
        DictSettings {
            show_image: true,
            show_tag: true,
            show_description: true,
            show_traits: true,
            show_spoilers: false,
            honorifics: true,
        }
    }

    /// Helper: settings with everything shown (old spoiler_level 2)
    fn settings_full() -> DictSettings {
        DictSettings::default()
    }

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
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_full());
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
    fn test_traits_spoilers_hidden() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let items = cb.build_traits_by_category(&char);
        // With show_spoilers=false, only traits with spoiler=0 pass
        for item in &items {
            let content = item["content"].as_str().unwrap();
            assert!(!content.contains("Secret trait"));
        }
    }

    #[test]
    fn test_traits_spoilers_hidden_includes_nonspoiler() {
        let cb = ContentBuilder::new(settings_no_spoilers());
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
    fn test_traits_spoilers_shown() {
        let cb = ContentBuilder::new(settings_full());
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
    fn test_build_content_minimal() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        // Minimal settings: should NOT contain <details> tags (no description/traits)
        let has_details = items.iter().any(|v| v["tag"].as_str() == Some("details"));
        assert!(
            !has_details,
            "Minimal settings should not contain details sections"
        );
        // Should contain name and role
        let has_span = items.iter().any(|v| v["tag"].as_str() == Some("span"));
        assert!(has_span, "Should contain role badge span");
    }

    #[test]
    fn test_build_content_no_spoilers() {
        let cb = ContentBuilder::new(settings_no_spoilers());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        // Should contain <details> tags (Description + Character Information)
        let details_count = items
            .iter()
            .filter(|v| v["tag"].as_str() == Some("details"))
            .count();
        assert!(
            details_count >= 1,
            "No-spoilers settings should have details sections"
        );
    }

    #[test]
    fn test_build_content_full() {
        let cb = ContentBuilder::new(settings_full());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        let items = content["content"].as_array().unwrap();
        let details_count = items
            .iter()
            .filter(|v| v["tag"].as_str() == Some("details"))
            .count();
        assert!(
            details_count >= 1,
            "Full settings should have details sections"
        );
    }

    #[test]
    fn test_build_content_with_image() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c123.jpg"), None, "Test Game");
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_full());
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
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.role = "custom_role".to_string();
        let content = cb.build_content(&char, None, None, "Test");
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
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "");
        let content_str = serde_json::to_string(&content).unwrap();
        // Empty game title should not produce a "From: " div
        assert!(!content_str.contains("From: "));
    }

    // === Edge case: description becomes empty after spoiler stripping ===

    #[test]
    fn test_build_content_description_only_spoilers() {
        let cb = ContentBuilder::new(settings_no_spoilers());
        let mut char = make_test_character();
        char.description = Some("[spoiler]everything is hidden[/spoiler]".to_string());
        let content = cb.build_content(&char, None, None, "Test");
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
        let cb = ContentBuilder::new(settings_full());
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

    // ===== Additional comprehensive tests =====

    // --- Spoiler stripping edge cases ---

    #[test]
    fn test_strip_spoilers_empty_spoiler_tags() {
        assert_eq!(ContentBuilder::strip_spoilers("[spoiler][/spoiler]"), "");
        assert_eq!(ContentBuilder::strip_spoilers("~!!~"), "");
    }

    #[test]
    fn test_strip_spoilers_adjacent_spoilers() {
        let result = ContentBuilder::strip_spoilers("[spoiler]a[/spoiler][spoiler]b[/spoiler]");
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_spoilers_preserves_surrounding_whitespace() {
        let result = ContentBuilder::strip_spoilers("before  [spoiler]hidden[/spoiler]  after");
        assert_eq!(result, "before    after");
    }

    #[test]
    fn test_strip_spoilers_case_insensitive_vndb() {
        // VNDB spoiler tags should be case-insensitive
        let result = ContentBuilder::strip_spoilers("[SPOILER]hidden[/SPOILER]");
        assert_eq!(result, "");
    }

    // --- VNDB markup parsing ---

    #[test]
    fn test_parse_vndb_markup_nested_url() {
        let result = ContentBuilder::parse_vndb_markup("[url=https://example.com]Click here[/url]");
        assert_eq!(result, "Click here");
    }

    #[test]
    fn test_parse_vndb_markup_multiple_urls() {
        let result = ContentBuilder::parse_vndb_markup(
            "[url=https://a.com]A[/url] and [url=https://b.com]B[/url]",
        );
        assert_eq!(result, "A and B");
    }

    #[test]
    fn test_parse_vndb_markup_all_tags_combined() {
        let input = "[url=x]link[/url] [quote]quoted[/quote] [code]code[/code] [raw]raw[/raw] [u]under[/u] [s]strike[/s]";
        let result = ContentBuilder::parse_vndb_markup(input);
        assert_eq!(result, "link quoted code raw under strike");
    }

    #[test]
    fn test_parse_vndb_markup_empty_tags() {
        assert_eq!(ContentBuilder::parse_vndb_markup("[quote][/quote]"), "");
        assert_eq!(ContentBuilder::parse_vndb_markup("[code][/code]"), "");
    }

    #[test]
    fn test_parse_vndb_markup_no_tags() {
        let text = "Just plain text with no markup at all.";
        assert_eq!(ContentBuilder::parse_vndb_markup(text), text);
    }

    // --- BBCode parsing ---

    #[test]
    fn test_parse_bbcode_bold_and_italic_combined() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b]bold[/b] and [i]italic[/i]");
        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        // Should have: bold node, " and ", italic node
        assert!(arr.len() >= 3);
    }

    #[test]
    fn test_parse_bbcode_deeply_nested() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b][i]bold italic[/i][/b]");
        // Should produce nested structure
        assert!(!result.is_null());
    }

    #[test]
    fn test_parse_bbcode_adjacent_same_tags() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b]first[/b][b]second[/b]");
        assert!(result.is_array());
    }

    #[test]
    fn test_parse_bbcode_with_special_chars() {
        let result = ContentBuilder::parse_bbcode_to_structured("[b]<>&\"'[/b]");
        // Should handle special chars without issues
        assert!(!result.is_null());
    }

    #[test]
    fn test_parse_bbcode_plain_text_returns_string() {
        let result = ContentBuilder::parse_bbcode_to_structured("no tags here");
        assert!(result.is_string());
        assert_eq!(result.as_str().unwrap(), "no tags here");
    }

    // --- Birthday formatting ---

    #[test]
    fn test_format_birthday_all_months() {
        let months = [
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
        for (month, name) in months {
            let result = ContentBuilder::format_birthday(&[month, 15]);
            assert_eq!(result, format!("{} 15", name));
        }
    }

    #[test]
    fn test_format_birthday_month_13() {
        let result = ContentBuilder::format_birthday(&[13, 1]);
        assert_eq!(result, "Unknown 1");
    }

    #[test]
    fn test_format_birthday_empty_array() {
        assert_eq!(ContentBuilder::format_birthday(&[]), "");
    }

    #[test]
    fn test_format_birthday_single_element() {
        assert_eq!(ContentBuilder::format_birthday(&[5]), "");
    }

    #[test]
    fn test_format_birthday_extra_elements_ignored() {
        // Only first two elements matter
        let result = ContentBuilder::format_birthday(&[3, 14, 1999]);
        assert_eq!(result, "March 14");
    }

    // --- Stats formatting ---

    #[test]
    fn test_format_stats_all_fields() {
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.sex = Some("m".to_string());
        char.age = Some("25".to_string());
        char.height = Some(180);
        char.weight = Some(75);
        char.blood_type = Some("AB".to_string());
        char.birthday = Some(vec![7, 4]);
        let stats = cb.format_stats(&char);
        assert!(stats.contains("♂ Male"));
        assert!(stats.contains("25 years"));
        assert!(stats.contains("180cm"));
        assert!(stats.contains("75kg"));
        assert!(stats.contains("Blood Type AB"));
        assert!(stats.contains("Birthday: July 4"));
    }

    #[test]
    fn test_format_stats_no_fields() {
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.sex = None;
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        assert_eq!(cb.format_stats(&char), "");
    }

    #[test]
    fn test_format_stats_separator() {
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.sex = Some("f".to_string());
        char.age = Some("17".to_string());
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let stats = cb.format_stats(&char);
        assert!(stats.contains(" • "), "Stats should be separated by bullet");
    }

    // --- Structured content validation ---

    #[test]
    fn test_build_content_has_required_structure() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        assert_eq!(content["type"], "structured-content");
        assert!(content["content"].is_array());
    }

    #[test]
    fn test_build_content_includes_japanese_name() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains(&char.name_original));
    }

    #[test]
    fn test_build_content_includes_romanized_name() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains(&char.name));
    }

    #[test]
    fn test_build_content_includes_game_title() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Steins;Gate");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("Steins;Gate"));
    }

    #[test]
    fn test_build_content_includes_role_badge() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Test");
        let content_str = serde_json::to_string(&content).unwrap();
        // Should contain a role label
        assert!(
            content_str.contains("Protagonist")
                || content_str.contains("Main Character")
                || content_str.contains("Side Character")
                || content_str.contains("Minor Role"),
            "Should contain a role label"
        );
    }

    #[test]
    fn test_build_content_description_disabled() {
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.description = Some("A detailed description".to_string());
        let content = cb.build_content(&char, None, None, "Test");
        let content_str = serde_json::to_string(&content).unwrap();
        // With show_description=false, description should NOT be included
        assert!(!content_str.contains("A detailed description"));
    }

    #[test]
    fn test_build_content_description_enabled() {
        let cb = ContentBuilder::new(settings_no_spoilers());
        let mut char = make_test_character();
        char.description = Some("A detailed description".to_string());
        let content = cb.build_content(&char, None, None, "Test");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("A detailed description"));
    }

    #[test]
    fn test_build_content_with_image_includes_img_tag() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 150)), "Test");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("img/c1.jpg"));
        assert!(content_str.contains("\"tag\":\"img\""));
    }

    #[test]
    fn test_build_content_image_dimensions_calculated() {
        let cb = ContentBuilder::new(settings_minimal());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Test");
        let content_str = serde_json::to_string(&content).unwrap();
        // Should contain width and height attributes
        assert!(content_str.contains("\"width\""));
        assert!(content_str.contains("\"height\""));
    }

    // --- Term entry format ---

    #[test]
    fn test_create_term_entry_is_8_element_array() {
        let content = json!({"type": "structured-content", "content": []});
        let entry = ContentBuilder::create_term_entry("田中", "たなか", "main", 100, &content);
        assert!(entry.is_array());
        assert_eq!(entry.as_array().unwrap().len(), 8);
    }

    #[test]
    fn test_create_term_entry_fields() {
        let content = json!({"type": "structured-content", "content": []});
        let entry = ContentBuilder::create_term_entry("田中", "たなか", "main", 100, &content);
        let arr = entry.as_array().unwrap();
        assert_eq!(arr[0], "田中"); // term
        assert_eq!(arr[1], "たなか"); // reading
        assert_eq!(arr[2], "name main"); // definition tags
        assert_eq!(arr[3], ""); // rules
        assert_eq!(arr[4], 100); // score
        assert!(arr[5].is_array()); // definitions array
        assert_eq!(arr[6], 0); // sequence
        assert_eq!(arr[7], ""); // term tags
    }

    #[test]
    fn test_create_term_entry_empty_role_uses_name_only() {
        let content = json!({"type": "structured-content", "content": []});
        let entry = ContentBuilder::create_term_entry("田中", "たなか", "", 0, &content);
        let arr = entry.as_array().unwrap();
        assert_eq!(arr[2], "name"); // just "name" without role
    }

    // --- Honorific content ---

    #[test]
    fn test_build_honorific_content_structure() {
        let base = json!({
            "type": "structured-content",
            "content": [
                {"tag": "div", "content": "test"}
            ]
        });
        let result =
            ContentBuilder::build_honorific_content(&base, "さん", "Generic polite suffix");
        assert_eq!(result["type"], "structured-content");
        let content = result["content"].as_array().unwrap();
        // First element should be the honorific banner
        assert!(content.len() >= 2);
        let banner = &content[0];
        assert_eq!(banner["tag"], "div");
        let banner_str = serde_json::to_string(banner).unwrap();
        assert!(banner_str.contains("さん"));
        assert!(banner_str.contains("Generic polite suffix"));
    }

    #[test]
    fn test_build_honorific_content_non_array_base() {
        // If base content is not an array, should return base unchanged
        let base = json!({"type": "structured-content", "content": "just a string"});
        let result = ContentBuilder::build_honorific_content(&base, "さん", "test");
        assert_eq!(result, base);
    }

    // --- Trait filtering ---

    #[test]
    fn test_traits_spoilers_off_filters_all_spoilers() {
        let cb = ContentBuilder::new(settings_minimal());
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Secret".to_string(),
                spoiler: 1,
            },
            CharacterTrait {
                name: "Big Secret".to_string(),
                spoiler: 2,
            },
        ];
        let traits = cb.build_traits_by_category(&char);
        let traits_str = serde_json::to_string(&traits).unwrap();
        assert!(traits_str.contains("Kind"));
        assert!(!traits_str.contains("Secret"));
    }

    #[test]
    fn test_traits_spoilers_off_excludes_all_spoiler_levels() {
        let cb = ContentBuilder::new(settings_no_spoilers());
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Minor".to_string(),
                spoiler: 1,
            },
            CharacterTrait {
                name: "Major".to_string(),
                spoiler: 2,
            },
        ];
        let traits = cb.build_traits_by_category(&char);
        let traits_str = serde_json::to_string(&traits).unwrap();
        assert!(traits_str.contains("Kind"));
        assert!(!traits_str.contains("Minor"));
        assert!(!traits_str.contains("Major"));
    }

    #[test]
    fn test_traits_spoilers_on_shows_all() {
        let cb = ContentBuilder::new(settings_full());
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Minor".to_string(),
                spoiler: 1,
            },
            CharacterTrait {
                name: "Major".to_string(),
                spoiler: 2,
            },
        ];
        let traits = cb.build_traits_by_category(&char);
        let traits_str = serde_json::to_string(&traits).unwrap();
        assert!(traits_str.contains("Kind"));
        assert!(traits_str.contains("Minor"));
        assert!(traits_str.contains("Major"));
    }

    // ===================================================================
    // DictSettings individual toggle tests — verify each setting gates
    // exactly the right section of the structured content card
    // ===================================================================

    // --- DictSettings::default() ---

    #[test]
    fn test_dict_settings_default_all_true() {
        let s = DictSettings::default();
        assert!(s.show_image);
        assert!(s.show_tag);
        assert!(s.show_description);
        assert!(s.show_traits);
        assert!(s.show_spoilers);
        assert!(s.honorifics);
    }

    #[test]
    fn test_dict_settings_clone() {
        let s = DictSettings {
            show_image: false,
            show_tag: true,
            show_description: false,
            show_traits: true,
            show_spoilers: false,
            honorifics: false,
        };
        let cloned = s.clone();
        assert_eq!(cloned.show_image, false);
        assert_eq!(cloned.show_tag, true);
        assert_eq!(cloned.show_description, false);
        assert_eq!(cloned.show_traits, true);
        assert_eq!(cloned.show_spoilers, false);
        assert_eq!(cloned.honorifics, false);
    }

    // --- show_image toggle ---

    #[test]
    fn test_show_image_true_includes_img_tag() {
        let cb = ContentBuilder::new(DictSettings {
            show_image: true,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Game");
        let items = content["content"].as_array().unwrap();
        let has_img = items.iter().any(|v| v["tag"].as_str() == Some("img"));
        assert!(has_img, "show_image=true should include img tag");
    }

    #[test]
    fn test_show_image_false_excludes_img_tag() {
        let cb = ContentBuilder::new(DictSettings {
            show_image: false,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Game");
        let items = content["content"].as_array().unwrap();
        let has_img = items.iter().any(|v| v["tag"].as_str() == Some("img"));
        assert!(!has_img, "show_image=false should exclude img tag");
    }

    #[test]
    fn test_show_image_false_still_shows_name_and_role() {
        let cb = ContentBuilder::new(DictSettings {
            show_image: false,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains(&char.name_original));
        assert!(content_str.contains(&char.name));
    }

    // --- show_tag toggle ---

    #[test]
    fn test_show_tag_true_includes_role_badge() {
        let cb = ContentBuilder::new(DictSettings {
            show_tag: true,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            content_str.contains("Protagonist")
                || content_str.contains("Main Character")
                || content_str.contains("Side Character")
                || content_str.contains("Minor Role"),
            "show_tag=true should include role badge"
        );
    }

    #[test]
    fn test_show_tag_false_excludes_role_badge() {
        let cb = ContentBuilder::new(DictSettings {
            show_tag: false,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let items = content["content"].as_array().unwrap();
        // Role badge uses background color styling — verify no span with background exists
        let role_spans: Vec<_> = items
            .iter()
            .filter(|v| v["tag"].as_str() == Some("span") && v["style"]["background"].is_string())
            .collect();
        assert!(
            role_spans.is_empty(),
            "show_tag=false should not have role badge spans"
        );
    }

    // --- show_description toggle ---

    #[test]
    fn test_show_description_true_includes_description_details() {
        let cb = ContentBuilder::new(DictSettings {
            show_description: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.description = Some("This is a description.".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("Description"));
        assert!(content_str.contains("This is a description."));
    }

    #[test]
    fn test_show_description_false_excludes_description() {
        let cb = ContentBuilder::new(DictSettings {
            show_description: false,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.description = Some("This is a description.".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            !content_str.contains("This is a description."),
            "show_description=false should not include description text"
        );
    }

    #[test]
    fn test_show_description_false_does_not_affect_traits() {
        let cb = ContentBuilder::new(DictSettings {
            show_description: false,
            show_traits: true,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        // Traits section should still be present
        assert!(
            content_str.contains("Character Information"),
            "Disabling description should not affect traits"
        );
    }

    // --- show_traits toggle ---

    #[test]
    fn test_show_traits_true_includes_character_information() {
        let cb = ContentBuilder::new(DictSettings {
            show_traits: true,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            content_str.contains("Character Information"),
            "show_traits=true should include Character Information section"
        );
    }

    #[test]
    fn test_show_traits_false_excludes_character_information() {
        let cb = ContentBuilder::new(DictSettings {
            show_traits: false,
            ..DictSettings::default()
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            !content_str.contains("Character Information"),
            "show_traits=false should exclude Character Information section"
        );
    }

    #[test]
    fn test_show_traits_false_does_not_affect_description() {
        let cb = ContentBuilder::new(DictSettings {
            show_traits: false,
            show_description: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.description = Some("A unique description.".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            content_str.contains("A unique description."),
            "Disabling traits should not affect description"
        );
    }

    // --- show_spoilers toggle ---

    #[test]
    fn test_show_spoilers_true_includes_spoiler_text_in_description() {
        let cb = ContentBuilder::new(DictSettings {
            show_spoilers: true,
            show_description: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.description = Some("Visible [spoiler]Secret info[/spoiler] end".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            content_str.contains("Secret info"),
            "show_spoilers=true should keep spoiler text"
        );
    }

    #[test]
    fn test_show_spoilers_false_strips_spoiler_text_from_description() {
        let cb = ContentBuilder::new(DictSettings {
            show_spoilers: false,
            show_description: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.description = Some("Visible [spoiler]Secret info[/spoiler] end".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            !content_str.contains("Secret info"),
            "show_spoilers=false should strip spoiler text"
        );
        assert!(
            content_str.contains("Visible"),
            "Non-spoiler text should remain"
        );
    }

    #[test]
    fn test_show_spoilers_false_filters_spoiler_traits() {
        let cb = ContentBuilder::new(DictSettings {
            show_spoilers: false,
            show_traits: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "Safe".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Minor spoiler".to_string(),
                spoiler: 1,
            },
            CharacterTrait {
                name: "Major spoiler".to_string(),
                spoiler: 2,
            },
        ];
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("Safe"));
        assert!(!content_str.contains("Minor spoiler"));
        assert!(!content_str.contains("Major spoiler"));
    }

    #[test]
    fn test_show_spoilers_true_includes_all_traits() {
        let cb = ContentBuilder::new(DictSettings {
            show_spoilers: true,
            show_traits: true,
            ..DictSettings::default()
        });
        let mut char = make_test_character();
        char.personality = vec![
            CharacterTrait {
                name: "Safe".to_string(),
                spoiler: 0,
            },
            CharacterTrait {
                name: "Dangerous".to_string(),
                spoiler: 2,
            },
        ];
        let content = cb.build_content(&char, None, None, "Game");
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(content_str.contains("Safe"));
        assert!(content_str.contains("Dangerous"));
    }

    // --- Combined toggles: all off ---

    #[test]
    fn test_all_toggles_off_only_name_and_source_remain() {
        let cb = ContentBuilder::new(DictSettings {
            show_image: false,
            show_tag: false,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            honorifics: false,
        });
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Game Title");
        let items = content["content"].as_array().unwrap();

        // Should only have: Japanese name div, Romanized name div, "From: Game Title" div
        for item in items {
            let tag = item["tag"].as_str().unwrap_or("");
            assert_eq!(tag, "div", "With all sections off, only div elements (names + source) should remain, got tag '{}'", tag);
        }
        let content_str = serde_json::to_string(&content).unwrap();
        assert!(
            content_str.contains(&char.name_original),
            "Japanese name should always be present"
        );
        assert!(
            content_str.contains(&char.name),
            "Romanized name should always be present"
        );
        assert!(
            content_str.contains("Game Title"),
            "Media source should always be present"
        );
        assert!(!content_str.contains("\"img\""), "No img tag");
        assert!(!content_str.contains("Description"), "No description");
        assert!(!content_str.contains("Character Information"), "No traits");
    }

    // --- Combined toggles: all on ---

    #[test]
    fn test_all_toggles_on_all_sections_present() {
        let cb = ContentBuilder::new(DictSettings::default());
        let mut char = make_test_character();
        char.description = Some("Has a description".to_string());
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Game");
        let content_str = serde_json::to_string(&content).unwrap();

        assert!(content_str.contains("\"img\""), "Should have img tag");
        assert!(
            content_str.contains("Protagonist"),
            "Should have role badge"
        );
        assert!(
            content_str.contains("Description"),
            "Should have description details"
        );
        assert!(
            content_str.contains("Character Information"),
            "Should have traits section"
        );
    }

    // --- Content structure counts ---

    #[test]
    fn test_content_element_count_minimal() {
        let cb = ContentBuilder::new(DictSettings {
            show_image: false,
            show_tag: false,
            show_description: false,
            show_traits: false,
            show_spoilers: false,
            honorifics: false,
        });
        let char = make_test_character();
        let content = cb.build_content(&char, None, None, "Game");
        let items = content["content"].as_array().unwrap();
        // Expect: Japanese name, Romanized name, "From: Game" = 3 divs
        assert_eq!(
            items.len(),
            3,
            "Minimal settings should produce 3 elements (jp name, romaji name, source)"
        );
    }

    #[test]
    fn test_content_element_count_full_with_image() {
        let cb = ContentBuilder::new(DictSettings::default());
        let mut char = make_test_character();
        char.description = Some("desc".to_string());
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((100, 200)), "Game");
        let items = content["content"].as_array().unwrap();
        // Expect: jp name, romaji name, img, source, role badge, description details, traits details = 7
        assert_eq!(
            items.len(),
            7,
            "Full settings with image should produce 7 elements"
        );
    }

    #[test]
    fn test_content_element_count_no_image() {
        let cb = ContentBuilder::new(DictSettings::default());
        let mut char = make_test_character();
        char.description = Some("desc".to_string());
        let content = cb.build_content(&char, None, None, "Game");
        let items = content["content"].as_array().unwrap();
        // No image: jp name, romaji name, source, role badge, description, traits = 6
        assert_eq!(
            items.len(),
            6,
            "Full settings without image should produce 6 elements"
        );
    }

    // --- Spoiler stripping in description vs. keeping in traits ---

    #[test]
    fn test_spoiler_stripping_only_affects_description_not_structure() {
        let cb_no_spoilers = ContentBuilder::new(DictSettings {
            show_spoilers: false,
            show_description: true,
            show_traits: true,
            ..DictSettings::default()
        });
        let cb_with_spoilers = ContentBuilder::new(DictSettings::default());
        let mut char = make_test_character();
        char.description = Some("Visible [spoiler]Hidden[/spoiler] end".to_string());

        let content_no = cb_no_spoilers.build_content(&char, None, None, "G");
        let content_yes = cb_with_spoilers.build_content(&char, None, None, "G");

        // Both should have description details section
        let items_no = content_no["content"].as_array().unwrap();
        let items_yes = content_yes["content"].as_array().unwrap();
        let has_details = |items: &[serde_json::Value]| {
            items.iter().any(|v| v["tag"].as_str() == Some("details"))
        };
        assert!(
            has_details(items_no),
            "Description section should exist even without spoilers"
        );
        assert!(
            has_details(items_yes),
            "Description section should exist with spoilers"
        );

        let str_no = serde_json::to_string(&content_no).unwrap();
        let str_yes = serde_json::to_string(&content_yes).unwrap();
        assert!(!str_no.contains("Hidden"));
        assert!(str_yes.contains("Hidden"));
    }

    // --- Empty character edge cases with settings ---

    #[test]
    fn test_empty_names_with_all_settings() {
        let cb = ContentBuilder::new(DictSettings::default());
        let mut char = make_test_character();
        char.name = String::new();
        char.name_original = String::new();
        char.description = None;
        char.personality = vec![];
        char.roles = vec![];
        char.sex = None;
        char.age = None;
        char.height = None;
        char.weight = None;
        char.blood_type = None;
        char.birthday = None;
        let content = cb.build_content(&char, None, None, "");
        let items = content["content"].as_array().unwrap();
        // With everything empty and no title: should only have role badge span
        assert!(
            !items.is_empty(),
            "Even empty character should produce at least the role badge"
        );
    }

    // --- Image dimension edge cases ---

    #[test]
    fn test_image_with_zero_dimensions_uses_fallback() {
        let cb = ContentBuilder::new(DictSettings::default());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((0, 0)), "Game");
        let items = content["content"].as_array().unwrap();
        let img = items
            .iter()
            .find(|v| v["tag"].as_str() == Some("img"))
            .unwrap();
        assert_eq!(
            img["width"].as_u64().unwrap(),
            67,
            "Should use fallback width"
        );
        assert_eq!(
            img["height"].as_u64().unwrap(),
            100,
            "Should use fallback height"
        );
    }

    #[test]
    fn test_image_with_valid_dimensions_calculates_proportionally() {
        let cb = ContentBuilder::new(DictSettings::default());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), Some((200, 400)), "Game");
        let items = content["content"].as_array().unwrap();
        let img = items
            .iter()
            .find(|v| v["tag"].as_str() == Some("img"))
            .unwrap();
        // height capped at 100, so width = (200 * 100 + 200) / 400 = 50
        assert_eq!(img["height"].as_u64().unwrap(), 100);
        assert_eq!(img["width"].as_u64().unwrap(), 50);
    }

    #[test]
    fn test_image_none_dimensions_uses_fallback() {
        let cb = ContentBuilder::new(DictSettings::default());
        let char = make_test_character();
        let content = cb.build_content(&char, Some("img/c1.jpg"), None, "Game");
        let items = content["content"].as_array().unwrap();
        let img = items
            .iter()
            .find(|v| v["tag"].as_str() == Some("img"))
            .unwrap();
        assert_eq!(img["width"].as_u64().unwrap(), 67);
        assert_eq!(img["height"].as_u64().unwrap(), 100);
    }

    // --- Role color/label mapping ---

    #[test]
    fn test_all_roles_produce_correct_labels() {
        let roles_labels = [
            ("main", "Protagonist"),
            ("primary", "Main Character"),
            ("side", "Side Character"),
            ("appears", "Minor Role"),
        ];
        for (role, expected_label) in roles_labels {
            let cb = ContentBuilder::new(DictSettings::default());
            let mut char = make_test_character();
            char.role = role.to_string();
            let content = cb.build_content(&char, None, None, "Game");
            let content_str = serde_json::to_string(&content).unwrap();
            assert!(
                content_str.contains(expected_label),
                "Role '{}' should produce label '{}', content: {}",
                role,
                expected_label,
                content_str
            );
        }
    }

    #[test]
    fn test_all_roles_produce_correct_colors() {
        let roles_colors = [
            ("main", "#4CAF50"),
            ("primary", "#2196F3"),
            ("side", "#FF9800"),
            ("appears", "#9E9E9E"),
        ];
        for (role, expected_color) in roles_colors {
            let cb = ContentBuilder::new(DictSettings::default());
            let mut char = make_test_character();
            char.role = role.to_string();
            let content = cb.build_content(&char, None, None, "Game");
            let content_str = serde_json::to_string(&content).unwrap();
            assert!(
                content_str.contains(expected_color),
                "Role '{}' should use color '{}'",
                role,
                expected_color
            );
        }
    }
}
