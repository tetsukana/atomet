"""Lighten (比較明) composite of the FF blocks marked "good" in review.csv.

usage: python ff_composite.py <ff dir> <hours...> [--all] [--out-prefix comp]
Writes
  <prefix>_maxpixel.png : max over blocks of MAXPIXEL (stars + meteors; stars
                          show as dotted trails because the sky turns)
  <prefix>_meteors.png  : max over blocks of (MAXPIXEL - AVEPIXEL), clipped at 0
                          = meteors only, over the faint mean sky/scene
--all ignores review.csv and uses every block that has an FF meteor.
"""
import argparse
import csv
import glob
import json
import os

import numpy as np
from astropy.io import fits
import matplotlib.pyplot as plt

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("hours", nargs="*")
ap.add_argument("--all", action="store_true")
ap.add_argument("--out-prefix", default="comp")
a = ap.parse_args()

marks = {}
cp = os.path.join(a.root, "review.csv")
if os.path.exists(cp) and not a.all:
    marks = {r[0]: r[1] for r in csv.reader(open(cp, newline="")) if len(r) >= 2}

blocks = []
for h in a.hours or sorted(os.listdir(a.root)):
    for p in sorted(glob.glob(os.path.join(a.root, h, "*.json"))):
        name = os.path.basename(p)[:-5]
        j = json.load(open(p, encoding="utf-8"))
        if marks:
            ok = marks.get(name) == "good"
        else:
            ok = bool(j.get("ff_meteors"))
        if ok:
            blocks.append(p[:-5] + ".fits")
print(len(blocks), "blocks")

mx_all = diff_all = ave_sum = None
for f in blocks:
    with fits.open(f) as hd:
        mx, ave = hd[0].data.astype(np.float32), hd[2].data.astype(np.float32)
    d = np.clip(mx - ave, 0, None)
    mx_all = mx if mx_all is None else np.maximum(mx_all, mx)
    diff_all = d if diff_all is None else np.maximum(diff_all, d)
    ave_sum = ave if ave_sum is None else ave_sum + ave
ave_mean = ave_sum / len(blocks)

# 1) plain lighten composite
lo, hi = np.percentile(mx_all, [1, 99.8])
img = np.clip((mx_all - lo) / (hi - lo), 0, 1) ** 0.8
plt.imsave(a.out_prefix + "_maxpixel.png", np.kron(img, np.ones((2, 2))), cmap="gray", vmin=0, vmax=1)

# 2) meteors only on a dim mean background; colour = how bright
bg_lo, bg_hi = np.percentile(ave_mean, [1, 99.5])
bg = np.clip((ave_mean - bg_lo) / (bg_hi - bg_lo), 0, 1) * 0.35
mt = np.clip(diff_all / max(np.percentile(diff_all, 99.9), 40), 0, 1) ** 0.6
rgb = np.clip(np.stack([bg + mt, bg + mt * 0.85, bg + mt * 0.6], -1), 0, 1)
plt.imsave(a.out_prefix + "_meteors.png", np.kron(rgb, np.ones((2, 2, 1))))
print("saved", a.out_prefix + "_maxpixel.png", a.out_prefix + "_meteors.png")
