#!/usr/bin/env python3
"""Turn the brand text into outlined paths and write `wordmark.json`.

The logo SVGs must not depend on an installed font, so `build.py` reads the glyph outlines from
`wordmark.json` instead of from a font. Run this script only when the wordmark, the `pel` file-icon
label or the tagline changes. It needs `pip install fonttools` and the two font files (both SIL Open
Font License, from https://github.com/google/fonts):

    python outline_fonts.py --comfortaa Comfortaa[wght].ttf --outfit Outfit[wght].ttf

Kerning: the pairs below were measured in Chromium (HarfBuzz) at 1000px with `font-kerning: normal`
against `none`; `l` followed by `a` pulls in by 30 units (of 1000). Extend KERN if the wordmark changes.
"""
import argparse
import json
import os

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

KERN = {("l", "a"): -30}
TAGLINE = "A self-hosting systems language."


def outline(path, weight, text, kern=None):
    font = instancer.instantiateVariableFont(TTFont(path), {"wght": weight})
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    x = 0
    parts = []
    prev = None
    for ch in text:
        if kern and (prev, ch) in kern:
            x += kern[(prev, ch)]
        name = cmap[ord(ch)]
        if ch != " ":
            pen = SVGPathPen(glyphs, ntos=lambda v: ("%.1f" % v).rstrip("0").rstrip("."))
            glyphs[name].draw(TransformPen(pen, (1, 0, 0, -1, x, 0)))
            parts.append(pen.getCommands())
        x += font["hmtx"][name][0]
        prev = ch
    return {"d": "".join(parts), "adv": x, "upm": font["head"].unitsPerEm}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--comfortaa", required=True)
    ap.add_argument("--outfit", required=True)
    args = ap.parse_args()
    data = {
        "pelago": outline(args.comfortaa, 700, "pelago", KERN),
        "pel": outline(args.outfit, 700, "pel"),
        "tagline": dict(outline(args.outfit, 400, TAGLINE), text=TAGLINE),
    }
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wordmark.json")
    with open(out, "w") as f:
        json.dump(data, f, separators=(",", ":"))
    print("wrote", out)


if __name__ == "__main__":
    main()
