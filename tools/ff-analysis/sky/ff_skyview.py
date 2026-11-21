"""Camera-frame view of FF meteors with shower radiants.

usage: python ff_skyview.py <ff dir> <hours...> [--tz 9] [--out skyview.png]

Everything is drawn in the camera's own pixel frame (the camera is fixed to
the Earth), using the WCS stored in each block's FITS.  So the tracks keep the
shape you saw in the images, and radiants appear where they really were.
  * background: mean of all blocks' AVEPIXEL (stars show up)
  * thick segment + arrow = observed meteor (numbered)
  * thin line = its great circle extended backwards (to where it came from)
  * star = a shower radiant where it was at the time of that block
  * the check printed at the top: pixel->sky with the FITS WCS vs the sidecar's
    own ra/dec (independent check of the Rust computation)
"""
import argparse
import glob
import json
import math
import os
import warnings
from datetime import datetime, timedelta, timezone

import numpy as np
import matplotlib.pyplot as plt
from astropy.io import fits
from astropy.wcs import WCS

warnings.filterwarnings("ignore")
ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("hours", nargs="*")
ap.add_argument("--tz", type=float, default=9.0)
ap.add_argument("--csv", default=r"E:\source\atomet\overlay_rootfs\usr\share\atomet\showers.csv")
ap.add_argument("--out", default="skyview.png")
a = ap.parse_args()

TOL_BASE, TOL_CAP = 3.0, 15.0


def vec(ra, dec):
    ra, dec = math.radians(ra), math.radians(dec)
    return np.array([math.cos(dec) * math.cos(ra), math.cos(dec) * math.sin(ra), math.sin(dec)])


def radec(v):
    return math.degrees(math.atan2(v[1], v[0])) % 360, math.degrees(math.asin(max(-1, min(1, v[2]))))


def wrap180(x):
    return (x + 180) % 360 - 180


def solar_lon(dt):
    n = (dt - datetime(2000, 1, 1, 12, tzinfo=timezone.utc)).total_seconds() / 86400
    g = math.radians(357.528 + 0.9856003 * n)
    return (280.460 + 0.9856474 * n + 1.915 * math.sin(g) + 0.020 * math.sin(2 * g)) % 360


showers = []
for line in open(a.csv, encoding="utf-8"):
    f = [s.strip() for s in line.split("|")]
    if line.startswith("#") or len(f) < 10:
        continue
    try:
        mx, ra, dec = float(f[4]), float(f[6]), float(f[8])
    except ValueError:
        continue
    showers.append(dict(code=f[1], begin=float(f[3]) if f[3] else mx - 5, max=mx,
                        end=float(f[5]) if f[5] else mx + 5, ra=ra, dra=float(f[7] or 0),
                        dec=dec, ddec=float(f[9] or 0)))


def radiants(lam):
    out = {}
    for s in showers:
        d = wrap180(lam - s["max"])
        if wrap180(s["begin"] - s["max"]) <= d <= wrap180(s["end"] - s["max"]):
            out[s["code"]] = ((s["ra"] + s["dra"] * d) % 360, s["dec"] + s["ddec"] * d)
    return out


mets, bg, blocks = [], [], []
for h in a.hours or sorted(os.listdir(a.root)):
    for p in sorted(glob.glob(os.path.join(a.root, h, "*.json"))):
        j = json.load(open(p, encoding="utf-8"))
        ms = [m for m in j.get("ff_meteors", []) if m.get("sky")]
        if not ms:
            continue
        with fits.open(p[:-5] + ".fits") as f:
            w = WCS(f[0].header)
            bg.append(f[2].data.astype(np.float32))
        bs = datetime.fromisoformat(j["block_start"]).replace(tzinfo=timezone(timedelta(hours=a.tz)))
        blocks.append((bs, w))
        for m in ms:
            when = bs + timedelta(seconds=m["sky"]["t0_s"])
            mets.append(dict(name=os.path.basename(p)[12:-5], when=when, m=m, w=w))

# ---- independent WCS check: FITS WCS pixel->sky vs the sidecar's own sky values
errs = []
for t in mets:
    m, s = t["m"], t["m"]["sky"]
    ra, dec = t["w"].wcs_pix2world([[m["x0"] + 1, m["y0"] + 1]], 1)[0]
    # sidecar end 0 is advanced by t0 of Earth rotation; FITS WCS is at block start
    ra_exp = (s["ra0"] - 360.0 * s["t0_s"] / 86164.09) % 360
    errs.append(math.hypot(wrap180(ra - ra_exp) * math.cos(math.radians(dec)), dec - s["dec0"]))
print(f"WCS check (FITS WCS vs sidecar): max diff {max(errs):.4f} deg, median {np.median(errs):.4f} deg")

