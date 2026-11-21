#!/usr/bin/env python3
"""Synthetic archive for checking page layout by eye: infinite-scroll alignment (pages far taller
than the screen) and how the overview grid fits rows of mixed aspect ratios.

Every page shows its number, size, a border, TOP/CENTER/BOTTOM marks and quarter lines. Pages come
in pairs because a phone shows the overview two to a row. Needs Pillow.

Usage: gen-aspect-test-archive.py <out.zip>  (or `mise run dev-aspect-archive`)
"""
import io
import sys
import zipfile

from PIL import Image, ImageDraw, ImageFont

FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"

# Pairs, because a phone shows the overview two to a row: (1,2), (3,4), ...
PAGES = [
    (1200, 1700, "normal"), (1200, 1700, "normal"),
    (800, 4000, "tall 1:5"), (1600, 900, "wide 16:9"),
    (1000, 1000, "square"), (600, 6000, "very tall 1:10"),
    (2400, 800, "very wide 3:1"), (1200, 1700, "normal"),
    (900, 2700, "tall 1:3"), (1000, 1000, "square"),
    (1600, 900, "wide 16:9"), (1600, 900, "wide 16:9"),
    (700, 3500, "tall 1:5"), (700, 3500, "tall 1:5"),
    (1200, 1700, "normal"), (2400, 800, "very wide 3:1"),
    (500, 8000, "extreme 1:16"), (1200, 1700, "normal"),
]
COLOURS = ["#d9e8fb", "#fbe3d9", "#dff5e1", "#f3e1fb", "#fbf6d9", "#d9f6f5"]


def page(n, w, h, label):
    img = Image.new("RGB", (w, h), COLOURS[(n - 1) % len(COLOURS)])
    d = ImageDraw.Draw(img)
    edge = max(8, min(w, h) // 60)
    d.rectangle([0, 0, w - 1, h - 1], outline="#c0392b", width=edge)
    # Quarter lines, then the centre line on top.
    for q in (0.25, 0.75):
        y = int(h * q)
        d.line([(0, y), (w, y)], fill="#7f8c8d", width=max(2, edge // 3))
    d.line([(0, h // 2), (w, h // 2)], fill="#2c3e50", width=edge)
    big = ImageFont.truetype(FONT, max(40, min(w, h) // 5))
    small = ImageFont.truetype(FONT, max(24, min(w, h) // 14))
    for text, font, y in (
        (f"P{n}", big, h // 2 - max(40, min(w, h) // 5) - edge),
        (f"{w}x{h} {label}", small, h // 2 + edge * 2),
        ("TOP", small, edge * 2),
        ("BOTTOM", small, h - edge * 2 - max(24, min(w, h) // 14)),
        ("CENTER", small, h // 2 + edge * 2 + max(24, min(w, h) // 14) + edge),
    ):
        tw = d.textlength(text, font=font)
        d.text(((w - tw) / 2, y), text, fill="#1a1a1a", font=font)
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=85)
    return buf.getvalue()


out = sys.argv[1]
with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
    for n, (w, h, label) in enumerate(PAGES, 1):
        # A fixed timestamp keeps the bytes, and so the archive ID, the same on every run.
        z.writestr(zipfile.ZipInfo(f"{n:03d}.jpg", date_time=(2026, 1, 1, 0, 0, 0)), page(n, w, h, label))
print(out, len(PAGES), "pages")
