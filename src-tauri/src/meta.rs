//! What a document says about ITSELF, beyond what its filename says.
//!
//! The university writes the revision into most filenames (`… R7.pdf`) and leaves it out of
//! others (`İA-0021.pdf`), but nearly every document prints its own header: `Değ. No: 1`,
//! `Değ. Tarihi: 10.06.2024`. Reading that header is the difference between telling a
//! student "revision 0" and "revision 1, 10 June 2024".
//!
//! Derived at load from the index already on disk, so an app update corrects every
//! document's metadata without a corpus rebuild. Pure: text in, fields out.

use crate::corpus::Corpus;
use crate::index::{code_fold, code_key, code_prefix};
use std::collections::HashMap;

/// The education level a document, or a query, is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Önlisans and lisans: the undergraduate programmes.
    Lisans,
    /// Yüksek lisans and doktora: what the graduate institutes run.
    Lisansustu,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match code_fold(s.trim()).as_str() {
            "lisans" | "onlisans" | "undergraduate" | "ug" => Some(Level::Lisans),
            "lisansustu" | "yuksek lisans" | "yukseklisans" | "yl" | "doktora" | "dr"
            | "graduate" | "postgraduate" => Some(Level::Lisansustu),
            _ => None,
        }
    }

    /// The word a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Level::Lisans => "lisans",
            Level::Lisansustu => "lisansüstü",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// The revision the document prints about itself, else its filename's `R<n>`.
    pub rev: u32,
    /// ISO date of that revision, when the document prints one.
    pub rev_date: Option<String>,
    /// ISO date it was first published, when the document prints one.
    pub pub_date: Option<String>,
    /// The unit that prepared it (`Lisansüstü Eğitim Enstitüsü`), when its header names one.
    pub unit: Option<String>,
    /// The collection it is published in: the source's own folder name (`İş Akışları`).
    pub collection: Option<String>,
    pub level: Option<Level>,
}

/// Every document's [`Meta`], in corpus order.
pub fn derive(corpus: &Corpus) -> Vec<Meta> {
    let docs = corpus.docs();
    // A document's header is at the top of its text, which is its first body passage.
    let mut first_body: Vec<&str> = vec![""; docs.len()];
    let mut seen = vec![false; docs.len()];
    for c in corpus.chunks() {
        let d = c.doc as usize;
        if c.ord == 1 && d < docs.len() && !seen[d] {
            seen[d] = true;
            first_body[d] = c.text.split_once('\n').map(|(_, body)| body).unwrap_or("");
        }
    }

    let root = common_dir(docs.iter().map(|d| d.url.as_str()));
    let mut out: Vec<Meta> = docs
        .iter()
        .zip(&first_body)
        .map(|(d, body)| {
            let own = d.code.as_deref().map(code_key);
            let h = header(own.as_deref(), body);
            let unit = unit(body);
            Meta {
                rev: h.rev.unwrap_or(d.rev),
                rev_date: h.rev_date,
                pub_date: h.pub_date,
                level: level_of(&d.title, unit.as_deref()),
                unit,
                collection: collection(&d.url, &root),
            }
        })
        .collect();

    // A document published at the root of the tree has no folder of its own; it belongs to
    // the collection most of its code family lives in.
    let mut by_family: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (d, m) in docs.iter().zip(&out) {
        if let (Some(code), Some(col)) = (d.code.as_deref(), &m.collection) {
            *by_family.entry(code_prefix(&code_key(code))).or_default().entry(col.clone()).or_default() += 1;
        }
    }
    for (d, m) in docs.iter().zip(out.iter_mut()) {
        if m.collection.is_some() {
            continue;
        }
        if let Some(counts) = d.code.as_deref().and_then(|c| by_family.get(&code_prefix(&code_key(c)))) {
            m.collection = counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(k, _)| k.clone());
        }
    }
    out
}

/// The deepest directory every URL shares, with its trailing `/`.
fn common_dir<'a>(urls: impl Iterator<Item = &'a str>) -> String {
    let mut prefix: Option<&str> = None;
    for u in urls {
        prefix = Some(match prefix {
            None => u,
            Some(p) => {
                let end = p
                    .char_indices()
                    .zip(u.chars())
                    .take_while(|((_, a), b)| a == b)
                    .last()
                    .map(|((i, c), _)| i + c.len_utf8())
                    .unwrap_or(0);
                &p[..end]
            }
        });
    }
    let p = prefix.unwrap_or("");
    p.rfind('/').map(|i| p[..=i].to_string()).unwrap_or_default()
}

fn collection(url: &str, root: &str) -> Option<String> {
    let rest = url.strip_prefix(root)?;
    let (folder, _) = rest.split_once('/')?;
    let folder = crate::util::decode_percent(folder.trim());
    (!folder.is_empty()).then_some(folder)
}

struct Header {
    rev: Option<u32>,
    rev_date: Option<String>,
    pub_date: Option<String>,
}

