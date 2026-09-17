//! The GUI role: Tauri wiring, the control pipe, and provisioning.
//!
//! Glue only. Every decision worth testing is in [`crate::state`], and the network lives in
//! [`crate::provision`] and [`crate::live`]; this file decides WHEN they run, and runs them
//! outside the state lock, because a window that freezes for a file transfer reads as a
//! crash.

use crate::corpus::{Corpus, Doc};
use crate::index::Filter;
use crate::live::{self, Live};
use crate::meta::Level;
use crate::provision::{self, Synced};
use crate::state::{AppState, By, Stage, SyncInfo, SyncOutcome};
use crate::{util, APP_ID, CLI};
use clappkit::app::{self as kit, Reply};
use clappkit::{Control, WindowPolicy};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

/// The app's own mark. Bytes stay per-app because they ARE the app's identity.
const ICON: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/icon.png"));

/// How long a site check stays true. Long enough that paging through results does not ask
/// the university about every row again; short enough that a window left open over a day
/// notices a revision published in the meantime.
const LIVE_TTL_SECS: u64 = 6 * 3600;

/// How many of a search's top results are checked against the site, in the background.
const LIVE_TOP: usize = 5;

/// How long a search must stand before its rows are checked. The window searches as the
/// human types; asking the university about every intermediate result list would be a
/// burst of requests for pages nobody looked at.
const LIVE_SETTLE_MS: u64 = 1500;

/// Bumped by every search; a background check that finds it moved on gives up.
static SEARCH_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The window handle and the core, reachable from a command handler.
///
/// `sync` restarts background work and arrives over the IPC channel, where neither is a
/// parameter. Set once, during `setup`, before anything can be served.
static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
static CORE: std::sync::OnceLock<Arc<Core>> = std::sync::OnceLock::new();

fn self_arc() -> Arc<Core> {
    CORE.get().expect("the core is set before the IPC listener binds").clone()
}

/// Everything a command handler needs. The embedder is separate from the state because it
/// is not state — it is a loaded model, it never changes, and putting it behind the same
/// lock would make a 50 ms encode block the window's next repaint.
pub struct Core {
    pub state: Mutex<AppState>,
    pub embedder: Mutex<Option<crate::embed::Embedder>>,
    pub control: Control,
}

/// What a command still has to do once its response is decided.
enum Then {
    Nothing,
    /// The CLI's `open` waits for the site check, so it can print the answer.
    CheckNow(String),
    /// The window's `open` is checked while the human reads it.
    CheckLater(Vec<String>),
    /// A search's top rows are checked once the search has stood for a moment.
    CheckRows(Vec<String>),
}

impl Core {
    /// Embed a query, or `None` when the model is not loaded — search then degrades to
    /// lexical rather than refusing, which is the difference between a slow first run and
    /// a broken one.
    async fn embed(&self, query: &str) -> Option<Vec<f32>> {
        let guard = self.embedder.lock().await;
        let e = guard.as_ref()?;
        match e.query(query) {
            Ok(v) => Some(v),
            Err(err) => {
                eprintln!("{CLI}: embedding failed, falling back to lexical search: {err}");
                None
            }
        }
    }

