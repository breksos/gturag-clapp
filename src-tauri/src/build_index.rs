//! Stage 2 of the corpus build: embed the passages `fetch.py` extracted, and write the
//! `corpus.gtu` the app downloads.
//!
//! This lives in the app's own binary rather than in a script, for one reason: it must use
//! [`crate::embed`] — the very same code that embeds queries at runtime. A separate
//! implementation (a Python script with sentence-transformers, say) would agree with this
//! one until it quietly didn't, and the symptom of a passage/query mismatch is not an
//! error, it is search that is a little bit wrong forever.
//!
//! It is a maintainer verb, deliberately absent from `clatch.json`'s `connector.commands`
//! and from `gturag -h`: an agent is never granted it, because it is part of building a
//! release, not of using the app.
//!
//!     gturag index-corpus tools/build-index/work --model <dir> --out corpus.gtu

use crate::corpus::{Chunk, Corpus, Doc, Header, DIM, MODEL_ID};
use crate::embed::{Embedder, BATCH};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// `docs.jsonl`, as `fetch.py` writes it.
#[derive(serde::Deserialize)]
struct RawDoc {
    id: String,
    code: Option<String>,
    rev: u32,
    lang: String,
    title: String,
    name: String,
    ext: String,
    url: String,
    chars: u64,
}

#[derive(serde::Deserialize)]
struct RawChunk {
    doc_id: String,
    ord: u32,
    text: String,
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {} — run tools/build-index/fetch.py first", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line)
                .with_context(|| format!("{}:{} will not parse", path.display(), i + 1))?,
        );
    }
    Ok(out)
}

pub fn run(work: &Path, model: &Path, out: &Path, source: &str, built: &str) -> Result<()> {
    let raw_docs: Vec<RawDoc> = read_jsonl(&work.join("docs.jsonl"))?;
    let raw_chunks: Vec<RawChunk> = read_jsonl(&work.join("chunks.jsonl"))?;
    println!("  {} documents, {} passages", raw_docs.len(), raw_chunks.len());

    let docs: Vec<Doc> = raw_docs
        .into_iter()
        .map(|d| Doc {
            id: d.id,
            code: d.code,
            rev: d.rev,
            lang: d.lang,
            title: d.title,
            name: d.name,
            ext: d.ext,
            url: d.url,
            chars: d.chars,
        })
        .collect();

    // Chunks carry a document *id* on the wire and a document *index* in the file. Resolve
    // once, here, and fail loudly on an orphan rather than letting `Corpus::read` reject
    // the finished artifact after an hour of embedding.
    let mut chunks = Vec::with_capacity(raw_chunks.len());
    let mut texts = Vec::with_capacity(raw_chunks.len());
    for rc in raw_chunks {
        let doc = docs
            .iter()
            .position(|d| d.id == rc.doc_id)
            .with_context(|| format!("passage references unknown document `{}`", rc.doc_id))?;
        chunks.push(Chunk { doc: doc as u32, ord: rc.ord, text: rc.text.clone() });
        texts.push(rc.text);
    }

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
        "\n  {} — {} documents, {} passages, {:.1} MB",
        out.display(),
        back.docs().len(),
        back.chunks().len(),
        size as f64 / 1_048_576.0
    );
    Ok(())
}

/// Parse the maintainer verb's arguments.
pub fn from_args(args: &[String]) -> Result<(PathBuf, PathBuf, PathBuf, String, String)> {
    let mut work = PathBuf::from("tools/build-index/work");
    let mut model = PathBuf::new();
    let mut out = PathBuf::from("corpus.gtu");
    let mut source = "https://www.gtu.edu.tr/kategori/2382/0/display.aspx".to_string();
    // No clock here: the build date is an input, so two runs of the same corpus produce
    // byte-identical output and a release is reproducible.
    let mut built = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => { model = PathBuf::from(args.get(i + 1).context("--model needs a path")?); i += 2 }
            "--out" => { out = PathBuf::from(args.get(i + 1).context("--out needs a path")?); i += 2 }
            "--source" => { source = args.get(i + 1).context("--source needs a URL")?.clone(); i += 2 }
            "--built" => { built = args.get(i + 1).context("--built needs a date")?.clone(); i += 2 }
            other if !other.starts_with("--") => { work = PathBuf::from(other); i += 1 }
            other => anyhow::bail!("unknown option `{other}`"),
        }
    }
    anyhow::ensure!(!model.as_os_str().is_empty(), "--model <dir> is required (the folder holding model.safetensors)");
    anyhow::ensure!(!built.is_empty(), "--built <YYYY-MM-DD> is required, so the build is reproducible");
    Ok((work, model, out, source, built))
}
