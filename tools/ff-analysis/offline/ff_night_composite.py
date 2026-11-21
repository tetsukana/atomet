"""Lighten (比較明) composites of the meteors of one night, from ff_night.py's events.csv.

usage: python ff_night_composite.py events.csv cache_a.jsonl [cache_b.jsonl ...] --out DIR

Writes into DIR
  composite_maxpixel.png   plain lighten composite of the blocks' MAXPIXEL (stars show as dotted trails)
  composite_meteors.png    lighten composite of (MAXPIXEL - AVEPIXEL): the meteors only, over a dim mean background
  composite_by_shower.png  the same, each meteor tinted by its shower (grey = sporadic), with a legend
The camera is fixed to the Earth, so every block shares the same pixel grid.
"""
import argparse
import csv
import json
import os
import sys
from collections import Counter

import numpy as np
from astropy.io import fits
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

ap = argparse.ArgumentParser()
ap.add_argument("events")
ap.add_argument("caches", nargs="+")
ap.add_argument("--out", required=True)
ap.add_argument("--scale", type=int, default=2)
ap.add_argument("--exclude", nargs="*", default=[], help="block names (or HHMMSS_mmm suffixes) judged false by eye")
ap.add_argument("--floor", type=float, default=12.0, help="subtract this many ADU (noise floor, ~5 sigma) before compositing")
ap.add_argument("--band", type=float, default=14.0, help="keep only pixels within this many px of a detected line")
ap.add_argument("--extend", type=float, default=40.0, help="extend each detected line by this many px at both ends "
                "(a dotted streak is often detected in pieces)")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)

paths = {}
for c in a.caches:
    for l in open(c, encoding="utf-8"):
        r = json.loads(l)
        if "error" not in r:
            paths[r["block"]] = r["path"]
ev = [e for e in csv.DictReader(open(a.events, encoding="utf-8"))
      if not any(e["block"] == x or e["block"].endswith(x) for x in a.exclude)]
segs = {}
for e in ev:
    segs.setdefault(e["block"], []).append(tuple(float(e[k]) for k in ("x_start", "y_start", "x_end", "y_end")))
blocks = {}
for e in ev:                      # a block may hold several events: keep the first shower for colouring
    blocks.setdefault(e["block"], e["shower"] or "SPO")
print(len(blocks), "blocks;", dict(Counter(blocks.values())))

H_, W_ = 360, 640
yy, xx = np.mgrid[0:H_, 0:W_]


def line_mask(seg_list):
    """Pixels within a.band of any detected segment (each extended by a.extend at both ends)."""
    m = np.zeros((H_, W_), bool)
    for x0, y0, x1, y1 in seg_list:
        dx, dy = x1 - x0, y1 - y0
        L = max(np.hypot(dx, dy), 1.0)
        ux, uy = dx / L, dy / L
        x0, y0, x1, y1 = x0 - ux * a.extend, y0 - uy * a.extend, x1 + ux * a.extend, y1 + uy * a.extend
        dx, dy = x1 - x0, y1 - y0
        t = np.clip(((xx - x0) * dx + (yy - y0) * dy) / (dx * dx + dy * dy), 0, 1)
        m |= np.hypot(xx - (x0 + t * dx), yy - (y0 + t * dy)) <= a.band
    return m


mx_all = None
diff_by_block = {}
ave_sum = None
for b, sh in blocks.items():
    with fits.open(paths[b], memmap=False) as h:
        mx, ave = h[0].data.astype(np.float32), h[2].data.astype(np.float32)
    diff_by_block[b] = np.clip(mx - ave - a.floor, 0, None) * line_mask(segs[b])   # meteors only: no cloud changes
    mx_all = mx if mx_all is None else np.maximum(mx_all, mx)
    ave_sum = ave if ave_sum is None else ave_sum + ave
ave_mean = ave_sum / len(blocks)
diff_all = np.maximum.reduce(list(diff_by_block.values()))
k = a.scale
up = lambda x: np.kron(x, np.ones((k, k) + (1,) * (x.ndim - 2)))

lo, hi = np.percentile(mx_all, [1, 99.8])
plt.imsave(os.path.join(a.out, "composite_maxpixel.png"), up(np.clip((mx_all - lo) / (hi - lo), 0, 1) ** 0.8), cmap="gray", vmin=0, vmax=1)

bg_lo, bg_hi = np.percentile(ave_mean, [1, 99.5])
bg = np.clip((ave_mean - bg_lo) / (bg_hi - bg_lo), 0, 1) * 0.35
norm = max(np.percentile(diff_all, 99.9), 40)
mt = np.clip(diff_all / norm, 0, 1) ** 0.6
plt.imsave(os.path.join(a.out, "composite_meteors.png"),
           up(np.clip(np.stack([bg + mt, bg + mt * 0.85, bg + mt * 0.6], -1), 0, 1)))

# by shower: each block tinted by its shower, combined by per-channel maximum
codes = [c for c, _ in Counter(blocks.values()).most_common() if c != "SPO"]
pal = dict(zip(codes, plt.cm.tab10.colors))
pal["SPO"] = (0.75, 0.75, 0.75)
rgb = np.stack([bg, bg, bg], -1)
for b, sh in blocks.items():
    m = (np.clip(diff_by_block[b] / norm, 0, 1) ** 0.6)[..., None] * np.array(pal[sh])
    rgb = np.maximum(rgb, np.clip(m + bg[..., None], 0, 1))
fig, ax = plt.subplots(figsize=(16, 9.2))
ax.imshow(np.clip(rgb, 0, 1))
ax.axis("off")
handles = [plt.Line2D([], [], color=pal[c], lw=4, label=f"{c} ({n})") for c, n in Counter(blocks.values()).most_common()]
ax.legend(handles=handles, loc="lower left", fontsize=10, facecolor="black", labelcolor="white", framealpha=0.6)
ax.set_title(f"{len(blocks)} meteors of the night, lighten composite, coloured by shower (grey = sporadic)", color="white")
fig.patch.set_facecolor("black")
plt.tight_layout()
plt.savefig(os.path.join(a.out, "composite_by_shower.png"), dpi=110, facecolor="black")
print("written to", a.out)
