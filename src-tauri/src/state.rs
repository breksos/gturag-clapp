//! The app's state and rules — pure, and the only truth.
//!
//! No I/O, no networking, no platform code, no clock. Both surfaces call the same methods
//! here, so they cannot drift, and because there is nothing to mock the rules are testable
//! in a plain `cargo test` — which is where the rules actually live.
//!
//! Two conventions carry most of the design:
//!
//! * **Every mutating method returns the signals it wants sent.** The state stays
//!   side-effect-free by *returning* [`Emit`]s; the caller drains them into the live pipe.
//! * **Only a human's action signals.** Every entry point takes a [`By`], and an agent's
//!   own write is never announced back to it — that is the loop that makes an app talk to
//!   itself (PLAYBOOK, field notes).

use crate::corpus::Corpus;
use crate::index::{self, Hit, Index, Sort};
use clappkit::{AgentRow, Emit};
use serde_json::{json, Value};

/// How many results a page holds. This belongs to the SHARED state, not to the caller's
/// `-n`: the moment an agent asking for 1 result repaginates the human's window to one row,
/// the bug reads as "why does searching staj return one form?" (PLAYBOOK §11). `-n` limits
/// what a terminal prints; the page is fixed and both surfaces say "N of TOTAL" about it.
pub const PAGE: usize = 25;

/// Who is acting. The app can tell because Clatch injects `CLATCH_AGENT_ID` into the
/// calling agent's shell, and [`clappkit::app::spawn_ipc`] hands it to the handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum By {
    Human,
    Agent(String),
}

impl By {
    fn is_human(&self) -> bool {
        matches!(self, By::Human)
    }
}

/// How far along one provisioned artifact is. This is state the human WATCHES, so it is
/// modelled as a value with a reason attached, not as a bare bool — "not ready" and
/// "failed because the disk is full" are different sentences.
#[derive(Clone, Debug, PartialEq, Default, serde::Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Missing,
    Downloading {
        /// 0–100. Whole percent: this drives a progress bar, not a benchmark.
        percent: u8,
    },
    Ready,
    Failed {
        reason: String,
    },
}

