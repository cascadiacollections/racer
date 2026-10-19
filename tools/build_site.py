#!/usr/bin/env python3
"""Racer: the project site's images and its licenses page.

Every picture on the site is a frame the game's own renderer painted. The
golden frames (`golden_scenes` in crates/racer-godot/src/render.rs, whose
digests the test suite pins) are dumped as PPM by an ignored test, then
written here as indexed PNG files: the sixteen colours of palette.rs, integer
nearest-neighbour scaling only, nothing blended. A pixel that is not a
palette colour fails the build rather than being quietly re-quantised.

    python3 tools/build_site.py       # needs Rust; writes into site/

Standard library only (zlib, struct): no Pillow, no ImageMagick. The output
is a pure function of the renderer: a rebuild with no renderer change writes
the same pixels. (The compressed bytes can differ between zlib builds, e.g.
Fedora's zlib-ng and Ubuntu's zlib; the pixels do not.)

Outputs:
    site/assets/frames/<scene>.png    each golden frame, 2x
    site/assets/hero.png              three frames side by side, 2x
    site/assets/palette_16.png        the palette, in index order
    site/assets/og.png                1200x630 social card
    site/assets/favicon.png           the car, 64x64 (32x32 at 2x)
    site/licenses.html                racer's license and third-party notices
"""
import html
import os
import re
import struct
import subprocess
import sys
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SITE = os.path.join(ROOT, "site")
OUT = os.path.join(SITE, "assets")
FRAMES = os.path.join(ROOT, "target", "frames")

# The scenes, in the order the page shows them. Names are golden_scenes()'s.
SCENES = (
    "drift_corner_signed",
    "sweep_mid_curve",
    "hairpin_entry",
    "crest_climbing",
    "crest_over",
    "hazards",
)
HERO = ("sweep_mid_curve", "drift_corner_signed", "crest_over")


# ------------------------------------------------------------- the palette
def palette():
    """The sixteen colours, read from palette.rs so the site cannot drift
    from the game. Returns a list of (r, g, b) in index order."""
    with open(
        os.path.join(ROOT, "crates", "racer-godot", "src", "palette.rs"),
        encoding="utf-8",
    ) as f:
        src = f.read()
    body = re.search(r"pub const COLOURS: \[Rgba; 16\] = \[(.*?)\];", src, re.S)
    if not body:
        sys.exit("build_site: COLOURS not found in palette.rs")
    cols = [
        tuple(int(v) for v in m.groups())
        for m in re.finditer(r"\[(\d+), (\d+), (\d+), 255\]", body.group(1))
    ]
    if len(cols) != 16 or len(set(cols)) != 16:
        sys.exit("build_site: expected 16 distinct opaque colours in palette.rs")
    return cols


PAL = palette()
INDEX = {c: i for i, c in enumerate(PAL)}
INK, WHITE, RED, YELLOW = 0, 3, 4, 14


