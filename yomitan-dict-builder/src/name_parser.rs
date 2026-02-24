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
    pub full: String,    // Full hiragana reading (family + given)
    pub family: String,  // Family name hiragana reading
    pub given: String,   // Given name hiragana reading
}

/// Honorific suffixes: (display form, hiragana reading, English description)
pub const HONORIFIC_SUFFIXES: &[(&str, &str, &str)] = &[
    // ===== Respectful / Formal =====
    ("さん", "さん", "Generic polite suffix (Mr./Ms./Mrs.)"),
    ("様", "さま", "Very formal/respectful (Lord/Lady/Dear)"),
    ("さま", "さま", "Kana form of 様 — very formal/respectful"),
    ("氏", "し", "Formal written suffix (Mr./Ms.)"),
    ("殿", "どの", "Formal/archaic (Lord, used in official documents)"),
    ("殿", "てん", "Alternate reading of 殿 (rare)"),
    ("御前", "おまえ", "Archaic respectful (Your Presence)"),
    ("御前", "ごぜん", "Alternate reading of 御前 (Your Excellency)"),
    ("貴殿", "きでん", "Very formal written (Your Honor)"),
    ("閣下", "かっか", "Your Excellency (diplomatic/military)"),
    ("陛下", "へいか", "Your Majesty (royalty)"),
    ("殿下", "でんか", "Your Highness (royalty)"),
    ("妃殿下", "ひでんか", "Her Royal Highness (princess consort)"),
    ("親王", "しんのう", "Prince of the Blood (Imperial family)"),
    ("内親王", "ないしんのう", "Princess of the Blood (Imperial family)"),
    ("宮", "みや", "Prince/Princess (Imperial branch family)"),
    ("上", "うえ", "Archaic superior address (e.g. 父上)"),
    ("公", "こう", "Duke / Lord (nobility)"),
    ("卿", "きょう", "Lord (archaic nobility, also used in fantasy)"),
    ("侯", "こう", "Marquis (nobility)"),
    ("伯", "はく", "Count/Earl (nobility)"),
    ("子", "し", "Viscount (nobility) / Master (classical)"),
    ("男", "だん", "Baron (nobility)"),

    // ===== Casual / Friendly =====
    ("君", "くん", "Familiar suffix (usually male, junior)"),
    ("くん", "くん", "Kana form of 君 — familiar (usually male)"),
    ("ちゃん", "ちゃん", "Endearing suffix (children, close friends, girls)"),
    ("たん", "たん", "Baby-talk version of ちゃん"),
    ("ちん", "ちん", "Cutesy/playful variant of ちゃん"),
    ("りん", "りん", "Cutesy suffix (internet/otaku culture)"),
    ("っち", "っち", "Playful/affectionate suffix"),
    ("ぴょん", "ぴょん", "Cutesy/bouncy suffix"),
    ("にゃん", "にゃん", "Cat-like cutesy suffix"),
    ("みん", "みん", "Cutesy diminutive suffix"),
    ("ぽん", "ぽん", "Playful suffix"),
    ("坊", "ぼう", "Young boy / little one"),
    ("坊ちゃん", "ぼっちゃん", "Young master / rich boy"),
    ("嬢", "じょう", "Young lady"),
    ("嬢ちゃん", "じょうちゃん", "Little miss"),
    ("お嬢", "おじょう", "Young lady (polite)"),
    ("お嬢様", "おじょうさま", "Young lady (very polite/rich girl)"),
    ("姫", "ひめ", "Princess (also used affectionately)"),
    ("姫様", "ひめさま", "Princess (formal)"),
    ("王子", "おうじ", "Prince"),
    ("王子様", "おうじさま", "Prince (formal/fairy-tale)"),
    ("王女", "おうじょ", "Princess (royal daughter)"),

    // ===== Academic / Educational =====
    ("先生", "せんせい", "Teacher/Doctor/Master"),
    ("先輩", "せんぱい", "Senior (school/work)"),
    ("後輩", "こうはい", "Junior (school/work)"),
    ("教授", "きょうじゅ", "Professor"),
    ("准教授", "じゅんきょうじゅ", "Associate Professor"),
    ("助教", "じょきょう", "Assistant Professor"),
    ("講師", "こうし", "Lecturer"),
    ("博士", "はかせ", "Doctor (academic/scientist)"),
    ("博士", "はくし", "Doctor (alternate formal reading)"),
    ("師匠", "ししょう", "Master/Mentor (arts, martial arts)"),
    ("師範", "しはん", "Master instructor (martial arts)"),
    ("老師", "ろうし", "Venerable teacher / Zen master"),
    ("塾長", "じゅくちょう", "Cram school principal"),
    ("校長", "こうちょう", "School principal"),
    ("学長", "がくちょう", "University president"),
    ("園長", "えんちょう", "Kindergarten/zoo director"),
    ("生徒", "せいと", "Student (used as address in some contexts)"),

    // ===== Corporate / Business =====
    ("社長", "しゃちょう", "Company president/CEO"),
    ("副社長", "ふくしゃちょう", "Vice president"),
    ("会長", "かいちょう", "Chairman"),
    ("部長", "ぶちょう", "Department head/Director"),
    ("副部長", "ふくぶちょう", "Deputy department head"),
    ("課長", "かちょう", "Section chief/Manager"),
    ("係長", "かかりちょう", "Subsection chief"),
    ("主任", "しゅにん", "Chief/Senior staff"),
    ("店長", "てんちょう", "Store manager"),
    ("支配人", "しはいにん", "Manager (hotel/theater)"),
    ("専務", "せんむ", "Senior Managing Director"),
    ("常務", "じょうむ", "Managing Director"),
    ("取締役", "とりしまりやく", "Board Director"),
    ("監督", "かんとく", "Director/Supervisor/Coach"),
    ("所長", "しょちょう", "Office/institute director"),
    ("局長", "きょくちょう", "Bureau director"),
    ("室長", "しつちょう", "Office chief / Lab head"),
    ("班長", "はんちょう", "Squad leader / Team leader"),
    ("組長", "くみちょう", "Group leader (also yakuza boss)"),
    ("番頭", "ばんとう", "Head clerk (traditional business)"),
    ("頭取", "とうどり", "Bank president"),
    ("理事長", "りじちょう", "Board chairman"),
    ("理事", "りじ", "Board member/Trustee"),
    ("総裁", "そうさい", "Governor/President (of institution)"),
    ("代表", "だいひょう", "Representative"),

    // ===== Government / Political =====
    ("大臣", "だいじん", "Minister (government)"),
    ("総理", "そうり", "Prime Minister (short form)"),
    ("総理大臣", "そうりだいじん", "Prime Minister (full)"),
    ("長官", "ちょうかん", "Director-General / Commissioner"),
    ("知事", "ちじ", "Governor (prefecture)"),
    ("市長", "しちょう", "Mayor"),
    ("町長", "ちょうちょう", "Town mayor"),
    ("村長", "そんちょう", "Village chief"),
    ("区長", "くちょう", "Ward mayor"),
    ("議長", "ぎちょう", "Chairman (assembly/parliament)"),
    ("議員", "ぎいん", "Legislator/Councilmember"),
    ("大使", "たいし", "Ambassador"),
    ("公使", "こうし", "Minister (diplomatic)"),
    ("領事", "りょうじ", "Consul"),
    ("奉行", "ぶぎょう", "Magistrate (Edo period)"),
    ("代官", "だいかん", "Magistrate/Intendant (historical)"),

    // ===== Military / Law Enforcement =====
    ("大将", "たいしょう", "General/Admiral"),
    ("中将", "ちゅうじょう", "Lieutenant General"),
    ("少将", "しょうしょう", "Major General"),
    ("大佐", "たいさ", "Colonel"),
    ("中佐", "ちゅうさ", "Lieutenant Colonel"),
    ("少佐", "しょうさ", "Major"),
    ("大尉", "たいい", "Captain (military)"),
    ("中尉", "ちゅうい", "First Lieutenant"),
    ("少尉", "しょうい", "Second Lieutenant"),
    ("軍曹", "ぐんそう", "Sergeant"),
    ("伍長", "ごちょう", "Corporal"),
    ("兵長", "へいちょう", "Lance Corporal / Senior Private"),
    ("上等兵", "じょうとうへい", "Private First Class"),
    ("元帥", "げんすい", "Marshal/Fleet Admiral"),
    ("提督", "ていとく", "Admiral (naval, common in anime)"),
    ("司令", "しれい", "Commander"),
    ("司令官", "しれいかん", "Commanding Officer"),
    ("総司令", "そうしれい", "Supreme Commander"),
    ("参謀", "さんぼう", "Staff Officer / Strategist"),
    ("隊長", "たいちょう", "Squad/Unit captain"),
    ("団長", "だんちょう", "Regiment/Group commander"),
    ("師団長", "しだんちょう", "Division commander"),
    ("艦長", "かんちょう", "Ship captain"),
    ("船長", "せんちょう", "Ship captain (civilian)"),
    ("機長", "きちょう", "Aircraft captain/Pilot in command"),
    ("警部", "けいぶ", "Police Inspector"),
    ("警視", "けいし", "Superintendent (police)"),
    ("巡査", "じゅんさ", "Police officer (patrol)"),
    ("刑事", "けいじ", "Detective"),
    ("署長", "しょちょう", "Police station chief"),
    ("長官", "ちょうかん", "Commissioner (police/agency)"),
    ("将軍", "しょうぐん", "Shogun / General (historical)"),
    ("大名", "だいみょう", "Feudal lord (historical)"),

    // ===== Religious / Spiritual =====
    ("神", "かみ", "God"),
    ("神様", "かみさま", "God (respectful)"),
    ("上人", "しょうにん", "Holy person (Buddhist)"),
    ("聖人", "せいじん", "Saint"),
    ("法師", "ほうし", "Buddhist priest"),
    ("坊主", "ぼうず", "Buddhist monk (casual)"),
    ("和尚", "おしょう", "Buddhist priest/monk"),
    ("住職", "じゅうしょく", "Head priest (temple)"),
    ("禅師", "ぜんじ", "Zen master"),
    ("大師", "だいし", "Great master (Buddhist title)"),
    ("上座", "じょうざ", "Senior monk"),
    ("尼", "あま", "Buddhist nun"),
    ("巫女", "みこ", "Shrine maiden"),
    ("宮司", "ぐうじ", "Chief Shinto priest"),
    ("神主", "かんぬし", "Shinto priest"),
    ("神父", "しんぷ", "Catholic priest / Father"),
    ("牧師", "ぼくし", "Protestant pastor"),
    ("司祭", "しさい", "Priest (Christian)"),
    ("司教", "しきょう", "Bishop"),
    ("枢機卿", "すうききょう", "Cardinal"),
    ("教皇", "きょうこう", "Pope"),
    ("法王", "ほうおう", "Pope (alternate) / Dharma King"),
    ("猊下", "げいか", "Your Holiness/Eminence"),

    // ===== Medical =====
    ("医師", "いし", "Doctor/Physician"),
    ("医者", "いしゃ", "Doctor (colloquial)"),
    ("看護師", "かんごし", "Nurse"),
    ("薬剤師", "やくざいし", "Pharmacist"),
    ("歯科医", "しかい", "Dentist"),
    ("獣医", "じゅうい", "Veterinarian"),
    ("院長", "いんちょう", "Hospital director"),

    // ===== Martial Arts / Traditional =====
    ("師範", "しはん", "Master instructor"),
    ("範士", "はんし", "Grand master (martial arts)"),
    ("教士", "きょうし", "Senior teacher (martial arts)"),
    ("達人", "たつじん", "Master/Expert"),
    ("名人", "めいじん", "Grand master (go, shogi, etc.)"),
    ("棋士", "きし", "Professional go/shogi player"),
    ("横綱", "よこづな", "Grand champion (sumo)"),
    ("大関", "おおぜき", "Champion (sumo)"),
    ("関脇", "せきわけ", "Junior champion (sumo)"),
    ("小結", "こむすび", "Junior champion 2nd (sumo)"),
    ("親方", "おやかた", "Stable master (sumo) / Boss (craftsman)"),
    ("力士", "りきし", "Sumo wrestler"),

    // ===== Family / Kinship (used as honorific address) =====
    ("兄", "にい", "Older brother (short)"),
    ("兄さん", "にいさん", "Older brother"),
    ("お兄さん", "おにいさん", "Older brother (polite)"),
    ("お兄ちゃん", "おにいちゃん", "Big bro (affectionate)"),
    ("兄ちゃん", "にいちゃん", "Big bro (casual)"),
    ("兄貴", "あにき", "Big bro (rough/yakuza)"),
    ("兄上", "あにうえ", "Older brother (archaic/formal)"),
    ("姉", "ねえ", "Older sister (short)"),
    ("姉さん", "ねえさん", "Older sister"),
    ("お姉さん", "おねえさん", "Older sister (polite)"),
    ("お姉ちゃん", "おねえちゃん", "Big sis (affectionate)"),
    ("姉ちゃん", "ねえちゃん", "Big sis (casual)"),
    ("姉貴", "あねき", "Big sis (rough)"),
    ("姉上", "あねうえ", "Older sister (archaic/formal)"),
    ("弟", "おとうと", "Younger brother"),
    ("妹", "いもうと", "Younger sister"),
    ("父上", "ちちうえ", "Father (archaic/formal)"),
    ("母上", "ははうえ", "Mother (archaic/formal)"),
    ("お父さん", "おとうさん", "Father"),
    ("お母さん", "おかあさん", "Mother"),
    ("おじさん", "おじさん", "Uncle / Middle-aged man"),
    ("おばさん", "おばさん", "Aunt / Middle-aged woman"),
    ("おじいさん", "おじいさん", "Grandfather / Old man"),
    ("おばあさん", "おばあさん", "Grandmother / Old woman"),
    ("じいちゃん", "じいちゃん", "Grandpa (casual)"),
    ("ばあちゃん", "ばあちゃん", "Grandma (casual)"),
    ("お嫁さん", "およめさん", "Bride / Wife (polite)"),
    ("奥様", "おくさま", "Wife (very polite)"),
    ("奥さん", "おくさん", "Wife (polite)"),
    ("旦那", "だんな", "Husband / Master"),
    ("旦那様", "だんなさま", "Husband / Master (formal)"),

    // ===== Historical / Feudal =====
    ("御所", "ごしょ", "Imperial Palace / Emperor (by metonymy)"),
    ("関白", "かんぱく", "Imperial Regent"),
    ("摂政", "せっしょう", "Regent"),
    ("太閤", "たいこう", "Retired regent (Hideyoshi's title)"),
    ("太政大臣", "だいじょうだいじん", "Grand Chancellor"),
    ("征夷大将軍", "せいいたいしょうぐん", "Shogun (full title)"),
    ("守護", "しゅご", "Provincial governor (medieval)"),
    ("地頭", "じとう", "Land steward (medieval)"),
    ("家老", "かろう", "Chief retainer (samurai)"),
    ("侍", "さむらい", "Samurai"),
    ("武士", "ぶし", "Warrior"),
    ("浪人", "ろうにん", "Masterless samurai"),
    ("忍", "しのび", "Ninja (short form)"),
    ("殿様", "とのさま", "Lord (feudal)"),
    ("お殿様", "おとのさま", "Lord (very polite)"),
    ("お館様", "おやかたさま", "Lord of the castle"),
    ("若", "わか", "Young lord/master"),
    ("若様", "わかさま", "Young lord (formal)"),
    ("若殿", "わかとの", "Young lord"),

    // ===== Fantasy / Fictional (common in VN/anime) =====
    ("王", "おう", "King"),
    ("王様", "おうさま", "King (polite)"),
    ("女王", "じょおう", "Queen"),
    ("女王様", "じょおうさま", "Queen (formal)"),
    ("皇帝", "こうてい", "Emperor"),
    ("皇后", "こうごう", "Empress"),
    ("天皇", "てんのう", "Emperor (Japanese)"),
    ("魔王", "まおう", "Demon King"),
    ("魔王様", "まおうさま", "Demon King (respectful)"),
    ("勇者", "ゆうしゃ", "Hero/Brave"),
    ("勇者様", "ゆうしゃさま", "Hero (respectful)"),
    ("聖女", "せいじょ", "Holy maiden / Saintess"),
    ("魔女", "まじょ", "Witch"),
    ("賢者", "けんじゃ", "Sage/Wise one"),
    ("導師", "どうし", "Guide/Mentor (fantasy)"),
    ("騎士", "きし", "Knight"),
    ("団長", "だんちょう", "Commander (guild/order)"),
    ("長老", "ちょうろう", "Elder"),
    ("族長", "ぞくちょう", "Clan chief / Tribal leader"),
    ("頭領", "とうりょう", "Boss / Chief (bandits, guilds)"),
    ("首領", "しゅりょう", "Leader / Boss"),
    ("大王", "だいおう", "Great King"),
    ("姫君", "ひめぎみ", "Princess (literary)"),
    ("御方", "おかた", "That person (very respectful)"),
    ("主", "ぬし", "Master/Lord (archaic)"),
    ("主", "あるじ", "Master/Lord (alternate reading)"),
    ("主人", "しゅじん", "Master/Head of household"),
    ("ご主人", "ごしゅじん", "Master (polite)"),
    ("ご主人様", "ごしゅじんさま", "Master (very polite, maid usage)"),
    ("お方", "おかた", "Person (respectful)"),

    // ===== Otaku / Internet / Modern Slang =====
    ("氏", "うじ", "Alternate reading of 氏 (internet)"),
    ("師", "し", "Master/Teacher (respectful, online)"),
    ("大先生", "だいせんせい", "Great teacher (sometimes ironic)"),
    ("御大", "おんたい", "The great one / Big boss"),
    ("大御所", "おおごしょ", "Grand old master / Authority"),
    ("パイセン", "ぱいせん", "Senpai (slang reversal)"),
    ("っす", "っす", "Casual desu (used as address marker)"),
    ("どの", "どの", "Kana form of 殿"),
    ("さま", "さま", "Kana form of 様 (duplicate)"),
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
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        } else {
            // Pure kana — use kata_to_hira on the Japanese text itself
            let full = kata_to_hira(&name_original.replace(' ', ""));
            return NameReadings {
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
        assert!(HONORIFIC_SUFFIXES.len() >= 200);
    }

    #[test]
    fn test_honorific_suffixes_contain_common() {
        let suffixes: Vec<&str> = HONORIFIC_SUFFIXES.iter().map(|(s, _, _)| *s).collect();
        assert!(suffixes.contains(&"さん"));
        assert!(suffixes.contains(&"ちゃん"));
        assert!(suffixes.contains(&"くん"));
    }

    #[test]
    fn test_honorific_suffixes_have_descriptions() {
        for (suffix, _reading, description) in HONORIFIC_SUFFIXES {
            assert!(
                !description.is_empty(),
                "Honorific '{}' should have a non-empty description",
                suffix
            );
        }
    }
}
