//! The corpus file: what the offline builder writes and the app reads.
//!
//! One file, not three, because provisioning is a step the human watches: "downloading
//! the index" must either finish or not, and a half-arrived set of three files that each
//! parsed fine on its own is the failure mode that reads as corruption much later.
//!
//! The layout is a JSON header for everything variable-width, then the vectors as a raw
//! little-endian `f32` block. The vectors are 90% of the bytes and are the one thing that
//! must not go through a JSON parser — 60k floats in JSON is 20× the size and a second of
//! parse time on every launch. Everything else is a struct, so it stays legible and
//! versioned rather than clever.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

/// `GTURAG` + a format major. Bumping the major is what makes an old cached index be
/// re-fetched rather than mis-read.
pub const MAGIC: &[u8; 8] = b"GTURAG\x00\x01";

/// The embedder these vectors came from. An index built with one model and queried with
/// another returns confident nonsense — nothing crashes, the scores are just meaningless —
/// so the app refuses to load an index whose model is not the one it embeds queries with.
pub const MODEL_ID: &str = "intfloat/multilingual-e5-small";
pub const DIM: usize = 384;

/// One document, as both surfaces show it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Doc {
    /// Stable, filesystem-safe: `FR-0083.tr`.
    pub id: String,
    /// `FR-0083`, or `None` for the handful of forms GTÜ publishes without a code.
    pub code: Option<String>,
    /// Revision, from the `R<n>` marker in the filename. 0 when unmarked.
    pub rev: u32,
    /// `tr` or `en`.
    pub lang: String,
    /// The form's human name, with the code and revision marker stripped off.
    pub title: String,
    /// The filename as published — what a human sees if they download it.
    pub name: String,
    pub ext: String,
    /// Where it came from. The window links to this and `get` downloads it.
    pub url: String,
    /// Characters of body text extracted. Zero means title-only — worth surfacing, since
    /// a title-only document answers name queries and nothing else.
    pub chars: u64,
}

/// One passage. `doc` indexes into [`Corpus::docs`], so a document's metadata is stored
/// once however many chunks it produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub doc: u32,
    pub ord: u32,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub model: String,
    pub dim: u32,
    /// ISO date the builder ran. Shown in `status` so a stale index is visible rather
    /// than merely suspected.
    pub built: String,
    /// The page this was scraped from — the provenance both surfaces can cite.
    pub source: String,
    pub docs: Vec<Doc>,
    pub chunks: Vec<Chunk>,
}

/// A loaded corpus: the header plus the vector block, `chunks.len() × dim` row-major and
/// already L2-normalised, so a cosine similarity is a plain dot product.
pub struct Corpus {
    pub header: Header,
    pub vectors: Vec<f32>,
}

/// Deliberately hand-written, not derived: a derive would print all 800k floats the first
/// time a test failed or a `{:?}` reached a log, which is not a debugging aid.
impl std::fmt::Debug for Corpus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Corpus")
            .field("model", &self.header.model)
            .field("built", &self.header.built)
            .field("docs", &self.header.docs.len())
            .field("chunks", &self.header.chunks.len())
            .field("dim", &self.header.dim)
            .finish()
    }
}

impl Corpus {
    pub fn docs(&self) -> &[Doc] {
        &self.header.docs
    }

    pub fn chunks(&self) -> &[Chunk] {
        &self.header.chunks
    }

    pub fn dim(&self) -> usize {
        self.header.dim as usize
    }

