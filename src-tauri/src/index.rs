//! Retrieval: how a query becomes a ranked list of forms.
//!
//! Three things run, in this order, and the order is the design:
//!
//! 1. **An exact form code is decisive.** "FR-0083" is not a topic, it is a name, and a
//!    user who types a name has already told us the answer. PLAYBOOK §15's corollary — a
//!    clear winner is not ambiguity, and an exact name match is decisive — so a matched
//!    code short-circuits to the top and nothing downstream can outrank it.
//! 2. **BM25 over the passage text**, because half of what people type at a form registry
//!    is literal: "staj", "yandal", "mazeret". A dense model will happily rank a
//!    semantically-adjacent form above a literal title match, which is the wrong answer.
//! 3. **Dense cosine over the embeddings**, because the other half is not literal at all:
//!    "danışmanımı değiştirmek istiyorum" names no word in the form's title.
//!
//! (2) and (3) are fused by Reciprocal Rank Fusion rather than by adding scores. BM25
//! scores are unbounded and corpus-relative, cosines are in [-1,1]; any weighted sum of
//! the two is a constant nobody can tune honestly. RRF only reads the RANKS, so it needs
//! no calibration and cannot be dominated by whichever scale happens to be larger.
//!
//! Everything here is pure: it takes a corpus and a query vector and returns hits. No I/O,
//! no model, no clock — which is what makes the ranking rules testable, and is why the
//! rules actually live in the tests below.

use crate::corpus::{Corpus, Doc};
use crate::embed::cosine;
use std::collections::HashMap;

/// RRF's smoothing constant. 60 is the value from the original paper and the one every
/// implementation uses; it decides how quickly rank 1 stops dominating rank 5.
const RRF_K: f32 = 60.0;

/// BM25 term-frequency saturation and length normalisation. The defaults; a form corpus
/// gives no reason to depart from them.
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// How many chunks each retriever contributes to the fusion. Deeper than the page the
/// human sees, so a document that ranks mid-list in one retriever and top in the other
/// still surfaces.
const CANDIDATES: usize = 200;

/// What a fully-covered title is worth, on the same scale as the fused RRF score.
///
/// Two retrievers each contribute at most `1/(60+1) ≈ 0.0164`, so an unbeatable ceiling of
/// evidence from body text and semantics alone is about `0.033`. At 0.20, a title that
/// contains every term of the query cannot be outvoted by any amount of that — while a
/// quarter-covered title (0.20 × 0.25² = 0.0125) stays below it, as it should.
const TITLE_WEIGHT: f32 = 0.20;

/// Why a document is in the results — shown in both surfaces, because "it matched the
/// code you typed" and "it looked similar" deserve different trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// The query named this form's code.
    Code,
    /// Fused lexical + semantic match.
    Match,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Hit {
    /// Index into [`Corpus::docs`].
    pub doc: usize,
    pub score: f32,
    pub why: Why,
    /// The passage that matched best — what the window shows under the title.
    pub snippet: String,
    /// The other passages of this form that also matched, best first. The window shows
    /// them so a human can see WHERE in a document the answer is, rather than trusting one
    /// excerpt the ranker happened to prefer.
    pub passages: Vec<String>,
}

/// How many passages of one form the window will show. Enough to see where a form talks
/// about the thing you asked for; few enough that one long spreadsheet cannot push every
/// other result off the screen.
const PASSAGES_SHOWN: usize = 3;

/// How results are ordered. This is STATE, not a per-call argument: a sort that only
/// reorders the page the caller happens to hold is a lie about the data underneath it
/// (PLAYBOOK §11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Relevance,
    Code,
    Title,
}

impl Sort {
    pub fn parse(s: &str) -> Option<Sort> {
        match s.trim().to_ascii_lowercase().as_str() {
            "relevance" | "relevans" | "score" => Some(Sort::Relevance),
            "code" | "kod" | "form" => Some(Sort::Code),
            "title" | "ad" | "isim" | "name" => Some(Sort::Title),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Relevance => "relevance",
            Sort::Code => "code",
            Sort::Title => "title",
        }
    }
}

