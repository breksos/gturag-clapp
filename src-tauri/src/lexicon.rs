//! What a query is asking for, before any document is scored.
//!
//! Three things a ranking cannot recover on its own:
//!
//! * **The education level.** "yüksek lisans" is two words, and one of them, `lisans`, is
//!   also the undergraduate programme. Scored as words, a graduate student's question
//!   matches every undergraduate title; read as a level, it is a preference.
//! * **Filler.** `istiyorum`, `öğrencisiyim`, `I need to` carry intent but no topic, and each
//!   one dilutes what the title signal measures.
//! * **Language.** The corpus is Turkish. An English question shares no word with it, so a
//!   glossary of this domain's nouns gives the lexical half something to match; the dense
//!   half reads the question as written.
//!
//! This is the one module that knows the DOMAIN, a university's paperwork. A fork over a
//! different registry edits these lists and nothing else.

use crate::index::{ascii_fold, tr_fold};
use crate::meta::Level;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// What the lexical half scores: the query without its level and filler words, plus the
    /// Turkish rendering of an English query's nouns.
    pub lexical: String,
    /// The education level the query names, if exactly one.
    pub level: Option<Level>,
    pub english: bool,
    /// A kind of question this archive does not answer.
    pub scope: Option<Scope>,
}

/// Questions about things a document registry does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Academic calendars, registration and exam dates, timetables.
    Calendar,
    /// When a board or committee meets.
    Meeting,
    /// Current announcements.
    Announcement,
}

/// Words that say what the person wants to DO with a topic, not what the topic is.
const FILLERS: &[&str] = &[
    // Turkish, written as `ascii_fold` leaves them
    "istiyorum", "istiyoruz", "istiyor", "isterim", "istedim", "yapmak", "yapmam", "yapmaliyim",
    "yapabilirim", "yapacagim", "etmek", "etmem", "olmak", "nasil", "nereden", "nerede", "nereye",
    "hangi", "hangisi", "gereken", "gerekli", "gerekiyor", "gerek", "lazim", "icin", "ile", "ve",
    "veya", "ya", "da", "de", "bir", "bu", "su", "ben", "benim", "bana", "beni", "biz", "ne",
    "neler", "nedir", "mi", "mu", "olarak", "olan", "var", "yok", "ogrencisiyim", "ogrenciyim",
    "acaba", "lutfen", "hakkinda", "ilgili", "konusunda", "bilgi", "almak",
    // English
    "i", "im", "need", "needs", "to", "my", "me", "the", "a", "an", "for", "of", "how", "do",
    "does", "want", "where", "is", "are", "what", "which", "can", "could", "with", "in", "on",
    "at", "please", "about", "get", "find", "should", "would", "it", "this", "that", "and", "or",
];

/// Words only English uses — the evidence that a query is English.
const EN_FUNCTION: &[&str] = &[
    "i", "need", "to", "my", "the", "an", "for", "of", "how", "do", "want", "where", "is",
    "what", "which", "can", "with", "please", "should", "me",
];

/// English nouns of this domain, as the Turkish TITLES spell them — `konusu`, not `konu`,
/// because a five-letter stem is what matching reaches and `konu` is shorter than that.
const GLOSSARY: &[(&str, &str)] = &[
    ("thesis", "tez"), ("dissertation", "tez"), ("topic", "konusu"), ("subject", "konusu"),
    ("advisor", "danışman"), ("adviser", "danışman"), ("supervisor", "danışman"),
    ("change", "değişikliği"), ("changing", "değişikliği"),
    ("register", "kayıt"), ("registration", "kayıt"), ("enroll", "kayıt"), ("enrollment", "kayıt"),
    ("enrolment", "kayıt"), ("internship", "staj"), ("course", "ders"), ("courses", "ders"),
    ("exam", "sınav"), ("examination", "sınav"), ("excuse", "mazeret"), ("excused", "mazeretli"),
    ("graduation", "mezuniyet"), ("notification", "bildirim"), ("notify", "bildirim"),
    ("declaration", "beyan"), ("declare", "beyan"), ("application", "başvuru"), ("apply", "başvuru"),
    ("extension", "uzatma"), ("freeze", "dondurma"), ("suspension", "dondurma"),
    ("scholarship", "burs"), ("defense", "savunma"), ("defence", "savunma"), ("jury", "jüri"),
    ("proposal", "öneri"), ("progress", "izleme"), ("monitoring", "izleme"), ("committee", "komite"),
    ("student", "öğrenci"), ("certificate", "belgesi"), ("petition", "dilekçe"),
    ("transcript", "transkript"), ("withdrawal", "ilişik"), ("leave", "izin"), ("permission", "izin"),
    ("approval", "onay"), ("report", "raporu"), ("plagiarism", "intihal"), ("similarity", "benzerlik"),
    ("publication", "yayın"), ("credit", "kredi"), ("minor", "yandal"), ("exemption", "muafiyet"),
    ("equivalence", "intibak"), ("guest", "misafir"), ("research", "araştırma"), ("project", "proje"),
    ("ethics", "etik"), ("consent", "onam"), ("laboratory", "laboratuvar"), ("lab", "laboratuvar"),
    ("device", "cihaz"), ("instruction", "talimatı"), ("instructions", "talimatı"),
    ("workflow", "iş akışı"), ("regulation", "yönetmelik"), ("directive", "yönerge"),
    ("guideline", "kılavuz"), ("guide", "kılavuz"), ("senate", "senato"), ("institute", "enstitü"),
    ("faculty", "fakülte"), ("department", "bölüm"), ("ai", "yapay zeka"), ("artificial", "yapay"),
    ("intelligence", "zeka"), ("english", "ingilizce"), ("proficiency", "yeterlik"),
    ("qualifying", "yeterlik"), ("survey", "anket"), ("policy", "politika"), ("meeting", "toplantı"),
    ("minutes", "tutanak"), ("appeal", "itiraz"), ("objection", "itiraz"), ("grade", "not"),
    ("military", "askerlik"), ("deferment", "erteleme"), ("form", "form"),
];

