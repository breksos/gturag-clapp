#!/usr/bin/env python3
"""Trace GTÜ's emblem into the two places this app wears it.

docs/ICONS.md rule 5: *use the real mark, and don't design one yourself.* The window used
to carry a butterfly drawn by hand — the one thing that rule forbids, because a
hand-approximated logo is close enough to read as a mistake. This re-derives the real mark
mechanically: nothing here is redrawn, retouched, or recoloured.

    python scripts/emblem.py

Writes:
    assets/icon.svg     the editable icon source (ICONS.md rule 1)
    src/Emblem.tsx      the same paths, as the window's mark

THE TRACE. The artwork is three flat colours on white, so this is a contour trace and not
a guess: each colour becomes a coverage mask that keeps the artwork's own anti-aliasing —
which is where the sub-pixel edge position lives — and the contours of that are emitted as
paths. Holes come from OpenCV's CCOMP hierarchy and are drawn into the same path, so
`evenodd` punches them out.

The colours are snapped to the values measured from the logo the university itself serves
at gtu.edu.tr (navy #1a3476, crimson #cd1239, orange #f58612). The source artwork here is
a lossy WebP whose colours have drifted a little in compression; snapping means the app
ships the university's palette rather than a codec's opinion of it.

THE TILE. Its shape is not a guess either — it was measured off a real macOS app icon
(`AppIcon.icns` rasterised to 1024). See SQUIRCLE_* below.
"""

from __future__ import annotations

import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "assets" / "gtu-emblem-source.webp"
ICON = ROOT / "assets" / "icon.svg"
COMPONENT = ROOT / "src" / "Emblem.tsx"

# Measured off the university's own published logo,
# https://www.gtu.edu.tr/fileman/anasayfa_images/gtu_logo_tr.png
PALETTE = {"navy": (0x1A, 0x34, 0x76), "crimson": (0xCD, 0x12, 0x39), "orange": (0xF5, 0x86, 0x12)}
ORDER = ["navy", "crimson", "orange"]
BACKGROUND = (0xFF, 0xFF, 0xFF)

UP = 2          # supersample for the trace; the source is already ~1500px tall
EPS = 1.1       # approxPolyDP tolerance in upsampled pixels
MIN_AREA = 6.0  # drop contours smaller than this many source pixels — codec speckle

SIDE = 1024

# --- the tile's shape, measured rather than assumed ----------------------------------
#
# /System/Applications/App Store.app/Contents/Resources/AppIcon.icns, rasterised to
# 1024x1024, has its opaque bounding box at exactly (100,100)-(924,924): the macOS icon
# grid is an 824 square centred in 1024, i.e. 80.5% of the canvas. Fitting its corner
# profile gives a SUPERELLIPSE of radius 220 and exponent 2.5 — mean error 0.59 px, against
# 0.70 px for the plain rounded rectangle of radius 185.4 that everyone quotes. The plain
# rectangle is an approximation of this shape; this is the shape.
#
# So the corner ratio is 220/824 = 0.2670, and the exponent is 2.5. ICONS.md rule 2 says
# 0.225 — that number describes the same corner as a plain arc, and is superseded here
# because the platform's own mask is available to measure.
#
# The tile stays FULL-BLEED at 1024 rather than being inset to 824. That is deliberate and
# it is what makes both platforms right at once:
#   * the Clatch shelf wants full-bleed, or one icon floats while its neighbours fill
#     (ICONS.md rule 2);
#   * `clappkit::icon::dock_icon` scales a full-bleed tile to DOCK_FILL = 0.80 of the
#     canvas before handing it to the macOS Dock — 819 px against the grid's 824, half a
#     percent under — so the Dock gets the macOS geometry without this file knowing about
#     the Dock at all.
SQUIRCLE_RADIUS = 0.2670
SQUIRCLE_EXPONENT = 2.5
SQUIRCLE_STEPS = 48     # points per corner; at 1024 px this is smooth past the pixel grid

GLYPH_HEIGHT = 0.62     # of the tile — see write_icon


