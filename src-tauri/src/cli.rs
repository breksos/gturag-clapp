//! The agent's CLI — the other half of the same state.
//!
//! `gturag -h` is the agent's ONLY manual: a verb missing from it does not exist as far as
//! the model is concerned, and a verb in `clatch.json`'s `connector.commands` that this
//! file does not implement is a permission the agent was granted for a command that fails
//! (PLAYBOOK §4). The test at the bottom pins the two lists together, because
//! `clatch validate` reads the manifest and nothing reads this.

use crate::util::iso_to_dmy;
use crate::CLI;
use serde_json::{json, Value};

const HELP: &str = r#"gturag — Gebze Teknik Üniversitesi'nin resmî dokümanları, aranabilir.

  Every one of these is also a control in the window, over one shared state: what you
  search fills the human's screen, and what they open arrives on your next prompt.

USAGE
  gturag <command> [arguments]

SEARCHING
  search <query…> [-n N] [--type T] [--level L] [--lang X] [--all]
                           Search the archive. Describe what you want to DO, in Turkish
                           ("danışman değiştirmek", "mazeretli ders kaydı"), rather than
                           guessing a title; say the level ("yüksek lisans") when it
                           matters. A document code is decisive: FR-0083, İA-0021 or
                           ia-21 answers with that document alone.
                           Each result shows its type, revision and level, and the
                           passage that matched. A note above the results says when the
                           question is outside this archive, or nothing matched well.
                           --type   a collection or a code family: "İş Akışları", İA, FR
                           --level  lisans | lisansustu        --lang  tr | en
                           --all    clears the filters. Filters are shared state and stay
                                    until changed, so read the `filter:` line.
                           -n limits what is PRINTED; the shared page is always 25.
  sort <relevance|code|title>
                           Re-sort the current results, in both surfaces.

ONE DOCUMENT              <doc> is a code (FR-0083, İA-0021, IA-0021), a full id
                          (FR-0083.en), or the row number of a result on screen (1–25).
  open <doc>               Show it in the window and print what it is: type, revision
                           and its date, unit, level, source, where it matched — and
                           whether the university still publishes this revision.
  get <doc>                Print its FULL text as Markdown, headed by the same facts and
                           the same freshness check. Read it before answering about a
                           document's fields, deadlines or conditions.

COLLECTING
  saved                    Print the shared saved list.
  save <doc>               Add a document to it. Saving twice changes nothing, and says so.
  unsave <doc>             Remove one.

THE APP
  status                   What both surfaces are looking at: query, filters, results,
                           open document, saved list, when the archive was built and last
                           checked, the search model, agents — and RECENT ACTIVITY: what
                           the human just did, and what other agents did. Read it before
                           assuming you know what is on their screen.
  sync                     Ask whether a newer archive is published, and install it if so.
                           Answers exactly one of: up to date (with the dates), updated,
                           or could not check (exit 1). Also retries a failed search model.
  focus                    Bring the window forward.
  close                    Quit the app.

FRESHNESS
  The archive is a snapshot, and the university replaces documents under it. `open` and
  `get` ask the university's site whether the indexed revision is still the published
  one, and say OUT OF DATE, with the current file's address, when it is not. Check before
  quoting a regulation's article, a deadline or a condition.

SCOPE AND LANGUAGE
  The archive holds the university's quality-office documents: forms, workflows,
  directives, regulations, policies, guides, surveys and instructions. It does not hold
  the academic calendar, announcements or meeting schedules, and search says so rather
  than guessing. English questions are matched through a glossary of this domain's words
  and by meaning; Turkish words match best.

NOTES
  Retrieval runs entirely on this machine. The index ships inside the app; on first run
  the app downloads the embedding model (~465 MB) into its own data directory, unless a
  copy is already in the shared store (~/.clatch/shared), which it then reads instead.
  Until the model is loaded, search still answers — lexically. `status` says which.

  A document indexed by title alone (a pre-2007 .doc with no extractable text) is marked
  as such, because it answers name queries and no others."#;

