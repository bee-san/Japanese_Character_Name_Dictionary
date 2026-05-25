use crate::kana;
use unicode_normalization::UnicodeNormalization;

use super::model::ScriptKind;

#[derive(Debug, Clone)]
pub struct NormalizedText {
    pub raw: String,
    pub normalized: String,
    pub script: ScriptKind,
}

pub fn normalize_name(value: &str) -> NormalizedText {
    let raw = value.trim().to_string();
    let normalized = harmonize_case_and_script(&fold_punctuation(&collapse_whitespace(
        &raw.nfkc().collect::<String>(),
    )));
    let script = detect_script(&raw);
    NormalizedText {
        raw,
        normalized,
        script,
    }
}

pub fn detect_script(value: &str) -> ScriptKind {
    let mut has_kanji = false;
    let mut has_hira = false;
    let mut has_kata = false;
    let mut has_latin = false;
    let mut has_other = false;

    for ch in value.chars().filter(|c| !c.is_whitespace()) {
        let code = ch as u32;
        if kana::contains_kanji(&ch.to_string()) {
            has_kanji = true;
        } else if (0x3040..=0x309F).contains(&code) {
            has_hira = true;
        } else if (0x30A0..=0x30FF).contains(&code) {
            has_kata = true;
        } else if ch.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&code) {
            has_latin = true;
        } else if ch.is_ascii_digit() {
            continue;
        } else {
            has_other = true;
        }
    }

    match (has_kanji, has_hira, has_kata, has_latin, has_other) {
        (false, false, false, false, false) => ScriptKind::Unknown,
        (true, false, false, false, false) => ScriptKind::Kanji,
        (false, true, false, false, false) => ScriptKind::Hiragana,
        (false, false, true, false, false) => ScriptKind::Katakana,
        (true, true, false, false, false)
        | (true, false, true, false, false)
        | (false, true, true, false, false)
        | (true, true, true, false, false) => ScriptKind::MixedJapanese,
        (false, false, false, true, false) => ScriptKind::Latin,
        (false, false, false, _, true) => ScriptKind::Other,
        _ => ScriptKind::Mixed,
    }
}

pub fn normalize_reading(value: &str) -> NormalizedText {
    let base = normalize_name(value);
    let normalized = kana::kata_to_hira(&base.normalized);
    NormalizedText {
        raw: base.raw,
        normalized,
        script: detect_script(value),
    }
}

pub fn derive_romaji_from_reading(reading: &str) -> Option<String> {
    let hira = kana::kata_to_hira(reading);
    if !hira
        .chars()
        .all(|c| c.is_whitespace() || (0x3040..=0x309F).contains(&(c as u32)) || c == 'ー')
    {
        return None;
    }

    let chars: Vec<char> = hira.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            out.push(' ');
            i += 1;
            continue;
        }

        if i + 1 < chars.len() {
            let pair: String = [chars[i], chars[i + 1]].iter().collect();
            if let Some(romaji) = digraph_romaji(&pair) {
                out.push_str(romaji);
                i += 2;
                continue;
            }
        }

        if chars[i] == 'っ' {
            if let Some(next) = chars.get(i + 1).copied() {
                let next_romaji = if i + 2 < chars.len() {
                    let pair: String = [next, chars[i + 2]].iter().collect();
                    digraph_romaji(&pair).or_else(|| single_romaji(next))
                } else {
                    single_romaji(next)
                };
                if let Some(next_romaji) = next_romaji {
                    if let Some(first) = next_romaji.chars().next() {
                        out.push(first);
                    }
                }
            }
            i += 1;
            continue;
        }

        if chars[i] == 'ー' {
            if let Some(last_vowel) = out.chars().rev().find(|c| "aeiou".contains(*c)) {
                out.push(last_vowel);
            }
            i += 1;
            continue;
        }

        if let Some(romaji) = single_romaji(chars[i]) {
            out.push_str(romaji);
        } else {
            return None;
        }
        i += 1;
    }

    Some(collapse_whitespace(&out).trim().to_string())
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fold_punctuation(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '　' => ' ',
            '・' | '･' => '·',
            '‐' | '‑' | '–' | '—' | '―' | 'ー' if !kana::contains_japanese(input) => '-',
            '’' | '‘' | '´' | '`' => '\'',
            '“' | '”' => '"',
            other => other,
        })
        .collect()
}

fn harmonize_case_and_script(input: &str) -> String {
    let lowered = input.to_lowercase();
    kana::kata_to_hira(&lowered)
}

