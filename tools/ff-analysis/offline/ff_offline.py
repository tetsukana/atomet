"""Offline port of crates/atometd/src/ff.rs `ff_detect_meteor` / `analyze_cluster`.

Same algorithm, same default constants, but every gate is a parameter and every
cluster is reported with the gate that rejected it.  Use it to
  validate : check the port reproduces the sidecars of the saved detections
  scan     : run (relaxed) parameters over saved blocks, list near-misses
  sheet    : contact sheet of the best candidates for visual review

usage:
  python ff_offline.py validate D:/ff/detections/2026/11/14
  python ff_offline.py scan     D:/ff/2026/11/14 --out cand.jsonl [--relaxed]
  python ff_offline.py sheet    cand.jsonl --top 40 --out sheet.png

Block time axis assumes ~25 fps unless the FITS header FPS is present.
"""
import argparse
import glob
import json
import math
import os
import sys
from concurrent.futures import ProcessPoolExecutor

import numpy as np
from scipy import ndimage as ndi
from astropy.io import fits

W, H = 640, 360
MASK_CELL, MASK_CX = 8, 80
MASK_PATH = "D:/config/mask.bin"
REF_FPS = 25.0

# defaults = ff.rs constants (2026-11-17)
STRICT = dict(sigma_k=5.0, min_excess=15.0, min_pixels=12, max_pixels=20000,
              min_lin=5.0, min_len=12.0, min_corr=0.8, min_speed=0.8)
# only the gates that decide "meteor-like"; relaxed to surface near-misses
RELAXED = dict(STRICT, min_pixels=8, min_lin=3.0, min_len=8.0, min_corr=0.5,
               min_speed=0.4, min_excess=12.0)