    /// Apply one command envelope and answer with the response plus a fresh snapshot.
    pub async fn apply(&self, req: Value, caller: Option<String>) -> Reply {
        let by = match caller {
            Some(id) => By::Agent(id),
            None => By::Human,
        };
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let arg = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_string();

        // The verbs whose whole job is a network round trip answer on their own.
        let networked = match cmd.as_str() {
            "get" => Some(self.get(&arg("id")).await),
            "sync" => Some(self.sync(&by).await),
            _ => None,
        };

        // Announce the search BEFORE embedding it: embedding is the slow step, and an
        // agent's search should be visible while it is the thing actually happening.
        if cmd == "search" {
            {
                let mut s = self.state.lock().await;
                s.begin_search(&arg("query"), &by);
            }
            self.push().await;
        }

        // Embedding happens OUTSIDE the state lock — it is the one slow local step.
        let qvec = match cmd.as_str() {
            "search" => self.embed(&arg("query")).await,
            "sort" | "filter" => {
                let q = self.state.lock().await.query().to_string();
                if q.is_empty() { None } else { self.embed(&q).await }
            }
            _ => None,
        };

        let mut then = Then::Nothing;
        let resp = {
            let mut state = self.state.lock().await;
            match cmd.as_str() {
                "state" | "status" => json!({ "ok": true }),
                "search" => {
                    let applied = match req.get("filter") {
                        Some(f) => parse_filter(f, state.filter()).map(|flt| state.put_filter(flt, &by)),
                        None => Ok(()),
                    };
                    match applied {
                        Err(e) => json!({ "ok": false, "error": e }),
                        Ok(()) => {
                            let emits = state.search(&arg("query"), qvec.as_deref(), &by);
                            self.control.emit_all(emits);
                            then = Then::CheckRows(state.top_ids(LIVE_TOP));
                            json!({ "ok": true })
                        }
                    }
                }
                "filter" => match parse_filter(&req["filter"], state.filter()) {
                    Ok(f) => {
                        let emits = state.set_filter(f, qvec.as_deref(), &by);
                        self.control.emit_all(emits);
                        json!({ "ok": true })
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                "open" => match state.open(&arg("id"), &by) {
                    Ok(emits) => {
                        self.control.emit_all(emits);
                        if let Some(id) = state.open_doc().map(|d| d.id.clone()) {
                            then = if req["wait"] == true { Then::CheckNow(id) } else { Then::CheckLater(vec![id]) };
                        }
                        json!({ "ok": true })
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                "save" | "unsave" => {
                    let done = if cmd == "save" { state.save(&arg("id"), &by) } else { state.unsave(&arg("id"), &by) };
                    match done {
                        Ok(saved) => {
                            self.control.emit_all(saved.emits);
                            json!({ "ok": true, "id": saved.id, "label": saved.label, "already": saved.already })
                        }
                        Err(e) => json!({ "ok": false, "error": e }),
                    }
                }
                "sort" => match crate::index::Sort::parse(&arg("by")) {
                    Some(s) => {
                        let emits = state.set_sort(s, qvec.as_deref(), &by);
                        self.control.emit_all(emits);
                        json!({ "ok": true })
                    }
                    None => json!({ "ok": false, "error": "sort by relevance, code or title" }),
                },
                "get" | "sync" => networked.clone().expect("answered above for this verb"),
                other => json!({ "ok": false, "error": format!("unknown command `{other}`") }),
            }
        };

        match then {
            Then::Nothing => {}
            Then::CheckNow(id) => {
                self.check_live(&id).await;
            }
            Then::CheckLater(ids) => spawn_live_checks(ids, None),
            Then::CheckRows(ids) => {
                let generation = SEARCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                spawn_live_checks(ids, Some(generation));
            }
        }

        let snapshot = self.snapshot().await;
        let mut resp = resp;
        // The caller gets the snapshot too, so a CLI can print state without a second trip.
        if let (Some(o), Some(s)) = (resp.as_object_mut(), snapshot.as_object()) {
            for (k, v) in s {
                o.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        Reply::new(resp, snapshot)
    }

    /// The snapshot both surfaces see, with the roster read fresh: a rename arrives as a new
    /// roster and must relabel history in place.
    async fn snapshot(&self) -> Value {
        let mut s = self.state.lock().await;
        s.agents = self.control.roster();
        clappkit::snapshot::with_rev(s.snapshot())
    }

    async fn push(&self) {
        if let Some(h) = APP.get() {
            let snap = self.snapshot().await;
            kit::push_state(h, snap);
        }
    }

    /// `get`: a document's full text as Markdown, headed by what it is and by what the
    /// university's site says about it. Resolved under the lock; fetched and checked
    /// without it, concurrently, because the agent is waiting on both.
    async fn get(&self, needle: &str) -> Value {
        let target = {
            let s = self.state.lock().await;
            s.resolve(needle).map(|i| {
                let c = s.corpus().expect("resolve succeeded, so a corpus is loaded");
                (c.docs()[i].clone(), s.meta(i).cloned().unwrap_or_default(), provision::text_base(Some(c)))
            })
        };
        let (doc, meta, base) = match target {
            Ok(t) => t,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        let id = doc.id.clone();
        let fetch = tokio::task::spawn_blocking({
            let id = id.clone();
            move || provision::fetch_form_text(CLI, &base, &id)
        });
        let (text, live) = tokio::join!(fetch, self.check_live(&id));
        match text {
            Ok(Ok(body)) => json!({
                "ok": true,
                "id": id,
                "url": doc.url,
                "text": crate::text::markdown(&doc, &meta, live.as_ref(), &body),
                "liveText": live.as_ref().map(Live::sentence),
            }),
            Ok(Err(e)) => json!({
                "ok": false,
                "error": format!("could not fetch the full text of {}: {e:#}. Read it at the source: {}", label(&doc), doc.url),
                "url": doc.url,
            }),
            Err(e) => json!({ "ok": false, "error": format!("the text fetch stopped unexpectedly: {e}") }),
        }
    }

    /// What the university's site says about one document — from the cache when that is
    /// fresh, otherwise asked now. `None` only when the document is not in the corpus.
    async fn check_live(&self, id: &str) -> Option<Live> {
        let (doc, built): (Doc, String) = {
            let s = self.state.lock().await;
            if let Some(l) = s.live(id).filter(|l| is_fresh(l)) {
                return Some(l.clone());
            }
            let c = s.corpus()?;
            (c.docs().iter().find(|d| d.id == id)?.clone(), c.header.built.clone())
        };
        let status = tokio::task::spawn_blocking(move || live::check(&doc, &built, &live::head))
            .await
            .unwrap_or_else(|e| live::Status::Unknown { reason: e.to_string() });
        let l = Live { status, checked_at: util::now_iso() };
        self.state.lock().await.set_live(id, l.clone());
        Some(l)
    }

    /// `sync`: a definitive answer to "is a newer archive published?", and the retry for a
    /// search model that failed to load.
    async fn sync(&self, by: &By) -> Value {
        let (url, have) = {
            let mut s = self.state.lock().await;
            s.note_action(by, "sync", "");
            s.syncing = true;
            (provision::update_url(s.corpus()), s.corpus().map(|c| c.header.built.clone()))
        };
        self.push().await;

        // Progress crosses from the blocking download to the window on a channel.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let forward = tauri::async_runtime::spawn(async move {
            let core = self_arc();
            while let Some(p) = rx.recv().await {
                core.state.lock().await.provision.index = Stage::Downloading { percent: p };
                core.push().await;
            }
        });
        let result = tokio::task::spawn_blocking({
            let (url, have) = (url.clone(), have.clone());
            move || provision::sync_index(CLI, &url, have.as_deref(), move |p| {
                let _ = tx.send(p);
            })
        })
        .await;
        let _ = forward.await;

        let checked_at = util::now_iso();
        let info = match result {
            Ok(Ok(Synced::Current { remote })) => SyncInfo {
                checked_at,
                outcome: SyncOutcome::Current,
                built: have.clone(),
                remote_built: Some(remote),
                error: None,
            },
            Ok(Ok(Synced::Updated { remote, corpus })) => {
                let built = corpus.header.built.clone();
                self.install(*corpus).await;
                SyncInfo { checked_at, outcome: SyncOutcome::Updated, built: Some(built), remote_built: Some(remote), error: None }
            }
            Ok(Err(e)) => SyncInfo {
                checked_at,
                outcome: SyncOutcome::Failed,
                built: have.clone(),
                remote_built: None,
                error: Some(format!("{e:#}")),
            },
            Err(e) => SyncInfo {
                checked_at,
                outcome: SyncOutcome::Failed,
                built: have.clone(),
                remote_built: None,
                error: Some(e.to_string()),
            },
        };
        let record = info.clone();
        let _ = tokio::task::spawn_blocking(move || provision::save_sync(CLI, &record)).await;

        let retry_model = {
            let mut s = self.state.lock().await;
            s.set_sync(info.clone());
            s.syncing = false;
            s.provision.index = match (&info.outcome, s.corpus().is_some()) {
                (_, true) => Stage::Ready,
                (SyncOutcome::Failed, false) => Stage::Failed { reason: info.error.clone().unwrap_or_default() },
                _ => Stage::Missing,
            };
            matches!(s.provision.model, Stage::Failed { .. })
        };
        // `sync` is the window's one "try again": a model that failed to load is retried.
        if retry_model {
            spawn_model();
        }

        match info.outcome {
            SyncOutcome::Failed => json!({
                "ok": false,
                "error": format!("could not check for a newer archive: {}", info.error.clone().unwrap_or_default()),
                "sync": info,
            }),
            _ => json!({ "ok": true, "sync": info }),
        }
    }

    /// Put a corpus in place and bring the screen along: the open document is kept by id,
    /// and the query on screen is searched again against the new documents.
    async fn install(&self, corpus: Corpus) {
        let query = {
            let mut s = self.state.lock().await;
            s.attach(corpus);
            s.provision.index = Stage::Ready;
            s.query().to_string()
        };
        if !query.is_empty() {
            let qvec = self.embed(&query).await;
            self.state.lock().await.refresh(qvec.as_deref());
        }
    }
}

fn label(d: &Doc) -> String {
    format!("{} {}", d.code.clone().unwrap_or_default(), d.title).trim().to_string()
}

/// A site check is reused while it is recent. "Could not ask" is never reused: the next
/// look should ask again.
fn is_fresh(l: &Live) -> bool {
    if matches!(l.status, live::Status::Unknown { .. }) {
        return false;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    l.checked_at >= util::iso_from_unix(now.saturating_sub(LIVE_TTL_SECS))
}

/// A filter as a surface sends it: `"clear"`, or an object whose fields set — and whose
/// empty strings unset — one dimension each. A field not mentioned keeps its value.
fn parse_filter(v: &Value, current: &Filter) -> Result<Filter, String> {
    if v.as_str() == Some("clear") {
        return Ok(Filter::default());
    }
    let obj = v.as_object().ok_or("a filter is an object, or \"clear\"")?;
    let mut f = current.clone();
    if let Some(t) = obj.get("type").and_then(Value::as_str) {
        let t = t.trim();
        f.collection = (!t.is_empty()).then(|| t.to_string());
    }
    if let Some(l) = obj.get("level").and_then(Value::as_str) {
        f.level = match l.trim() {
            "" => None,
            other => Some(Level::parse(other).ok_or_else(|| format!("level is lisans or lisansustu, not `{other}`"))?),
        };
    }
    if let Some(l) = obj.get("lang").and_then(Value::as_str) {
        f.lang = match l.trim().to_lowercase().as_str() {
            "" => None,
            code @ ("tr" | "en") => Some(code.to_string()),
            other => return Err(format!("lang is tr or en, not `{other}`")),
        };
    }
    Ok(f)
}

/// Ask the site about these documents in the background, publishing each answer as it lands.
/// With a search generation, wait for the search to stand first, and drop the work if a
/// newer search has replaced it.
fn spawn_live_checks(ids: Vec<String>, generation: Option<u64>) {
    if ids.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Some(g) = generation {
            tokio::time::sleep(std::time::Duration::from_millis(LIVE_SETTLE_MS)).await;
            if SEARCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != g {
                return;
            }
        }
        let mut set = tokio::task::JoinSet::new();
        for id in ids {
            set.spawn(async move {
                let core = self_arc();
                core.check_live(&id).await
            });
        }
        while set.join_next().await.is_some() {
            self_arc().push().await;
        }
    });
}

#[tauri::command]
async fn run_cmd(core: tauri::State<'_, Arc<Core>>, app: tauri::AppHandle, req: Value) -> Result<Value, String> {
    // The window's own calls are the human's, so no `agent` key is passed.
    let reply = core.apply(req, None).await;
    kit::push_state(&app, reply.snapshot);
    Ok(reply.resp)
}

/// An agent's avatar for the window, as a data URI. Confined to the paths the roster itself
/// names (clappkit K1): without that, `asset` was a way for the webview to read any file the
/// user can.
#[tauri::command]
fn asset(core: tauri::State<'_, Arc<Core>>, path: String) -> Option<String> {
    kit::avatar_uri(&path, &core.control)
}

/// Open a document on the university's own site, in the user's real browser.
///
/// A plain `<a target="_blank">` does nothing in a Tauri window: WebView2 raises
/// `NewWindowRequested` and, with no handler, the click is swallowed. So the link travels
/// through the core, with two constraints, because this hands a string to the OS:
///
/// * **`https` only.** `file:`, `javascript:` and the shell schemes are exactly what a URL
///   opener is abused for, and every URL this app opens is an `https://` link.
/// * **No shell.** `cmd /C start` would parse `&` and `|` out of the URL; `rundll32
///   url.dll,FileProtocolHandler` takes it as one argument and no shell sees it.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("refusing to open a non-https URL: {url}"));
    }
    // Almost every URL here carries spaces and Turkish letters, and a URL handler that
    // receives a raw space treats what follows as another argument.
    let encoded = util::encode_url(&url);
    let spawned = if cfg!(target_os = "windows") {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &encoded])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&encoded).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&encoded).spawn()
    };
    spawned.map(|_| ()).map_err(|e| format!("cannot open the browser: {e}"))
}

