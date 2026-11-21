"""Cut the recorded video around every FF detection.

usage: python ff_clips.py <ff dir> <records dir> <hours...> [--all] [--lead 5] [--tail 4] [--out DIR]

 * records are one file per minute: <records>/<HH>/<MM>.mp4 (60 s, GOP 4 s).  A
   file starts at the first IDR after the minute change, i.e. 0-4 s late, so the
   event time within a file is only known to ~4 s: hence the lead/tail padding.
 * event time = FF block_start + maxframe/fps (f0..f1 of the FF meteor); a block
   with only a realtime event (no FF line) uses the whole 10 s block.
 * block_start / record dirs are both device-local time (no conversion).
 * clips are cut with -c copy (keyframe aligned, instant, lossless); an event
   that crosses a minute boundary is joined from two files.
 * only blocks marked "good" in review.csv are cut, unless --all.
"""
import argparse
import csv
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta

FFMPEG = "ffmpeg"

ap = argparse.ArgumentParser()
ap.add_argument("ff_root")
ap.add_argument("rec_root")
ap.add_argument("hours", nargs="*")
ap.add_argument("--all", action="store_true")
ap.add_argument("--lead", type=float, default=5.0)
ap.add_argument("--tail", type=float, default=4.0)
ap.add_argument("--out")
a = ap.parse_args()
out_dir = a.out or os.path.join(a.ff_root, "clips")
os.makedirs(out_dir, exist_ok=True)

marks = {}
cp = os.path.join(a.ff_root, "review.csv")
if os.path.exists(cp) and not a.all:
    marks = {r[0]: r[1] for r in csv.reader(open(cp, newline="")) if len(r) >= 2}

tmp = tempfile.mkdtemp(prefix="ff_clips_")


def rec_file(t):
    return os.path.join(a.rec_root, t.strftime("%H"), t.strftime("%M") + ".mp4")


def cut(src, start_s, dur_s, dst):
    subprocess.run([FFMPEG, "-v", "error", "-y", "-ss", f"{start_s:.2f}", "-t", f"{dur_s:.2f}",
                    "-i", src, "-c", "copy", "-avoid_negative_ts", "make_zero", dst], check=True)


done = skipped = 0
for h in a.hours or sorted(os.listdir(a.ff_root)):
    for p in sorted(glob.glob(os.path.join(a.ff_root, h, "*.json"))):
        name = os.path.basename(p)[:-5]
        if marks and marks.get(name) != "good":
            continue
        j = json.load(open(p, encoding="utf-8"))
        bs = datetime.fromisoformat(j["block_start"])
        fps = j.get("fps") or 25.0
        ms = j.get("ff_meteors") or []
        if ms:
            f0 = min(m["f0"] for m in ms)
            f1 = max(m["f1"] for m in ms)
        else:
            f0, f1 = 0, j.get("n_frames", 256)
        t0 = bs + timedelta(seconds=f0 / fps - a.lead)
        t1 = bs + timedelta(seconds=f1 / fps + a.tail)
        # minute files covering [t0, t1)
        parts, t = [], t0
        while t < t1:
            mstart = t.replace(second=0, microsecond=0)
            mend = mstart + timedelta(minutes=1)
            seg_end = min(t1, mend)
            src = rec_file(mstart)
            if not os.path.exists(src):
                parts = None
                print(f"{name}: no recording {src}  -> skipped")
                break
            parts.append((src, (t - mstart).total_seconds(), (seg_end - t).total_seconds()))
            t = mend
        if parts is None:
            skipped += 1
            continue
        dst = os.path.join(out_dir, f"{name[3:]}.mp4")
        tmp_parts = []
        for i, (src, ss, d) in enumerate(parts):
            tp = os.path.join(tmp, f"{name}_{i}.mp4")
            cut(src, ss, d, tp)
            tmp_parts.append(tp)
        if len(tmp_parts) == 1:
            shutil.move(tmp_parts[0], dst)
        else:
            lst = os.path.join(tmp, name + ".txt")
            with open(lst, "w") as f:
                f.writelines(f"file '{tp.replace(os.sep, '/')}'\n" for tp in tmp_parts)
            subprocess.run([FFMPEG, "-v", "error", "-y", "-f", "concat", "-safe", "0",
                            "-i", lst, "-c", "copy", dst], check=True)
        span = (t1 - t0).total_seconds()
        print(f"{name}: event {bs + timedelta(seconds=f0 / fps):%H:%M:%S}"
              f" ({'FF' if ms else 'RT'})  clip {t0:%H:%M:%S}+{span:.0f}s"
              f"{'  (2 files)' if len(parts) > 1 else ''}  -> {dst}")
        done += 1
print(f"done {done}, skipped {skipped}")
