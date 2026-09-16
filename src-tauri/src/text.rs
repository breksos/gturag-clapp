//! A form's extracted text, as something a person or an agent can read.
//!
//! Extraction leaves furniture behind: the page header of every page, `Sayfa 3 / 23`, the
//! template's own `Form No:` line, Word field codes, words split across a line break. None of
//! it is content, and all of it costs trust. This removes what is certainly furniture and
//! leaves everything else exactly as extracted — it never rewrites a sentence.

use crate::corpus::Doc;
use crate::index::{code_fold, code_key};
use crate::live::Live;
use crate::meta::Meta;
use crate::util::iso_to_dmy;
use std::collections::HashMap;

/// The Markdown `get` prints: what the document is, then its text.
pub fn markdown(doc: &Doc, meta: &Meta, live: Option<&Live>, body: &str) -> String {
    let code = doc.code.as_deref().unwrap_or("—");
    let mut out = format!("# {code} · {}\n\n", doc.title);
    if let Some(c) = &meta.collection {
        out.push_str(&format!("- **Type:** {c}\n"));
    }
    let mut revision = format!("{}", meta.rev);
    if let Some(d) = &meta.rev_date {
        revision.push_str(&format!(" of {}", iso_to_dmy(d)));
    }
    if let Some(d) = &meta.pub_date {
        revision.push_str(&format!(" (first published {})", iso_to_dmy(d)));
    }
    out.push_str(&format!("- **Revision:** {revision}\n"));
    if let Some(u) = &meta.unit {
        out.push_str(&format!("- **Prepared by:** {u}\n"));
    }
    if let Some(l) = meta.level {
        out.push_str(&format!("- **Level:** {}\n", l.label()));
    }
    out.push_str(&format!("- **Language:** {}\n", doc.lang));
    out.push_str(&format!("- **Source:** {}\n", doc.url));
    if let Some(l) = live {
        out.push_str(&format!("- **Status:** {}\n", l.sentence()));
    }

    let cleaned = clean(body, doc.code.as_deref());
    if cleaned.is_empty() {
        out.push_str("\n> This document's text could not be extracted; it is indexed by its title alone. Read it at the source.\n");
        return out;
    }
    out.push_str("\n> Text extracted automatically from the published file");
    if looks_garbled(&cleaned) {
        out.push_str(" — this one extracted poorly, with split words and lost layout; read the source for anything that matters");
    } else {
        out.push_str("; the published document is authoritative");
    }
    out.push_str(".\n\n");
    out.push_str(&cleaned);
    out.push('\n');
    out
}

/// Remove extraction furniture, keep the content.
pub fn clean(body: &str, own_code: Option<&str>) -> String {
    let own = own_code.map(code_key);
    let lines: Vec<String> = body
        .lines()
        .map(|l| strip_field_codes(&unescape(l.trim_end())))
        .filter_map(|l| drop_template_header(&l, own.as_deref()))
        .filter(|l| !is_page_furniture(l))
        .collect();

    // A long line printed on most pages is a running header, not content: keep its first
    // appearance only. Short ones (`İmza`, `Tarih`) are form fields and stay.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for l in &lines {
        if l.trim().chars().count() >= 12 {
            *counts.entry(l.trim()).or_default() += 1;
        }
    }
    let mut seen: HashMap<&str, bool> = HashMap::new();
    let mut kept: Vec<String> = Vec::new();
    for l in &lines {
        let t = l.trim();
        if counts.get(t).is_some_and(|n| *n >= 3) {
            if seen.insert(t, true).is_some() {
                continue;
            }
        }
        kept.push(heading(t).unwrap_or_else(|| l.clone()));
    }

    // Rejoin a word hyphenated across a line break: `yapıl-` + `maktadır`.
    let mut joined: Vec<String> = Vec::new();
    for l in kept {
        if let Some(prev) = joined.last_mut() {
            let starts_lower = l.trim_start().chars().next().is_some_and(char::is_lowercase);
            let mut tail = prev.chars().rev();
            let hyphenated = tail.next() == Some('-') && tail.next().is_some_and(char::is_alphabetic);
            if hyphenated && starts_lower {
                prev.pop();
                prev.push_str(l.trim_start());
                continue;
            }
        }
        joined.push(l);
    }

    // At most one blank line in a row.
    let mut out = String::new();
    let mut blank = 0;
    for l in joined {
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(&l);
        out.push('\n');
    }
    out.trim().to_string()
}