/// What the provisioning worker reports back as it goes.
enum Prov {
    IndexStage(Stage),
    IndexReady(Box<Corpus>),
    ModelStage(Stage),
    ModelReady(Box<crate::embed::Embedder>),
}

/// Apply one provisioning message, then show it.
async fn handle(core: &Core, msg: Prov) {
    match msg {
        Prov::IndexStage(st) => core.state.lock().await.provision.index = st,
        Prov::IndexReady(c) => core.install(*c).await,
        Prov::ModelStage(st) => core.state.lock().await.provision.model = st,
        Prov::ModelReady(e) => {
            *core.embedder.lock().await = Some(*e);
            let query = {
                let mut s = core.state.lock().await;
                s.provision.model = Stage::Ready;
                s.query().to_string()
            };
            // A search typed while the model loaded was answered lexically. Now that meaning
            // is available, the same query is answered again with it.
            if !query.is_empty() {
                let qvec = core.embed(&query).await;
                core.state.lock().await.refresh(qvec.as_deref());
            }
        }
    }
    core.push().await;
}

/// Load the search model, downloading it first when no copy exists, and warm it.
///
/// The first encode pays for faulting half a gigabyte of weights in, which is what made
/// the first search of a session take twenty seconds. It is paid here instead, while the
/// window says "loading".
fn provision_model(tx: &UnboundedSender<Prov>) {
    let fetched = if provision::model_present(CLI) {
        Ok(())
    } else {
        provision::fetch_model(CLI, |st| {
            if matches!(st, Stage::Downloading { .. }) {
                let _ = tx.send(Prov::ModelStage(st));
            }
        })
    };
    let _ = tx.send(Prov::ModelStage(Stage::Loading));
    let loaded = fetched
        .and_then(|_| crate::embed::Embedder::load(&provision::model_dir(CLI)))
        .and_then(|e| e.query("hazırlık").map(|_| e));
    let _ = tx.send(match loaded {
        Ok(e) => Prov::ModelReady(Box::new(e)),
        Err(e) => Prov::ModelStage(Stage::Failed { reason: format!("{e:#}") }),
    });
}

