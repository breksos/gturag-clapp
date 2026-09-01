# Third-party notices

## The GTÜ emblem — the app's mark

Gebze Teknik Üniversitesi's butterfly emblem is used as this app's icon
(`assets/icon.svg`, and the PNG/ICO derived from it) and as the mark in its window
(`src/Emblem.tsx`).

- Artwork: `assets/gtu-emblem-source.webp`, a clean rendition of the university's emblem,
  kept in the repository so the derivation is auditable.
- Colours: snapped to the values measured from the logo the university itself serves at
  https://www.gtu.edu.tr/fileman/anasayfa_images/gtu_logo_tr.png — navy `#1a3476`, crimson
  `#cd1239`, orange `#f58612`. The artwork above is a lossy WebP whose colours drifted in
  compression; snapping means the app ships the university's palette rather than a codec's
  approximation of it.
- Derivation: `scripts/emblem.py` traces those three flat colours into paths. It is a
  mechanical contour trace — nothing is redrawn, retouched or restyled, which is the point
  (clappkit `docs/icons.md` rule 5: *use the real mark, and don't design one yourself*).
  The wordmark is dropped; only the emblem is used.
- One deliberate deviation: in the window's **dark** colour scheme the emblem's navy is
  lifted to `#5f80d4`. At its true `#1a3476` it sits at a 1.6:1 contrast ratio against the
  dark background and the upper wings simply do not render. Crimson and orange are never
  altered, and the light scheme and the app icon use the mark exactly as drawn.

**The mark is the university's, and this app is not affiliated with or endorsed by Gebze
Teknik Üniversitesi.** It is an independent tool that searches the documents the university
publishes; the emblem identifies whose documents these are, and claims nothing else. It can
be removed on request — `scripts/emblem.py` and `src/Emblem.tsx` are the only two places it
lives.

## intfloat/multilingual-e5-small — the embedding model

Downloaded at first run into the app's data directory; **not** redistributed in this
repository or in the `.clapp` depot.

- Model: https://huggingface.co/intfloat/multilingual-e5-small
- Licence: MIT
- Paper: *Multilingual E5 Text Embeddings: A Technical Report* (Wang et al., 2024)

## The indexed documents

The corpus is the set of forms published by Gebze Teknik Üniversitesi's quality office at
https://www.gtu.edu.tr/kategori/2382/0/display.aspx. The documents remain the
university's; this project stores extracted text and embeddings for retrieval, and links
to — and downloads on demand — the originals at their published URLs. It does not
redistribute the files themselves.

## Rust and JavaScript dependencies

Full dependency licences are resolvable from `src-tauri/Cargo.lock` and
`package-lock.json`. The direct ones are Apache-2.0/MIT dual-licensed
(`tauri`, `candle`, `tokenizers`, `serde`, `tokio`, `anyhow`, `ureq`, `react`), except
`clappkit` and the `clatch-*` crates, which are Apache-2.0.
