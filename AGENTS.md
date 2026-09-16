# AGENTS.md — working in gturag-clapp

A **clapp**: one binary, two roles over one state — a Tauri window for the human, a CLI for
the agent. (`CLAUDE.md` points here.) The product is search over every document Gebze
Teknik Üniversitesi publishes; the engine underneath knows nothing about GTÜ (README,
"One engine, many registries").

## Orient

- Rust + Tauri v2 on the [`clappkit`](clappkit/README.md) submodule. `gturag app` is the
  window; `gturag <verb>` is the agent's CLI. The identity (`id`, `connector.cli`, `name`)
  is read from `clatch.json` by `build.rs`, so a fork edits the manifest, not the Rust.
- `src-tauri/src/index.rs` is retrieval: an exact code first, then title coverage, then
  BM25 fused with dense cosine. `embed.rs` is multilingual-e5-small in pure Rust, used by
  both the index builder and the running app so the two can never disagree.
- `lexicon.rs` reads a query before it is scored: the education level, filler, an English
  glossary, and the questions this archive cannot answer. The only file that knows the
  domain.
- `meta.rs` reads what a document prints about itself — revision, its date, the unit — and
  its collection and level. `live.rs` asks the university's site whether that revision is
  still the published one. `text.rs` turns extracted text into the Markdown `get` prints.
- `corpus.rs` is the one-file `corpus.gtu` format. `build_index.rs` builds it from
  `forms/*.json`. `provision.rs` downloads the model on first run, fetches a document's
  text, and answers `sync`.
- `state.rs` holds the shared state and its rules. Pure, no I/O, and where the tests are.
- `forms/*.json` and `corpus.gtu` are both committed: the repository is the database.

## The rules you must not soften

- **The app retrieves; the agent answers.** No language model runs here or on a server.
  `get` prints a form's full text so the agent answers from it instead of guessing.
- **An exact document code is decisive**, and answers alone — however it is typed:
  `İA-0021`, `IA-0021` and `ia-21` are one code.
- **Never present a stale or out-of-scope answer as an answer.** A document the university
  has replaced is marked out of date; a question the archive does not cover is told so.
  Both surfaces say it before the results, and an unreachable site is "not checked", never
  "current".
- **Only the human's actions signal.** `doc.opened` and `saved.changed` are sent for what
  the human does; the agent's own `open` or `save` never echoes back to it. `state.rs`
  enforces this, and a test holds it.
- **Every agent verb is declared, and every declared verb is in `gturag -h`.** Two verbs are
  deliberately neither: `index-corpus` builds the index without the app, and `doctor`
  prints the raw pipe diagnosis. They are maintainer tools, never an agent's grant.

## Commands

| goal | command |
|---|---|
| build | `npm run build` |
| **prove it works** | `npm run verify` |
| tests · types · manifest vs code | `npm test` · `npm run typecheck` · `npm run check` |
| package · validate · pack | `npm run package` · `npm run validate` · `npm run pack` |
| refresh the corpus | `npm run index`, then `npm run corpus` |

Clone with `--recurse-submodules`, or clappkit is missing and nothing builds.

## Honest limits

- The freshness check asks gtu.edu.tr with HEAD requests when a document is opened, and in
  the background for a search's top five. It knows a newer revision only when the file name
  carries `R<n>`; an unmarked file is judged by its date.
- The level, scope and weak-match rules are heuristics, tuned on the real corpus with the
  real model (`GTURAG_DEBUG_RANK=1` logs what each query scored). Re-check them after a
  large corpus change.

- The first run downloads the embedding model (~465 MB). Until it lands, search is lexical
  only, and `status` says so. A copy already in `~/.clatch/shared` is read instead.
- `npm run verify` runs on macOS. The Windows and Linux depots come from `release.yml` or a
  build by hand; their script branches follow each platform's rules, and a green macOS run
  proves neither.
- The macOS bundle is `bin/gturag.app`, not `bin/GTÜ Formlar.app`: a space and a non-ASCII
  letter are not safe in `connector.cliBin`. The header of `scripts/package.sh` says why.

## Where the rules are written

- [`clappkit/docs/protocol.md`](clappkit/docs/protocol.md) — the Clapp Protocol. Normative.
- [`clappkit/docs/format.md`](clappkit/docs/format.md) — the `.clapp` package and every manifest field.
- [`clappkit/docs/playbook.md`](clappkit/docs/playbook.md) — shipping rules learned the hard way.
- [README.md](README.md) — retrieval, the corpus loop, forking the engine, releasing.

## House rules

- A comment answers *why*, in a line or two.
- Never `cargo build --release` on its own: without Tauri's `custom-protocol` feature the
  window opens white. `npm run build`.
- `cargo build` cannot see `#[cfg(test)]`. Run `npm test` before believing anything.
- rustls, never native-tls. `pkg/` and `*.clapp` are derived and never committed.
- `scripts/lib.sh`, `build.sh`, `validate.sh`, `pack.sh`, `.github/smoke.sh`, and the shared
  bodies of `package.sh` and `verify.sh` are the family's, byte-identical to template-clapp.
  Fix them there, then copy them here.