/// Lowercase the Turkish way, then split on anything that is not a letter or digit.
///
/// `to_lowercase` is not enough and the failure is not cosmetic: Rust maps `I` to `i` and
/// `İ` to `i̇` (an `i` plus a combining dot), so "İZİN" and "izin" tokenise differently
/// and a search for the word as the user typed it misses the form that contains it. The
/// two dotted/dotless pairs are handled first, and everything else takes the default.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut folded = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            'I' => folded.push('ı'),
            'İ' => folded.push('i'),
            _ => folded.extend(ch.to_lowercase()),
        }
    }
    let mut out = Vec::new();
    for raw in folded.split(|c: char| !c.is_alphanumeric()) {
        if raw.len() < 2 && !raw.chars().next().is_some_and(|c| c.is_numeric()) {
            continue;
        }
        // Both the word and its stem go in. Turkish is agglutinative — `danışman`,
        // `danışmanımı`, `danışmanlık` and `danışmanı` are the same word wearing different
        // suffixes, and a query never wears the same one the form's title does. Emitting
        // BOTH means an exact match still scores twice (word + stem) and therefore still
        // outranks a merely-related word, while `danışmanımı` can at last find
        // `Danışman Değişikliği Formu` at all.
        out.push(raw.to_string());
        if let Some(s) = stem(raw) {
            out.push(s);
        }
    }
    out
}

/// How many characters of a word survive stemming. Turkish IR gets most of the benefit of
/// a full morphological analyser from fixed-prefix truncation, and this length is the one
/// usually reported as the sweet spot: long enough that `değişiklik`/`değiştirmek` collapse
/// to `değiş` without `form` and `formasyon` colliding.
const STEM_LEN: usize = 5;

/// The stem of a word, or `None` when it is already at or below [`STEM_LEN`] (in which case
/// it would duplicate the word itself and double-count in BM25).
fn stem(word: &str) -> Option<String> {
    let mut chars = word.chars();
    let prefix: String = chars.by_ref().take(STEM_LEN).collect();
    // `chars` is now positioned past the prefix; if anything remains, the word was longer.
    if chars.next().is_some() {
        Some(prefix)
    } else {
        None
    }
}

/// Pull a form code out of free text: `FR-0083`, `fr 83`, `FR_0083`, bare `0083`.
/// Normalised to the canonical `FR-0083` so it can be compared against [`Doc::code`].
pub fn find_code(query: &str) -> Option<String> {
    let up: String = query
        .chars()
        .map(|c| if c == 'ı' { 'I' } else { c })
        .flat_map(char::to_uppercase)
        .collect();
    // With a prefix: the normal case.
    if let Some(caps) = regex_lite_code(&up) {
        return Some(caps);
    }
    // A bare number, but only if the query is essentially just that number — "2 nüsha"
    // must not be read as FR-0002.
    let t = up.trim();
    if t.len() >= 2 && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("FR-{:04}", t.parse::<u32>().ok()?));
    }
    None
}

/// A two-letter prefix, an optional separator, then 2–4 digits. Hand-rolled rather than
/// pulling in a regex crate for one pattern the app uses once per query.
fn regex_lite_code(up: &str) -> Option<String> {
    let b: Vec<char> = up.chars().collect();
    for i in 0..b.len().saturating_sub(2) {
        if !b[i].is_ascii_alphabetic() || !b[i + 1].is_ascii_alphabetic() {
            continue;
        }
        // Must start a word, or "PROJEFR-1" would match.
        if i > 0 && b[i - 1].is_alphanumeric() {
            continue;
        }
        let mut j = i + 2;
        while j < b.len() && (b[j] == '-' || b[j] == '_' || b[j] == ' ') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let digits = j - start;
        if (2..=4).contains(&digits) {
            let n: u32 = b[start..j].iter().collect::<String>().parse().ok()?;
            let prefix: String = b[i..i + 2].iter().collect();
            return Some(format!("{prefix}-{n:04}"));
        }
    }
    None
}

/// One BM25-scorable field: term frequencies per unit, plus the corpus statistics its
/// scores need. Split out because the corpus has two of them — passage bodies and titles —
/// and they must be scored against their OWN statistics. A title is six words long; scoring
/// it against the average passage length would penalise every title for being short.
struct Field {
    postings: Vec<HashMap<String, u32>>,
    lengths: Vec<u32>,
    doc_freq: HashMap<String, u32>,
    avg_len: f32,
    n: f32,
}