def squircle_path(side: float) -> str:
    """The macOS icon silhouette at `side` px, as one closed path.

    A superellipse corner joined to straight edges. Sampled rather than fitted to Béziers:
    the shape is generated, never hand-tuned, and at 48 points a corner the polygon and the
    true curve differ by far less than a pixel at any size this is rasterised to.
    """
    r = SQUIRCLE_RADIUS * side
    n = SQUIRCLE_EXPONENT

    def corner(cx: float, cy: float, sx: float, sy: float) -> list[tuple[float, float]]:
        """One quarter, centred on (cx,cy), heading in the (sx,sy) quadrant."""
        pts = []
        for i in range(SQUIRCLE_STEPS + 1):
            t = i / SQUIRCLE_STEPS
            # |u|^n + |v|^n = 1, walked from (1,0) to (0,1).
            u = (1.0 - t**n) ** (1.0 / n)
            v = t
            pts.append((cx + sx * r * u, cy + sy * r * v))
        return pts

    pts: list[tuple[float, float]] = []
    pts += corner(side - r, r, 1, -1)        # right edge -> top edge  (top-right)
    pts += corner(r, r, -1, -1)[::-1]        # top -> left             (top-left)
    pts += corner(r, side - r, -1, 1)        # left -> bottom          (bottom-left)
    pts += corner(side - r, side - r, 1, 1)[::-1]   # bottom -> right  (bottom-right)

    d = f"M{pts[0][0]:.2f} {pts[0][1]:.2f}"
    d += "".join(f"L{x:.2f} {y:.2f}" for x, y in pts[1:])
    return d + "Z"


def load() -> tuple[np.ndarray, np.ndarray]:
    """The artwork, cropped to its ink, plus a per-pixel ink coverage in [0,1]."""
    im = Image.open(SRC)
    rgb = np.array(im.convert("RGB")).astype(np.float32)
    if "A" in im.getbands():
        alpha = np.array(im.convert("RGBA"))[..., 3].astype(np.float32) / 255.0
        rgb = rgb * alpha[..., None] + 255.0 * (1 - alpha[..., None])
    # Distance from the white ground, normalised per pixel by how far its own colour is
    # from white. That recovers the anti-aliased edge as partial coverage instead of
    # rounding it to on/off.
    white = np.array(BACKGROUND, np.float32)
    ink = np.linalg.norm(rgb - white, axis=-1) > 26
    ys, xs = np.nonzero(ink)
    if not len(ys):
        sys.exit(f"emblem.py: {SRC.name} is blank")
    y0, y1, x0, x1 = ys.min(), ys.max() + 1, xs.min(), xs.max() + 1
    return rgb[y0:y1, x0:x1], ink[y0:y1, x0:x1]


def coverage(rgb: np.ndarray, ink: np.ndarray, name: str) -> np.ndarray:
    white = np.array(BACKGROUND, np.float32)
    target = np.array(PALETTE[name], np.float32)
    d = np.stack([np.linalg.norm(rgb - np.array(PALETTE[k], np.float32), axis=-1) for k in ORDER], -1)
    mine = (np.argmin(d, axis=-1) == ORDER.index(name)) & ink
    # How far this pixel travelled from white towards its colour: 1 inside, a fraction on
    # an anti-aliased edge.
    frac = np.linalg.norm(rgb - white, axis=-1) / max(np.linalg.norm(target - white), 1e-6)
    return np.clip(frac, 0.0, 1.0) * mine


def trace(cov: np.ndarray) -> list[np.ndarray]:
    h, w = cov.shape
    big = cv2.resize(cov, (w * UP, h * UP), interpolation=cv2.INTER_CUBIC)
    mask = (big > 0.5).astype(np.uint8)
    mask = cv2.morphologyEx(mask, cv2.MORPH_OPEN, np.ones((3, 3), np.uint8))
    contours, _ = cv2.findContours(mask, cv2.RETR_CCOMP, cv2.CHAIN_APPROX_SIMPLE)
    out = []
    for c in contours:
        if cv2.contourArea(c) < (UP * UP) * MIN_AREA:
            continue
        p = cv2.approxPolyDP(c, EPS, True)
        if len(p) >= 3:
            out.append(p.reshape(-1, 2).astype(np.float32) / UP)
    return out


def path_d(polys: list[np.ndarray], scale: float = 1.0, ox: float = 0.0, oy: float = 0.0) -> str:
    parts = []
    for p in polys:
        pts = [(x * scale + ox, y * scale + oy) for x, y in p]
        seg = f"M{pts[0][0]:.2f} {pts[0][1]:.2f}"
        seg += "".join(f"L{x:.2f} {y:.2f}" for x, y in pts[1:])
        parts.append(seg + "Z")
    return "".join(parts)


