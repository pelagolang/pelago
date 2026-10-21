#!/usr/bin/env python3
"""Build every Pelago brand asset from the drawings in this file.

    python assets/brand/build/build.py            # SVGs, sprite, PNGs, ICO, ICNS, social images
    python assets/brand/build/build.py --svg-only # only the SVG files and the sprite (no browser needed)

Needs Python 3.8+ and, for the PNG files, a Chromium-based browser (Chrome, Chromium or Edge). It
finds the browser itself; set PELAGO_CHROME to its path to override. Nothing else is required: the
PNG crop, the ICO and the ICNS are written by this script, and the wordmark outlines come from
`wordmark.json` (made by `outline_fonts.py`, which is only needed when the wordmark or tagline
changes).

The SVG files are the source of truth. Everything else is an export of them. Running the script
rewrites the files under `assets/brand/` in place. The SVGs come out byte-identical; the PNGs can
differ in a few bytes between browser versions, so judge them by eye, not by `git diff`.
"""
import argparse
import base64
import glob
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
BRAND = HERE.parent

# Palette (docs/design/brand.md)
TEAL = "#0E7C86"
LAGOON = "#5FD0C8"
DEEP = "#0B2239"
PAGE_DARK = "#16222d"
WHITE = "#ffffff"

WORD = json.load(open(HERE / "wordmark.json"))
SVG_OPEN = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="%s">'


# ---------------------------------------------------------------- drawings

def mark_body(fg, wave_opacity=0.55, wave=None):
    """The mark: monoline P, the dot, the wave. 128 x 128 units."""
    wave = wave or fg
    return (
        f'<path d="M32 85V25H60a21 21 0 0 1 0 42H32" fill="none" stroke="{fg}" stroke-width="14" '
        f'stroke-linecap="round" stroke-linejoin="round"/>'
        f'<circle cx="102" cy="85" r="8" fill="{fg}"/>'
        f'<path d="M28 116q8-8 16 0t16 0 16 0 16 0" fill="none" stroke="{wave}" stroke-width="7" '
        f'stroke-linecap="round" opacity="{wave_opacity}"/>'
    )


def text_path(key, fg, x, baseline, size):
    """An outlined text run from wordmark.json, `size` units high (the font size)."""
    w = WORD[key]
    s = size / w["upm"]
    return (
        f'<path transform="translate({x:.2f} {baseline:.2f}) scale({s:.5f})" fill="{fg}" d="{w["d"]}"/>',
        w["adv"] * s,
    )


NAME_SIZE = 84.5  # font size of the name next to the mark, in mark units
NAME_X = 130
NAME_BASE = 88

COLOURS = {  # file suffix: (colour, wave opacity)
    "": (TEAL, 0.55),
    "-dark": (LAGOON, 0.55),
    "-black": ("#000000", 1),
    "-white": (WHITE, 1),
}


def lockups():
    out = {}
    for suf, (fg, wo) in COLOURS.items():
        out[f"logo/pelago-mark{suf}.svg"] = SVG_OPEN % "0 0 128 128" + mark_body(fg, wo) + "</svg>"
        name, adv = text_path("pelago", fg, NAME_X, NAME_BASE, NAME_SIZE)
        out[f"logo/pelago-horizontal{suf}.svg"] = (
            SVG_OPEN % f"0 0 {NAME_X + adv + 6:.0f} 128" + mark_body(fg, wo) + name + "</svg>"
        )
        size = 54
        sw = WORD["pelago"]["adv"] * size / WORD["pelago"]["upm"]
        width = max(sw, 128) + 24
        name, _ = text_path("pelago", fg, (width - sw) / 2, 180, size)
        out[f"logo/pelago-stacked{suf}.svg"] = (
            SVG_OPEN % f"0 0 {width:.0f} 214"
            + f'<g transform="translate({(width - 128) / 2:.1f} 0)">{mark_body(fg, wo)}</g>'
            + name
            + "</svg>"
        )
    return out


def badge(bg, fg, wave_opacity=0.55, radius=28, scale=0.8, flat=False):
    o = (128 - 128 * scale) / 2
    rx = "" if flat else f' rx="{radius}"'
    return (
        f'<rect width="128" height="128"{rx} fill="{bg}"/>'
        f'<g transform="translate({o + 0.5:.1f} {o + 2:.1f}) scale({scale})">{mark_body(fg, wave_opacity)}</g>'
    )