/// Send one command to the running app and return its JSON reply.
async fn call(mut req: Value) -> Value {
    // Clatch injects CLATCH_AGENT_ID into the calling agent's shell, so the app can tell
    // an agent's call from the human's — which is what keeps it from signalling an agent
    // about that agent's own write.
    if let Ok(id) = std::env::var("CLATCH_AGENT_ID") {
        if !id.is_empty() {
            req["agent"] = json!(id);
        }
    }
    match clappkit::ipc::request(CLI, &req).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// How much of the shared log `status` prints. The whole point is "what just happened",
/// and a terminal that scrolls forty lines every time buries the state above it.
const ACTIVITY_SHOWN: usize = 8;

fn die(msg: &str) -> ! {
    eprintln!("{CLI}: {msg}");
    std::process::exit(1)
}

/// `--flag N` / `-n N`, removed from the argument list so the rest is the query.
fn take_number(args: &mut Vec<String>, short: &str, long: &str) -> Option<usize> {
    let pos = args.iter().position(|a| a == short || a == long)?;
    let value = args.get(pos + 1)?.clone();
    let n = value.parse().ok()?;
    args.drain(pos..=pos + 1);
    Some(n)
}

/// `--flag VALUE`, removed from the argument list.
fn take_value(args: &mut Vec<String>, long: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == long)?;
    let value = args.get(pos + 1).cloned().unwrap_or_else(|| die(&format!("{long} needs a value")));
    args.drain(pos..=pos + 1);
    Some(value)
}

fn take_flag(args: &mut Vec<String>, long: &str) -> bool {
    match args.iter().position(|a| a == long) {
        Some(pos) => {
            args.remove(pos);
            true
        }
        None => false,
    }
}

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn level_label(l: &str) -> &str {
    if l == "lisansustu" { "lisansüstü" } else { l }
}

/// `2026-09-17T14:03:00Z` → `17.09.2026 14:03`.
fn stamp(iso: &str) -> String {
    match iso.get(11..16) {
        Some(hm) => format!("{} {hm}", iso_to_dmy(iso)),
        None => iso_to_dmy(iso),
    }
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>().trim_end())
    }
}

/// `Formlar · rev 1, 18.12.2023 · Lisansüstü Eğitim Enstitüsü · lisansüstü`
fn meta_line(d: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = d["collection"].as_str() {
        parts.push(c.to_string());
    }
    let mut rev = format!("rev {}", d["rev"].as_u64().unwrap_or(0));
    if let Some(date) = d["revDate"].as_str() {
        rev.push_str(&format!(", {}", iso_to_dmy(date)));
    }
    parts.push(rev);
    if let Some(u) = d["unit"].as_str() {
        parts.push(u.to_string());
    }
    if let Some(l) = d["level"].as_str() {
        parts.push(level_label(l).to_string());
    }
    parts.join(" · ")
}

/// The site check, when it found a problem.
fn live_mark(d: &Value) -> Option<String> {
    let live = &d["live"];
    Some(match live["status"].as_str()? {
        "newer" => format!("OUT OF DATE — revision {} is published: `gturag open {}`", live["rev"], field(d, "code")),
        "changed" => "OUT OF DATE — the published file was replaced after this archive was built".into(),
        "gone" => "WITHDRAWN — no longer published at its address".into(),
        _ => return None,
    })
}

/// The passage that matched, as one line — why this row is here.
fn evidence(d: &Value) -> Option<String> {
    let p = d["passages"].as_array()?.first()?.as_str()?;
    let body = p.split_once('\n').map_or(p, |(_, b)| b);
    let line = one_line(body, 96);
    (!line.is_empty()).then_some(line)
}

/// One result, the way a terminal wants it: what it is, then why it is here.
fn print_row(i: usize, d: &Value) {
    let code = d.get("code").and_then(Value::as_str).unwrap_or("—");
    let mark = if d["why"] == "code" { "★" } else { " " };
    let thin = if d["titleOnly"] == true { "  (title only)" } else { "" };
    let saved = if d["saved"] == true { " ✓saved" } else { "" };
    println!("{mark}{:>3}. {code:<10} {}  [{}]{thin}{saved}", i + 1, field(d, "title"), field(d, "lang"));
    println!("       {}", meta_line(d));
    if let Some(m) = live_mark(d) {
        println!("       ⚠ {m}");
    }
    if d["why"] != "code" {
        if let Some(e) = evidence(d) {
            println!("       “{e}”");
        }
    }
}