/// Words that say what KIND of paper a document is — an application, a form, a petition —
/// rather than what it is about. They are in hundreds of titles, and a question that pairs
/// one with a topic (`staj başvurusu`) is about the topic: weighed like the topic, `başvuru`
/// ranks every other application above the internship documents.
const GENERIC: &[&str] = &[
    "basvuru", "basvurusu", "basvurulari", "form", "formu", "formlari", "formun", "dilekce",
    "dilekcesi", "talep", "talebi", "belge", "belgesi", "belgeler", "tutanak", "tutanagi",
    "rapor", "raporu", "anket", "anketi", "bilgi", "bilgileri", "onay", "onayi", "beyan",
    "beyani", "bildirim", "bildirimi", "islem", "islemleri", "surec", "sureci",
];

/// Is this (Turkish-folded) word a document-kind word?
pub fn is_generic(word: &str) -> bool {
    GENERIC.contains(&ascii_fold(word).as_str())
}

/// Is this an English word the glossary renders in Turkish? Its Turkish rendering stands for
/// it, so it is not a separate word the archive failed to contain.
pub fn has_gloss(word: &str) -> bool {
    gloss(&ascii_fold(word)).is_some()
}

fn gloss(word: &str) -> Option<&'static str> {
    GLOSSARY.iter().find(|(en, _)| *en == word).map(|(_, tr)| *tr)
}

pub fn analyze(query: &str) -> Analysis {
    let folded = tr_fold(query);
    let words: Vec<&str> = folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let flat: Vec<String> = words.iter().map(|w| ascii_fold(w)).collect();

    // Read on the query AS TYPED: folding turns an English capital `I` into a Turkish `ı`,
    // which would make "I need to…" look Turkish.
    let turkish_letters = query.chars().any(|c| "çğıöşüÇĞİÖŞÜ".contains(c));
    let function_words = flat.iter().filter(|w| EN_FUNCTION.contains(&w.as_str())).count();
    let glossary_hits = flat.iter().filter(|w| gloss(w).is_some()).count();
    let english = !turkish_letters
        && (function_words >= 2 || glossary_hits >= 2 || (function_words >= 1 && glossary_hits >= 1));

    let (mut graduate, mut undergraduate) = (false, false);
    let mut kept: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (word, flat_word) = (words[i], flat[i].as_str());
        // `yüksek lisans…` is one concept in two words.
        if flat_word == "yuksek"
            && flat.get(i + 1).is_some_and(|n| n.starts_with("lisans") && !n.starts_with("lisansustu"))
        {
            graduate = true;
            i += 2;
            continue;
        }
        if flat_word.starts_with("lisansustu")
            || matches!(flat_word, "master" | "masters" | "msc" | "graduate" | "postgraduate")
        {
            graduate = true;
        } else if matches!(flat_word, "phd" | "doctoral" | "doctorate") {
            graduate = true;
            kept.push("doktora".into());
        } else if flat_word.starts_with("doktora") || matches!(flat_word, "tezli" | "tezsiz") {
            // A level, AND a topic: `Doktora Yeterlik`, `Tezsiz Yüksek Lisans Başvurusu`.
            graduate = true;
            kept.push(word.to_string());
        } else if (flat_word.starts_with("lisans") && !flat_word.starts_with("lisansl"))
            || matches!(flat_word, "onlisans" | "undergraduate" | "bachelor" | "bachelors" | "bsc")
        {
            undergraduate = true;
        } else if !FILLERS.contains(&flat_word) {
            if english {
                if let Some(tr) = gloss(flat_word) {
                    kept.push(tr.to_string());
                }
            }
            kept.push(word.to_string());
        }
        i += 1;
    }

    // Nothing but level words ("lisansüstü") is still a query about something.
    if kept.is_empty() {
        kept = words
            .iter()
            .zip(&flat)
            .filter(|(_, f)| !FILLERS.contains(&f.as_str()))
            .map(|(w, _)| w.to_string())
            .collect();
    }

    Analysis {
        lexical: kept.join(" "),
        level: match (graduate, undergraduate) {
            (true, false) => Some(Level::Lisansustu),
            (false, true) => Some(Level::Lisans),
            _ => None,
        },
        english,
        scope: scope_of(&flat),
    }
}