/// XML entities an older extraction left in the text: `GÖRÜŞLER &amp; AÇIKLAMALAR`.
fn unescape(line: &str) -> String {
    if !line.contains('&') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let entity = tail.find(';').filter(|&end| end <= 10).map(|end| &tail[1..end]);
        let decoded = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => e
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        });
        match (entity, decoded) {
            (Some(e), Some(c)) => {
                out.push(c);
                rest = &tail[e.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `HYPERLINK "https://…" bu linkten` → `bu linkten (https://…)`; an internal anchor
/// (`HYPERLINK \l "_Toc1"`) is dropped.
fn strip_field_codes(line: &str) -> String {
    let mut out = line.to_string();
    while let Some(at) = out.find("HYPERLINK") {
        let rest = &out[at + "HYPERLINK".len()..];
        let internal = rest.trim_start().starts_with("\\l");
        let (url, consumed) = match rest.find('"') {
            Some(q1) => match rest[q1 + 1..].find('"') {
                Some(q2) => (rest[q1 + 1..q1 + 1 + q2].to_string(), q1 + q2 + 2),
                None => (String::new(), rest.len()),
            },
            None => (String::new(), 0),
        };
        let after = rest[consumed..].trim_start().to_string();
        let mut replaced = out[..at].to_string();
        replaced.push_str(&after);
        if !internal && url.starts_with("http") {
            replaced.push_str(&format!(" ({url})"));
        }
        out = replaced;
    }
    out
}

/// A template's `Form No:` header is about the template; everything from it on is dropped.
/// A line that was nothing else disappears.
fn drop_template_header(line: &str, own: Option<&str>) -> Option<String> {
    let folded = code_fold(line);
    let Some(at) = folded.find("form no") else { return Some(line.to_string()) };
    let after = folded[at + "form no".len()..].trim_start_matches([' ', ':', '.', '\t']);
    let code: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if code.is_empty() || Some(code_key(&code).as_str()) == own {
        return Some(line.to_string());
    }
    // Folding keeps one character per character, so the byte position maps back by count.
    let chars_before = folded[..at].chars().count();
    let kept: String = line.chars().take(chars_before).collect();
    let kept = kept.trim_end();
    (!kept.is_empty()).then(|| kept.to_string())
}

/// `Sayfa 3 / 23`, `Sayfa 23-1`, `Page 2 of 5`, `3/23`.
fn is_page_furniture(line: &str) -> bool {
    let f = code_fold(line.trim());
    let rest = f.strip_prefix("sayfa").or_else(|| f.strip_prefix("page")).unwrap_or(&f);
    let rest = rest.trim();
    if rest.is_empty() || f == rest && !rest.contains('/') {
        return false;
    }
    let digits_and_seps = rest.chars().all(|c| c.is_ascii_digit() || c == '/' || c == '-' || c == ' ')
        || rest.split_whitespace().collect::<Vec<_>>().as_slice().windows(3).any(|w| w[1] == "of")
            && rest.split_whitespace().all(|t| t == "of" || t.chars().all(|c| c.is_ascii_digit()));
    digits_and_seps && rest.chars().any(|c| c.is_ascii_digit())
}

/// `BİRİNCİ BÖLÜM` → a heading; `MADDE 12 – (1) …` → the article label in bold.
fn heading(line: &str) -> Option<String> {
    let f = code_fold(line);
    let upper = line.chars().any(char::is_alphabetic) && !line.chars().any(char::is_lowercase);
    if upper && f.ends_with("bolum") && line.chars().count() <= 40 {
        return Some(format!("## {line}"));
    }
    if let Some(rest) = f.strip_prefix("madde ") {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            let label_chars = "madde ".len() + digits;
            let label: String = line.chars().take(label_chars).collect();
            let tail: String = line.chars().skip(label_chars).collect();
            return Some(format!("**{label}**{tail}"));
        }
    }
    None
}

/// Text where a fifth of the "words" are single letters was split letter by letter.
fn looks_garbled(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).collect();
    if words.len() < 40 {
        return false;
    }
    let single = words.iter().filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() == 1).count();
    single * 5 > words.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_furniture_and_template_headers_go_and_content_stays() {
        let body = "LİSANSÜSTÜ EĞİTİM ÖĞRETİM YÖNETMELİĞİ SENATO UYGULAMA ESASLARI\n\
                    Sayfa 1 / 23\n\
                    Form No: FR-0356 Yayın Tarihi: 23.11.2017 Değ.No:0 Değ.Tarihi:-\n\
                    BİRİNCİ BÖLÜM\n\
                    MADDE 1 - SE (1) Bu esasların amacı\n\
                    LİSANSÜSTÜ EĞİTİM ÖĞRETİM YÖNETMELİĞİ SENATO UYGULAMA ESASLARI\n\
                    Sayfa 2 / 23\n\
                    düzenlemektir ve uygula-\n\
                    maktadır.\n\
                    LİSANSÜSTÜ EĞİTİM ÖĞRETİM YÖNETMELİĞİ SENATO UYGULAMA ESASLARI\n\
                    İmza\nİmza\nİmza";
        let out = clean(body, Some("YÖ-0054"));
        assert_eq!(out.matches("SENATO UYGULAMA ESASLARI").count(), 1, "{out}");
        assert!(!out.contains("Sayfa"), "{out}");
        assert!(!out.contains("FR-0356"), "{out}");
        assert!(out.contains("## BİRİNCİ BÖLÜM"), "{out}");
        assert!(out.contains("**MADDE 1** - SE (1)"), "{out}");
        assert!(out.contains("uygulamaktadır."), "{out}");
        assert_eq!(out.matches("İmza").count(), 3, "short repeated fields are content: {out}");
    }

    #[test]
    fn entities_left_by_an_older_extraction_are_decoded() {
        assert_eq!(unescape("GÖRÜŞLER &amp; AÇIKLAMALAR"), "GÖRÜŞLER & AÇIKLAMALAR");
        assert_eq!(unescape("a &lt;b&gt; &#252; &#xFC; &quot;q&quot;"), "a <b> ü ü \"q\"");
        assert_eq!(unescape("R&D & AR-GE; 5 & 6"), "R&D & AR-GE; 5 & 6", "not an entity, kept");
    }

    #[test]
    fn a_form_keeps_its_own_header_line() {
        let body = "Form No:FR-0083 Yayın Tarihi:21.06.2017 Değ.No:1\nI. ÖĞRENCİ BİLGİLERİ";
        assert!(clean(body, Some("FR-0083")).starts_with("Form No:FR-0083"));
    }

    #[test]
    fn a_field_code_keeps_its_link_and_loses_its_syntax() {
        assert_eq!(
            strip_field_codes(r#"inceleyin HYPERLINK "https://www.gtu.edu.tr/x.pdf" bu linkten"#),
            "inceleyin bu linkten (https://www.gtu.edu.tr/x.pdf)"
        );
        assert_eq!(strip_field_codes(r#"İçindekiler HYPERLINK \l "_Toc1" Giriş"#), "İçindekiler Giriş");
    }

    #[test]
    fn page_numbers_are_furniture_and_ordinary_numbers_are_not() {
        for l in ["Sayfa 1 / 23", "Sayfa 23-1", "Page 2 of 5", "3/23"] {
            assert!(is_page_furniture(l), "{l}");
        }
        for l in ["2 nüsha", "Madde 5", "1/2 oranında", "Sayfa sayısı: 3 adet"] {
            assert!(!is_page_furniture(l), "{l}");
        }
    }
}