def icons():
    box = "0 0 128 128"
    clip = '<clipPath id="pelago-avatar-clip"><circle cx="64" cy="64" r="64"/></clipPath><g clip-path="url(#pelago-avatar-clip)">'
    return {
        "icon/pelago-icon.svg": SVG_OPEN % box + badge(TEAL, WHITE, 0.6) + "</svg>",
        "icon/pelago-icon-dark.svg": SVG_OPEN % box + badge(DEEP, LAGOON) + "</svg>",
        "icon/favicon.svg": SVG_OPEN % box + badge(TEAL, WHITE, 0.6) + "</svg>",
        "icon/apple-touch-icon.svg": SVG_OPEN % box + badge(TEAL, WHITE, 0.6, flat=True, scale=0.74) + "</svg>",
        "icon/maskable.svg": SVG_OPEN % box + badge(TEAL, WHITE, 0.6, flat=True, scale=0.6) + "</svg>",
        "icon/avatar.svg": SVG_OPEN % box + clip + badge(TEAL, WHITE, 0.6, flat=True, scale=0.7) + "</g></svg>",
        "icon/avatar-dark.svg": SVG_OPEN % box + clip + badge(DEEP, LAGOON, flat=True, scale=0.7) + "</g></svg>",
    }


PAGE = "M26 8h52l26 26v86H26z"


def small_mark(fg, tx, ty, scale, wave=None):
    return f'<g transform="translate({tx} {ty}) scale({scale})">{mark_body(fg, 0.55, wave)}</g>'


def fold(colour, opacity=0.35):
    return f'<path d="M78 8v26h26" fill="{colour}" opacity="{opacity}"/>'


def pal(dark):
    return {"fg": LAGOON if dark else TEAL, "page": PAGE_DARK if dark else WHITE, "ink": LAGOON if dark else DEEP}


def pel_icon(style, dark):
    p = pal(dark)
    fg = p["fg"]
    outline = f'<path d="{PAGE}" fill="{p["page"]}" stroke="{fg}" stroke-width="7" stroke-linejoin="round"/>'
    if style == "outline":
        return outline + fold(fg) + small_mark(fg, 31, 38, 0.56)
    if style == "glyph":
        return mark_body(fg)
    if style == "solid":
        mc = DEEP if dark else WHITE
        return (
            f'<path d="{PAGE}" fill="{fg}" stroke="{fg}" stroke-width="4" stroke-linejoin="round"/>'
            f'<path d="M78 8v26h26" fill="#000" opacity=".22"/>' + small_mark(mc, 31, 38, 0.56, wave=mc)
        )
    if style == "band":
        tc = DEEP if dark else WHITE
        label, adv = text_path("pel", tc, 0, 113, 25)
        label = label.replace('translate(0.00 113.00)', f"translate({65 - adv / 2:.2f} 113.00)")
        return outline + fold(fg) + small_mark(fg, 36, 18, 0.48) + f'<rect x="26" y="88" width="78" height="32" fill="{fg}"/>' + label
    raise ValueError(style)


def waves(colour, x0, ys):
    return "".join(
        f'<path d="M{x0} {y}q7-7 14 0t14 0 14 0 14 0" fill="none" stroke="{colour}" stroke-width="7" stroke-linecap="round"/>'
        for y in ys
    )


def toml_icon(style, dark):
    p = pal(dark)
    fg, ink = p["fg"], p["ink"]
    if style == "chart":
        return (
            f'<path d="{PAGE}" fill="{p["page"]}" stroke="{ink}" stroke-width="7" stroke-linejoin="round"/>'
            + fold(ink) + waves(fg, 38, (56, 74)) + f'<circle cx="82" cy="94" r="7" fill="{fg}"/>'
        )
    if style == "solid":
        bg = LAGOON if dark else DEEP
        c1 = DEEP if dark else LAGOON
        return (
            f'<path d="{PAGE}" fill="{bg}" stroke="{bg}" stroke-width="4" stroke-linejoin="round"/>'
            f'<path d="M78 8v26h26" fill="#000" opacity=".25"/>' + waves(c1, 38, (58, 76)) + f'<circle cx="82" cy="96" r="7" fill="{c1}"/>'
        )
    if style == "anchor":
        return (
            f'<g fill="none" stroke="{fg}" stroke-width="10" stroke-linecap="round" stroke-linejoin="round">'
            '<circle cx="64" cy="24" r="10"/><path d="M64 34V108M42 56H86M24 78C28 104 52 114 64 114S100 104 104 78"/></g>'
        )
    raise ValueError(style)


