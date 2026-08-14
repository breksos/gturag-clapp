#!/usr/bin/env python3
"""Regenerate every icon artifact from the one editable source, assets/icon.svg.

docs/ICONS.md: the mark is regenerated, never hand-traced, and the .ico is derived from
the same PNG so the two cannot drift. PLAYBOOK §9: icons/icon.ico is MANDATORY on Windows
— tauri-build compiles a Windows resource from the first .ico in bundle.icon and fails
without one, even with bundle.active: false.

    python scripts/icon.py

Rasterises with LibreOffice, which is a real SVG renderer and is already a build
dependency of the corpus builder, so this adds nothing new to install.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "assets" / "icon.svg"
PNG = ROOT / "assets" / "icon.png"
ICONS = ROOT / "src-tauri" / "icons"
SIDE = 1024
# The Windows shell size set, in full. Windows picks a different bitmap per context and
# scales the nearest one when the exact size is absent — and a 40px taskbar icon resampled
# from 48 looks resampled. 16/32/48/256 are the classic four; 20/24/40/64/96 are the ones
# 125%/150%/175%/200% display scaling actually asks for, and 128 is what the extra-large
# Explorer view uses.
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]


def soffice() -> str:
    for c in ("soffice", "soffice.exe",
              r"C:\Program Files\LibreOffice\program\soffice.exe",
              r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
              "/Applications/LibreOffice.app/Contents/MacOS/soffice"):
        p = shutil.which(c) or (c if os.path.exists(c) else None)
        if p:
            return p
    # PLAYBOOK: a packaging fallback that "just copies" ships the wrong artifact on
    # exactly the machines nobody watches. Fail loud instead.
    sys.exit("icon.py: LibreOffice not found — it is the rasteriser. Install it and re-run.")


def rasterise() -> Image.Image:
    conv = soffice()
    with tempfile.TemporaryDirectory() as tmp:
        # A private user profile, because LibreOffice allows exactly ONE headless instance
        # per profile: without this, running the icon build while the corpus builder is
        # converting .doc files fails with a bare exit 1 and no explanation.
        profile = (Path(tmp) / "profile").as_uri()
        subprocess.run(
            [conv, f"-env:UserInstallation={profile}",
             "--headless", "--norestore", "--convert-to", "png",
             "--outdir", tmp, str(SVG)],
            check=True, timeout=300,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        out = Path(tmp) / (SVG.stem + ".png")
        if not out.exists():
            sys.exit("icon.py: LibreOffice produced no PNG")
        im = Image.open(out).convert("RGBA")
        im.load()
    # LibreOffice honours the document size but not always to the pixel; the source is
    # square by construction, so a resample here is a normalisation, not a crop.
    if im.size != (SIDE, SIDE):
        im = im.resize((SIDE, SIDE), Image.LANCZOS)
    return im


def measure(im: Image.Image) -> str:
    """ICONS.md: measure the fill rather than trusting your eye."""
    box = im.getbbox()
    if not box:
        return "empty"
    w, h = im.size
    return f"{100 * (box[2] - box[0]) // w}% x {100 * (box[3] - box[1]) // h}%"


def main() -> int:
    if not SVG.exists():
        sys.exit(f"icon.py: {SVG} is missing")
    im = rasterise()

    PNG.parent.mkdir(parents=True, exist_ok=True)
    im.save(PNG, "PNG")

    ICONS.mkdir(parents=True, exist_ok=True)
    # Tauri reads the .ico for the Windows resource and the .png for everything else.
    im.resize((512, 512), Image.LANCZOS).save(ICONS / "icon.png", "PNG")
    im.save(ICONS / "icon.ico", format="ICO",
            sizes=[(s, s) for s in ICO_SIZES])

    size_kib = PNG.stat().st_size / 1024
    print(f"  {PNG.relative_to(ROOT)}  {im.size[0]}x{im.size[1]}  {size_kib:.0f} KiB")
    print(f"  fill: {measure(im)}   (a full-bleed tile should read ~100% x ~100%)")
    print(f"  {(ICONS / 'icon.ico').relative_to(ROOT)}  {ICO_SIZES}")
    if size_kib > 1024:
        sys.exit("icon.py: the PNG exceeds the protocol's 1 MiB ceiling")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
