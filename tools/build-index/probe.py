#!/usr/bin/env python3
"""Resolve register entries to live CDN URLs — the discovery half of the corpus expansion.

The university's register (LS-0003) lists 1700+ documents across 19 families, but only the
Formlar family has a listing page. The rest exist as files under predictable paths:

    /fileman/Files/UserFiles/kalite/<Family folder>/<CODE> <Title> R<rev>.<ext>

Nothing links them, so the URL must be CONSTRUCTED from the register and then PROVEN by a
HEAD request — we ingest only URLs that actually resolve, and we report every miss rather
than pretending coverage. Naming on the CDN is inconsistent (spaces vs underscores, R
suffix styles, ASCII-folded Turkish), so each document gets a small ladder of candidate
spellings, most-likely first, with an early exit on the first hit.

    python tools/build-index/probe.py            # writes work/probe.json + a report

Output work/probe.json: {code: {url, size, ext, title, sheet, rev, status}} for every hit.
"""
from __future__ import annotations
import json, re, sys, unicodedata, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORK = HERE / "work"

# The source's shape — CDN root, family folders, extensions — lives in source.py,
# so pointing this pipeline at a different registry never means editing this file.
from source import CDN_BASE as BASE, CDN_EXTS as EXTS, CDN_FOLDERS as FOLDERS, UA


UNSURE = -1


def head(url: str) -> int:
    """Content-Length on success, 0 when the server says the file is not there, and
    UNSURE when it could not be asked. The difference matters: fetch.py deletes a document
    nothing lists, and a timeout is not the server saying "not listed"."""
    import time, urllib.error
    for attempt in range(3):
        try:
            req = urllib.request.Request(urllib.parse.quote(url, safe=":/"), headers=UA, method="HEAD")
            with urllib.request.urlopen(req, timeout=25) as r:
                return int(r.headers.get("Content-Length") or 1)
        except urllib.error.HTTPError as e:
            if e.code < 500:
                return 0
        except Exception:
            pass
        time.sleep(1.5 * (attempt + 1))
    return UNSURE


def find(urls: list[tuple[str, int]], unsure: list[bool] | None = None) -> tuple[str, int, int] | None:
    """The first address that answers, as (url, revision, size) — composed spellings
    first, then the same list decomposed, since the server stores some names as NFD. A
    candidate that could not be asked sets `unsure[0]`."""
    for form in ("NFC", "NFD"):
        for url, rev in urls:
            spelled = unicodedata.normalize(form, url)
            if form == "NFD" and spelled == url:
                continue
            size = head(spelled)
            if size == UNSURE:
                if unsure is not None:
                    unsure[0] = True
            elif size:
                return spelled, rev, size
    return None


def ascii_fold(s: str) -> str:
    table = str.maketrans("çğıöşüÇĞİÖŞÜ", "cgiosuCGIOSU")
    return s.translate(table)


def candidates(folder: str, code: str, title: str, rev: int) -> list[tuple[str, int]]:
    """The spellings to try, with the revision each one is, most likely first. Kept short:
    every entry is a network hit."""
    title = re.sub(r'[\\/:*?"<>|]', "-", title).strip()
    out = []
    stems = [f"{code} {title}", f"{code}_{title.replace(' ', '_')}"]
    revs = [(f" R{rev}", rev), (f"_R{rev}", rev), ("", rev)] if rev > 0 else [("", 0), (" R0", 0), ("_R0", 0)]
    for stem in stems:
        for r, n in revs:
            r2 = r.replace(" ", "_") if "_" in stem else r
            for ext in EXTS[:3]:  # pdf/docx/xlsx cover the register families
                out.append((f"{BASE}/{folder}/{stem}{r2}.{ext}", n))
    # last resort: ASCII-folded, the CDN's occasional habit
    out.append((f"{BASE}/{folder}/{ascii_fold(f'{code} {title}')} R{rev}.pdf", rev))
    return out


