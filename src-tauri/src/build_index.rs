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
//!     gturag index-corpus forms --model <dir> --built 2026-08-13 --out corpus.gtu

use crate::corpus::{Chunk, Corpus, Doc, Header, DIM, MODEL_ID};
use crate::embed::{Embedder, BATCH};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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

pub fn run(forms_dir: &Path, model: &Path, out: &Path, source: &str, built: &str) -> Result<()> {
    let forms = read_forms(forms_dir)?;
    println!("  {} forms from {}", forms.len(), forms_dir.display());

    let mut docs = Vec::with_capacity(forms.len());
    let mut chunks = Vec::new();
    let mut texts = Vec::new();
    for (i, f) in forms.iter().enumerate() {
        for (ord, piece) in chunk(&f.title, &f.text).into_iter().enumerate() {
            chunks.push(Chunk { doc: i as u32, ord: ord as u32, text: piece.clone() });
            texts.push(piece);
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
            hash: None,
        });
    }
    println!("  {} passages", chunks.len());

    println!("  loading the embedder from {}", model.display());
    let embedder = Embedder::load(model)?;

    let mut vectors: Vec<f32> = Vec::with_capacity(texts.len() * DIM);
    let total = texts.len();
    for (n, batch) in texts.chunks(BATCH).enumerate() {
        let embedded = embedder
            .passages(batch)
            .with_context(|| format!("embedding batch {n}"))?;
        for v in embedded {
            anyhow::ensure!(v.len() == DIM, "the model returned {} dims, expected {DIM}", v.len());
            vectors.extend(v);
        }
        let done = ((n + 1) * BATCH).min(total);
        if n % 10 == 0 || done == total {
            println!("  embedded {done}/{total}");
        }
    }

    let corpus = Corpus {
        header: Header {
            version: 1,
            model: MODEL_ID.into(),
            dim: DIM as u32,
            built: built.to_string(),
            source: source.to_string(),
            update_url: None,
            text_base: None,
            default_family: None,
            docs,
            chunks,
        },
        vectors,
    };
    corpus.write(out)?;

    // Read it back before declaring success: the app will run exactly this check on a
    // user's machine, and finding out there rather than here is the wrong order.
    let back = Corpus::read(out).context("the index we just wrote does not load")?;
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    println!(
        "\n  {} — {} forms, {} passages, {:.1} MB",
        out.display(),
        back.docs().len(),
        back.chunks().len(),
        size as f64 / 1_048_576.0
    );
    Ok(())
}

/// Parse the maintainer verb's arguments.
pub fn from_args(args: &[String]) -> Result<(PathBuf, PathBuf, PathBuf, String, String)> {
    let mut forms = PathBuf::from("forms");
    let mut model = PathBuf::new();
    let mut out = PathBuf::from("corpus.gtu");
    let mut source = "https://www.gtu.edu.tr/kategori/2382/0/display.aspx".to_string();
    // No clock here: the build date is an input, so two runs over the same database
    // produce byte-identical output and a release is reproducible.
    let mut built = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => { model = PathBuf::from(args.get(i + 1).context("--model needs a path")?); i += 2 }
            "--out" => { out = PathBuf::from(args.get(i + 1).context("--out needs a path")?); i += 2 }
            "--source" => { source = args.get(i + 1).context("--source needs a URL")?.clone(); i += 2 }
            "--built" => { built = args.get(i + 1).context("--built needs a date")?.clone(); i += 2 }
            other if !other.starts_with("--") => { forms = PathBuf::from(other); i += 1 }
            other => anyhow::bail!("unknown option `{other}`"),
        }
    }
    anyhow::ensure!(!model.as_os_str().is_empty(), "--model <dir> is required (the folder holding model.safetensors)");
    anyhow::ensure!(!built.is_empty(), "--built <YYYY-MM-DD> is required, so the build is reproducible");
    Ok((forms, model, out, source, built))
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
        let (forms, _, out, _, built) = from_args(&args).unwrap();
        assert_eq!(forms, PathBuf::from("forms"));
        assert_eq!(out, PathBuf::from("corpus.gtu"));
        assert_eq!(built, "2026-08-13");
    }

    #[test]
    fn a_build_without_a_date_is_refused_rather_than_stamped_from_the_clock() {
        let args: Vec<String> = ["--model", "m"].iter().map(|s| s.to_string()).collect();
        assert!(from_args(&args).unwrap_err().to_string().contains("--built"));
    }
}
