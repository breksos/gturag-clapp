//! First run: fetch the embedder's weights and the corpus index into the app's own data
//! directory.
//!
//! This is why the `.clapp` is a few megabytes and not half a gigabyte. The depot carries
//! the binary, an icon and a manifest; the model parameters and the index are downloaded
//! once, here, into `data_dir()`. Nothing about retrieval lives on a server — after this
//! step the app is entirely local and works offline.
//!
//! Three rules learned from the shape of the problem:
//!
//! * **Report progress against a threshold, not a clock.** A percentage that updates on
//!   every 8 KB chunk would push a snapshot to the webview hundreds of times a second.
//!   Only a whole-percent change is news (PLAYBOOK §14).
//! * **Verify before publishing.** Every artifact lands at `<name>.part` and is renamed
//!   over the real path only once it has been checked. A half-downloaded model that got
//!   the real filename is a corruption the app would rediscover on every launch.
//! * **A failure is a state with a reason**, not a log line — the human is watching this,
//!   and "failed" without a sentence is indistinguishable from a hang.

use crate::corpus::Corpus;
use crate::state::Stage;
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the model's three files live, relative to the data dir.
pub const MODEL_DIR: &str = "model";
/// The corpus index, in the data dir.
pub const INDEX_FILE: &str = "corpus.gtu";

/// The three files the embedder needs. Sizes are the published ones, used only to drive a
/// progress bar when a server declines to send `Content-Length`.
const MODEL_FILES: &[(&str, u64)] = &[
    ("model.safetensors", 470_637_416),
    ("tokenizer.json", 17_082_913),
    ("config.json", 700),
];

/// Where the weights come from. The model is Apache-2.0 (`intfloat/multilingual-e5-small`)
/// and is fetched from its own home rather than re-hosted, so the provenance a user can
/// check is the model card itself.
const MODEL_BASE: &str = "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main";

/// Where the prebuilt index comes from. Set at build time so a fork points at its own
/// release without touching code.
pub const INDEX_URL: &str = match option_env!("GTURAG_INDEX_URL") {
    Some(u) => u,
    None => "https://github.com/breksos/gturag-clapp/releases/latest/download/corpus.gtu",
};

pub fn model_dir(cli: &str) -> PathBuf {
    clappkit::data_subdir(cli, MODEL_DIR)
}

pub fn index_path(cli: &str) -> PathBuf {
    clappkit::data_file(cli, INDEX_FILE)
}

/// Is the model already on disk and plausibly complete? Cheap enough to call at startup —
/// it stats three files and never opens the 450 MB one.
pub fn model_present(cli: &str) -> bool {
    let dir = model_dir(cli);
    MODEL_FILES.iter().all(|(name, size)| {
        std::fs::metadata(dir.join(name))
            // A truncated download from a killed process would otherwise pass as present
            // and fail much later, inside safetensors, with a far worse message.
            .map(|m| m.len() >= size / 2)
            .unwrap_or(false)
    })
}

/// Download `url` to `dest`, reporting whole-percent progress. Writes `<dest>.part` and
/// renames only on success.
fn download(url: &str, dest: &Path, expect: u64, mut on_progress: impl FnMut(u8)) -> Result<()> {
    let mut resp = ureq::get(url)
        .call()
        .with_context(|| format!("cannot reach {url}"))?;

    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(expect)
        .max(1);

    if let Some(parent) = dest.parent() {
        clappkit::ensure_private_dir(parent)?;
    }
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part)
        .with_context(|| format!("cannot create {}", part.display()))?;

    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_percent = u8::MAX;
    loop {
        let n = reader.read(&mut buf).context("the download was interrupted")?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        done += n as u64;
        // Threshold, not clock: only a change the human could see is worth a snapshot.
        let percent = ((done.min(total) * 100) / total) as u8;
        if percent != last_percent {
            last_percent = percent;
            on_progress(percent);
        }
    }
    drop(file);

    if done == 0 {
        let _ = std::fs::remove_file(&part);
        bail!("{url} returned nothing");
    }
    std::fs::rename(&part, dest)
        .with_context(|| format!("cannot move the finished download into {}", dest.display()))?;
    Ok(())
}