def hexof(name: str) -> str:
    return "#%02x%02x%02x" % PALETTE[name]


def write_icon(layers: dict[str, list[np.ndarray]], w: int, h: int) -> None:
    scale = SIDE * GLYPH_HEIGHT / h
    ox = (SIDE - w * scale) / 2
    oy = (SIDE - h * scale) / 2
    paths = "\n  ".join(
        f'<path fill="{hexof(k)}" fill-rule="evenodd" d="{path_d(layers[k], scale, ox, oy)}"/>'
        for k in ORDER)
    ICON.write_text(f"""<svg xmlns="http://www.w3.org/2000/svg" width="{SIDE}" height="{SIDE}" viewBox="0 0 {SIDE} {SIDE}">
  <!-- GENERATED by scripts/emblem.py — do not edit by hand. Rasterise to assets/icon.png
       and src-tauri/icons/icon.ico with scripts/icon.py.

       The tile is WHITE: the emblem's own upper wings are navy, and a mark that shares its
       background's colour is a mark with a hole in it.

       The silhouette is the macOS app-icon squircle — a superellipse of exponent
       {SQUIRCLE_EXPONENT}, corner {SQUIRCLE_RADIUS:.4f} of the side — measured off a real
       AppIcon.icns rather than quoted. Full-bleed here so the Clatch shelf stays even;
       clappkit scales it to 0.80 for the Dock, which lands on the macOS 824/1024 grid.

       The mark is Gebze Teknik Üniversitesi's; this app is not affiliated with or endorsed
       by the university. See THIRD_PARTY_NOTICES.md. -->
  <path fill="#ffffff" d="{squircle_path(SIDE)}"/>
  {paths}
</svg>
""", encoding="utf-8")


def write_component(layers: dict[str, list[np.ndarray]], w: int, h: int) -> None:
    paths = "\n      ".join(
        f'<path fill="var(--emblem-{k})" fillRule="evenodd" d="{path_d(layers[k])}" />'
        for k in ORDER)
    COMPONENT.write_text(f"""// GENERATED by scripts/emblem.py — do not edit by hand.
//
// The university's own emblem, traced from the published mark rather than approximated by
// hand (docs/ICONS.md rule 5).
//
// The fills are custom properties rather than literals for exactly one reason: the mark's
// navy is #1a3476, and on a dark background that is a contrast ratio of 1.6:1 — the upper
// wings do not read, they vanish, and half the butterfly goes with them. styles.css holds
// the real colours in light mode and lifts only the navy in dark mode. That is the
// smallest change that keeps the mark legible; the alternative was standing it on a white
// plate, which puts a box in the middle of the interface.
//
// The wordmark is not here on purpose — "GEBZE TEKNİK ÜNİVERSİTESİ" set beside this app's
// name would read as the university publishing it, and it does not.

/** Natural size of the traced artwork, and therefore its aspect. */
const W = {w};
const H = {h};

export function Emblem({{ size = 26, muted }}: {{ size?: number; muted?: boolean }}) {{
  return (
    <svg
      className={{`emblem ${{muted ? "muted" : ""}}`}}
      width={{(size * W) / H}}
      height={{size}}
      viewBox="0 0 {w} {h}"
      role="img"
      aria-label="Gebze Teknik Üniversitesi"
    >
      {paths}
    </svg>
  );
}}
""", encoding="utf-8")


def main() -> int:
    if not SRC.is_file():
        sys.exit(f"emblem.py: {SRC} is missing")
    rgb, ink = load()
    h, w = ink.shape
    print(f"  source {SRC.name}  ink {w}x{h}")
    layers = {}
    for name in ORDER:
        layers[name] = trace(coverage(rgb, ink, name))
        print(f"  {name:8} {len(layers[name]):3d} contours, "
              f"{sum(len(p) for p in layers[name]):5d} points")
    write_icon(layers, w, h)
    write_component(layers, w, h)
    print(f"  {ICON.relative_to(ROOT)}       {ICON.stat().st_size / 1024:.1f} KiB")
    print(f"  {COMPONENT.relative_to(ROOT)}       {COMPONENT.stat().st_size / 1024:.1f} KiB")
    print("\n  next: python scripts/icon.py   (rasterise to PNG + ICO)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
