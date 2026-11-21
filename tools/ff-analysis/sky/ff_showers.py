"""Offline shower association + sky plot for FF sidecars.

usage: python ff_showers.py <ff dir> <hours...> [--tz 9] [--out out.png]
Same logic as crates/atometd/src/showers.rs (great-circle offset of the radiant,
radiant must lie behind the start point).  Uses the sky coordinates already in
the sidecar (sky.ra0/dec0/ra1/dec1) and the repo's showers.csv.
"""
import argparse
import glob
import json
import math
import os
from datetime import datetime, timedelta, timezone

import numpy as np
import matplotlib.pyplot as plt

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("hours", nargs="*")
ap.add_argument("--tz", type=float, default=9.0, help="UTC offset of block_start (hours)")
ap.add_argument("--csv", default=r"E:\source\atomet\overlay_rootfs\usr\share\atomet\showers.csv")
ap.add_argument("--out", default="showers.png")
a = ap.parse_args()

TOL_BASE, TOL_CAP, LIST_MAX = 3.0, 15.0, 30.0


def vec(ra, dec):
    ra, dec = math.radians(ra), math.radians(dec)
    return np.array([math.cos(dec) * math.cos(ra), math.cos(dec) * math.sin(ra), math.sin(dec)])


def radec(v):
    return math.degrees(math.atan2(v[1], v[0])) % 360, math.degrees(math.asin(max(-1, min(1, v[2]))))


def wrap180(x):
    return (x + 180) % 360 - 180


def solar_lon(dt_utc):
    n = (dt_utc - datetime(2000, 1, 1, 12, tzinfo=timezone.utc)).total_seconds() / 86400
    l = 280.460 + 0.9856474 * n
    g = math.radians(357.528 + 0.9856003 * n)
    return (l + 1.915 * math.sin(g) + 0.020 * math.sin(2 * g)) % 360


showers = []
for line in open(a.csv, encoding="utf-8"):
    if line.startswith("#") or "|" not in line:
        continue
    f = [s.strip() for s in line.split("|")]
    if len(f) < 10:
        continue
    num = lambda s: float(s) if s not in ("", None) else None
    try:
        mx, ra, dec = float(f[4]), float(f[6]), float(f[8])
    except ValueError:
        continue
    showers.append(dict(code=f[1], name=f[2], begin=num(f[3]) if f[3] else mx - 5, max=mx,
                        end=num(f[5]) if f[5] else mx + 5, ra=ra, dra=float(f[7] or 0),
                        dec=dec, ddec=float(f[9] or 0)))

# --- load meteors
mets = []
for h in a.hours or sorted(os.listdir(a.root)):
    for p in sorted(glob.glob(os.path.join(a.root, h, "*.json"))):
        j = json.load(open(p, encoding="utf-8"))
        bs = datetime.fromisoformat(j["block_start"]).replace(tzinfo=timezone(timedelta(hours=a.tz)))
        for m in j.get("ff_meteors", []):
            s = m.get("sky")
            if not s:
                continue
            when = bs + timedelta(seconds=s["t0_s"])
            mets.append(dict(name=os.path.basename(p)[3:-5], when=when, m=m, s=s,
                             v0=vec(s["ra0"], s["dec0"]), v1=vec(s["ra1"], s["dec1"])))
print(f"{len(mets)} meteors with sky coords")

# --- association
for t in mets:
    lam = solar_lon(t["when"].astimezone(timezone.utc))
    t["lam"] = lam
    L = t["m"]["length"]
    tol = min(TOL_BASE + 3 * math.degrees(1.0 / max(L, 1.0)), TOL_CAP)
    t["tol"] = tol
    n = np.cross(t["v0"], t["v1"])
    n /= np.linalg.norm(n)
    t["n"] = n
    res = []
    for s in showers:
        d = wrap180(lam - s["max"])
        if d < wrap180(s["begin"] - s["max"]) or d > wrap180(s["end"] - s["max"]):
            continue
        ra = (s["ra"] + s["dra"] * d) % 360
        dec = max(-90, min(90, s["dec"] + s["ddec"] * d))
        r = vec(ra, dec)
        off = math.degrees(math.asin(min(1, abs(float(r @ n)))))
        if off > LIST_MAX:
            continue
        behind = math.acos(np.clip(r @ t["v1"], -1, 1)) > math.acos(np.clip(r @ t["v0"], -1, 1))
        res.append(dict(code=s["code"], name=s["name"], ra=ra, dec=dec, off=off, behind=behind,
                        hit=behind and off <= tol))
    res.sort(key=lambda x: x["off"])
    t["res"] = res
    t["best"] = next((x for x in res if x["hit"]), None)

