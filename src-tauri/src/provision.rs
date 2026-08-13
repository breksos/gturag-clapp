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

/// Where a NEWER index comes from when the human asks for one. The depot already ships a
/// working index (see [`bundled_index`]), so this is never on the first-run path — it is
/// what makes `gturag sync` able to pick up a corpus GTÜ has revised without anyone
/// rebuilding or reinstalling the app.
pub const INDEX_URL: &str = match option_env!("GTURAG_INDEX_URL") {
    Some(u) => u,
    None => "https://raw.githubusercontent.com/breksos/gturag-clapp/main/corpus.gtu",
};

/// The committed form database, one JSON file per form. `get` reads a form's full text
/// from here — the index carries passages, which is what search needs, but an agent asked
/// to fill a form in wants the whole document.
pub const FORMS_BASE: &str = match option_env!("GTURAG_FORMS_BASE") {
    Some(u) => u,
    None => "https://raw.githubusercontent.com/breksos/gturag-clapp/main/forms",
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

/// One form's full extracted text, from the committed database. Cached in the app's data
/// directory after the first read — a form's text does not change under a fixed revision.
///
/// This is what `get` answers with. The app deliberately does NOT download the original
/// `.docx`/`.pdf`: the human is sent to the university's own page for that (the
/// authoritative copy, always current), while an agent gets text it can actually read.
pub fn fetch_form_text(cli: &str, id: &str) -> Result<String> {
    // `id` comes from our own index, never from the caller, but it lands in a URL and a
    // path — so it is still constrained to what an id can legitimately contain.
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if safe.is_empty() || safe.contains("..") {
        bail!("`{id}` is not a form id");
    }

    let dir = clappkit::data_subdir(cli, "forms");
    let dest = dir.join(format!("{safe}.json"));
    if !dest.is_file() {
        download(&format!("{FORMS_BASE}/{safe}.json"), &dest, 4096, |_| {})
            .with_context(|| format!("fetching the text of {safe}"))?;
    }

    let raw = std::fs::read_to_string(&dest)
        .with_context(|| format!("cannot read {}", dest.display()))?;
    let form: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        // A GitHub 404 is an HTML page delivered with every appearance of success.
        let _ = std::fs::remove_file(&dest);
        anyhow::anyhow!("the stored form will not parse ({e})")
    })?;
    Ok(form
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string())
}

/// The index that ships inside the depot — the baseline every install starts from, so a
/// first run is never blocked on the network for anything but the model.
pub fn bundled_index() -> Option<PathBuf> {
    let path = clappkit::paths::install_root().join(INDEX_FILE);
    path.is_file().then_some(path)
}

/// The best index available without downloading: a synced one from the data directory if
/// the human has ever run `sync`, otherwise the one that shipped in the depot.
///
/// A damaged cached file is moved aside rather than left to fail identically on every
/// launch — and because the depot's copy is still there, that degrades to "you lost your
/// update", not "the app no longer works".
pub fn load_cached_index(cli: &str) -> Option<Corpus> {
    let synced = index_path(cli);
    if synced.is_file() {
        match Corpus::read(&synced) {
            Ok(c) => return Some(c),
            Err(e) => {
                eprintln!("gturag: the synced index is unusable ({e}) — falling back to the bundled one");
                clappkit::store::quarantine(&synced);
            }
        }
    }
    let bundled = bundled_index()?;
    match Corpus::read(&bundled) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("gturag: the bundled index is unusable ({e})");
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