const REV_NO: &[&str] = &[
    "degisiklik no", "deg. no", "deg.no", "deg no", "revizyon no", "rev. no", "rev.no",
    "revision no",
];
const REV_DATE: &[&str] = &[
    "degisiklik tarihi", "deg. tarihi", "deg.tarihi", "deg tarihi", "revizyon tarihi",
    "rev. tarihi", "rev.tarihi", "revision date",
];
const PUB_DATE: &[&str] = &["yayin tarihi", "yayim tarihi", "publication date", "issue date", "date of issue"];

/// The header fields a document prints about itself.
///
/// Most documents carry TWO headers: their own, and the one of the template they were
/// written on — `Form No: FR-0138 Yayın Tarihi: 21.06.2017 Değ. No:0` is the flowchart
/// template, not İA-0021. A `Form No` naming some other code starts a template header, so
/// everything after it on that line is not about this document.
fn header(own: Option<&str>, body: &str) -> Header {
    let mut kept = String::new();
    for line in body.lines().take(60) {
        let folded = code_fold(line);
        let part = match folded.find("form no") {
            Some(at) => match leading_code(&folded[at + "form no".len()..]) {
                Some(code) if Some(code.as_str()) != own => &folded[..at],
                _ => folded.as_str(),
            },
            None => folded.as_str(),
        };
        kept.push_str(part);
        kept.push('\n');
    }
    Header {
        rev: first_after(&kept, REV_NO, number),
        rev_date: first_after(&kept, REV_DATE, date),
        pub_date: first_after(&kept, PUB_DATE, date),
    }
}

/// `: FR-0138 …` → `fr-0138`.
fn leading_code(after: &str) -> Option<String> {
    let rest = after.trim_start_matches([' ', ':', '.', '\t']);
    let raw: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (raw.chars().any(|c| c.is_ascii_digit()) && raw.chars().any(char::is_alphabetic)).then(|| code_key(&raw))
}

/// The value after the EARLIEST label that has one.
fn first_after<T>(text: &str, labels: &[&str], parse: impl Fn(&str) -> Option<T>) -> Option<T> {
    let mut best: Option<(usize, T)> = None;
    for label in labels {
        let mut from = 0;
        while let Some(pos) = text[from..].find(label) {
            let at = from + pos;
            let rest = text[at + label.len()..].trim_start_matches([' ', ':', '.', '\t', '\r', '\n']);
            if let Some(v) = parse(rest) {
                if best.as_ref().map_or(true, |(b, _)| at < *b) {
                    best = Some((at, v));
                }
                break;
            }
            from = at + label.len();
        }
    }
    best.map(|(_, v)| v)
}

fn number(rest: &str) -> Option<u32> {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    digits.parse().ok()
}

fn date(rest: &str) -> Option<String> {
    let raw: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '/' | '-'))
        .collect();
    crate::util::dmy_to_iso(raw.trim_end_matches(['.', '/', '-']))
}

/// The unit a document's `HAZIRLAYAN` block names.
fn unit(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    for (i, line) in lines.iter().enumerate().take(40) {
        let folded = code_fold(line);
        let Some(rest) = folded.strip_prefix("hazirlayan").or_else(|| folded.strip_prefix("prepared by")) else {
            continue;
        };
        let inline = rest.trim_start_matches([' ', ':', '\t']).trim_end();
        let candidate = if inline.is_empty() {
            (*lines[i + 1..].iter().find(|l| !l.is_empty())?).to_string()
        } else {
            // Folding keeps one character per character for Turkish text, so the tail of
            // the folded line is the same length as the tail of the original.
            let n = line.chars().count() - inline.chars().count();
            line.chars().skip(n).collect::<String>().trim().to_string()
        };
        let folded = code_fold(&candidate);
        let label_not_unit = ["onay", "kalite", "yonetim temsilcisi", "imza", "tarih", "adi soyadi"]
            .iter()
            .any(|w| folded.starts_with(w));
        if folded.is_empty() || label_not_unit || candidate.chars().count() > 80 {
            return None;
        }
        return Some(candidate);
    }
    None
}

