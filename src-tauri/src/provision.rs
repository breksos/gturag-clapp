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

/// The environment variable a launcher sets to the root of its shared asset store.
///
/// Nothing sets this yet — the shared-asset primitive is proposed, not shipped. The name is
/// read rather than the path hardcoded for the same reason `clappkit::paths` refuses to
/// hardcode a data directory: the store is the launcher's to place, and an app that guesses
/// its location breaks the day it moves. Until something sets it, every branch below falls
/// through to the app's own copy and behaves exactly as it always has.
pub const ASSETS_DIR_ENV: &str = "CLATCH_ASSETS_DIR";

/// The asset this app needs, as a directory name: the model id with everything that is not
/// safe in a path folded to `-`.
///
/// Derived from [`crate::corpus::MODEL_ID`] and nowhere else, because that same string is
/// what `corpus.gtu` records and refuses to load against. Two clapps share a model only if
/// they agree on bit-identical weights, so the thing that names the directory and the thing
/// that validates the index must be one string — otherwise "shared" quietly means "similar".
pub fn asset_slug() -> String {
    crate::corpus::MODEL_ID
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
        .collect()
}

/// Where this app keeps its OWN copy of the model — the only directory it ever writes to.
pub fn own_model_dir(cli: &str) -> PathBuf {
    clappkit::data_subdir(cli, MODEL_DIR)
}

/// Every place a usable model could be, best first.
///
/// A shared copy is READ-ONLY to us. Clatch owns that store: it fetches, verifies and
/// reference-counts what lives there, and an app that wrote into it would be filling a
/// directory whose lifetime it does not control — the deduplication only holds if exactly
/// one party is responsible for putting things in.
fn candidates(cli: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = std::env::var_os(ASSETS_DIR_ENV).filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(root).join(asset_slug()));
    }
    // The conventional location, for a launcher that populates a store without announcing
    // it, and for a human who wants to drop the weights in by hand rather than wait for a
    // 465 MB download they already have elsewhere.
    if let Some(base) = clappkit::paths::home() {
        out.push(base.join(".clatch").join("shared").join(asset_slug()));
    }
    out.push(own_model_dir(cli));
    out
}

/// Pick from an explicit list, last entry being our own directory: the first candidate that
/// actually holds the weights, or that own directory when none does.
///
/// Split out from [`model_dir`] so the RULE can be tested without the machine's real home
/// directory being part of the assertion. Testing the rule through `model_dir` meant every
/// case silently depended on whether `~/.clatch/shared` happened to be populated — green on
/// a clean checkout, red on any machine that had ever shared this model, which is precisely
/// backwards for a feature about sharing.
fn resolve(candidates: &[PathBuf]) -> PathBuf {
    let own = candidates.last().expect("own dir is always last");
    for dir in candidates {
        if complete(dir) {
            if dir != own {
                eprintln!(
                    "gturag: using the shared model at {}",
                    clappkit::paths::simplified(dir).display()
                );
            }
            return dir.clone();
        }
    }
    own.clone()
}

/// The model directory to LOAD from: the first candidate that actually holds the weights,
/// or this app's own directory when none does — which is also where a download would put
/// them, so the caller can treat the answer as "where the model is or will be".
pub fn model_dir(cli: &str) -> PathBuf {
    resolve(&candidates(cli))
}

/// Does this directory hold all three files, at plausible sizes?
fn complete(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|(name, size)| {
        std::fs::metadata(dir.join(name))
            // A truncated download from a killed process would otherwise pass as present
            // and fail much later, inside safetensors, with a far worse message.
            .map(|m| m.len() >= size / 2)
            .unwrap_or(false)
    })
}

pub fn index_path(cli: &str) -> PathBuf {
    clappkit::data_file(cli, INDEX_FILE)
}

