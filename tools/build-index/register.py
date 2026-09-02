#!/usr/bin/env python3
"""Parse the university's document register (LS-0003) into work/register.json.

This is the missing first link of the corpus pipeline. probe.py READS
work/register.json and fetch.py borrows titles from it — but nothing in the
repository ever WROTE it, so the pipeline could not run from a clean clone. The
parsing happened once, by hand, on one machine, and its output was never
reproducible. Now it is: the register workbook is committed beside this script,
and this parses it the same way every time.

    python tools/build-index/register.py                 # tools/build-index/LS-0003.xlsx
    python tools/build-index/register.py path/to/LS.xlsx # a newer copy

The workbook is the quality office's own inventory: nineteen sheets, one per
document family, each row one REVISION of one document — so a document with five
revisions is five rows, and the register truth for "current revision" is the
highest Değ.No its code carries. The Formlar family is deliberately absent from
the register; it has a real listing page and fetch.py owns it.

Output — work/register.json, the shape probe.py already consumes:

    { "YÖ-0001": {"title": "...", "rev": 3, "sheet": "Yönergeler", "status": "Güncel"} }

`status` is always "Güncel" here because THIS workbook is the current-documents
list; the cancelled list is a separate SharePoint export the university does not
serve anonymously. probe.py filters on `status != "İptal"`, so if a cancelled
export ever becomes available, merging it in is one update here and zero
elsewhere.
"""
from __future__ import annotations

import json
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

from source import REGISTER_CODE as CODE
from source import REGISTER_COLUMNS, REGISTER_WORKBOOK as DEFAULT_WORKBOOK

HERE = Path(__file__).resolve().parent
WORK = HERE / "work"

NS = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"

COL_CODE, COL_TITLE, COL_REV = (REGISTER_COLUMNS[k] for k in ("code", "title", "rev"))


def shared_strings(z: zipfile.ZipFile) -> list[str]:
    root = ET.fromstring(z.read("xl/sharedStrings.xml"))
    return ["".join(t.text or "" for t in si.iter(f"{NS}t"))
            for si in root.findall(f"{NS}si")]


def sheet_names(z: zipfile.ZipFile) -> list[str]:
    wb = ET.fromstring(z.read("xl/workbook.xml"))
    return [s.get("name") for s in wb.iter(f"{NS}sheet")]


def rows(z: zipfile.ZipFile, index: int, shared: list[str]):
    """Every row of sheet<index>, as a list of cell strings."""
    sheet = ET.fromstring(z.read(f"xl/worksheets/sheet{index}.xml"))
    for row in sheet.findall(f".//{NS}row"):
        cells = []
        for c in row.findall(f"{NS}c"):
            v = c.find(f"{NS}v")
            if v is None:
                cells.append("")
            elif c.get("t") == "s":
                cells.append(shared[int(v.text)])
            else:
                cells.append(v.text or "")
        yield cells


def canonical(family: str, number: str) -> str:
    """`YÖ-1` and `YÖ-0001` are the same document; the corpus writes 4 digits."""
    return f"{family}-{int(number):04d}"


def parse(workbook: Path) -> dict[str, dict]:
    z = zipfile.ZipFile(workbook)
    shared = shared_strings(z)
    register: dict[str, dict] = {}

    for index, sheet in enumerate(sheet_names(z), 1):
        for cells in rows(z, index, shared):
            if len(cells) <= COL_CODE:
                continue
            m = CODE.match(cells[COL_CODE].strip())
            if not m:
                continue  # headers, banners, blank rows
            code = canonical(m.group(1), m.group(2))
            title = cells[COL_TITLE].strip() if len(cells) > COL_TITLE else ""
            rev_cell = cells[COL_REV].strip() if len(cells) > COL_REV else ""
            rev = int(rev_cell) if rev_cell.isdigit() else 0

            entry = register.get(code)
            if entry is None:
                register[code] = {"title": title, "rev": rev,
                                  "sheet": sheet, "status": "Güncel"}
            else:
                # A later revision row wins, and brings its title with it: the
                # university renames documents across revisions, and the search
                # index should carry the name the current revision actually has.
                if rev >= entry["rev"]:
                    entry["rev"] = rev
                    if title:
                        entry["title"] = title
    return register


def main() -> int:
    workbook = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_WORKBOOK
    if not workbook.is_file():
        sys.exit(f"register.py: {workbook} is missing")

    register = parse(workbook)
    if not register:
        sys.exit(f"register.py: {workbook.name} yielded no codes — is it the register?")

    WORK.mkdir(parents=True, exist_ok=True)
    out = WORK / "register.json"
    out.write_text(json.dumps(register, ensure_ascii=False, indent=1, sort_keys=True),
                   encoding="utf-8")

    from collections import Counter
    fams = Counter(c.rsplit("-", 1)[0] for c in register)
    print(f"  {workbook.name} → {out.relative_to(HERE)}  ({len(register)} documents)")
    for fam, n in fams.most_common():
        print(f"    {fam:8} {n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