/// The level a title, or failing that its unit, is about. `None` when it says nothing, or
/// both (`Lisans-Lisansüstü İlişik Kesme Formu`).
pub fn level_of(title: &str, unit: Option<&str>) -> Option<Level> {
    let folded = code_fold(title);
    let words: Vec<&str> = folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let (mut graduate, mut undergraduate) = (false, false);
    for (i, w) in words.iter().enumerate() {
        let prev = if i > 0 { words[i - 1] } else { "" };
        if w.starts_with("lisansustu")
            || w.starts_with("doktora")
            || matches!(*w, "tezli" | "tezsiz" | "phd" | "graduate" | "doctoral")
            || w.starts_with("master")
        {
            graduate = true;
        } else if w.starts_with("lisans") && !w.starts_with("lisansl") {
            if prev == "yuksek" { graduate = true } else { undergraduate = true }
        } else if matches!(*w, "onlisans" | "undergraduate") || w.starts_with("bachelor") {
            undergraduate = true;
        } else if matches!(*w, "yl" | "dr") && (i == 0 || (i == 1 && matches!(words[0], "yl" | "dr"))) {
            // The graduate institute's own prefix: `YL-DR …`, `DR …`. Only at the start —
            // `HACH LANGE DR 3800` is a spectrometer, not a doctorate.
            graduate = true;
        }
    }
    if !graduate && !undergraduate {
        if let Some(u) = unit.map(code_fold) {
            graduate = u.contains("lisansustu") || u.contains("enstitu");
        }
    }
    match (graduate, undergraduate) {
        (true, false) => Some(Level::Lisansustu),
        (false, true) => Some(Level::Lisans),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workflow_reads_its_own_header_and_ignores_its_templates() {
        // İA-0021, as extracted: its own header, then the flowchart template's.
        let body = "Yayın Tarihi: 21.06.2017\nYayın No: 1\nDeğ. Tarihi: 10.06.2024\nDeğ. No: 1\n\
                    İŞ AKIŞ NO: İA-0021 İŞ AKIŞ ADI:\nHAZIRLAYAN\nLisansüstü Eğitim Enstitüsü\n\
                    Kalite Sorumlusu\nForm No: FR-0138 Yayın Tarihi: 21.06.2017 Değ. No:0 Değ. Tarihi:-";
        let h = header(Some("ia-0021"), body);
        assert_eq!(h.rev, Some(1));
        assert_eq!(h.rev_date.as_deref(), Some("2024-06-10"));
        assert_eq!(h.pub_date.as_deref(), Some("2017-06-21"));
        assert_eq!(unit(body).as_deref(), Some("Lisansüstü Eğitim Enstitüsü"));
    }

    #[test]
    fn a_template_header_alone_says_nothing_about_the_document() {
        let body = "Form No: FR-0356 Yayın Tarihi: 23.11.2017 Değ.No:0 Değ.Tarihi:-\nBİRİNCİ BÖLÜM";
        let h = header(Some("yo-0054"), body);
        assert_eq!((h.rev, h.rev_date, h.pub_date), (None, None, None));
    }

    #[test]
    fn a_form_whose_header_names_itself_is_read() {
        let body = "Form No:FR-0083 Yayın Tarihi:21.06.2017 Değ.No:1 Değ.Tarihi:18.12.2023\nI. ÖĞRENCİ BİLGİLERİ";
        let h = header(Some("fr-0083"), body);
        assert_eq!(h.rev, Some(1));
        assert_eq!(h.rev_date.as_deref(), Some("2023-12-18"));
    }

    #[test]
    fn a_regulation_prints_its_revision_without_colons() {
        let body = "Doküman No YÖ-0054\nYayın Tarihi 14.12.2018\nRevizyon Tarihi 13.07.2026\nRevizyon No 7\n\
                    Sayfa 23-1\nForm No: FR-0356 Yayın Tarihi: 23.11.2017 Değ.No:0 Değ.Tarihi:-";
        let h = header(Some("yo-0054"), body);
        assert_eq!(h.rev, Some(7));
        assert_eq!(h.rev_date.as_deref(), Some("2026-07-13"));
        assert_eq!(h.pub_date.as_deref(), Some("2018-12-14"));
    }

    #[test]
    fn levels_come_from_the_title_and_never_from_a_device_name() {
        assert_eq!(level_of("YL-DR Mazeretli Kayıt Formu", None), Some(Level::Lisansustu));
        assert_eq!(level_of("Lisans Mazaretli Ders Kayıt Formu", None), Some(Level::Lisans));
        assert_eq!(level_of("Mühendislik Fakültesi Lisans Mazeretli Ders Kayıt Formu", None), Some(Level::Lisans));
        assert_eq!(level_of("Tezsiz Yüksek Lisans Başvurusu", None), Some(Level::Lisansustu));
        assert_eq!(level_of("Lisans-Lisansüstü İlişik Kesme Formu", None), None, "both is neither");
        assert_eq!(level_of("HACH LANGE DR 3800 UVVIS SPEKTROMETRE KULLANIM TALMATI", None), None);
        assert_eq!(level_of("DR Tez İzleme Komitesi Formu", None), Some(Level::Lisansustu));
        assert_eq!(level_of("Danışman Değişikliği", Some("Lisansüstü Eğitim Enstitüsü")), Some(Level::Lisansustu));
        assert_eq!(level_of("Staj Belgesi", None), None);
    }

    #[test]
    fn the_collection_is_the_folder_under_the_shared_root() {
        let root = common_dir(
            [
                "https://x.invalid/kalite/İş Akışları/İA-0021.pdf",
                "https://x.invalid/kalite/Formlar/Formlar-Türkçe/FR-0083 R1.pdf",
                "https://x.invalid/kalite/FR-0044 Toplantı R2.xls",
            ]
            .into_iter(),
        );
        assert_eq!(root, "https://x.invalid/kalite/");
        assert_eq!(collection("https://x.invalid/kalite/İş Akışları/İA-0021.pdf", &root).as_deref(), Some("İş Akışları"));
        assert_eq!(collection("https://x.invalid/kalite/FR-0044 Toplantı R2.xls", &root), None, "a root file has no folder");
    }
}
