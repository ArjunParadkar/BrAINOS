#!/usr/bin/env python3
"""Generate the body's brand wallpaper (pixel brain + BR[AI]N OS in dotted caps, near-black
ground, pink accent) → wallpaper.png next to this file. Pure PIL, no fonts needed."""
import os, math
from PIL import Image, ImageDraw
W, H = 2560, 1600; BG = (11, 5, 7); PINK = (255, 79, 163); WHITE = (238, 226, 232)
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wallpaper.png")

def brain_grid(cols=30, rows=24):
    """Pixel brain like the seed's brand screen: two lobes + stem, grooves fanning from
    the stem to the crown (carved on the pixel grid)."""
    g = [[0] * cols for _ in range(rows)]
    def blob(cx, cy, rx, ry):
        for y in range(rows):
            for x in range(cols):
                if ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1: g[y][x] = 1
    blob(12, 10, 11, 9); blob(19, 10.5, 10, 9.5); blob(9, 16, 6, 4)     # lobes + temporal bulge
    for y in range(18, 23):                                              # stem
        for x in range(15, 19): g[y][x] = 1
    # grooves: rays from the stem root, 2 px wide, stop short of the rim
    import math
    ox, oy = 16.5, 17.5
    for ang in (-150, -122, -95, -68, -42):
        a = math.radians(ang)
        for t in range(3, 13):
            x, y = ox + math.cos(a) * t * 0.95, oy + math.sin(a) * t * 0.95
            for dx, dy in ((0, 0), (1, 0)):
                X, Y = int(x) + dx, int(y) + dy
                if 0 <= X < cols and 0 <= Y < rows and 2 <= X < cols - 2 and Y >= 2: g[Y][X] = 0
    for x in range(8, 15): g[16][x] = 0; g[17][x] = 0                    # the horizontal fold
    return ["".join("1" if v else "." for v in row) for row in g]
BRAIN = brain_grid()
GROOVES = []

# 5x7 dotted glyph font for the title
F = {
"B":["1110","1001","1001","1110","1001","1001","1110"],
"R":["1110","1001","1001","1110","1010","1001","1001"],
"A":["0110","1001","1001","1111","1001","1001","1001"],
"I":["111","010","010","010","010","010","111"],
"N":["1001","1101","1101","1011","1011","1001","1001"],
"O":["0110","1001","1001","1001","1001","1001","0110"],
"S":["0111","1000","1000","0110","0001","0001","1110"],
" ":["00","00","00","00","00","00","00"],
}
im = Image.new("RGB", (W, H), BG); d = ImageDraw.Draw(im)
# subtle radial warmth behind the brain
cx, cy = W // 2, int(H * 0.40)
glow = Image.new("RGB", (W, H), BG); gd = ImageDraw.Draw(glow)
for r in range(700, 0, -14):
    k = (1 - r / 700) ** 2 * 0.22
    gd.ellipse((cx - r, cy - r * 0.8, cx + r, cy + r * 0.8), fill=tuple(int(b + (p - b) * k) for b, p in zip(BG, PINK)))
im = Image.blend(im, glow, 0.9); d = ImageDraw.Draw(im)
# pixel brain
px = 18; bw = len(BRAIN[0]) * px; bx = cx - bw // 2; by = cy - len(BRAIN) * px // 2 - 60
for y, row in enumerate(BRAIN):
    for x, c in enumerate(row):
        if c == "1": d.rectangle((bx + x * px, by + y * px, bx + x * px + px - 1, by + y * px + px - 1), fill=PINK)
for (x0, y0, x1, y1) in GROOVES:
    d.rectangle((bx + x0 * px + 4, by + y0 * px + 4, bx + x1 * px + px - 5, by + y1 * px + px - 5), fill=BG)
# dotted title
title = "BRAIN OS"; dot = 16; gapc = 26; cell = dot + 10
tw = sum((len(F[ch][0]) + 1) * cell for ch in title) - cell; tx = cx - tw // 2; ty = by + len(BRAIN) * px + 90
for ch in title:
    g = F[ch]; col = PINK if ch in "AI" else WHITE
    for yy, row in enumerate(g):
        for xx, c in enumerate(row):
            if c == "1":
                X, Y = tx + xx * cell, ty + yy * cell
                if ch in "AI": d.rectangle((X, Y, X + dot, Y + dot), fill=col)
                else: d.ellipse((X, Y, X + dot, Y + dot), fill=col)
    tx += (len(g[0]) + 1) * cell
# footer line
from PIL import ImageFont
font = None
for f in ("/usr/share/fonts/inter/Inter-Medium.otf", "/usr/share/fonts/TTF/Inter-Medium.ttf", "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Medium.ttf", "/usr/share/fonts/noto/NotoSans-Medium.ttf"):
    if os.path.exists(f):
        font = ImageFont.truetype(f, 20); break
foot = "ARENDA INNOVATIONS   ·   THE AI-NATIVE OPERATING SYSTEM"
fw = d.textlength(foot, font=font) if font else 300
d.text((cx - fw / 2, ty + 7 * cell + 48), foot, fill=(120, 96, 108), font=font)
im = im.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
im.save(OUT, optimize=True); print(OUT, os.path.getsize(OUT), "bytes")
