# GTÜ Formlar

Every form Gebze Teknik Üniversitesi publishes, on one screen you and your agent share.

1850 documents from the university's own quality system — forms, workflows, device and
laboratory instructions, directives, regulations, policies, surveys and guides, Turkish and
English, `.docx`, `.xlsx`, `.pdf` and pre-2007 `.doc`/`.xls` alike — indexed so you can
search by **what you want to do** rather than by what the document is called.

```
gturag search "danışman değiştirmek istiyorum"
gturag open FR-0083
gturag get FR-0083          # → the form's full text, for an agent to answer from
```

Type `FR-0083` and that form wins outright. Type a sentence and it is found by meaning.

## The shape of it

A [clapp][clatch]: one binary, two roles over one state. A window for the human, a CLI for
the agent, and both calling the same methods so they cannot drift. What you open by hand
rides along on your next prompt; what the agent searches for fills your screen.

**The app retrieves; the agent answers.** No language model ships here and none runs on a
server. The only model involved is a sentence embedder, downloaded once per machine into a
cache every clapp of this family shares — after that, retrieval is entirely local and works
offline.

Retrieval is hybrid, in this order:

1. **An exact form code is decisive**, and answers alone. A code is a name, not a topic;
   someone who types one has already said what they want, and padding that with two hundred
   near-random results buries the answer.
2. **Title coverage** — what share of the query's *achievable* words the title contains,
   each weighted by how rare that word is. A form's title is its identity; a word in its
   body is a mention. By word rather than by token, because a word expands to between one
   and four spellings and scoring the expanded list weights it by how many it happened to
   produce — which is how `kullanım talimatı` outvoted `etüv`.
3. **BM25** over the passage text, because half of what people type at a form registry is
   literal — *staj*, *yandal*, *mazeret*.
4. **Dense cosine** over multilingual embeddings, because the other half is not literal at
   all.

(3) and (4) are fused by Reciprocal Rank Fusion, which reads only ranks — BM25 scores are
unbounded and cosines live in [-1,1], so any weighted sum of them is a constant nobody can
tune honestly. (2) is deliberately **not** in that fusion: RRF throws away magnitude, and
with `k=60` a title matching every query term scores barely above one matching a quarter of
them. That is not a subtlety — it is why `danışman değişikliği` once returned
`Danışman Değişikliği Formu` fourth.

Turkish gets three things it needs. `İ` folds to `i` — Rust's own `to_lowercase` produces
`i` plus a combining dot, so `İZİN` never matched `izin`. Every word is indexed alongside a
5-character stem, because `danışmanımı` and `Danışman` share no token otherwise. And every
word is indexed alongside an ASCII-folded spelling, because the university writes Turkish
both ways: `ETUV KULLANIM TALMATI` is a real filename and 874 titles carry no Turkish
letters at all. The folded form is a matching aid only — never displayed, because
`Talimati` is a misspelling and we should not be the ones making it.

## The repository is the database

There is no server and no hosted vector store. This repo holds both halves:

| | |
|---|---|
| `forms/FR-0083.tr.json` | one file per form — metadata and extracted text, ~2 KB each |
| `corpus.gtu` | the built index: 1850 documents, their passages, and their vectors |

`forms/` is the source: reviewable, diffable, and small, so when GTÜ revises a form the
change is visible in a pull request rather than buried in a binary. `corpus.gtu` is derived
from it and **ships inside the `.clapp`**, so a first run is never blocked on the network
for anything but the model.

The original `.docx`/`.pdf` files are never committed and never shipped — only their text.
The authoritative copies stay on the university's servers, and the app links to them.

## Installing

Requires [Clatch][clatch]. A clapp runs only under it.

```sh
clatch install breksos/gturag-clapp
clatch run com.breksos.gturag
```

The index is already inside the app, so search works immediately — lexically. The only
download is the embedding model (~465 MB, from HuggingFace), which switches on
meaning-based search when it lands. There is nothing to configure: no key, no account, no
file to edit.

To let an agent drive it:

```sh
clatch agent grant <agent> app:com.breksos.gturag
```

## Building