PEL_STYLES = ("outline", "glyph", "solid", "band")
TOML_STYLES = ("chart", "solid", "anchor")


def file_icons():
    out = {}
    for dark, suf in ((False, ""), (True, "-dark")):
        for s in PEL_STYLES:
            out[f"editor/file-icons/pel-{s}{suf}.svg"] = SVG_OPEN % "0 0 128 128" + pel_icon(s, dark) + "</svg>"
        for s in TOML_STYLES:
            out[f"editor/file-icons/pelago-toml-{s}{suf}.svg"] = SVG_OPEN % "0 0 128 128" + toml_icon(s, dark) + "</svg>"
    return out


def social_svg(width, height, dark, tagline=None):
    """`tagline` is a key of wordmark.json (`tagline` or `tagline2`), or None for no tagline."""
    bg = DEEP if dark else WHITE
    fg = LAGOON if dark else TEAL
    name_colour = WHITE if dark else DEEP
    sub = "#9fb6c9" if dark else "#58708a"
    wave_lines = "".join(
        f'<path d="M0 {y}q25-20 50 0t50 0 50 0 50 0 50 0 50 0 50 0 50 0" fill="none" stroke="{fg}" stroke-width="3" opacity="{o}"/>'
        for y, o in ((30, 0.14), (60, 0.07), (90, 0.045))
    )
    m = height * 0.42  # lockup height
    name, adv = text_path("pelago", name_colour, NAME_X, NAME_BASE, NAME_SIZE)
    lock_w = (NAME_X + adv + 6) * m / 128
    tag_size = height * 0.075
    gap = height * 0.04
    block = m + (gap + tag_size if tagline else 0)
    top = (height - block) / 2
    body = (
        f'<svg x="{(width - lock_w) / 2:.1f}" y="{top:.1f}" width="{lock_w:.1f}" height="{m:.1f}" '
        f'viewBox="0 0 {NAME_X + adv + 6:.0f} 128">{mark_body(fg)}{name}</svg>'
    )
    if tagline:
        tag, tw = text_path(tagline, sub, 0, 0, tag_size)
        x = (width - tw) / 2
        tag = tag.replace('translate(0.00 0.00)', f"translate({x:.2f} {top + m + gap + tag_size * 0.78:.2f})")
        body += tag
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}">'
        f'<rect width="{width}" height="{height}" fill="{bg}"/>'
        f'<svg x="0" y="{height * 0.64:.1f}" width="{width}" height="{height * 0.36:.1f}" viewBox="0 0 400 100" preserveAspectRatio="none">{wave_lines}</svg>'
        + body + "</svg>"
    )


SOCIAL = (  # name, width, height, dark, has a tagline
    ("github-social-1280x640", 1280, 640, True, True),
    ("og-1200x630", 1200, 630, False, True),
    ("x-header-1500x500", 1500, 500, True, True),
    ("linkedin-1584x396", 1584, 396, False, True),
    ("readme-banner-1280x320", 1280, 320, True, False),
)
# Both taglines are kept, each on its own set of images: the plain name carries "A self-hosting systems
# language.", the `-native` file carries "Native code. No GC. No runtime."
TAGLINES = (("", "tagline"), ("-native", "tagline2"))


def social_files():
    """(file stem, width, height, dark, tagline key) for every social image."""
    out = []
    for n, w, h, d, t in SOCIAL:
        for suffix, key in TAGLINES if t else (("", None),):
            out.append((n + suffix, w, h, d, key))
    return out


def social():
    return {f"social/{n}.svg": social_svg(w, h, d, key) for n, w, h, d, key in social_files()}