    /// The vector for one chunk.
    pub fn vector(&self, chunk: usize) -> &[f32] {
        let d = self.dim();
        &self.vectors[chunk * d..(chunk + 1) * d]
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec(&self.header)?;
        let mut out = Vec::with_capacity(json.len() + self.vectors.len() * 4 + 12);
        out.write_all(MAGIC)?;
        out.write_all(&(json.len() as u32).to_le_bytes())?;
        out.write_all(&json)?;
        for v in &self.vectors {
            out.write_all(&v.to_le_bytes())?;
        }
        // Through clappkit's store, so a half-written index can never replace a good one.
        clappkit::store::atomic_write(path, &out)
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Corpus> {
        let mut f = std::fs::File::open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        Corpus::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Corpus> {
        if bytes.len() < 12 || &bytes[..8] != MAGIC {
            bail!("not a GTÜ Formlar index (bad magic) — the download was truncated or is the wrong file");
        }
        let json_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let end = 12 + json_len;
        if bytes.len() < end {
            bail!("index header is truncated ({} bytes, header claims {json_len})", bytes.len());
        }
        let header: Header = serde_json::from_slice(&bytes[12..end])
            .context("the index header will not parse")?;

        if header.model != MODEL_ID {
            // Not pedantry: mismatched vectors produce confident, meaningless rankings.
            bail!(
                "index was built with {} but this app embeds queries with {MODEL_ID} — \
                 the scores would be meaningless. Run `gturag sync` for a matching index.",
                header.model
            );
        }
        let dim = header.dim as usize;
        let want = header.chunks.len() * dim * 4;
        let got = bytes.len() - end;
        if got != want {
            bail!(
                "index has {} chunks × {dim} dims = {want} vector bytes, but {got} are present",
            header.chunks.len()
            );
        }
        let mut vectors = Vec::with_capacity(header.chunks.len() * dim);
        for c in bytes[end..].chunks_exact(4) {
            vectors.push(f32::from_le_bytes(c.try_into().unwrap()));
        }
        // Every chunk must point at a document that exists, or ranking panics on the
        // first hit rather than at load, which is the wrong place to find out.
        let n = header.docs.len() as u32;
        if let Some(bad) = header.chunks.iter().find(|c| c.doc >= n) {
            bail!("index chunk points at document {} of {n}", bad.doc);
        }
        Ok(Corpus { header, vectors })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Corpus {
        Corpus {
            header: Header {
                version: 1,
                model: MODEL_ID.into(),
                dim: 2,
                built: "2026-08-12".into(),
                source: "https://example.invalid".into(),
                docs: vec![Doc {
                    id: "FR-0083.tr".into(),
                    code: Some("FR-0083".into()),
                    rev: 1,
                    lang: "tr".into(),
                    title: "Danışman Değişikliği Formu".into(),
                    name: "FR-0083 … R1.pdf".into(),
                    ext: "pdf".into(),
                    url: "https://example.invalid/f.pdf".into(),
                    chars: 120,
                }],
                chunks: vec![
                    Chunk { doc: 0, ord: 0, text: "danışman değişikliği".into() },
                    Chunk { doc: 0, ord: 1, text: "advisor change".into() },
                ],
            },
            vectors: vec![1.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn a_corpus_round_trips_through_bytes() {
        let dir = std::env::temp_dir().join(format!("gturag-corpus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corpus.gtu");
        tiny().write(&path).unwrap();

        let back = Corpus::read(&path).unwrap();
        assert_eq!(back.docs().len(), 1);
        assert_eq!(back.chunks().len(), 2);
        assert_eq!(back.vector(1), &[0.0, 1.0]);
        assert_eq!(back.docs()[0].code.as_deref(), Some("FR-0083"));
        assert_eq!(back.docs()[0].title, "Danışman Değişikliği Formu", "UTF-8 must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_an_index_is_refused_by_name() {
        let err = Corpus::from_bytes(b"<!doctype html><html>404").unwrap_err().to_string();
        assert!(err.contains("bad magic"), "{err}");
    }

    #[test]
    fn an_index_from_a_different_model_is_refused_rather_than_ranked() {
        let mut c = tiny();
        c.header.model = "some/other-model".into();
        let json = serde_json::to_vec(&c.header).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&json);
        for v in &c.vectors {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let err = Corpus::from_bytes(&bytes).unwrap_err().to_string();
        assert!(err.contains("meaningless"), "the refusal must say why: {err}");
    }

    #[test]
    fn a_truncated_vector_block_is_caught_at_load() {
        let dir = std::env::temp_dir().join(format!("gturag-trunc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corpus.gtu");
        tiny().write(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 4); // one vector short: a half-finished download
        let err = Corpus::from_bytes(&bytes).unwrap_err().to_string();
        assert!(err.contains("vector bytes"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
