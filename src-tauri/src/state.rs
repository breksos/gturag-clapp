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
use std::collections::VecDeque;

/// How many results a page holds. This belongs to the SHARED state, not to the caller's
/// `-n`: the moment an agent asking for 1 result repaginates the human's window to one row,
/// the bug reads as "why does searching staj return one form?" (PLAYBOOK §11). `-n` limits
/// what a terminal prints; the page is fixed and both surfaces say "N of TOTAL" about it.
pub const PAGE: usize = 25;

/// How many actions the shared log remembers. Long enough that a human returning to the
/// window can see what an agent did while they were away; short enough that the snapshot —
/// which is sent on every single command — stays small.
pub const ACTIVITY_MAX: usize = 40;

/// One thing that happened, and who did it.
///
/// This is what makes the two surfaces one app rather than two programs sharing a file.
/// Both of them already act on the same state; without a record of WHO acted, the human
/// sees their search box change for no visible reason and the agent cannot tell what the
/// human has been doing. Attribution is the missing half of the loop.
///
/// `who` is an agent **id**, or `None` for the human. Ids, never names: a name is
/// re-pointable and the roster carries the current one, so the window resolves it at render
/// time and a rename relabels history instead of orphaning it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Activity {
    /// Monotonic, so the window can order and key these. Deliberately NOT a timestamp:
    /// this module has no clock, which is what keeps it pure and testable.
    pub seq: u64,
    pub who: Option<String>,
    /// The verb, matching the CLI's own vocabulary: `search`, `open`, `save`, `unsave`,
    /// `sort`, `sync`.
    pub action: String,
    /// What it was done to — the query, the form code.
    pub detail: String,
}

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

    /// The agent id to record against an action, or `None` for the human.
    fn actor(&self) -> Option<String> {
        match self {
            By::Human => None,
            By::Agent(id) => Some(id.clone()),
        }
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

    /// What both surfaces have been doing, newest last.
    activity: VecDeque<Activity>,
    seq: u64,

    /// Who ran the search currently on screen — an agent id, or `None` for the human.
    /// Attribution belongs on the VIEW, not only in a log: the log says what happened,
    /// this says whose the thing in front of you is. The point of a shared screen is
    /// knowing when it was not you.
    searched_by: Option<String>,
    /// A search is in flight. Both surfaces show it, so neither is left wondering whether
    /// anything is happening.
    searching: bool,
}