```sh
git clone --recurse-submodules https://github.com/breksos/gturag-clapp
cd gturag-clapp
npm install
npm run verify        # tests → package → validate → CLI ⇄ GUI round-trip
```

`clappkit` is carried as a submodule; `git submodule update --remote clappkit` moves it
forward deliberately.

> **A note on dependencies.** There are no private ones. clappkit carries `clapp-ipc` and
> `clapp-pipe` as in-tree crates, so `cargo build` needs no key, no URL rewrite and no
> vendored copy — just the submodule. (Earlier versions pinned the launcher's crates over
> SSH; if you are looking for `vendor/clatch` or `scripts/with-clatch-deps.sh` from an older
> checkout, they are gone and nothing replaced them.)

## Updating the corpus — after publishing, without a release

The corpus is a database, not a release artifact. After the app is published, the loop is:

```sh
python tools/build-index/fetch.py    # stage 1: refresh forms/ from every source
npm run corpus                       # stage 2: re-index — only what changed is embedded
git commit -am "…" && git push       # every installed app picks it up with `gturag sync`
```

**Stage 1** is Python, because prying text out of pre-2007 Office files means driving
LibreOffice, and that never ships. It lists every source page, proves the register's
unlisted documents on the CDN, downloads, extracts and culls — and keeps a document's text
when the university's own link to that revision breaks. `tools/build-index/README.md` maps
the stages; everything source-specific lives in `tools/build-index/source.py`.

**Stage 2** is the app's own binary, because passages must be embedded by the same code that
embeds queries — a separate implementation would agree until it quietly didn't, and that
failure is invisible. It is **incremental**: every document in `corpus.gtu` carries a
fingerprint of what was embedded, so a rebuild reuses the vectors of every unchanged
document and embeds only the rest. Adding one document to 1850 embeds one document;
removing one embeds nothing, and needs no model at all. `--fresh` embeds everything again;
`--built` is an input, not the clock, so two builds over the same database are
byte-identical.

The index carries its own `update_url` and `text_base`, so the app never has to be told
where updates live — the data says. `gturag sync` fetches that URL and installs what
arrives only if it is newer than what is loaded.

## One engine, many registries

Nothing GTÜ-specific is compiled into the engine. The app's identity comes from
`clatch.json` (read by `build.rs`), the scraper's knowledge of the site lives in
`tools/build-index/source.py`, and the index tells the app where its updates and document
texts are. To make `hacettepe-clapp`, or a `hukuk-clapp` over a statute book:

1. Fork. Edit `clatch.json`: `id`, `name`, `connector.cli`, `about`.
2. Replace `assets/gtu-emblem-source.webp` with the new mark and run `scripts/emblem.py`
   then `scripts/icon.py`; redraw `scripts/banner.py`'s motif if you like.
3. Point `tools/build-index/source.py` at the new source: listing pages, code grammar, CDN
   folders, register columns. Run stage 1.
4. Build the index once with its addresses:
   `gturag index-corpus forms --built <date> --model <dir> --update-url <raw corpus.gtu URL> --text-base <raw forms/ URL>`.
   Every later `npm run corpus` carries them forward.
5. `npm run verify`, tag, release.

The retrieval code learns every code family from the corpus it loads — `FR`, `YÖ`, or a
statute book's `KHK` — and what a bare number means is the corpus's most common family,
stamped in the header. The one language-specific piece, Turkish `İ`/`ı` folding and
5-character stemming in `index.rs`, is a property of the documents, not of the site.

## Releasing

A release carries only the depots; the index rides inside each one:

```
com.breksos.gturag-windows-x64.clapp        binary + icon + manifest + corpus.gtu
com.breksos.gturag-windows-x64.clapp.sha256
```

Push a `v*` tag and `release.yml` builds one per platform. It needs no repository secret and
no credentials of any kind — clappkit's crates are in-tree, so a checkout with submodules is
everything the build requires.

## Licence

Apache-2.0. The indexed documents remain the university's — this project stores extracted
text and embeddings for retrieval and links to the originals. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Not affiliated with or endorsed by Gebze Teknik Üniversitesi.

[formlar]: https://www.gtu.edu.tr/kategori/2382/0/display.aspx
[clatch]: https://github.com/arfium/clatch