def sprite(svgs):
    """One SVG with a <symbol> per logo, icon and file icon: `<use href="sprite.svg#pelago-icon"/>`."""
    order = [k for k in svgs if k.startswith(("logo/", "icon/pelago-icon", "editor/file-icons/"))]
    symbols = []
    for key in order:
        src = svgs[key]
        vb = src.split('viewBox="')[1].split('"')[0]
        inner = src.split(">", 1)[1].rsplit("</svg>", 1)[0]
        symbols.append(f'<symbol id="{Path(key).stem}" viewBox="{vb}">{inner}</symbol>')
    return '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">' + "".join(symbols) + "</svg>"


# ---------------------------------------------------------------- PNG, ICO, ICNS

def find_chrome():
    env = os.environ.get("PELAGO_CHROME")
    if env:
        return env
    for name in ("google-chrome", "chromium", "chromium-browser", "chrome", "msedge"):
        found = shutil.which(name)
        if found:
            return found
    candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] + glob.glob("/opt/pw-browsers/chromium-*/chrome-linux/chrome")
    for c in candidates:
        if os.path.exists(c):
            return c
    sys.exit("No Chrome, Chromium or Edge found. Set PELAGO_CHROME to the browser executable, or use --svg-only.")


def read_png(path):
    data = Path(path).read_bytes()
    pos, idat, header = 8, b"", None
    while pos < len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", chunk)
        elif kind == b"IDAT":
            idat += chunk
        pos += 12 + n
    w, h, depth, ctype, _, _, interlace = header
    assert depth == 8 and ctype in (2, 6) and interlace == 0, "unsupported PNG from the browser"
    bpp = 4 if ctype == 6 else 3
    raw = zlib.decompress(idat)
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        ft = raw[y * (stride + 1)]
        row = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = row[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if ft == 1:
                row[i] = (row[i] + a) & 255
            elif ft == 2:
                row[i] = (row[i] + b) & 255
            elif ft == 3:
                row[i] = (row[i] + (a + b) // 2) & 255
            elif ft == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                row[i] = (row[i] + pr) & 255
        rows.append(row)
        prev = row
    return w, h, bpp, rows


def write_png(path, w, h, bpp, rows):
    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload))

    raw = b"".join(b"\x00" + bytes(r) for r in rows)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6 if bpp == 4 else 2, 0, 0, 0)
    Path(path).write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def crop_png(path, width, height):
    w, h, bpp, rows = read_png(path)
    write_png(path, width, height, bpp, [r[: width * bpp] for r in rows[:height]])


def svg_size(path):
    vb = Path(path).read_text().split('viewBox="')[1].split('"')[0].split()
    return float(vb[2]), float(vb[3])


def export_png(chrome, svg, out, width, transparent=True):
    vw, vh = svg_size(svg)
    height = round(width * vh / vw)
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "page.html"
        page.write_text(
            f'<html><body style="margin:0"><img src="{Path(svg).resolve().as_uri()}" width="{width}" height="{height}" style="display:block"></body></html>'
        )
        cmd = [
            chrome, "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
            f"--user-data-dir={Path(tmp) / 'profile'}",
            "--default-background-color=" + ("00000000" if transparent else "ffffffff"),
            f"--screenshot={out}", f"--window-size={width},{height + 150}", page.resolve().as_uri(),
        ]
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    crop_png(out, width, height)


def write_ico(path, pngs):
    """A .ico that stores one PNG per size (supported since Windows Vista)."""
    entries, blobs, offset = b"", b"", 6 + 16 * len(pngs)
    for size, png in pngs:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    Path(path).write_bytes(struct.pack("<HHH", 0, 1, len(pngs)) + entries + blobs)