print(f"solar longitude {mets[0]['lam']:.2f}..{mets[-1]['lam']:.2f}")
print(f"{'block':<16}{'len°':>6}{'°/s':>6}{'tol':>5}  best hit / nearest radiants (offset°, behind)")
for t in mets:
    near = ", ".join(f"{x['code']}({x['off']:.1f}{'' if x['behind'] else ',front'})" for x in t["res"][:4])
    b = t["best"]
    print(f"{t['name']:<16}{t['s']['length_deg']:>6.1f}{t['s']['speed_deg_s']:>6.1f}{t['tol']:>5.1f}  "
          f"{(b['code'] if b else '-'):<5} | {near}")
hits = {}
for t in mets:
    k = t["best"]["code"] if t["best"] else "sporadic"
    hits[k] = hits.get(k, 0) + 1
print("summary:", hits)

# --- plot: RA/Dec map, RA increasing to the left, centred on the data
ras = np.array([radec(t["v0"])[0] for t in mets])
c = math.degrees(math.atan2(np.sin(np.radians(ras)).mean(), np.cos(np.radians(ras)).mean())) % 360
rel = lambda ra: wrap180(ra - c)

fig, ax = plt.subplots(figsize=(14, 8))
colors = plt.cm.tab10.colors
codes = sorted({t["best"]["code"] for t in mets if t["best"]})
cmap = {k: colors[i % 10] for i, k in enumerate(codes)}
for t in mets:
    col = cmap.get(t["best"]["code"], "0.5") if t["best"] else "0.5"
    # great circle: 70 deg back from start, a bit past the end
    e1 = np.cross(t["n"], t["v0"])
    pts = []
    L = t["s"]["length_deg"]
    for dd in np.linspace(-70, L + 10, 200):
        v = math.cos(math.radians(dd)) * t["v0"] + math.sin(math.radians(dd)) * e1
        pts.append(radec(v))
    pts = np.array(pts)
    x = np.array([rel(r) for r in pts[:, 0]])
    y = pts[:, 1]
    br = np.where(np.abs(np.diff(x)) > 180)[0] + 1
    for xs, ys in zip(np.split(x, br), np.split(y, br)):
        ax.plot(-xs, ys, "-", color=col, lw=0.6, alpha=0.5)
    r0, d0 = radec(t["v0"]); r1, d1 = radec(t["v1"])
    ax.plot([-rel(r0), -rel(r1)], [d0, d1], "-", color=col, lw=3)
    ax.annotate("", xy=(-rel(r1), d1), xytext=(-rel(r0), d0),
                arrowprops=dict(arrowstyle="->", color=col, lw=1.5))
# radiants active at the mean solar longitude
lam_mid = float(np.mean([t["lam"] for t in mets]))
for s in showers:
    d = wrap180(lam_mid - s["max"])
    if d < wrap180(s["begin"] - s["max"]) or d > wrap180(s["end"] - s["max"]):
        continue
    ra = (s["ra"] + s["dra"] * d) % 360
    dec = s["dec"] + s["ddec"] * d
    used = s["code"] in codes
    ax.plot(-rel(ra), dec, "*", ms=14 if used else 9, color=cmap.get(s["code"], "k"),
            mec="k", mew=0.6, zorder=5)
    ax.annotate(s["code"], (-rel(ra), dec), xytext=(4, 4), textcoords="offset points", fontsize=8, weight="bold" if used else None)
ax.set_xlim(-130, 130); ax.set_ylim(-60, 90)
ax.set_xticks(np.arange(-120, 121, 30))
ax.set_xticklabels([f"{(c - x) % 360:.0f}°" for x in np.arange(-120, 121, 30)])
ax.set_xlabel("RA (increasing to the left)"); ax.set_ylabel("Dec")
ax.grid(alpha=0.3)
ax.set_title(f"FF meteors {mets[0]['when'].date()}  λ☉={lam_mid:.1f}°  "
             f"thick=observed (arrow=direction), thin=great circle back-projected, ★=active radiants  "
             + ", ".join(f"{k}:{v}" for k, v in hits.items()), fontsize=9)