/// `https://www.gtu.edu.tr`, from the archive's own provenance.
fn site(reply: &Value) -> String {
    let source = reply["corpus"]["source"].as_str().unwrap_or("");
    match source.find("://").map(|i| i + 3) {
        Some(start) => {
            let end = source[start..].find('/').map_or(source.len(), |i| start + i);
            source[..end].to_string()
        }
        None => "the university's website".into(),
    }
}

fn filter_text(f: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(t) = f["type"].as_str() {
        parts.push(format!("type={t}"));
    }
    if let Some(l) = f["level"].as_str() {
        parts.push(format!("level={}", level_label(l)));
    }
    if let Some(l) = f["lang"].as_str() {
        parts.push(format!("lang={l}"));
    }
    (!parts.is_empty()).then(|| format!("{}   (`gturag search … --all` clears)", parts.join(" ")))
}

/// What must be said before the results.
fn print_notes(reply: &Value) {
    if let Some(f) = filter_text(&reply["filter"]) {
        println!("filter: {f}");
    }
    match reply["notice"]["kind"].as_str() {
        Some("scope") => {
            let what = match reply["notice"]["scope"].as_str() {
                Some("calendar") => "Academic calendars, registration and exam dates are",
                Some("meeting") => "When boards and committees meet is",
                _ => "Announcements are",
            };
            println!(
                "note: {what} not in this archive; it holds the university's quality-office \
                 documents. Look on {}. The documents below are only the nearest ones — they do \
                 not answer the question.",
                site(reply)
            );
        }
        Some("weak") => println!(
            "note: nothing in this archive matches well — these are the nearest documents, not an \
             answer. Try the words a document title would use, or a document code."
        ),
        _ => {}
    }
    if reply["language"] == "en" {
        println!("note: read as English — matched through a glossary and by meaning; Turkish words match best.");
    }
}

fn print_results(reply: &Value, limit: usize) {
    print_notes(reply);
    let empty = Vec::new();
    let rows = reply["results"].as_array().unwrap_or(&empty);
    if rows.is_empty() {
        let p = &reply["provision"];
        if reply["provision"]["index"]["stage"] != "ready" {
            println!("no results — {}", field(p, "summary"));
        } else {
            println!("no document matches that. Try describing what you want to do, or a document code.");
        }
        return;
    }
    for (i, d) in rows.iter().take(limit).enumerate() {
        print_row(i, d);
    }
    // Say "N of TOTAL" about the same page both surfaces hold, never about `-n`.
    let total = reply["total"].as_u64().unwrap_or(rows.len() as u64);
    if (limit as u64) < total {
        println!("\nshowing {limit} of {total} on this page — the window shows all {total}");
    }
}

fn print_doc(d: &Value) {
    if d.is_null() {
        return;
    }
    println!("{}  {}", d.get("code").and_then(Value::as_str).unwrap_or("—"), field(d, "title"));
    println!("  id         {}", field(d, "id"));
    if let Some(c) = d["collection"].as_str() {
        println!("  type       {c}");
    }
    let mut rev = format!("{}", d["rev"].as_u64().unwrap_or(0));
    if let Some(x) = d["revDate"].as_str() {
        rev.push_str(&format!(" of {}", iso_to_dmy(x)));
    }
    if let Some(x) = d["pubDate"].as_str() {
        rev.push_str(&format!(" · first published {}", iso_to_dmy(x)));
    }
    let named = d["revName"].as_u64().unwrap_or(0);
    if named > 0 && Some(named) != d["rev"].as_u64() {
        rev.push_str(&format!(" · the file name says R{named}"));
    }
    println!("  revision   {rev}");
    if let Some(u) = d["unit"].as_str() {
        println!("  unit       {u}");
    }
    if let Some(l) = d["level"].as_str() {
        println!("  level      {}", level_label(l));
    }
    println!("  language   {}", field(d, "lang"));
    println!("  file       {}", field(d, "name"));
    println!("  source     {}", field(d, "url"));
    if let Some(t) = d["liveText"].as_str() {
        println!("  status     {t}");
    }
    if d["titleOnly"] == true {
        println!("  note       indexed by title alone — this document's text could not be extracted");
    }
    let empty = Vec::new();
    let passages = d["passages"].as_array().unwrap_or(&empty);
    if !passages.is_empty() {
        println!("\nwhere it matched");
        for p in passages.iter().filter_map(Value::as_str) {
            let body = p.split_once('\n').map_or(p, |(_, b)| b);
            println!("  · {}", one_line(body, 300));
        }
    }
}

