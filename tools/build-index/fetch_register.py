#!/usr/bin/env python3
"""Download and extract the register documents probe.py resolved, into forms/.

The Formlar family comes from its listing page via fetch.py; every OTHER family (yönergeler,
yönetmelikler, görev tanımları, iş akışları, talimatlar, anketler, planlar…) comes from the
university's own register (LS-0003) via probe.py, which proved each URL with a HEAD before
we spend a byte here. This stage downloads the proven set, extracts text with the same
extractors fetch.py uses — one set of extraction bugs, not two — and writes one
forms/<CODE>.tr.json per document in exactly the shape `gturag index-corpus` reads.

    python tools/build-index/fetch_register.py            # incremental; re-uses raw cache
    python tools/build-index/fetch_register.py --refresh
"""
from __future__ import annotations
import argparse, importlib.util, json, sys, time, urllib.parse, urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORK = HERE / "work"
RAW = WORK / "raw-register"
FORMS = HERE.parent.parent / "forms"
from source import UA

# Reuse fetch.py's extractors (docx/xlsx/pdf/legacy via LibreOffice) — one implementation.
spec = importlib.util.spec_from_file_location("fetchmod", HERE / "fetch.py")
fetchmod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetchmod)


def download(url: str, dest: Path, refresh: bool) -> bool:
    if dest.exists() and dest.stat().st_size > 0 and not refresh:
        return True
    q = urllib.parse.quote(url, safe=":/")
    for attempt in range(3):
        try:
            req = urllib.request.Request(q, headers=UA)
            with urllib.request.urlopen(req, timeout=120) as r:
                body = r.read()
            if not body:
                raise OSError("empty body")
            dest.write_bytes(body)
            return True
        except Exception:
            if attempt == 2:
                return False
            time.sleep(1.5 * (attempt + 1))
    return False


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--refresh", action="store_true")
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    probe = json.loads((WORK / "probe.json").read_text(encoding="utf-8"))
    items = list(probe.items())
    if args.limit:
        items = items[: args.limit]
    RAW.mkdir(parents=True, exist_ok=True)
    FORMS.mkdir(exist_ok=True)

    converter = fetchmod.soffice()
    legacy = sum(1 for _, h in items if h["ext"] in ("doc", "xls"))
    if legacy and not converter:
        print(f"!! LibreOffice not found — {legacy} legacy files will be title-only", file=sys.stderr)

    written, empty, failed = 0, [], []
    for i, (code, h) in enumerate(items, 1):
        # The code is filesystem-safe already (letters, digits, hyphens); keep it verbatim
        # so the id in the index and the file on disk can never disagree.
        raw = RAW / f"{code}.{h['ext']}"
        if not download(h["url"], raw, args.refresh):
            failed.append(code)
            continue
        body = fetchmod.clean(fetchmod.extract(raw, h["ext"], converter))
        if not body:
            empty.append(code)
        rec = {
            "id": f"{code}.tr",
            "code": code,
            "rev": h["rev"],
            "lang": "tr",
            "title": h["title"],
            "name": urllib.parse.unquote(h["url"].rsplit("/", 1)[-1]),
            "ext": h["ext"],
            "url": h["url"],
            "text": body,
        }
        out = FORMS / f"{code}.tr.json"
        out.write_text(json.dumps(rec, ensure_ascii=False, indent=1, sort_keys=True) + "\n",
                       encoding="utf-8")
        written += 1
        if i % 100 == 0 or i == len(items):
            print(f"  {i}/{len(items)}  written={written} empty={len(empty)} failed={len(failed)}")

    (WORK / "register-report.json").write_text(json.dumps(
        {"written": written, "empty": empty, "failed": failed}, ensure_ascii=False, indent=1),
        encoding="utf-8")
    print(f"\nwritten : {written}")
    print(f"no text : {len(empty)}  (indexed by title alone)")
    print(f"failed  : {len(failed)}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