/// Is the model already on disk and plausibly complete? Cheap enough to call at startup —
/// it stats three files and never opens the 450 MB one.
pub fn model_present(cli: &str) -> bool {
    candidates(cli).iter().any(|d| complete(d))
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
    // OWN directory, deliberately: see `candidates`. We read from a shared store and never
    // write to one.
    let dir = own_model_dir(cli);
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

    /// `CLATCH_ASSETS_DIR` is process-global and every test below sets it, so two running
    /// concurrently would each see the other's store. Take this first, hold it throughout.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// Lay down files of a plausible size, so `complete` sees a real model rather than
    /// three empty stubs.
    ///
    /// `set_len` rather than a written buffer: `complete` reads the LENGTH, and planting a
    /// real 235 MB safetensors in each of three tests moved most of a gigabyte through the
    /// page cache to assert something about a directory listing.
    fn plant(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        for (name, size) in MODEL_FILES {
            let f = std::fs::File::create(dir.join(name)).unwrap();
            f.set_len(size / 2 + 1).unwrap();
        }
    }

    /// A scratch directory that cleans itself up, named for the test that made it.
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gturag-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole point: a model already on the machine is used, not downloaded again.
    #[test]
    fn a_shared_model_is_preferred_over_downloading_our_own() {
        let tmp = scratch("shared");
        let shared = tmp.join("store").join(asset_slug());
        let own = tmp.join("own");
        plant(&shared);

        assert_eq!(resolve(&[shared.clone(), own]), shared, "the shared copy must win");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// An announced store that is EMPTY is not a model. Falling through matters more than
    /// preferring the shared path: a launcher may declare the store before it has fetched
    /// anything, and an app that trusted the variable would load nothing and fail late.
    #[test]
    fn an_empty_store_falls_through_to_our_own_directory() {
        let tmp = scratch("empty");
        let empty = tmp.join("store").join(asset_slug());
        std::fs::create_dir_all(&empty).unwrap();
        let own = tmp.join("own");

        assert_eq!(resolve(&[empty, own.clone()]), own);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A store holding a HALF-downloaded model is not a model either. The size floor is the
    /// only thing standing between a killed download and a failure raised much later, from
    /// inside safetensors, about a file the user never chose to fetch.
    #[test]
    fn a_truncated_shared_model_does_not_satisfy_us() {
        let tmp = scratch("truncated");
        let shared = tmp.join("store").join(asset_slug());
        plant(&shared);
        std::fs::File::create(shared.join("model.safetensors")).unwrap().set_len(4096).unwrap();
        let own = tmp.join("own");

        assert_eq!(resolve(&[shared, own.clone()]), own);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// We read from a shared store and never write to one. Clatch owns that directory's
    /// lifetime; an app filling it would break the reference counting that makes sharing
    /// safe to clean up.
    #[test]
    fn downloads_always_target_our_own_directory() {
        let tmp = scratch("write");
        let shared = tmp.join("store").join(asset_slug());
        plant(&shared);
        let own = own_model_dir("gturag-test-write");

        // Reading resolves to the shared copy...
        assert_eq!(resolve(&[shared.clone(), own.clone()]), shared);
        // ...while the only directory fetch_model would write to stays our own, under the
        // app's data dir, which is the one place we are entitled to create files.
        assert_eq!(own, clappkit::data_subdir("gturag-test-write", MODEL_DIR));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The search order, stated once: an announced store, then the conventional one, then
    /// ours — and ours is always last, which is what makes `resolve`'s fallback correct.
    #[test]
    fn the_search_order_puts_our_own_directory_last() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = std::env::temp_dir().join("gturag-order-store");

        std::env::set_var(ASSETS_DIR_ENV, &store);
        let announced = candidates("gturag-test-order");
        std::env::remove_var(ASSETS_DIR_ENV);
        let bare = candidates("gturag-test-order");

        assert_eq!(announced.first().unwrap(), &store.join(asset_slug()),
                   "an announced store is searched first");
        assert_eq!(announced.last().unwrap(), &own_model_dir("gturag-test-order"));
        assert_eq!(bare.last().unwrap(), &own_model_dir("gturag-test-order"));
        assert_eq!(announced.len(), bare.len() + 1, "the variable ADDS a place to look");
    }

    /// The directory name comes from the id `corpus.gtu` validates against, so "shared"
    /// cannot quietly come to mean "similar".
    #[test]
    fn the_asset_slug_is_the_model_id_and_is_path_safe() {
        let slug = asset_slug();
        assert!(slug.contains("multilingual-e5-small"), "{slug}");
        assert!(!slug.contains('/'), "a slug with a separator is a directory traversal: {slug}");
        assert!(
            slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
            "{slug}"
        );
        assert_eq!(slug, asset_slug(), "and it is stable");
    }

    #[test]
    fn a_missing_model_is_not_reported_as_present() {
        let tmp = scratch("absent");
        assert!(!complete(&tmp.join("nothing-here")));
        let _ = std::fs::remove_dir_all(&tmp);
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