/// An agent id resolved against the roster. Done HERE rather than in the window so the CLI
/// prints names too — otherwise `gturag status` shows raw ids while the window shows names,
/// and they are describing the same event.
fn name_for(agents: &[AgentRow], id: &str) -> String {
    agents
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| id.to_string())
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

    /// Record who did what. Called by every mutating method, including the ones that emit
    /// no signal — a signal is a wake-up, this is the record, and the two answer different
    /// questions.
    fn note(&mut self, by: &By, action: &str, detail: impl Into<String>) {
        self.seq += 1;
        self.activity.push_back(Activity {
            seq: self.seq,
            who: by.actor(),
            action: action.to_string(),
            detail: detail.into(),
        });
        while self.activity.len() > ACTIVITY_MAX {
            self.activity.pop_front();
        }
    }

    /// The shared log, oldest first.
    pub fn activity(&self) -> &VecDeque<Activity> {
        &self.activity
    }

    /// Mark a search as started, before the slow part (embedding) runs. The window paints
    /// "running…" from this, so an agent's search is visible while it happens rather than
    /// only once it lands.
    pub fn begin_search(&mut self, query: &str, by: &By) {
        self.query = query.trim().to_string();
        self.searched_by = by.actor();
        self.searching = true;
    }

    /// Record an action the state itself does not perform — `sync` runs in the app layer,
    /// but a human watching the feed should still see who asked for it.
    pub fn note_action(&mut self, by: &By, action: &str, detail: &str) {
        self.note(by, action, detail);
    }

    /// Run a search. `query_vec` is the embedded query, or `None` when the model is not
    /// provisioned yet — search degrades to lexical rather than refusing to answer.
    ///
    /// Searching does not SIGNAL: a signal wakes an agent, and being woken for every
    /// keystroke-equivalent is noise. It is still RECORDED, which is a different thing —
    /// the agent reads the log when it next looks, and the human watches their window fill
    /// in under an agent's hand. That distinction is the whole point of the activity log.
    pub fn search(&mut self, query: &str, query_vec: Option<&[f32]>, by: &By) -> Vec<Emit> {
        self.query = query.trim().to_string();
        self.searched_by = by.actor();
        self.searching = false;
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
        if !self.query.is_empty() {
            self.note(by, "search", self.query.clone());
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
        let label = format!("{} {}", doc.code.clone().unwrap_or_default(), doc.title);
        self.note(by, "open", label.trim());
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
        let id = doc.id.clone();
        let label = format!("{} {}", doc.code.clone().unwrap_or_default(), doc.title);
        self.saved.push(id.clone());
        self.note(by, "save", label.trim());
        Ok(self.saved_changed(by, "saved", &id))
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
        self.note(by, "unsave", id.clone());
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
        let emits = self.search(&q, query_vec, by);
        self.note(by, "sort", sort.as_str());
        emits
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

        // One sentence describing what is on screen, built once so both surfaces say the
        // same thing about the same state.
        let title = if self.query.is_empty() {
            String::new()
        } else {
            format!("“{}” — {} sonuç", self.query, self.results.len())
        };

        json!({
            "ok": true,
            "query": self.query,
            "title": title,
            "searching": self.searching,
            "searchedBy": self.searched_by,
            "searchedByName": self.searched_by.as_ref().map(|id| name_for(&self.agents, id)),
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
            // Who did what, newest last. Each row carries the resolved name as well as the
            // id, so the window and the terminal label it identically.
            "activity": self.activity.iter().map(|a| json!({
                "seq": a.seq,
                "who": a.who,
                "whoName": a.who.as_ref().map(|id| name_for(&self.agents, id)),
                "action": a.action,
                "detail": a.detail,
            })).collect::<Vec<_>>(),
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

    /// The half of the loop that was missing: both surfaces act on one state, and now the
    /// state remembers WHO acted. Without this the human watches their search box change
    /// for no visible reason, and the agent cannot tell what the human has been doing.
    #[test]
    fn the_log_records_who_did_what_on_both_sides() {
        let mut s = state();
        s.search("staj", None, &By::Human);
        s.open("FR-0083", &By::Agent("a1".into())).unwrap();
        s.save("FR-0083", &By::Human).unwrap();

        let log: Vec<(Option<&str>, &str, &str)> = s
            .activity()
            .iter()
            .map(|a| (a.who.as_deref(), a.action.as_str(), a.detail.as_str()))
            .collect();
        assert_eq!(log[0].0, None, "the human is recorded as no agent id");
        assert_eq!(log[0].1, "search");
        assert_eq!(log[0].2, "staj");
        assert_eq!(log[1].0, Some("a1"), "an agent's action carries its id");
        assert_eq!(log[1].1, "open");
        assert_eq!(log[2].1, "save");
        // Monotonic, so the window can order and key them without a clock.
        let seqs: Vec<u64> = s.activity().iter().map(|a| a.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
    }

    /// A signal wakes an agent; the log is the record. Searching must do the second
    /// without the first, or every keystroke-equivalent becomes an interruption.
    #[test]
    fn a_search_is_recorded_but_never_signals() {
        let mut s = state();
        let emits = s.search("staj", None, &By::Human);
        assert!(emits.is_empty(), "a search must not wake an agent");
        assert_eq!(s.activity().len(), 1, "but it must still be visible");
    }

    #[test]
    fn the_log_is_bounded_so_the_snapshot_stays_small() {
        // The snapshot is sent on EVERY command; an unbounded log would grow it forever.
        let mut s = state();
        for i in 0..(ACTIVITY_MAX + 25) {
            s.search(&format!("q{i}"), None, &By::Human);
        }
        assert_eq!(s.activity().len(), ACTIVITY_MAX);
        assert_eq!(s.activity().back().unwrap().detail, format!("q{}", ACTIVITY_MAX + 24));
    }

    #[test]
    fn an_empty_search_is_not_worth_recording() {
        // Clearing the box is not an action anyone needs to see attributed.
        let mut s = state();
        s.search("", None, &By::Human);
        assert!(s.activity().is_empty());
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