/// Run a provisioning job on a blocking thread and apply what it reports.
///
/// A plain `std::thread` has no Tokio context, and the first version of this called
/// `Handle::current()` inside one — which panics, leaving an app that runs perfectly and is
/// permanently "not provisioned". So: an async task owns the state, a `spawn_blocking`
/// worker owns the I/O, and progress crosses between them on a channel.
fn spawn_job(job: impl FnOnce(UnboundedSender<Prov>) + Send + 'static) {
    tauri::async_runtime::spawn(async move {
        let core = self_arc();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Prov>();
        let worker = tokio::task::spawn_blocking(move || job(tx));
        while let Some(msg) = rx.recv().await {
            handle(&core, msg).await;
        }
        if let Err(e) = worker.await {
            eprintln!("{CLI}: the provisioning worker died: {e}");
        }
    });
}

/// Startup: the index first — it is small, and a lexical search within seconds beats a
/// blank window that is technically busy — then the model.
fn spawn_provisioning() {
    tauri::async_runtime::spawn(async {
        let core = self_arc();
        if let Ok(Some(info)) = tokio::task::spawn_blocking(|| provision::load_sync(CLI)).await {
            core.state.lock().await.set_sync(info);
        }
    });
    spawn_job(|tx| {
        match provision::load_cached_index(CLI) {
            Some(c) => {
                let _ = tx.send(Prov::IndexReady(Box::new(c)));
            }
            None => {
                let url = provision::update_url(None);
                let progress = tx.clone();
                match provision::fetch_index(CLI, &url, None, move |st| {
                    let _ = progress.send(Prov::IndexStage(st));
                }) {
                    Ok(Some(c)) => {
                        let _ = tx.send(Prov::IndexReady(Box::new(c)));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = tx.send(Prov::IndexStage(Stage::Failed { reason: format!("{e:#}") }));
                    }
                }
            }
        }
        provision_model(&tx);
    });
}

/// Retry only the model.
fn spawn_model() {
    spawn_job(|tx| provision_model(&tx));
}

/// The GUI entry point. Owns the main thread — on macOS the window server accepts nothing
/// else — so it is NOT run on a runtime of its own; Tauri builds one.
pub fn run() {
    let control = tauri::async_runtime::block_on(clappkit::connect_or_die(CLI));
    let core = Arc::new(Core {
        state: Mutex::new(AppState::default()),
        embedder: Mutex::new(None),
        control,
    });

    // Closing quits: this app does nothing between sessions, so a background process with
    // no window would be a process the human cannot see and did not ask for.
    let policy = WindowPolicy::default();

    tauri::Builder::default()
        .manage(core.clone())
        .invoke_handler(tauri::generate_handler![run_cmd, asset, open_url])
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let _ = CORE.set(core.clone());
            kit::apply_icon(&handle, ICON);

            let ipc_core = core.clone();
            kit::spawn_ipc(handle, CLI, policy, move |req, caller| {
                let core = ipc_core.clone();
                async move { core.apply(req, caller).await }
            });

            spawn_provisioning();
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("{CLI}: cannot start the window: {e}");
            std::process::exit(1);
        });

    let _ = APP_ID;
}
