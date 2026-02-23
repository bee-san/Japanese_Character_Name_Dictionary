/// Name parts result from splitting a Japanese name.
#[derive(Debug, Clone)]
pub struct JapaneseNameParts {
    pub has_space: bool,
    pub original: String,
    pub combined: String,
    pub family: Option<String>,
    pub given: Option<String>,
}

/// Name reading results.
#[derive(Debug, Clone)]
pub struct NameReadings {
    pub has_space: bool,
    pub original: String,
    pub full: String,    // Full hiragana reading (family + given)
    pub family: String,  // Family name hiragana reading
    pub given: String,   // Given name hiragana reading
}

/// Honorific suffixes: (display form appended to term, hiragana appended to reading)
pub const HONORIFIC_SUFFIXES: &[(&str, &str)] = &[
    // Respectful/Formal
    ("さん", "さん"),
    ("様", "さま"),
    ("先生", "せんせい"),
    ("先輩", "せんぱい"),
    ("後輩", "こうはい"),
    ("氏", "し"),
    // Casual/Friendly
    ("君", "くん"),
    ("くん", "くん"),
    ("ちゃん", "ちゃん"),
    ("たん", "たん"),
    ("坊", "ぼう"),
    // Old-fashioned/Archaic
    ("殿", "どの"),
    ("博士", "はかせ"),
    // Occupational/Specific
    ("社長", "しゃちょう"),
    ("部長", "ぶちょう"),
];

/// Check if text contains kanji characters.
/// Unicode ranges: CJK Unified Ideographs (0x4E00–0x9FFF) + Extension A (0x3400–0x4DBF).
pub fn contains_kanji(text: &str) -> bool {
    text.chars().any(|c| {
        let code = c as u32;
        (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code)
    })
}

/// Split a Japanese name on the first space.
/// Returns (family, given, combined, original, has_space)
pub fn split_japanese_name(name_original: &str) -> JapaneseNameParts {
    if name_original.is_empty() || !name_original.contains(' ') {
        return JapaneseNameParts {
            has_space: false,
            original: name_original.to_string(),
            combined: name_original.to_string(),
            family: None,
            given: None,
        };
    }

    // Split on first space only
    let pos = name_original.find(' ').unwrap();
    let family = name_original[..pos].to_string();
    let given = name_original[pos + 1..].to_string();
    let combined = format!("{}{}", family, given);

    JapaneseNameParts {
        has_space: true,
        original: name_original.to_string(),
        combined,
        family: Some(family),
        given: Some(given),
    }
}

