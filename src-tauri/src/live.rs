//! Is the copy in this archive still the one the university publishes?
//!
//! The archive is a snapshot, and the university replaces documents under it: when a
//! revision is published, the old file is taken down (`YÖ-0054 … R7.pdf` answers 404 once
//! `R8.pdf` is up). So a document can be asked about directly, with a HEAD request or two,
//! without scraping anything:
//!
//! * a filename that carries `R<n>` is checked for `R<n+1>` — found, and a newer revision
//!   exists, at a known address;
//! * the indexed address itself answers 404 — the file was withdrawn or replaced;
//! * an unmarked file (`İA-0021.pdf`) is compared by date — modified after the archive was
//!   built, and the text here is older than the one published.
//!
//! The decision is pure and takes the prober as a function, so the rules are tested without
//! a network. Only [`head`] talks to the site.

use crate::corpus::Doc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Status {
    /// The published file is the one indexed.
    Current,
    /// A later revision is published, at `url`.
    Newer { rev: u32, url: String },
    /// Same address, but the file there was replaced after the archive was built.
    Changed { modified: String },
    /// Nothing is published at the indexed address any more.
    Gone,
    /// The site could not be asked, so nothing is claimed either way.
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    #[serde(flatten)]
    pub status: Status,
    pub checked_at: String,
}

impl Live {
    /// One line for a terminal.
    pub fn sentence(&self) -> String {
        let at = self.checked_at.get(..16).unwrap_or(&self.checked_at).replace('T', " ");
        match &self.status {
            Status::Current => format!("current: the university publishes this revision (checked {at} UTC)"),
            Status::Newer { rev, url } => format!(
                "OUT OF DATE: revision {rev} is published and this archive has an older one. Read the current one: {url}"
            ),
            Status::Changed { modified } => format!(
                "OUT OF DATE: the published file was replaced on {} — after this archive was built. Read it at the source",
                crate::util::iso_to_dmy(modified)
            ),
            Status::Gone => "WITHDRAWN: the university no longer publishes this file at its address; a newer revision may have replaced it. Check the official list".to_string(),
            Status::Unknown { reason } => format!("not checked: the university's site could not be reached ({reason})"),
        }
    }
}

/// What one HEAD request found.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub status: u16,
    /// `Last-Modified`, as ISO.
    pub modified: Option<String>,
}

/// Ask the site about one document. `index_built` is the archive's ISO build time.
pub fn check(doc: &Doc, index_built: &str, probe: &dyn Fn(&str) -> Result<Head, String>) -> Status {
    let here = match probe(&doc.url) {
        Ok(h) => h,
        Err(reason) => return Status::Unknown { reason },
    };
    let marked = revision_in_name(&doc.url);
    let gone = matches!(here.status, 404 | 410);

    if let Some((rev, _)) = marked {
        // One step ahead when the file is still up; a few when it is gone, since that is
        // exactly when a newer revision is most likely.
        for (next, url) in later_revisions(&doc.url, rev, gone) {
            match probe(&url) {
                Ok(h) if h.status == 200 => return Status::Newer { rev: next, url },
                Ok(_) => {}
                Err(reason) if gone => return Status::Unknown { reason },
                Err(_) => break,
            }
        }
    }

    match here.status {
        200 => match here.modified {
            Some(m) if m.as_str() > index_built => Status::Changed { modified: m },
            _ => Status::Current,
        },
        _ if gone => Status::Gone,
        other => Status::Unknown { reason: format!("the site answered {other}") },
    }
}

/// The `R<n>` marker in a URL's file name: its number and the byte range of the digits.
pub fn revision_in_name(url: &str) -> Option<(u32, std::ops::Range<usize>)> {
    let name_start = url.rfind('/').map_or(0, |i| i + 1);
    let stem_end = url[name_start..].rfind('.').map_or(url.len(), |i| name_start + i);
    let b = url.as_bytes();
    let mut found = None;
    let mut i = name_start;
    while i < stem_end {
        let boundary = i == name_start || !b[i - 1].is_ascii_alphanumeric();
        if boundary && matches!(b[i], b'R' | b'r') {
            let d0 = i + 1;
            let mut d1 = d0;
            while d1 < stem_end && b[d1].is_ascii_digit() {
                d1 += 1;
            }
            let ends = d1 == stem_end || !b[d1].is_ascii_alphanumeric();
            if (1..=2).contains(&(d1 - d0)) && ends {
                found = url[d0..d1].parse().ok().map(|n| (n, d0..d1));
            }
        }
        i += 1;
    }
    found
}

/// Addresses a later revision would be published at: the same name with the number raised,
/// and — for a withdrawn file — the other formats the university publishes in.
fn later_revisions(url: &str, rev: u32, withdrawn: bool) -> Vec<(u32, String)> {
    let Some((_, digits)) = revision_in_name(url) else { return Vec::new() };
    let with = |n: u32| format!("{}{n}{}", &url[..digits.start], &url[digits.end..]);
    let mut out = vec![(rev + 1, with(rev + 1))];
    if withdrawn {
        out.push((rev + 2, with(rev + 2)));
        let next = with(rev + 1);
        if let Some(dot) = next.rfind('.') {
            let ext = next[dot + 1..].to_ascii_lowercase();
            for other in ["pdf", "docx", "xlsx"] {
                if other != ext {
                    out.push((rev + 1, format!("{}.{other}", &next[..dot])));
                }
            }
        }
    }
    out
}

