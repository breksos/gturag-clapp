# Third-party notices

## Lucide — the app icon's glyph

`assets/icon.svg` embeds the unmodified path data of Lucide's `file-search` icon.

- Project: https://lucide.dev — https://github.com/lucide-icons/lucide
- Licence: ISC

```
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of
Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any purpose with or
without fee is hereby granted, provided that the above copyright notice and this
permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO
THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO
EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL
DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

The tile colours are Gebze Teknik Üniversitesi's own (navy `#1a3b79`, orange `#f26522`),
read from the university's live stylesheet. **The university's crest is deliberately not
used**: this is an independent tool, not an official university application, and a mark
that implied otherwise would be the one detail every user notices.

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