def load_mask(path=MASK_PATH):
    try:
        m = np.fromfile(path, np.uint8)
        if m.size == MASK_CX * (H // MASK_CELL):
            return m
    except OSError:
        pass
    return np.zeros(MASK_CX * (H // MASK_CELL), np.uint8)


def pixel_mask(cell_mask):
    cm = cell_mask.reshape(H // MASK_CELL, MASK_CX)
    return np.kron(cm, np.ones((MASK_CELL, MASK_CELL), np.uint8)) != 0


def analyze(xs, ys, fr, fps, P):
    """Return (metrics dict, first failing gate or None)."""
    n = len(xs)
    mx, my = xs.mean(), ys.mean()
    dx, dy = xs - mx, ys - my
    sxx, sxy, syy = (dx * dx).mean(), (dx * dy).mean(), (dy * dy).mean()
    tr = sxx + syy
    det = sxx * syy - sxy * sxy
    disc = math.sqrt(max(tr * tr / 4 - det, 0.0))
    l1 = tr / 2 + disc
    l2 = max(tr / 2 - disc, 0.0625)
    length = math.sqrt(12 * l1)
    m = dict(pixels=n, lin=l1 / l2, length=length)
    theta = 0.5 * math.atan2(2 * sxy, sxx - syy)
    ux, uy = math.cos(theta), math.sin(theta)
    pr = dx * ux + dy * uy
    df = fr - fr.mean()
    spp, spf, sff = (pr * pr).sum(), (pr * df).sum(), (df * df).sum()
    if spp <= 0 or sff <= 0:
        m.update(corr=0.0, span=0.0, speed=0.0)
        return m, "no_time_gradient"
    r = spf / math.sqrt(spp * sff)
    span = max(fr.max() - fr.min(), 1.0)
    speed = length / span
    m.update(corr=r, span=float(span), speed=speed, f0=float(fr.min()), f1=float(fr.max()),
             x0=float(mx + ux * pr.min()), y0=float(my + uy * pr.min()),
             x1=float(mx + ux * pr.max()), y1=float(my + uy * pr.max()))
    min_speed = P["min_speed"] * REF_FPS / min(max(fps, 1.0), 60.0)
    if m["lin"] < P["min_lin"] or length < P["min_len"]:
        return m, "shape"
    if abs(r) < P["min_corr"]:
        return m, "time_corr"
    if speed < min_speed:
        return m, "speed"
    return m, None


def detect(maxpix, maxfr, ave, std, pmask, fps, P):
    """Mirror of ff_detect_meteor. Returns (clusters, info).
    Each cluster: metrics + 'reject' (None = accepted)."""
    mp, av = maxpix.astype(np.float32), ave.astype(np.float32)
    sd = np.maximum(std.astype(np.float32), 1.0)
    ex = mp - av
    flag = (ex > P["sigma_k"] * sd) & (ex > P["min_excess"]) & ~pmask
    total = int(flag.sum())
    info = dict(total=total)
    if total > P["max_pixels"]:
        info["skip"] = "flood"
        return [], info
    if total < P["min_pixels"]:
        return [], info
    grow = ndi.binary_dilation(flag, np.ones((3, 3), bool))
    lab, n = ndi.label(grow, np.ones((3, 3), int))
    out = []
    objs = ndi.find_objects(lab)
    for k, sl in enumerate(objs, 1):
        if sl is None:
            continue
        sub = (lab[sl] == k) & flag[sl]
        cnt = int(sub.sum())
        if cnt < P["min_pixels"]:
            continue
        ys, xs = np.nonzero(sub)
        fr = maxfr[sl][sub].astype(np.float32)
        m, rej = analyze(xs.astype(np.float64) + sl[1].start, ys.astype(np.float64) + sl[0].start,
                         fr.astype(np.float64), fps, P)
        m["reject"] = rej
        out.append(m)
    out.sort(key=lambda c: -c["pixels"])
    return out, info


def read_block(path):
    with fits.open(path, memmap=False) as h:
        fps = h[0].header.get("FPS") or 25.0
        return h[0].data, h[1].data, h[2].data, h[3].data, float(fps)


def block_name(path):
    return os.path.basename(path)[3:-5]


def _work(args):
    path, P, pmask = args
    try:
        mp, mf, av, sd, fps = read_block(path)
        cl, info = detect(mp, mf, av, sd, pmask, fps, P)
    except Exception as e:  # unreadable / partial file
        return dict(block=block_name(path), path=path, error=str(e))
    return dict(block=block_name(path), path=path, fps=fps, total=info["total"],
                skip=info.get("skip"), clusters=cl)


def files_under(root, hours=None):
    pat = os.path.join(root, "*", "FF_*.fits") if hours is None else None
    fs = []
    for h in (hours or sorted(os.listdir(root))):
        fs += sorted(glob.glob(os.path.join(root, h, "FF_*.fits")))
    return fs


def cmd_validate(a):
    pm = pixel_mask(load_mask())
    fs = files_under(a.root, a.hours or None)
    ok = bad = 0
    for p in fs:
        try:
            j = json.load(open(p[:-5] + ".json", encoding="utf-8"))
        except (OSError, ValueError) as e:
            print(f"SKIP {block_name(p)}: sidecar unreadable ({e.__class__.__name__})")
            continue
        want = j.get("ff_meteors", [])
        mp, mf, av, sd, fps = read_block(p)
        if j.get("mixed_fps"):
            continue
        fps = j.get("fps") if j.get("fps", 0) > 1 else fps
        cl, _ = detect(mp, mf, av, sd, pm, fps, STRICT)
        got = [c for c in cl if c["reject"] is None][:8]
        same = len(got) == len(want)
        if same and want:
            same = abs(got[0]["length"] - want[0]["length"]) < 0.5 and got[0]["pixels"] == want[0]["pixels"]
        ok += same
        bad += not same
        if not same:
            print(f"MISMATCH {block_name(p)}: sidecar {len(want)} (px {[w['pixels'] for w in want]}) "
                  f"python {len(got)} (px {[g['pixels'] for g in got]})")
    print(f"validate: {ok} match, {bad} mismatch of {ok + bad}")


def cmd_scan(a):
    P = dict(RELAXED if a.relaxed else STRICT)
    for kv in a.set or []:
        k, v = kv.split("=")
        P[k] = type(P[k])(float(v))
    pm = pixel_mask(load_mask())
    fs = files_under(a.root, a.hours or None)
    print(f"{len(fs)} blocks, params {P}")
    n_acc = n_near = n_flood = n_err = 0
    with open(a.out, "w", encoding="utf-8") as fo, ProcessPoolExecutor(max_workers=a.jobs) as ex:
        for i, r in enumerate(ex.map(_work, [(f, P, pm) for f in fs], chunksize=8), 1):
            if "error" in r:
                n_err += 1
                continue
            n_flood += bool(r["skip"])
            keep = [c for c in r["clusters"]]
            if keep:
                n_acc += any(c["reject"] is None for c in keep)
                fo.write(json.dumps(dict(r, clusters=keep)) + "\n")
            if i % 500 == 0:
                print(f"  {i}/{len(fs)}", flush=True)
    print(f"done: blocks with an accepted cluster {n_acc}, flood-skipped {n_flood}, errors {n_err} -> {a.out}")


def cmd_sheet(a):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    rows = [json.loads(l) for l in open(a.cands, encoding="utf-8")]
    items = []
    for r in rows:
        for c in r["clusters"]:
            if a.accepted_only and c.get("reject") is not None:
                continue
            if c.get("pixels", 0) >= a.min_pixels and "x0" in c:
                # rank: long, linear, time-ordered, enough pixels
                score = min(c["length"], 80) * min(abs(c["corr"]), 1) * min(c["lin"], 50) ** 0.5
                items.append((score, r, c))
    items.sort(key=lambda t: -t[0])
    items = items[:a.top]
    cols = 6
    nr = (len(items) + cols - 1) // cols
    fig, axs = plt.subplots(nr, cols, figsize=(cols * 3.2, nr * 3.4))
    for ax in np.atleast_1d(axs).flat:
        ax.axis("off")
    for ax, (s, r, c) in zip(np.atleast_1d(axs).flat, items):
        mp, mf, av, sd, fps = read_block(r["path"])
        cx, cy = int((c["x0"] + c["x1"]) / 2), int((c["y0"] + c["y1"]) / 2)
        x0 = max(0, min(W - 100, cx - 50)); y0 = max(0, min(H - 100, cy - 50))
        d = mp.astype(float)[y0:y0 + 100, x0:x0 + 100] - av.astype(float)[y0:y0 + 100, x0:x0 + 100]
        ax.imshow(d, cmap="gray", vmin=0, vmax=max(40, np.percentile(d, 99.8)), extent=(x0, x0 + 100, y0 + 100, y0))
        ax.plot([c["x0"], c["x1"]], [c["y0"], c["y1"]], "r-", lw=0.5, alpha=0.5)
        ax.set_title(f"{r['block'][9:]} [{c['reject'] or 'ACCEPT'}]\nL{c['length']:.0f} px{c['pixels']} r{c['corr']:.2f} lin{c['lin']:.0f}", fontsize=7)
        ax.axis("on"); ax.tick_params(labelsize=5)
    plt.tight_layout(); plt.savefig(a.out, dpi=100)
    print(f"{len(items)} candidates -> {a.out}")


# ---------------------------------------------------------------------------
# cache + confidence tiers
# ---------------------------------------------------------------------------
# A cluster's metrics depend on the excess level (which pixels are flagged) but
# the other gates are cheap comparisons on those metrics. So: compute the
# clusters once per excess level with all gates open (cache), then evaluate any
# number of gate sets in memory.
EXCESS_LEVELS = (15.0, 12.0)
OPEN = dict(STRICT, min_pixels=8, min_lin=0.0, min_len=0.0, min_corr=0.0, min_speed=0.0)

# confidence tiers, strictest first. A = the current detector (excess 15 only,
# no edge exclusion, as in ff.rs). The rest add leniency; a block gets the best
# tier any of its clusters reaches.
TIERS = [
    ("A", dict(excess=(15.0,), min_pixels=12, min_lin=5.0, min_len=12.0, min_corr=0.8, min_speed=0.8, edge=0)),
    ("B", dict(excess=(15.0, 12.0), min_pixels=12, min_lin=5.0, min_len=12.0, min_corr=0.7, min_speed=0.8, edge=8)),
    ("C", dict(excess=(15.0, 12.0), min_pixels=8, min_lin=3.0, min_len=8.0, min_corr=0.5, min_speed=0.4, edge=8)),
]


def _cache_work(args):
    path, pmask = args
    try:
        mp, mf, av, sd, fps = read_block(path)
        lv = {}
        for ex in EXCESS_LEVELS:
            cl, info = detect(mp, mf, av, sd, pmask, fps, dict(OPEN, min_excess=ex))
            lv[str(ex)] = dict(total=info["total"], skip=info.get("skip"),
                               clusters=[{k: v for k, v in c.items() if k != "reject"} for c in cl])
        return dict(block=block_name(path), path=path, fps=fps, levels=lv)
    except Exception as e:
        return dict(block=block_name(path), path=path, error=str(e))


def cmd_cache(a):
    pm = pixel_mask(load_mask())
    fs = []
    for root in a.roots:
        fs += files_under(root, a.hours or None)
    print(f"{len(fs)} blocks", flush=True)
    with open(a.out, "w", encoding="utf-8") as fo, ProcessPoolExecutor(max_workers=a.jobs) as ex:
        for i, r in enumerate(ex.map(_cache_work, [(f, pm) for f in fs], chunksize=8), 1):
            fo.write(json.dumps(r) + "\n")
            if i % 500 == 0:
                print(f"  {i}/{len(fs)}", flush=True)
    print("cache ->", a.out)


def tier_events(rec, tiers=TIERS):
    """Accepted clusters of one cached block, each with its best tier."""
    fps = rec.get("fps") or 25.0
    ev = []
    for name, T in tiers:
        ms = T["min_speed"] * REF_FPS / min(max(fps, 1.0), 60.0)
        for ex in T["excess"]:
            lv = rec["levels"][str(ex)]
            if lv["skip"] or lv["total"] < T["min_pixels"]:
                continue
            for c in lv["clusters"]:
                if "x0" not in c or c["pixels"] < T["min_pixels"] or c["lin"] < T["min_lin"] \
                        or c["length"] < T["min_len"] or abs(c["corr"]) < T["min_corr"] or c["speed"] < ms:
                    continue
                e = T["edge"]
                if e and (min(c["x0"], c["x1"]) < e or max(c["x0"], c["x1"]) > 639 - e
                          or min(c["y0"], c["y1"]) < e or max(c["y0"], c["y1"]) > 359 - e):
                    continue
                ev.append(dict(tier=name, excess=ex, **c))
    # keep the best tier per place (centre within 15 px)
    ev.sort(key=lambda e: e["tier"])
    out = []
    for e in ev:
        cx, cy = (e["x0"] + e["x1"]) / 2, (e["y0"] + e["y1"]) / 2
        if all(abs(cx - (o["x0"] + o["x1"]) / 2) > 15 or abs(cy - (o["y0"] + o["y1"]) / 2) > 15 for o in out):
            out.append(e)
    return out


def cmd_tiers(a):
    recs = [json.loads(l) for l in open(a.cache, encoding="utf-8")]
    recs = [r for r in recs if "error" not in r]
    labels = {}
    if a.labels:
        for root in a.labels:
            p = os.path.join(root, "review.csv")
            if os.path.exists(p):
                import csv
                labels.update({r[0][3:]: r[1] for r in csv.reader(open(p, newline="")) if len(r) >= 2})
    truth = set(a.true or [])
    # blocks in a detections dir are meteors unless labelled "false"
    def is_true(r):
        b = r["block"]
        if b in labels:
            return labels[b] == "good"
        if b[9:] in truth or b in truth:
            return True
        return "detections" in r["path"].replace("\\", "/") and b not in labels
    n_true = sum(is_true(r) for r in recs)
    print(f"{len(recs)} blocks, {n_true} known-true blocks")
    best = {}
    for r in recs:
        ev = tier_events(r)
        if ev:
            best[r["block"]] = (min(e["tier"] for e in ev), ev)
    print("\ntier (cumulative) | blocks flagged | true | false-pos | recall of known-true")
    tp_names = {r["block"] for r in recs if is_true(r)}
    for k, (name, _) in enumerate(TIERS):
        ok = {"ABC"[i] for i in range(k + 1)}
        flagged = {b for b, (t, _) in best.items() if t in ok}
        tp = flagged & tp_names
        print(f"  <= {name}           | {len(flagged):4d}           | {len(tp):3d}  | {len(flagged - tp):3d}       | {len(tp)}/{n_true} = {100 * len(tp) / max(n_true, 1):.0f}%")
    print("\nfalse positives by tier (block, tier, L, px, corr, pos):")
    for b, (t, ev) in sorted(best.items(), key=lambda kv: (kv[1][0], kv[0])):
        if b in tp_names:
            continue
        e = sorted(ev, key=lambda e: e["tier"])[0]
        print(f"  {t} {b[9:]} L={e['length']:.0f} px={e['pixels']} corr={e['corr']:.2f} lin={e['lin']:.0f} ({e['x0']:.0f},{e['y0']:.0f})->({e['x1']:.0f},{e['y1']:.0f}) exc={e['excess']:.0f}")
    print("\nknown-true blocks NOT flagged by any tier:")
    for r in recs:
        if is_true(r) and r["block"] not in best:
            print("  ", r["block"][9:])
    print("\nknown-true blocks by their best tier:", {t: sum(1 for b in tp_names if best.get(b, ("-",))[0] == t) for t in "ABC-"})


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("validate"); v.add_argument("root"); v.add_argument("hours", nargs="*")
    s = sub.add_parser("scan"); s.add_argument("root"); s.add_argument("hours", nargs="*")
    s.add_argument("--out", default="cand.jsonl"); s.add_argument("--relaxed", action="store_true")
    s.add_argument("--set", nargs="*", help="override e.g. min_corr=0.7"); s.add_argument("--jobs", type=int, default=6)
    t = sub.add_parser("sheet"); t.add_argument("cands"); t.add_argument("--top", type=int, default=36)
    t.add_argument("--min-pixels", type=int, default=15); t.add_argument("--accepted-only", action="store_true"); t.add_argument("--out", default="sheet.png")
    c = sub.add_parser("cache"); c.add_argument("roots", nargs="+"); c.add_argument("--hours", nargs="*")
    c.add_argument("--out", default="cache.jsonl"); c.add_argument("--jobs", type=int, default=6)
    ti = sub.add_parser("tiers"); ti.add_argument("cache"); ti.add_argument("--labels", nargs="*", help="dirs holding review.csv")
    ti.add_argument("--true", nargs="*", help="extra known-true block suffixes, e.g. 001505_693")
    a = ap.parse_args()
    {"validate": cmd_validate, "scan": cmd_scan, "sheet": cmd_sheet, "cache": cmd_cache, "tiers": cmd_tiers}[a.cmd](a)
