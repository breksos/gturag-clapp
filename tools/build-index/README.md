# The corpus pipeline

Four stages, each its own script, each re-runnable and incremental. Everything
source-specific lives in **`source.py`**; everything below it is generic RAG
plumbing that never names the university.

```
LS-0003.xlsx ──register.py──▶ work/register.json          the inventory, parsed
                     │
                     ▼
              probe.py ───────▶ work/probe.json           register rows proven on
                     │                                    the CDN (HEAD, never GET)
                     ▼
     fetch.py ────────────────▶ ../../forms/*.json        listing pages + proven
     fetch_register.py                                    register rows → downloaded,
                     │                                    text extracted, culled
                     ▼
     npm run corpus ──────────▶ ../../corpus.gtu          chunked + embedded by the SAME
     (gturag index-corpus)                                code that embeds queries — and
                                                          INCREMENTAL: only changed
                                                          documents are embedded
```

- **`register.py`** — parses the source's own inventory workbook into
  `work/register.json`. One row per revision in, one entry per document out.
- **`probe.py`** — the register lists documents nothing links to; this constructs
  their CDN URLs from the family-folder map and proves each with a HEAD request.
  Only proven URLs are ever downloaded.
- **`fetch.py`** — the full build: scrapes the listing pages, merges the proven
  register rows, downloads, extracts text (`docx`/`xlsx` natively, `pdf` via
  pypdf, legacy `.doc`/`.xls` via LibreOffice), and **culls** — a document no
  source lists any more leaves `forms/`, loudly. Run it only with
  `work/probe.json` present, or the register half of the corpus will be culled
  as unlisted.
- **`fetch_register.py`** — the incremental half: downloads and extracts ONLY
  the proven register rows, culls nothing. Safe to run alone.
- **`crawl.py`** — a maintainer's discovery tool: sweeps the site's category
  pages recording which ones link files. Not part of the build.

Stage 2 (chunk + embed) is deliberately in the app's own binary — see
`src-tauri/src/build_index.rs` — because passages must be embedded by the same
code that embeds queries, or retrieval drifts in ways nothing ever reports. It
is incremental: each document's fingerprint is stored in the index, and a
rebuild reuses every unchanged document's vectors. Removing a document needs no
model; adding one embeds one.

## Attaching a different source

This pipeline is not GTÜ-shaped; its instance is. To index a different
registry — another university, a company QMS, any site publishing coded
documents:

1. **`source.py`** — replace the listing pages, code grammar, CDN map and
   keep-rule roots. This is the only file that knows the source.
2. **`register.py`** — if the new source keeps an inventory workbook, adjust
   `REGISTER_COLUMNS` in `source.py`; if it keeps none, skip the register and
   probe stages entirely and let `fetch.py` work from listing pages alone.
3. **The Rust side needs nothing.** The index format carries its `source` URL in
   the header, `find_code` learns every code family from the corpus it loads,
   and the embedder is language-agnostic. The one Turkish-specific piece — the
   `İ/ı` fold and 5-char stemming in `index.rs::tokenize` — is a property of the
   documents' language, not of the site, and would be the thing to revisit for a
   non-Turkish corpus.
4. **Identity** — `clatch.json`, `APP_ID`/`CLI` in `src-tauri/src/main.rs`, and
   the `GTURAG_INDEX_URL`/`GTURAG_FORMS_BASE` build-time overrides in
   `provision.rs` rebrand the app itself.

## What the source cannot give us

The register names 19 families; five of them (Görev Tanımları, SPİKler, Risk
Analizleri, Kaplumbağa şemaları, Organizasyon şeması) are hosted behind the
university's SharePoint sign-in. Even the guestaccess links on the Doküman
Yönetimi page answer anonymously with a sign-in form — verified by fetching
them, not assumed. Those documents would take a credentialed export from the
quality office; every family the CDN actually serves is probed and ingested.
