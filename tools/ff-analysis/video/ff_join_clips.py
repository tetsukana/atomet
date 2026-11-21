"""Join the marked clips of ff_clips_cand.py into one video, with a caption on every clip.

usage: python ff_join_clips.py <clips dir with index.csv and *_marked.mp4> <out.mp4> [--speed 1.0]

The caption shows: clip number, time (JST), tier(s) and group, length, frames. Clips are in index order
(time order). A block that holds several events is one clip with the tiers joined.
"""
import argparse
import csv
import os
import shutil
import subprocess
import tempfile
from collections import OrderedDict

import numpy as np

ap = argparse.ArgumentParser()
ap.add_argument("clips")
ap.add_argument("out")
ap.add_argument("--speed", type=float, default=1.0, help="playback speed (0.5 = half speed)")
ap.add_argument("--pad", type=float, default=0.3, help="seconds kept before/after the red box (the box already has 0.4 s before, 0.6 s after)")
ap.add_argument("--font", default=r"C:\Windows\Fonts\arial.ttf")
a = ap.parse_args()

rows = list(csv.DictReader(open(os.path.join(a.clips, "index.csv"), encoding="utf-8")))
clips = OrderedDict()
for r in rows:
    p = os.path.join(a.clips, r["clip"])
    if not os.path.exists(p):
        continue
    clips.setdefault(r["clip"], []).append(r)
print(len(clips), "clips")

def box_window(path):
    """(start s, duration s) of the frames that show the red box, widened by --pad; (0, 0) = keep all."""
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-vf", "scale=480:270,format=rgb24",
                          "-f", "rawvideo", "-"], capture_output=True).stdout
    v = np.frombuffer(raw, np.uint8).reshape(-1, 270, 480, 3).astype(np.int16)
    red = ((v[..., 0] > 170) & (v[..., 1] < 90) & (v[..., 2] < 90)).sum((1, 2))
    on = np.where(red >= 15)[0]
    if len(on) == 0 or len(on) >= len(v) - 2:      # box never / always shown: cannot trim
        return 0.0, 0.0
    t0 = max(on.min() / 25 - a.pad, 0.0)
    t1 = min((on.max() + 1) / 25 + a.pad, len(v) / 25)
    return t0, t1 - t0


tmp = tempfile.mkdtemp(prefix="join_")
shutil.copy(a.font, os.path.join(tmp, "font.ttf"))
parts = []
for i, (name, rs) in enumerate(clips.items(), 1):
    r0 = rs[0]
    tiers = "/".join(sorted({r["tier"] for r in rs}))
    text = (f"{i}/{len(clips)}  {r0['time_JST']} JST   tier {tiers}   {r0['group']}\n"
            f"L{r0['length_px']}px  {r0['frames']} frames  {r0['speed_px_f']} px/f" + (f"   ({len(rs)} events)" if len(rs) > 1 else ""))
    open(os.path.join(tmp, "label.txt"), "w", encoding="utf-8").write(text)
    seg = f"seg_{i:03d}.mp4"
    ss, dur = box_window(os.path.join(a.clips, name))
    vf = (f"setpts={1 / a.speed}*PTS," if a.speed != 1.0 else "") + \
         "drawtext=fontfile=font.ttf:textfile=label.txt:x=30:y=24:fontsize=40:fontcolor=white:box=1:boxcolor=black@0.65:boxborderw=12"
    cut = ["-ss", f"{ss:.2f}", "-t", f"{dur:.2f}"] if dur else []
    subprocess.run(["ffmpeg", "-v", "error", "-y", *cut, "-i", os.path.abspath(os.path.join(a.clips, name)), "-vf", vf,
                    "-an", "-r", "25", "-c:v", "libx264", "-crf", "22", "-preset", "veryfast", "-pix_fmt", "yuv420p", seg],
                   cwd=tmp, check=True)
    parts.append(seg)
    print(f"  {i}/{len(clips)} {name}", flush=True)
with open(os.path.join(tmp, "list.txt"), "w") as f:
    f.writelines(f"file '{p}'\n" for p in parts)
if os.path.exists(a.out):
    os.remove(a.out)
subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", "list.txt", "-c", "copy",
                os.path.abspath(a.out)], cwd=tmp, check=True)
print("written", a.out)
