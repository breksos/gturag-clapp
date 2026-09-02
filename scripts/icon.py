#!/usr/bin/env python3
"""Regenerate every icon artifact from the one editable source, assets/icon.svg.

clappkit docs/icons.md: the mark is regenerated, never hand-traced, and every derived
artifact comes off the same PNG so they cannot drift. playbook §9: icons/icon.ico is
MANDATORY on Windows — tauri-build compiles a Windows resource from the first .ico in
bundle.icon and fails without one, even with bundle.active: false. The .icns is what the
macOS Dock and Finder read out of the .app bundle scripts/package.sh builds.

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

# The macOS iconset: five sizes, each with its @2x. `iconutil` wants exactly these names.
ICNS_SIZES = [16, 32, 128, 256, 512]

# How much of the 1024 canvas a macOS app icon's ARTWORK occupies. Measured, not quoted:
# /System/Applications/App Store.app's AppIcon.icns rasterised to 1024 has its opaque
# bounding box at exactly (100,100)-(924,924). The library tile is full-bleed on purpose
# — the Clatch shelf wants that (icons.md rule 2) — but the Dock insets every icon, and an
# .app whose icns is full-bleed towers over its neighbours. So the icns carries the margin
# the platform expects, and the two files differ by exactly this number.
MACOS_GRID = 824 / 1024


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


def dock_grid(im: Image.Image) -> Image.Image:
    """The full-bleed tile, inset onto the macOS icon grid with a transparent margin."""
    side = im.size[0]
    art = round(side * MACOS_GRID)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    off = (side - art) // 2
    canvas.paste(im.resize((art, art), Image.LANCZOS), (off, off))
    return canvas


def write_icns(im: Image.Image, dest: Path) -> str:
    """Write a macOS .icns from the tile.

    `iconutil` is the canonical writer and is present wherever a macOS depot is built, so
    it is preferred; Pillow's own ICNS encoder is the fallback for building the artifact
    off a Mac. Both are real encoders — this is not the "packaging fallback that just
    copies" the playbook warns about, and a missing encoder is still a hard failure.
    """
    inset = dock_grid(im)
    tool = shutil.which("iconutil")
    if tool:
        with tempfile.TemporaryDirectory() as tmp:
            iconset = Path(tmp) / "icon.iconset"
            iconset.mkdir()
            for s in ICNS_SIZES:
                inset.resize((s, s), Image.LANCZOS).save(iconset / f"icon_{s}x{s}.png")
                inset.resize((s * 2, s * 2), Image.LANCZOS).save(
                    iconset / f"icon_{s}x{s}@2x.png")
            subprocess.run([tool, "-c", "icns", str(iconset), "-o", str(dest)],
                           check=True, timeout=120)
        return "iconutil"
    try:
        inset.save(dest, format="ICNS")
    except Exception as e:  # noqa: BLE001 — the message is the whole point
        sys.exit(f"icon.py: cannot write {dest.name}: {e}")
    return "Pillow"


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
    icns = write_icns(im, ICONS / "icon.icns")

    size_kib = PNG.stat().st_size / 1024
    print(f"  {PNG.relative_to(ROOT)}  {im.size[0]}x{im.size[1]}  {size_kib:.0f} KiB")
    print(f"  fill: {measure(im)}   (a full-bleed tile should read ~100% x ~100%)")
    print(f"  {(ICONS / 'icon.ico').relative_to(ROOT)}  {ICO_SIZES}")
    print(f"  {(ICONS / 'icon.icns').relative_to(ROOT)}  {icns}  "
          f"(artwork at {MACOS_GRID:.1%} — the macOS grid)")
    if size_kib > 1024:
        sys.exit("icon.py: the PNG exceeds the protocol's 1 MiB ceiling")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