ICNS_TYPES = (  # PNG-compressed icon types: (type, pixel size)
    ("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128), ("ic13", 256), ("ic08", 256),
    ("ic14", 512), ("ic09", 512), ("ic10", 1024), ("ic11", 32), ("ic12", 64),
)


def write_icns(path, png_for_size):
    parts = b""
    for kind, size in ICNS_TYPES:
        png = png_for_size[size]
        parts += kind.encode() + struct.pack(">I", len(png) + 8) + png
    Path(path).write_bytes(b"icns" + struct.pack(">I", len(parts) + 8) + parts)


# ---------------------------------------------------------------- design specs

INK = "#0B2239"
GUIDE = "#e0457b"   # construction lines
GRID = "#c9d6df"
FONT = "Outfit, sans-serif"


def label(x, y, text, size=13, anchor="start", fill=INK, weight=400):
    return (
        f'<text x="{x:.1f}" y="{y:.1f}" font-family="{FONT}" font-size="{size}" font-weight="{weight}" '
        f'text-anchor="{anchor}" fill="{fill}">{text}</text>'
    )


def line(x1, y1, x2, y2, colour=GUIDE, w=1.5, dash=None):
    d = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="{colour}" stroke-width="{w}"{d}/>'


def dim(x1, y1, x2, y2, text, side=1, size=13):
    """A dimension line with end ticks and a label (pixel coordinates). side picks the label side."""
    horizontal = abs(y1 - y2) < 0.01
    out = line(x1, y1, x2, y2, GUIDE, 1.5)
    if horizontal:
        out += line(x1, y1 - 5, x1, y1 + 5) + line(x2, y2 - 5, x2, y2 + 5)
        out += label((x1 + x2) / 2, y1 + side * 18, text, size, "middle", GUIDE, 600)
    else:
        out += line(x1 - 5, y1, x1 + 5, y1) + line(x2 - 5, y2, x2 + 5, y2)
        out += label(x1 + side * 10, (y1 + y2) / 2 + 5, text, size, "start" if side > 0 else "end", GUIDE, 600)
    return out


def grid(ox, oy, S, n=128, step=8):
    out = ""
    for i in range(0, n + 1, step):
        major = i % 32 == 0
        c, w = ("#9fb3c1", 1.2) if major else (GRID, 0.8)
        out += line(ox + i * S, oy, ox + i * S, oy + n * S, c, w) + line(ox, oy + i * S, ox + n * S, oy + i * S, c, w)
    return out


def font_face():
    """Outfit, embedded so the spec sheets look the same on every machine (SIL OFL, assets/brand/fonts)."""
    data = base64.b64encode((BRAND / "fonts/Outfit-VariableFont_wght.ttf").read_bytes()).decode()
    return (
        "<style>@font-face{font-family:Outfit;font-weight:100 900;"
        f"src:url(data:font/ttf;base64,{data}) format('truetype')}}</style>"
    )


def spec_head(width, title, sub, height=900):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}">'
        + font_face() +
        f'<rect width="{width}" height="{height}" fill="#ffffff"/>'
        + label(48, 62, title, 30, weight=700)
        + label(48, 90, sub, 15, fill="#58708a")
    )


