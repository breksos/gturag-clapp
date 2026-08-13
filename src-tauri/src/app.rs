//! The GUI role: Tauri wiring, the control pipe, and provisioning.
//!
//! Glue only. Every decision worth testing is in [`crate::state`], which is why nothing
//! here has a test — there is nothing here to be right or wrong about that a window server
//! is not required to observe.

use crate::provision;
use crate::state::{AppState, By, Stage};
use crate::{APP_ID, CLI};
use clappkit::app::{self as kit, Reply};
use clappkit::{Control, WindowPolicy};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

/// The app's own mark. Bytes stay per-app because they ARE the app's identity.
const ICON: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/icon.png"));

/// The window handle and the core, reachable from a command handler.
///
/// `sync` is the reason these exist: it is the one verb that restarts a background job,
/// so it needs the handle that pushes snapshots and an owned `Arc<Core>` to hand the
/// thread — and it arrives over the IPC channel, where neither is a parameter. Set once,
/// during `setup`, before anything can be served.
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
    ///
    /// Both come out of ONE critical section on purpose: an app that returns a response
    /// and then re-locks to take a snapshot leaves a window in which provisioning can
    /// interleave, so the state pushed to the webview describes a different moment than
    /// the response the caller got.
    pub async fn apply(&self, req: Value, caller: Option<String>) -> Reply {
        let by = match caller {
            Some(id) => By::Agent(id),
            None => By::Human,
        };
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let arg = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_string();

        // `get` reaches the network, so it resolves under the lock, downloads WITHOUT it,
        // and only then rejoins — holding the state lock across a download would freeze
        // the window for the length of a file transfer.
        let fetched: Option<Result<(String, String), String>> = if cmd == "get" {
            let target = {
                let s = self.state.lock().await;
                s.corpus().and_then(|c| {
                    crate::index::resolve(c, &arg("id")).map(|(_, d)| (d.id.clone(), d.url.clone()))
                })
            };
            Some(match target {
                Some((id, url)) => provision::fetch_form_text(CLI, &id)
                    .map(|text| (text, url))
                    .map_err(|e| e.to_string()),
                None => Err(format!("no form matches `{}`", arg("id"))),
            })
        } else {
            None
        };

        // Embedding must happen OUTSIDE the state lock — it is the one slow step here.
        let qvec = match cmd.as_str() {
            "search" => self.embed(&arg("query")).await,
            "sort" => {
                let q = self.state.lock().await.query().to_string();
                if q.is_empty() { None } else { self.embed(&q).await }
            }
            _ => None,
        };

        let mut state = self.state.lock().await;
        let mut resp = match cmd.as_str() {
            "state" | "status" => json!({ "ok": true }),
            "search" => {
                let emits = state.search(&arg("query"), qvec.as_deref(), &by);
                self.control.emit_all(emits);
                json!({ "ok": true })
            }
            "open" => match state.open(&arg("id"), &by) {
                Ok(emits) => {
                    self.control.emit_all(emits);
                    json!({ "ok": true })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "save" => match state.save(&arg("id"), &by) {
                Ok(emits) => {
                    self.control.emit_all(emits);
                    json!({ "ok": true })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "unsave" => match state.unsave(&arg("id"), &by) {
                Ok(emits) => {
                    self.control.emit_all(emits);
                    json!({ "ok": true })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "sort" => match crate::index::Sort::parse(&arg("by")) {
                Some(s) => {
                    let emits = state.set_sort(s, qvec.as_deref(), &by);
                    self.control.emit_all(emits);
                    json!({ "ok": true })
                }
                None => json!({ "ok": false, "error": "sort by relevance, code or title" }),
            },
            "get" => match fetched.expect("computed above for this verb") {
                Ok((text, url)) => json!({ "ok": true, "text": text, "url": url }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "sync" => {
                match APP.get() {
                    Some(handle) => {
                        // Force: `sync` is both "is there a newer index?" and the retry
                        // for a step that failed, so it re-runs rather than short-circuits.
                        spawn_provisioning(self_arc(), handle.clone(), true);
                        json!({ "ok": true })
                    }
                    None => json!({ "ok": false, "error": "the window is not up yet" }),
                }
            }
            other => json!({ "ok": false, "error": format!("unknown command `{other}`") }),
        };

        // The roster is read from the live pipe every time rather than cached: a rename
        // arrives as a fresh snapshot and must update the label in place.
        state.agents = self.control.roster();
        let snapshot = clappkit::snapshot::with_rev(state.snapshot());
        // The caller gets the snapshot too, so a CLI can print state without a second
        // round trip.
        if let (Some(o), Some(s)) = (resp.as_object_mut(), snapshot.as_object()) {
            for (k, v) in s {
                o.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        Reply::new(resp, snapshot)
    }
}

#[tauri::command]
async fn run_cmd(core: tauri::State<'_, Arc<Core>>, app: tauri::AppHandle, req: Value) -> Result<Value, String> {
    // The window's own calls are the human's, so no `agent` key is passed.
    let reply = core.apply(req, None).await;
    kit::push_state(&app, reply.snapshot);
    Ok(reply.resp)
}

#[tauri::command]
fn asset(path: String) -> Option<String> {
    kit::asset(&path)
}

/// Open a form on the university's own site, in the user's real browser.
///
/// A plain `<a target="_blank">` does nothing in a Tauri window: WebView2 raises
/// `NewWindowRequested` and, with no handler, the click is swallowed. So the one link this
/// app has must travel through the core.
///
/// Two constraints, because this hands a string to the operating system's URL handler:
///
/// * **`https` only.** Not a formality — `file:`, `javascript:` and the various shell
///   schemes are exactly what a URL opener is abused for, and every URL we legitimately
///   open comes from our own index and is an `https://www.gtu.edu.tr/...` link.
/// * **No shell.** `cmd /C start` would parse `&` and `|` out of the URL; `rundll32
///   url.dll,FileProtocolHandler` takes the URL as one argument and no shell sees it.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("refusing to open a non-https URL: {url}"));
    }
    // Percent-encode before handing it over. Almost every URL in this corpus contains
    // spaces and Turkish letters — `.../FR-0083 YL-DR Danışman Değişikliği Formu R1.pdf` —
    // and a URL handler that receives a raw space treats what follows as another argument.
    // Encoding the whole set at once is why this is done here rather than left to callers.
    let encoded = encode_url(&url);
    let spawned = if cfg!(target_os = "windows") {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &encoded])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&encoded).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&encoded).spawn()
    };
    spawned
        .map(|_| ())
        .map_err(|e| format!("cannot open the browser: {e}"))
}

/// Percent-encode everything a URL may not carry literally, leaving the characters that
/// are structural (`:/?#[]@` and the sub-delimiters) alone — and leaving `%` alone so a
/// URL that is already encoded is not double-encoded into nonsense.
fn encode_url(url: &str) -> String {
    const KEEP: &str = "-._~:/?#[]@!$&'()*+,;=%";
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        if ch.is_ascii_alphanumeric() || KEEP.contains(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_form_url_survives_encoding() {
        // Every character that broke this: a space, and four Turkish letters.
        let raw = "https://www.gtu.edu.tr/fileman/Formlar-Türkçe/FR-0083 Danışman Değişikliği Formu R1.pdf";
        let got = encode_url(raw);
        assert!(!got.contains(' '), "a space reaching the URL handler splits the argument");
        assert!(got.starts_with("https://www.gtu.edu.tr/fileman/"), "{got}");
        assert!(got.ends_with("R1.pdf"), "{got}");
        assert!(got.contains("%20"), "{got}");
        assert!(!got.contains('ü') && !got.contains('ç'), "{got}");
    }

    #[test]
    fn an_already_encoded_url_is_not_encoded_twice() {
        // `%` is preserved, so %20 stays %20 rather than becoming %2520.
        let once = encode_url("https://x.invalid/a%20b");
        assert_eq!(once, "https://x.invalid/a%20b");
        assert_eq!(encode_url(&once), once, "encoding must be idempotent");
    }

    #[test]
    fn query_structure_is_left_alone() {
        let u = "https://x.invalid/p?a=1&b=2#frag";
        assert_eq!(encode_url(u), u);
    }
}

/// What the provisioning worker reports back as it goes.
enum Prov {
    IndexStage(Stage),
    IndexReady(Box<crate::corpus::Corpus>),
    ModelStage(Stage),
    ModelReady(Box<crate::embed::Embedder>),
}

/// Provision in the background: load whatever is already cached, then fetch what is not.
/// Every step pushes a snapshot, because this is the one part of the app the human waits on.
///
/// The shape matters. Downloading and reading 450 MB is *blocking* work, so it belongs on
/// a blocking thread — but a plain `std::thread` has no Tokio context, and the first
/// version of this called `Handle::current()` inside one. That panics immediately, and the
/// only symptom is an app that runs perfectly and is permanently "not provisioned": the
/// window opens, the CLI answers, and nothing ever loads. So: an async task owns the
/// state, a `spawn_blocking` worker owns the I/O, and progress crosses between them on a
/// channel instead of a runtime handle.
fn spawn_provisioning(core: Arc<Core>, app: tauri::AppHandle, force: bool) {
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Prov>();

        let worker = tokio::task::spawn_blocking(move || {
            // The index first: it is small, and a lexical search working within seconds
            // beats a blank window that is technically busy.
            let cached = if force { None } else { provision::load_cached_index(CLI) };
            match cached {
                Some(c) => {
                    let _ = tx.send(Prov::IndexReady(Box::new(c)));
                }
                None => {
                    let progress = tx.clone();
                    match provision::fetch_index(CLI, move |st| {
                        let _ = progress.send(Prov::IndexStage(st));
                    }) {
                        Ok(c) => {
                            let _ = tx.send(Prov::IndexReady(Box::new(c)));
                        }
                        Err(e) => {
                            let _ = tx.send(Prov::IndexStage(Stage::Failed { reason: e.to_string() }));
                        }
                    }
                }
            }

            // Then the model — 450 MB, so it is deliberately last.
            let fetched = if provision::model_present(CLI) {
                Ok(())
            } else {
                let progress = tx.clone();
                provision::fetch_model(CLI, move |st| {
                    let _ = progress.send(Prov::ModelStage(st));
                })
            };
            match fetched.and_then(|_| crate::embed::Embedder::load(&provision::model_dir(CLI))) {
                Ok(e) => {
                    let _ = tx.send(Prov::ModelReady(Box::new(e)));
                }
                Err(e) => {
                    let _ = tx.send(Prov::ModelStage(Stage::Failed { reason: e.to_string() }));
                }
            }
        });

        while let Some(msg) = rx.recv().await {
            match msg {
                Prov::IndexStage(st) => core.state.lock().await.provision.index = st,
                Prov::IndexReady(c) => {
                    let mut s = core.state.lock().await;
                    s.attach(*c);
                    s.provision.index = Stage::Ready;
                }
                Prov::ModelStage(st) => core.state.lock().await.provision.model = st,
                Prov::ModelReady(e) => {
                    *core.embedder.lock().await = Some(*e);
                    core.state.lock().await.provision.model = Stage::Ready;
                }
            }
            let snap = {
                let s = core.state.lock().await;
                clappkit::snapshot::with_rev(s.snapshot())
            };
            kit::push_state(&app, snap);
        }

        if let Err(e) = worker.await {
            eprintln!("{CLI}: the provisioning worker died: {e}");
        }
    });
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
            kit::spawn_ipc(handle.clone(), CLI, policy, move |req, caller| {
                let core = ipc_core.clone();
                async move { core.apply(req, caller).await }
            });

            spawn_provisioning(core.clone(), handle, false);
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("{CLI}: cannot start the window: {e}");
            std::process::exit(1);
        });

    let _ = APP_ID;
}
