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
use crate::index::{self, Confidence, Filter, Hit, Index, Sort};
use crate::lexicon::{self, Scope};
use crate::live::Live;
use crate::meta::{self, Meta};
use crate::util::iso_to_dmy;
use clappkit::{AgentRow, Emit};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

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

/// Below both of these, the best evidence is too weak to call the results an answer: no title
/// carries a third of the question, and no passage is semantically close. Measured on the
/// real corpus with multilingual-e5-small, where unrelated passages still score ~0.8.
const WEAK_TITLE: f32 = 0.40;
const WEAK_DENSE: f32 = 0.88;

/// Something both surfaces must say ABOUT the results, before any of them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Notice {
    /// The question is about something this archive does not hold.
    Scope { scope: Scope },
    /// Nothing matched well: the results are the nearest documents, not an answer.
    Weak,
}

/// What the last `sync` found. Kept, and shown, because "ready" after a sync reads as "you
/// have the newest", which is a claim only a completed check can make.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncInfo {
    pub checked_at: String,
    pub outcome: SyncOutcome,
    /// The build of the index in use after the check.
    pub built: Option<String>,
    /// The build the update address offered, when it could be read.
    pub remote_built: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncOutcome {
    /// Nothing newer is published.
    Current,
    /// A newer index was installed.
    Updated,
    /// The check could not be completed.
    Failed,
}

/// What `save` did, so a surface can say it rather than print a count.
#[derive(Debug)]
pub struct Saved {
    pub emits: Vec<Emit>,
    pub id: String,
    pub label: String,
    /// It was already in the list; nothing changed.
    pub already: bool,
}

