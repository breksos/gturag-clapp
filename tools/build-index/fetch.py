#!/usr/bin/env python3
"""Stage 1 of the corpus build: scrape, cull, download, extract, chunk.

Everything ugly about the source material lives here, on purpose. This script never
ships — it runs on a maintainer's machine, so it may lean on LibreOffice for the 153
pre-2007 Office files that no pure-Rust reader will ever open. What ships is stage 2's
output, and the clapp only ever *loads* that.

    python tools/build-index/fetch.py            # incremental; re-uses work/raw
    python tools/build-index/fetch.py --refresh  # re-download everything

Writes, under tools/build-index/work/:
    docs.jsonl     one record per kept document (id, code, rev, lang, title, url, ext)
    chunks.jsonl   one record per passage  (doc_id, ord, text)
    report.json    what was kept, what was culled, and why
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time
import unicodedata
import urllib.parse
import urllib.request
import zipfile
from collections import defaultdict
from pathlib import Path

# Everything source-specific — the listing pages, the code grammar, the CDN map,
# the keep-rule roots — lives in source.py. This file is the generic half: list,
# download, extract, clean, cull, write. Attaching a different registry means
# editing source.py, not this.
from source import (
    CODE_RE, EN_DIR, KEEP_ROOT, LEGACY_EXT, ORIGIN, PAGES, REV_RE, TEXT_EXT, UA,
)

HERE = Path(__file__).resolve().parent
WORK = HERE / "work"
RAW = WORK / "raw"
TXT = WORK / "txt"


# ---------------------------------------------------------------- scraping the page

def fetch_page(url: str) -> str:
    """A listing page, with the same patience the document downloader already had.

    This used to be a bare urlopen. One connection timeout on one of eight pages then
    aborted the whole build with a traceback — after the university had already served
    most of them. A page fetch is exactly as likely to blip as a file fetch; it deserves
    the same three tries.
    """
    last = None
    for attempt in range(3):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=120) as r:
                return r.read().decode("utf-8", "replace")
        except Exception as e:  # noqa: BLE001 — reported below if all tries fail
            last = e
            if attempt < 2:
                print(f"    retrying {url} ({e})")
                time.sleep(3 * (attempt + 1))
    raise SystemExit(f"fetch.py: cannot read {url} after 3 tries: {last}")


def parse_listing(html: str) -> list[dict]:
    """Every <a> pointing into /fileman/, with its visible label."""
    out = []
    for href, label in re.findall(
        r'<a\b[^>]*href\s*=\s*"([^"]*fileman[^"]*)"[^>]*>(.*?)</a>', html, re.I | re.S
    ):
        text = re.sub(r"<[^>]+>", " ", label)
        text = text.replace("&nbsp;", " ").replace("&amp;", "&")
        text = re.sub(r"\s+", " ", text).strip()
        # NFC first, always. The site serves some Turkish filenames DECOMPOSED — `İ` as
        # `I` + U+0307 — which is a different codepoint from the `İ` in CODE_RE's character
        # class, so `İA-0163` matched nothing while `İA-0164` matched fine. Normalising the
        # path and the label here means every comparison downstream is in one form.
        path = unicodedata.normalize("NFC", urllib.parse.unquote(href))
        text = unicodedata.normalize("NFC", text)
        url = path if path.startswith("http") else ORIGIN + path
        name = path.rsplit("/", 1)[-1]
        stem, _, ext = name.rpartition(".")
        ext = ext.lower()

        in_corpus = KEEP_ROOT in path
        lang = ("en" if any(d in path for d in EN_DIR) else "tr") if in_corpus else None
        m = CODE_RE.search(f"{stem} {text}")
        code = f"{m.group(1)}-{int(m.group(2)):04d}" if m else None
        r = REV_RE.search(stem)
        rev = int(r.group(1)) if r else 0

        # The filename carries the human title; strip the code and the revision marker
        # off the front and back so the displayed title is the form's actual name.
        title = re.sub(r"[_]+", " ", stem)
        title = CODE_RE.sub("", title, count=1)
        title = REV_RE.sub("", title)
        title = re.sub(r"\s+", " ", title).strip(" -_")
        out.append(
            dict(url=url, path=path, name=name, ext=ext, lang=lang,
                 code=code, rev=rev, title=title or stem, label=text)
        )
    return out


def cull(records: list[dict]) -> tuple[list[dict], list[dict]]:
    """The keep-rule, stated once.

    Two things get dropped and nothing else: a document outside the two Formlar folders
    (not a form), and a superseded revision of a form we also have at a higher R.
    Written as a rule rather than a hand-listed exclusion so that re-running this against
    the live page keeps "currently applied" true as GTÜ revises documents.
    """
    kept, culled = [], []

    seen_url = set()
    staged = []
    for r in records:
        if r["url"] in seen_url:
            continue
        seen_url.add(r["url"])
        if r["lang"] is None:
            r["cull_reason"] = "outside the quality office's document tree — not a form"
            culled.append(r)
        elif r["ext"] not in TEXT_EXT:
            r["cull_reason"] = f"unreadable format .{r['ext']}"
            culled.append(r)
        else:
            staged.append(r)

    groups = defaultdict(list)
    for r in staged:
        groups[(r["code"], r["lang"]) if r["code"] else (r["name"], r["lang"])].append(r)

    for _, group in groups.items():
        best = max(group, key=lambda x: (x["rev"], x["ext"] in ("docx", "xlsx")))
        for r in group:
            if r is best:
                kept.append(r)
            else:
                r["cull_reason"] = (
                    f"superseded by R{best['rev']} ({best['name']})"
                    if r["rev"] < best["rev"] else f"duplicate of {best['name']}"
                )
                culled.append(r)
    return kept, culled


# ---------------------------------------------------------------- downloading

def doc_id(rec: dict) -> str:
    """A stable, filesystem-safe id: the form code plus language, or a slug of the name."""
    base = rec["code"] or re.sub(r"[^A-Za-z0-9]+", "-", rec["name"].rsplit(".", 1)[0])[:40]
    return f"{base}.{rec['lang']}".strip("-.")


def download(rec: dict, refresh: bool) -> Path | None:
    RAW.mkdir(parents=True, exist_ok=True)
    dest = RAW / f"{doc_id(rec)}.{rec['ext']}"
    if dest.exists() and dest.stat().st_size > 0 and not refresh:
        return dest
    # The page's hrefs carry raw UTF-8; the server wants them percent-encoded.
    url = urllib.parse.quote(rec["url"], safe=":/?&=%")
    for attempt in range(3):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=120) as r:
                body = r.read()
            if not body:
                raise OSError("empty body")
            dest.write_bytes(body)
            return dest
        except Exception as e:  # noqa: BLE001 — the report records every failure
            if attempt == 2:
                rec["error"] = f"download failed: {e}"
                return None
            time.sleep(1.5 * (attempt + 1))
    return None


# ---------------------------------------------------------------- text extraction

def _xml_text(blob: bytes) -> str:
    s = blob.decode("utf-8", "replace")
    s = re.sub(r"<w:p\b[^>]*>", "\n", s)          # a Word paragraph is a line break
    s = re.sub(r"<w:br\b[^>]*/?>", "\n", s)
    s = re.sub(r"<[^>]+>", " ", s)
    return s


def text_docx(path: Path) -> str:
    with zipfile.ZipFile(path) as z:
        parts = [n for n in z.namelist()
                 if n.startswith("word/") and n.endswith(".xml")
                 and ("document" in n or "header" in n or "footer" in n)]
        return "\n".join(_xml_text(z.read(n)) for n in sorted(parts))


def text_xlsx(path: Path) -> str:
    with zipfile.ZipFile(path) as z:
        shared = []
        if "xl/sharedStrings.xml" in z.namelist():
            xml = z.read("xl/sharedStrings.xml").decode("utf-8", "replace")
            for si in re.findall(r"<si>(.*?)</si>", xml, re.S):
                shared.append("".join(re.findall(r"<t[^>]*>(.*?)</t>", si, re.S)))
        out = []
        for n in sorted(x for x in z.namelist() if re.match(r"xl/worksheets/sheet\d+\.xml$", x)):
            sheet = z.read(n).decode("utf-8", "replace")
            for row in re.findall(r"<row[^>]*>(.*?)</row>", sheet, re.S):
                cells = []
                for attrs, body in re.findall(r"<c\b([^>]*)>(.*?)</c>", row, re.S):
                    v = re.search(r"<v>(.*?)</v>", body, re.S)
                    if v:
                        val = v.group(1)
                        t = re.search(r'\bt="([^"]+)"', attrs)
                        if t and t.group(1) == "s":
                            i = int(val)
                            val = shared[i] if i < len(shared) else ""
                    else:
                        val = "".join(re.findall(r"<t[^>]*>(.*?)</t>", body, re.S))
                    val = val.strip()
                    if val:
                        cells.append(val)
                if cells:
                    out.append(" | ".join(cells))
        return "\n".join(out)


def text_pdf(path: Path) -> str:
    try:
        from pypdf import PdfReader
    except ImportError:
        return ""
    try:
        return "\n".join((p.extract_text() or "") for p in PdfReader(str(path)).pages)
    except Exception:  # noqa: BLE001
        return ""


def soffice() -> str | None:
    for c in ("soffice", "soffice.exe",
              r"C:\Program Files\LibreOffice\program\soffice.exe",
              r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
              "/Applications/LibreOffice.app/Contents/MacOS/soffice"):
        p = shutil.which(c) or (c if os.path.exists(c) else None)
        if p:
            return p
    return None


def text_legacy(path: Path, converter: str | None) -> str:
    """.doc / .xls — convert to the modern format first, then read it as usual."""
    if not converter:
        return ""
    target = "docx" if path.suffix.lower() == ".doc" else "xlsx"
    outdir = WORK / "converted"
    outdir.mkdir(parents=True, exist_ok=True)
    converted = outdir / (path.stem + "." + target)
    if not converted.exists():
        try:
            subprocess.run(
                [converter, "--headless", "--norestore", "--convert-to", target,
                 "--outdir", str(outdir), str(path)],
                check=True, timeout=180,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
        except Exception:  # noqa: BLE001
            return ""
    if not converted.exists():
        return ""
    return text_docx(converted) if target == "docx" else text_xlsx(converted)


def extract(path: Path, ext: str, converter: str | None) -> str:
    try:
        if ext == "docx":
            return text_docx(path)
        if ext in ("xlsx", "xlsm"):
            return text_xlsx(path)
        if ext == "pdf":
            return text_pdf(path)
        if ext in LEGACY_EXT:
            return text_legacy(path, converter)
    except Exception as e:  # noqa: BLE001
        return f""
    return ""


def clean(text: str) -> str:
    text = unicodedata.normalize("NFC", text)
    text = text.replace("\u00a0", " ").replace("\r", "\n")
    text = re.sub(r"[ \t]+", " ", text)
    text = re.sub(r"\n\s*\n\s*", "\n", text)
    return "\n".join(line.strip() for line in text.split("\n") if line.strip())


# ---------------------------------------------------------------- the form database
#
# One JSON file per form, committed to the repository. This is the project's database:
# small, diffable, and readable by anything. The originals stay on the university's
# servers — we hold their text, not their bytes, which is also what keeps the licensing
# honest.
#
# Chunking deliberately does NOT happen here. A chunk is an index-building decision, and
# keeping it on the Rust side means the passage policy can change without re-scraping 791
# documents or needing LibreOffice ever again.

FORMS = HERE.parent.parent / "forms"


def write_form(rec: dict, did: str, body: str) -> Path:
    FORMS.mkdir(parents=True, exist_ok=True)
    path = FORMS / f"{did}.json"
    payload = {
        "id": did,
        "code": rec["code"],
        "rev": rec["rev"],
        "lang": rec["lang"],
        "title": rec["title"],
        "name": rec["name"],
        "ext": rec["ext"],
        "url": rec["url"],
        "text": body,
    }
    # Sorted keys and a trailing newline so a re-scrape produces a MINIMAL diff — the point
    # of committing these is being able to see what actually changed when GTÜ revises a form.
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=1, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return path


# ---------------------------------------------------------------- main

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--refresh", action="store_true", help="re-download every document")
    ap.add_argument("--limit", type=int, default=0, help="stop after N documents (a smoke test)")
    args = ap.parse_args()

    WORK.mkdir(parents=True, exist_ok=True)
    records = []
    for url, family in PAGES:
        html = fetch_page(url)
        found = parse_listing(html)
        records += found
        print(f"→ {family:<28} {len(found)} links  ({url.rsplit('/', 3)[1]})")

    # The regulations nobody links to. probe.py resolved these against the university's own
    # register by HEAD-checking constructed URLs, so each one here is known to exist.
    probe = WORK / "probe.json"
    if probe.exists():
        extra = json.loads(probe.read_text(encoding="utf-8"))
        for code, h in extra.items():
            path = urllib.parse.urlparse(h["url"]).path
            name = urllib.parse.unquote(path.rsplit("/", 1)[-1])
            stem, _, ext = name.rpartition(".")
            records.append(dict(
                url=h["url"], path=urllib.parse.unquote(path), name=name, ext=ext.lower(),
                lang="tr", code=code, rev=h["rev"], title=h["title"], label=h["title"],
            ))
        print(f"→ {'from the register':<28} {len(extra)} links  (probe.json)")

    # The workflows are published as bare codes — `İA-0432.pdf`, no title in the filename
    # and no link text either — so 379 of them arrived titled only with their own number.
    # A document whose title is its code is findable by code and by body text and by
    # nothing else, which for a corpus people search in sentences is most of the way to
    # invisible. The university's register names every one of them, so borrow the name.
    register_path = WORK / "register.json"
    named = 0
    if register_path.exists():
        register = json.loads(register_path.read_text(encoding="utf-8"))
        for r in records:
            code = r.get("code")
            if not code or code not in register:
                continue
            bare = r["title"].replace(" ", "") == code.replace(" ", "") or not r["title"]
            better = register[code].get("title", "").strip()
            if bare and better:
                r["title"] = unicodedata.normalize("NFC", better)
                named += 1
        print(f"→ {'titled from the register':<28} {named} documents")

    kept, culled = cull(records)
    print()
    print(f"  listed {len(records)} links → kept {len(kept)}, culled {len(culled)}")
    # Only the interesting culls by name; a corpus this size supersedes too many to list.
    superseded = sum(1 for c in culled if "superseded" in c.get("cull_reason", ""))
    for c in culled:
        if "superseded" not in c.get("cull_reason", ""):
            print(f"    – {c['name'][:70]}  ({c['cull_reason']})")
    print(f"    – …and {superseded} superseded revisions")

    if args.limit:
        kept = kept[: args.limit]

    converter = soffice()
    legacy = sum(1 for r in kept if r["ext"] in LEGACY_EXT)
    if legacy and not converter:
        # PLAYBOOK: a packaging fallback that "just copies" ships the wrong artifact on
        # exactly the machines nobody watches. Say it loudly rather than quietly indexing
        # 153 documents as title-only.
        print(f"  !! LibreOffice not found — {legacy} legacy .doc/.xls files will be "
              f"indexed by title alone. Install it and re-run to fix.", file=sys.stderr)

    TXT.mkdir(parents=True, exist_ok=True)
    docs, failures, written = [], [], set()
    for i, rec in enumerate(kept, 1):
        did = doc_id(rec)
        path = download(rec, args.refresh)
        body = ""
        if path:
            body = clean(extract(path, rec["ext"], converter))
        else:
            failures.append({"id": did, "name": rec["name"], "error": rec.get("error")})
            # The page lists this document but its file 404s — the university's own link
            # is broken. If the database already holds text for the SAME revision, keep
            # it: a broken link must not erase text we extracted from that very revision.
            # A different revision is not kept — that text would be silently wrong.
            stale = FORMS / f"{did}.json"
            if stale.is_file():
                prev = json.loads(stale.read_text(encoding="utf-8"))
                if prev.get("text") and prev.get("rev") == rec["rev"]:
                    body = prev["text"]
                    failures[-1]["kept_previous_text"] = True

        write_form(rec, did, body)
        written.add(f"{did}.json")
        docs.append(dict(id=did, chars=len(body)))

        if i % 50 == 0 or i == len(kept):
            print(f"  {i}/{len(kept)}")

    # A form GTÜ has withdrawn must LEAVE the database, or it stays searchable forever.
    # The live page is the authority on what currently applies, so anything in forms/ that
    # the page no longer lists is removed — and named, so the deletion is never silent.
    removed = []
    if FORMS.is_dir() and not args.limit:
        for stale in sorted(FORMS.glob("*.json")):
            if stale.name not in written:
                stale.unlink()
                removed.append(stale.name)
    for name in removed:
        print(f"    – removed {name} (no longer listed on any source page)")

    empty = [d["id"] for d in docs if d["chars"] == 0]
    report = dict(pages=[u for u, _ in PAGES], listed=len(records), kept=len(kept), culled=len(culled),
                  culled_detail=[{"name": c["name"], "reason": c["cull_reason"]} for c in culled],
                  documents=len(docs), removed=removed,
                  download_failures=failures, no_text=empty,
                  libreoffice=bool(converter))
    (WORK / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2),
                                      encoding="utf-8")

    print(f"\n  forms     : {len(docs)} → {FORMS}")
    print(f"  no text   : {len(empty)}  (indexed by title alone)")
    print(f"  removed   : {len(removed)}")
    print(f"  failures  : {len(failures)}")
    print(f"\n  next:  gturag index-corpus --built $(date +%F) --model <dir>")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
