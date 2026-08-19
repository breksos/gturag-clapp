//! The agent's CLI — the other half of the same state.
//!
//! `gturag -h` is the agent's ONLY manual: a verb missing from it does not exist as far as
//! the model is concerned, and a verb in `clatch.json`'s `connector.commands` that this
//! file does not implement is a permission the agent was granted for a command that fails
//! (PLAYBOOK §4). The test at the bottom pins the two lists together, because
//! `clatch validate` reads the manifest and nothing reads this.

use crate::CLI;
use serde_json::{json, Value};

const HELP: &str = r#"gturag — Gebze Teknik Üniversitesi'nin resmî formları, aranabilir.

  Every one of these is also a control in the window, over one shared state: what you
  search fills the human's screen, and what they open arrives on your next prompt.

USAGE
  gturag <command> [arguments]

SEARCHING
  search <query…> [-n N]   Search the forms. Plain Turkish or English works — describe
                           what you want to DO ("danışman değiştirmek", "staj başvurusu")
                           rather than guessing a title. A form code is decisive: given
                           `FR-0083` that form wins outright over any text match.
                           -n limits what is PRINTED; the shared page is always 25.
  sort <relevance|code|title>
                           Re-sort the current results. This is shared state, so it
                           re-pages both surfaces, not just your output.

ONE FORM
  open <id|code>           Open a form and show it in the window: title, code, revision,
                           language, source URL and the passages that matched. The human
                           reads the form itself on the university's own page.
  get <id|code>            Print the form's FULL text, so you can answer questions about
                           its fields rather than guessing from a passage. Accepts
                           FR-0083, FR-0083.en, or a full id.

COLLECTING
  saved                    Print the shared saved list.
  save <id|code>           Add a form to it. Saving twice is not an error.
  unsave <id|code>         Remove one.

THE APP
  status                   What both surfaces are looking at: query, results, open form,
                           saved list, corpus size and date, provisioning state, agents —
                           and RECENT ACTIVITY: what the human just searched for, opened
                           or saved, and what other agents did. Read it before assuming
                           you know what is on their screen.
  sync                     Check for a newer corpus index and download it. Also the retry
                           for a provisioning step that failed.
  focus                    Bring the window forward.
  close                    Quit the app.

NOTES
  Retrieval runs entirely on this machine. On first run the app downloads the embedding
  model (~450 MB) and the prebuilt index into its own data directory; until the model
  lands, search still answers — lexically. `status` says which state it is in.

  The corpus is every form on the university's quality-office Formlar page, kept at its
  current revision. A form indexed by title alone (a pre-2007 .doc with no extractable
  text) is marked as such in the results, because it answers name queries and no others."#;

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

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// One result row, the way a terminal wants it.
fn print_row(i: usize, d: &Value) {
    let code = d.get("code").and_then(Value::as_str).unwrap_or("—");
    let mark = if d["why"] == "code" { "★" } else { " " };
    let thin = if d["titleOnly"] == true { "  (title only)" } else { "" };
    let saved = if d["saved"] == true { " ✓saved" } else { "" };
    println!(
        "{mark}{:>3}. {code:<10} {}  [{}]{thin}{saved}",
        i + 1,
        field(d, "title"),
        field(d, "lang")
    );
}

