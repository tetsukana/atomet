"""Cut video clips for candidate clusters found by ff_offline.py (blocks that
have no sidecar), and mark where/when the event happens with a red box.

usage: python ff_clips_cand.py cand.jsonl D:/records/2026/11/14 --out DIR [--edge 8] [--blocks 025044_090 ...]

 * block start = file name FF_YYYYMMDD_HHMMSS_mmm (device local time = record dir time)
 * event time = block start + cluster f0..f1 frames; recording files start 0-4 s after
   the minute boundary (GOP 4 s), so the clip is cut with a margin and the real
   event frames are located by looking at the ROI in the decoded clip.
 * the box is drawn on the 1920x1080 clip at the cluster position (x3 of 640x360).
 * candidates touching the image edge (--edge px) are skipped (sensor edge noise).
"""
import argparse
import json
import os
import shutil
import subprocess
import tempfile
from datetime import datetime, timedelta

import numpy as np

ap = argparse.ArgumentParser()
ap.add_argument("cands")
ap.add_argument("rec_root")
ap.add_argument("--out", required=True)
ap.add_argument("--edge", type=float, default=8.0)
ap.add_argument("--lead", type=float, default=6.0)
ap.add_argument("--tail", type=float, default=5.0)
ap.add_argument("--by-day", action="store_true", help="record dirs are <rec_root>/<DD>/<HH>/<MM>.mp4 (an event may cross midnight)")
ap.add_argument("--blocks", nargs="*", help="only these block suffixes, e.g. 025044_090")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)
tmp = tempfile.mkdtemp(prefix="cand_clips_")


def run(cmd):
    subprocess.run(cmd, check=True)


def rec(t):
    parts = [a.rec_root] + ([t.strftime("%d")] if a.by_day else []) + [t.strftime("%H"), t.strftime("%M") + ".mp4"]
    return os.path.join(*parts)


def decode_gray(path):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-vf", "scale=640:360,format=gray",
                          "-f", "rawvideo", "-"], capture_output=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, 360, 640).astype(np.int16)


for line in open(a.cands, encoding="utf-8"):
    r = json.loads(line)
    for c in r["clusters"]:
        if c.get("reject") is not None or "x0" not in c:
            continue
        xs, ys = [c["x0"], c["x1"]], [c["y0"], c["y1"]]
        if min(xs) < a.edge or max(xs) > 639 - a.edge or min(ys) < a.edge or max(ys) > 359 - a.edge:
            continue
        name = r["block"]  # YYYYMMDD_HHMMSS_mmm
        if a.blocks and name[9:] not in a.blocks:
            continue
        bs = datetime.strptime(name[:15], "%Y%m%d_%H%M%S") + timedelta(milliseconds=int(name[16:19]))
        fps = r.get("fps") or 25.0
        t0 = bs + timedelta(seconds=c["f0"] / fps - a.lead)
        t1 = bs + timedelta(seconds=c["f1"] / fps + a.tail)
        parts, t = [], t0
        while t < t1:
            m0 = t.replace(second=0, microsecond=0)
            m1 = m0 + timedelta(minutes=1)
            src = rec(m0)
            if not os.path.exists(src):
                parts = None
                break
            parts.append((src, (t - m0).total_seconds(), (min(t1, m1) - t).total_seconds()))
            t = m1
        if not parts:
            print(f"{name}: no recording -> skipped")
            continue
        out = os.path.join(a.out, name[9:] + "_marked.mp4")
        if os.path.exists(out):
            continue                                   # already done (re-run after a failure)
        segs = []
        try:
            for i, (src, ss, d) in enumerate(parts):
                dst = os.path.join(tmp, f"{name}_{i}.mp4")
                run(["ffmpeg", "-v", "error", "-y", "-ss", f"{ss:.2f}", "-t", f"{d:.2f}", "-i", src,
                     "-c", "copy", "-avoid_negative_ts", "make_zero", dst])
                segs.append(dst)
        except subprocess.CalledProcessError:
            print(f"{name}: recording unreadable ({', '.join(os.path.basename(p) for p, _, _ in parts)}) -> skipped")
            continue
        raw_clip = os.path.join(tmp, name + ".mp4")
        if len(segs) == 1:
            shutil.move(segs[0], raw_clip)
        else:
            lst = os.path.join(tmp, name + ".txt")
            open(lst, "w").write("".join(f"file '{s.replace(os.sep, '/')}'\n" for s in segs))
            run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", lst, "-c", "copy", raw_clip])

        # find the event frames inside the clip (ROI around the cluster)
        v = decode_gray(raw_clip)
        x0, x1 = int(min(xs)) - 8, int(max(xs)) + 9
        y0, y1 = int(min(ys)) - 8, int(max(ys)) + 9
        x0, y0, x1, y1 = max(x0, 0), max(y0, 0), min(x1, 640), min(y1, 360)
        roi = v[:, y0:y1, x0:x1]
        med = np.median(roi, axis=0)
        cnt = (np.clip(roi - med, 0, None).reshape(len(v), -1) > 25).sum(1)
        act = np.where(cnt >= 3)[0]
        if len(act):
            ts, te = max(act.min() / 25 - 0.4, 0), act.max() / 25 + 0.6
            when = f"{act.min() / 25:.1f}-{act.max() / 25:.1f}s"
        else:
            ts, te, when = 0, len(v) / 25, "not located (box always on)"
        bx, by = x0 * 3 - 20, y0 * 3 - 20
        bw, bh = (x1 - x0) * 3 + 40, (y1 - y0) * 3 + 40
        out = os.path.join(a.out, name[9:] + "_marked.mp4")
        run(["ffmpeg", "-v", "error", "-y", "-i", raw_clip, "-vf",
             f"drawbox=x={bx}:y={by}:w={bw}:h={bh}:color=red@0.9:t=3:enable='between(t,{ts:.2f},{te:.2f})'",
             "-c:v", "libx264", "-crf", "20", "-preset", "fast", out])
        shutil.copy(raw_clip, os.path.join(a.out, name[9:] + ".mp4"))
        print(f"{name}: L={c['length']:.0f}px at ({c['x0']:.0f},{c['y0']:.0f})->({c['x1']:.0f},{c['y1']:.0f}) "
              f"event in clip at {when} -> {out}")
