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
        assert_eq!(readings.full, "そうま ゆきひら");
        assert_eq!(readings.family, "そうま ゆきひら");
        assert_eq!(readings.given, "そうま ゆきひら");
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