def spec_construction():
    S, ox, oy = 5.6, 70, 150
    px = lambda u: ox + u * S
    py = lambda u: oy + u * S
    body = grid(ox, oy, S)
    # the mark, drawn translucent so the guides show through
    body += f'<g transform="translate({ox} {oy}) scale({S})" opacity="0.88">{mark_body(TEAL, 0.55)}</g>'
    # centre lines of the P stroke and the baseline / cap line
    body += line(px(32), py(10), px(32), py(100), GUIDE, 1.2, "5 4")
    body += line(px(10), py(25), px(100), py(25), GUIDE, 1.2, "5 4")
    body += line(px(10), py(85), px(116), py(85), GUIDE, 1.2, "5 4")
    body += line(px(10), py(67), px(100), py(67), GUIDE, 1.2, "5 4")
    # the bowl: centre-line circle, outer and inner edges
    cx, cy = px(60), py(46)
    for r, d in ((21, None), (28, "2 4"), (14, "2 4")):
        body += f'<circle cx="{cx}" cy="{cy}" r="{r * S}" fill="none" stroke="{GUIDE}" stroke-width="1.2"' + (f' stroke-dasharray="{d}"' if d else "") + "/>"
    body += f'<circle cx="{cx}" cy="{cy}" r="3" fill="{GUIDE}"/>'
    body += f'<circle cx="{px(102)}" cy="{py(85)}" r="{8 * S}" fill="none" stroke="{GUIDE}" stroke-width="1.2" stroke-dasharray="2 4"/>'
    # measurements
    body += dim(px(25), py(104), px(39), py(104), "14 stem", -1)
    body += dim(px(94), py(100), px(110), py(100), "16 dot", 1)
    body += dim(px(28), py(122), px(92), py(122), "wave 64 = 4 half-waves of 16", 1)
    body += dim(px(25), py(5), px(88), py(5), "63 wide", -1)
    body += dim(px(118), py(18), px(118), py(92), "74 tall", 1)
    body += label(px(60), py(46) - 6, "r 21", 13, "middle", GUIDE, 700) 
    body += label(px(60) + 28 * S + 8, py(46) - 28 * S + 4, "outer 28", 12, "start", GUIDE)
    right = 70 + 128 * S + 120
    notes = [
        ("Unit", "The mark is built on a 128 x 128 grid. One stroke is 14 units."),
        ("P", "A monoline capital: stem 14, bowl centre-line radius 21 (1.5 strokes), round caps and joins."),
        ("Dot", "Radius 8 (a little more than half a stroke), sits on the baseline at x = 102."),
        ("Wave", "Four half-waves, 7 thick (half a stroke), 16 long, 55 % opacity, 31 below the baseline."),
        ("Colour", "Teal #0E7C86 on light. Lagoon #5FD0C8 on dark. White or black in one colour."),
    ]
    y = 190
    for head, text in notes:
        body += label(right, y, head, 15, weight=700, fill=TEAL)
        words, line_, ly = text.split(), "", y + 22
        for w in words:
            if len(line_ + w) > 38:
                body += label(right, ly, line_, 14, fill=INK)
                line_, ly = "", ly + 20
            line_ += w + " "
        body += label(right, ly, line_, 14, fill=INK)
        y = ly + 44
    width = right + 360
    return spec_head(width, "Mark construction", "Grid of 128 units. Pink lines are construction lines; the mark is drawn over them.") + body + "</svg>"


def spec_icon():
    S, ox, oy = 5.6, 70, 150
    px = lambda u: ox + u * S
    py = lambda u: oy + u * S
    body = f'<g transform="translate({ox} {oy}) scale({S})">{badge(TEAL, WHITE, 0.6)}</g>'
    body += grid(ox, oy, S, step=16)
    # corner radius: the circle that makes the corner
    body += f'<circle cx="{px(28)}" cy="{py(28)}" r="{28 * S}" fill="none" stroke="{GUIDE}" stroke-width="1.5" stroke-dasharray="4 4"/>'
    body += line(px(28), py(0), px(28), py(28), GUIDE, 1.5) + line(px(0), py(28), px(28), py(28), GUIDE, 1.5)
    body += label(px(28) + 8, py(14) + 5, "28", 14, "start", GUIDE, 700)
    # content box: the mark at 80 %
    body += f'<rect x="{px(12.8)}" y="{py(12.8)}" width="{102.4 * S}" height="{102.4 * S}" fill="none" stroke="{LAGOON}" stroke-width="1.8" stroke-dasharray="6 4"/>'
    body += dim(px(0), py(135), px(12.8), py(135), "12.8", 1, 12)
    body += dim(px(12.8), py(135), px(115.2), py(135), "mark at 80 % = 102.4", 1, 12)
    # inscribed circle and centre cross
    body += f'<circle cx="{px(64)}" cy="{py(64)}" r="{64 * S}" fill="none" stroke="{GUIDE}" stroke-width="1" stroke-dasharray="2 5"/>'
    body += line(px(64), py(0), px(64), py(128), GUIDE, 1, "3 5") + line(px(0), py(64), px(128), py(64), GUIDE, 1, "3 5")
    right = 70 + 128 * S + 90
    body += label(right, 190, "App icon", 18, weight=700, fill=TEAL)
    for i, t in enumerate(("Rounded square, corner radius 28 of 128 (21.9 %).",
                           "The mark fills 80 %, nudged half a unit right",
                           "and two down so the wave sits in the optical centre.",
                           "Teal tile, white mark, wave at 60 %.",
                           "Dark: Deep Sea tile, Lagoon mark.")):
        body += label(right, 218 + i * 22, t, 13)
    # variants
    variants = (
        ("Apple touch", badge(TEAL, WHITE, 0.6, flat=True, scale=0.74), "square, mark 74 %, the OS rounds it", False),
        ("Maskable", badge(TEAL, WHITE, 0.6, flat=True, scale=0.6), "mark 60 %, inside the 80 % safe circle", True),
        ("Avatar", '<clipPath id="spec-av"><circle cx="64" cy="64" r="64"/></clipPath><g clip-path="url(#spec-av)">' + badge(TEAL, WHITE, 0.6, flat=True, scale=0.7) + "</g>", "circle crop, mark 70 %", False),
    )
    t, vs = 150, 160
    for i, (name, art, note, safe) in enumerate(variants):
        x, y = right + i * (vs + 60), 400
        body += f'<g transform="translate({x} {y}) scale({vs / 128})">{art}</g>'
        if safe:
            body += f'<circle cx="{x + vs / 2}" cy="{y + vs / 2}" r="{vs * 0.4}" fill="none" stroke="{LAGOON}" stroke-width="1.8" stroke-dasharray="5 4"/>'
        body += label(x, y + vs + 26, name, 14, weight=700)
        for j, part in enumerate(note.split(", ")):
            body += label(x, y + vs + 46 + j * 16, part, 11, fill="#58708a")
    width = right + 3 * (vs + 60)
    return spec_head(width, "App icon construction", "Corner radius, safe area and the three variants. Pink: construction. Dashed lagoon: safe area.", 960) + body + "</svg>"


