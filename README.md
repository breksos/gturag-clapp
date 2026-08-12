# GTÜ Formlar

Every form Gebze Teknik Üniversitesi publishes, on one screen you and your agent share.

791 documents from the university's [quality-office Formlar page][formlar] — Turkish and
English, `.docx`, `.xlsx`, `.pdf` and pre-2007 `.doc`/`.xls` alike — indexed so you can
search by **what you want to do** rather than by what the form is called.

```
gturag search "danışman değiştirmek istiyorum"
gturag open FR-0083
gturag get FR-0083          # → a local path you can read or fill in
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

1. **An exact form code is decisive.** A code is a name, not a topic; someone who types one
   has already said what they want.
2. **BM25** over the passage text, because half of what people type at a form registry is
   literal — *staj*, *yandal*, *mazeret*.
3. **Dense cosine** over multilingual embeddings, because the other half is not literal at
   all.

(2) and (3) are fused by Reciprocal Rank Fusion, which reads only the ranks — BM25 scores
are unbounded and cosines live in [-1,1], so any weighted sum of them is a constant nobody
can tune honestly.

## Installing

Requires [Clatch][clatch]. A clapp runs only under it.

```sh
clatch install github:breksos/gturag-clapp
clatch run com.gtu.rag
```

First launch downloads the embedding model (~450 MB) and the prebuilt index into the app's
own data directory. There is nothing to configure — no key, no account, no file to edit.
Search answers lexically while the model is still arriving.

To let an agent drive it:

```sh
clatch agent grant <agent> app:com.gtu.rag
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

Two stages, deliberately split. Stage 1 is Python because prying text out of pre-2007
Office files means driving LibreOffice, and that never ships. Stage 2 is the app's own
binary because it must embed passages with **the same code that embeds queries** — a
separate implementation would agree until it quietly didn't, and that failure is invisible.

```sh
python tools/build-index/fetch.py                    # scrape → cull → download → extract
gturag index-corpus tools/build-index/work \
    --model tools/build-index/work/model \
    --built 2026-08-12 --out corpus.gtu              # embed → write the index
```

Stage 1 keeps a document if it lives under the quality office's tree, and keeps only the
highest revision of each form. Re-running it against the live page is what keeps
"currently applied" true as the university revises documents. It reports what it culled
and why, and it says so loudly when LibreOffice is absent rather than silently indexing
153 documents by title alone.

`--built` is an input, not the clock, so two runs over the same corpus produce identical
bytes.

## Releasing

A release carries the depot **and** the index:

```
com.gtu.rag-windows-x64.clapp    the app  (binary + icon + manifest)
com.gtu.rag-windows-x64.clapp.sha256
corpus.gtu                       the index, fetched at first run
```

They are separate so the depot stays small — a `.clapp` is per-OS-arch, and bundling the
index would ship it once per platform — and so the corpus can be refreshed without
rebuilding the app. Because the app fetches `releases/latest/download/corpus.gtu`, **every**
release must attach one, including an app-only fix.

## Licence

Apache-2.0. The indexed documents remain the university's — this project stores extracted
text and embeddings for retrieval and links to the originals. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Not affiliated with or endorsed by Gebze Teknik Üniversitesi.

[formlar]: https://www.gtu.edu.tr/kategori/2382/0/display.aspx
[clatch]: https://github.com/arfium/clatch