# ----------------------------------------------------------------- images
class Img:
    """An image of palette indices, one bytearray row per line."""

    def __init__(self, w, h, fill=INK):
        self.w, self.h = w, h
        self.rows = [bytearray([fill]) * w for _ in range(h)]

    @staticmethod
    def from_ppm(path):
        with open(path, "rb") as f:
            data = f.read()
        m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
        if not m:
            sys.exit(f"build_site: {path} is not an 8-bit P6 PPM")
        w, h = int(m.group(1)), int(m.group(2))
        px = data[m.end():]
        if len(px) != w * h * 3:
            sys.exit(f"build_site: {path} is truncated")
        img = Img(w, h)
        for y in range(h):
            row = img.rows[y]
            for x in range(w):
                o = (y * w + x) * 3
                c = (px[o], px[o + 1], px[o + 2])
                if c not in INDEX:
                    sys.exit(f"build_site: {path} ({x}, {y}) {c} is not a palette colour")
                row[x] = INDEX[c]
        return img

    def crop(self, x, y, w, h):
        out = Img(w, h)
        out.rows = [bytearray(r[x:x + w]) for r in self.rows[y:y + h]]
        return out

    def paste(self, src, x, y):
        for j, row in enumerate(src.rows):
            if 0 <= y + j < self.h:
                lo, hi = max(0, -x), min(src.w, self.w - x)
                if lo < hi:
                    self.rows[y + j][x + lo:x + hi] = row[lo:hi]

    def rect(self, x, y, w, h, c):
        for j in range(max(0, y), min(self.h, y + h)):
            self.rows[j][max(0, x):min(self.w, x + w)] = bytes([c]) * (
                min(self.w, x + w) - max(0, x)
            )

    def scaled(self, k):
        out = Img(self.w * k, self.h * k)
        out.rows = []
        for r in self.rows:
            wide = bytearray(b for b in r for _ in range(k))
            out.rows.extend(bytearray(wide) for _ in range(k))
        return out

    def png(self, path):
        """Write an indexed (colour type 3) PNG. Every row uses filter 0 and
        zlib level 9, so one zlib gives equal bytes for equal images."""

        def chunk(tag, body):
            return (
                struct.pack(">I", len(body))
                + tag
                + body
                + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
            )

        raw = b"".join(b"\x00" + bytes(r) for r in self.rows)
        data = (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", self.w, self.h, 8, 3, 0, 0, 0))
            + chunk(b"PLTE", b"".join(bytes(c) for c in PAL))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b"")
        )
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)


# ----------------------------------------------------------------- frames
def dump_frames():
    """Have the renderer paint its golden scenes into target/frames."""
    subprocess.run(
        [
            "cargo", "test", "--quiet", "-p", "racer-godot", "--lib",
            "render::tests::dump_golden_frames", "--", "--ignored", "--exact",
        ],
        cwd=ROOT,
        check=True,
    )
    frames = {}
    for name in SCENES:
        path = os.path.join(FRAMES, name + ".ppm")
        if not os.path.exists(path):
            sys.exit(f"build_site: the renderer wrote no {name}.ppm; "
                     "update SCENES to match golden_scenes()")
        frames[name] = Img.from_ppm(path)
    return frames


def layout_const(name):
    """A constant from layout.rs, so the crops follow the portrait layout."""
    with open(
        os.path.join(ROOT, "crates", "racer-godot", "src", "layout.rs"),
        encoding="utf-8",
    ) as f:
        m = re.search(rf"pub const {name}: i32 = (\d+);", f.read())
    if not m:
        sys.exit(f"build_site: {name} not found in layout.rs")
    return int(m.group(1))


PAD_H, HUD_H = layout_const("PAD_H"), layout_const("HUD_H")


def road_rows(img):
    """Height of the road view: everything above the HUD strip and the pad
    (`Layout::new`, no inset)."""
    return img.h - PAD_H - HUD_H


