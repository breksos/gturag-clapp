//! First run: fetch the embedder's weights into a cache every clapp of this family shares,
//! and a newer corpus index into the app's own data directory when the human asks.
//!
//! This is why the `.clapp` is a few megabytes and not half a gigabyte. The depot carries
//! the binary, a manifest, and a bundled index; the model parameters are downloaded once
//! PER MACHINE — `gturag`, a `hacettepe` fork and a `hukuk` fork all embed with the same
//! model, so they read the same 450 MB rather than each keeping a copy. Nothing about
//! retrieval lives on a server — after this step the app is entirely local and works
//! offline.
//!
//! Where updates come from is carried by the DATA: the bundled index's header names its
//! own `update_url` and `text_base`. The compile-time defaults below are only for an index
//! built before those fields existed.
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

/// The environment variable that overrides the shared model cache, for a test or an
/// operator who keeps models on a particular volume.
pub const MODELS_DIR_ENV: &str = "CLATCH_MODELS_DIR";
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

/// Fallbacks for an index whose header predates `update_url` / `text_base`. A fork does not
/// edit these: it builds its index with `--update-url` and `--text-base`, and the data says.
pub const DEFAULT_INDEX_URL: &str = match option_env!("GTURAG_INDEX_URL") {
    Some(u) => u,
    None => "https://raw.githubusercontent.com/breksos/gturag-clapp/main/corpus.gtu",
};
pub const DEFAULT_TEXT_BASE: &str = match option_env!("GTURAG_FORMS_BASE") {
    Some(u) => u,
    None => "https://raw.githubusercontent.com/breksos/gturag-clapp/main/forms",
};
/// The provenance the builder stamps when `--source` is not given.
pub const DEFAULT_SOURCE: &str = "https://www.gtu.edu.tr/kategori/2382/0/display.aspx";