/// A HEAD request, bounded in time, that treats every status as an answer.
pub fn head(url: &str) -> Result<Head, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .user_agent(concat!("gturag/", env!("CARGO_PKG_VERSION"), " (document freshness check)"))
        .build()
        .into();
    let resp = agent
        .head(crate::util::encode_url(url))
        .call()
        .map_err(|e| match e {
            ureq::Error::Timeout(_) => "the site did not answer in time".to_string(),
            ureq::Error::HostNotFound => "offline, or the site's name did not resolve".to_string(),
            other => other.to_string(),
        })?;
    let modified = resp
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .and_then(crate::util::http_date_to_iso);
    Ok(Head { status: resp.status().as_u16(), modified })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn doc(url: &str) -> Doc {
        Doc {
            id: "X.tr".into(),
            code: Some("YÖ-0054".into()),
            rev: 7,
            lang: "tr".into(),
            title: "t".into(),
            name: "n".into(),
            ext: "pdf".into(),
            url: url.into(),
            chars: 1,
            hash: None,
        }
    }

    fn site(pages: &[(&str, u16, Option<&str>)]) -> impl Fn(&str) -> Result<Head, String> {
        let map: HashMap<String, Head> = pages
            .iter()
            .map(|(u, s, m)| (u.to_string(), Head { status: *s, modified: m.map(String::from) }))
            .collect();
        move |u: &str| Ok(map.get(u).cloned().unwrap_or(Head { status: 404, modified: None }))
    }

    const R7: &str = "https://x.invalid/Yönergeler/YÖ-0054 Senato Esasları R7.pdf";
    const R8: &str = "https://x.invalid/Yönergeler/YÖ-0054 Senato Esasları R8.pdf";
    const BUILT: &str = "2026-09-02T14:01:35Z";

    /// What the site actually did on 16 September 2026.
    #[test]
    fn a_withdrawn_revision_points_at_the_one_that_replaced_it() {
        let status = check(&doc(R7), BUILT, &site(&[(R8, 200, Some("2026-09-11T13:14:04Z"))]));
        assert_eq!(status, Status::Newer { rev: 8, url: R8.into() });
    }

    #[test]
    fn a_published_revision_with_nothing_newer_is_current() {
        assert_eq!(check(&doc(R7), BUILT, &site(&[(R7, 200, Some("2025-01-01T00:00:00Z"))])), Status::Current);
    }

    #[test]
    fn a_newer_revision_is_found_even_while_the_old_file_is_still_up() {
        let status = check(&doc(R7), BUILT, &site(&[(R7, 200, None), (R8, 200, None)]));
        assert!(matches!(status, Status::Newer { rev: 8, .. }), "{status:?}");
    }

    #[test]
    fn an_unmarked_file_is_judged_by_its_date() {
        let url = "https://x.invalid/İş Akışları/İA-0021.pdf";
        let old = check(&doc(url), BUILT, &site(&[(url, 200, Some("2024-12-10T07:34:36Z"))]));
        assert_eq!(old, Status::Current, "modified before the archive was built");
        let new = check(&doc(url), BUILT, &site(&[(url, 200, Some("2026-09-10T08:00:00Z"))]));
        assert!(matches!(new, Status::Changed { .. }), "{new:?}");
    }

    #[test]
    fn a_file_that_is_simply_gone_says_so() {
        assert_eq!(check(&doc(R7), BUILT, &site(&[])), Status::Gone);
    }

    #[test]
    fn no_network_claims_nothing() {
        let status = check(&doc(R7), BUILT, &|_| Err("offline".into()));
        assert_eq!(status, Status::Unknown { reason: "offline".into() });
    }

    #[test]
    fn the_revision_marker_is_read_from_the_file_name_only() {
        assert_eq!(revision_in_name(R7).map(|r| r.0), Some(7));
        assert_eq!(revision_in_name("https://x.invalid/FR-0553 Yandal Basvuru Formu-R1.doc").map(|r| r.0), Some(1));
        assert_eq!(revision_in_name("https://x.invalid/R5/İA-0021.pdf"), None, "a folder is not a revision");
        assert_eq!(revision_in_name("https://x.invalid/CH-TL-0114 HACH DR3800 R2.pdf").map(|r| r.0), Some(2));
        assert_eq!(revision_in_name("https://x.invalid/FR-0001 FORM.pdf"), None);
        let later = later_revisions(R7, 7, true);
        assert_eq!(later[0], (8, R8.to_string()));
        assert!(later.iter().any(|(_, u)| u.ends_with("R8.docx")), "{later:?}");
    }
}