def car_box(img):
    """The player's car: the bounding box of red in the road view's lower
    third, where nothing else is red except kerbs, which sit at the edges."""
    top = road_rows(img)
    xs, ys = [], []
    for y in range(top * 2 // 3, top):
        row = img.rows[y]
        for x in range(img.w // 4, img.w * 3 // 4):
            if row[x] == RED:
                xs.append(x)
                ys.append(y)
    if not xs:
        sys.exit("build_site: no car found in the frame")
    return min(xs), min(ys), max(xs) - min(xs) + 1, max(ys) - min(ys) + 1


# ------------------------------------------------------------------ media
def hero(frames):
    """Three road views side by side, cropped to the same height, with a
    two-pixel ink gutter."""
    views = [frames[n] for n in HERO]
    h = min(road_rows(v) for v in views)
    gap = 2
    w = sum(v.w for v in views) + gap * (len(views) - 1)
    out = Img(w, h)
    x = 0
    for v in views:
        out.paste(v.crop(0, road_rows(v) - h, v.w, h), x, 0)
        x += v.w + gap
    return out.scaled(2)


def palette_strip():
    out = Img(16 * 12, 20)
    for i in range(16):
        out.rect(i * 12, 0, 12, 20, i)
    return out.scaled(4)


def og_card(frames):
    """1200x630: a 400x210 card drawn at 1x, then scaled 3x. Two road views
    on the right, the palette down the left edge, ink behind."""
    w, h = 400, 210
    card = Img(w, h, INK)
    x = w - 2 * 180 - 2 - 8
    for name in ("drift_corner_signed", "crest_over"):
        v = frames[name]
        top = road_rows(v)
        card.paste(v.crop(0, top - (h - 16), 180, h - 16), x, 8)
        x += 182
    # A one-pixel frame, so swatch 0 (ink) shows against the ink behind it.
    card.rect(7, 7, 18, 16 * 12 + 2, 1)
    for i in range(16):
        card.rect(8, 8 + i * 12, 16, 12, i)
    return card.scaled(3)


def favicon(frames):
    """The car, centred on ink in a 32x32 tile, scaled 2x."""
    f = frames["sweep_mid_curve"]
    x, y, w, h = car_box(f)
    w, h = min(w, 30), min(h, 30)
    tile = Img(32, 32, INK)
    tile.paste(f.crop(x, y, w, h), (32 - w) // 2, (32 - h) // 2)
    return tile.scaled(2)


# ---------------------------------------------------------------- licenses
NOTICES = (
    ("racer: MIT", "LICENSE"),
    ("Godot Engine: MIT", "third_party/godot-LICENSE.txt"),
    ("Godot Rust bindings (gdext): MPL-2.0", "third_party/gdext-LICENSE-MPL-2.0.txt"),
    ("glam: MIT option", "third_party/glam-LICENSE-MIT.txt"),
    ("libc (desktop builds): MIT option", "third_party/libc-LICENSE-MIT.txt"),
    ("godot-device-power: MIT", "addons/device_power/LICENSE"),
)


def license_page():
    sections = []
    for title, rel in NOTICES:
        with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
            notice = f.read()
        sections.append(
            f'<details><summary>{html.escape(title)}</summary>'
            f'<pre class="license-text">{html.escape(notice)}</pre></details>'
        )
    page = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>racer: credits and licenses</title>
<meta name="description" content="racer's MIT license and the notices for the free and open-source software it is built on.">
<meta name="theme-color" content="#10121c">
<link rel="icon" href="assets/favicon.png" type="image/png">
<link rel="stylesheet" href="style.css">
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="topbar"><a class="brand" href="index.html"><span>RACER</span></a>
<nav aria-label="Project links"><a href="index.html#dev">Dev build</a></nav></header>
<main id="main"><section aria-labelledby="licenses-h">
<h1 id="licenses-h">Credits &amp; licenses</h1>
<p>racer is by Cascadia Collections and is licensed under MIT. It is built on
Godot Engine, the gdext Rust bindings and godot-device-power, and the Android
build carries their code; thank you to everyone behind them.</p>
<p>The notices below cover the engine, the Rust crates compiled into the
game's extension, and the vendored addon. The simulation crate has no
dependencies at all.</p>
<p><a href="index.html">Back to racer</a></p>
""" + "\n".join(sections) + "\n</section></main>\n</body>\n</html>\n"
    with open(os.path.join(SITE, "licenses.html"), "w", encoding="utf-8") as f:
        f.write(page)


# ------------------------------------------------------------------ driver
def main():
    frames = dump_frames()
    for name in SCENES:
        frames[name].scaled(2).png(os.path.join(OUT, "frames", name + ".png"))
        print(f"frames/{name}.png ({frames[name].w}x{frames[name].h}, 2x)")
    hero(frames).png(os.path.join(OUT, "hero.png"))
    print("hero.png")
    palette_strip().png(os.path.join(OUT, "palette_16.png"))
    print("palette_16.png")
    og_card(frames).png(os.path.join(OUT, "og.png"))
    print("og.png")
    favicon(frames).png(os.path.join(OUT, "favicon.png"))
    print("favicon.png")
    license_page()
    print("licenses.html")


if __name__ == "__main__":
    main()