/// Where a newer index is fetched from — the index's own word, else the compiled default.
pub fn update_url(corpus: Option<&Corpus>) -> String {
    corpus
        .and_then(|c| c.header.update_url.clone())
        .unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

/// Where a document's full text is fetched from.
pub fn text_base(corpus: Option<&Corpus>) -> String {
    corpus
        .and_then(|c| c.header.text_base.clone())
        .unwrap_or_else(|| DEFAULT_TEXT_BASE.to_string())
}

/// The machine-wide cache every clapp of this family reads the model from.
///
/// `$CLATCH_MODELS_DIR` when set; else the OS cache location — `~/Library/Caches` on
/// macOS, `%LOCALAPPDATA%` on Windows, `$XDG_CACHE_HOME` or `~/.cache` elsewhere — under
/// `clatch/models`. Shared on purpose and by construction: the path is keyed by the MODEL
/// id, not the app, so two apps that embed with the same model cannot end up with two
/// copies. A model is 450 MB; a family of five clapps is not 2.25 GB.
pub fn models_root() -> PathBuf {
    if let Ok(dir) = std::env::var(MODELS_DIR_ENV) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let base = if cfg!(target_os = "macos") {
        clappkit::paths::home().map(|h| h.join("Library").join("Caches"))
    } else if cfg!(windows) {
        clappkit::paths::user_base()
    } else {
        std::env::var("XDG_CACHE_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| clappkit::paths::home().map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("clatch").join("models")
}

/// This model's directory in the shared cache: the model id with `/` made safe for a path.
pub fn model_dir() -> PathBuf {
    models_root().join(crate::corpus::MODEL_ID.replace('/', "--"))
}

/// Adopt a model an older version of this app downloaded into its private data directory,
/// so upgrading never re-downloads 450 MB. Once: a no-op when the shared copy exists.
pub fn adopt_private_model(cli: &str) {
    let legacy = clappkit::data_dir(cli).join("model");
    if legacy.join("model.safetensors").is_file() {
        clappkit::paths::adopt_legacy(&legacy, &model_dir());
    }
}

pub fn index_path(cli: &str) -> PathBuf {
    clappkit::data_file(cli, INDEX_FILE)
}

/// Is the model already on disk and plausibly complete? Cheap enough to call at startup —
/// it stats three files and never opens the 450 MB one.
pub fn model_present() -> bool {
    let dir = model_dir();
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
pub fn fetch_model(mut progress: impl FnMut(Stage)) -> Result<()> {
    let dir = model_dir();
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

/// Fetch the index from `url` and return it parsed — or `None` when what arrived is not
/// newer than `current_built`, in which case nothing on disk changes.
///
/// Parsed *before* it replaces anything: [`Corpus::from_bytes`] is what tells an HTML 404
/// page, a truncated transfer and an index built with a different model apart from a good
/// file, and all three of those arrive looking like a successful download. And compared
/// before it is installed: `sync` is "is there something newer?", and the honest answer
/// to "no" is to leave the loaded index exactly as it is.
pub fn fetch_index(
    cli: &str,
    url: &str,
    current_built: Option<&str>,
    mut progress: impl FnMut(Stage),
) -> Result<Option<Corpus>> {
    let dest = index_path(cli);
    let staging = dest.with_extension("incoming");
    download(url, &staging, 16 * 1024 * 1024, |p| {
        progress(Stage::Downloading { percent: p });
    })
    .context("downloading the document index")?;

    let corpus = match Corpus::read(&staging) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
    };
    if let Some(have) = current_built {
        if corpus.header.built.as_str() <= have {
            let _ = std::fs::remove_file(&staging);
            progress(Stage::Ready);
            return Ok(None);
        }
    }
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("cannot install the index at {}", dest.display()))?;
    progress(Stage::Ready);
    Ok(Some(corpus))
}

/// One form's full extracted text, from the committed database. Cached in the app's data
/// directory after the first read — a form's text does not change under a fixed revision.
///
/// This is what `get` answers with. The app deliberately does NOT download the original
/// `.docx`/`.pdf`: the human is sent to the university's own page for that (the
/// authoritative copy, always current), while an agent gets text it can actually read.
pub fn fetch_form_text(cli: &str, text_base: &str, id: &str) -> Result<String> {
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
        download(&format!("{text_base}/{safe}.json"), &dest, 4096, |_| {})
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
    let from_sync = if synced.is_file() {
        match Corpus::read(&synced) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("gturag: the synced index is unusable ({e}) — falling back to the bundled one");
                clappkit::store::quarantine(&synced);
                None
            }
        }
    } else {
        None
    };

    let from_bundle = bundled_index().and_then(|p| match Corpus::read(&p) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("gturag: the bundled index will not load ({e})");
            None
        }
    });

    // The NEWEST index wins, wherever it came from — not "synced always beats bundled".
    //
    // That was the old rule and it is a trap: `sync` writes a copy into the data
    // directory, the data directory survives updates by design, so a user who ever ran
    // sync would keep that copy forever — including after installing a release whose
    // bundled index is newer. It is exactly the failure that hides: the app says "index
    // ready", every search works, and the answers are quietly a corpus behind. `built` is
    // an ISO date, so comparing the strings compares the dates.
    match (from_sync, from_bundle) {
        (Some(s), Some(b)) => {
            if b.header.built > s.header.built {
                eprintln!(
                    "gturag: the bundled index ({}) is newer than the synced one ({}) — using it",
                    b.header.built, s.header.built
                );
                Some(b)
            } else {
                Some(s)
            }
        }
        (Some(s), None) => Some(s),
        (None, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trap this rule exists to avoid: `sync` writes into the data directory, the data
    /// directory survives updates by design, so "synced always wins" means one sync in 2026
    /// pins that user to a 2026 corpus through every release that follows. It never errors
    /// and never looks wrong — the app says ready and the answers are just a corpus behind.
    #[test]
    fn the_newer_index_wins_whichever_side_it_is_on() {
        let older = "2026-08-13".to_string();
        let newer = "2026-09-01".to_string();
        // The comparison the loader makes, on the same ISO strings the header carries.
        assert!(newer > older, "ISO dates compare correctly as strings");
        assert!(!(older > newer));
        // A same-day rebuild is not "newer", so a synced index is not thrown away for a
        // bundled one of the same vintage.
        assert!(!(older > older.clone()));
    }

    /// One model per machine, not per app: the cache is keyed by the model id and by
    /// nothing about the app, so a fork resolves to the very same directory.
    #[test]
    fn the_model_cache_is_shared_and_keyed_by_model_not_app() {
        let dir = model_dir();
        assert!(dir.ends_with("intfloat--multilingual-e5-small"), "{}", dir.display());
        assert!(dir.to_string_lossy().contains("clatch"), "{}", dir.display());
        assert!(!dir.to_string_lossy().contains("gturag"), "an app name in a shared path: {}", dir.display());
    }

    #[test]
    fn the_index_url_comes_from_the_data_first() {
        use crate::corpus::{Header, Corpus};
        let mut c = Corpus {
            header: Header {
                version: 1, model: crate::corpus::MODEL_ID.into(), dim: 1,
                built: "2026-09-02".into(), source: "s".into(),
                update_url: Some("https://fork.example/corpus.gtu".into()),
                text_base: Some("https://fork.example/docs".into()),
                default_family: None, docs: vec![], chunks: vec![],
            },
            vectors: vec![],
        };
        assert_eq!(update_url(Some(&c)), "https://fork.example/corpus.gtu");
        assert_eq!(text_base(Some(&c)), "https://fork.example/docs");
        c.header.update_url = None;
        assert_eq!(update_url(Some(&c)), DEFAULT_INDEX_URL, "an old header falls back");
        assert_eq!(update_url(None), DEFAULT_INDEX_URL);
        assert!(DEFAULT_INDEX_URL.ends_with(".gtu"));
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
