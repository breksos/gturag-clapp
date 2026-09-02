//! Stage 2 of the corpus build: chunk and embed the committed form database, and write
//! the `corpus.gtu` that ships inside the depot.
//!
//! The input is `forms/*.json` — one file per form, committed to the repository, holding
//! metadata and extracted text. That is the project's database: scraping the university's
//! site and prying text out of pre-2007 Office files happens once, in Python, and its
//! result is a reviewable artifact. Rebuilding the index from it needs neither the network
//! nor LibreOffice.
//!
//! This lives in the app's own binary for one reason: it must use [`crate::embed`] — the
//! very same code that embeds queries at runtime. A separate implementation would agree
//! with this one until it quietly didn't, and the symptom of a passage/query mismatch is
//! not an error, it is search that is a little bit wrong forever.
//!
//! It is a maintainer verb, deliberately absent from `clatch.json`'s `connector.commands`
//! and from `gturag -h`: an agent is never granted it, because it is part of building a
//! release, not of using the app.
//!
//!     gturag index-corpus forms --built 2026-09-02          # incremental, against corpus.gtu
//!     gturag index-corpus forms --built 2026-09-02 --fresh  # embed everything again
//!
//! **Incremental by default.** Every document carries a fingerprint of what was embedded;
//! when the output already exists, a document whose fingerprint is unchanged keeps its
//! passages and vectors from there, and only what changed is embedded. Adding one
//! document to a registry of two thousand embeds one document, and removing one embeds
//! nothing — so the model is not even loaded for a pure removal. That is what makes the
//! corpus an updatable database rather than a release artifact.

use crate::corpus::{Chunk, Corpus, Doc, Header, DIM, MODEL_ID};
use crate::embed::{Embedder, BATCH};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Part of every fingerprint. Bump it when [`chunk`] changes, and every document re-embeds
/// once — because a passage boundary that moved is a vector that no longer means what the
/// stored one did, even though the text is identical.
const CHUNKER_VERSION: u32 = 1;

/// How long a passage may be, in characters, and how much neighbouring passages share.
/// A form's body is mostly a table of blanks, so passages are small and the overlap keeps
/// a sentence that straddles a boundary findable from either side.
const MAX_CHARS: usize = 900;
const OVERLAP: usize = 150;
/// A runaway spreadsheet must not dominate the index with hundreds of passages.
const MAX_CHUNKS_PER_DOC: usize = 40;

/// One `forms/<id>.json`.
#[derive(serde::Deserialize)]
struct Form {
    id: String,
    code: Option<String>,
    rev: u32,
    lang: String,
    title: String,
    name: String,
    ext: String,
    url: String,
    text: String,
}

fn read_forms(dir: &Path) -> Result<Vec<Form>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {} — run tools/build-index/fetch.py first", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    // Sorted, so two builds of the same database produce byte-identical output.
    paths.sort();

    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let text = std::fs::read_to_string(&p)
            .with_context(|| format!("cannot read {}", p.display()))?;
        out.push(serde_json::from_str(&text).with_context(|| format!("{} will not parse", p.display()))?);
    }
    anyhow::ensure!(!out.is_empty(), "no forms found in {}", dir.display());
    Ok(out)
}

