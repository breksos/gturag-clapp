#!/usr/bin/env python3
"""Find the real document URLs by crawling the university's own pages.

The first attempt CONSTRUCTED CDN paths from the register and probed them. It resolved 58
of 1813: the folder names are not the register's sheet names, and the filenames are not the
register's titles. Guessing a URL scheme that nobody promised was the mistake.

This does what worked for the Formlar corpus instead — harvest hrefs a page actually
publishes. A bounded breadth-first crawl of gtu.edu.tr's category/content pages, collecting
every /fileman/ link it meets, so the corpus is defined by what the university links to
rather than by what we can guess it might have named a file.

    python tools/build-index/crawl.py            # writes work/crawl.json

Output: {url: {text, page}} for every document link found, plus a per-directory tally so
the families that are actually reachable are visible rather than assumed.
"""
from __future__ import annotations
import collections, json, re, sys, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

WORK = Path(__file__).resolve().parent / "work"
HOST = "www.gtu.edu.tr"
UA = {"User-Agent": "Mozilla/5.0 (compatible; gturag-index/0.2)"}

# Where the quality office's documents live. Everything under this prefix is in scope;
# anything else on the site (news, staff pages, the press office's KVKK notice) is not.
DOC_PREFIX = "/fileman/Files/UserFiles/kalite"
DOC_EXT = re.compile(r"\.(pdf|docx?|xlsx?|xlsm|pptx?)$", re.I)

SEEDS = [
    "https://www.gtu.edu.tr/kategori/2363/3/display.aspx",   # the quality office itself
    "https://www.gtu.edu.tr/kategori/2368/0/display.aspx",   # Doküman Yönetimi
    "https://www.gtu.edu.tr/kategori/2382/0/display.aspx",   # Formlar (known good)
    "https://www.gtu.edu.tr/kategori/7088/0/display.aspx",   # Süreç Kayıtları
]
MAX_PAGES = 900


def fetch(url: str) -> str:
    try:
        req = urllib.request.Request(url, headers=UA)
        with urllib.request.urlopen(req, timeout=45) as r:
            ctype = r.headers.get("Content-Type", "")
            if "html" not in ctype.lower():
                return ""
            return r.read().decode("utf-8", "replace")
    except Exception:
        return ""


def links(html: str, base: str):
    """(absolute url, visible text) for every anchor."""
    for m in re.finditer(r'<a\b[^>]*href="([^"]+)"[^>]*>(.*?)</a>', html, re.S | re.I):
        href = urllib.parse.unquote(m.group(1)).strip()
        if href.startswith(("mailto:", "tel:", "javascript:", "#")):
            continue
        text = re.sub(r"<[^>]+>", " ", m.group(2))
        text = re.sub(r"&nbsp;?", " ", text)
        yield urllib.parse.urljoin(base, href), re.sub(r"\s+", " ", text).strip()


def main() -> int:
    seen_pages: set[str] = set()
    queue = list(SEEDS)
    docs: dict[str, dict] = {}
    page_count = 0

    while queue and page_count < MAX_PAGES:
        batch, queue = queue[:16], queue[16:]
        batch = [u for u in batch if u not in seen_pages]
        seen_pages.update(batch)
        if not batch:
            continue
        with ThreadPoolExecutor(max_workers=16) as ex:
            pages = list(ex.map(fetch, batch))
        page_count += len(batch)

        for page_url, html in zip(batch, pages):
            if not html:
                continue
            for url, text in links(html, page_url):
                p = urllib.parse.urlparse(url)
                if p.netloc and p.netloc != HOST:
                    continue
                path = urllib.parse.unquote(p.path)
                if path.startswith(DOC_PREFIX) and DOC_EXT.search(path):
                    docs.setdefault(url.split("#")[0], {"text": text, "page": page_url})
                elif re.search(r"/(kategori|icerik)/\d+", path):
                    # Normalise the /tr/ and ?languageId= variants so one page is one page.
                    clean = f"https://{HOST}{path.replace('/tr/', '/')}"
                    if clean not in seen_pages and clean not in queue:
                        queue.append(clean)

        print(f"  pages={page_count} queued={len(queue)} docs={len(docs)}", flush=True)

    WORK.mkdir(parents=True, exist_ok=True)
    (WORK / "crawl.json").write_text(json.dumps(docs, ensure_ascii=False, indent=1), encoding="utf-8")

    dirs = collections.Counter(
        urllib.parse.unquote(urllib.parse.urlparse(u).path).rsplit("/", 1)[0] for u in docs
    )
    print(f"\ncrawled {page_count} pages, found {len(docs)} documents")
    print("--- per directory ---")
    for d, n in dirs.most_common(30):
        print(f"  {n:>5}  {d.replace(DOC_PREFIX, '…')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