fn digraph_romaji(input: &str) -> Option<&'static str> {
    match input {
        "きゃ" => Some("kya"),
        "きゅ" => Some("kyu"),
        "きょ" => Some("kyo"),
        "ぎゃ" => Some("gya"),
        "ぎゅ" => Some("gyu"),
        "ぎょ" => Some("gyo"),
        "しゃ" => Some("sha"),
        "しゅ" => Some("shu"),
        "しょ" => Some("sho"),
        "じゃ" => Some("ja"),
        "じゅ" => Some("ju"),
        "じょ" => Some("jo"),
        "ちゃ" => Some("cha"),
        "ちゅ" => Some("chu"),
        "ちょ" => Some("cho"),
        "にゃ" => Some("nya"),
        "にゅ" => Some("nyu"),
        "にょ" => Some("nyo"),
        "ひゃ" => Some("hya"),
        "ひゅ" => Some("hyu"),
        "ひょ" => Some("hyo"),
        "びゃ" => Some("bya"),
        "びゅ" => Some("byu"),
        "びょ" => Some("byo"),
        "ぴゃ" => Some("pya"),
        "ぴゅ" => Some("pyu"),
        "ぴょ" => Some("pyo"),
        "みゃ" => Some("mya"),
        "みゅ" => Some("myu"),
        "みょ" => Some("myo"),
        "りゃ" => Some("rya"),
        "りゅ" => Some("ryu"),
        "りょ" => Some("ryo"),
        _ => None,
    }
}

fn single_romaji(ch: char) -> Option<&'static str> {
    match ch {
        'あ' => Some("a"),
        'い' => Some("i"),
        'う' => Some("u"),
        'え' => Some("e"),
        'お' => Some("o"),
        'か' => Some("ka"),
        'き' => Some("ki"),
        'く' => Some("ku"),
        'け' => Some("ke"),
        'こ' => Some("ko"),
        'さ' => Some("sa"),
        'し' => Some("shi"),
        'す' => Some("su"),
        'せ' => Some("se"),
        'そ' => Some("so"),
        'た' => Some("ta"),
        'ち' => Some("chi"),
        'つ' => Some("tsu"),
        'て' => Some("te"),
        'と' => Some("to"),
        'な' => Some("na"),
        'に' => Some("ni"),
        'ぬ' => Some("nu"),
        'ね' => Some("ne"),
        'の' => Some("no"),
        'は' => Some("ha"),
        'ひ' => Some("hi"),
        'ふ' => Some("fu"),
        'へ' => Some("he"),
        'ほ' => Some("ho"),
        'ま' => Some("ma"),
        'み' => Some("mi"),
        'む' => Some("mu"),
        'め' => Some("me"),
        'も' => Some("mo"),
        'や' => Some("ya"),
        'ゆ' => Some("yu"),
        'よ' => Some("yo"),
        'ら' => Some("ra"),
        'り' => Some("ri"),
        'る' => Some("ru"),
        'れ' => Some("re"),
        'ろ' => Some("ro"),
        'わ' => Some("wa"),
        'を' => Some("wo"),
        'ん' => Some("n"),
        'が' => Some("ga"),
        'ぎ' => Some("gi"),
        'ぐ' => Some("gu"),
        'げ' => Some("ge"),
        'ご' => Some("go"),
        'ざ' => Some("za"),
        'じ' => Some("ji"),
        'ず' => Some("zu"),
        'ぜ' => Some("ze"),
        'ぞ' => Some("zo"),
        'だ' => Some("da"),
        'ぢ' => Some("ji"),
        'づ' => Some("zu"),
        'で' => Some("de"),
        'ど' => Some("do"),
        'ば' => Some("ba"),
        'び' => Some("bi"),
        'ぶ' => Some("bu"),
        'べ' => Some("be"),
        'ぼ' => Some("bo"),
        'ぱ' => Some("pa"),
        'ぴ' => Some("pi"),
        'ぷ' => Some("pu"),
        'ぺ' => Some("pe"),
        'ぽ' => Some("po"),
        'ぁ' => Some("a"),
        'ぃ' => Some("i"),
        'ぅ' => Some("u"),
        'ぇ' => Some("e"),
        'ぉ' => Some("o"),
        'ゔ' => Some("vu"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_nfkc_and_kana() {
        let normalized = normalize_name("ＡＢＣ・カタカナ");
        assert_eq!(normalized.normalized, "abc·かたかな");
        assert_eq!(normalized.script, ScriptKind::Mixed);
    }

    #[test]
    fn derives_romaji_from_hiragana() {
        assert_eq!(
            derive_romaji_from_reading("おかべ りんたろう").as_deref(),
            Some("okabe rintarou")
        );
    }
}
