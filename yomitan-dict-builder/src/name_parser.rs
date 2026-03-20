/// Name handling: splitting Japanese names, generating readings, and honorific data.
///
/// Uses low-level kana utilities from `crate::kana`.
use crate::kana;

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
    pub full: String,   // Full hiragana reading (family + given)
    pub family: String, // Family name hiragana reading
    pub given: String,  // Given name hiragana reading
}

/// Honorific suffixes: (display form, hiragana reading, English description)
pub const HONORIFIC_SUFFIXES: &[(&str, &str, &str)] = &[
    // ===== Respectful / Formal =====
    ("さん", "さん", "Generic polite suffix (Mr./Ms./Mrs.)"),
    ("様", "さま", "Very formal/respectful (Lord/Lady/Dear)"),
    ("さま", "さま", "Kana form of 様 — very formal/respectful"),
    ("氏", "し", "Formal written suffix (Mr./Ms.)"),
    (
        "殿",
        "どの",
        "Formal/archaic (Lord, used in official documents)",
    ),
    ("殿", "てん", "Alternate reading of 殿 (rare)"),
    ("御前", "おまえ", "Archaic respectful (Your Presence)"),
    (
        "御前",
        "ごぜん",
        "Alternate reading of 御前 (Your Excellency)",
    ),
    ("貴殿", "きでん", "Very formal written (Your Honor)"),
    ("閣下", "かっか", "Your Excellency (diplomatic/military)"),
    ("陛下", "へいか", "Your Majesty (royalty)"),
    ("殿下", "でんか", "Your Highness (royalty)"),
    (
        "妃殿下",
        "ひでんか",
        "Her Royal Highness (princess consort)",
    ),
    ("親王", "しんのう", "Prince of the Blood (Imperial family)"),
    (
        "内親王",
        "ないしんのう",
        "Princess of the Blood (Imperial family)",
    ),
    ("宮", "みや", "Prince/Princess (Imperial branch family)"),
    ("上", "うえ", "Archaic superior address (e.g. 父上)"),
    ("公", "こう", "Duke / Lord (nobility)"),
    (
        "卿",
        "きょう",
        "Lord (archaic nobility, also used in fantasy)",
    ),
    ("侯", "こう", "Marquis (nobility)"),
    ("伯", "はく", "Count/Earl (nobility)"),
    ("子", "し", "Viscount (nobility) / Master (classical)"),
    ("男", "だん", "Baron (nobility)"),
    // ===== Casual / Friendly =====
    ("君", "くん", "Familiar suffix (usually male, junior)"),
    ("くん", "くん", "Kana form of 君 — familiar (usually male)"),
    (
        "ちゃん",
        "ちゃん",
        "Endearing suffix (children, close friends, girls)",
    ),
    ("たん", "たん", "Baby-talk version of ちゃん"),
    ("ちん", "ちん", "Cutesy/playful variant of ちゃん"),
    ("ちょ", "ちょ", "Playful/cutesy clipped nickname suffix"),
    ("ちょる", "ちょる", "Playful/cutesy nickname suffix"),
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
    (
        "お嬢様",
        "おじょうさま",
        "Young lady (very polite/rich girl)",
    ),
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
    (
        "生徒",
        "せいと",
        "Student (used as address in some contexts)",
    ),
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
    (
        "親方",
        "おやかた",
        "Stable master (sumo) / Boss (craftsman)",
    ),
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
    (
        "ご主人様",
        "ごしゅじんさま",
        "Master (very polite, maid usage)",
    ),
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

/// Split a Japanese name on the first space (internal helper).
/// Returns (family, given, combined, original, has_space)
fn split_japanese_name(name_original: &str) -> JapaneseNameParts {
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

/// Generate hiragana readings for a name using positional romaji mapping (internal helper).
///
/// For each name part (family, given) independently:
/// - If part contains kanji → convert corresponding romanized part via alphabet_to_kana
/// - If part is kana only → use kata_to_hira directly on the Japanese text
///
/// IMPORTANT: Romanized names from VNDB are Western order ("Given Family").
/// Japanese names are Japanese order ("Family Given").
/// romanized_parts[0] maps to Japanese family; romanized_parts[1] maps to Japanese given.
fn generate_mixed_name_readings(name_original: &str, romanized_name: &str) -> NameReadings {
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
        if kana::contains_kanji(name_original) {
            // Has kanji — use romanized reading
            let full = kana::alphabet_to_kana(romanized_name);
            return NameReadings {
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        } else {
            // Pure kana — use kata_to_hira on the Japanese text itself
            let full = kana::kata_to_hira(&name_original.replace(' ', ""));
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

    let family_has_kanji = kana::contains_kanji(family_jp);
    let given_has_kanji = kana::contains_kanji(given_jp);

    // Split romanized name (Western order: first_word second_word)
    let rom_parts: Vec<&str> = romanized_name.splitn(2, ' ').collect();
    let rom_first = rom_parts.first().copied().unwrap_or(""); // romanized_parts[0]
    let rom_second = rom_parts.get(1).copied().unwrap_or(""); // romanized_parts[1]

    // Family reading: if kanji, use rom_first (romanized_parts[0]) via alphabet_to_kana
    //                 if kana, use Japanese family text via kata_to_hira
    let family_reading = if family_has_kanji {
        kana::alphabet_to_kana(rom_first)
    } else {
        kana::kata_to_hira(family_jp)
    };

    // Given reading: if kanji, use rom_second (romanized_parts[1]) via alphabet_to_kana
    //                if kana, use Japanese given text via kata_to_hira
    let given_reading = if given_has_kanji {
        kana::alphabet_to_kana(rom_second)
    } else {
        kana::kata_to_hira(given_jp)
    };

    let full_reading = format!("{}{}", family_reading, given_reading);

    NameReadings {
        full: full_reading,
        family: family_reading,
        given: given_reading,
    }
}

/// Split a Japanese name into family/given parts.
///
/// This is the primary name splitting function. It accepts optional romaji hints
/// from AniList (first=given, last=family in Western order) to split native names
/// that have no space separator.
///
/// Behavior:
/// - Native has space → splits on space (hints ignored for splitting, used for readings)
/// - Native has no space + both hints provided → uses reading lengths to find split point
/// - Native has no space + missing hints → returns as single unsplit block
/// - Katakana with middle dot (・) → never split (foreign names)
///
/// For VNDB characters, pass `None` for both hints — falls back to space-based splitting.
/// For AniList characters, pass `first_name_hint` (given) and `last_name_hint` (family).
pub fn split_japanese_name_with_hints(
    name_original: &str,
    first_name_hint: Option<&str>,
    last_name_hint: Option<&str>,
) -> JapaneseNameParts {
    // If native already has a space, use the existing split logic
    if name_original.contains(' ') {
        return split_japanese_name(name_original);
    }

    // Trim hints and treat empty as None
    let first = first_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());
    let last = last_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());

    // Need both hints to attempt a split on a spaceless name
    let (first_hint, last_hint) = match (first, last) {
        (Some(f), Some(l)) => (f, l),
        _ => return split_japanese_name(name_original),
    };

    // Don't try to split katakana names with middle dots (foreign names)
    if name_original.contains('・') {
        return split_japanese_name(name_original);
    }

    // Convert hints to kana to estimate character boundaries
    let family_kana = kana::alphabet_to_kana(last_hint);
    let given_kana = kana::alphabet_to_kana(first_hint);

    // Try to find the split point in the native name.
    // Strategy: the family name comes first in Japanese order.
    // We try to find where the family portion ends by matching
    // kana/kanji character counts against the family reading length.
    if let Some((family_str, given_str)) =
        find_split_point(name_original, &family_kana, &given_kana)
    {
        let combined = format!("{}{}", family_str, given_str);
        JapaneseNameParts {
            has_space: false,
            original: name_original.to_string(),
            combined,
            family: Some(family_str),
            given: Some(given_str),
        }
    } else {
        // Couldn't determine split point — return as single block
        split_japanese_name(name_original)
    }
}

/// Try to find where to split a spaceless Japanese name into family/given parts.
///
/// Uses the kana readings of family and given names to estimate the character
/// boundary. For mixed kanji/kana names, detects the transition point between
/// kanji and kana characters as a likely boundary.
fn find_split_point(native: &str, family_kana: &str, given_kana: &str) -> Option<(String, String)> {
    let chars: Vec<char> = native.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let family_kana_len = family_kana.chars().count();
    let given_kana_len = given_kana.chars().count();

    // Strategy 1: Look for a kanji→kana transition point.
    // Many Japanese names have kanji family + kana given (e.g., 薙切えりな).
    // Find the first transition from kanji to non-kanji (hiragana/katakana).
    let mut last_kanji_idx = None;
    let mut first_kana_after_kanji = None;
    for (i, &c) in chars.iter().enumerate() {
        if kana::contains_kanji(&c.to_string()) {
            last_kanji_idx = Some(i);
        } else if last_kanji_idx.is_some() && first_kana_after_kanji.is_none() {
            first_kana_after_kanji = Some(i);
        }
    }

    // If we found a kanji→kana boundary, check if the kana portion matches
    // the given name reading length
    if let Some(boundary) = first_kana_after_kanji {
        let candidate_given: String = chars[boundary..].iter().collect();
        let candidate_given_hira = kana::kata_to_hira(&candidate_given);

        // Check if the kana portion matches the given name reading
        if candidate_given_hira.chars().count() == given_kana_len
            || candidate_given_hira == kana::kata_to_hira(given_kana)
        {
            let family_str: String = chars[..boundary].iter().collect();
            return Some((family_str, candidate_given));
        }
    }

    // Strategy 2: For all-kanji names, use the reading lengths to estimate
    // the split point. Each kanji typically maps to 1-3 kana.
    // We try each possible split position and check if the resulting
    // character counts are plausible given the reading lengths.
    let total_chars = chars.len();
    if total_chars < 2 {
        return None;
    }

    // For all-kanji names, try to find the split by testing each position.
    // A kanji character typically produces 1-3 kana. We look for a split
    // where family_chars * avg_kana_per_kanji ≈ family_kana_len.
    let total_kana = family_kana_len + given_kana_len;
    if total_kana == 0 {
        return None;
    }

    // Try each possible split position
    let mut best_split = None;
    let mut best_score = f64::MAX;

    for split_pos in 1..total_chars {
        let family_chars = split_pos;
        let given_chars = total_chars - split_pos;

        // Expected kana per character for each part
        let family_ratio = family_kana_len as f64 / family_chars as f64;
        let given_ratio = given_kana_len as f64 / given_chars as f64;

        // Kanji typically produce 1-3 kana (most commonly 2)
        // Penalize ratios outside this range
        if !(0.5..=4.0).contains(&family_ratio) {
            continue;
        }
        if !(0.5..=4.0).contains(&given_ratio) {
            continue;
        }

        // Score: penalize kana-per-kanji ratios outside the [1.0, 2.0] sweet spot
        // (1 mora per kanji is common in names using ateji like 亜/耶/美,
        //  2 mora per kanji is the most typical on-yomi/kun-yomi length).
        // Use a small balance tie-breaker to prefer more even splits when
        // ratio penalties are equal.
        let family_penalty = if family_ratio < 1.0 {
            1.0 - family_ratio
        } else if family_ratio > 2.0 {
            family_ratio - 2.0
        } else {
            0.0
        };
        let given_penalty = if given_ratio < 1.0 {
            1.0 - given_ratio
        } else if given_ratio > 2.0 {
            given_ratio - 2.0
        } else {
            0.0
        };
        let balance = (family_chars as f64 - given_chars as f64).abs();
        let score = family_penalty + given_penalty + balance * 0.01;

        if score < best_score {
            best_score = score;
            best_split = Some(split_pos);
        }
    }

    if let Some(pos) = best_split {
        let family_str: String = chars[..pos].iter().collect();
        let given_str: String = chars[pos..].iter().collect();
        Some((family_str, given_str))
    } else {
        None
    }
}

/// Find ALL plausible split points for a spaceless Japanese name.
///
/// Returns every split position whose score is within a small epsilon of the
/// best score. For most names only one position wins clearly; for names with
/// symmetric kana lengths (e.g., "石井守" where family and given both have 3
/// kana) multiple tied positions are returned so the caller can generate
/// dictionary entries for all of them.
fn find_all_split_points(
    native: &str,
    family_kana: &str,
    given_kana: &str,
) -> Vec<(String, String)> {
    let chars: Vec<char> = native.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let family_kana_len = family_kana.chars().count();
    let given_kana_len = given_kana.chars().count();

    // Strategy 1: kanji→kana transition (unambiguous — returns at most one result)
    let mut last_kanji_idx = None;
    let mut first_kana_after_kanji = None;
    for (i, &c) in chars.iter().enumerate() {
        if kana::contains_kanji(&c.to_string()) {
            last_kanji_idx = Some(i);
        } else if last_kanji_idx.is_some() && first_kana_after_kanji.is_none() {
            first_kana_after_kanji = Some(i);
        }
    }

    if let Some(boundary) = first_kana_after_kanji {
        let candidate_given: String = chars[boundary..].iter().collect();
        let candidate_given_hira = kana::kata_to_hira(&candidate_given);

        if candidate_given_hira.chars().count() == given_kana_len
            || candidate_given_hira == kana::kata_to_hira(given_kana)
        {
            let family_str: String = chars[..boundary].iter().collect();
            return vec![(family_str, candidate_given)];
        }
    }

    // Strategy 2: ratio-based scoring — collect ALL positions within epsilon of best
    let total_chars = chars.len();
    if total_chars < 2 {
        return Vec::new();
    }

    let total_kana = family_kana_len + given_kana_len;
    if total_kana == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(usize, f64)> = Vec::new();

    for split_pos in 1..total_chars {
        let family_chars = split_pos;
        let given_chars = total_chars - split_pos;

        let family_ratio = family_kana_len as f64 / family_chars as f64;
        let given_ratio = given_kana_len as f64 / given_chars as f64;

        if !(0.5..=4.0).contains(&family_ratio) {
            continue;
        }
        if !(0.5..=4.0).contains(&given_ratio) {
            continue;
        }

        let family_penalty = if family_ratio < 1.0 {
            1.0 - family_ratio
        } else if family_ratio > 2.0 {
            family_ratio - 2.0
        } else {
            0.0
        };
        let given_penalty = if given_ratio < 1.0 {
            1.0 - given_ratio
        } else if given_ratio > 2.0 {
            given_ratio - 2.0
        } else {
            0.0
        };
        let balance = (family_chars as f64 - given_chars as f64).abs();
        let score = family_penalty + given_penalty + balance * 0.01;

        scored.push((split_pos, score));
    }

    if scored.is_empty() {
        return Vec::new();
    }

    let best_score = scored.iter().map(|(_, s)| *s).fold(f64::MAX, f64::min);
    let epsilon = 1e-6;

    scored
        .into_iter()
        .filter(|(_, s)| (s - best_score).abs() < epsilon)
        .map(|(pos, _)| {
            let family_str: String = chars[..pos].iter().collect();
            let given_str: String = chars[pos..].iter().collect();
            (family_str, given_str)
        })
        .collect()
}

/// Return all plausible name splits for a Japanese name, given optional hints.
///
/// Like [`split_japanese_name_with_hints`] but returns multiple candidates when
/// the split is ambiguous (e.g., symmetric kana lengths in an all-kanji name).
/// The dict builder uses this to generate entries for every candidate so that
/// lookup works regardless of which split is correct.
pub fn split_japanese_name_all_candidates(
    name_original: &str,
    first_name_hint: Option<&str>,
    last_name_hint: Option<&str>,
) -> Vec<JapaneseNameParts> {
    // Space, middle-dot, or missing hints → single unambiguous result
    if name_original.contains(' ') || name_original.contains('・') {
        return vec![split_japanese_name(name_original)];
    }

    let first = first_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());
    let last = last_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());

    let (first_hint, last_hint) = match (first, last) {
        (Some(f), Some(l)) => (f, l),
        _ => return vec![split_japanese_name(name_original)],
    };

    let family_kana = kana::alphabet_to_kana(last_hint);
    let given_kana = kana::alphabet_to_kana(first_hint);

    let splits = find_all_split_points(name_original, &family_kana, &given_kana);

    if splits.is_empty() {
        return vec![split_japanese_name(name_original)];
    }

    splits
        .into_iter()
        .map(|(family_str, given_str)| {
            let combined = format!("{}{}", family_str, given_str);
            JapaneseNameParts {
                has_space: false,
                original: name_original.to_string(),
                combined,
                family: Some(family_str),
                given: Some(given_str),
            }
        })
        .collect()
}

