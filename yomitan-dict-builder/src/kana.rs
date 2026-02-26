/// Low-level kana conversion utilities.
///
/// Provides romaji→hiragana, katakana↔hiragana conversion, and kanji detection.
/// These are pure text transforms with no name-level semantics.

/// Check if text contains kanji characters.
/// Covers CJK Unified Ideographs, Extensions A–H, and Compatibility Ideographs.
pub fn contains_kanji(text: &str) -> bool {
    text.chars().any(is_kanji)
}

/// Returns true if the character is a CJK ideograph (kanji).
fn is_kanji(c: char) -> bool {
    let code = c as u32;
    // CJK Unified Ideographs
    (0x4E00..=0x9FFF).contains(&code)
    // Extension A
    || (0x3400..=0x4DBF).contains(&code)
    // Extension B
    || (0x20000..=0x2A6DF).contains(&code)
    // Extension C
    || (0x2A700..=0x2B73F).contains(&code)
    // Extension D
    || (0x2B740..=0x2B81F).contains(&code)
    // Extension E
    || (0x2B820..=0x2CEAF).contains(&code)
    // Extension F
    || (0x2CEB0..=0x2EBEF).contains(&code)
    // Extension G
    || (0x30000..=0x3134F).contains(&code)
    // Extension H
    || (0x31350..=0x323AF).contains(&code)
    // CJK Compatibility Ideographs
    || (0xF900..=0xFAFF).contains(&code)
    // CJK Compatibility Ideographs Supplement
    || (0x2F800..=0x2FA1F).contains(&code)
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

/// Convert hiragana to katakana.
/// Hiragana range: U+3041 (ぁ) to U+3096 (ゖ). Add 0x60 to get katakana equivalent.
pub fn hira_to_kata(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (0x3041..=0x3096).contains(&code) {
                char::from_u32(code + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Returns true if the character is a syllable boundary marker in romanized Japanese.
///
/// In romanized names, apostrophes and hyphens serve as disambiguation markers:
/// - Apostrophe: "Shin'ichi" means しんいち (ん+い), not しにち (に)
/// - Hyphen: occasionally used similarly in some romanization systems
///
/// These characters force the preceding 'n' to be treated as ん (syllabic n)
/// rather than the start of a な-row syllable.
fn is_syllable_boundary(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '-' | '.')
}

/// Convert romanized text to hiragana.
/// Handles double consonants (っ), special 'n' rules, multi-char sequences,
/// and syllable boundary markers (apostrophes, hyphens).
///
/// Syllable boundary markers like apostrophes force the preceding 'n' to become ん.
/// For example: "Shin'ichi" → し+ん+い+ち = しんいち (not しにち).
/// Other non-alphabetic characters (digits, misc punctuation) are silently dropped.
pub fn alphabet_to_kana(input: &str) -> String {
    let text = input.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Handle syllable boundary markers: force pending 'n' to ん
        // The 'n' would already have been emitted or not by this point,
        // but the boundary marker tells us to treat the NEXT segment as
        // a fresh syllable start. We just skip the marker itself.
        if is_syllable_boundary(c) {
            // If the previous character was 'n' that got consumed as part of
            // a な-row check, we need to handle that. But actually, the way
            // the algorithm works, we need to check: if the char before the
            // boundary is 'n' and it hasn't been consumed yet...
            //
            // Simpler approach: when we see a boundary marker after 'n',
            // the 'n' rule (step 4) would have already NOT matched because
            // the next char is the boundary marker (not a vowel/y), so 'n'
            // would have been emitted as ん. The boundary marker just needs
            // to be skipped.
            i += 1;
            continue;
        }

        // Skip non-ASCII-alphabetic, non-space characters (digits, misc punctuation)
        if !c.is_ascii_alphabetic() && c != ' ' {
            i += 1;
            continue;
        }

        // Preserve spaces (used for name part splitting upstream)
        if c == ' ' {
            result.push(' ');
            i += 1;
            continue;
        }

        // 1. Double consonant check: if chars[i] == chars[i+1] and both are consonants → っ
        if i + 1 < chars.len() && chars[i] == chars[i + 1] && is_consonant(chars[i]) {
            result.push('っ');
            i += 1;
            continue;
        }

        // 2. Try 3-character sequence (skip non-alpha chars when building the window)
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

        // 4. Special 'n' handling: ん when NOT followed by a vowel or 'y'
        //    A syllable boundary marker after 'n' means the next char is NOT
        //    a vowel (it's the marker), so 'n' correctly becomes ん.
        if chars[i] == 'n' {
            let next = next_alpha_char(&chars, i + 1);
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
            // Unknown alphabetic character — pass through unchanged
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

/// Look ahead past syllable boundary markers to find the next alphabetic character.
fn next_alpha_char(chars: &[char], start: usize) -> Option<char> {
    chars.get(start).copied()
}

fn is_consonant(c: char) -> bool {
    matches!(
        c,
        'b' | 'c'
            | 'd'
            | 'f'
            | 'g'
            | 'h'
            | 'j'
            | 'k'
            | 'l'
            | 'm'
            | 'n'
            | 'p'
            | 'q'
            | 'r'
            | 's'
            | 't'
            | 'v'
            | 'w'
            | 'x'
            | 'y'
            | 'z'
    )
}

fn is_vowel_or_y(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o' | 'y')
}

fn lookup_romaji(key: &str) -> Option<&'static str> {
    match key {
        // === 3-character sequences ===
        // Hepburn standard
        "sha" => Some("しゃ"),
        "shi" => Some("し"),
        "shu" => Some("しゅ"),
        "sho" => Some("しょ"),
        "she" => Some("しぇ"),
        "chi" => Some("ち"),
        "tsu" => Some("つ"),
        "cha" => Some("ちゃ"),
        "chu" => Some("ちゅ"),
        "cho" => Some("ちょ"),
        "che" => Some("ちぇ"),
        "nya" => Some("にゃ"),
        "nyu" => Some("にゅ"),
        "nyo" => Some("にょ"),
        "hya" => Some("ひゃ"),
        "hyu" => Some("ひゅ"),
        "hyo" => Some("ひょ"),
        "mya" => Some("みゃ"),
        "myu" => Some("みゅ"),
        "myo" => Some("みょ"),
        "rya" => Some("りゃ"),
        "ryu" => Some("りゅ"),
        "ryo" => Some("りょ"),
        "gya" => Some("ぎゃ"),
        "gyu" => Some("ぎゅ"),
        "gyo" => Some("ぎょ"),
        "bya" => Some("びゃ"),
        "byu" => Some("びゅ"),
        "byo" => Some("びょ"),
        "pya" => Some("ぴゃ"),
        "pyu" => Some("ぴゅ"),
        "pyo" => Some("ぴょ"),
        "kya" => Some("きゃ"),
        "kyu" => Some("きゅ"),
        "kyo" => Some("きょ"),
        "jya" => Some("じゃ"),
        "jyu" => Some("じゅ"),
        "jyo" => Some("じょ"),
        // Nihon-shiki / Kunrei-shiki variants
        "tya" => Some("ちゃ"),
        "tyu" => Some("ちゅ"),
        "tyo" => Some("ちょ"),
        "sya" => Some("しゃ"),
        "syu" => Some("しゅ"),
        "syo" => Some("しょ"),
        "zya" => Some("じゃ"),
        "zyu" => Some("じゅ"),
        "zyo" => Some("じょ"),
        "dya" => Some("ぢゃ"),
        "dyu" => Some("ぢゅ"),
        "dyo" => Some("ぢょ"),
        // Foreign-sound kana
        "tsa" => Some("つぁ"),
        "tsi" => Some("つぃ"),
        "tse" => Some("つぇ"),
        "tso" => Some("つぉ"),

        // === 2-character sequences ===
        "ka" => Some("か"),
        "ki" => Some("き"),
        "ku" => Some("く"),
        "ke" => Some("け"),
        "ko" => Some("こ"),
        "sa" => Some("さ"),
        "si" => Some("し"),
        "su" => Some("す"),
        "se" => Some("せ"),
        "so" => Some("そ"),
        "ta" => Some("た"),
        "ti" => Some("ち"),
        "tu" => Some("つ"),
        "te" => Some("て"),
        "to" => Some("と"),
        "na" => Some("な"),
        "ni" => Some("に"),
        "nu" => Some("ぬ"),
        "ne" => Some("ね"),
        "no" => Some("の"),
        "ha" => Some("は"),
        "hi" => Some("ひ"),
        "hu" => Some("ふ"),
        "fu" => Some("ふ"),
        "he" => Some("へ"),
        "ho" => Some("ほ"),
        "fa" => Some("ふぁ"),
        "fi" => Some("ふぃ"),
        "fe" => Some("ふぇ"),
        "fo" => Some("ふぉ"),
        "ji" => Some("じ"),
        "je" => Some("じぇ"),
        "la" => Some("ら"),
        "li" => Some("り"),
        "lu" => Some("る"),
        "le" => Some("れ"),
        "lo" => Some("ろ"),
        "ma" => Some("ま"),
        "mi" => Some("み"),
        "mu" => Some("む"),
        "me" => Some("め"),
        "mo" => Some("も"),
        "ra" => Some("ら"),
        "ri" => Some("り"),
        "ru" => Some("る"),
        "re" => Some("れ"),
        "ro" => Some("ろ"),
        "ya" => Some("や"),
        "yu" => Some("ゆ"),
        "yo" => Some("よ"),
        "wa" => Some("わ"),
        "wi" => Some("ゐ"),
        "we" => Some("ゑ"),
        "wo" => Some("を"),
        "ga" => Some("が"),
        "gi" => Some("ぎ"),
        "gu" => Some("ぐ"),
        "ge" => Some("げ"),
        "go" => Some("ご"),
        "za" => Some("ざ"),
        "zi" => Some("じ"),
        "zu" => Some("ず"),
        "ze" => Some("ぜ"),
        "zo" => Some("ぞ"),
        "da" => Some("だ"),
        "di" => Some("ぢ"),
        "du" => Some("づ"),
        "de" => Some("で"),
        "do" => Some("ど"),
        "ba" => Some("ば"),
        "bi" => Some("び"),
        "bu" => Some("ぶ"),
        "be" => Some("べ"),
        "bo" => Some("ぼ"),
        "pa" => Some("ぱ"),
        "pi" => Some("ぴ"),
        "pu" => Some("ぷ"),
        "pe" => Some("ぺ"),
        "po" => Some("ぽ"),
        "ja" => Some("じゃ"),
        "ju" => Some("じゅ"),
        "jo" => Some("じょ"),

        // === 1-character sequences (vowels only; 'n' handled separately) ===
        "a" => Some("あ"),
        "i" => Some("い"),
        "u" => Some("う"),
        "e" => Some("え"),
        "o" => Some("お"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Kanji detection ===

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

    #[test]
    fn test_contains_kanji_cjk_extension_a() {
        assert!(contains_kanji("\u{3400}"));
    }

    #[test]
    fn test_contains_kanji_compatibility_ideographs() {
        assert!(contains_kanji("\u{F900}"));
    }

    // === Katakana ↔ Hiragana ===

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

    #[test]
    fn test_kata_to_hira_long_vowel_mark() {
        assert_eq!(kata_to_hira("セイバー"), "せいばー");
        assert_eq!(kata_to_hira("ー"), "ー");
    }

    #[test]
    fn test_kata_to_hira_voiced_marks() {
        assert_eq!(kata_to_hira("ガギグゲゴ"), "がぎぐげご");
        assert_eq!(kata_to_hira("ザジズゼゾ"), "ざじずぜぞ");
        assert_eq!(kata_to_hira("パピプペポ"), "ぱぴぷぺぽ");
    }

    #[test]
    fn test_kata_to_hira_vu() {
        assert_eq!(kata_to_hira("ヴ"), "ゔ");
    }

    #[test]
    fn test_hira_to_kata_basic() {
        assert_eq!(hira_to_kata("あいうえお"), "アイウエオ");
        assert_eq!(hira_to_kata("かきくけこ"), "カキクケコ");
    }

    #[test]
    fn test_hira_to_kata_long_vowel_passthrough() {
        assert_eq!(hira_to_kata("ー"), "ー");
    }

    #[test]
    fn test_hira_kata_roundtrip() {
        let original = "あいうえおかきくけこ";
        assert_eq!(kata_to_hira(&hira_to_kata(original)), original);
    }

    // === Romaji to Kana ===

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
        assert_eq!(alphabet_to_kana("kantan"), "かんたん");
        assert_eq!(alphabet_to_kana("san"), "さん");
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

    #[test]
    fn test_alphabet_to_kana_nn_before_vowel() {
        let result = alphabet_to_kana("nna");
        assert_eq!(result, "っな");
    }

    #[test]
    fn test_alphabet_to_kana_nn_at_end() {
        let result = alphabet_to_kana("nn");
        assert_eq!(result, "っん");
    }

    #[test]
    fn test_alphabet_to_kana_n_before_n_before_consonant() {
        let result = alphabet_to_kana("anna");
        assert_eq!(result, "あっな");
    }

    #[test]
    fn test_alphabet_to_kana_consecutive_vowels() {
        assert_eq!(alphabet_to_kana("aoi"), "あおい");
        assert_eq!(alphabet_to_kana("oui"), "おうい");
    }

    #[test]
    fn test_alphabet_to_kana_nihon_shiki_variants() {
        assert_eq!(alphabet_to_kana("si"), "し");
        assert_eq!(alphabet_to_kana("ti"), "ち");
        assert_eq!(alphabet_to_kana("tu"), "つ");
        assert_eq!(alphabet_to_kana("hu"), "ふ");
        assert_eq!(alphabet_to_kana("tya"), "ちゃ");
        assert_eq!(alphabet_to_kana("sya"), "しゃ");
    }

    // === Syllable boundary handling (apostrophe/punctuation fix) ===

    #[test]
    fn test_apostrophe_as_boundary_shinichi() {
        // VNDB uses apostrophe to disambiguate: Shin'ichi → しんいち (not しにち)
        // The apostrophe forces 'n' to be ん, then 'i' starts a new syllable.
        // Previously, the apostrophe was passed through into the kana output.
        assert_eq!(alphabet_to_kana("Shin'ichi"), "しんいち");
    }

    #[test]
    fn test_apostrophe_as_boundary_junichi() {
        assert_eq!(alphabet_to_kana("Jun'ichi"), "じゅんいち");
    }

    #[test]
    fn test_apostrophe_as_boundary_kenichi() {
        assert_eq!(alphabet_to_kana("Ken'ichi"), "けんいち");
    }

    #[test]
    fn test_apostrophe_as_boundary_shinichiro() {
        assert_eq!(alphabet_to_kana("Shin'ichirou"), "しんいちろう");
    }

    #[test]
    fn test_apostrophe_as_boundary_genichiro() {
        assert_eq!(alphabet_to_kana("Gen'ichirou"), "げんいちろう");
    }

    #[test]
    fn test_apostrophe_as_boundary_tenyou() {
        // ten'you → てんよう (ん + よう, not てにょう)
        assert_eq!(alphabet_to_kana("Ten'you"), "てんよう");
    }

    #[test]
    fn test_without_apostrophe_gives_different_result() {
        // Without apostrophe: Shinichi → しにち (ni syllable, not ん+い)
        assert_eq!(alphabet_to_kana("Shinichi"), "しにち");
        // With apostrophe: Shin'ichi → しんいち
        assert_eq!(alphabet_to_kana("Shin'ichi"), "しんいち");
    }

    #[test]
    fn test_hyphen_as_boundary() {
        assert_eq!(alphabet_to_kana("Sei-ichi"), "せいいち");
    }

    #[test]
    fn test_period_stripped() {
        assert_eq!(alphabet_to_kana("A.ko"), "あこ");
    }

    #[test]
    fn test_multiple_punctuation() {
        assert_eq!(alphabet_to_kana("Shin'ichi-rou"), "しんいちろう");
    }

    #[test]
    fn test_numbers_dropped() {
        // Numbers have no kana equivalent and are silently dropped
        assert_eq!(alphabet_to_kana("2B"), "b");
    }

    #[test]
    fn test_curly_apostrophe_handled() {
        // Unicode right single quotation mark (U+2019), sometimes used in data
        assert_eq!(alphabet_to_kana("Shin\u{2019}ichi"), "しんいち");
    }

    #[test]
    fn test_spaces_preserved_in_output() {
        // Spaces pass through for upstream name splitting
        assert_eq!(alphabet_to_kana("Rin Tarou"), "りん たろう");
    }

    // === Soundness: end-to-end name conversion scenarios ===

    #[test]
    fn test_common_vndb_name_okabe_rintarou() {
        assert_eq!(alphabet_to_kana("rintarou"), "りんたろう");
        assert_eq!(alphabet_to_kana("okabe"), "おかべ");
    }

    #[test]
    fn test_common_vndb_name_makise_kurisu() {
        assert_eq!(alphabet_to_kana("kurisu"), "くりす");
        assert_eq!(alphabet_to_kana("makise"), "まきせ");
    }

    #[test]
    fn test_long_vowel_ou_pattern() {
        assert_eq!(alphabet_to_kana("yuuko"), "ゆうこ");
        assert_eq!(alphabet_to_kana("shouichi"), "しょういち");
    }

    #[test]
    fn test_double_consonant_in_names() {
        assert_eq!(alphabet_to_kana("kappei"), "かっぺい");
        assert_eq!(alphabet_to_kana("seppuku"), "せっぷく");
    }

    #[test]
    fn test_n_disambiguation_with_and_without_apostrophe() {
        // The apostrophe is the ONLY way to distinguish ん+vowel from な-row.
        // This is by design in Hepburn romanization.
        assert_eq!(alphabet_to_kana("kana"), "かな"); // ka + na
        assert_eq!(alphabet_to_kana("kan'a"), "かんあ"); // ka + n + a
        assert_eq!(alphabet_to_kana("kantan"), "かんたん"); // n before consonant → ん
    }

    // === New romaji entries: ji, la/li/lu/le/lo ===

    #[test]
    fn test_ji_conversion() {
        assert_eq!(alphabet_to_kana("ji"), "じ");
        assert_eq!(alphabet_to_kana("jima"), "じま");
        assert_eq!(alphabet_to_kana("doujima"), "どうじま");
        assert_eq!(alphabet_to_kana("shouji"), "しょうじ");
    }

    #[test]
    fn test_la_li_lu_le_lo_conversion() {
        assert_eq!(alphabet_to_kana("la"), "ら");
        assert_eq!(alphabet_to_kana("li"), "り");
        assert_eq!(alphabet_to_kana("lu"), "る");
        assert_eq!(alphabet_to_kana("le"), "れ");
        assert_eq!(alphabet_to_kana("lo"), "ろ");
        assert_eq!(alphabet_to_kana("lain"), "らいん");
    }
}
