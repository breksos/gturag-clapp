#!/usr/bin/env python3
"""Draw assets/banner.png — the library's detail-page hero strip.

    python scripts/banner.py

The bounds are the protocol's (clappkit docs/format.md § Picture limits): PNG at 215:32,
at least 3440x512, under 2 MiB. Everything else here comes from docs/icons.md § "The
banner is a strip, not a picture", and every one of its rules is load-bearing:

* **It is drawn 128px tall.** That is a 0.25 scale, so a 4px hairline at full size is one
  pixel in the library and effectively gone. Nothing here is thinner than MIN_STROKE, and
  the text-line rules inside the cards — the finest thing in the picture — are 18px, which
  lands at 4.5px where anyone actually sees it.

* **The left 40% is not yours.** The launcher lays a left-dark scrim over it and prints
  the app's name in white on top. So the motif starts at MOTIF_X0, and everything left of
  it is flat ground: nothing to fight the one thing that has to be read.

* **Draw what the app DOES.** Not the emblem again — the icon already said whose this is,
  and a logo at banner size tells a person scrolling a shelf nothing new. What this app
  does is find ONE document in a registry of 1819, so that is the picture: a row of
  documents, and one of them pulled up, filled, and marked. The row runs off the right
  edge on purpose; a registry does not end where the strip does.

* **Use the icon's own colours.** Sampled from the emblem, not picked: navy #1a3476,
  crimson #cd1239, orange #f58612. The banner and the icon sit inches apart in the
  library, and a second palette makes them look like two products.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "assets" / "banner.png"

W, H = 3440, 512          # 215:32, the format's minimum resolution and its design ratio
RENDER_SCALE = 128 / H    # what the library actually draws it at
MIN_STROKE = 6            # docs/icons.md: nothing thinner, measured at full size

# The emblem's own three.
NAVY = (0x1A, 0x34, 0x76)
CRIMSON = (0xCD, 0x12, 0x39)
ORANGE = (0xF5, 0x86, 0x12)
PAPER = (0xFF, 0xFF, 0xFF)
# One step off the ground, for the rules inside a card: at 0.25 scale a mid-tone reads as
# texture where full contrast would read as stripes.
RULE = (0xA8, 0xB6, 0xD8)

MOTIF_X0 = 1764           # 51% — RIGHT of centre, which is what icons.md asks for, and
                          # clear of the scrim's 40% by a margin the crop cannot eat
CARD_W, CARD_GAP = 200, 60
PITCH = CARD_W + CARD_GAP
BASELINE = 392            # every card sits ON this line, like documents on a shelf

# Heights of the ordinary cards, cycled. Uneven on purpose: a registry is not a barcode.
HEIGHTS = [236, 268, 214, 252, 226, 260, 220, 244]
# Which card is the answer. Kept near the start of the row on purpose: the strip is
# cover-cropped from the sides on a narrow window, and the one thing that must survive
# that crop is the thing the picture is about.
FOUND = 1
FOUND_H, FOUND_LIFT = 330, 34


def card(d: ImageDraw.ImageDraw, x: int, h: int, *, found: bool) -> None:
    """One document. A filled block, a header band, and rules standing in for text."""
    top = BASELINE - h - (FOUND_LIFT if found else 0)
    bottom = BASELINE - (FOUND_LIFT if found else 0)
    body = ORANGE if found else PAPER
    d.rectangle([x, top, x + CARD_W, bottom], fill=body)

    # The header band: a document's code strip. Crimson on the found one so the answer
    # carries two of the three colours and the row carries one.
    band = 34
    d.rectangle([x, top, x + CARD_W, top + band], fill=CRIMSON if found else NAVY)

    # Text rules. 18px tall — 4.5px where the library draws this, which is texture rather
    # than stripes, and still three times the ceiling the format's minimum would allow.
    line_h, line_gap = 18, 26
    y = top + band + 32
    ink = NAVY if found else RULE
    widths = (0.72, 0.86, 0.55, 0.80, 0.62)
    i = 0
    while y + line_h < bottom - 20 and i < len(widths):
        d.rectangle([x + 24, y, x + 24 + int((CARD_W - 48) * widths[i]), y + line_h], fill=ink)
        y += line_h + line_gap
        i += 1


def main() -> int:
    im = Image.new("RGB", (W, H), NAVY)
    d = ImageDraw.Draw(im)

    # The shelf the documents stand on. 10px: visible at 0.25 scale, quiet at full size.
    d.rectangle([MOTIF_X0 - 40, BASELINE, W, BASELINE + 10], fill=RULE)

    x = MOTIF_X0
    i = 0
    while x < W + CARD_W:                     # deliberately past the edge — see the module docstring
        found = i == FOUND
        card(d, x, FOUND_H if found else HEIGHTS[i % len(HEIGHTS)], found=found)
        x += PITCH
        i += 1

    # The marker under the answer: the same orange rule the window puts down the left of an
    # open result, so the two surfaces say "this one" the same way.
    fx = MOTIF_X0 + FOUND * PITCH
    d.rectangle([fx, BASELINE + 34, fx + CARD_W, BASELINE + 34 + 16], fill=ORANGE)

    im.save(DEST, "PNG", optimize=True)

    kib = DEST.stat().st_size / 1024
    print(f"  {DEST.relative_to(ROOT)}  {W}x{H}  ({W / H:.3f}:1, want {215 / 32:.3f})  {kib:.0f} KiB")
    print(f"  drawn at {int(W * RENDER_SCALE)}x{int(H * RENDER_SCALE)} in the library; "
          f"finest stroke 18px -> {18 * RENDER_SCALE:.1f}px")
    print(f"  motif starts at {100 * MOTIF_X0 // W}% — the scrim owns the left 40%")
    if kib > 2048:
        sys.exit("banner.py: over the 2 MiB ceiling")
    if abs(W / H - 215 / 32) > 1e-6:
        sys.exit("banner.py: aspect is not 215:32")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
