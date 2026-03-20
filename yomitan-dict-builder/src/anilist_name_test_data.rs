/// Test suite for AniList name handling, generated from anilist_characters-1.jsonl.
///
/// Tests the unified name resolution API that handles both VNDB and AniList characters:
/// - Accepts optional first/last name hints (from AniList)
/// - Uses native name directly when available
/// - Falls back to romaji→kana when native is missing
/// - Splits native names into family/given using hints when no space exists
/// - Produces correct readings for both VNDB and AniList characters

#[cfg(test)]
mod tests {
    use crate::kana;
    use crate::name_parser;

    // =========================================================================
    // Backward compatibility: VNDB-style (no hints)
    // =========================================================================

    #[test]
    fn test_split_with_space_no_hints() {
        let parts = name_parser::split_japanese_name_with_hints("千俵 おりえ", None, None);
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("千俵"));
        assert_eq!(parts.given.as_deref(), Some("おりえ"));
    }

    #[test]
    fn test_single_name_no_hints() {
        let parts = name_parser::split_japanese_name_with_hints("徳蔵", None, None);
        assert!(!parts.has_space);
        assert_eq!(parts.family, None);
        assert_eq!(parts.given, None);
    }

    #[test]
    fn test_katakana_middledot_no_hints() {
        let parts = name_parser::split_japanese_name_with_hints("ローランド・シャペル", None, None);
        assert!(!parts.has_space);
    }

    #[test]
    fn test_readings_pure_kana_single_no_hints() {
        let readings = name_parser::generate_name_readings("ヒミコ", "Himiko", None, None);
        assert_eq!(readings.full, "ひみこ");
    }

    #[test]
    fn test_readings_pure_hiragana_single_no_hints() {
        let readings = name_parser::generate_name_readings("みほ", "Miho", None, None);
        assert_eq!(readings.full, "みほ");
    }

    #[test]
    fn test_readings_kanji_with_space_no_hints() {
        let readings =
            name_parser::generate_name_readings("蔵木 滋乃", "Shigeno Kuraki", None, None);
        assert_eq!(readings.family, kana::alphabet_to_kana("Shigeno"));
        assert_eq!(readings.given, kana::alphabet_to_kana("Kuraki"));
    }

    #[test]
    fn test_bulk_no_panics_no_hints() {
        let cases: Vec<(&str, &str)> = vec![
            ("幸平創真", "Souma Yukihira"),
            ("薙切えりな", "Erina Nakiri"),
            ("田所恵", "Megumi Tadokoro"),
            ("ローランド・シャペル", "Roland Chapelle"),
            ("ごうだばやしきよし", "Kiyoshi Goudabayashi"),
            ("タクミ・アルディーニ", "Takumi Aldini"),
            ("ヒミコ", "Himiko"),
            ("みほ", "Miho"),
            ("ユキ", "Yuki"),
            ("鷹嘴", "Takanohashi"),
            ("田所の母", "Tadokoro no Haha"),
            ("岩倉玲音", "Lain Iwakura"),
            ("坂本竜太", "Ryouta Sakamoto"),
        ];
        for (native, romaji) in &cases {
            let readings = name_parser::generate_name_readings(native, romaji, None, None);
            assert!(
                !readings.full.is_empty(),
                "Failed for {} / {}",
                native,
                romaji
            );
        }
    }

    // =========================================================================
    // AniList hint-based reading generation
    // =========================================================================

    // --- Two-part kanji names without space, WITH hints ---

    #[test]
    fn test_souma_yukihira_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "幸平創真",
            "Souma Yukihira",
            Some("Souma"),
            Some("Yukihira"),
        );
        assert_eq!(
            readings.family, "ゆきひら",
            "Family should be Yukihira→ゆきひら"
        );
        assert_eq!(readings.given, "そうま", "Given should be Souma→そうま");
        assert_eq!(readings.full, "ゆきひらそうま", "Full = family + given");
    }

    #[test]
    fn test_erina_nakiri_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "薙切えりな",
            "Erina Nakiri",
            Some("Erina"),
            Some("Nakiri"),
        );
        assert_eq!(readings.family, "なきり");
        assert_eq!(readings.given, "えりな");
        assert_eq!(readings.full, "なきりえりな");
    }

    #[test]
    fn test_megumi_tadokoro_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "田所恵",
            "Megumi Tadokoro",
            Some("Megumi"),
            Some("Tadokoro"),
        );
        assert_eq!(readings.family, "たどころ");
        assert_eq!(readings.given, "めぐみ");
        assert_eq!(readings.full, "たどころめぐみ");
    }

    #[test]
    fn test_lain_iwakura_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "岩倉玲音",
            "Lain Iwakura",
            Some("Lain"),
            Some("Iwakura"),
        );
        assert_eq!(readings.family, "いわくら");
        assert_eq!(readings.given, "らいん");
        assert_eq!(readings.full, "いわくららいん");
    }

    #[test]
    fn test_jouichirou_yukihira_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "幸平城一郎",
            "Jouichirou Yukihira",
            Some("Jouichirou"),
            Some("Yukihira"),
        );
        assert_eq!(readings.family, "ゆきひら");
        assert_eq!(readings.given, "じょういちろう");
        assert_eq!(readings.full, "ゆきひらじょういちろう");
    }

    #[test]
    fn test_gin_doujima_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "堂島銀",
            "Gin Doujima",
            Some("Gin"),
            Some("Doujima"),
        );
        assert_eq!(readings.family, "どうじま");
        assert_eq!(readings.given, "ぎん");
        assert_eq!(readings.full, "どうじまぎん");
    }

    #[test]
    fn test_ryouta_sakamoto_split_with_hints() {
        let readings = name_parser::generate_name_readings(
            "坂本竜太",
            "Ryouta Sakamoto",
            Some("Ryouta"),
            Some("Sakamoto"),
        );
        assert_eq!(readings.family, "さかもと");
        assert_eq!(readings.given, "りょうた");
        assert_eq!(readings.full, "さかもとりょうた");
    }

    // --- Mixed kana/kanji names without space, WITH hints ---

    #[test]
    fn test_alice_nakiri_katakana_given() {
        let readings = name_parser::generate_name_readings(
            "薙切アリス",
            "Alice Nakiri",
            Some("Alice"),
            Some("Nakiri"),
        );
        assert_eq!(readings.family, "なきり");
        assert_eq!(readings.given, "ありす");
        assert_eq!(readings.full, "なきりありす");
    }

    #[test]
    fn test_kurokiba_ryou_katakana_given() {
        let readings = name_parser::generate_name_readings(
            "黒木場リョウ",
            "Ryou Kurokiba",
            Some("Ryou"),
            Some("Kurokiba"),
        );
        assert_eq!(readings.family, "くろきば");
        assert_eq!(readings.given, "りょう");
        assert_eq!(readings.full, "くろきばりょう");
    }

    #[test]
    fn test_sadatsuka_nao_katakana_given() {
        let readings = name_parser::generate_name_readings(
            "貞塚ナオ",
            "Nao Sadatsuka",
            Some("Nao"),
            Some("Sadatsuka"),
        );
        assert_eq!(readings.family, "さだつか");
        assert_eq!(readings.given, "なお");
        assert_eq!(readings.full, "さだつかなお");
    }

    #[test]
    fn test_hayama_akira_katakana_given() {
        let readings = name_parser::generate_name_readings(
            "葉山アキラ",
            "Akira Hayama",
            Some("Akira"),
            Some("Hayama"),
        );
        assert_eq!(readings.family, "はやま");
        assert_eq!(readings.given, "あきら");
        assert_eq!(readings.full, "はやまあきら");
    }

    #[test]
    fn test_nakamozu_kinu_hiragana_given() {
        let readings = name_parser::generate_name_readings(
            "中百舌鳥きぬ",
            "Kinu Nakamozu",
            Some("Kinu"),
            Some("Nakamozu"),
        );
        assert_eq!(readings.family, "なかもず");
        assert_eq!(readings.given, "きぬ");
        assert_eq!(readings.full, "なかもずきぬ");
    }

    #[test]
    fn test_sendawara_natsume_hiragana_given() {
        let readings = name_parser::generate_name_readings(
            "千俵なつめ",
            "Natsume Sendawara",
            Some("Natsume"),
            Some("Sendawara"),
        );
        assert_eq!(readings.family, "せんだわら");
        assert_eq!(readings.given, "なつめ");
        assert_eq!(readings.full, "せんだわらなつめ");
    }

    #[test]
    fn test_daimidou_fumio_mixed_given() {
        let readings = name_parser::generate_name_readings(
            "大御堂ふみ緒",
            "Fumio Daimidou",
            Some("Fumio"),
            Some("Daimidou"),
        );
        assert_eq!(readings.family, "だいみどう");
        assert_eq!(readings.given, "ふみお");
        assert_eq!(readings.full, "だいみどうふみお");
    }

    // --- Native name already has space — hints used for readings ---

    #[test]
    fn test_native_with_space_uses_hints_for_readings() {
        let readings = name_parser::generate_name_readings(
            "千俵 おりえ",
            "Orie Sendawara",
            Some("Orie"),
            Some("Sendawara"),
        );
        assert_eq!(readings.family, "せんだわら");
        assert_eq!(readings.given, "おりえ");
        assert_eq!(readings.full, "せんだわらおりえ");
    }

    #[test]
    fn test_kuraki_shigeno_with_space() {
        let readings = name_parser::generate_name_readings(
            "蔵木 滋乃",
            "Shigeno Kuraki",
            Some("Shigeno"),
            Some("Kuraki"),
        );
        assert_eq!(readings.family, "くらき");
        assert_eq!(readings.given, "しげの");
        assert_eq!(readings.full, "くらきしげの");
    }

    // --- Single-name characters (no last name) ---

    #[test]
    fn test_single_name_tokuzou() {
        let readings =
            name_parser::generate_name_readings("徳蔵", "Tokuzou", Some("Tokuzou"), None);
        assert_eq!(readings.full, "とくぞう");
        assert_eq!(readings.family, "とくぞう");
        assert_eq!(readings.given, "とくぞう");
    }

    #[test]
    fn test_single_name_himiko_katakana() {
        let readings =
            name_parser::generate_name_readings("ヒミコ", "Himiko", Some("Himiko"), None);
        assert_eq!(readings.full, "ひみこ");
    }

    #[test]
    fn test_single_name_miho_hiragana() {
        let readings = name_parser::generate_name_readings("みほ", "Miho", Some("Miho"), None);
        assert_eq!(readings.full, "みほ");
    }

    // --- Katakana foreign names with middle dot ---

    #[test]
    fn test_katakana_middledot_roland() {
        let readings = name_parser::generate_name_readings(
            "ローランド・シャペル",
            "Roland Chapelle",
            Some("Roland"),
            Some("Chapelle"),
        );
        assert_eq!(readings.full, "ろーらんど・しゃぺる");
    }

    #[test]
    fn test_katakana_middledot_takumi_aldini() {
        let readings = name_parser::generate_name_readings(
            "タクミ・アルディーニ",
            "Takumi Aldini",
            Some("Takumi"),
            Some("Aldini"),
        );
        assert_eq!(readings.full, "たくみ・あるでぃーに");
    }

    // --- Empty native fallback ---

    #[test]
    fn test_null_native_returns_empty() {
        let readings =
            name_parser::generate_name_readings("", "Lin Sui-Xi", Some("Lin"), Some("Sui-Xi"));
        assert!(readings.full.is_empty());
    }

    // --- No hints (VNDB path) — backward compatible ---

    #[test]
    fn test_no_hints_vndb_with_space() {
        let readings =
            name_parser::generate_name_readings("須々木 心一", "Shinichi Suzuki", None, None);
        assert_eq!(readings.family, "しにち");
        assert_eq!(readings.given, "すずき");
    }

    #[test]
    fn test_no_hints_single_katakana() {
        let readings = name_parser::generate_name_readings("セイバー", "Saber", None, None);
        assert_eq!(readings.full, "せいばー");
    }

    #[test]
    fn test_no_hints_no_space_kanji() {
        let readings =
            name_parser::generate_name_readings("幸平創真", "Souma Yukihira", None, None);
        // Whitespace is stripped from readings — romaji spaces should not appear in kana
        assert_eq!(readings.full, "そうまゆきひら");
        assert_eq!(readings.family, "そうまゆきひら");
        assert_eq!(readings.given, "そうまゆきひら");
    }

    // --- Split function with hints ---

    #[test]
    fn test_split_with_hints_souma_yukihira() {
        let parts = name_parser::split_japanese_name_with_hints(
            "幸平創真",
            Some("Souma"),
            Some("Yukihira"),
        );
        assert!(
            parts.has_space || parts.family.is_some(),
            "Should produce family/given parts even without space"
        );
        assert!(parts.family.is_some(), "Should have family part");
        assert!(parts.given.is_some(), "Should have given part");
        assert_eq!(parts.combined, "幸平創真");
    }

    #[test]
    fn test_split_with_hints_native_has_space() {
        let parts = name_parser::split_japanese_name_with_hints(
            "千俵 おりえ",
            Some("Orie"),
            Some("Sendawara"),
        );
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("千俵"));
        assert_eq!(parts.given.as_deref(), Some("おりえ"));
    }

    #[test]
    fn test_split_with_hints_single_name() {
        let parts = name_parser::split_japanese_name_with_hints("徳蔵", Some("Tokuzou"), None);
        assert!(!parts.has_space);
        assert_eq!(parts.family, None);
        assert_eq!(parts.given, None);
    }

    #[test]
    fn test_split_no_hints_falls_back() {
        let parts = name_parser::split_japanese_name_with_hints("須々木 心一", None, None);
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("須々木"));
        assert_eq!(parts.given.as_deref(), Some("心一"));
    }

    #[test]
    fn test_split_with_hints_katakana_middledot() {
        let parts = name_parser::split_japanese_name_with_hints(
            "ローランド・シャペル",
            Some("Roland"),
            Some("Chapelle"),
        );
        assert!(!parts.has_space);
    }

    #[test]
    fn test_split_with_hints_empty_last() {
        let parts = name_parser::split_japanese_name_with_hints(
            "田所の母",
            Some("Tadokoro no Haha"),
            Some(""),
        );
        assert!(!parts.has_space);
    }

    // --- Iteration mark (々) handling ---
    //
    // 々 (U+3005, IDEOGRAPHIC ITERATION MARK) is not classified as kanji by
    // `is_kanji` (it falls outside the CJK Unified Ideographs range 0x4E00–
    // 0x9FFF). The split heuristics use kanji→kana transitions to find name
    // boundaries, so 々 can be misidentified as a kana boundary and end up as
    // an isolated `given` part. When `contains_kanji("々")` returns false,
    // `kata_to_hira("々")` is called which passes the iteration mark through
    // unchanged — producing a literal 々 in the reading instead of the correct
    // kana.  These tests guard against that regression.

    #[test]
    fn test_nene_iteration_mark_given_name_only() {
        // 寧々 (Nene) — 々 repeats 寧, so the full name is read as ねね.
        // With no last-name hint this takes the single-name path; the romaji
        // hint must be used without 々 leaking into the output.
        let readings = name_parser::generate_name_readings("寧々", "Nene", Some("Nene"), None);
        assert_eq!(readings.full, "ねね", "寧々 should read as ねね");
        assert!(
            !readings.full.contains('々'),
            "Reading must not contain the raw iteration mark 々"
        );
    }

    #[test]
    fn test_nene_iteration_mark_both_hints_triggers_split() {
        // This is the exact failure mode from the bug report: some databases
        // (or malformed AniList entries) supply a last-name hint even for a
        // single-name character, causing split_japanese_name_with_hints to
        // attempt a split of 寧々. Strategy 2 scores split_pos=1 as the best
        // candidate (family="寧", given="々"). Because contains_kanji("々") is
        // false (U+3005 is outside all CJK ranges), kata_to_hira("々") returns
        // "々" unchanged — the iteration mark leaks directly into the reading.
        //
        // Fix: is_kanji() must return true for U+3005 so that 々 is never
        // treated as a kana character in either the boundary heuristic or the
        // reading-generation fallback.
        let readings = name_parser::generate_name_readings(
            "寧々",
            "Nene",
            Some("Nene"),
            Some("Nene"), // same value forced as last — triggers the split path
        );
        assert!(
            !readings.full.contains('々'),
            "Reading must not contain the raw iteration mark 々 — got: '{}'",
            readings.full
        );
        assert!(!readings.full.is_empty());
    }

    #[test]
    fn test_nene_iteration_mark_with_family_name() {
        // 田中寧々 — family 田中 (Tanaka) + given 寧々 (Nene).
        // Strategy 1 sees 々 as a non-kanji char and might set a boundary at
        // position 3 (after 田中寧), leaving given = "々". The split scoring
        // should instead prefer the boundary at position 2 so that given =
        // "寧々" which contains_kanji → true and uses the hint reading ねね.
        let readings = name_parser::generate_name_readings(
            "田中寧々",
            "Nene Tanaka",
            Some("Nene"),
            Some("Tanaka"),
        );
        assert!(
            !readings.full.contains('々'),
            "Reading of 田中寧々 must not contain the raw iteration mark 々"
        );
        assert!(
            !readings.family.contains('々'),
            "Family reading must not contain 々"
        );
        assert!(
            !readings.given.contains('々'),
            "Given reading must not contain 々"
        );
        assert!(!readings.full.is_empty());
    }

    #[test]
    fn test_ririko_iteration_mark_in_name() {
        // 莉々子 (Ririko) — 々 repeats 莉, so the name is 莉莉子 phonetically.
        // Single given-name; the iteration mark must not appear in the reading.
        let readings =
            name_parser::generate_name_readings("莉々子", "Ririko", Some("Ririko"), None);
        assert!(
            !readings.full.contains('々'),
            "Reading of 莉々子 must not contain the raw iteration mark 々"
        );
        assert!(
            !readings.full.is_empty(),
            "Reading of 莉々子 must be non-empty"
        );
    }

    #[test]
    fn test_iteration_mark_in_family_name_with_space() {
        // 須々木 心一 — 々 already covered by split_no_hints test, but verify
        // that hints-based reading generation also keeps 々 out of the output.
        let readings = name_parser::generate_name_readings(
            "須々木 心一",
            "Shinichi Suzuki",
            Some("Shinichi"),
            Some("Suzuki"),
        );
        assert!(
            !readings.full.contains('々'),
            "Reading of 須々木 心一 must not contain the raw iteration mark 々"
        );
        assert!(!readings.full.is_empty());
    }

    // --- Whitespace handling ---

    #[test]
    fn test_trims_hint_whitespace() {
        let readings = name_parser::generate_name_readings(
            "佐藤昭二",
            "Shouji Satou",
            Some("Shouji "),
            Some("Satou "),
        );
        assert_eq!(readings.family, "さとう");
        assert_eq!(readings.given, "しょうじ");
    }

    // --- Bulk: all JSONL characters through the unified API ---

    // --- Large-scale test: Popular anime and VN characters from multiple franchises ---
    //
    // This test verifies parsing of character names from:
    // - Jujutsu Kaisen
    // - Demon Slayer
    // - Steins;Gate
    // - Code Geass
    // - Attack on Titan
    // - My Hero Academia
    // - Fate series
    // - Original anime and more
    //
    // Covers diverse patterns: kanji names, katakana names, mixed kana/kanji,
    // single names, family names with complex kanji, etc.

    #[test]
    fn test_popular_anime_vn_characters() {
        struct Case {
            native: Option<&'static str>,
            full: &'static str,
            first: &'static str,
            last: Option<&'static str>,
        }

        let cases = vec![
            // === Jujutsu Kaisen ===
            Case {
                native: Some("虎杖悠仁"),
                full: "Yuji Itadori",
                first: "Yuji",
                last: Some("Itadori"),
            },
            Case {
                native: Some("伏黒恵"),
                full: "Megumi Fushiguro",
                first: "Megumi",
                last: Some("Fushiguro"),
            },
            Case {
                native: Some("釘崎野薔薇"),
                full: "Nobara Kugisaki",
                first: "Nobara",
                last: Some("Kugisaki"),
            },
            Case {
                native: Some("両面宿儺"),
                full: "Sukuna Ryomen",
                first: "Sukuna",
                last: Some("Ryomen"),
            },
            Case {
                native: Some("狗巻棘"),
                full: "Maki Zenin",
                first: "Maki",
                last: Some("Zenin"),
            },
            Case {
                native: Some("禅院真希"),
                full: "Maki Zenin",
                first: "Maki",
                last: Some("Zenin"),
            },
            // === Demon Slayer ===
            Case {
                native: Some("竈門炭治郎"),
                full: "Tanjiro Kamado",
                first: "Tanjiro",
                last: Some("Kamado"),
            },
            Case {
                native: Some("竈門禰豆子"),
                full: "Nezuko Kamado",
                first: "Nezuko",
                last: Some("Kamado"),
            },
            Case {
                native: Some("我妻善逸"),
                full: "Zenitsu Agatsuma",
                first: "Zenitsu",
                last: Some("Agatsuma"),
            },
            Case {
                native: Some("嘴平伊之助"),
                full: "Inosuke Hashibira",
                first: "Inosuke",
                last: Some("Hashibira"),
            },
            Case {
                native: Some("鬼舞辻無惨"),
                full: "Muzan Kibutsuji",
                first: "Muzan",
                last: Some("Kibutsuji"),
            },
            Case {
                native: Some("胡蝶しのぶ"),
                full: "Shinobu Kochou",
                first: "Shinobu",
                last: Some("Kochou"),
            },
            Case {
                native: Some("栗花落カナヲ"),
                full: "Kanao Tsuyuri",
                first: "Kanao",
                last: Some("Tsuyuri"),
            },
            // === Steins;Gate ===
            Case {
                native: Some("岡部倫太郎"),
                full: "Rintaro Okabe",
                first: "Rintaro",
                last: Some("Okabe"),
            },
            Case {
                native: Some("牧瀬紅莉栖"),
                full: "Kurisu Makise",
                first: "Kurisu",
                last: Some("Makise"),
            },
            Case {
                native: Some("橋田至"),
                full: "Itaru Hashida",
                first: "Itaru",
                last: Some("Hashida"),
            },
            Case {
                native: Some("椎名まゆり"),
                full: "Mayuri Shiina",
                first: "Mayuri",
                last: Some("Shiina"),
            },
            Case {
                native: Some("鈴羽"),
                full: "Suzuha Amane",
                first: "Suzuha",
                last: Some("Amane"),
            },
            // === Code Geass ===
            Case {
                native: Some("ルルーシュ・ヴィ・ブリタニア"),
                full: "Lelouch vi Britannia",
                first: "Lelouch",
                last: Some("Britannia"),
            },
            Case {
                native: Some("枢木スザク"),
                full: "Suzaku Kururugi",
                first: "Suzaku",
                last: Some("Kururugi"),
            },
            Case {
                native: Some("紅月カレン"),
                full: "Kallen Kozuki",
                first: "Kallen",
                last: Some("Kozuki"),
            },
            Case {
                native: Some("藤堂鏡志郎"),
                full: "Kyoshiro Tohdoh",
                first: "Kyoshiro",
                last: Some("Tohdoh"),
            },
            // === Attack on Titan ===
            Case {
                native: Some("エレン・イェーガー"),
                full: "Eren Yeager",
                first: "Eren",
                last: Some("Yeager"),
            },
            Case {
                native: Some("ミカサ・アッカーマン"),
                full: "Mikasa Ackerman",
                first: "Mikasa",
                last: Some("Ackerman"),
            },
            Case {
                native: Some("アルミン・アルレルト"),
                full: "Armin Arlert",
                first: "Armin",
                last: Some("Arlert"),
            },
            Case {
                native: Some("リヴァイ・アッカーマン"),
                full: "Levi Ackerman",
                first: "Levi",
                last: Some("Ackerman"),
            },
            // === My Hero Academia ===
            Case {
                native: Some("緑谷出久"),
                full: "Midoriya Deku",
                first: "Midoriya",
                last: Some("Deku"),
            },
            Case {
                native: Some("爆豪勝己"),
                full: "Bakugo Katsuki",
                first: "Bakugo",
                last: Some("Katsuki"),
            },
            Case {
                native: Some("麗日お茶子"),
                full: "Uraraka Ochako",
                first: "Uraraka",
                last: Some("Ochako"),
            },
            Case {
                native: Some("飯田天哉"),
                full: "Iida Tenya",
                first: "Iida",
                last: Some("Tenya"),
            },
            Case {
                native: Some("轟焦凍"),
                full: "Todoroki Shoto",
                first: "Todoroki",
                last: Some("Shoto"),
            },
            // === Haikyu!! ===
            Case {
                native: Some("烏野日向翔陽"),
                full: "Shoyo Hinata",
                first: "Shoyo",
                last: Some("Hinata"),
            },
            Case {
                native: Some("宮侑"),
                full: "Issei Miyagi",
                first: "Issei",
                last: Some("Miyagi"),
            },
            Case {
                native: Some("及川徹"),
                full: "Tooru Oikawa",
                first: "Tooru",
                last: Some("Oikawa"),
            },
            // === Fate Series ===
            Case {
                native: Some("衛宮士郎"),
                full: "Shirou Emiya",
                first: "Shirou",
                last: Some("Emiya"),
            },
            Case {
                native: Some("遠坂凛"),
                full: "Rin Tohsaka",
                first: "Rin",
                last: Some("Tohsaka"),
            },
            Case {
                native: Some("間桐慎二"),
                full: "Shinji Matou",
                first: "Shinji",
                last: Some("Matou"),
            },
            // === Solo Leveling ===
            Case {
                native: Some("キムチホ"),
                full: "Jinwoo Kim",
                first: "Jinwoo",
                last: Some("Kim"),
            },
            // === Assassination Classroom ===
            Case {
                native: Some("渚"),
                full: "Nagisa Shiota",
                first: "Nagisa",
                last: Some("Shiota"),
            },
            Case {
                native: Some("烏間惟臣"),
                full: "Tadaomi Karasuma",
                first: "Tadaomi",
                last: Some("Karasuma"),
            },
            // === Sword Art Online ===
            Case {
                native: Some("キリト"),
                full: "Kazuto Kirigaya",
                first: "Kazuto",
                last: Some("Kirigaya"),
            },
            Case {
                native: Some("アスナ"),
                full: "Asuna Yuuki",
                first: "Asuna",
                last: Some("Yuuki"),
            },
            // === Tokyo Ghoul ===
            Case {
                native: Some("金木研"),
                full: "Kaneki Ken",
                first: "Kaneki",
                last: Some("Ken"),
            },
            Case {
                native: Some("霧島董香"),
                full: "Touka Kirishima",
                first: "Touka",
                last: Some("Kirishima"),
            },
            // === Death Note ===
            Case {
                native: Some("夜神月"),
                full: "Light Yagami",
                first: "Light",
                last: Some("Yagami"),
            },
            Case {
                native: Some("L・ローライト"),
                full: "L Lawliet",
                first: "L",
                last: Some("Lawliet"),
            },
            // === Fullmetal Alchemist ===
            Case {
                native: Some("エドワード・エルリック"),
                full: "Edward Elric",
                first: "Edward",
                last: Some("Elric"),
            },
            Case {
                native: Some("アルフォンス・エルリック"),
                full: "Alphonse Elric",
                first: "Alphonse",
                last: Some("Elric"),
            },
            // === Mob Psycho 100 ===
            Case {
                native: Some("モブ"),
                full: "Mob Kageyama",
                first: "Mob",
                last: Some("Kageyama"),
            },
            // === One Punch Man ===
            Case {
                native: Some("サイタマ"),
                full: "Saitama",
                first: "Saitama",
                last: None,
            },
            Case {
                native: Some("ジェノス"),
                full: "Genos",
                first: "Genos",
                last: None,
            },
            // === Bleach ===
            Case {
                native: Some("黒崎一護"),
                full: "Ichigo Kurosaki",
                first: "Ichigo",
                last: Some("Kurosaki"),
            },
            Case {
                native: Some("朽木ルキア"),
                full: "Rukia Kuchiki",
                first: "Rukia",
                last: Some("Kuchiki"),
            },
            // === Naruto ===
            Case {
                native: Some("うずまきナルト"),
                full: "Naruto Uzumaki",
                first: "Naruto",
                last: Some("Uzumaki"),
            },
            Case {
                native: Some("うちはサスケ"),
                full: "Sasuke Uchiha",
                first: "Sasuke",
                last: Some("Uchiha"),
            },
            // === Mixed kana/kanji names ===
            Case {
                native: Some("水波レナ"),
                full: "Rena Mizunami",
                first: "Rena",
                last: Some("Mizunami"),
            },
            Case {
                native: Some("新城ユノ"),
                full: "Yuno Shinshiro",
                first: "Yuno",
                last: Some("Shinshiro"),
            },
            // === Complex family names ===
            Case {
                native: Some("四乃森蒼紫"),
                full: "Aoshi Shinomori",
                first: "Aoshi",
                last: Some("Shinomori"),
            },
            Case {
                native: Some("須々木心一"),
                full: "Shinichi Suzuki",
                first: "Shinichi",
                last: Some("Suzuki"),
            },
            // === Names with iteration marks ===
            Case {
                native: Some("長々間巡"),
                full: "Meguru Naganaima",
                first: "Meguru",
                last: Some("Naganaima"),
            },
            // === Single names from various franchises ===
            Case {
                native: Some("スネイプ"),
                full: "Snape",
                first: "Snape",
                last: None,
            },
            Case {
                native: Some("レオン"),
                full: "Leon",
                first: "Leon",
                last: None,
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let native = case.native.map(|n| n.trim()).unwrap_or("");
            let readings = name_parser::generate_name_readings(
                native,
                case.full.trim(),
                Some(case.first),
                case.last,
            );
            if !native.is_empty() {
                assert!(
                    !readings.full.is_empty(),
                    "Character {} ({}, native={}) should produce a non-empty reading",
                    i,
                    case.full,
                    native
                );
                // Verify no invalid characters in readings
                assert!(
                    !readings.full.contains('々'),
                    "Character {} ({}, native={}): Reading contains iteration mark: {}",
                    i,
                    case.full,
                    native,
                    readings.full
                );
            }
        }
    }

    // --- Advanced anime character parsing tests ---
    // Tests for complex kanji combinations, special characters, and regional variations

    #[test]
    fn test_complex_kanji_names() {
        // Three-character family names and longer given names
        let cases = vec![
            ("四乃森蒼紫", "Aoshi", Some("Shinomori")), // 4 chars family + 1 char given
            ("薫桜町火家", "Kae", Some("Kaore")),       // Complex kanji
            ("五条悟", "Satoru", Some("Gojo")),         // Gojo Satoru from JJK
            ("夏油傑", "Geto", Some("Getsuga")),        // Geto from JJK
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            assert!(
                !readings.full.is_empty(),
                "Complex name {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_long_katakana_names() {
        // Foreign names in katakana with various structures
        let cases = vec![
            ("シャーロック・ホームズ", "Sherlock", Some("Holmes")),
            (
                "アレキサンダー・アンダーソン",
                "Alexander",
                Some("Anderson"),
            ),
            ("フェリックス・アルジャンダー", "Felix", Some("Argyndale")),
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            assert!(
                !readings.full.is_empty(),
                "Katakana name {} should produce readings",
                native
            );
            assert!(
                !readings.full.contains('々'),
                "Katakana name {} should not contain iteration marks",
                native
            );
        }
    }

    #[test]
    fn test_special_character_sequences() {
        // Names with special patterns like small kana, prolonged sounds, etc.
        let cases = vec![
            ("藤和エリオ", "Elio", Some("Fujiwara")), // Small kana
            ("ティナ", "Tina", None),                 // Small ti sound
            ("ヴァイオレット", "Violet", None),       // Special katakana combinations
            ("シュヴァリエ", "Chevalier", None),      // More special combinations
        ];

        for (native, first, last) in &cases {
            let full_name = if let Some(l) = last {
                format!("{} {}", first, l)
            } else {
                first.to_string()
            };
            let readings =
                name_parser::generate_name_readings(native, &full_name, Some(first), *last);
            assert!(
                !readings.full.is_empty(),
                "Special sequence name {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_historical_and_traditional_names() {
        // Names from historical/traditional anime (samurai, etc.)
        let cases = vec![
            ("桂小五郎", "Kousaku", Some("Katsura")), // Historical name
            ("坂本龍馬", "Ryouma", Some("Sakamoto")), // Sakamoto Ryouma
            ("新選組局長近藤勇", "Isamu", Some("Kondo")), // Kondo Isami
            ("高杉晋作", "Shinsaku", Some("Takasugi")), // Takasugi Shinsaku
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            if !native.is_empty() {
                assert!(
                    !readings.full.is_empty(),
                    "Historical name {} should produce readings",
                    native
                );
            }
        }
    }

    #[test]
    fn test_mixed_cjk_characters() {
        // Names mixing different Japanese scripts
        let cases = vec![
            ("猫田 虎之助", "Toranosuke", Some("Nekota")), // Kanji with space
            ("小田切 敏也", "Toshiya", Some("Odagiri")),   // Kanji family + space
            ("竜嶺 透", "Toru", Some("Ryumine")),          // Mixed stroke counts
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            assert!(
                !readings.full.is_empty(),
                "Mixed CJK name {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_unusual_given_names() {
        // Unique given names from anime (sometimes single character or unusual)
        let cases = vec![
            ("結城梨斗", "Rito", Some("Yuuki")), // Unique katakana/kanji mix
            ("佐藤和真", "Kazuma", Some("Satou")), // Complex reading
            ("春日野さくら", "Sakura", Some("Kasugano")), // Hiragana given name
            ("一ノ瀬京", "Kyou", Some("Ichinose")), // One-character given name
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            assert!(
                !readings.full.is_empty(),
                "Unusual given name {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_phonetic_name_variations() {
        // Names where hiragana/katakana phonetic spelling is used
        let cases = vec![
            ("日向ひなた", "Hinata", Some("Hinata")), // Same family/given
            ("桃井りんご", "Ringo", Some("Momoi")),   // Hiragana given
            ("楠木ともり", "Tomori", Some("Kusunoki")), // Hiragana given with marks
        ];

        for (native, first, last) in &cases {
            let readings = name_parser::generate_name_readings(
                native,
                &format!("{} {}", first, last.unwrap_or("")),
                Some(first),
                *last,
            );
            assert!(
                !readings.full.is_empty(),
                "Phonetic variation {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_visual_novel_specific_names() {
        // Names from key visual novel titles
        struct Case {
            native: &'static str,
            first: &'static str,
            last: Option<&'static str>,
        }

        let cases = vec![
            // Clannad
            Case {
                native: "岡崎朋也",
                first: "Tomoya",
                last: Some("Okazaki"),
            },
            Case {
                native: "古河渚",
                first: "Nagisa",
                last: Some("Furukawa"),
            },
            // Little Busters
            Case {
                native: "直枝理樹",
                first: "Riki",
                last: Some("Naoe"),
            },
            // Ever17
            Case {
                native: "吉川拓也",
                first: "Takuya",
                last: Some("Yoshikawa"),
            },
            // Danganronpa
            Case {
                native: "苗木誠",
                first: "Makoto",
                last: Some("Naegi"),
            },
            Case {
                native: "江ノ島盾子",
                first: "Junko",
                last: Some("Enoshima"),
            },
            // Umineko
            Case {
                native: "右代宫家",
                first: "Battler",
                last: Some("Ushiromiya"),
            },
        ];

        for case in &cases {
            let full_name = if let Some(l) = case.last {
                format!("{} {}", case.first, l)
            } else {
                case.first.to_string()
            };
            let readings = name_parser::generate_name_readings(
                case.native,
                &full_name,
                Some(case.first),
                case.last,
            );
            assert!(
                !readings.full.is_empty(),
                "Visual novel name {} should produce readings",
                case.native
            );
        }
    }

    #[test]
    fn test_western_inspired_anime_names() {
        // Names that are transliterations or inspired by Western names
        let cases = vec![
            ("シャーロット", "Charlotte", None),
            ("ヴィクトル・ニキフォロフ", "Victor", Some("Nikiforov")),
            ("カルロス・パレロ", "Carlos", Some("Parero")),
            ("レイラ・シイサ", "Layla", Some("Shiisa")),
        ];

        for (native, first, last) in &cases {
            let full_name = if let Some(l) = last {
                format!("{} {}", first, l)
            } else {
                first.to_string()
            };
            let readings =
                name_parser::generate_name_readings(native, &full_name, Some(first), *last);
            assert!(
                !readings.full.is_empty(),
                "Western-inspired name {} should produce readings",
                native
            );
        }
    }

    #[test]
    fn test_year_2024_2025_popular_anime() {
        // Recent popular anime characters (2024-2025)
        struct Case {
            native: Option<&'static str>,
            full: &'static str,
            first: &'static str,
            last: Option<&'static str>,
        }

        let cases = vec![
            // Frieren: Beyond Journey's End
            Case {
                native: Some("フリーレン"),
                full: "Frieren",
                first: "Frieren",
                last: None,
            },
            // The Apothecary Diaries
            Case {
                native: Some("猫猫"),
                full: "Maomao",
                first: "Maomao",
                last: None,
            },
            Case {
                native: Some("羅漢"),
                full: "Rakan",
                first: "Rakan",
                last: None,
            },
            // Tsurune
            Case {
                native: Some("鶴見知利"),
                full: "Chiri Tsurumi",
                first: "Chiri",
                last: Some("Tsurumi"),
            },
            // Dandadan
            Case {
                native: Some("真子"),
                full: "Mako",
                first: "Mako",
                last: None,
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let native = case.native.unwrap_or("");
            let readings = name_parser::generate_name_readings(
                native,
                case.full.trim(),
                Some(case.first),
                case.last,
            );
            if !native.is_empty() {
                assert!(
                    !readings.full.is_empty(),
                    "Recent anime character {} ({}) should produce readings",
                    i,
                    case.full
                );
            }
        }
    }

    #[test]
    fn test_bulk_all_characters() {
        struct Case {
            native: Option<&'static str>,
            full: &'static str,
            first: &'static str,
            last: Option<&'static str>,
        }

        let cases = vec![
            Case {
                native: Some("幸平創真"),
                full: "Souma Yukihira",
                first: "Souma",
                last: Some("Yukihira"),
            },
            Case {
                native: Some("薙切えりな"),
                full: "Erina Nakiri",
                first: "Erina",
                last: Some("Nakiri"),
            },
            Case {
                native: Some("田所恵"),
                full: "Megumi Tadokoro",
                first: "Megumi",
                last: Some("Tadokoro"),
            },
            Case {
                native: Some("薙切仙左衛門"),
                full: "Senzaemon Nakiri",
                first: "Senzaemon",
                last: Some("Nakiri"),
            },
            Case {
                native: Some("大御堂ふみ緒"),
                full: "Fumio Daimidou",
                first: "Fumio",
                last: Some("Daimidou"),
            },
            Case {
                native: Some("ローランド・シャペル"),
                full: "Roland Chapelle",
                first: "Roland",
                last: Some("Chapelle"),
            },
            Case {
                native: Some("幸平城一郎"),
                full: "Jouichirou Yukihira",
                first: "Jouichirou",
                last: Some("Yukihira"),
            },
            Case {
                native: Some("ごうだばやしきよし"),
                full: "Kiyoshi Goudabayashi",
                first: "Kiyoshi",
                last: Some("Goudabayashi"),
            },
            Case {
                native: Some("一色慧"),
                full: "Satoshi Isshiki",
                first: "Satoshi",
                last: Some("Isshiki"),
            },
            Case {
                native: Some("水戸郁魅"),
                full: "Ikumi Mito",
                first: "Ikumi",
                last: Some("Mito"),
            },
            Case {
                native: Some("榊涼子"),
                full: "Ryouko Sakaki",
                first: "Ryouko",
                last: Some("Sakaki"),
            },
            Case {
                native: Some("吉野悠姫"),
                full: "Yuuki Yoshino",
                first: "Yuuki",
                last: Some("Yoshino"),
            },
            Case {
                native: Some("伊武崎峻"),
                full: "Shun Ibusaki",
                first: "Shun",
                last: Some("Ibusaki"),
            },
            Case {
                native: Some("タクミ・アルディーニ"),
                full: "Takumi Aldini",
                first: "Takumi",
                last: Some("Aldini"),
            },
            Case {
                native: Some("乾日向子"),
                full: "Hinako Inui",
                first: "Hinako",
                last: Some("Inui"),
            },
            Case {
                native: Some("四宮小次郎"),
                full: "Koujirou Shinomiya",
                first: "Koujirou",
                last: Some("Shinomiya"),
            },
            Case {
                native: Some("堂島銀"),
                full: "Gin Doujima",
                first: "Gin",
                last: Some("Doujima"),
            },
            Case {
                native: Some("イサミ・アルディーニ"),
                full: "Isami Aldini",
                first: "Isami",
                last: Some("Aldini"),
            },
            Case {
                native: Some("水原冬美"),
                full: "Fuyumi Mizuhara",
                first: "Fuyumi",
                last: Some("Mizuhara"),
            },
            Case {
                native: Some("関守平"),
                full: "Hitoshi Seikimori",
                first: "Hitoshi",
                last: Some("Seikimori"),
            },
            Case {
                native: Some("ドナート梧桐田"),
                full: "Goutoda Donato",
                first: "Goutoda",
                last: Some("Donato"),
            },
            Case {
                native: Some("安東伸吾"),
                full: "Shingo Andou",
                first: "Shingo",
                last: Some("Andou"),
            },
            Case {
                native: Some("丸井善二"),
                full: "Zenji Marui",
                first: "Zenji",
                last: Some("Marui"),
            },
            Case {
                native: Some("薙切アリス"),
                full: "Alice Nakiri",
                first: "Alice",
                last: Some("Nakiri"),
            },
            Case {
                native: Some("黒木場リョウ"),
                full: "Ryou Kurokiba",
                first: "Ryou",
                last: Some("Kurokiba"),
            },
            Case {
                native: Some("枝津也叡山"),
                full: "Eizan Etsuya",
                first: "Eizan",
                last: Some("Etsuya"),
            },
            Case {
                native: Some("葉山アキラ"),
                full: "Akira Hayama",
                first: "Akira",
                last: Some("Hayama"),
            },
            Case {
                native: Some("新戸緋沙子"),
                full: "Hisako Arato",
                first: "Hisako",
                last: Some("Arato"),
            },
            Case {
                native: Some("貞塚ナオ"),
                full: "Nao Sadatsuka",
                first: "Nao",
                last: Some("Sadatsuka"),
            },
            Case {
                native: Some("汐見潤"),
                full: "Jun Shiomi",
                first: "Jun",
                last: Some("Shiomi"),
            },
            Case {
                native: Some("峰ヶ崎八重子"),
                full: "Yaeko Minegasaki",
                first: "Yaeko",
                last: Some("Minegasaki"),
            },
            Case {
                native: Some("北条美代子"),
                full: "Miyoko Houjou",
                first: "Miyoko",
                last: Some("Houjou"),
            },
            Case {
                native: Some("佐藤昭二"),
                full: "Shouji Satou",
                first: "Shouji",
                last: Some("Satou"),
            },
            Case {
                native: Some("青木大吾"),
                full: "Daigo Aoki",
                first: "Daigo",
                last: Some("Aoki"),
            },
            Case {
                native: Some("小金井亞紀"),
                full: "Aki Koganei",
                first: "Aki",
                last: Some("Koganei"),
            },
            Case {
                native: Some("中百舌鳥きぬ"),
                full: "Kinu Nakamozu",
                first: "Kinu",
                last: Some("Nakamozu"),
            },
            Case {
                native: Some("徳蔵"),
                full: "Tokuzou",
                first: "Tokuzou",
                last: None,
            },
            Case {
                native: Some("小西寛一"),
                full: "Kanichi Konishi",
                first: "Kanichi",
                last: Some("Konishi"),
            },
            Case {
                native: Some("川島麗"),
                full: "Urara Kawashima",
                first: "Urara",
                last: Some("Kawashima"),
            },
            Case {
                native: Some("喜田修治"),
                full: "Osaji Kita",
                first: "Osaji",
                last: Some("Kita"),
            },
            Case {
                native: Some("千俵なつめ"),
                full: "Natsume Sendawara",
                first: "Natsume",
                last: Some("Sendawara"),
            },
            Case {
                native: Some("千俵 おりえ"),
                full: "Orie Sendawara",
                first: "Orie",
                last: Some("Sendawara"),
            },
            Case {
                native: Some("佐々木由愛"),
                full: "Yua Sasaki",
                first: "Yua",
                last: Some("Sasaki"),
            },
            Case {
                native: Some("倉瀬真由美"),
                full: "Mayumi Kurase",
                first: "Mayumi",
                last: Some("Kurase"),
            },
            Case {
                native: Some("富田友哉"),
                full: "Yuuya Tomita",
                first: "Yuuya",
                last: Some("Tomita"),
            },
            Case {
                native: Some("蔵木 滋乃"),
                full: "Shigeno Kuraki",
                first: "Shigeno",
                last: Some("Kuraki"),
            },
            Case {
                native: Some("田所の母"),
                full: "Tadokoro no Haha",
                first: "Tadokoro no Haha",
                last: Some(""),
            },
            Case {
                native: Some("香田茂之進"),
                full: "Shigenoshin Kouda",
                first: "Shigenoshin",
                last: Some("Kouda"),
            },
            Case {
                native: Some("榎本円"),
                full: "Madoka Enomoto",
                first: "Madoka",
                last: Some("Enomoto"),
            },
            Case {
                native: Some("佐久間時彦"),
                full: "Tokihiko Sakuma",
                first: "Tokihiko",
                last: Some("Sakuma"),
            },
            Case {
                native: Some("岩倉玲音"),
                full: "Lain Iwakura",
                first: "Lain",
                last: Some("Iwakura"),
            },
            Case {
                native: Some("英利政美"),
                full: "Masami Eiri",
                first: "Masami",
                last: Some("Eiri"),
            },
            Case {
                native: Some("瑞城ありす"),
                full: "Arisu Mizuki",
                first: "Arisu",
                last: Some("Mizuki"),
            },
            Case {
                native: Some("岩倉美香"),
                full: "Mika Iwakura",
                first: "Mika",
                last: Some("Iwakura"),
            },
            Case {
                native: Some("岩倉康男"),
                full: "Yasuo Iwakura",
                first: "Yasuo",
                last: Some("Iwakura"),
            },
            Case {
                native: Some("タロウ"),
                full: "Tarou",
                first: "Tarou",
                last: None,
            },
            Case {
                native: None,
                full: "Lin Sui-Xi",
                first: "Lin",
                last: Some("Sui-Xi"),
            },
            Case {
                native: Some("カール・ハウスホーファー"),
                full: "Karl",
                first: "Karl",
                last: None,
            },
            Case {
                native: Some("岩倉美穂"),
                full: "Miho Iwakura",
                first: "Miho",
                last: Some("Iwakura"),
            },
            Case {
                native: None,
                full: "J.J",
                first: "J.J",
                last: None,
            },
            Case {
                native: Some("四方田千砂"),
                full: "Chisa Yomoda",
                first: "Chisa",
                last: Some("Yomoda"),
            },
            Case {
                native: Some("山本麗華"),
                full: "Reika Yamamoto",
                first: "Reika",
                last: Some("Yamamoto"),
            },
            Case {
                native: Some("加藤樹莉"),
                full: "Juri Katou",
                first: "Juri",
                last: Some("Katou"),
            },
            Case {
                native: None,
                full: "Myu-Myu",
                first: "Myu-Myu",
                last: None,
            },
            Case {
                native: Some("坂本竜太"),
                full: "Ryouta Sakamoto",
                first: "Ryouta",
                last: Some("Sakamoto"),
            },
            Case {
                native: Some("ヒミコ"),
                full: "Himiko",
                first: "Himiko",
                last: None,
            },
            Case {
                native: Some("今川義明"),
                full: "Yoshiaki Imagawa",
                first: "Yoshiaki",
                last: Some("Imagawa"),
            },
            Case {
                native: Some("吉良康介"),
                full: "Kousuke Kira",
                first: "Kousuke",
                last: Some("Kira"),
            },
            Case {
                native: Some("夏目総一"),
                full: "Souichi Natsume",
                first: "Souichi",
                last: Some("Natsume"),
            },
            Case {
                native: Some("宮本雅志"),
                full: "Masashi Miyamoto",
                first: "Masashi",
                last: Some("Miyamoto"),
            },
            Case {
                native: Some("織田信隆"),
                full: "Nobutaka Oda",
                first: "Nobutaka",
                last: Some("Oda"),
            },
            Case {
                native: Some("平清"),
                full: "Kiyoshi Taira",
                first: "Kiyoshi",
                last: Some("Taira"),
            },
            Case {
                native: Some("みほ"),
                full: "Miho",
                first: "Miho",
                last: None,
            },
            Case {
                native: Some("木下秀美"),
                full: "Hidemi Kinoshita",
                first: "Hidemi",
                last: Some("Kinoshita"),
            },
            Case {
                native: Some("伊達政人"),
                full: "Masahito Date",
                first: "Masahito",
                last: Some("Date"),
            },
            Case {
                native: Some("村崎志紀"),
                full: "Shiki Murasaki",
                first: "Shiki",
                last: Some("Murasaki"),
            },
            Case {
                native: Some("鷹嘴"),
                full: "Takanohashi",
                first: "",
                last: Some("Takanohashi"),
            },
            Case {
                native: Some("飯田恒明"),
                full: "Tsuneaki Iida",
                first: "Tsuneaki",
                last: Some("Iida"),
            },
            Case {
                native: Some("明智 光男"),
                full: "Mitsuo Akechi",
                first: "Mitsuo",
                last: Some("Akechi"),
            },
            Case {
                native: Some("吉良義久"),
                full: "Yoshihisa Kira",
                first: "Yoshihisa",
                last: Some("Kira"),
            },
            Case {
                native: Some("近藤勇"),
                full: "Isamu Kondou",
                first: "Isamu",
                last: Some("Kondou"),
            },
            Case {
                native: Some("坂本幸江"),
                full: "Yukie Sakamoto",
                first: "Yukie",
                last: Some("Sakamoto"),
            },
            Case {
                native: Some("坂本信久"),
                full: "Hisanobu Sakamoto",
                first: "Hisanobu",
                last: Some("Sakamoto"),
            },
            Case {
                native: Some("ありさ"),
                full: "Arisa",
                first: "Arisa",
                last: None,
            },
            Case {
                native: Some("ユキ"),
                full: "Yuki",
                first: "Yuki",
                last: None,
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let native = case.native.map(|n| n.trim()).unwrap_or("");
            let readings = name_parser::generate_name_readings(
                native,
                case.full.trim(),
                Some(case.first),
                case.last,
            );
            if !native.is_empty() {
                assert!(
                    !readings.full.is_empty(),
                    "Character {} ({}, native={}) should produce a non-empty reading",
                    i,
                    case.full,
                    native
                );
            }
        }
    }
}
