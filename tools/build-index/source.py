#!/usr/bin/env python3
"""Everything the corpus pipeline knows about ITS SOURCE, in one module.

The pipeline itself is generic RAG plumbing — list, resolve, download, extract,
clean, name, cull — and none of it should know whose documents it is moving.
What makes this instance "GTÜ Formlar" is the set of constants below: where the
listing pages are, how a document code is spelled, which CDN folders the
register's families map to, what counts as the English sub-collection.

To attach this pipeline to a different registry — another university, a
company's QMS, any site that publishes coded documents — replace this module's
values and, if the new source keeps an inventory workbook, teach register.py its
column layout. fetch.py, probe.py and fetch_register.py import from here and
contain no source names of their own; the Rust side needs no change at all,
because the index format carries its `source` in the header and the retrieval
code derives every code prefix from the corpus it loads.

The one thing that is NOT configuration is the Turkish token folding in the Rust
tokenizer — that is a property of the language the documents are written in, not
of the site they came from, and it lives with the index code on purpose.
"""
from __future__ import annotations

import re
from pathlib import Path

HERE = Path(__file__).resolve().parent

# ---------------------------------------------------------------- identity

ORIGIN = "https://www.gtu.edu.tr"
UA = {"User-Agent": "Mozilla/5.0 (compatible; gturag-index/0.2)"}

# ---------------------------------------------------------------- listing pages
#
# Every page the source publishes quality documents on. ONE list, because
# fetch.py's cull deletes any forms/*.json it did not write this run — split
# across two owners, each run would wipe the other's output.
#
# Not here, and not reachable at all: the families the source hosts behind its
# own SharePoint sign-in (Görev Tanımları, SPİKler, Risk Analizleri, Kaplumbağa
# şemaları, Organizasyon şeması). Even the guestaccess links on the Doküman
# Yönetimi page answer with a sign-in form when fetched anonymously — verified,
# not assumed. They would take a credentialed export from the quality office.
PAGES = [
    ("https://www.gtu.edu.tr/kategori/2382/0/display.aspx", "Formlar"),
    ("https://www.gtu.edu.tr/kategori/3103/0/display.aspx", "Cihaz Kullanım Talimatları"),
    ("https://www.gtu.edu.tr/kategori/2381/0/display.aspx", "İş Akışları"),
    ("https://www.gtu.edu.tr/kategori/9598/0/display.aspx", "Anketler"),
    ("https://www.gtu.edu.tr/kategori/3186/0/display.aspx", "Laboratuvar Talimatları"),
    ("https://www.gtu.edu.tr/kategori/2364/0/display.aspx", "Politikalar"),
    ("https://www.gtu.edu.tr/kategori/7097/0/display.aspx", "Raporlar"),
    ("https://www.gtu.edu.tr/kategori/2877/0/display.aspx", "Kılavuzlar"),
]

# What makes a listed document part of the corpus: it lives under the quality
# office's tree. NOT its exact folder — four real forms sit one and two levels
# above Formlar-Türkçe, and a folder-literal rule silently culled all four.
KEEP_ROOT = "/kalite/"

# The English sub-collections, wherever they appear. A TUPLE, not a string:
# `any(d in path for d in EN_DIR)` over a bare string iterates its CHARACTERS,
# and every path containing an "F" would have been called English.
EN_DIR = ("Formlar-İngilizce", "İngilizce Anketler")

# ---------------------------------------------------------------- codes
#
# `FR-0083`, `FR_0083`, `FR 0083`, the real typo on the page `FR- 0784`, and the
# compound families: `CH-TL-0001`, `LAB-TL-0042`, `İSG-TL-0007`. The trailing
# guard is (?!\d), NOT \b: `CH-TL-0001_VAKUMLU...` continues with an underscore,
# which IS a word character, so a word boundary fails there — what we mean is
# "the number ends here".
CODE_RE = re.compile(r"\b([A-ZÇĞİÖŞÜ]{2,4}(?:-[A-ZÇĞİÖŞÜ]{2,4})?)[-_ ]{0,2}(\d{3,4})(?!\d)")
REV_RE = re.compile(r"\bR(\d{1,2})\b", re.IGNORECASE)

TEXT_EXT = {"docx", "xlsx", "xlsm", "pdf", "doc", "xls"}
LEGACY_EXT = {"doc", "xls"}       # pre-2007 binary formats; LibreOffice territory

# ---------------------------------------------------------------- the register
#
# The source's own inventory workbook: one sheet per family, one row per
# REVISION. register.py parses it; probe.py resolves its rows to CDN URLs.

REGISTER_WORKBOOK = HERE / "LS-0003.xlsx"

# A register CELL is a code or is not one — anchored, unlike the free-text CODE_RE.
REGISTER_CODE = re.compile(r"^([A-ZÇĞİÖŞÜ]{2,6}(?:-[A-ZÇĞİÖŞÜ]{2,6})?)-(\d{1,4})$")

# Column positions shared by every sheet:
# Doküman No | Doküman Adı | Yayın Tarihi | Yayın No | Değ.Tarihi | Değ.No | …
REGISTER_COLUMNS = {"code": 0, "title": 1, "rev": 5}

# Where the register's families live on the CDN. Register sheet name → folder
# candidates under CDN_BASE, most likely first; probe.py proves each with a HEAD
# before anything is downloaded.
CDN_BASE = "https://www.gtu.edu.tr/fileman/Files/UserFiles/kalite"
CDN_FOLDERS = {
    "Organizasyon Şeması": ["Organizasyon Şeması", "Organizasyon Semasi"],
    "Kaplumbaga Şemaları": ["Kaplumbağa Şemaları", "Kaplumbaga Şemaları", "Kaplumbaga Semalari"],
    "SPİKler": ["SPİK", "SPİKler", "SPIK"],
    "Risk Analizleri": ["Risk Analizleri"],
    "Listeler": ["Listeler"],
    "Politikalar": ["Politikalar"],
    "Görev Tanımları": ["Görev Tanımları", "Gorev Tanimlari"],
    "İş Akışları": ["İş Akışları", "İş Akış Şemaları", "Is Akislari"],
    "Yönergeler": ["Yönergeler"],
    "Yönetmelikler": ["Yönetmelikler"],
    "Prosedürler": ["Prosedürler"],
    "El Kitapları": ["El Kitapları", "El Kitaplari"],
    "Sistem Talimatları": ["Sistem Talimatları", "Talimatlar"],
    "Kılavuzlar": ["Kılavuzlar", "Kilavuzlar"],
    "İSG Talimatları": ["İSG Talimatları", "İSG Talimatlar", "ISG Talimatlari"],
    "Laboratuvar Talimatları": ["Laboratuvar Talimatları", "Laboratuvar Talimatlari"],
    "Cihaz Kullanım Talimatları": ["Cihaz Kullanım Talimatları", "Cihaz Kullanma Talimatları"],
    "Anketler": ["Anketler"],
    "Planlar": ["Planlar"],
}
CDN_EXTS = ["pdf", "docx", "xlsx", "doc", "xls"]