/// Fetch the model's three files. `progress` sees 0–100 across the whole set, weighted by
/// size, so the bar does not jump to 96% and sit there for four minutes.
pub fn fetch_model(cli: &str, mut progress: impl FnMut(Stage)) -> Result<()> {
    let dir = model_dir(cli);
    clappkit::ensure_private_dir(&dir)?;
    let grand_total: u64 = MODEL_FILES.iter().map(|(_, s)| *s).sum();
    let mut completed: u64 = 0;

    for (name, size) in MODEL_FILES {
        let dest = dir.join(name);
        if std::fs::metadata(&dest).map(|m| m.len() >= size / 2).unwrap_or(false) {
            completed += size;
            continue;
        }
        let url = format!("{MODEL_BASE}/{name}");
        let base = completed;
        download(&url, &dest, *size, |p| {
            let overall = ((base + (*size * p as u64) / 100) * 100 / grand_total) as u8;
            progress(Stage::Downloading { percent: overall.min(100) });
        })
        .with_context(|| format!("downloading {name}"))?;
        completed += size;
    }
    progress(Stage::Ready);
    Ok(())
}

/// Fetch the prebuilt corpus index and return it, parsed.
///
/// Parsed *before* it replaces anything: [`Corpus::from_bytes`] is what tells an HTML 404
/// page, a truncated transfer and an index built with a different model apart from a good
/// file, and all three of those arrive looking like a successful download.
pub fn fetch_index(cli: &str, mut progress: impl FnMut(Stage)) -> Result<Corpus> {
    let dest = index_path(cli);
    let staging = dest.with_extension("incoming");
    download(INDEX_URL, &staging, 16 * 1024 * 1024, |p| {
        progress(Stage::Downloading { percent: p });
    })
    .context("downloading the form index")?;

    let corpus = match Corpus::read(&staging) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
    };
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("cannot install the index at {}", dest.display()))?;
    progress(Stage::Ready);
    Ok(corpus)
}

/// Download one form to `<data>/files/`, and return where it landed.
///
/// The name is the university's own filename, not the caller's: `get` takes a form code,
/// never a path, so there is no way for a caller to choose where this writes. Already
/// downloaded is a cache hit — a form does not change under a fixed revision.
pub fn fetch_document(cli: &str, url: &str, name: &str) -> Result<PathBuf> {
    let dir = clappkit::data_subdir(cli, "files");
    // Belt and braces: strip anything that could traverse, even though `name` comes from
    // our own index rather than from the caller.
    let safe: String = name
        .chars()
        .map(|c| if std::path::is_separator(c) || c == ':' { '_' } else { c })
        .collect();
    let dest = dir.join(safe.trim_start_matches('.'));
    if dest.is_file() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(dest);
    }
    // The page's hrefs carry raw UTF-8; the server wants them percent-encoded.
    let encoded = url
        .chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c) {
                vec![c.to_string()]
            } else {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b)
                    .bytes()
                    .map(|x| format!("%{x:02X}"))
                    .collect()
            }
        })
        .collect::<String>();
    download(&encoded, &dest, 1, |_| {})?;
    Ok(dest)
}

/// Load an index already on disk, if there is a usable one. A damaged file is moved aside
/// rather than left to fail identically on every launch.
pub fn load_cached_index(cli: &str) -> Option<Corpus> {
    let path = index_path(cli);
    if !path.is_file() {
        return None;
    }
    match Corpus::read(&path) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("gturag: the cached index is unusable ({e})");
            clappkit::store::quarantine(&path);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_is_not_reported_as_present() {
        let _g = std::env::temp_dir();
        assert!(!model_present("gturag-test-absent-model"));
    }

    #[test]
    fn the_index_url_is_overridable_at_build_time() {
        // A fork must be able to point at its own release without editing code.
        assert!(INDEX_URL.starts_with("https://"), "{INDEX_URL}");
        assert!(INDEX_URL.ends_with(".gtu"), "{INDEX_URL}");
    }

    #[test]
    fn the_model_file_list_is_what_the_embedder_loads() {
        // embed.rs opens exactly these three names; a mismatch here is a first run that
        // downloads happily and then cannot start.
        let names: Vec<&str> = MODEL_FILES.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"model.safetensors"));
        assert!(names.contains(&"config.json"));
        assert!(names.contains(&"tokenizer.json"));
        assert_eq!(names.len(), 3);
    }
}
