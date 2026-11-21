"""FF block reviewer (matplotlib).

usage: python ff_review.py <ff detections dir> [hour ...]
  e.g. python ff_review.py D:/ff/detections/2026/11/14 01 02 03 04 05

keys:  →/space next   ←/backspace prev
       g = mark GOOD (real meteor)   x = mark FALSE positive   u = clear mark
       q = quit
Marks are saved to review.csv in the dir you pass (reloaded next time).
Red line = FF detection, cyan line = realtime event.
"""
import csv
import glob
import json
import os
import sys

import numpy as np
from astropy.io import fits
import matplotlib.pyplot as plt

root = sys.argv[1]
hours = sys.argv[2:] or sorted(d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d)))
jsons = []
for h in hours:
    jsons += sorted(glob.glob(os.path.join(root, h, "*.json")))
if not jsons:
    sys.exit("no json found")

csv_path = os.path.join(root, "review.csv")
marks = {}
if os.path.exists(csv_path):
    with open(csv_path, newline="") as f:
        marks = {r[0]: r[1] for r in csv.reader(f) if len(r) >= 2}

state = {"i": 0}
fig, axs = plt.subplots(1, 3, figsize=(17, 6.2), gridspec_kw={"width_ratios": [1.8, 1, 1]})


def save():
    with open(csv_path, "w", newline="") as f:
        csv.writer(f).writerows(sorted(marks.items()))


def draw():
    p = jsons[state["i"]]
    name = os.path.basename(p)[:-5]
    j = json.load(open(p))
    with fits.open(p[:-5] + ".fits") as f:
        mx, mf, av = (f[k].data.astype(float) for k in (0, 1, 2))
    ffm = j.get("ff_meteors", [])
    rt = [e for e in j.get("realtime_events", []) if e["type"] == "meteor"]
    diff = mx - av

    for a in axs:
        a.clear()
    axs[0].imshow(mx, cmap="gray", vmin=0, vmax=max(60, np.percentile(mx, 99.9)))
    for m in ffm:
        axs[0].plot([m["x0"], m["x1"]], [m["y0"], m["y1"]], "r-", lw=0.8, alpha=0.7)
    for e in rt:
        t = np.array(e["trajectory"], float)
        axs[0].plot(t[:, 0], t[:, 1], "c-", lw=0.8, alpha=0.7)

    # zoom on first FF meteor, else first realtime event, else frame centre
    if ffm:
        m = ffm[0]
        cx, cy = (m["x0"] + m["x1"]) / 2, (m["y0"] + m["y1"]) / 2
        half = max(40, int(m["length"]))
    elif rt:
        t = np.array(rt[0]["trajectory"], float)
        cx, cy = t[:, 0].mean(), t[:, 1].mean()
        half = 40
    else:
        cx, cy, half = 320, 180, 80
    x0 = int(max(0, min(640 - 2 * half, cx - half)))
    y0 = int(max(0, min(360 - 2 * half, cy - half)))
    sl = (slice(y0, y0 + 2 * half), slice(x0, x0 + 2 * half))
    ext = (x0, x0 + 2 * half, y0 + 2 * half, y0)
    axs[1].imshow(diff[sl], cmap="gray", vmin=0, vmax=max(40, np.percentile(diff[sl], 99.8)), extent=ext)
    flagged = diff[sl] > 15
    axs[2].imshow(np.where(flagged, mf[sl], np.nan), cmap="turbo", vmin=0, vmax=255, extent=ext)
    axs[0].set_title(f"maxpixel  {name}", fontsize=9)
    axs[1].set_title("maxpixel - avepixel (zoom)", fontsize=9)
    axs[2].set_title("maxframe (excess>15), turbo 0-255", fontsize=9)

    lines = [f"[{state['i'] + 1}/{len(jsons)}]  mark: {marks.get(name, '-')}   reason={j['reason']}"]
    for m in ffm:
        lines.append(
            f"FF len={m['length']:.0f}px frames={m['frames']} (f{m['f0']:.0f}-{m['f1']:.0f}) "
            f"speed={m['speed']:.2f} px/f px={m['pixels']} corr={m['time_corr']:.2f}"
        )
    for e in rt:
        lines.append(f"RT id={e['id']} frames={e['frames']} speed={e['speed']} short={e.get('short')}")
    if not ffm:
        lines.append("(no FF detection in this block)")
    fig.suptitle("\n".join(lines), fontsize=9, ha="left", x=0.01)
    fig.tight_layout(rect=(0, 0, 1, 0.9))
    fig.canvas.draw_idle()


def on_key(ev):
    name = os.path.basename(jsons[state["i"]])[:-5]
    k = ev.key
    if k in ("right", " "):
        state["i"] = min(state["i"] + 1, len(jsons) - 1)
    elif k in ("left", "backspace"):
        state["i"] = max(state["i"] - 1, 0)
    elif k in ("g", "x"):
        marks[name] = "good" if k == "g" else "false"
        save()
        state["i"] = min(state["i"] + 1, len(jsons) - 1)
    elif k == "u":
        marks.pop(name, None)
        save()
    elif k == "q":
        plt.close(fig)
        return
    draw()


fig.canvas.mpl_connect("key_press_event", on_key)
draw()
plt.show()