# ---- association (same as showers.rs)
for t in mets:
    s = t["m"]["sky"]
    v0, v1 = vec(s["ra0"], s["dec0"]), vec(s["ra1"], s["dec1"])
    n = np.cross(v0, v1); n /= np.linalg.norm(n)
    t.update(v0=v0, v1=v1, n=n, lam=solar_lon(t["when"].astimezone(timezone.utc)))
    tol = min(TOL_BASE + 3 * math.degrees(1.0 / max(t["m"]["length"], 1.0)), TOL_CAP)
    t["tol"] = tol
    best = None
    for code, (ra, dec) in radiants(t["lam"]).items():
        r = vec(ra, dec)
        off = math.degrees(math.asin(min(1, abs(float(r @ n)))))
        behind = math.acos(np.clip(r @ v1, -1, 1)) > math.acos(np.clip(r @ v0, -1, 1))
        if behind and off <= tol and (best is None or off < best[1]):
            best = (code, off)
    t["best"] = best

codes = sorted({t["best"][0] for t in mets if t["best"]})
pal = dict(zip(codes, plt.cm.Set1.colors))
img = np.mean(bg, axis=0)

fig, ax = plt.subplots(figsize=(16, 9.4))
lo, hi = np.percentile(img, [2, 99.7])
ax.imshow(img, cmap="gray", vmin=lo, vmax=hi, origin="upper", extent=(-0.5, 639.5, 359.5, -0.5))


def to_pix(t, ra, dec):
    x, y = t["w"].wcs_world2pix([[ra, dec]], 0)[0]
    return x, y


for i, t in enumerate(mets, 1):
    col = pal[t["best"][0]] if t["best"] else "#888888"
    # back-projection along the great circle (up to 60 deg), clipped to the frame
    e1 = np.cross(t["n"], t["v0"])
    xy = []
    for dd in np.linspace(-60, 0, 120):
        v = math.cos(math.radians(dd)) * t["v0"] + math.sin(math.radians(dd)) * e1
        # undo the Earth rotation applied for this meteor's time (block-start WCS)
        ra, dec = radec(v)
        ra = (ra + 360.0 * t["m"]["sky"]["t0_s"] / 86164.09) % 360
        xy.append(to_pix(t, ra, dec))
    xy = np.array(xy)
    ok = (xy[:, 0] > -40) & (xy[:, 0] < 680) & (xy[:, 1] > -40) & (xy[:, 1] < 400)
    ax.plot(xy[ok, 0], xy[ok, 1], "--", color=col, lw=0.9, alpha=0.8)
    m = t["m"]
    ax.annotate("", xy=(m["x1"], m["y1"]), xytext=(m["x0"], m["y0"]),
                arrowprops=dict(arrowstyle="-|>", color=col, lw=2.6, mutation_scale=14))
    ax.text(m["x0"], m["y0"] - 5, str(i), color="w", fontsize=9, weight="bold", ha="center",
            bbox=dict(boxstyle="circle,pad=0.15", fc=col, ec="none", alpha=0.9))

# radiants at three times (block of the nearest time), joined to show diurnal drift
marks = [blocks[0], blocks[len(blocks) // 2], blocks[-1]]
for code in codes:
    pts = []
    for bs, w in marks:
        lam = solar_lon(bs.astimezone(timezone.utc))
        ra, dec = radiants(lam)[code]
        x, y = w.wcs_world2pix([[ra, dec]], 0)[0]
        pts.append((x, y, bs.strftime("%H:%M")))
    ax.plot([p[0] for p in pts], [p[1] for p in pts], "-", color=pal[code], lw=1, alpha=0.6)
    for x, y, lab in pts:
        if -60 < x < 700 and -60 < y < 420:
            ax.plot(x, y, "*", ms=18, color=pal[code], mec="k")
            ax.text(x + 6, y + 6, f"{code} {lab}", color=pal[code], fontsize=9, weight="bold")
ax.set_xlim(-60, 700); ax.set_ylim(420, -60)
ax.set_xlabel("x px"); ax.set_ylabel("y px")
ax.set_title(
    "Camera view, all FF meteors of the night overlaid (number = list order)\n"
    "arrow = observed meteor, dashed = back-projection (up to 60°), ★ = radiant position at start/mid/end of night.  "
    "grey = no shower match", fontsize=10)
handles = [plt.Line2D([], [], color=pal[c], lw=3, label=c) for c in codes] + \
          [plt.Line2D([], [], color="#888", lw=3, label="sporadic")]
ax.legend(handles=handles, loc="lower right", fontsize=9)
plt.tight_layout()
plt.savefig(a.out, dpi=110)
for i, t in enumerate(mets, 1):
    m = t["m"]
    print(f"{i:2d} {t['name']}  {t['when'].strftime('%H:%M:%S')}  len={m['length']:.0f}px/{m['sky']['length_deg']:.1f}deg  "
          f"{(t['best'][0] + ' off=%.1f' % t['best'][1]) if t['best'] else 'sporadic':<14} tol={t['tol']:.1f}")
print("saved", a.out)