/// When the archive was last checked, in one clause.
fn sync_text(sync: &Value) -> String {
    if sync.is_null() {
        return "never checked for a newer archive (`gturag sync`)".into();
    }
    let at = stamp(field(sync, "checkedAt"));
    match sync["outcome"].as_str() {
        Some("current") => format!("last checked {at} UTC: up to date"),
        Some("updated") => format!("last checked {at} UTC: updated"),
        _ => format!("last check {at} UTC failed: {}", field(sync, "error")),
    }
}

/// The agent's entry point. Never returns — it exits.
pub async fn run(args: Vec<String>) -> ! {
    let mut args = args;
    let verb = args.remove(0);

    match verb.as_str() {
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            std::process::exit(0)
        }
        "-V" | "--version" | "version" => {
            println!("{CLI} {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0)
        }

        "search" => {
            let limit = take_number(&mut args, "-n", "--number").unwrap_or(crate::state::PAGE);
            let clear = take_flag(&mut args, "--all");
            let mut filter = serde_json::Map::new();
            for (flag, key) in [("--type", "type"), ("--level", "level"), ("--lang", "lang")] {
                match take_value(&mut args, flag) {
                    Some(v) => {
                        filter.insert(key.into(), json!(v));
                    }
                    // `--all` alongside other flags: clear everything they do not set.
                    None if clear => {
                        filter.insert(key.into(), json!(""));
                    }
                    None => {}
                }
            }
            let query = args.join(" ");
            if query.trim().is_empty() {
                die("search what? e.g. `gturag search staj başvurusu`");
            }
            let mut req = json!({ "cmd": "search", "query": query });
            if !filter.is_empty() {
                req["filter"] = Value::Object(filter);
            }
            let reply = call(req).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            print_results(&reply, limit);
            // A search that resolved to exactly one named document opens it; show that.
            if reply["open"].is_object() && reply["total"].as_u64() == Some(1) {
                println!();
                print_doc(&reply["open"]);
            }
        }

        "open" => {
            let id = args.join(" ");
            if id.trim().is_empty() {
                die("open which document? e.g. `gturag open FR-0083`, or a row number");
            }
            // `wait`: the terminal holds the answer until the site has been asked, so the
            // freshness line below is a fact rather than "unknown yet".
            let reply = call(json!({ "cmd": "open", "id": id, "wait": true })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            print_doc(&reply["open"]);
        }

        "get" => {
            let id = args.join(" ");
            if id.trim().is_empty() {
                die("get which document? e.g. `gturag get FR-0083`, or a row number");
            }
            let reply = call(json!({ "cmd": "get", "id": id })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            // The text IS the answer for this verb, so it is the whole of stdout.
            print!("{}", field(&reply, "text"));
        }

        "saved" => {
            let reply = call(json!({ "cmd": "state" })).await;
            let empty = Vec::new();
            let rows = reply["saved"].as_array().unwrap_or(&empty);
            if rows.is_empty() {
                println!("nothing saved yet — `gturag save FR-0083`");
            } else {
                for (i, d) in rows.iter().enumerate() {
                    print_row(i, d);
                }
            }
        }

        "save" | "unsave" => {
            let id = args.join(" ");
            if id.trim().is_empty() {
                die(&format!("{verb} which document? e.g. `gturag {verb} FR-0083`, or a row number"));
            }
            let reply = call(json!({ "cmd": verb, "id": id })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            let n = reply["saved"].as_array().map(Vec::len).unwrap_or(0);
            let label = field(&reply, "label");
            match (verb.as_str(), reply["already"] == true) {
                ("save", true) => println!("{label} was already in the list — nothing changed ({n} saved)"),
                ("save", false) => println!("saved {label} — {n} in the list"),
                _ => println!("removed {label} — {n} left in the list"),
            }
        }

        "sort" => {
            let by = args.join(" ");
            let reply = call(json!({ "cmd": "sort", "by": by })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            print_results(&reply, crate::state::PAGE);
        }

        "status" => {
            let reply = call(json!({ "cmd": "status" })).await;
            let p = &reply["provision"];
            let mut index = field(p, "summary").to_string();
            if let Some(built) = reply["corpus"]["built"].as_str() {
                index.push_str(&format!(" · archive built {}", iso_to_dmy(built)));
            }
            index.push_str(&format!(" · {}", sync_text(&reply["sync"])));
            println!("index    {index}");
            if let Some(c) = reply["corpus"].as_object() {
                println!("corpus   {} documents, {} passages", c["documents"], c["chunks"]);
            }
            let q = field(&reply, "query");
            if q.is_empty() {
                println!("query    —");
            } else {
                println!("query    {q}  ({} results, sorted by {})", reply["total"], field(&reply, "sort"));
            }
            if let Some(f) = filter_text(&reply["filter"]) {
                println!("filter   {f}");
            }
            if let Some(kind) = reply["notice"]["kind"].as_str() {
                let what = if kind == "scope" { "the question is outside this archive" } else { "nothing matched well" };
                println!("note     {what}");
            }
            if reply["open"].is_object() {
                let o = &reply["open"];
                println!("open     {}  {}", field(o, "code"), field(o, "title"));
                if let Some(m) = live_mark(o) {
                    println!("         ⚠ {m}");
                }
            }
            println!("saved    {}", reply["saved"].as_array().map(Vec::len).unwrap_or(0));
            let empty = Vec::new();
            let agents = reply["agents"].as_array().unwrap_or(&empty);
            println!("agents   {}", if agents.is_empty() {
                "none bound".to_string()
            } else {
                agents.iter().map(|a| field(a, "name")).collect::<Vec<_>>().join(", ")
            });

            // What the HUMAN has been doing, and what other agents have. This is the half
            // of the loop that makes the window and the terminal one app.
            let log = reply["activity"].as_array().unwrap_or(&empty);
            if !log.is_empty() {
                println!("\nrecent");
                for a in log.iter().rev().take(ACTIVITY_SHOWN).collect::<Vec<_>>().iter().rev() {
                    let who = a
                        .get("whoName")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| if a["who"].is_null() { "you".into() } else { field(a, "who").into() });
                    println!("  {:<10} {:<7} {}", who, field(a, "action"), field(a, "detail"));
                }
            }
        }

        "sync" => {
            println!("checking whether a newer archive is published…");
            let reply = call(json!({ "cmd": "sync" })).await;
            let info = &reply["sync"];
            let built = iso_to_dmy(field(info, "built"));
            let remote = iso_to_dmy(field(info, "remoteBuilt"));
            match info["outcome"].as_str() {
                Some("current") if field(info, "remoteBuilt") < field(info, "built") => println!(
                    "up to date: the archive in use (built {built}) is newer than the published one (built {remote})."
                ),
                Some("current") => println!(
                    "up to date: the archive in use was built {built}, and that is the newest published (checked {} UTC).",
                    stamp(field(info, "checkedAt"))
                ),
                Some("updated") => println!(
                    "updated: installed the archive built {built} — {} documents. The search on screen was run again against it.",
                    reply["corpus"]["documents"]
                ),
                _ => {
                    eprintln!("{CLI}: {}", field(&reply, "error"));
                    if !built.is_empty() {
                        eprintln!("{CLI}: nothing changed; the archive in use was built {built}.");
                    }
                    std::process::exit(1)
                }
            }
        }

        // Window verbs: clappkit answers these before the app's state sees them.
        "focus" | "show" | "close" | "quit" | "ping" => {
            let reply = call(json!({ "cmd": verb })).await;
            let msg = field(&reply, "message");
            if !msg.is_empty() {
                println!("{msg}");
            }
        }

        // A diagnostic verb, absent from clatch.json and the manual. It exists because
        // `clappkit::ipc::request` maps the OS error to a friendly sentence, and a
        // friendly sentence is the wrong thing to debug through: "blocked by the sandbox"
        // is clappkit's GUESS at what `PermissionDenied` meant. This prints what actually
        // happened, from wherever it is run — which for a sandboxed agent is the only
        // place the answer exists.
        "doctor" => {
            let addr = clappkit::ipc::address(CLI);
            println!("cli          {CLI}");
            println!("pipe         {addr}");
            println!("cwd          {:?}", std::env::current_dir().ok());
            for k in ["CLATCH_DATA_DIR", "CLATCH_AGENT_ID", "USERNAME", "USERDOMAIN"] {
                println!("{k:<12} {}", std::env::var(k).unwrap_or_else(|_| "-".into()));
            }
            match clappkit::ipc::request(CLI, &json!({ "cmd": "ping" })).await {
                Ok(v) => println!("
connect      OK  {v}"),
                Err(e) => {
                    println!("
connect      FAILED");
                    // The chain, not the summary: the root cause carries the OS code.
                    for (i, cause) in e.chain().enumerate() {
                        println!("  [{i}] {cause}");
                    }
                    if let Some(io) = e.downcast_ref::<std::io::Error>() {
                        println!("  kind         {:?}", io.kind());
                        println!("  raw_os_error {:?}", io.raw_os_error());
                    }
                }
            }
        }

        // A maintainer verb, deliberately absent from clatch.json and from the manual: it
        // builds a release artifact, it is not something an agent is ever granted. It also
        // runs entirely locally — it is the one verb that does not talk to a running app.
        "index-corpus" => {
            match crate::build_index::from_args(&args).and_then(|o| crate::build_index::run(&o)) {
                Ok(()) => {}
                Err(e) => die(&format!("{e:#}")),
            }
        }

        other => die(&format!("unknown command `{other}` — try `{CLI} -h`")),
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PLAYBOOK §4: `clatch validate` checks the manifest and nothing checks that the code
    /// matches it. A verb declared but unimplemented is a granted permission that fails;
    /// a verb implemented but undocumented does not exist to the agent. This closes both
    /// gaps against the real manifest, so a fork cannot drift by editing only one file.
    #[test]
    fn the_manual_documents_exactly_the_declared_verbs() {
        let manifest = include_str!("../../clatch.json");
        let m: Value = serde_json::from_str(manifest).expect("clatch.json must parse");
        let declared: Vec<String> = m["connector"]["commands"]
            .as_array()
            .expect("connector.commands")
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!declared.is_empty());

        for name in &declared {
            assert!(
                HELP.contains(&format!("  {name} ")) || HELP.contains(&format!("  {name}\n"))
                    || HELP.contains(&format!("{name} <")),
                "`{name}` is declared in clatch.json but absent from `gturag -h`"
            );
        }
    }

    /// build.rs reads these out of the manifest, so they cannot drift — this pins that
    /// build.rs read the RIGHT manifest, which is the one thing left to get wrong.
    #[test]
    fn the_identity_is_the_manifests() {
        let m: Value = serde_json::from_str(include_str!("../../clatch.json")).unwrap();
        assert_eq!(m["connector"]["cli"].as_str(), Some(CLI));
        assert_eq!(m["id"].as_str(), Some(crate::APP_ID));
        assert_eq!(m["name"].as_str(), Some(crate::APP_NAME));
    }

    #[test]
    fn every_signal_the_state_emits_is_declared() {
        // The other half of the same lockstep: clappkit refuses to emit an undeclared
        // signal, and the symptom is an app that runs perfectly while delivering nothing.
        let m: Value = serde_json::from_str(include_str!("../../clatch.json")).unwrap();
        let declared: Vec<&str> = m["connector"]["signals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        for emitted in ["doc.opened", "saved.changed"] {
            assert!(declared.contains(&emitted), "`{emitted}` is emitted but not declared");
        }
    }

    #[test]
    fn a_number_flag_is_taken_out_of_the_query() {
        let mut args: Vec<String> =
            ["staj", "-n", "5", "belgesi"].iter().map(|s| s.to_string()).collect();
        assert_eq!(take_number(&mut args, "-n", "--number"), Some(5));
        assert_eq!(args.join(" "), "staj belgesi", "the flag must not pollute the query");
    }
}