/// Convert katakana to hiragana.
/// Katakana range: U+30A1 (ァ) to U+30F6 (ヶ). Subtract 0x60 to get hiragana equivalent.
pub fn kata_to_hira(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (0x30A1..=0x30F6).contains(&code) {
                char::from_u32(code - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Convert romanized text to hiragana.
/// Handles double consonants (っ), special 'n' rules, and multi-char sequences.
pub fn alphabet_to_kana(input: &str) -> String {
    let text = input.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < chars.len() {
        // 1. Double consonant check: if chars[i] == chars[i+1] and both are consonants → っ
        if i + 1 < chars.len()
            && chars[i] == chars[i + 1]
            && is_consonant(chars[i])
        {
            result.push('っ');
            i += 1; // Skip one; the second consonant starts the next match
            continue;
        }

        // 2. Try 3-character sequence
        if i + 3 <= chars.len() {
            let three: String = chars[i..i + 3].iter().collect();
            if let Some(kana) = lookup_romaji(&three) {
                result.push_str(kana);
                i += 3;
                continue;
            }
        }

        // 3. Try 2-character sequence
        if i + 2 <= chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            if let Some(kana) = lookup_romaji(&two) {
                result.push_str(kana);
                i += 2;
                continue;
            }
        }

        // 4. Special 'n' handling: ん only when NOT followed by a vowel or 'y'
        if chars[i] == 'n' {
            let next = chars.get(i + 1).copied();
            if next.is_none() || !is_vowel_or_y(next.unwrap()) {
                result.push('ん');
                i += 1;
                continue;
            }
        }

        // 5. Try 1-character sequence (vowels)
        let one = chars[i].to_string();
        if let Some(kana) = lookup_romaji(&one) {
            result.push_str(kana);
        } else {
            // Unknown character — pass through unchanged
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

fn is_consonant(c: char) -> bool {
    matches!(
        c,
        'b' | 'c' | 'd' | 'f' | 'g' | 'h' | 'j' | 'k' | 'l' | 'm' | 'n' | 'p' | 'q'
            | 'r' | 's' | 't' | 'v' | 'w' | 'x' | 'y' | 'z'
    )
}

fn is_vowel_or_y(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o' | 'y')
}

fn lookup_romaji(key: &str) -> Option<&'static str> {
    match key {
        // === 3-character sequences ===
        "sha" => Some("しゃ"), "shi" => Some("し"),  "shu" => Some("しゅ"), "sho" => Some("しょ"),
        "chi" => Some("ち"),   "tsu" => Some("つ"),
        "cha" => Some("ちゃ"), "chu" => Some("ちゅ"), "cho" => Some("ちょ"),
        "nya" => Some("にゃ"), "nyu" => Some("にゅ"), "nyo" => Some("にょ"),
        "hya" => Some("ひゃ"), "hyu" => Some("ひゅ"), "hyo" => Some("ひょ"),
        "mya" => Some("みゃ"), "myu" => Some("みゅ"), "myo" => Some("みょ"),
        "rya" => Some("りゃ"), "ryu" => Some("りゅ"), "ryo" => Some("りょ"),
        "gya" => Some("ぎゃ"), "gyu" => Some("ぎゅ"), "gyo" => Some("ぎょ"),
        "bya" => Some("びゃ"), "byu" => Some("びゅ"), "byo" => Some("びょ"),
        "pya" => Some("ぴゃ"), "pyu" => Some("ぴゅ"), "pyo" => Some("ぴょ"),
        "kya" => Some("きゃ"), "kyu" => Some("きゅ"), "kyo" => Some("きょ"),
        "jya" => Some("じゃ"), "jyu" => Some("じゅ"), "jyo" => Some("じょ"),

        // === 2-character sequences ===
        "ka" => Some("か"), "ki" => Some("き"), "ku" => Some("く"), "ke" => Some("け"), "ko" => Some("こ"),
        "sa" => Some("さ"), "si" => Some("し"), "su" => Some("す"), "se" => Some("せ"), "so" => Some("そ"),
        "ta" => Some("た"), "ti" => Some("ち"), "tu" => Some("つ"), "te" => Some("て"), "to" => Some("と"),
        "na" => Some("な"), "ni" => Some("に"), "nu" => Some("ぬ"), "ne" => Some("ね"), "no" => Some("の"),
        "ha" => Some("は"), "hi" => Some("ひ"), "hu" => Some("ふ"), "fu" => Some("ふ"), "he" => Some("へ"), "ho" => Some("ほ"),
        "ma" => Some("ま"), "mi" => Some("み"), "mu" => Some("む"), "me" => Some("め"), "mo" => Some("も"),
        "ra" => Some("ら"), "ri" => Some("り"), "ru" => Some("る"), "re" => Some("れ"), "ro" => Some("ろ"),
        "ya" => Some("や"), "yu" => Some("ゆ"), "yo" => Some("よ"),
        "wa" => Some("わ"), "wi" => Some("ゐ"), "we" => Some("ゑ"), "wo" => Some("を"),
        "ga" => Some("が"), "gi" => Some("ぎ"), "gu" => Some("ぐ"), "ge" => Some("げ"), "go" => Some("ご"),
        "za" => Some("ざ"), "zi" => Some("じ"), "zu" => Some("ず"), "ze" => Some("ぜ"), "zo" => Some("ぞ"),
        "da" => Some("だ"), "di" => Some("ぢ"), "du" => Some("づ"), "de" => Some("で"), "do" => Some("ど"),
        "ba" => Some("ば"), "bi" => Some("び"), "bu" => Some("ぶ"), "be" => Some("べ"), "bo" => Some("ぼ"),
        "pa" => Some("ぱ"), "pi" => Some("ぴ"), "pu" => Some("ぷ"), "pe" => Some("ぺ"), "po" => Some("ぽ"),
        "ja" => Some("じゃ"), "ju" => Some("じゅ"), "jo" => Some("じょ"),

        // === 1-character sequences (vowels only; 'n' handled separately) ===
        "a" => Some("あ"), "i" => Some("い"), "u" => Some("う"), "e" => Some("え"), "o" => Some("お"),

        _ => None,
    }
}

/// Generate hiragana readings for a name that may have mixed kanji/kana parts.
///
/// For each name part (family, given) independently:
/// - If part contains kanji → convert corresponding romanized part via alphabet_to_kana
/// - If part is kana only → use kata_to_hira directly on the Japanese text
///
/// IMPORTANT: Romanized names from VNDB are Western order ("Given Family").
/// Japanese names are Japanese order ("Family Given").
/// romanized_parts[0] maps to Japanese family; romanized_parts[1] maps to Japanese given.
pub fn generate_mixed_name_readings(
    name_original: &str,
    romanized_name: &str,
) -> NameReadings {
    // Handle empty names
    if name_original.is_empty() {
        return NameReadings {
            has_space: false,
            original: String::new(),
            full: String::new(),
            family: String::new(),
            given: String::new(),
        };
    }

    // For single-word names (no space)
    if !name_original.contains(' ') {
        if contains_kanji(name_original) {
            // Has kanji — use romanized reading
            let full = alphabet_to_kana(romanized_name);
            return NameReadings {
                has_space: false,
                original: name_original.to_string(),
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        } else {
            // Pure kana — use kata_to_hira on the Japanese text itself
            let full = kata_to_hira(&name_original.replace(' ', ""));
            return NameReadings {
                has_space: false,
                original: name_original.to_string(),
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        }
    }

    // Two-part name: split Japanese (Family Given order)
    let jp_parts = split_japanese_name(name_original);
    let family_jp = jp_parts.family.as_deref().unwrap_or("");
    let given_jp = jp_parts.given.as_deref().unwrap_or("");

    let family_has_kanji = contains_kanji(family_jp);
    let given_has_kanji = contains_kanji(given_jp);

    // Split romanized name (Western order: first_word second_word)
    let rom_parts: Vec<&str> = romanized_name.splitn(2, ' ').collect();
    let rom_first = rom_parts.first().copied().unwrap_or("");   // romanized_parts[0]
    let rom_second = rom_parts.get(1).copied().unwrap_or("");   // romanized_parts[1]

    // Family reading: if kanji, use rom_first (romanized_parts[0]) via alphabet_to_kana
    //                 if kana, use Japanese family text via kata_to_hira
    let family_reading = if family_has_kanji {
        alphabet_to_kana(rom_first)
    } else {
        kata_to_hira(family_jp)
    };

    // Given reading: if kanji, use rom_second (romanized_parts[1]) via alphabet_to_kana
    //                if kana, use Japanese given text via kata_to_hira
    let given_reading = if given_has_kanji {
        alphabet_to_kana(rom_second)
    } else {
        kata_to_hira(given_jp)
    };

    let full_reading = format!("{}{}", family_reading, given_reading);

    NameReadings {
        has_space: true,
        original: name_original.to_string(),
        full: full_reading,
        family: family_reading,
        given: given_reading,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Kanji detection tests ===

    #[test]
    fn test_contains_kanji_with_kanji() {
        assert!(contains_kanji("漢字"));
        assert!(contains_kanji("漢a"));
        assert!(contains_kanji("a漢"));
        assert!(contains_kanji("須々木"));
    }

    #[test]
    fn test_contains_kanji_without_kanji() {
        assert!(!contains_kanji("kana"));
        assert!(!contains_kanji("ひらがな"));
        assert!(!contains_kanji("カタカナ"));
        assert!(!contains_kanji("abc123"));
    }

    #[test]
    fn test_contains_kanji_empty() {
        assert!(!contains_kanji(""));
    }

    // === Name splitting tests ===

    #[test]
    fn test_split_japanese_name_with_space() {
        let parts = split_japanese_name("須々木 心一");
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("須々木"));
        assert_eq!(parts.given.as_deref(), Some("心一"));
        assert_eq!(parts.combined, "須々木心一");
        assert_eq!(parts.original, "須々木 心一");
    }

    #[test]
    fn test_split_japanese_name_no_space() {
        let parts = split_japanese_name("single");
        assert!(!parts.has_space);
        assert_eq!(parts.family, None);
        assert_eq!(parts.given, None);
        assert_eq!(parts.combined, "single");
    }

    #[test]
    fn test_split_japanese_name_empty() {
        let parts = split_japanese_name("");
        assert!(!parts.has_space);
        assert_eq!(parts.combined, "");
    }

    #[test]
    fn test_split_japanese_name_multiple_spaces() {
        // Should split on first space only
        let parts = split_japanese_name("A B C");
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("A"));
        assert_eq!(parts.given.as_deref(), Some("B C"));
    }

    // === Katakana to Hiragana tests ===

    #[test]
    fn test_kata_to_hira_basic() {
        assert_eq!(kata_to_hira("アイウエオ"), "あいうえお");
        assert_eq!(kata_to_hira("カキクケコ"), "かきくけこ");
    }

    #[test]
    fn test_kata_to_hira_mixed() {
        assert_eq!(kata_to_hira("あいカキ"), "あいかき");
    }

    #[test]
    fn test_kata_to_hira_romaji_passthrough() {
        assert_eq!(kata_to_hira("abc"), "abc");
    }

    #[test]
    fn test_kata_to_hira_empty() {
        assert_eq!(kata_to_hira(""), "");
    }

    // === Romaji to Kana tests ===

    #[test]
    fn test_alphabet_to_kana_simple_vowels() {
        assert_eq!(alphabet_to_kana("a"), "あ");
        assert_eq!(alphabet_to_kana("i"), "い");
        assert_eq!(alphabet_to_kana("u"), "う");
        assert_eq!(alphabet_to_kana("e"), "え");
        assert_eq!(alphabet_to_kana("o"), "お");
    }

    #[test]
    fn test_alphabet_to_kana_basic_syllables() {
        assert_eq!(alphabet_to_kana("ka"), "か");
        assert_eq!(alphabet_to_kana("shi"), "し");
        assert_eq!(alphabet_to_kana("tsu"), "つ");
        assert_eq!(alphabet_to_kana("fu"), "ふ");
    }

    #[test]
    fn test_alphabet_to_kana_words() {
        assert_eq!(alphabet_to_kana("sakura"), "さくら");
        assert_eq!(alphabet_to_kana("tokyo"), "ときょ");
    }

    #[test]
    fn test_alphabet_to_kana_double_consonant() {
        assert_eq!(alphabet_to_kana("kappa"), "かっぱ");
        assert_eq!(alphabet_to_kana("matte"), "まって");
    }

    #[test]
    fn test_alphabet_to_kana_n_rules() {
        // n before consonant = ん
        assert_eq!(alphabet_to_kana("kantan"), "かんたん");
        // n at end of string = ん
        assert_eq!(alphabet_to_kana("san"), "さん");
        // n before vowel = な/に/etc
        assert_eq!(alphabet_to_kana("kana"), "かな");
    }

    #[test]
    fn test_alphabet_to_kana_case_insensitive() {
        assert_eq!(alphabet_to_kana("Sakura"), "さくら");
        assert_eq!(alphabet_to_kana("TOKYO"), "ときょ");
    }

    #[test]
    fn test_alphabet_to_kana_compound_syllables() {
        assert_eq!(alphabet_to_kana("sha"), "しゃ");
        assert_eq!(alphabet_to_kana("chi"), "ち");
        assert_eq!(alphabet_to_kana("nya"), "にゃ");
        assert_eq!(alphabet_to_kana("ryo"), "りょ");
    }

    #[test]
    fn test_alphabet_to_kana_empty() {
        assert_eq!(alphabet_to_kana(""), "");
    }

    // === Mixed name reading tests ===

    #[test]
    fn test_mixed_readings_empty() {
        let r = generate_mixed_name_readings("", "");
        assert_eq!(r.full, "");
        assert_eq!(r.family, "");
        assert_eq!(r.given, "");
    }

    #[test]
    fn test_mixed_readings_single_kanji() {
        let r = generate_mixed_name_readings("漢", "Kan");
        assert_eq!(r.full, alphabet_to_kana("kan"));
    }

    #[test]
    fn test_mixed_readings_single_kana() {
        let r = generate_mixed_name_readings("あいう", "unused");
        assert_eq!(r.full, "あいう"); // Pure hiragana passes through
    }

    #[test]
    fn test_mixed_readings_single_katakana() {
        let r = generate_mixed_name_readings("アイウ", "unused");
        assert_eq!(r.full, "あいう"); // Katakana converted to hiragana
    }

    #[test]
    fn test_mixed_readings_two_part_both_kanji() {
        let r = generate_mixed_name_readings("漢 字", "Given Family");
        // Family (漢) has kanji -> uses rom_parts[0] ("Given")
        assert_eq!(r.family, alphabet_to_kana("given"));
        // Given (字) has kanji -> uses rom_parts[1] ("Family")
        assert_eq!(r.given, alphabet_to_kana("family"));
    }

    #[test]
    fn test_mixed_readings_two_part_mixed() {
        // Family has kanji, given is kana
        let r = generate_mixed_name_readings("漢 かな", "Romaji Unused");
        assert_eq!(r.family, alphabet_to_kana("romaji"));
        assert_eq!(r.given, "かな"); // Pure kana uses Japanese text directly
    }

    #[test]
    fn test_mixed_readings_two_part_all_kana() {
        let r = generate_mixed_name_readings("あい うえ", "Unused Unused2");
        assert_eq!(r.family, "あい");
        assert_eq!(r.given, "うえ");
        assert_eq!(r.full, "あいうえ");
    }

    // === Honorific suffixes tests ===

    #[test]
    fn test_honorific_suffixes_not_empty() {
        assert!(!HONORIFIC_SUFFIXES.is_empty());
        assert!(HONORIFIC_SUFFIXES.len() >= 10);
    }

    #[test]
    fn test_honorific_suffixes_contain_common() {
        let suffixes: Vec<&str> = HONORIFIC_SUFFIXES.iter().map(|(s, _)| *s).collect();
        assert!(suffixes.contains(&"さん"));
        assert!(suffixes.contains(&"ちゃん"));
        assert!(suffixes.contains(&"くん"));
    }
}