/// Generate hiragana readings for a character name.
///
/// This is the primary reading generation function. It accepts optional romaji hints
/// from AniList to correctly map family/given readings even when the native name
/// has no space separator.
///
/// Parameters:
/// - `name_original`: Japanese name (native script)
/// - `romanized_name`: Full romanized name (used as fallback when no hints)
/// - `first_name_hint`: Given name in romaji (AniList "first"), Western order
/// - `last_name_hint`: Family name in romaji (AniList "last"), Western order
///
/// Behavior:
/// - Empty native → empty readings
/// - Katakana with middle dot → kata_to_hira on whole name (no split)
/// - No last hint → single-name treatment
/// - Native has space + hints → space-based split, hints for readings
/// - Native no space + hints → hint-based split and readings
/// - No hints at all → VNDB-style positional romaji mapping
///
/// For VNDB characters, pass `None` for both hints.
/// For AniList characters, pass the `first` and `last` fields from the API.
/// For future sources, pass whatever name hints are available.
pub fn generate_name_readings(
    name_original: &str,
    romanized_name: &str,
    first_name_hint: Option<&str>,
    last_name_hint: Option<&str>,
) -> NameReadings {
    let mut readings = generate_name_readings_inner(
        name_original,
        romanized_name,
        first_name_hint,
        last_name_hint,
    );

    // Strip internal whitespace from readings — romaji→kana conversion can
    // introduce spaces from multi-word romanized names (e.g. "Elaina no Haha"
    // would produce "えらいな の はは" instead of "えらいなのはは").
    readings.full = readings.full.split_whitespace().collect();
    readings.family = readings.family.split_whitespace().collect();
    readings.given = readings.given.split_whitespace().collect();

    readings
}