def later(folder: str, code: str, title: str, rev: int) -> list[tuple[str, int]]:
    """The register is a snapshot, and the university replaces a file when it publishes a
    new revision: YÖ-0054 R7 answers 404 once R8 is up. So a miss is retried a few
    revisions ahead, in the two most common spellings."""
    title = re.sub(r'[\\/:*?"<>|]', "-", title).strip()
    return [(f"{BASE}/{folder}/{code} {title} R{n}.{ext}", n)
            for n in range(rev + 1, rev + 4) for ext in EXTS[:2]]


def main() -> int:
    register = json.loads((WORK / "register.json").read_text(encoding="utf-8"))
    include = {c: d for c, d in register.items() if d["status"] != "İptal"}
    print(f"register: {len(register)} docs, probing {len(include)} (İptal excluded)")

    # Stage 1 — folder discovery: for each family, a handful of samples across candidates.
    confirmed: dict[str, str] = {}
    for sheet, folders in FOLDERS.items():
        fam = [(c, d) for c, d in include.items() if d["sheet"] == sheet][:8]
        if not fam:
            continue
        for folder in folders:
            hits = 0
            for code, d in fam[:4]:
                for url, _ in candidates(folder, code, d["title"], d["rev"])[:6]:
                    if head(url):
                        hits += 1
                        break
            if hits:
                confirmed[sheet] = folder
                print(f"  folder ok: {sheet!r} -> {folder!r} ({hits}/{min(4,len(fam))} samples)")
                break
        if sheet not in confirmed:
            print(f"  !! no folder found for {sheet!r} — its {sum(1 for d in include.values() if d['sheet']==sheet)} docs will be reported as unreachable")

    # Stage 2 — resolve every included doc in the confirmed families, concurrently.
    def resolve(item):
        code, d = item
        folder = confirmed.get(d["sheet"])
        if not folder:
            return code, None
        unsure = [False]
        hit = (find(candidates(folder, code, d["title"], d["rev"]), unsure)
               or find(later(folder, code, d["title"], d["rev"]), unsure))
        if hit:
            url, rev, size = hit
            return code, {"url": url, "size": size, "ext": url.rsplit(".", 1)[-1],
                          "title": d["title"], "sheet": d["sheet"], "rev": rev,
                          "status": d["status"]}
        return code, ("unsure" if unsure[0] else None)

    hits, misses, unsure = {}, [], []
    # Twelve at a time: the university's server is shared, and a throttled request is a
    # timeout, which is exactly the answer this probe cannot use.
    with ThreadPoolExecutor(max_workers=12) as ex:
        for i, (code, res) in enumerate(ex.map(resolve, include.items()), 1):
            if isinstance(res, dict):
                hits[code] = res
            elif res == "unsure":
                unsure.append(code)
            else:
                misses.append(code)
            if i % 150 == 0:
                print(f"  {i}/{len(include)}  hits={len(hits)}")

    total = sum(h["size"] for h in hits.values())
    (WORK / "probe.json").write_text(json.dumps(hits, ensure_ascii=False, indent=1), encoding="utf-8")
    (WORK / "probe-misses.json").write_text(json.dumps(misses, ensure_ascii=False, indent=1), encoding="utf-8")
    # Codes the server could not be asked about. fetch.py keeps their current text rather
    # than culling them as unlisted.
    (WORK / "probe-unsure.json").write_text(json.dumps(unsure, ensure_ascii=False, indent=1), encoding="utf-8")
    if unsure:
        print(f"  !! {len(unsure)} documents could not be checked (timeouts); fetch.py will keep them: {unsure[:8]}")

    from collections import Counter
    per = Counter(h["sheet"] for h in hits.values())
    print("\n--- resolved per family ---")
    for k, v in per.most_common():
        want = sum(1 for d in include.values() if d["sheet"] == k)
        print(f"  {k:<28} {v}/{want}")
    print(f"\nresolved : {len(hits)}/{len(include)}")
    print(f"misses   : {len(misses)}  (work/probe-misses.json)")
    print(f"TOTAL DOWNLOAD SIZE: {total/1_048_576:.1f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