plt.tight_layout()
plt.savefig(a.out, dpi=110)
print("saved", a.out)

# ---------------- polar (north celestial pole) view ----------------
def pol(ra, dec):
    r = 90 - dec
    return r * math.cos(math.radians(ra)), r * math.sin(math.radians(ra))

fig2, ax2 = plt.subplots(figsize=(10, 10))
for rr in (30, 60, 90):
    ax2.add_patch(plt.Circle((0, 0), rr, fill=False, color="0.8", lw=0.6))
for ra in range(0, 360, 30):
    x, y = pol(ra, -10)
    ax2.plot([0, x], [0, y], color="0.9", lw=0.5)
    x, y = pol(ra, -18)
    ax2.text(x, y, f"{ra // 15}h", ha="center", va="center", fontsize=8, color="0.4")
for t in mets:
    col = cmap.get(t["best"]["code"], "0.5") if t["best"] else "0.5"
    e1 = np.cross(t["n"], t["v0"])
    pts = np.array([radec(math.cos(math.radians(dd)) * t["v0"] + math.sin(math.radians(dd)) * e1)
                    for dd in np.linspace(-70, t["s"]["length_deg"] + 5, 150)])
    xy = np.array([pol(r, d) for r, d in pts])
    ok = pts[:, 1] > -10
    ax2.plot(xy[ok, 0], xy[ok, 1], "-", color=col, lw=0.6, alpha=0.6)
    (x0, y0), (x1, y1) = pol(*radec(t["v0"])), pol(*radec(t["v1"]))
    ax2.annotate("", xy=(x1, y1), xytext=(x0, y0), arrowprops=dict(arrowstyle="-|>", color=col, lw=2.5))
for s in showers:
    d = wrap180(lam_mid - s["max"])
    if d < wrap180(s["begin"] - s["max"]) or d > wrap180(s["end"] - s["max"]):
        continue
    ra = (s["ra"] + s["dra"] * d) % 360; dec = s["dec"] + s["ddec"] * d
    x, y = pol(ra, dec)
    ax2.plot(x, y, "*", ms=15, color=cmap.get(s["code"], "w"), mec="k", zorder=5)
    ax2.text(x + 2, y + 2, s["code"], fontsize=9, weight="bold")
ax2.plot(0, 0, "k+", ms=10)
ax2.set_xlim(-100, 100); ax2.set_ylim(-100, 100); ax2.set_aspect("equal"); ax2.axis("off")
ax2.set_title("North celestial pole view (centre = Dec +90, ring = Dec 0 at r=90)\n"
              "thick arrow = observed, thin = back-projected great circle, ★ = active radiants", fontsize=10)
plt.tight_layout(); fig2.savefig("showers_polar.png", dpi=100)

# ---------------- chance-association baseline ----------------
rng = np.random.default_rng(1)
active = []
for s in showers:
    d = wrap180(lam_mid - s["max"])
    if wrap180(s["begin"] - s["max"]) <= d <= wrap180(s["end"] - s["max"]):
        active.append(vec((s["ra"] + s["dra"] * d) % 360, s["dec"] + s["ddec"] * d))
NS = 4000
exp_total = 0.0
for t in mets:
    L = math.radians(t["s"]["length_deg"])
    hit = 0
    for _ in range(NS):
        p = t["v0"]  # keep the real start point; only the heading is randomised
        q = rng.normal(size=3); q -= (q @ p) * p; q /= np.linalg.norm(q)  # random heading
        v0 = p; v1 = math.cos(L) * p + math.sin(L) * q
        n = np.cross(v0, v1); n /= np.linalg.norm(n)
        for r in active:
            if abs(r @ n) <= math.sin(math.radians(t["tol"])) and \
               math.acos(np.clip(r @ v1, -1, 1)) > math.acos(np.clip(r @ v0, -1, 1)):
                hit += 1; break
    exp_total += hit / NS
n_hit = sum(1 for t in mets if t["best"])
print(f"chance baseline: expect {exp_total:.1f} of {len(mets)} to 'hit' some active shower by luck; observed {n_hit}")
