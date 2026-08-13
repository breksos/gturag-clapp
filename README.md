# GTÜ Formlar

Every form Gebze Teknik Üniversitesi publishes, on one screen you and your agent share.

791 documents from the university's [quality-office Formlar page][formlar] — Turkish and
English, `.docx`, `.xlsx`, `.pdf` and pre-2007 `.doc`/`.xls` alike — indexed so you can
search by **what you want to do** rather than by what the form is called.

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
server. The only model involved is a sentence embedder, and it is downloaded to your own
machine on first run — after that, retrieval is entirely local and works offline.

Retrieval is hybrid, in this order:

1. **An exact form code is decisive**, and answers alone. A code is a name, not a topic;
   someone who types one has already said what they want, and padding that with two hundred
   near-random results buries the answer.
2. **Title coverage** — what fraction of the query's *achievable* terms the title contains,
   squared. A form's title is its identity; a word in its body is a mention.
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

Turkish gets two things it needs: `İ` folds to `i` (Rust's own `to_lowercase` produces `i`
plus a combining dot, so `İZİN` never matched `izin`), and every word is indexed alongside
a 5-character stem, because `danışmanımı` and `Danışman` share no token otherwise.

## The repository is the database

There is no server and no hosted vector store. This repo holds both halves:

| | |
|---|---|
| `forms/FR-0083.tr.json` | one file per form — metadata and extracted text, ~2 KB each |
| `corpus.gtu` | the built index: 791 documents, their passages, and the vectors |

`forms/` is the source: reviewable, diffable, and small, so when GTÜ revises a form the
change is visible in a pull request rather than buried in a binary. `corpus.gtu` is derived
from it and **ships inside the `.clapp`**, so a first run is never blocked on the network
for anything but the model.

The original `.docx`/`.pdf` files are never committed and never shipped — only their text.
The authoritative copies stay on the university's servers, and the app links to them.

## Installing

Requires [Clatch][clatch]. A clapp runs only under it.

```sh
clatch install github:breksos/gturag-clapp
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

> **A note on dependencies.** clappkit pins the `clatch-*` crates over
> `ssh://git@github.com/arfium/clatch.git`. The same repository is public over HTTPS, so
> rather than requiring a deploy key on every clone and CI run, build through
> `scripts/with-clatch-deps.sh` (or `.ps1`), which rewrites that one remote for the
> duration of one command and touches nothing global. If you hold a key for that remote,
> ignore all of this and build normally.

## Rebuilding the corpus

Two stages, deliberately split.

**Stage 1 — refresh the database.** Python, because prying text out of pre-2007 Office
files means driving LibreOffice, and that never ships:

```sh
python tools/build-index/fetch.py       # scrape → cull → extract → forms/*.json
```

It keeps a document if it lives under the quality office's tree, and keeps only the highest
revision of each form; anything the page no longer lists is **deleted** from `forms/`, so a
withdrawn form stops being searchable. It reports what it culled and why, and says so
loudly when LibreOffice is absent rather than silently indexing 153 documents by title
alone. Commit the diff — that is the corpus update.

**Stage 2 — rebuild the index.** The app's own binary, because it must embed passages with
**the same code that embeds queries**; a separate implementation would agree until it
quietly didn't, and that failure is invisible:

```sh
gturag index-corpus forms --model <dir> --built 2026-08-13 --out corpus.gtu
```

Chunking happens here rather than in the scraper, so passage policy can change without
re-scraping 791 documents or needing LibreOffice again. `--built` is an input, not the
clock, so two runs over the same database produce byte-identical output.

Users pick the new corpus up with `gturag sync`, which fetches `corpus.gtu` from the
default branch — **no release required** to update the forms.

## Releasing

A release carries only the depots; the index rides inside each one:

```
com.breksos.gturag-windows-x64.clapp        binary + icon + manifest + corpus.gtu
com.breksos.gturag-windows-x64.clapp.sha256
```

Push a `v*` tag and `release.yml` builds one per platform. It needs no repository secret:
clappkit pins the clatch crates over SSH, but the same repo is public over HTTPS, so a
single `insteadOf` rewrite in the environment replaces the whole deploy-key problem.

## Licence

Apache-2.0. The indexed documents remain the university's — this project stores extracted
text and embeddings for retrieval and links to the originals. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Not affiliated with or endorsed by Gebze Teknik Üniversitesi.

[formlar]: https://www.gtu.edu.tr/kategori/2382/0/display.aspx
[clatch]: https://github.com/arfium/clatch
