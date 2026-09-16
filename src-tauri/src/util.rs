//! Small pure helpers more than one module needs: URL encoding, and dates.
//!
//! `now_iso` is the app's only clock. The state never calls it; the app layer does and
//! hands the value in, which is what keeps `state.rs` testable without mocking time.

use std::time::{SystemTime, UNIX_EPOCH};

/// Percent-encode everything a URL may not carry literally, leaving the structural
/// characters (`:/?#[]@` and the sub-delimiters) alone, and `%` too, so a URL that is
/// already encoded is not encoded twice.
pub fn encode_url(url: &str) -> String {
    const KEEP: &str = "-._~:/?#[]@!$&'()*+,;=%";
    encode(url, |c| c.is_ascii_alphanumeric() || KEEP.contains(c))
}

/// Percent-encode one path segment: only the unreserved characters survive. `İA-0021.tr`
/// becomes `%C4%B0A-0021.tr`, which is both a valid URL segment and a safe file name.
pub fn encode_segment(s: &str) -> String {
    encode(s, |c| c.is_ascii_alphanumeric() || "-._~".contains(c))
}

fn encode(s: &str, keep: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if keep(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `%C4%B0A` back to `İA`. Anything that is not a valid escape is kept as written.
pub fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// A date the way a document prints it — `10.06.2024`, `10/06/2024`, `1.6.2024` — as ISO
/// `2024-06-10`. `None` for anything that is not a plausible calendar date.
pub fn dmy_to_iso(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.trim().split(['.', '/', '-']).collect();
    if parts.len() != 3 || parts[2].len() != 4 {
        return None;
    }
    let d: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let y: u32 = parts[2].parse().ok()?;
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) || !(1900..=2100).contains(&y) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// `2024-06-10` (or a full ISO timestamp) the way a Turkish reader writes the day:
/// `10.06.2024`. Anything else comes back unchanged.
pub fn iso_to_dmy(iso: &str) -> String {
    let day = iso.get(..10).unwrap_or(iso);
    let p: Vec<&str> = day.split('-').collect();
    if p.len() == 3 {
        format!("{}.{}.{}", p[2], p[1], p[0])
    } else {
        iso.to_string()
    }
}

/// An HTTP date (`Tue, 10 Dec 2024 07:34:36 GMT`) as ISO `2024-12-10T07:34:36Z`. Only the
/// IMF-fixdate form, which is the only one servers are allowed to send.
pub fn http_date_to_iso(s: &str) -> Option<String> {
    let mut it = s.split_whitespace();
    it.next()?; // weekday
    let day: u32 = it.next()?.parse().ok()?;
    let month = match it.next()? {
        "Jan" => 1, "Feb" => 2, "Mar" => 3, "Apr" => 4, "May" => 5, "Jun" => 6,
        "Jul" => 7, "Aug" => 8, "Sep" => 9, "Oct" => 10, "Nov" => 11, "Dec" => 12,
        _ => return None,
    };
    let year: u32 = it.next()?.parse().ok()?;
    let time = it.next()?;
    if time.len() != 8 || time.as_bytes()[2] != b':' || time.as_bytes()[5] != b':' {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}T{time}Z"))
}

/// Now, in UTC, as `2026-09-16T14:03:00Z`.
pub fn now_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    iso_from_unix(secs)
}

pub fn iso_from_unix(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turkish_id_becomes_a_url_segment_and_comes_back() {
        assert_eq!(encode_segment("İA-0021.tr"), "%C4%B0A-0021.tr");
        assert_eq!(encode_segment("YÖ-0054.tr"), "Y%C3%96-0054.tr");
        assert_eq!(decode_percent("%C4%B0%C5%9F%20Ak%C4%B1%C5%9Flar%C4%B1"), "İş Akışları");
        assert_eq!(decode_percent("100%"), "100%", "a stray percent is kept");
    }

    #[test]
    fn a_real_form_url_survives_encoding() {
        let raw = "https://www.gtu.edu.tr/fileman/Formlar-Türkçe/FR-0083 Danışman Değişikliği Formu R1.pdf";
        let got = encode_url(raw);
        assert!(!got.contains(' '), "{got}");
        assert!(got.starts_with("https://www.gtu.edu.tr/fileman/"), "{got}");
        assert!(got.ends_with("R1.pdf") && got.contains("%20"), "{got}");
        assert_eq!(encode_url(&got), got, "encoding must be idempotent");
        assert_eq!(encode_url("https://x.invalid/p?a=1&b=2#frag"), "https://x.invalid/p?a=1&b=2#frag");
    }

    #[test]
    fn document_dates_become_iso_and_nonsense_does_not() {
        assert_eq!(dmy_to_iso("10.06.2024").as_deref(), Some("2024-06-10"));
        assert_eq!(dmy_to_iso("1/6/2024").as_deref(), Some("2024-06-01"));
        assert_eq!(dmy_to_iso("-"), None);
        assert_eq!(dmy_to_iso("32.01.2024"), None);
        assert_eq!(dmy_to_iso("10.06.24"), None, "a two-digit year is ambiguous");
        assert_eq!(iso_to_dmy("2026-09-02T14:01:35Z"), "02.09.2026");
    }

    #[test]
    fn http_dates_and_the_clock_agree_on_iso() {
        assert_eq!(
            http_date_to_iso("Tue, 10 Dec 2024 07:34:36 GMT").as_deref(),
            Some("2024-12-10T07:34:36Z")
        );
        assert_eq!(http_date_to_iso("yesterday"), None);
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00Z", "a leap day");
        assert_eq!(iso_from_unix(1_733_816_076), "2024-12-10T07:34:36Z");
    }
}