impl Field {
    fn build<'a>(units: impl Iterator<Item = &'a str>) -> Field {
        let mut postings = Vec::new();
        let mut lengths = Vec::new();
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        for text in units {
            let tokens = tokenize(text);
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in &tokens {
                *tf.entry(t.clone()).or_default() += 1;
            }
            for term in tf.keys() {
                *doc_freq.entry(term.clone()).or_default() += 1;
            }
            lengths.push(tokens.len() as u32);
            postings.push(tf);
        }
        let n = postings.len().max(1) as f32;
        let avg_len = lengths.iter().map(|l| *l as f32).sum::<f32>() / n;
        Field { postings, lengths, doc_freq, avg_len: avg_len.max(1.0), n }
    }

    fn bm25(&self, unit: usize, terms: &[String]) -> f32 {
        let tf = &self.postings[unit];
        let len = self.lengths[unit] as f32;
        let mut score = 0.0;
        for term in terms {
            let f = match tf.get(term) {
                Some(f) => *f as f32,
                None => continue,
            };
            let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
            let idf = (((self.n - df + 0.5) / (df + 0.5)) + 1.0).ln();
            let denom = f + BM25_K1 * (1.0 - BM25_B + BM25_B * len / self.avg_len);
            score += idf * (f * (BM25_K1 + 1.0)) / denom;
        }
        score
    }

    /// What fraction of the query's distinct terms appear in this unit, in [0,1].
    ///
    /// Deliberately NOT a BM25 score and NOT a rank. Coverage answers "is this title the
    /// thing being asked for?", which is the question a title should be judged on, and it
    /// is comparable across documents in a way neither of the others is. An exact word
    /// contributes twice (the word and its stem are both terms) while a merely-related
    /// word contributes once, so `danışman` scores double what `danışmanlığı` does — for
    /// free, out of the same arithmetic.
    fn coverage(&self, unit: usize, terms: &[String]) -> f32 {
        if terms.is_empty() {
            return 0.0;
        }
        let tf = &self.postings[unit];
        let mut seen: Vec<&str> = Vec::new();
        let mut hit = 0.0;
        let mut total = 0.0;
        for t in terms {
            if seen.contains(&t.as_str()) {
                continue;
            }
            seen.push(t.as_str());
            // Only terms SOME title contains count towards the denominator. A word like
            // `istiyorum` ("I want") appears in no form title, so no document can ever
            // cover it — leaving it in the denominator does not change the ordering, but
            // it deflates every score alike, and the title signal stops outweighing the
            // rank-fused ones. Measured against the achievable, `danışmanımı değiştirmek
            // istiyorum` covers "Danışman Değişikliği Formu" completely, which is the
            // honest reading of that query.
            if !self.doc_freq.contains_key(t) {
                continue;
            }
            total += 1.0;
            if tf.contains_key(t) {
                hit += 1.0;
            }
        }
        if total == 0.0 {
            0.0
        } else {
            hit / total
        }
    }

    /// Every unit with a non-zero score, best first, capped.
    fn rank(&self, terms: &[String], cap: usize) -> Vec<(usize, f32)> {
        if terms.is_empty() {
            return Vec::new();
        }
        let mut v: Vec<(usize, f32)> = (0..self.postings.len())
            .map(|u| (u, self.bm25(u, terms)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.truncate(cap);
        v
    }
}

/// The searchable form of a corpus: BM25 statistics, computed once at load.
pub struct Index {
    /// Passage bodies, one unit per chunk.
    body: Field,
    /// Form titles, one unit per document.
    title: Field,
}

impl Index {
    pub fn build(corpus: &Corpus) -> Index {
        // The code goes in with the title so `FR-0083` is findable as a WORD even when
        // `find_code` declines to treat the query as naming a form. That happens whenever a
        // bare number is only part of a longer question — `find_code` reads a lone number
        // as a code but refuses "0083 formu nerede", because "2 nüsha halinde" must not be
        // read as FR-0002. Without the code in this field that query reached the form by no
        // route at all; with it, the number is simply a term the title covers.
        let titles: Vec<String> = corpus
            .docs()
            .iter()
            .map(|d| match &d.code {
                Some(code) => format!("{code} {}", d.title),
                None => d.title.clone(),
            })
            .collect();
        Index {
            body: Field::build(corpus.chunks().iter().map(|c| c.text.as_str())),
            title: Field::build(titles.iter().map(String::as_str)),
        }
    }

    /// Rank the corpus for one query. `query_vec` is `None` when the embedder has not
    /// finished provisioning — search still works, lexically, rather than refusing to
    /// answer at all.
    pub fn search(
        &self,
        corpus: &Corpus,
        query: &str,
        query_vec: Option<&[f32]>,
        sort: Sort,
        limit: usize,
    ) -> Vec<Hit> {
        let terms = tokenize(query);
        let mut hits: Vec<Hit> = Vec::new();
        let mut placed: HashMap<usize, usize> = HashMap::new(); // doc → index into hits

        // 1. An exact code wins outright.
        let code = find_code(query);
        if let Some(code) = &code {
            for (i, d) in corpus.docs().iter().enumerate() {
                if d.code.as_deref() == Some(code.as_str()) {
                    placed.insert(i, hits.len());
                    hits.push(Hit {
                        doc: i,
                        score: f32::INFINITY,
                        why: Why::Code,
                        snippet: first_chunk(corpus, i),
                        // A code match is an identity, not a text match — there is no
                        // "where it matched" to point at.
                        passages: Vec::new(),
                    });
                }
            }
        }

        // Someone who typed nothing but a form number has asked ONE question and it has
        // been answered. Padding the reply with 24 unrelated forms — which is what the
        // retrievers produce for a query with no real words in it — reads as "here are
        // your results" and buries the actual answer in noise.
        let bare_code = !hits.is_empty()
            && code
                .as_ref()
                .is_some_and(|c| terms.iter().all(|t| is_part_of_code(t, c)));
        if bare_code {
            return hits;
        }

        if !terms.is_empty() || query_vec.is_some() {
            // doc → (fused score, every passage that contributed and by how much)
            let mut fused: HashMap<usize, (f32, Vec<(usize, f32)>)> = HashMap::new();

            // 2a. TITLES, scored by COVERAGE rather than by rank.
            //
            // A form's title is its identity; a word in its body is a mention. But feeding
            // the title through RRF like the others threw away the only thing that
            // distinguished the right answer: RRF reads ranks, and with K=60 rank 1 and
            // rank 5 differ by 8%. So a title matching ALL FOUR query terms scored barely
            // above one matching a single term, and `danışman değişikliği` returned
            // "Danışman Değişikliği Formu" fourth — behind a nutrition-counselling form.
            //
            // Coverage keeps the magnitude: full match 1.0, one-term-in-four 0.25. Squared,
            // so partial credit falls away fast, and scaled to dominate any realistic
            // accumulation of RRF contributions (two lists cap out around 0.033).
            for (doc, _) in self.title.rank(&terms, CANDIDATES) {
                let coverage = self.title.coverage(doc, &terms);
                let entry = fused
                    .entry(doc)
                    .or_insert_with(|| (0.0, vec![(first_chunk_of(corpus, doc), 0.0)]));
                entry.0 += TITLE_WEIGHT * coverage * coverage;
            }

            // 2b. Passage bodies.
            let body = self.body.rank(&terms, CANDIDATES);

            // 3. Semantic ranking of chunks.
            let dense: Vec<(usize, f32)> = match query_vec {
                Some(q) => {
                    let mut v: Vec<(usize, f32)> = (0..corpus.chunks().len())
                        .map(|c| (c, cosine(q, corpus.vector(c))))
                        .collect();
                    v.sort_by(|a, b| b.1.total_cmp(&a.1));
                    v.truncate(CANDIDATES);
                    v
                }
                None => Vec::new(),
            };

            // Collapse chunk-level lists to their documents, keeping each document's best
            // passage as the snippet.
            for list in [&body, &dense] {
                // Each list contributes a document's BEST chunk once, not every chunk it
                // has. Summing per chunk lets length buy rank: a long protocol with ten
                // passages in the top 200 collects ~0.15 while a short form whose title IS
                // the query collects 0.07, so `zorunlu staj başvurusu` ranked
                // `Zorunlu Staj Stajyeri Bilgi Formu` — the only title containing that
                // phrase — third. Retrieval must answer "is this form about it?", not
                // "how many pages does it have?".
                let mut best_rank: HashMap<usize, usize> = HashMap::new();
                for (rank, (chunk, _)) in list.iter().enumerate() {
                    let doc = corpus.chunks()[*chunk].doc as usize;
                    best_rank.entry(doc).or_insert(rank);

                    // Passage COLLECTION is separate and still per chunk: one that placed
                    // in both lists is the most worth showing a human, and this is only
                    // deciding what to display, never what to rank.
                    let contribution = 1.0 / (RRF_K + rank as f32 + 1.0);
                    let entry = fused.entry(doc).or_insert_with(|| (0.0, Vec::new()));
                    match entry.1.iter_mut().find(|(c, _)| c == chunk) {
                        Some(slot) => slot.1 += contribution,
                        None => entry.1.push((*chunk, contribution)),
                    }
                }
                for (doc, rank) in best_rank {
                    if let Some(entry) = fused.get_mut(&doc) {
                        entry.0 += 1.0 / (RRF_K + rank as f32 + 1.0);
                    }
                }
            }

            let mut ranked: Vec<(usize, f32, Vec<(usize, f32)>)> =
                fused.into_iter().map(|(d, (s, c))| (d, s, c)).collect();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

            for (doc, score, mut matched) in ranked {
                if placed.contains_key(&doc) {
                    continue; // already in, by code — nothing may outrank that
                }
                matched.sort_by(|a, b| b.1.total_cmp(&a.1));
                let passages: Vec<String> = matched
                    .iter()
                    .take(PASSAGES_SHOWN)
                    .map(|(c, _)| corpus.chunks()[*c].text.clone())
                    .collect();
                let snippet = passages
                    .first()
                    .cloned()
                    .unwrap_or_else(|| first_chunk(corpus, doc));
                placed.insert(doc, hits.len());
                hits.push(Hit { doc, score, why: Why::Match, snippet, passages });
            }
        }

        // The sort is applied to the whole result set, never to the page — but a code
        // match stays pinned, because it is an answer, not a ranking.
        match sort {
            Sort::Relevance => {}
            Sort::Code => hits.sort_by(|a, b| {
                (a.why != Why::Code)
                    .cmp(&(b.why != Why::Code))
                    .then_with(|| code_of(corpus, a.doc).cmp(&code_of(corpus, b.doc)))
            }),
            Sort::Title => hits.sort_by(|a, b| {
                (a.why != Why::Code)
                    .cmp(&(b.why != Why::Code))
                    .then_with(|| {
                        tokenize(&corpus.docs()[a.doc].title)
                            .cmp(&tokenize(&corpus.docs()[b.doc].title))
                    })
            }),
        }

        hits.truncate(limit);
        hits
    }
}

fn code_of(corpus: &Corpus, doc: usize) -> String {
    corpus.docs()[doc].code.clone().unwrap_or_else(|| "ZZ-9999".into())
}

/// Is this query token merely a piece of the form code we already matched? `FR-0083`
/// tokenises to `fr` and `0083`, and both are the code, not a topic.
fn is_part_of_code(token: &str, code: &str) -> bool {
    let (prefix, digits) = match code.split_once('-') {
        Some(p) => p,
        None => return false,
    };
    if token.eq_ignore_ascii_case(prefix) {
        return true;
    }
    match token.parse::<u32>() {
        Ok(n) => digits.parse::<u32>().map(|d| d == n).unwrap_or(false),
        Err(_) => false,
    }
}

/// The index of a document's first chunk — the snippet to show when a document was matched
/// on its title rather than on any particular passage.
fn first_chunk_of(corpus: &Corpus, doc: usize) -> usize {
    corpus
        .chunks()
        .iter()
        .position(|c| c.doc as usize == doc)
        .unwrap_or(0)
}

fn first_chunk(corpus: &Corpus, doc: usize) -> String {
    corpus
        .chunks()
        .iter()
        .find(|c| c.doc as usize == doc)
        .map(|c| c.text.clone())
        .unwrap_or_else(|| corpus.docs()[doc].title.clone())
}

/// Find a document by its id or its form code — what `open`, `get` and `save` all take.
/// Accepts `FR-0083`, `FR-0083.tr`, `fr83`, or the exact id.
pub fn resolve<'a>(corpus: &'a Corpus, needle: &str) -> Option<(usize, &'a Doc)> {
    let n = needle.trim();
    if let Some((i, d)) = corpus.docs().iter().enumerate().find(|(_, d)| d.id == n) {
        return Some((i, d));
    }
    let code = find_code(n)?;
    // Prefer Turkish when a code exists in both languages: it is the primary corpus.
    let mut best: Option<(usize, &Doc)> = None;
    for (i, d) in corpus.docs().iter().enumerate() {
        if d.code.as_deref() == Some(code.as_str()) {
            if d.lang == "tr" {
                return Some((i, d));
            }
            best.get_or_insert((i, d));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{Chunk, Header};

    fn doc(id: &str, code: Option<&str>, lang: &str, title: &str) -> crate::corpus::Doc {
        crate::corpus::Doc {
            id: id.into(),
            code: code.map(String::from),
            rev: 1,
            lang: lang.into(),
            title: title.into(),
            name: format!("{title}.docx"),
            ext: "docx".into(),
            url: "https://example.invalid".into(),
            chars: 10,
        }
    }

    /// A corpus with orthogonal unit vectors, so the dense half is exactly predictable.
    fn corpus() -> Corpus {
        let docs = vec![
            doc("FR-0083.tr", Some("FR-0083"), "tr", "Danışman Değişikliği Formu"),
            doc("FR-0336.tr", Some("FR-0336"), "tr", "Staj Belgesi"),
            doc("FR-0083.en", Some("FR-0083"), "en", "Advisor Change Form"),
        ];
        let chunks = vec![
            Chunk { doc: 0, ord: 0, text: "Danışman Değişikliği Formu tez danışmanı".into() },
            Chunk { doc: 1, ord: 0, text: "Staj Belgesi zorunlu staj işletme".into() },
            Chunk { doc: 2, ord: 0, text: "Advisor Change Form thesis".into() },
        ];
        Corpus {
            header: Header {
                version: 1,
                model: crate::corpus::MODEL_ID.into(),
                dim: 3,
                built: "2026-08-12".into(),
                source: "x".into(),
                docs,
                chunks,
            },
            vectors: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn turkish_dotted_i_folds_to_the_same_token_as_the_lowercase_form() {
        // The bug this exists to kill: `"İZİN".to_lowercase()` is "i̇zi̇n" in Rust, which
        // never equals the "izin" a user types.
        assert_eq!(tokenize("İZİN"), tokenize("izin"));
        assert_eq!(tokenize("IŞIK"), tokenize("ışık"));
        // `staj` is short enough to be its own stem; `belgesi` also contributes `belge`.
        assert_eq!(tokenize("Staj BELGESİ"), vec!["staj", "belgesi", "belge"]);
    }

    #[test]
    fn form_codes_are_recognised_however_they_are_typed() {
        for q in ["FR-0083", "fr 0083", "FR_83", "fr83", "form FR-0083 nerede"] {
            assert_eq!(find_code(q).as_deref(), Some("FR-0083"), "{q}");
        }
        assert_eq!(find_code("0083").as_deref(), Some("FR-0083"));
    }

    #[test]
    fn ordinary_words_are_not_mistaken_for_codes() {
        // The over-eager cases: a bare number inside a sentence, and a word that happens
        // to end in letters before digits.
        assert_eq!(find_code("2 nüsha halinde doldurulacak"), None);
        assert_eq!(find_code("staj belgesi"), None);
        assert_eq!(find_code("PROJEFR-1"), None);
    }

    #[test]
    fn an_exact_code_outranks_everything_including_a_better_text_match() {
        let c = corpus();
        let idx = Index::build(&c);
        // The query names FR-0083 but its WORDS are all about the staj form.
        let hits = idx.search(&c, "FR-0083 staj belgesi zorunlu işletme", None, Sort::Relevance, 5);
        assert_eq!(hits[0].why, Why::Code);
        assert!(
            c.docs()[hits[0].doc].code.as_deref() == Some("FR-0083"),
            "a named form must win outright, got {:?}",
            c.docs()[hits[0].doc].title
        );
    }

    #[test]
    fn lexical_search_works_with_no_embedder_yet() {
        // Provisioning may still be running; search must degrade, not refuse.
        let c = corpus();
        let idx = Index::build(&c);
        let hits = idx.search(&c, "staj", None, Sort::Relevance, 5);
        assert!(!hits.is_empty(), "a lexical hit must survive a missing model");
        assert_eq!(c.docs()[hits[0].doc].title, "Staj Belgesi");
    }

    #[test]
    fn the_dense_half_finds_a_form_that_shares_no_word_with_the_query() {
        let c = corpus();
        let idx = Index::build(&c);
        // Query vector points exactly at chunk 1 (the staj form) and the query text
        // shares no token with it, so only the dense half can produce this hit.
        let hits = idx.search(&c, "qqqq wwww", Some(&[0.0, 1.0, 0.0]), Sort::Relevance, 5);
        assert!(!hits.is_empty());
        assert_eq!(c.docs()[hits[0].doc].title, "Staj Belgesi");
    }

    #[test]
    fn sorting_is_over_the_whole_result_set_and_never_unseats_a_code_match() {
        let c = corpus();
        let idx = Index::build(&c);
        let hits = idx.search(&c, "FR-0336 danışman staj", None, Sort::Title, 5);
        assert_eq!(hits[0].why, Why::Code, "a code match stays pinned under any sort");
        let rest: Vec<&str> = hits[1..].iter().map(|h| c.docs()[h.doc].title.as_str()).collect();
        let mut sorted = rest.clone();
        sorted.sort_by_key(|t| tokenize(t));
        assert_eq!(rest, sorted, "the tail must be in title order");
    }

    #[test]
    fn resolve_prefers_turkish_when_a_code_exists_in_both_languages() {
        let c = corpus();
        assert_eq!(resolve(&c, "FR-0083").unwrap().1.lang, "tr");
        assert_eq!(resolve(&c, "FR-0083.en").unwrap().1.lang, "en", "an exact id still wins");
        assert!(resolve(&c, "FR-9999").is_none());
    }

    /// The regression that a real 791-form corpus found and every unit test had missed:
    /// querying a form's own title words ranked an unrelated form above it, because that
    /// form merely *mentioned* the word in its body and so appeared in both retrievers,
    /// while the right answer was #1 in only one. Plain RRF rewards weak-but-broad
    /// evidence over one strong signal; weighting the title field is the fix.
    #[test]
    fn a_title_match_beats_a_document_that_merely_mentions_the_words() {
        let mut c = corpus();
        // A form about something else entirely, whose body mentions the query's words.
        c.header.docs.push(doc("FR-0537.tr", Some("FR-0537"), "tr", "Öğrenci Topluluğu Tüzük Formu"));
        c.header.chunks.push(Chunk {
            doc: 3,
            ord: 0,
            // Mentioned twice, in a long body — exactly what fools an unweighted fusion.
            text: "Öğrenci Topluluğu Tüzük Formu topluluk danışman öğretim üyesi \
                   danışman değişikliği durumunda bu form doldurulur ve dekanlığa verilir"
                .into(),
        });
        c.vectors.extend([0.0, 0.0, 0.0]);

        // ...and the one that actually beat it on the real corpus: a form whose TITLE
        // shares a word, but only one. Rank-based fusion could not tell "matched every
        // term" from "matched a quarter of them"; coverage can.
        c.header.docs.push(doc("FR-0826.tr", Some("FR-0826"), "tr", "Beslenme Danışmanlığı ve Diyet Hizmetleri Onam Formu"));
        c.header.chunks.push(Chunk {
            doc: 4,
            ord: 0,
            text: "Beslenme Danışmanlığı ve Diyet Hizmetleri Onam Formu danışmanlık".into(),
        });
        c.vectors.extend([0.0, 0.0, 0.0]);

        let idx = Index::build(&c);
        let hits = idx.search(&c, "danışman değişikliği", None, Sort::Relevance, 5);
        assert!(!hits.is_empty());
        assert_eq!(
            c.docs()[hits[0].doc].title,
            "Danışman Değişikliği Formu",
            "the form whose TITLE is the query must win; got {:?}",
            hits.iter().map(|h| &c.docs()[h.doc].title).collect::<Vec<_>>()
        );
    }

    /// Length must not buy rank. A long document has many passages and therefore many
    /// chances to appear in a candidate list; if each one added score, the ranking would
    /// answer "how long is this form?" instead of "is it about the query?".
    #[test]
    fn a_long_document_does_not_outrank_a_short_one_by_sheer_bulk() {
        let mut c = corpus();
        c.header.docs.push(doc("FR-0744.tr", Some("FR-0744"), "tr", "Sektör Stajı Protokolü"));
        // Twelve passages, every one of them mentioning the query's words.
        for ord in 0..12u32 {
            c.header.chunks.push(Chunk {
                doc: 3,
                ord,
                text: "Sektör Stajı Protokolü zorunlu staj hükümleri madde".into(),
            });
            c.vectors.extend([0.0, 0.0, 0.0]);
        }
        // ...against one short form whose TITLE is what was asked for.
        c.header.docs.push(doc("FR-0751.tr", Some("FR-0751"), "tr", "Zorunlu Staj Stajyeri Bilgi Formu"));
        c.header.chunks.push(Chunk {
            doc: 4,
            ord: 0,
            text: "Zorunlu Staj Stajyeri Bilgi Formu".into(),
        });
        c.vectors.extend([0.0, 0.0, 0.0]);

        let idx = Index::build(&c);
        let hits = idx.search(&c, "zorunlu staj", None, Sort::Relevance, 5);
        assert_eq!(
            c.docs()[hits[0].doc].code.as_deref(),
            Some("FR-0751"),
            "the short exact-title form must win; got {:?}",
            hits.iter().map(|h| &c.docs()[h.doc].title).collect::<Vec<_>>()
        );
    }

    /// Turkish is agglutinative: a query never wears the suffix the title does.
    #[test]
    fn a_suffixed_turkish_word_finds_the_form_it_names() {
        let c = corpus();
        let idx = Index::build(&c);
        for query in ["danışmanımı", "danışmanı", "danışmanlık"] {
            let hits = idx.search(&c, query, None, Sort::Relevance, 5);
            assert!(
                hits.iter().any(|h| c.docs()[h.doc].code.as_deref() == Some("FR-0083")),
                "`{query}` must reach Danışman Değişikliği Formu"
            );
        }
    }

    #[test]
    fn stemming_collapses_suffixes_without_merging_unrelated_words() {
        assert_eq!(stem("danışmanımı"), stem("danışmanlık"));
        assert_eq!(stem("değişikliği"), stem("değiştirmek"));
        assert_eq!(stem("staj"), None, "a short word is already its own stem");
        assert_ne!(stem("öğrenci"), stem("danışman"));
    }

    /// A bare form number is one question with one answer. Padding it with the two hundred
    /// near-random chunks the retrievers produce for a query with no real words buries that
    /// answer in noise.
    #[test]
    fn a_bare_code_query_answers_with_the_form_and_nothing_else() {
        let c = corpus();
        let idx = Index::build(&c);
        for query in ["FR-0083", "fr 83", "0083"] {
            let hits = idx.search(&c, query, Some(&[0.0, 1.0, 0.0]), Sort::Relevance, 25);
            assert!(!hits.is_empty(), "{query}");
            assert!(
                hits.iter().all(|h| h.why == Why::Code),
                "`{query}` returned {} padded results",
                hits.iter().filter(|h| h.why != Why::Code).count()
            );
        }
        // ...but a code WITH a real question still searches for the rest.
        let hits = idx.search(&c, "FR-0083 staj belgesi", None, Sort::Relevance, 25);
        assert!(hits.len() > 1, "a code plus real words must still search");
    }

    /// A bare number inside a longer question is the one way of naming a form that
    /// `find_code` deliberately refuses to read as a code — it cannot, or "2 nüsha halinde"
    /// would resolve to FR-0002. The number must therefore reach the form as an ordinary
    /// TERM, which it only can if the code is part of the title field.
    ///
    /// On the real corpus this was the difference between FR-0083 ranking first for
    /// "0083 formu nerede" and not appearing in the results at all.
    #[test]
    fn a_bare_number_inside_a_question_still_reaches_its_form() {
        let c = corpus();
        let idx = Index::build(&c);
        // Not a code as far as `find_code` is concerned...
        assert_eq!(find_code("0083 formu nerede"), None);
        // ...so this hit can only have come from the title field carrying the code.
        let hits = idx.search(&c, "0083 formu nerede", None, Sort::Relevance, 5);
        assert_eq!(
            c.docs()[hits[0].doc].code.as_deref(),
            Some("FR-0083"),
            "got {:?}",
            hits.iter().map(|h| &c.docs()[h.doc].title).collect::<Vec<_>>()
        );
    }

    /// ...and the reason that is safe: a small number that is nobody's form code must not
    /// start matching forms merely because codes are now indexed.
    #[test]
    fn indexing_the_code_does_not_make_stray_numbers_match() {
        let c = corpus();
        let idx = Index::build(&c);
        let hits = idx.search(&c, "2 nüsha halinde doldurulacak", None, Sort::Relevance, 5);
        assert!(
            hits.iter().all(|h| c.docs()[h.doc].code.as_deref() != Some("FR-0002")),
            "a bare `2` must not resolve to a form code"
        );
    }

    #[test]
    fn every_hit_carries_a_snippet_to_show() {
        let c = corpus();
        let idx = Index::build(&c);
        for h in idx.search(&c, "FR-0083 staj", None, Sort::Relevance, 5) {
            assert!(!h.snippet.is_empty(), "doc {} has no snippet", h.doc);
        }
    }
}