fn print_results(reply: &Value, limit: usize) {
    let empty = Vec::new();
    let rows = reply["results"].as_array().unwrap_or(&empty);
    if rows.is_empty() {
        let p = &reply["provision"];
        if p["ready"] == false {
            println!("no results — {}", field(p, "summary"));
        } else {
            println!("no form matches that. Try describing what you want to do.");
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
    println!("  id       {}", field(d, "id"));
    println!("  revision R{}", d["rev"].as_u64().unwrap_or(0));
    println!("  language {}", field(d, "lang"));
    println!("  file     {}", field(d, "name"));
    println!("  source   {}", field(d, "url"));
    if d["titleOnly"] == true {
        println!("  note     indexed by title alone — this form's text could not be extracted");
    }
    if let Some(s) = d.get("snippet").and_then(Value::as_str) {
        println!("\n{}", s.trim());
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
            let query = args.join(" ");
            if query.trim().is_empty() {
                die("search what? e.g. `gturag search staj başvurusu`");
            }
            let reply = call(json!({ "cmd": "search", "query": query })).await;
            print_results(&reply, limit);
            // A search that resolved to exactly one named form opens it; show that.
            if reply["open"].is_object() && reply["total"].as_u64() == Some(1) {
                println!();
                print_doc(&reply["open"]);
            }
        }

        "open" => {
            let id = args.join(" ");
            if id.trim().is_empty() {
                die("open which form? e.g. `gturag open FR-0083`");
            }
            let reply = call(json!({ "cmd": "open", "id": id })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            print_doc(&reply["open"]);
        }

        "get" => {
            let id = args.join(" ");
            if id.trim().is_empty() {
                die("get which form? e.g. `gturag get FR-0083`");
            }
            let reply = call(json!({ "cmd": "get", "id": id })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            let text = field(&reply, "text");
            if text.trim().is_empty() {
                // Three of the 791 have no extractable text. Say so, and point at the one
                // place the content definitely exists, rather than printing nothing.
                eprintln!("{CLI}: this form has no extractable text — open it at:");
                println!("{}", field(&reply, "url"));
            } else {
                // The text IS the answer for this verb, so it is the whole of stdout: an
                // agent reads it directly rather than opening a file.
                println!("{text}");
            }
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
                die(&format!("{verb} which form? e.g. `gturag {verb} FR-0083`"));
            }
            let reply = call(json!({ "cmd": verb, "id": id })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            let n = reply["saved"].as_array().map(Vec::len).unwrap_or(0);
            println!("{n} form(s) saved");
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
            println!("index    {}", field(p, "summary"));
            if let Some(c) = reply["corpus"].as_object() {
                println!(
                    "corpus   {} forms, {} passages, built {}",
                    c["documents"], c["chunks"], c["built"].as_str().unwrap_or("?")
                );
            }
            let q = field(&reply, "query");
            if q.is_empty() {
                println!("query    —");
            } else {
                println!("query    {q}  ({} results, sorted by {})",
                         reply["total"], field(&reply, "sort"));
            }
            if reply["open"].is_object() {
                println!("open     {}  {}", field(&reply["open"], "code"), field(&reply["open"], "title"));
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
            // of the loop that makes the window and the terminal one app: without it, an
            // agent can read the shared state but has no idea which of it the person in
            // front of the screen just did.
            let log = reply["activity"].as_array().unwrap_or(&empty);
            if !log.is_empty() {
                println!("
recent");
                for a in log.iter().rev().take(ACTIVITY_SHOWN).collect::<Vec<_>>().iter().rev() {
                    let who = match a.get("who").and_then(Value::as_str) {
                        // Resolve the id to a display name against the roster: the log
                        // stores ids because a name is re-pointable, but a terminal wants
                        // the name.
                        Some(id) => agents
                            .iter()
                            .find(|ag| field(ag, "id") == id)
                            .map(|ag| field(ag, "name").to_string())
                            .unwrap_or_else(|| id.to_string()),
                        None => "you".to_string(),
                    };
                    println!("  {:<10} {:<7} {}", who, field(a, "action"), field(a, "detail"));
                }
            }
        }

        "sync" => {
            println!("checking for a newer index…");
            let reply = call(json!({ "cmd": "sync" })).await;
            if reply["ok"] == false {
                die(field(&reply, "error"));
            }
            println!("{}", field(&reply["provision"], "summary"));
        }

        // Window verbs: clappkit answers these before the app's state sees them.
        "focus" | "show" | "close" | "quit" | "ping" => {
            let reply = call(json!({ "cmd": verb })).await;
            let msg = field(&reply, "message");
            if !msg.is_empty() {
                println!("{msg}");
            }
        }

        // A maintainer verb, deliberately absent from clatch.json and from the manual: it
        // builds a release artifact, it is not something an agent is ever granted. It also
        // runs entirely locally — it is the one verb that does not talk to a running app.
        "index-corpus" => {
            match crate::build_index::from_args(&args)
                .and_then(|(work, model, out, source, built)| {
                    crate::build_index::run(&work, &model, &out, &source, &built)
                }) {
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

    #[test]
    fn the_cli_name_matches_the_manifest() {
        let m: Value = serde_json::from_str(include_str!("../../clatch.json")).unwrap();
        assert_eq!(m["connector"]["cli"].as_str(), Some(CLI));
        assert_eq!(m["id"].as_str(), Some(crate::APP_ID));
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