/// FNV-1a over the title, the text and the chunker version. Deliberately not `DefaultHasher`:
/// its algorithm is unspecified across Rust versions, and a fingerprint that silently changes
/// with a toolchain upgrade would re-embed the whole corpus for no reason and defeat the
/// point of storing it. Not cryptographic; it does not need to be — a collision here costs a
/// stale vector, not a secret.
fn fingerprint(title: &str, text: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in CHUNKER_VERSION
        .to_le_bytes()
        .iter()
        .chain(title.as_bytes())
        .chain(b"\n")
        .chain(text.as_bytes())
    {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

/// The code family most documents belong to — what a bare number will mean to the index.
fn most_common_family(forms: &[Form]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for f in forms {
        if let Some(code) = f.code.as_deref() {
            if let Some(family) = code.rsplit_once('-').map(|(fam, _)| fam) {
                *counts.entry(family.to_string()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(k, _)| k)
}

/// Split a form's text into passages, each carrying the title.
///
/// The title rides on EVERY chunk on purpose: a form's body is often a bare table of
/// blanks, and a passage that has lost "Danışman Değişikliği Formu" is a passage no query
/// can find.
fn chunk(title: &str, body: &str) -> Vec<String> {
    let body = body.trim();
    // The title, on its own, is always the first passage.
    //
    // Every other passage is `title + body`, and these bodies are near-identical across
    // the corpus: ADI SOYADI, İMZA, UYGUNDUR, a table of blanks. That boilerplate dominates
    // the embedding, so 791 forms come out looking alike to the dense retriever and a
    // query about advisors ranks a nutrition-counselling form first. A passage that is
    // ONLY the title gives each form one vector that means what the form is, uncontaminated
    // by the paperwork around it.
    let mut out = vec![title.to_string()];
    if body.is_empty() {
        return out;
    }
    // Character indices, because Turkish text is not one byte per character and slicing a
    // UTF-8 string on a byte boundary panics.
    let chars: Vec<char> = body.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + MAX_CHARS).min(chars.len());
        if end < chars.len() {
            // Prefer to break on a line boundary in the back half of the window.
            if let Some(cut) = chars[start + MAX_CHARS / 2..end]
                .iter()
                .rposition(|c| *c == '\n')
            {
                end = start + MAX_CHARS / 2 + cut;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(format!("{title}\n{piece}"));
        }
        if end >= chars.len() || out.len() >= MAX_CHUNKS_PER_DOC {
            break;
        }
        start = end.saturating_sub(OVERLAP).max(start + 1);
    }
    out
}

/// Everything `index-corpus` was asked to do, parsed once.
#[derive(Debug)]
pub struct Options {
    pub forms: PathBuf,
    pub model: Option<PathBuf>,
    pub out: PathBuf,
    pub source: String,
    pub built: String,
    pub update_url: Option<String>,
    pub text_base: Option<String>,
    /// Ignore the existing output and embed everything.
    pub fresh: bool,
}

/// What one document needs: reuse from the previous index, or embedding.
enum Plan {
    Reuse { prev_doc: usize },
    Embed,
}

/// Decide, per form, whether the previous index already holds its vectors. Pure — this is
/// the rule, and it is what the tests pin.
fn plan(forms: &[Form], previous: Option<&Corpus>) -> Vec<(Plan, String)> {
    let by_id: HashMap<&str, usize> = previous
        .map(|c| c.docs().iter().enumerate().map(|(i, d)| (d.id.as_str(), i)).collect())
        .unwrap_or_default();
    forms
        .iter()
        .map(|f| {
            let hash = fingerprint(&f.title, &f.text);
            let reuse = previous.and_then(|c| {
                let i = *by_id.get(f.id.as_str())?;
                (c.docs()[i].hash.as_deref() == Some(hash.as_str())).then_some(i)
            });
            match reuse {
                Some(prev_doc) => (Plan::Reuse { prev_doc }, hash),
                None => (Plan::Embed, hash),
            }
        })
        .collect()
}

pub fn run(opts: &Options) -> Result<()> {
    let forms = read_forms(&opts.forms)?;
    println!("  {} documents from {}", forms.len(), opts.forms.display());

    // The previous index, when there is one and it is wanted. It must be OUR model's: a
    // vector from a different embedder is not reusable, and Corpus::read already refuses it.
    let previous = if opts.fresh || !opts.out.is_file() {
        None
    } else {
        match Corpus::read(&opts.out) {
            Ok(c) => {
                println!("  previous index: {} documents, built {}", c.docs().len(), c.header.built);
                Some(c)
            }
            Err(e) => {
                println!("  previous index at {} is unusable ({e}) — embedding everything", opts.out.display());
                None
            }
        }
    };

    let plans = plan(&forms, previous.as_ref());
    let to_embed = plans.iter().filter(|(p, _)| matches!(p, Plan::Embed)).count();
    let reused = forms.len() - to_embed;
    let removed = previous
        .as_ref()
        .map(|c| {
            let keep: std::collections::HashSet<&str> = forms.iter().map(|f| f.id.as_str()).collect();
            c.docs().iter().filter(|d| !keep.contains(d.id.as_str())).count()
        })
        .unwrap_or(0);
    println!("  reuse {reused} · embed {to_embed} · removed {removed}");

    // Chunk everything (cheap, and it is what the header stores), but only collect the
    // texts that actually need the model.
    let mut docs = Vec::with_capacity(forms.len());
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut vectors: Vec<f32> = Vec::new();
    let mut pending: Vec<(usize, String)> = Vec::new(); // (chunk index, text) awaiting a vector

    for (i, (f, (p, hash))) in forms.iter().zip(plans.iter()).enumerate() {
        match p {
            Plan::Reuse { prev_doc } => {
                let prev = previous.as_ref().expect("a reuse plan implies a previous index");
                for (ci, c) in prev.chunks().iter().enumerate() {
                    if c.doc as usize == *prev_doc {
                        chunks.push(Chunk { doc: i as u32, ord: c.ord, text: c.text.clone() });
                        vectors.extend_from_slice(prev.vector(ci));
                    }
                }
            }
            Plan::Embed => {
                for (ord, piece) in chunk(&f.title, &f.text).into_iter().enumerate() {
                    chunks.push(Chunk { doc: i as u32, ord: ord as u32, text: piece.clone() });
                    pending.push((chunks.len() - 1, piece));
                    vectors.extend(std::iter::repeat(0.0).take(DIM));
                }
            }
        }
        docs.push(Doc {
            id: f.id.clone(),
            code: f.code.clone(),
            rev: f.rev,
            lang: f.lang.clone(),
            title: f.title.clone(),
            name: f.name.clone(),
            ext: f.ext.clone(),
            url: f.url.clone(),
            chars: f.text.chars().count() as u64,
            hash: Some(hash.clone()),
        });
    }
    println!("  {} passages, {} to embed", chunks.len(), pending.len());

    if !pending.is_empty() {
        let model = opts
            .model
            .as_deref()
            .context("--model <dir> is required: there are documents to embed")?;
        println!("  loading the embedder from {}", model.display());
        let embedder = Embedder::load(model)?;
        let total = pending.len();
        for (n, batch) in pending.chunks(BATCH).enumerate() {
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let embedded = embedder
                .passages(&texts)
                .with_context(|| format!("embedding batch {n}"))?;
            for ((ci, _), v) in batch.iter().zip(embedded) {
                anyhow::ensure!(v.len() == DIM, "the model returned {} dims, expected {DIM}", v.len());
                vectors[ci * DIM..(ci + 1) * DIM].copy_from_slice(&v);
            }
            let done = ((n + 1) * BATCH).min(total);
            if n % 10 == 0 || done == total {
                println!("  embedded {done}/{total}");
            }
        }
    }

    let corpus = Corpus {
        header: Header {
            version: 1,
            model: MODEL_ID.into(),
            dim: DIM as u32,
            built: opts.built.clone(),
            source: opts.source.clone(),
            // Carried forward from the previous index unless this build says otherwise, so
            // an incremental update never silently drops where updates come from.
            update_url: opts
                .update_url
                .clone()
                .or_else(|| previous.as_ref().and_then(|c| c.header.update_url.clone())),
            text_base: opts
                .text_base
                .clone()
                .or_else(|| previous.as_ref().and_then(|c| c.header.text_base.clone())),
            default_family: most_common_family(&forms),
            docs,
            chunks,
        },
        vectors,
    };
    corpus.write(&opts.out)?;

    // Read it back before declaring success: the app will run exactly this check on a
    // user's machine, and finding out there rather than here is the wrong order.
    let back = Corpus::read(&opts.out).context("the index we just wrote does not load")?;
    let size = std::fs::metadata(&opts.out).map(|m| m.len()).unwrap_or(0);
    println!(
        "\n  {} — {} documents, {} passages, {:.1} MB (family {}, updates from {})",
        opts.out.display(),
        back.docs().len(),
        back.chunks().len(),
        size as f64 / 1_048_576.0,
        back.header.default_family.as_deref().unwrap_or("—"),
        back.header.update_url.as_deref().unwrap_or("—"),
    );
    Ok(())
}

/// Parse the maintainer verb's arguments.
pub fn from_args(args: &[String]) -> Result<Options> {
    let mut o = Options {
        forms: PathBuf::from("forms"),
        model: None,
        out: PathBuf::from("corpus.gtu"),
        source: String::new(),
        // No clock here: the build date is an input, so two runs over the same database
        // produce byte-identical output and a release is reproducible.
        built: String::new(),
        update_url: None,
        text_base: None,
        fresh: false,
    };
    let mut i = 0;
    let value = |args: &[String], i: usize, flag: &str| -> Result<String> {
        args.get(i + 1).cloned().with_context(|| format!("{flag} needs a value"))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--model" => { o.model = Some(PathBuf::from(value(args, i, "--model")?)); i += 2 }
            "--out" => { o.out = PathBuf::from(value(args, i, "--out")?); i += 2 }
            "--source" => { o.source = value(args, i, "--source")?; i += 2 }
            "--built" => { o.built = value(args, i, "--built")?; i += 2 }
            "--update-url" => { o.update_url = Some(value(args, i, "--update-url")?); i += 2 }
            "--text-base" => { o.text_base = Some(value(args, i, "--text-base")?); i += 2 }
            "--fresh" => { o.fresh = true; i += 1 }
            other if !other.starts_with("--") => { o.forms = PathBuf::from(other); i += 1 }
            other => anyhow::bail!("unknown option `{other}`"),
        }
    }
    anyhow::ensure!(!o.built.is_empty(), "--built <YYYY-MM-DD> is required, so the build is reproducible");
    if o.source.is_empty() {
        o.source = crate::provision::DEFAULT_SOURCE.to_string();
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chunk_carries_the_title() {
        // A form body is a table of blanks; a passage that lost its title is unfindable.
        let body = "satır\n".repeat(600);
        let chunks = chunk("Danışman Değişikliği Formu", &body);
        assert!(chunks.len() > 1, "a long body must split");
        assert!(chunks.iter().all(|c| c.starts_with("Danışman Değişikliği Formu")));
    }

    /// The first passage is the title ALONE — one vector per form that means what the form
    /// is, rather than what every form's paperwork looks like.
    #[test]
    fn the_first_passage_is_the_bare_title() {
        let chunks = chunk("Danışman Değişikliği Formu", "ADI SOYADI\nİMZA\nUYGUNDUR");
        assert_eq!(chunks[0], "Danışman Değişikliği Formu");
        assert!(chunks.len() > 1, "the body must still be indexed too");
        assert!(chunks[1].contains("ADI SOYADI"));
    }

    #[test]
    fn a_form_with_no_text_still_produces_its_title() {
        // 3 of the 791 have no extractable body; they must still be searchable by name.
        assert_eq!(chunk("Staj Belgesi", ""), vec!["Staj Belgesi"]);
        assert_eq!(chunk("Staj Belgesi", "   \n  "), vec!["Staj Belgesi"]);
    }

    #[test]
    fn chunking_turkish_text_never_splits_a_character() {
        // The bug this guards: slicing a UTF-8 String on a byte index panics, and Turkish
        // is full of two-byte characters. A pure-ASCII test would never catch it.
        let body = "şğüöçİĞÜÖÇı".repeat(400);
        let chunks = chunk("Başlık", &body);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() > 0);
        }
    }

    #[test]
    fn a_runaway_document_is_capped() {
        let body = "x".repeat(MAX_CHARS * 200);
        assert!(chunk("T", &body).len() <= MAX_CHUNKS_PER_DOC);
    }

    #[test]
    fn the_defaults_point_at_the_committed_database() {
        let args: Vec<String> = ["--model", "m", "--built", "2026-08-13"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let o = from_args(&args).unwrap();
        assert_eq!(o.forms, PathBuf::from("forms"));
        assert_eq!(o.out, PathBuf::from("corpus.gtu"));
        assert_eq!(o.built, "2026-08-13");
        assert!(!o.fresh, "incremental is the default");
    }

    #[test]
    fn a_build_without_a_date_is_refused_rather_than_stamped_from_the_clock() {
        let args: Vec<String> = ["--model", "m"].iter().map(|s| s.to_string()).collect();
        assert!(from_args(&args).unwrap_err().to_string().contains("--built"));
    }

    fn form(id: &str, title: &str, text: &str) -> Form {
        Form {
            id: id.into(), code: Some(id.trim_end_matches(".tr").into()), rev: 1,
            lang: "tr".into(), title: title.into(), name: format!("{id}.pdf"),
            ext: "pdf".into(), url: "u".into(), text: text.into(),
        }
    }

    /// A previous index holding exactly these forms, fingerprinted the way run() does.
    fn previous(forms: &[Form]) -> Corpus {
        let mut docs = Vec::new();
        let mut chunks = Vec::new();
        let mut vectors = Vec::new();
        for (i, f) in forms.iter().enumerate() {
            for (ord, piece) in chunk(&f.title, &f.text).into_iter().enumerate() {
                chunks.push(Chunk { doc: i as u32, ord: ord as u32, text: piece });
                vectors.extend(std::iter::repeat(i as f32 + 1.0).take(DIM));
            }
            docs.push(Doc {
                id: f.id.clone(), code: f.code.clone(), rev: f.rev, lang: f.lang.clone(),
                title: f.title.clone(), name: f.name.clone(), ext: f.ext.clone(),
                url: f.url.clone(), chars: 0, hash: Some(fingerprint(&f.title, &f.text)),
            });
        }
        Corpus {
            header: Header {
                version: 1, model: MODEL_ID.into(), dim: DIM as u32, built: "2026-09-01".into(),
                source: "s".into(), update_url: Some("https://x/corpus.gtu".into()),
                text_base: None, default_family: None, docs, chunks,
            },
            vectors,
        }
    }

    /// The whole point of the incremental build: an unchanged document is never embedded
    /// again, a changed one is, a new one is, and a removed one simply is not there.
    #[test]
    fn only_what_changed_is_embedded() {
        let old = vec![form("FR-0001.tr", "A", "aaa"), form("FR-0002.tr", "B", "bbb"), form("FR-0003.tr", "C", "ccc")];
        let prev = previous(&old);
        let now = vec![
            form("FR-0001.tr", "A", "aaa"),          // unchanged
            form("FR-0002.tr", "B", "bbb changed"),  // text changed
            form("FR-0004.tr", "D", "ddd"),          // new; FR-0003 removed
        ];
        let plans = plan(&now, Some(&prev));
        assert!(matches!(plans[0].0, Plan::Reuse { prev_doc: 0 }));
        assert!(matches!(plans[1].0, Plan::Embed));
        assert!(matches!(plans[2].0, Plan::Embed));
    }

    #[test]
    fn a_title_change_alone_re_embeds_because_the_title_rides_every_passage() {
        let old = vec![form("FR-0001.tr", "Old title", "same text")];
        let prev = previous(&old);
        let now = vec![form("FR-0001.tr", "New title", "same text")];
        assert!(matches!(plan(&now, Some(&prev))[0].0, Plan::Embed));
    }

    #[test]
    fn without_a_previous_index_everything_is_embedded() {
        let now = vec![form("FR-0001.tr", "A", "a")];
        assert!(matches!(plan(&now, None)[0].0, Plan::Embed));
    }

    #[test]
    fn the_fingerprint_is_stable_and_sensitive() {
        assert_eq!(fingerprint("T", "x"), fingerprint("T", "x"));
        assert_ne!(fingerprint("T", "x"), fingerprint("T", "y"));
        assert_ne!(fingerprint("T", "x"), fingerprint("U", "x"));
        // A known value, so a toolchain upgrade that changed it would fail here rather than
        // silently re-embedding two thousand documents.
        assert_eq!(fingerprint("T", "x"), "efedc68965b100a2");
    }

    #[test]
    fn the_default_family_is_the_most_common_one() {
        let forms = vec![form("FR-0001.tr", "A", "a"), form("FR-0002.tr", "B", "b"), form("YÖ-0001.tr", "C", "c")];
        assert_eq!(most_common_family(&forms).as_deref(), Some("FR"));
        assert_eq!(most_common_family(&[]), None);
    }
}