impl Stage {
    pub fn is_ready(&self) -> bool {
        matches!(self, Stage::Ready)
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Provision {
    pub model: Stage,
    pub index: Stage,
}

impl Provision {
    /// Both artifacts present. Search is only fully itself when this is true — below it,
    /// the app still answers lexically rather than refusing.
    pub fn ready(&self) -> bool {
        self.model.is_ready() && self.index.is_ready()
    }

    /// One sentence for a status line, in the app's own voice.
    pub fn summary(&self) -> String {
        match (&self.model, &self.index) {
            (Stage::Ready, Stage::Ready) => "ready".into(),
            (_, Stage::Failed { reason }) | (Stage::Failed { reason }, _) => {
                format!("provisioning failed: {reason}")
            }
            (Stage::Downloading { percent }, _) => format!("downloading the model — {percent}%"),
            (_, Stage::Downloading { percent }) => format!("downloading the index — {percent}%"),
            _ => "not provisioned yet — run `gturag sync`".into(),
        }
    }
}

/// The whole of the app's state.
#[derive(Default)]
pub struct AppState {
    corpus: Option<Corpus>,
    index: Option<Index>,
    pub provision: Provision,

    /// What was last searched for, by either surface. Empty means nothing yet.
    query: String,
    results: Vec<Hit>,
    sort: Sort,
    /// Index into `corpus.docs` of the form on screen.
    open: Option<usize>,
    /// Document ids the human is collecting for the task at hand. Ids, not indexes:
    /// an index is meaningless the moment a new corpus is provisioned.
    saved: Vec<String>,

    /// The live roster, refreshed from the control pipe.
    pub agents: Vec<AgentRow>,
}

impl AppState {
    /// Attach a freshly loaded corpus and build its lexical index. Called by provisioning;
    /// the state itself never reads a file.
    pub fn attach(&mut self, corpus: Corpus) {
        self.index = Some(Index::build(&corpus));
        self.corpus = Some(corpus);
        // A previously open document was an index into the OLD corpus. Dropping it is the
        // honest move: keeping the number would silently point at a different form.
        self.open = None;
        self.results.clear();
    }

    pub fn corpus(&self) -> Option<&Corpus> {
        self.corpus.as_ref()
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn saved_ids(&self) -> &[String] {
        &self.saved
    }

    /// Run a search. `query_vec` is the embedded query, or `None` when the model is not
    /// provisioned yet — search degrades to lexical rather than refusing to answer.
    ///
    /// Searching is not itself news for an agent: it is how either surface looks around,
    /// and a signal per keystroke-equivalent would be noise. What signals is *opening*
    /// something, which is a decision.
    pub fn search(&mut self, query: &str, query_vec: Option<&[f32]>, _by: &By) -> Vec<Emit> {
        self.query = query.trim().to_string();
        self.results = match (&self.index, &self.corpus) {
            (Some(idx), Some(c)) if !self.query.is_empty() => {
                idx.search(c, &self.query, query_vec, self.sort, PAGE)
            }
            _ => Vec::new(),
        };
        // A search that lands on exactly one named form opens it: the user typed a name,
        // and making them click the single row they already identified is ceremony.
        if self.results.len() == 1 && self.results[0].why == index::Why::Code {
            self.open = Some(self.results[0].doc);
        }
        Vec::new()
    }

    /// Open one document by id or form code. Returns `Err` with a sentence the agent can
    /// act on, never a silent no-op.
    pub fn open(&mut self, needle: &str, by: &By) -> Result<Vec<Emit>, String> {
        let corpus = self.corpus.as_ref().ok_or_else(|| self.not_ready())?;
        let (i, doc) = index::resolve(corpus, needle)
            .ok_or_else(|| format!("no form matches `{needle}` — try `gturag search {needle}`"))?;
        let payload = json!({
            "id": doc.id, "code": doc.code, "title": doc.title,
            "lang": doc.lang, "url": doc.url,
        });
        self.open = Some(i);
        // Buffered: it rides the user's next prompt, so "how do I fill this in?" already
        // knows which form "this" is. The agent's own `open` is not news to the agent.
        Ok(if by.is_human() {
            vec![Emit { id: "doc.opened".into(), target: vec![], payload }]
        } else {
            Vec::new()
        })
    }

    pub fn open_doc(&self) -> Option<&crate::corpus::Doc> {
        let (c, i) = (self.corpus.as_ref()?, self.open?);
        c.docs().get(i)
    }

    /// Add a form to the shared saved list. Idempotent: saving twice is not an error, it
    /// is the same list, and an agent retrying must not double a row.
    pub fn save(&mut self, needle: &str, by: &By) -> Result<Vec<Emit>, String> {
        let corpus = self.corpus.as_ref().ok_or_else(|| self.not_ready())?;
        let (_, doc) = index::resolve(corpus, needle)
            .ok_or_else(|| format!("no form matches `{needle}`"))?;
        if self.saved.contains(&doc.id) {
            return Ok(Vec::new());
        }
        self.saved.push(doc.id.clone());
        Ok(self.saved_changed(by, "saved", &doc.id.clone()))
    }

    pub fn unsave(&mut self, needle: &str, by: &By) -> Result<Vec<Emit>, String> {
        let corpus = self.corpus.as_ref().ok_or_else(|| self.not_ready())?;
        let (_, doc) = index::resolve(corpus, needle)
            .ok_or_else(|| format!("no form matches `{needle}`"))?;
        let id = doc.id.clone();
        let before = self.saved.len();
        self.saved.retain(|s| *s != id);
        if self.saved.len() == before {
            return Err(format!("`{id}` is not in the saved list"));
        }
        Ok(self.saved_changed(by, "removed", &id))
    }

    fn saved_changed(&self, by: &By, what: &str, id: &str) -> Vec<Emit> {
        if !by.is_human() {
            return Vec::new();
        }
        // Context, not buffered: a saved list is built up over several actions, and each
        // one matters. `context` is queued in order and lossless; `buffered` keeps one.
        vec![Emit {
            id: "saved.changed".into(),
            target: vec![],
            payload: json!({ "action": what, "id": id, "saved": self.saved }),
        }]
    }

    /// Re-sort. State, so it re-pages: a control that only reorders the current page is a
    /// lie about the data underneath it (PLAYBOOK §11).
    pub fn set_sort(&mut self, sort: Sort, query_vec: Option<&[f32]>, by: &By) -> Vec<Emit> {
        self.sort = sort;
        let q = self.query.clone();
        self.search(&q, query_vec, by)
    }

    fn not_ready(&self) -> String {
        format!("the form index is not loaded — {}", self.provision.summary())
    }

    /// One document, as both surfaces render it.
    fn doc_json(&self, i: usize, hit: Option<&Hit>) -> Value {
        let d = &self.corpus.as_ref().unwrap().docs()[i];
        json!({
            "id": d.id,
            "code": d.code,
            "rev": d.rev,
            "lang": d.lang,
            "title": d.title,
            "name": d.name,
            "ext": d.ext,
            "url": d.url,
            // Honest about depth: a title-only document answers name queries and no
            // others, and the human deserves to know which kind they are looking at.
            "titleOnly": d.chars == 0,
            "saved": self.saved.contains(&d.id),
            "score": hit.map(|h| if h.score.is_finite() { h.score } else { 1.0 }),
            "why": hit.map(|h| h.why),
            "snippet": hit.map(|h| h.snippet.clone()),
            // Where in this form the query was found. The window shows these and marks the
            // matched words inside them.
            "passages": hit.map(|h| h.passages.clone()).unwrap_or_default(),
        })
    }

    /// The snapshot both surfaces see. Stamped with a `rev` by the caller
    /// ([`clappkit::snapshot::with_rev`]) in ONE place, so the response and the pushed
    /// event carry the same number when they describe the same moment.
    ///
    /// Nothing secret is in here by construction: this app holds no credential, and the
    /// snapshot is the one structure that goes everywhere.
    pub fn snapshot(&self) -> Value {
        let results: Vec<Value> = self
            .results
            .iter()
            .map(|h| self.doc_json(h.doc, Some(h)))
            .collect();
        let saved: Vec<Value> = match self.corpus.as_ref() {
            Some(c) => self
                .saved
                .iter()
                .filter_map(|id| c.docs().iter().position(|d| d.id == *id))
                .map(|i| self.doc_json(i, None))
                .collect(),
            None => Vec::new(),
        };

        json!({
            "ok": true,
            "query": self.query,
            // The terms the INDEX actually matched on, stems included — so the window
            // highlights what was really found rather than doing a naive substring search
            // that would miss `danışman` inside a query for `danışmanımı`.
            "terms": index::tokenize(&self.query),
            "sort": self.sort.as_str(),
            "results": results,
            "total": self.results.len(),
            "page": PAGE,
            "open": self.open.map(|i| self.doc_json(i, None)),
            "saved": saved,
            "provision": {
                "model": self.provision.model,
                "index": self.provision.index,
                "ready": self.provision.ready(),
                "summary": self.provision.summary(),
            },
            "corpus": self.corpus.as_ref().map(|c| json!({
                "documents": c.docs().len(),
                "chunks": c.chunks().len(),
                "built": c.header.built,
                "source": c.header.source,
            })),
            "agents": self.agents,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{Chunk, Doc, Header};

    fn doc(id: &str, code: &str, lang: &str, title: &str, chars: u64) -> Doc {
        Doc {
            id: id.into(),
            code: Some(code.into()),
            rev: 1,
            lang: lang.into(),
            title: title.into(),
            name: format!("{code} {title}.docx"),
            ext: "docx".into(),
            url: format!("https://example.invalid/{code}.docx"),
            chars,
        }
    }

    fn state() -> AppState {
        let docs = vec![
            doc("FR-0083.tr", "FR-0083", "tr", "Danışman Değişikliği Formu", 200),
            doc("FR-0336.tr", "FR-0336", "tr", "Staj Belgesi", 0),
        ];
        let chunks = vec![
            Chunk { doc: 0, ord: 0, text: "Danışman Değişikliği Formu tez".into() },
            Chunk { doc: 1, ord: 0, text: "Staj Belgesi zorunlu".into() },
        ];
        let corpus = Corpus {
            header: Header {
                version: 1,
                model: crate::corpus::MODEL_ID.into(),
                dim: 2,
                built: "2026-08-12".into(),
                source: "https://example.invalid".into(),
                docs,
                chunks,
            },
            vectors: vec![1.0, 0.0, 0.0, 1.0],
        };
        let mut s = AppState::default();
        s.attach(corpus);
        s.provision = Provision { model: Stage::Ready, index: Stage::Ready };
        s
    }

    #[test]
    fn a_humans_open_signals_and_an_agents_open_does_not() {
        let mut s = state();
        let emits = s.open("FR-0083", &By::Human).unwrap();
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].id, "doc.opened");
        assert!(emits[0].target.is_empty(), "an empty target broadcasts");

        let mut s = state();
        // The agent already knows about its own write — telling it is the loop that makes
        // an app talk to itself.
        assert!(s.open("FR-0083", &By::Agent("a1".into())).unwrap().is_empty());
    }

    #[test]
    fn saving_signals_as_context_and_is_idempotent() {
        let mut s = state();
        let first = s.save("FR-0083", &By::Human).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, "saved.changed");
        // A retry must not double the row, nor announce a change that did not happen.
        let again = s.save("FR-0083", &By::Human).unwrap();
        assert!(again.is_empty());
        assert_eq!(s.saved_ids().len(), 1);
    }

    #[test]
    fn unsaving_something_absent_is_an_error_the_agent_can_read() {
        let mut s = state();
        let err = s.unsave("FR-0336", &By::Human).unwrap_err();
        assert!(err.contains("not in the saved list"), "{err}");
    }

    #[test]
    fn an_unknown_form_is_refused_with_a_next_step() {
        let mut s = state();
        let err = s.open("FR-9999", &By::Human).unwrap_err();
        assert!(err.contains("no form matches"), "{err}");
        assert!(err.contains("search"), "the refusal must point somewhere: {err}");
    }

    #[test]
    fn before_provisioning_the_refusal_says_what_is_missing() {
        let mut s = AppState::default();
        let err = s.open("FR-0083", &By::Human).unwrap_err();
        assert!(err.contains("not provisioned"), "{err}");
    }

    #[test]
    fn searching_one_named_form_opens_it() {
        let mut s = state();
        s.search("FR-0083", None, &By::Human);
        assert_eq!(s.open_doc().map(|d| d.id.as_str()), Some("FR-0083.tr"));
    }

    #[test]
    fn a_new_corpus_drops_the_open_document_rather_than_repointing_it() {
        // An index is meaningless across a re-provision; keeping the number would show a
        // different form under the same heading.
        let mut s = state();
        s.open("FR-0083", &By::Human).unwrap();
        assert!(s.open_doc().is_some());
        let fresh = state();
        s.attach(fresh.corpus.unwrap());
        assert!(s.open_doc().is_none());
    }

    #[test]
    fn the_snapshot_carries_what_both_surfaces_need() {
        let mut s = state();
        s.save("FR-0083", &By::Human).unwrap();
        s.search("staj", None, &By::Human);
        let snap = s.snapshot();
        assert_eq!(snap["ok"], true);
        assert_eq!(snap["query"], "staj");
        assert_eq!(snap["sort"], "relevance");
        assert_eq!(snap["provision"]["ready"], true);
        assert_eq!(snap["corpus"]["documents"], 2);
        assert_eq!(snap["saved"][0]["id"], "FR-0083.tr");
        assert_eq!(snap["saved"][0]["saved"], true);
    }

    #[test]
    fn a_title_only_document_says_so_in_the_snapshot() {
        // 153 of the real corpus are legacy .doc/.xls. If LibreOffice was unavailable at
        // build time they carry no body text, and pretending otherwise would make their
        // empty results look like a search bug.
        let mut s = state();
        s.search("staj", None, &By::Human);
        let snap = s.snapshot();
        let staj = snap["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["code"] == "FR-0336")
            .expect("the staj form must be found");
        assert_eq!(staj["titleOnly"], true);
    }

    #[test]
    fn sorting_re_runs_the_search_rather_than_reordering_a_page() {
        let mut s = state();
        s.search("formu belgesi", None, &By::Human);
        let before = s.results.len();
        s.set_sort(Sort::Title, None, &By::Human);
        assert_eq!(s.sort(), Sort::Title);
        assert_eq!(s.results.len(), before, "the same result SET, re-ordered");
        assert_eq!(s.snapshot()["sort"], "title");
    }
}