def spec_lockup():
    k = 3.4
    ox, oy = 150, 300
    name, adv = text_path("pelago", TEAL, NAME_X, NAME_BASE, NAME_SIZE)
    width_u = NAME_X + adv + 6
    x = 28  # clear space: two strokes, also the icon's corner radius
    px = lambda u: ox + u * k
    py = lambda u: oy + u * k
    body = f'<g transform="translate({ox} {oy}) scale({k})">{mark_body(INK, 0.55)}{name.replace(TEAL, INK)}</g>'
    # art box and clear-space box
    body += f'<rect x="{px(0)}" y="{py(0)}" width="{width_u * k}" height="{128 * k}" fill="none" stroke="{GUIDE}" stroke-width="1.2"/>'
    body += f'<rect x="{px(-x)}" y="{py(-x)}" width="{(width_u + 2 * x) * k}" height="{(128 + 2 * x) * k}" fill="none" stroke="{LAGOON}" stroke-width="1.8" stroke-dasharray="7 5"/>'
    for (a, b) in ((-x, -x), (width_u, -x), (-x, 128), (width_u, 128)):
        body += f'<rect x="{px(a)}" y="{py(b)}" width="{x * k}" height="{x * k}" fill="{LAGOON}" opacity="0.25"/>'
    body += dim(px(-x), py(-x) - 20, px(0), py(-x) - 20, "x", -1)
    body += label(px(width_u / 2), py(-x) - 30, "x = 28 units: two strokes, the same as the icon corner radius", 13, "middle", GUIDE, 600)
    # mark to name gap, name height
    body += dim(px(110), py(104), px(NAME_X), py(104), "20", 1, 12)
    body += dim(px(width_u) + 48 + x * k, py(0), px(width_u) + 48 + x * k, py(128), "128", 1)
    # minimum sizes
    y = oy + (128 + x) * k + 70
    body += label(70, y - 18, "Smallest sizes", 18, weight=700, fill=TEAL)
    sizes = ((16, "mark 16 px"), (24, "mark 24 px"), (32, "badge 32 px"), (64, "badge 64 px"))
    cx = 70
    for s, text in sizes:
        art = mark_body(INK, 0.55) if "mark" in text else badge(TEAL, WHITE, 0.6)
        body += f'<svg x="{cx}" y="{y}" width="{s}" height="{s}" viewBox="0 0 128 128">{art}</svg>'
        body += label(cx, y + 64 + 24, text, 12, fill="#58708a")
        cx += max(s, 80) + 40
    lw = 140
    body += f'<svg x="{cx + 20}" y="{y}" width="{lw}" height="{lw * 128 / width_u:.1f}" viewBox="0 0 {width_u:.0f} 128">{mark_body(INK, 0.55)}{name.replace(TEAL, INK)}</svg>'
    body += label(cx + 20, y + 64 + 24, "lockup, 140 px wide is the minimum", 12, fill="#58708a")
    body += label(70, y + 130, "The wave is the first thing to vanish at small sizes. Under 24 px prefer the badge to the bare mark.", 13)
    return spec_head(int(px(width_u) + x * k + 140), "Lockup, clear space and minimum size", "Keep x clear on every side. Pink: artwork bounds. Dashed lagoon: clear space.", int(y + 190)) + body + "</svg>"