/// A question about dates, meetings or announcements: things that live on the university's
/// news pages, not in its document registry. Read in context, because `toplantı` and
/// `duyuru` are also the titles of real forms.
fn scope_of(words: &[String]) -> Option<Scope> {
    let has = |w: &str| words.iter().any(|x| x == w);
    let starts = |p: &str| words.iter().any(|x| x.starts_with(p));
    let text = words.join(" ");

    let period = starts("guz") || starts("bahar") || starts("donem") || starts("yariyil")
        || text.contains("ders kay") || starts("sinav");
    if text.contains("akademik takvim")
        || (starts("takvim") && period)
        || (starts("tarih") && period)
        || text.contains("ders program")
        || text.contains("sinav program")
        || text.contains("academic calendar")
        || ((has("schedule") || has("dates")) && (has("exam") || has("course") || has("registration")))
    {
        return Some(Scope::Calendar);
    }
    let meets = starts("toplan") || has("meet") || has("meets") || has("meeting");
    if meets && (text.contains("hangi gun") || text.contains("ne zaman") || text.contains("what day")
        || text.contains("when does") || text.contains("when is"))
    {
        return Some(Scope::Meeting);
    }
    if text.contains("son duyuru") || text.contains("guncel duyuru") || has("duyurular") || has("announcements") {
        return Some(Scope::Announcement);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuksek_lisans_is_a_level_not_two_words() {
        let a = analyze("yüksek lisansta tez danışmanımı değiştirmek istiyorum");
        assert_eq!(a.level, Some(Level::Lisansustu));
        assert_eq!(a.lexical, "tez danışmanımı değiştirmek");
        assert!(!a.english);
    }

    #[test]
    fn an_undergraduate_question_says_so() {
        let a = analyze("lisans mazeretli ders kaydı");
        assert_eq!(a.level, Some(Level::Lisans));
        assert_eq!(a.lexical, "mazeretli ders kaydı");
    }

    #[test]
    fn filler_is_dropped_and_the_topic_kept() {
        let a = analyze("yüksek lisans öğrencisiyim mazeretli ders kaydı yapmak istiyorum");
        assert_eq!(a.level, Some(Level::Lisansustu));
        assert_eq!(a.lexical, "mazeretli ders kaydı");
    }

    #[test]
    fn an_english_question_gets_turkish_words_to_match() {
        let a = analyze("I need to register my master's thesis topic");
        assert!(a.english);
        assert_eq!(a.level, Some(Level::Lisansustu));
        for w in ["kayıt", "tez", "konusu", "thesis", "topic"] {
            assert!(a.lexical.split(' ').any(|x| x == w), "{w} missing from {:?}", a.lexical);
        }
        let b = analyze("master thesis topic notification form");
        assert!(b.english, "no function word, but four domain nouns");
        assert!(b.lexical.contains("bildirim"));
    }

    #[test]
    fn a_turkish_question_is_never_taken_for_english() {
        assert!(!analyze("danisman degisikligi formu").english);
        assert!(!analyze("staj başvurusu").english);
        assert!(!analyze("yl tez konusu form").english);
    }

    #[test]
    fn a_level_alone_is_still_a_query() {
        let a = analyze("lisansüstü");
        assert_eq!(a.level, Some(Level::Lisansustu));
        assert_eq!(a.lexical, "lisansüstü");
        assert_eq!(analyze("lisans lisansüstü ilişik kesme").level, None, "both is neither");
    }

    #[test]
    fn calendars_and_meeting_days_are_out_of_scope_but_forms_about_them_are_not() {
        assert_eq!(analyze("2026 2027 güz akademik takvim ders kayıt tarihleri yüksek lisans").scope, Some(Scope::Calendar));
        assert_eq!(analyze("Lisansüstü Eğitim Enstitüsü Yönetim Kurulu hangi gün toplanıyor").scope, Some(Scope::Meeting));
        assert_eq!(analyze("toplantı tutanak formu").scope, None);
        assert_eq!(analyze("web sayfası duyuru başvuru formu").scope, None);
        assert_eq!(analyze("mezuniyet tarihi belgesi").scope, None);
        assert_eq!(analyze("proje iş takvimi").scope, None);
    }
}