/// How far along one provisioned artifact is. This is state the human WATCHES, so it is
/// modelled as a value with a reason attached, not as a bare bool — "not ready" and
/// "failed because the disk is full" are different sentences.
#[derive(Clone, Debug, PartialEq, Default, serde::Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Missing,
    /// On disk and being loaded into memory — the first search after launch waits for this.
    Loading,
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
            (Stage::Loading, _) => "loading the search model — searches are lexical until it is ready".into(),
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
    /// What each document says about itself, in corpus order.
    meta: Vec<Meta>,
    pub provision: Provision,

    /// What was last searched for, by either surface. Empty means nothing yet.
    query: String,
    results: Vec<Hit>,
    sort: Sort,
    filter: Filter,
    /// What must be said about the results on screen, if anything.
    notice: Option<Notice>,
    /// The query on screen was read as English.
    english: bool,
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

    /// What the university's site said about a document, by id. Filled by the app layer,
    /// which is the only part that talks to the network.
    live: HashMap<String, Live>,
    /// The last `sync`.
    sync: Option<SyncInfo>,
    /// A `sync` is in flight.
    pub syncing: bool,
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
    ///
    /// The results are cleared, because they were positions in the old corpus; the caller
    /// re-runs the query, which needs a fresh query vector this module cannot make. The open
    /// form is re-found by its ID: a position is meaningless in a new corpus, but the same
    /// form is still the same form.
    pub fn attach(&mut self, corpus: Corpus) {
        let open_id = self.open_doc().map(|d| d.id.clone());
        self.meta = meta::derive(&corpus);
        self.index = Some(Index::build(&corpus, &self.meta));
        self.open = open_id.and_then(|id| corpus.docs().iter().position(|d| d.id == id));
        self.corpus = Some(corpus);
        self.results.clear();
        // A site check describes one revision of one document, and the corpus just changed.
        self.live.clear();
    }

    pub fn meta(&self, i: usize) -> Option<&Meta> {
        self.meta.get(i)
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn set_live(&mut self, id: &str, live: Live) {
        self.live.insert(id.to_string(), live);
    }

    pub fn live(&self, id: &str) -> Option<&Live> {
        self.live.get(id)
    }

    pub fn set_sync(&mut self, info: SyncInfo) {
        self.sync = Some(info);
    }

    /// The ids of the first `n` results, for the app to check against the site.
    pub fn top_ids(&self, n: usize) -> Vec<String> {
        let Some(c) = self.corpus.as_ref() else { return Vec::new() };
        self.results.iter().take(n).map(|h| c.docs()[h.doc].id.clone()).collect()
    }

    /// What `open`, `get`, `save` and `unsave` take: the row number of a result on screen,
    /// an id, or a code however it is typed. A number is a row only when that row exists,
    /// so `open 83` with 25 rows still means FR-0083.
    pub fn resolve(&self, needle: &str) -> Result<usize, String> {
        let corpus = self.corpus.as_ref().ok_or_else(|| self.not_ready())?;
        let n = needle.trim();
        if let Some(row) = row_number(n) {
            if let Some(hit) = self.results.get(row - 1) {
                return Ok(hit.doc);
            }
            if self.results.is_empty() && index::resolve(corpus, n).is_none() {
                return Err(format!("there are no results to take row {row} from — search first"));
            }
        }
        index::resolve(corpus, n).map(|(i, _)| i).ok_or_else(|| self.not_found(n))
    }

    fn not_found(&self, needle: &str) -> String {
        let (built, source) = self
            .corpus
            .as_ref()
            .map(|c| (iso_to_dmy(&c.header.built), c.header.source.clone()))
            .unwrap_or_default();
        format!(
            "no document matches `{needle}` in this archive (built {built}). A document newer than \
             that is not in it yet: `gturag sync` checks for a newer archive, and the official \
             lists are at {source}. To look by topic: `gturag search <what you want to do>`"
        )
    }

    pub fn corpus(&self) -> Option<&Corpus> {
        self.corpus.as_ref()
    }

    #[cfg(test)]
    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    #[cfg(test)]
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
    #[cfg(test)]
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
        self.run_query(query_vec);
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

    /// Run the query on screen again — after a new corpus, or once the search model has
    /// loaded — without recording it as anyone's action: nobody searched, the evidence
    /// changed.
    pub fn refresh(&mut self, query_vec: Option<&[f32]>) {
        if !self.query.is_empty() {
            self.run_query(query_vec);
        }
    }

    fn run_query(&mut self, query_vec: Option<&[f32]>) {
        let analysis = lexicon::analyze(&self.query);
        let (results, confidence) = match (&self.index, &self.corpus) {
            (Some(idx), Some(c)) if !self.query.is_empty() => {
                idx.search(c, &self.query, &analysis, query_vec, self.sort, &self.filter, PAGE)
            }
            _ => (Vec::new(), Confidence::default()),
        };
        if std::env::var_os("GTURAG_DEBUG_RANK").is_some() {
            eprintln!(
                "rank: {:?} lexical={:?} level={:?} title={:.3} strict={:.3} dense={:?} top={:?}",
                self.query,
                analysis.lexical,
                analysis.level,
                confidence.title,
                confidence.strict,
                confidence.dense,
                self.corpus.as_ref().and_then(|c| results.first().map(|h| c.docs()[h.doc].id.clone()))
            );
        }
        self.results = results;
        self.english = analysis.english;
        self.notice = if self.query.is_empty() {
            None
        } else if let Some(scope) = analysis.scope {
            Some(Notice::Scope { scope })
        } else if is_weak(&confidence, &self.results) {
            Some(Notice::Weak)
        } else {
            None
        };
    }

    /// Open one document by id or form code. Returns `Err` with a sentence the agent can
    /// act on, never a silent no-op.
    pub fn open(&mut self, needle: &str, by: &By) -> Result<Vec<Emit>, String> {
        let i = self.resolve(needle)?;
        let doc = &self.corpus.as_ref().expect("resolve succeeded").docs()[i];
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
    pub fn save(&mut self, needle: &str, by: &By) -> Result<Saved, String> {
        let i = self.resolve(needle)?;
        let doc = &self.corpus.as_ref().expect("resolve succeeded").docs()[i];
        let id = doc.id.clone();
        let label = format!("{} {}", doc.code.clone().unwrap_or_default(), doc.title).trim().to_string();
        if self.saved.contains(&id) {
            return Ok(Saved { emits: Vec::new(), id, label, already: true });
        }
        self.saved.push(id.clone());
        self.note(by, "save", label.clone());
        let emits = self.saved_changed(by, "saved", &id);
        Ok(Saved { emits, id, label, already: false })
    }

    pub fn unsave(&mut self, needle: &str, by: &By) -> Result<Saved, String> {
        let i = self.resolve(needle)?;
        let doc = &self.corpus.as_ref().expect("resolve succeeded").docs()[i];
        let id = doc.id.clone();
        let label = format!("{} {}", doc.code.clone().unwrap_or_default(), doc.title).trim().to_string();
        let before = self.saved.len();
        self.saved.retain(|s| *s != id);
        if self.saved.len() == before {
            return Err(format!("`{label}` is not in the saved list"));
        }
        self.note(by, "unsave", label.clone());
        let emits = self.saved_changed(by, "removed", &id);
        Ok(Saved { emits, id, label, already: false })
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

    /// Set the filter without searching: a search that carries its own filter applies it
    /// first and then runs once.
    pub fn put_filter(&mut self, filter: Filter, by: &By) {
        if self.filter != filter {
            self.filter = filter;
            let d = describe_filter(&self.filter);
            self.note(by, "filter", d);
        }
    }

    /// Narrow the results. State, like the sort, so it re-runs the search for both surfaces.
    pub fn set_filter(&mut self, filter: Filter, query_vec: Option<&[f32]>, by: &By) -> Vec<Emit> {
        self.filter = filter;
        let q = self.query.clone();
        let emits = self.search(&q, query_vec, by);
        self.note(by, "filter", describe_filter(&self.filter));
        emits
    }

    fn not_ready(&self) -> String {
        format!("the form index is not loaded — {}", self.provision.summary())
    }

    /// One document, as both surfaces render it.
    fn doc_json(&self, i: usize, hit: Option<&Hit>) -> Value {
        let d = &self.corpus.as_ref().unwrap().docs()[i];
        let m = self.meta.get(i).cloned().unwrap_or_default();
        json!({
            "id": d.id,
            "code": d.code,
            // The revision the document prints about itself; the filename's is kept beside it.
            "rev": m.rev,
            "revName": d.rev,
            "revDate": m.rev_date,
            "pubDate": m.pub_date,
            "unit": m.unit,
            "collection": m.collection,
            "level": m.level,
            "live": self.live.get(&d.id),
            "liveText": self.live.get(&d.id).map(Live::sentence),
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

        let mut collections: Vec<&str> = self.meta.iter().filter_map(|m| m.collection.as_deref()).collect();
        collections.sort_unstable();
        collections.dedup();

        // One sentence describing what is on screen, built once so both surfaces say the
        // same thing about the same state.
        let title = if self.query.is_empty() {
            String::new()
        } else {
            format!("“{}” — {} sonuç", self.query, self.results.len())
        };

        json!({
            "ok": true,
            // The app's own name, from the manifest via build.rs — so the window's wordmark
            // and the CLI's banner are the same string a fork changes in one place.
            "app": { "name": crate::APP_NAME },
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
            "filter": self.filter,
            "notice": self.notice,
            "language": if self.english { "en" } else { "tr" },
            "results": results,
            "total": self.results.len(),
            "page": PAGE,
            // The open form carries its matched passages when it is one of the results.
            "open": self.open.map(|i| self.doc_json(i, self.results.iter().find(|h| h.doc == i))),
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
                "updateUrl": c.header.update_url,
                // What the window's type filter offers: every collection the corpus has.
                "collections": collections,
            })),
            "sync": self.sync,
            "syncing": self.syncing,
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

/// A result row number as a person types it: 1–99, never with a leading zero (`0083` is a
/// form number).
fn row_number(s: &str) -> Option<usize> {
    let plain = !s.is_empty() && s.len() <= 2 && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit());
    plain.then(|| s.parse().ok()).flatten()
}

fn is_weak(c: &Confidence, results: &[Hit]) -> bool {
    !results.is_empty()
        && results.iter().all(|h| h.why != index::Why::Code)
        && c.strict < WEAK_TITLE
        && c.dense.map_or(true, |d| d < WEAK_DENSE)
}

pub fn describe_filter(f: &Filter) -> String {
    let mut parts = Vec::new();
    if let Some(t) = &f.collection {
        parts.push(format!("type={t}"));
    }
    if let Some(l) = f.level {
        parts.push(format!("level={}", l.label()));
    }
    if let Some(l) = &f.lang {
        parts.push(format!("lang={l}"));
    }
    if parts.is_empty() { "none".into() } else { parts.join(" ") }
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
            hash: None,
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
                update_url: None,
                text_base: None,
                default_family: None,
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
        assert_eq!(first.emits.len(), 1);
        assert_eq!(first.emits[0].id, "saved.changed");
        assert!(!first.already);
        // A retry must not double the row, nor announce a change that did not happen — and
        // it says so, rather than repeating the count as if something had been saved.
        let again = s.save("FR-0083", &By::Human).unwrap();
        assert!(again.emits.is_empty());
        assert!(again.already);
        assert_eq!(again.id, "FR-0083.tr");
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
        assert!(err.contains("no document matches `FR-9999`"), "{err}");
        assert!(err.contains("search"), "the refusal must point somewhere: {err}");
        assert!(err.contains("sync") && err.contains("https://example.invalid"), "and say where else to look: {err}");
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
    fn a_new_corpus_keeps_the_open_form_by_id_not_by_position() {
        // A position is meaningless across a re-provision: keeping the number would show a
        // different form under the same heading. The id is what survives.
        let mut s = state();
        s.open("FR-0336", &By::Human).unwrap();
        let mut fresh = state().corpus.unwrap();
        fresh.header.docs.reverse();
        for c in fresh.header.chunks.iter_mut() {
            c.doc = 1 - c.doc;
        }
        s.attach(fresh);
        assert_eq!(s.open_doc().map(|d| d.id.as_str()), Some("FR-0336.tr"), "same form, new position");

        let mut without = state().corpus.unwrap();
        without.header.docs.truncate(1);
        without.header.chunks.truncate(1);
        s.attach(without);
        assert!(s.open_doc().is_none(), "a form the new corpus lacks is closed, not repointed");
    }

    #[test]
    fn a_row_number_picks_from_the_results_on_screen() {
        let mut s = state();
        let err = s.open("1", &By::Human).unwrap_err();
        assert!(err.contains("search first"), "{err}");
        s.search("staj", None, &By::Human);
        let first = s.results[0].doc;
        s.open("1", &By::Human).unwrap();
        assert_eq!(s.open, Some(first));
        assert!(s.save("1", &By::Human).is_ok());
        // Past the rows on screen, a number is a form number again.
        assert_eq!(s.resolve("83").ok(), s.resolve("FR-0083").ok());
    }

    #[test]
    fn an_out_of_scope_question_is_flagged_before_its_results() {
        let mut s = state();
        s.search("2026 güz akademik takvim ders kayıt tarihleri", None, &By::Human);
        assert_eq!(s.snapshot()["notice"]["kind"], "scope");
        assert_eq!(s.snapshot()["notice"]["scope"], "calendar");
        s.search("danışman değişikliği", None, &By::Human);
        assert!(s.snapshot()["notice"].is_null(), "a good match needs no warning");
    }

    #[test]
    fn a_filter_is_shared_state_and_re_runs_the_search() {
        let mut s = state();
        s.search("formu belgesi", None, &By::Human);
        let f = Filter { level: Some(meta::Level::Lisansustu), ..Filter::default() };
        s.set_filter(f.clone(), None, &By::Human);
        assert_eq!(s.filter(), &f);
        assert_eq!(s.snapshot()["filter"]["level"], "lisansustu");
        assert_eq!(s.activity().back().unwrap().action, "filter");
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