def specs():
    return {"spec/mark-construction.svg": spec_construction(), "spec/icon-construction.svg": spec_icon(), "spec/lockup-clear-space.svg": spec_lockup()}


# ---------------------------------------------------------------- main

def write_all(files):
    for rel, text in files.items():
        p = BRAND / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, newline="\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--svg-only", action="store_true", help="write the SVG files and the sprite, skip every export")
    args = ap.parse_args()

    svgs = {}
    for part in (lockups(), icons(), file_icons(), social(), specs()):
        svgs.update(part)
    write_all(svgs)
    write_all({"icon/sprite.svg": sprite(svgs)})
    print(f"{len(svgs) + 1} SVG files written")
    if args.svg_only:
        return

    chrome = find_chrome()
    jobs = []

    def job(src, out, width, transparent=True):
        jobs.append((str(BRAND / src), str(BRAND / out), width, transparent))

    for suf in COLOURS:
        job(f"logo/pelago-mark{suf}.svg", f"logo/pelago-mark{suf}-1024.png", 1024)
        job(f"logo/pelago-horizontal{suf}.svg", f"logo/pelago-horizontal{suf}-2000.png", 2000)
        job(f"logo/pelago-stacked{suf}.svg", f"logo/pelago-stacked{suf}-1200.png", 1200)
    for s in (16, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024):
        job("icon/pelago-icon.svg", f"icon/pelago-icon-{s}.png", s)
    for s in (128, 512):
        job("icon/pelago-icon-dark.svg", f"icon/pelago-icon-dark-{s}.png", s)
    job("icon/apple-touch-icon.svg", "icon/apple-touch-icon-180.png", 180)
    job("icon/apple-touch-icon.svg", "icon/mstile-150.png", 150)
    job("icon/pelago-icon.svg", "icon/android-chrome-192.png", 192)
    job("icon/pelago-icon.svg", "icon/android-chrome-512.png", 512)
    job("icon/maskable.svg", "icon/maskable-512.png", 512)
    job("icon/avatar.svg", "icon/avatar-512.png", 512)
    job("icon/avatar-dark.svg", "icon/avatar-dark-512.png", 512)
    job("icon/pelago-icon.svg", "editor/vscode-icon-256.png", 256)
    for f in sorted(glob.glob(str(BRAND / "editor/file-icons/*.svg"))):
        rel = os.path.relpath(f, BRAND).replace(os.sep, "/")
        job(rel, rel[:-4] + "-128.png", 128)
    for n, w, h, d, key in social_files():
        job(f"social/{n}.svg", f"social/{n}.png", w, transparent=False)
    for f in sorted(glob.glob(str(BRAND / "spec/*.svg"))):
        job(os.path.relpath(f, BRAND).replace(os.sep, "/"), os.path.relpath(f, BRAND).replace(os.sep, "/")[:-4] + ".png", 1600, transparent=False)

    with ThreadPoolExecutor(4) as pool:
        for _ in pool.map(lambda a: export_png(chrome, *a), jobs):
            pass
    print(f"{len(jobs)} PNG files written")

    icon = lambda s: (BRAND / f"icon/pelago-icon-{s}.png").read_bytes()
    write_ico(BRAND / "icon/favicon.ico", [(s, icon(s)) for s in (16, 32, 48)])
    write_ico(BRAND / "icon/pelago.ico", [(s, icon(s)) for s in (16, 32, 48, 64, 128, 256)])
    write_icns(BRAND / "icon/pelago.icns", {s: icon(s) for s in (16, 32, 64, 128, 256, 512, 1024)})
    print("favicon.ico, pelago.ico, pelago.icns written")


if __name__ == "__main__":
    main()