fn generate_name_readings_inner(
    name_original: &str,
    romanized_name: &str,
    first_name_hint: Option<&str>,
    last_name_hint: Option<&str>,
) -> NameReadings {
    // Handle empty native name
    if name_original.is_empty() {
        return NameReadings {
            full: String::new(),
            family: String::new(),
            given: String::new(),
        };
    }

    // Trim hints and treat empty as None
    let first = first_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());
    let last = last_name_hint.map(|s| s.trim()).filter(|s| !s.is_empty());

    // If no hints provided, fall back to existing behavior
    if first.is_none() && last.is_none() {
        return generate_mixed_name_readings(name_original, romanized_name);
    }

    // Katakana names with middle dot — just convert to hiragana, don't split
    if name_original.contains('・') {
        let full = kana::kata_to_hira(name_original);
        return NameReadings {
            full: full.clone(),
            family: full.clone(),
            given: full,
        };
    }

    // Single-name character (no last name hint)
    if last.is_none() {
        // Only first_name_hint — treat as single name
        if kana::contains_kanji(name_original) {
            let reading = kana::alphabet_to_kana(first.unwrap_or(romanized_name));
            return NameReadings {
                full: reading.clone(),
                family: reading.clone(),
                given: reading,
            };
        } else {
            let full = kana::kata_to_hira(name_original);
            return NameReadings {
                full: full.clone(),
                family: full.clone(),
                given: full,
            };
        }
    }

    // We have at least a last_name_hint (family)
    let last_hint = last.unwrap();
    let first_hint = first.unwrap_or("");

    // If native name has a space, split on it and use hints for readings
    if name_original.contains(' ') {
        let parts = split_japanese_name(name_original);
        let family_jp = parts.family.as_deref().unwrap_or("");
        let given_jp = parts.given.as_deref().unwrap_or("");

        let family_reading = if kana::contains_kanji(family_jp) {
            kana::alphabet_to_kana(last_hint)
        } else {
            kana::kata_to_hira(family_jp)
        };

        let given_reading = if kana::contains_kanji(given_jp) {
            kana::alphabet_to_kana(first_hint)
        } else {
            kana::kata_to_hira(given_jp)
        };

        let full = format!("{}{}", family_reading, given_reading);
        return NameReadings {
            full,
            family: family_reading,
            given: given_reading,
        };
    }

    // No space in native name — use hints to generate readings
    let family_kana = kana::alphabet_to_kana(last_hint);
    let given_kana = if !first_hint.is_empty() {
        kana::alphabet_to_kana(first_hint)
    } else {
        String::new()
    };

    // Try to split the native name using hints
    let parts = split_japanese_name_with_hints(name_original, first_name_hint, last_name_hint);

    if parts.family.is_some() && parts.given.is_some() {
        let family_jp = parts.family.as_deref().unwrap();
        let given_jp = parts.given.as_deref().unwrap();

        // For each part: if it's kana, use it directly; if kanji, use hint reading
        let family_reading = if kana::contains_kanji(family_jp) {
            family_kana
        } else {
            kana::kata_to_hira(family_jp)
        };

        let given_reading = if kana::contains_kanji(given_jp) {
            given_kana
        } else {
            kana::kata_to_hira(given_jp)
        };

        let full = format!("{}{}", family_reading, given_reading);
        NameReadings {
            full,
            family: family_reading,
            given: given_reading,
        }
    } else {
        // Couldn't split — use hint readings directly
        let full = format!("{}{}", family_kana, given_kana);
        NameReadings {
            full,
            family: family_kana,
            given: given_kana,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let parts = split_japanese_name("A B C");
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("A"));
        assert_eq!(parts.given.as_deref(), Some("B C"));
    }

    #[test]
    fn test_split_japanese_name_middle_dot() {
        let parts = split_japanese_name("ルルーシュ・ランペルージ");
        assert!(
            !parts.has_space,
            "Middle dot should not be treated as space"
        );
        assert_eq!(parts.combined, "ルルーシュ・ランペルージ");
    }

    #[test]
    fn test_split_japanese_name_single_space() {
        let parts = split_japanese_name(" ");
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some(""));
        assert_eq!(parts.given.as_deref(), Some(""));
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
        assert_eq!(r.full, kana::alphabet_to_kana("kan"));
    }

    #[test]
    fn test_mixed_readings_single_kana() {
        let r = generate_mixed_name_readings("あいう", "unused");
        assert_eq!(r.full, "あいう");
    }

    #[test]
    fn test_mixed_readings_single_katakana() {
        let r = generate_mixed_name_readings("アイウ", "unused");
        assert_eq!(r.full, "あいう");
    }

    #[test]
    fn test_mixed_readings_two_part_both_kanji() {
        let r = generate_mixed_name_readings("漢 字", "Given Family");
        assert_eq!(r.family, kana::alphabet_to_kana("given"));
        assert_eq!(r.given, kana::alphabet_to_kana("family"));
    }

    #[test]
    fn test_mixed_readings_two_part_mixed() {
        let r = generate_mixed_name_readings("漢 かな", "Romaji Unused");
        assert_eq!(r.family, kana::alphabet_to_kana("romaji"));
        assert_eq!(r.given, "かな");
    }

    #[test]
    fn test_mixed_readings_two_part_all_kana() {
        let r = generate_mixed_name_readings("あい うえ", "Unused Unused2");
        assert_eq!(r.family, "あい");
        assert_eq!(r.given, "うえ");
        assert_eq!(r.full, "あいうえ");
    }

    #[test]
    fn test_mixed_readings_kanji_with_empty_romanized() {
        let r = generate_mixed_name_readings("漢字", "");
        assert_eq!(r.full, "");
    }

    #[test]
    fn test_mixed_readings_two_part_kanji_single_word_romanized() {
        let r = generate_mixed_name_readings("漢 字", "SingleWord");
        assert_eq!(r.family, kana::alphabet_to_kana("singleword"));
        assert_eq!(r.given, "");
    }

    #[test]
    fn test_mixed_readings_two_part_romanized_has_extra_spaces() {
        let r = generate_mixed_name_readings("漢 字", "Given  Family");
        assert_eq!(r.family, kana::alphabet_to_kana("given"));
        assert_eq!(r.given, kana::alphabet_to_kana(" family"));
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

    // === End-to-end: name reading with apostrophe in romanized name ===

    #[test]
    fn test_mixed_readings_apostrophe_in_romanized() {
        // Character: 岡部 倫太郎, romanized: "Rin'tarou Okabe"
        // (VNDB Western order: Given Family)
        // Family 岡部 has kanji → use rom_first "Rin'tarou" → りんたろう
        // Wait, that's wrong — rom_first is the given name in Western order.
        // Let me use the correct mapping:
        // Japanese: "岡部 倫太郎" (Family Given)
        // Romanized: "Rintarou Okabe" (Given Family)
        // rom_first = "Rintarou" → maps to family (岡部)
        // rom_second = "Okabe" → maps to given (倫太郎)
        // But that's the VNDB name order swap (see agents.md critical detail #1).
        let r = generate_mixed_name_readings("岡部 倫太郎", "Rintarou Okabe");
        assert_eq!(r.family, "りんたろう"); // rom_first for family
        assert_eq!(r.given, "おかべ"); // rom_second for given
    }

    #[test]
    fn test_mixed_readings_apostrophe_disambiguation() {
        // Name with apostrophe: "Shin'ichi Kudou" → しんいち for family reading
        let r = generate_mixed_name_readings("工藤 新一", "Shin'ichi Kudou");
        assert_eq!(r.family, "しんいち"); // Apostrophe correctly produces ん+い
        assert_eq!(r.given, "くどう");
    }

    // === Unified generate_name_readings tests ===

    #[test]
    fn test_name_readings_no_hints_delegates_to_vndb_behavior() {
        // Without hints, should produce the same VNDB-style positional mapping
        let r = generate_name_readings("須々木 心一", "Shinichi Suzuki", None, None);
        // VNDB Western order: rom_first="Shinichi" → family, rom_second="Suzuki" → given
        assert_eq!(r.family, kana::alphabet_to_kana("shinichi"));
        assert_eq!(r.given, kana::alphabet_to_kana("suzuki"));
    }

    #[test]
    fn test_name_readings_empty_native() {
        let r = generate_name_readings("", "Some Name", Some("Some"), Some("Name"));
        assert!(r.full.is_empty());
    }

    #[test]
    fn test_name_readings_hints_with_space() {
        // Native has space + hints → use hints for readings
        let r = generate_name_readings(
            "田所 恵",
            "Megumi Tadokoro",
            Some("Megumi"),
            Some("Tadokoro"),
        );
        assert_eq!(r.family, "たどころ");
        assert_eq!(r.given, "めぐみ");
    }

    #[test]
    fn test_name_readings_hints_no_space_kanji() {
        // All kanji, no space, with hints
        let r = generate_name_readings(
            "幸平創真",
            "Souma Yukihira",
            Some("Souma"),
            Some("Yukihira"),
        );
        assert_eq!(r.family, "ゆきひら");
        assert_eq!(r.given, "そうま");
    }

    #[test]
    fn test_name_readings_hints_katakana_given() {
        // Kanji family + katakana given, no space
        let r = generate_name_readings("薙切アリス", "Alice Nakiri", Some("Alice"), Some("Nakiri"));
        assert_eq!(r.family, "なきり");
        assert_eq!(r.given, "ありす");
    }

    #[test]
    fn test_name_readings_hints_middledot() {
        // Katakana with middle dot — should not split
        let r = generate_name_readings(
            "タクミ・アルディーニ",
            "Takumi Aldini",
            Some("Takumi"),
            Some("Aldini"),
        );
        assert_eq!(r.full, "たくみ・あるでぃーに");
    }

    #[test]
    fn test_name_readings_single_name_only_first() {
        let r = generate_name_readings("ヒミコ", "Himiko", Some("Himiko"), None);
        assert_eq!(r.full, "ひみこ");
    }

    #[test]
    fn test_name_readings_single_kanji_only_first() {
        let r = generate_name_readings("徳蔵", "Tokuzou", Some("Tokuzou"), None);
        assert_eq!(r.full, "とくぞう");
    }

    #[test]
    fn test_name_readings_trims_whitespace() {
        let r =
            generate_name_readings("佐藤昭二", "Shouji Satou", Some("Shouji "), Some(" Satou "));
        assert_eq!(r.family, "さとう");
        assert_eq!(r.given, "しょうじ");
    }

    #[test]
    fn test_name_readings_empty_last_hint_treated_as_none() {
        // Empty last hint → treated as single name
        let r = generate_name_readings(
            "田所の母",
            "Tadokoro no Haha",
            Some("Tadokoro no Haha"),
            Some(""),
        );
        // Empty last → no family, single name behavior
        assert!(!r.full.is_empty());
    }

    // === split_japanese_name_with_hints tests ===

    #[test]
    fn test_split_hints_no_hints_delegates() {
        let parts = split_japanese_name_with_hints("須々木 心一", None, None);
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("須々木"));
        assert_eq!(parts.given.as_deref(), Some("心一"));
    }

    #[test]
    fn test_split_hints_with_space_uses_space() {
        let parts = split_japanese_name_with_hints("千俵 おりえ", Some("Orie"), Some("Sendawara"));
        assert!(parts.has_space);
        assert_eq!(parts.family.as_deref(), Some("千俵"));
        assert_eq!(parts.given.as_deref(), Some("おりえ"));
    }

    #[test]
    fn test_split_hints_middledot_not_split() {
        let parts = split_japanese_name_with_hints(
            "ローランド・シャペル",
            Some("Roland"),
            Some("Chapelle"),
        );
        assert!(!parts.has_space);
        assert_eq!(parts.family, None);
    }

    #[test]
    fn test_split_hints_empty_last_no_split() {
        let parts = split_japanese_name_with_hints("田所の母", Some("Tadokoro no Haha"), Some(""));
        assert!(!parts.has_space);
        assert_eq!(parts.family, None);
    }

    #[test]
    fn test_split_hints_kanji_no_space_produces_parts() {
        let parts = split_japanese_name_with_hints("幸平創真", Some("Souma"), Some("Yukihira"));
        assert!(parts.family.is_some(), "Should produce family part");
        assert!(parts.given.is_some(), "Should produce given part");
        assert_eq!(parts.combined, "幸平創真");
    }

    #[test]
    fn test_split_hints_mixed_kana_kanji() {
        let parts = split_japanese_name_with_hints("薙切えりな", Some("Erina"), Some("Nakiri"));
        assert!(parts.family.is_some());
        assert!(parts.given.is_some());
        // Family should be the kanji part, given should be the kana part
        assert_eq!(parts.family.as_deref(), Some("薙切"));
        assert_eq!(parts.given.as_deref(), Some("えりな"));
    }

    // === find_split_point tests ===

    #[test]
    fn test_find_split_kanji_kana_boundary() {
        // 薙切アリス → family=薙切, given=アリス
        let result = find_split_point("薙切アリス", "なきり", "ありす");
        assert!(result.is_some());
        let (family, given) = result.unwrap();
        assert_eq!(family, "薙切");
        assert_eq!(given, "アリス");
    }

    #[test]
    fn test_find_split_all_kanji() {
        // 幸平創真 → family=幸平, given=創真
        let result = find_split_point("幸平創真", "ゆきひら", "そうま");
        assert!(result.is_some());
        let (family, given) = result.unwrap();
        assert_eq!(family, "幸平");
        assert_eq!(given, "創真");
    }

    #[test]
    fn test_find_split_single_char() {
        // Single character — can't split
        let result = find_split_point("漢", "かん", "");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_split_empty() {
        let result = find_split_point("", "かん", "じ");
        assert!(result.is_none());
    }

    // === Regression: "石井守" (Mamoru Ishii) symmetric kana-length split ===

    #[test]
    fn test_find_split_ishii_mamoru() {
        // "石井守": family_kana=いしい (3), given_kana=まもる (3)
        // Split positions 1 and 2 produce identical ratio scores.
        // find_split_point returns the first (split_pos=1 → "石"+"井守").
        // That's fine — find_all_split_points covers both.
        let result = find_split_point("石井守", "いしい", "まもる");
        assert!(result.is_some(), "Should find a split point for 石井守");
    }

    #[test]
    fn test_find_all_splits_ishii_mamoru_returns_both() {
        // Symmetric kana lengths → both split positions are equally plausible.
        // find_all_split_points must return both so the dict covers both.
        let results = find_all_split_points("石井守", "いしい", "まもる");
        assert_eq!(results.len(), 2, "Should return two tied splits");

        let families: Vec<&str> = results.iter().map(|(f, _)| f.as_str()).collect();
        let givens: Vec<&str> = results.iter().map(|(_, g)| g.as_str()).collect();
        assert!(
            families.contains(&"石"),
            "Should include 1-char family split"
        );
        assert!(
            families.contains(&"石井"),
            "Should include 2-char family split"
        );
        assert!(
            givens.contains(&"井守"),
            "Should include 2-char given split"
        );
        assert!(givens.contains(&"守"), "Should include 1-char given split");
    }

    #[test]
    fn test_find_all_splits_unambiguous_returns_one() {
        // 幸平創真: family=ゆきひら (4 kana), given=そうま (3 kana)
        // Split pos=2 clearly wins — only one result expected.
        let results = find_all_split_points("幸平創真", "ゆきひら", "そうま");
        assert_eq!(
            results.len(),
            1,
            "Unambiguous split should return one result"
        );
        assert_eq!(results[0].0, "幸平");
        assert_eq!(results[0].1, "創真");
    }

    #[test]
    fn test_find_all_splits_kanji_kana_boundary_returns_one() {
        // 薙切アリス: kanji→kana boundary is unambiguous (Strategy 1)
        let results = find_all_split_points("薙切アリス", "なきり", "ありす");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "薙切");
        assert_eq!(results[0].1, "アリス");
    }

    #[test]
    fn test_split_all_candidates_ishii_mamoru() {
        // End-to-end: both candidates generated from hints
        let candidates =
            split_japanese_name_all_candidates("石井守", Some("Mamoru"), Some("Ishii"));
        assert_eq!(candidates.len(), 2, "Should produce two candidates");

        let families: Vec<Option<&str>> = candidates.iter().map(|c| c.family.as_deref()).collect();
        assert!(families.contains(&Some("石")));
        assert!(families.contains(&Some("石井")));
    }

    #[test]
    fn test_split_all_candidates_with_space_returns_one() {
        // Space in native → single unambiguous split
        let candidates =
            split_japanese_name_all_candidates("田所 恵", Some("Megumi"), Some("Tadokoro"));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].family.as_deref(), Some("田所"));
    }

    #[test]
    fn test_split_hints_ishii_mamoru() {
        // split_japanese_name_with_hints still returns a single result (first candidate)
        let parts = split_japanese_name_with_hints("石井守", Some("Mamoru"), Some("Ishii"));
        assert!(parts.family.is_some(), "Should produce family part");
        assert!(parts.given.is_some(), "Should produce given part");
        assert_eq!(parts.combined, "石井守");
    }

    #[test]
    fn test_name_readings_ishii_mamoru() {
        // Readings are correct regardless of which split wins (they come from hints)
        let r = generate_name_readings("石井守", "Mamoru Ishii", Some("Mamoru"), Some("Ishii"));
        assert_eq!(r.family, "いしい", "Family reading should be いしい");
        assert_eq!(r.given, "まもる", "Given reading should be まもる");
        assert_eq!(r.full, "いしいまもる");
    }

    // ===== Additional comprehensive tests =====

    // --- split_japanese_name_all_candidates edge cases ---

    #[test]
    fn test_all_candidates_empty_name() {
        let candidates = split_japanese_name_all_candidates("", Some("A"), Some("B"));
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].family.is_none());
    }

    #[test]
    fn test_all_candidates_no_hints() {
        let candidates = split_japanese_name_all_candidates("田中太郎", None, None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].family.is_none()); // no space, no hints → unsplit
    }

    #[test]
    fn test_all_candidates_with_space() {
        let candidates =
            split_japanese_name_all_candidates("田中 太郎", Some("Tarou"), Some("Tanaka"));
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].has_space);
        assert_eq!(candidates[0].family.as_deref(), Some("田中"));
        assert_eq!(candidates[0].given.as_deref(), Some("太郎"));
    }

    #[test]
    fn test_all_candidates_middledot() {
        let candidates = split_japanese_name_all_candidates(
            "ルルーシュ・ランペルージ",
            Some("Lelouch"),
            Some("Lamperouge"),
        );
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].has_space);
    }

    #[test]
    fn test_all_candidates_only_first_hint() {
        let candidates = split_japanese_name_all_candidates("ヒミコ", Some("Himiko"), None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].family.is_none());
    }

    #[test]
    fn test_all_candidates_only_last_hint() {
        let candidates = split_japanese_name_all_candidates("田中", None, Some("Tanaka"));
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].family.is_none());
    }

    #[test]
    fn test_all_candidates_single_char_name() {
        let candidates = split_japanese_name_all_candidates("守", Some("Mamoru"), Some(""));
        assert_eq!(candidates.len(), 1);
    }

    // --- generate_name_readings: comprehensive scenarios ---

    #[test]
    fn test_readings_vndb_two_part_kanji() {
        // VNDB style: no hints, romanized name in Western order
        let r = generate_name_readings("岡部 倫太郎", "Okabe Rintarou", None, None);
        assert_eq!(r.family, "おかべ");
        assert_eq!(r.given, "りんたろう");
        assert_eq!(r.full, "おかべりんたろう");
    }

    #[test]
    fn test_readings_vndb_mixed_kanji_kana() {
        // Family is kanji, given is kana
        let r = generate_name_readings("千俵 おりえ", "Senpyou Orie", None, None);
        assert_eq!(r.family, "せんぴょう");
        assert_eq!(r.given, "おりえ"); // kana → kata_to_hira (already hiragana)
    }

    #[test]
    fn test_readings_vndb_pure_katakana() {
        let r = generate_name_readings("セイバー", "Saber", None, None);
        assert_eq!(r.full, "せいばー");
    }

    #[test]
    fn test_readings_anilist_with_hints_spaced() {
        let r = generate_name_readings(
            "岡部 倫太郎",
            "Okabe Rintarou",
            Some("Rintarou"),
            Some("Okabe"),
        );
        assert_eq!(r.family, "おかべ");
        assert_eq!(r.given, "りんたろう");
    }

    #[test]
    fn test_readings_anilist_with_hints_no_space() {
        let r = generate_name_readings(
            "幸平創真",
            "Souma Yukihira",
            Some("Souma"),
            Some("Yukihira"),
        );
        assert_eq!(r.family, "ゆきひら");
        assert_eq!(r.given, "そうま");
        assert_eq!(r.full, "ゆきひらそうま");
    }

    #[test]
    fn test_readings_katakana_middledot_with_hints() {
        // Foreign name with middle dot — should not split, just convert to hiragana
        let r = generate_name_readings(
            "ルルーシュ・ランペルージ",
            "Lelouch Lamperouge",
            Some("Lelouch"),
            Some("Lamperouge"),
        );
        assert_eq!(r.full, "るるーしゅ・らんぺるーじ");
    }

    #[test]
    fn test_readings_single_name_with_first_hint_only() {
        let r = generate_name_readings("ヒミコ", "Himiko", Some("Himiko"), None);
        assert_eq!(r.full, "ひみこ");
    }

    #[test]
    fn test_readings_single_kanji_with_first_hint_only() {
        let r = generate_name_readings("守", "Mamoru", Some("Mamoru"), None);
        assert_eq!(r.full, "まもる");
    }

    #[test]
    fn test_readings_empty_hints_treated_as_none() {
        let r = generate_name_readings("岡部 倫太郎", "Okabe Rintarou", Some(""), Some(""));
        // Empty hints → treated as None → falls back to VNDB behavior
        assert_eq!(r.family, "おかべ");
        assert_eq!(r.given, "りんたろう");
    }

    #[test]
    fn test_readings_whitespace_only_hints_treated_as_none() {
        let r = generate_name_readings("岡部 倫太郎", "Okabe Rintarou", Some("  "), Some("  "));
        assert_eq!(r.family, "おかべ");
        assert_eq!(r.given, "りんたろう");
    }

    // --- split_japanese_name_with_hints: more edge cases ---

    #[test]
    fn test_split_hints_three_kanji_name() {
        // 3 kanji, family=2 kanji, given=1 kanji
        let parts = split_japanese_name_with_hints("田中太", Some("Futo"), Some("Tanaka"));
        assert_eq!(parts.combined, "田中太");
        // Should attempt to split based on kana lengths
    }

    #[test]
    fn test_split_hints_all_hiragana_no_space() {
        let parts = split_japanese_name_with_hints("たなかたろう", Some("Tarou"), Some("Tanaka"));
        // Hiragana names can still be split using kana length hints
        assert_eq!(parts.combined, "たなかたろう");
    }

    #[test]
    fn test_split_hints_katakana_no_middledot() {
        // Katakana without middle dot — should attempt split
        let parts =
            split_japanese_name_with_hints("セイバーアルトリア", Some("Artoria"), Some("Saber"));
        assert_eq!(parts.combined, "セイバーアルトリア");
    }

    // --- Honorific suffixes data validation ---

    #[test]
    fn test_honorific_suffixes_all_have_three_fields() {
        for (display, reading, desc) in HONORIFIC_SUFFIXES {
            assert!(!display.is_empty(), "Display form should not be empty");
            assert!(!reading.is_empty(), "Reading should not be empty");
            assert!(!desc.is_empty(), "Description should not be empty");
        }
    }

    #[test]
    fn test_honorific_suffixes_contain_san_kun_chan() {
        let displays: Vec<&str> = HONORIFIC_SUFFIXES.iter().map(|(d, _, _)| *d).collect();
        assert!(displays.contains(&"さん"), "Missing さん");
        assert!(displays.contains(&"君"), "Missing 君");
        assert!(displays.contains(&"ちゃん"), "Missing ちゃん");
        assert!(displays.contains(&"ちょ"), "Missing ちょ");
        assert!(displays.contains(&"ちょる"), "Missing ちょる");
        assert!(displays.contains(&"様"), "Missing 様");
        assert!(displays.contains(&"先生"), "Missing 先生");
        assert!(displays.contains(&"先輩"), "Missing 先輩");
    }

    #[test]
    fn test_honorific_suffixes_unique_display_reading_desc_triple() {
        // Some honorifics share display+reading but have different descriptions
        // (e.g., 長官 appears in multiple categories). Verify the full triple is unique.
        let mut seen = std::collections::HashSet::new();
        for (display, reading, desc) in HONORIFIC_SUFFIXES {
            let key = format!("{}:{}:{}", display, reading, desc);
            assert!(
                seen.insert(key.clone()),
                "Duplicate honorific triple: {}",
                key
            );
        }
    }

    #[test]
    fn test_honorific_count_is_substantial() {
        // AGENTS.md says 257 entries across 14 categories
        assert!(
            HONORIFIC_SUFFIXES.len() > 200,
            "Expected 200+ honorifics, got {}",
            HONORIFIC_SUFFIXES.len()
        );
    }

    // --- Real-world AniList character name scenarios ---

    #[test]
    fn test_readings_anilist_emiya_shirou() {
        let r = generate_name_readings("衛宮士郎", "Shirou Emiya", Some("Shirou"), Some("Emiya"));
        assert_eq!(r.family, "えみや");
        assert_eq!(r.given, "しろう");
    }

    #[test]
    fn test_readings_anilist_toosaka_rin() {
        let r = generate_name_readings("遠坂 凛", "Rin Toosaka", Some("Rin"), Some("Toosaka"));
        assert_eq!(r.family, "とおさか");
        assert_eq!(r.given, "りん");
    }

    #[test]
    fn test_readings_anilist_matou_sakura() {
        let r = generate_name_readings("間桐 桜", "Sakura Matou", Some("Sakura"), Some("Matou"));
        assert_eq!(r.family, "まとう");
        assert_eq!(r.given, "さくら");
    }

    #[test]
    fn test_readings_vndb_apostrophe_name() {
        // VNDB romanized name with apostrophe for n-disambiguation
        let r = generate_name_readings("須々木 心一", "Suzuki Shin'ichi", None, None);
        assert_eq!(r.family, "すずき");
        assert_eq!(r.given, "しんいち");
    }

    #[test]
    fn test_split_preserves_original_text() {
        let parts = split_japanese_name_with_hints("岡部 倫太郎", None, None);
        assert_eq!(parts.original, "岡部 倫太郎");
        assert_eq!(parts.combined, "岡部倫太郎");
        assert!(parts.has_space);
    }

    #[test]
    fn test_split_no_space_no_hints_returns_unsplit() {
        let parts = split_japanese_name_with_hints("岡部倫太郎", None, None);
        assert!(!parts.has_space);
        assert!(parts.family.is_none());
        assert!(parts.given.is_none());
        assert_eq!(parts.original, "岡部倫太郎");
        assert_eq!(parts.combined, "岡部倫太郎");
    }

    // === Regression: "小湊亜耶" (Kominato Aya) split at wrong position ===

    #[test]
    fn test_split_kominato_aya() {
        // 小湊亜耶: family=小湊 (こみなと, 4 kana), given=亜耶 (あや, 2 kana)
        // Bug: was splitting as 小湊亜 + 耶 instead of 小湊 + 亜耶
        let result = find_split_point("小湊亜耶", "こみなと", "あや");
        assert!(result.is_some());
        let (family, given) = result.unwrap();
        assert_eq!(family, "小湊", "Family should be 小湊, not 小湊亜");
        assert_eq!(given, "亜耶", "Given should be 亜耶, not 耶");
    }

    #[test]
    fn test_split_hints_kominato_aya() {
        let parts = split_japanese_name_with_hints("小湊亜耶", Some("Aya"), Some("Kominato"));
        assert_eq!(
            parts.family.as_deref(),
            Some("小湊"),
            "Family should be 小湊"
        );
        assert_eq!(parts.given.as_deref(), Some("亜耶"), "Given should be 亜耶");
    }

    #[test]
    fn test_readings_kominato_aya() {
        let r = generate_name_readings("小湊亜耶", "Aya Kominato", Some("Aya"), Some("Kominato"));
        assert_eq!(r.family, "こみなと", "Family reading should be こみなと");
        assert_eq!(r.given, "あや", "Given reading should be あや");
        assert_eq!(r.full, "こみなとあや");
    }

    #[test]
    fn test_readings_consistency_with_and_without_space() {
        // With space (VNDB style)
        let r1 = generate_name_readings("岡部 倫太郎", "Okabe Rintarou", None, None);
        // With hints (AniList style)
        let r2 = generate_name_readings(
            "岡部 倫太郎",
            "Okabe Rintarou",
            Some("Rintarou"),
            Some("Okabe"),
        );
        // Both should produce the same readings
        assert_eq!(r1.family, r2.family);
        assert_eq!(r1.given, r2.given);
        assert_eq!(r1.full, r2.full);
    }

    // === Reading whitespace stripping ===

    #[test]
    fn test_readings_have_no_internal_whitespace() {
        // Multi-word romanized names should not produce readings with spaces
        // e.g. "Elaina no Haha" should produce "えれいなのはは", not "えれいな の はは"
        let readings = generate_name_readings("イレイナの母", "Elaina no Haha", None, None);
        assert!(
            !readings.full.contains(' '),
            "Full reading should not contain spaces, got: '{}'",
            readings.full
        );
        assert!(
            !readings.family.contains(' '),
            "Family reading should not contain spaces, got: '{}'",
            readings.family
        );
        assert!(
            !readings.given.contains(' '),
            "Given reading should not contain spaces, got: '{}'",
            readings.given
        );
    }

    #[test]
    fn test_readings_with_hints_no_whitespace() {
        // With hints, multi-word hints should also not produce spaces
        let readings = generate_name_readings(
            "須々木 心一",
            "Shinichi Suzuki",
            Some("Shinichi"),
            Some("Suzuki"),
        );
        assert!(
            !readings.full.contains(' '),
            "Full reading should not contain spaces, got: '{}'",
            readings.full
        );
    }
}
