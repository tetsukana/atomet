"""One night: tier statistics over time + shower association + plots.

usage:
  python ff_night.py cache_a.jsonl [cache_b.jsonl ...] --from 20261118_172430 --to 20261119_014240 --out DIR
                     [--tz 9] [--lat 35.6812] [--lon 139.7671] [--min-tier C]

Input is the output of `ff_offline.py cache` (clusters per block at excess 15/12).
Events are assigned a tier with ff_offline.TIERS (A best). For events whose block FITS carries a WCS the
pixel ends are converted to RA/Dec (the camera turns with the Earth: RA += 360*t/86164 s), and each event is
matched to the shower list (same rule as crates/atometd/src/showers.rs: the radiant must lie within a tolerance of the
great circle and *behind* the start point).

Writes into DIR: events.csv, tiers_by_hour.csv, night_timeline.png, night_showers.png, night_sky_polar.png
"""
import argparse
import csv
import json
import math
import os
import sys
import warnings
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone

import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from astropy.io import fits
from astropy.wcs import WCS

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ff_offline as F  # noqa: E402

warnings.filterwarnings("ignore")
ap = argparse.ArgumentParser()
ap.add_argument("caches", nargs="+")
ap.add_argument("--from", dest="t_from", required=True)
ap.add_argument("--to", dest="t_to", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--tz", type=float, default=9.0)
ap.add_argument("--lat", type=float, default=35.6812)
ap.add_argument("--lon", type=float, default=139.7671)
ap.add_argument("--csv", default=r"E:\source\atomet\overlay_rootfs\usr\share\atomet\showers.csv")
ap.add_argument("--min-tier", default="C", choices=["A", "B", "C"])
ap.add_argument("--c-corr", type=float, default=0.0, help="drop tier-C events with |corr| below this")
ap.add_argument("--c-speed", type=float, default=0.0, help="drop tier-C events slower than this (px/frame)")
ap.add_argument("--max-sun-alt", type=float, default=-12.0,
                help="shower association / sky plots only for events with the sun below this altitude (deg)")
ap.add_argument("--one-per-block", action="store_true",
                help="keep only the best (tier, then longest) event per block: a dotted streak is often split into pieces")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)
TZ = timezone(timedelta(hours=a.tz))
SID = 86164.09
TIER_COL = {"A": "#d62728", "B": "#ff7f0e", "C": "#7f7f7f"}


def vec(ra, dec):
    ra, dec = math.radians(ra), math.radians(dec)
    return np.array([math.cos(dec) * math.cos(ra), math.cos(dec) * math.sin(ra), math.sin(dec)])


def radec(v):
    return math.degrees(math.atan2(v[1], v[0])) % 360, math.degrees(math.asin(max(-1, min(1, v[2]))))


def wrap(x):
    return (x + 180) % 360 - 180


def jd(dt):
    return dt.timestamp() / 86400 + 2440587.5


def solar_lon(dt):
    n = jd(dt) - 2451545.0
    g = math.radians(357.528 + 0.9856003 * n)
    return (280.460 + 0.9856474 * n + 1.915 * math.sin(g) + 0.020 * math.sin(2 * g)) % 360


def sun_alt(dt):
    n = jd(dt) - 2451545.0
    L = (280.460 + 0.9856474 * n) % 360
    g = math.radians((357.528 + 0.9856003 * n) % 360)
    lam = math.radians(L + 1.915 * math.sin(g) + 0.020 * math.sin(2 * g))
    eps = math.radians(23.439 - 0.0000004 * n)
    ra = math.atan2(math.cos(eps) * math.sin(lam), math.cos(lam))
    dec = math.asin(math.sin(eps) * math.sin(lam))
    gmst = (280.46061837 + 360.98564736629 * n) % 360
    ha = math.radians(gmst + a.lon) - ra
    lat = math.radians(a.lat)
    return math.degrees(math.asin(math.sin(lat) * math.sin(dec) + math.cos(lat) * math.cos(dec) * math.cos(ha)))


# ---------------------------------------------------------------- showers
showers = []
for line in open(a.csv, encoding="utf-8"):
    f = [s.strip() for s in line.split("|")]
    if line.startswith("#") or len(f) < 10:
        continue
    try:
        mx, ra, dec = float(f[4]), float(f[6]), float(f[8])
    except ValueError:
        continue
    showers.append(dict(code=f[1], name=f[2], begin=float(f[3]) if f[3] else mx - 5, max=mx,
                        end=float(f[5]) if f[5] else mx + 5, ra=ra, dra=float(f[7] or 0),
                        dec=dec, ddec=float(f[9] or 0)))


def radiant_alt(ra, dec, dt):
    """Altitude of (ra, dec) at the station at time dt (deg)."""
    n = jd(dt) - 2451545.0
    lst = (280.46061837 + 360.98564736629 * n + a.lon) % 360
    ha = math.radians(lst - ra)
    lat, d_ = math.radians(a.lat), math.radians(dec)
    return math.degrees(math.asin(math.sin(lat) * math.sin(d_) + math.cos(lat) * math.cos(d_) * math.cos(ha)))


def active(lam, dt=None, min_alt=0.0):
    """Radiants of the showers active at solar longitude `lam`; with `dt`, only those above `min_alt`
    (a shower whose radiant is below the horizon cannot produce meteors seen from here)."""
    out = {}
    for s in showers:
        d = wrap(lam - s["max"])
        if wrap(s["begin"] - s["max"]) <= d <= wrap(s["end"] - s["max"]):
            ra, dec = (s["ra"] + s["dra"] * d) % 360, s["dec"] + s["ddec"] * d
            if dt is not None and radiant_alt(ra, dec, dt) < min_alt:
                continue
            out[s["code"]] = vec(ra, dec)
    return out


# ---------------------------------------------------------------- events
recs = []
for c in a.caches:
    for l in open(c, encoding="utf-8"):
        r = json.loads(l)
        if "error" in r:
            continue
        if a.t_from <= r["block"] <= a.t_to:
            recs.append(r)
recs.sort(key=lambda r: r["block"])
print(f"{len(recs)} blocks in window {a.t_from}..{a.t_to}")
tier_order = "ABC"
events = []


def block_time(r):
    return datetime.strptime(r["block"][:15], "%Y%m%d_%H%M%S").replace(tzinfo=TZ) + timedelta(milliseconds=int(r["block"][16:19]))


def header_of(path):
    try:
        return fits.getheader(path, 0)
    except Exception:
        return None


def get_wcs(i):
    """WCS of block i. If it has none (the plate solver was not running), borrow the nearest block's WCS and
    turn it with the Earth: the camera is fixed, so only the RA of the reference point changes (360 deg per
    sidereal day). Returns (WCS or None, synthesized?)."""
    h = header_of(recs[i]["path"])
    if h is not None and "CTYPE1" in h:
        return WCS(h), False
    t = block_time(recs[i])
    for off in range(1, len(recs)):
        for j in (i - off, i + off):
            if 0 <= j < len(recs):
                hj = header_of(recs[j]["path"])
                if hj is not None and "CTYPE1" in hj:
                    hj = hj.copy()
                    dt = (t - block_time(recs[j])).total_seconds()
                    hj["CRVAL1"] = (hj["CRVAL1"] + 360.0 * dt / SID) % 360
                    return WCS(hj), True
    return None, False


for i_rec, r in enumerate(recs):
    evs = [e for e in F.tier_events(r) if tier_order.index(e["tier"]) <= tier_order.index(a.min_tier)
           and not (e["tier"] == "C" and (abs(e["corr"]) < a.c_corr or e["speed"] < a.c_speed))]
    if a.one_per_block and evs:
        evs = [sorted(evs, key=lambda e: (e["tier"], -e["length"]))[0]]
    if not evs:
        continue
    bs = datetime.strptime(r["block"][:15], "%Y%m%d_%H%M%S").replace(tzinfo=TZ) + timedelta(milliseconds=int(r["block"][16:19]))
    w, synth = get_wcs(i_rec)
    for e in evs:
        (xa, ya), (xb, yb) = ((e["x1"], e["y1"]), (e["x0"], e["y0"])) if e["corr"] < 0 else ((e["x0"], e["y0"]), (e["x1"], e["y1"]))
        d = dict(block=r["block"], time=bs + timedelta(seconds=e["f0"] / (r["fps"] or 25.0)), tier=e["tier"],
                 length=e["length"], pixels=e["pixels"], corr=e["corr"], span=e["span"], speed=e["speed"],
                 xa=xa, ya=ya, xb=xb, yb=yb, f0=e["f0"], f1=e["f1"], fps=r["fps"] or 25.0, wcs=w is not None, synth=synth)
        d["sun_alt"] = sun_alt(d["time"])
        if w is not None and d["sun_alt"] <= a.max_sun_alt:
            def sky(x, y, f):
                ra, dec = w.wcs_pix2world([[x + 1, y + 1]], 1)[0]
                return vec((ra + 360 * (f / d["fps"]) / SID) % 360, dec)
            v0, v1 = sky(xa, ya, e["f0"]), sky(xb, yb, e["f1"])
            n = np.cross(v0, v1)
            if np.linalg.norm(n) > 1e-9:
                n /= np.linalg.norm(n)
                d.update(v0=v0, v1=v1, n=n, len_deg=math.degrees(math.acos(np.clip(v0 @ v1, -1, 1))),
                         dur=max((e["f1"] - e["f0"]) / d["fps"], 1 / d["fps"]))
                d["speed_deg_s"] = d["len_deg"] / d["dur"]
                d["lam"] = solar_lon(d["time"].astimezone(timezone.utc))
                d["tol"] = min(3.0 + 3 * math.degrees(1.0 / max(e["length"], 1.0)), 15.0)
                best = None
                for code, rv in active(d["lam"], d["time"]).items():
                    off = math.degrees(math.asin(min(1, abs(float(rv @ n)))))
                    behind = math.acos(np.clip(rv @ v1, -1, 1)) > math.acos(np.clip(rv @ v0, -1, 1))
                    if behind and off <= d["tol"] and (best is None or off < best[1]):
                        best = (code, off)
                d["shower"], d["offset"] = best if best else ("SPO", float("nan"))
        events.append(d)
print(f"{len(events)} events (<= tier {a.min_tier}); with sky coordinates: {sum('n' in e for e in events)}"
      f" ({sum(e.get('synth', False) and 'n' in e for e in events)} from a borrowed WCS)")

# ---------------------------------------------------------------- per-hour table
t_start = datetime.strptime(a.t_from[:15], "%Y%m%d_%H%M%S").replace(tzinfo=TZ)
hours = defaultdict(lambda: dict(blocks=0, A=0, B=0, C=0))
for r in recs:
    t = datetime.strptime(r["block"][:15], "%Y%m%d_%H%M%S")
    hours[t.strftime("%m-%d %H")]["blocks"] += 1
for e in events:
    hours[e["time"].strftime("%m-%d %H")][e["tier"]] += 1
with open(os.path.join(a.out, "tiers_by_hour.csv"), "w", newline="", encoding="utf-8") as f:
    w_ = csv.writer(f)
    w_.writerow(["hour_JST", "blocks", "A", "B", "C", "sun_alt_deg(mid)"])
    for k in sorted(hours):
        mid = datetime.strptime("2026-" + k, "%Y-%m-%d %H").replace(tzinfo=TZ) + timedelta(minutes=30)
        h = hours[k]
        w_.writerow([k, h["blocks"], h["A"], h["B"], h["C"], f"{sun_alt(mid):.1f}"])
        print(f"{k}h  blocks={h['blocks']:4d}  A={h['A']:3d} B={h['B']:3d} C={h['C']:3d}  sun {sun_alt(mid):6.1f} deg")

# ---------------------------------------------------------------- chance baseline (per tier)
rng = np.random.default_rng(1)


def chance_hits(e, N=1500):
    p = e["v0"]
    L = math.radians(e["len_deg"])
    q = rng.normal(size=(N, 3))
    q -= (q @ p)[:, None] * p
    q /= np.linalg.norm(q, axis=1)[:, None]
    v1 = math.cos(L) * p + math.sin(L) * q
    n = np.cross(p, v1)
    n /= np.linalg.norm(n, axis=1)[:, None]
    hit = np.zeros(N, bool)
    for rv in active(e["lam"], e["time"]).values():
        off = np.degrees(np.arcsin(np.minimum(1, np.abs(n @ rv))))
        behind = np.arccos(np.clip(v1 @ rv, -1, 1)) > math.acos(np.clip(p @ rv, -1, 1))
        hit |= behind & (off <= e["tol"])
    return hit.mean()


sk = [e for e in events if "n" in e]
for e in sk:
    e["chance"] = chance_hits(e)

# ---------------------------------------------------------------- events.csv
with open(os.path.join(a.out, "events.csv"), "w", newline="", encoding="utf-8") as f:
    w_ = csv.writer(f)
    w_.writerow(["block", "time_JST", "tier", "length_px", "pixels", "corr", "frames", "speed_px_f", "x_start", "y_start", "x_end", "y_end",
                 "wcs", "wcs_borrowed", "sun_alt_deg", "len_deg", "speed_deg_s", "shower", "offset_deg", "tol_deg", "chance_hit_prob"])
    for e in events:
        w_.writerow([e["block"], e["time"].strftime("%H:%M:%S"), e["tier"], f"{e['length']:.0f}", e["pixels"], f"{e['corr']:.2f}", f"{e['span']:.0f}",
                     f"{e['speed']:.2f}", f"{e['xa']:.0f}", f"{e['ya']:.0f}", f"{e['xb']:.0f}", f"{e['yb']:.0f}", int(e["wcs"]), int(e.get("synth", False)), f"{e['sun_alt']:.1f}",
                     f"{e.get('len_deg', float('nan')):.1f}", f"{e.get('speed_deg_s', float('nan')):.1f}", e.get("shower", ""),
                     f"{e.get('offset', float('nan')):.1f}", f"{e.get('tol', float('nan')):.1f}", f"{e.get('chance', float('nan')):.2f}"])

# ---------------------------------------------------------------- shower summary
print("\nshower association (events with sky coordinates):")
tab = defaultdict(Counter)
for e in sk:
    tab[e["shower"]][e["tier"]] += 1
for code in sorted(tab, key=lambda c: -sum(tab[c].values())):
    print(f"  {code:4s} " + " ".join(f"{t}={tab[code][t]:3d}" for t in "ABC") + f"  total {sum(tab[code].values())}")
for t in "ABC":
    ev_t = [e for e in sk if e["tier"] == t]
    if ev_t:
        hits = sum(e["shower"] != "SPO" for e in ev_t)
        print(f"  tier {t}: {len(ev_t)} events, {hits} matched a shower ({100 * hits / len(ev_t):.0f}%); expected by chance {sum(e['chance'] for e in ev_t):.1f} ({100 * np.mean([e['chance'] for e in ev_t]):.0f}%)")

# ---------------------------------------------------------------- plot 1: timeline
fig, ax = plt.subplots(2, 1, figsize=(14, 7), sharex=True, gridspec_kw={"height_ratios": [3, 1.2]})
t0 = t_start
bins = sorted({(e["time"] - t0).total_seconds() // 1800 for e in events} | {0})
nb = int(((datetime.strptime(a.t_to[:15], "%Y%m%d_%H%M%S").replace(tzinfo=TZ) - t0).total_seconds()) // 1800) + 1
cnt = {t: np.zeros(nb) for t in "ABC"}
for e in events:
    cnt[e["tier"]][int((e["time"] - t0).total_seconds() // 1800)] += 1
x = [t0 + timedelta(minutes=30 * i + 15) for i in range(nb)]
bottom = np.zeros(nb)
for t in "ABC":
    ax[0].bar(x, cnt[t], width=timedelta(minutes=27), bottom=bottom, color=TIER_COL[t], label=f"tier {t} ({int(cnt[t].sum())})")
    bottom += cnt[t]
ax[0].set_ylabel("detections / 30 min"); ax[0].legend(); ax[0].grid(alpha=0.3)
ax[0].set_title(f"FF detections by tier, {a.t_from[:13]}..{a.t_to[:13]} JST (all blocks saved in this window)")
ts = [t0 + timedelta(minutes=m) for m in range(0, int((x[-1] - t0).total_seconds() // 60) + 15, 10)]
ax[1].plot(ts, [sun_alt(t) for t in ts], "k-")
for y_, lab in ((0, "sunset"), (-6, "civil"), (-12, "nautical"), (-18, "astronomical")):
    ax[1].axhline(y_, color="0.7", lw=0.6); ax[1].text(ts[0], y_ + 1, lab, fontsize=7, color="0.4")
ax[1].set_ylabel("sun altitude [deg]"); ax[1].grid(alpha=0.3)
import matplotlib.dates as mdates
ax[1].xaxis.set_major_formatter(mdates.DateFormatter("%H:%M", tz=TZ)); plt.tight_layout()
plt.savefig(os.path.join(a.out, "night_timeline.png"), dpi=110); plt.close()

# ---------------------------------------------------------------- plot 2: shower counts by tier
if sk:
    codes = sorted(tab, key=lambda c: -sum(tab[c].values()))
    fig, ax = plt.subplots(figsize=(10, 5))
    bottom = np.zeros(len(codes))
    for t in "ABC":
        v = np.array([tab[c][t] for c in codes], float)
        ax.bar(codes, v, bottom=bottom, color=TIER_COL[t], label=f"tier {t}")
        bottom += v
    exp = sum(e["chance"] for e in sk)
    ax.set_title(f"Shower association ({len(sk)} events with WCS). SPO = no match.\n"
                 f"By chance ~{exp:.0f} of {len(sk)} would match some active shower (tolerance 3-15 deg)")
    ax.legend(); ax.grid(alpha=0.3, axis="y"); plt.tight_layout()
    plt.savefig(os.path.join(a.out, "night_showers.png"), dpi=110); plt.close()

# ---------------------------------------------------------------- plot 3: polar sky map
if sk:
    def pol(ra, dec):
        r = 90 - dec
        return r * math.cos(math.radians(ra)), r * math.sin(math.radians(ra))
    codes_hit = [c for c in sorted(tab, key=lambda c: -sum(tab[c].values())) if c != "SPO"]
    pal = dict(zip(codes_hit, plt.cm.tab10.colors))
    fig, ax = plt.subplots(figsize=(11, 11))
    for rr in (30, 60, 90):
        ax.add_patch(plt.Circle((0, 0), rr, fill=False, color="0.85", lw=0.6))
    for ra in range(0, 360, 30):
        xx, yy = pol(ra, -10); ax.plot([0, xx], [0, yy], color="0.92", lw=0.5)
        xx, yy = pol(ra, -18); ax.text(xx, yy, f"{ra // 15}h", ha="center", va="center", fontsize=8, color="0.45")
    for e in sk:
        col = pal.get(e["shower"], "#999999")
        ls = "-" if e["tier"] != "C" else (0, (3, 2))
        e1 = np.cross(e["n"], e["v0"])
        pts = np.array([radec(math.cos(math.radians(dd)) * e["v0"] + math.sin(math.radians(dd)) * e1)
                        for dd in np.linspace(-60, e["len_deg"] + 4, 80)])
        xy = np.array([pol(*p) for p in pts]); ok = pts[:, 1] > -10
        ax.plot(xy[ok, 0], xy[ok, 1], ls=ls, color=col, lw=0.5, alpha=0.5)
        (xa_, ya_), (xb_, yb_) = pol(*radec(e["v0"])), pol(*radec(e["v1"]))
        ax.annotate("", xy=(xb_, yb_), xytext=(xa_, ya_),
                    arrowprops=dict(arrowstyle="-|>", color=col, lw=2.2 if e["tier"] != "C" else 1.2, ls="-"))
    lam_mid = float(np.mean([e["lam"] for e in sk]))
    for s in showers:
        d = wrap(lam_mid - s["max"])
        if not (wrap(s["begin"] - s["max"]) <= d <= wrap(s["end"] - s["max"])):
            continue
        ra_, dec_ = (s["ra"] + s["dra"] * d) % 360, s["dec"] + s["ddec"] * d
        xx, yy = pol(ra_, dec_)
        ax.plot(xx, yy, "*", ms=15 if s["code"] in pal else 9, color=pal.get(s["code"], "w"), mec="k", zorder=5)
        ax.text(xx + 2, yy + 2, s["code"], fontsize=9, weight="bold" if s["code"] in pal else None)
    ax.plot(0, 0, "k+", ms=10)
    ax.set_xlim(-100, 100); ax.set_ylim(-100, 100); ax.set_aspect("equal"); ax.axis("off")
    handles = [plt.Line2D([], [], color=pal[c], lw=3, label=f"{c} ({sum(tab[c].values())})") for c in codes_hit]
    handles.append(plt.Line2D([], [], color="#999999", lw=3, label=f"SPO ({sum(tab['SPO'].values())})"))
    handles += [plt.Line2D([], [], color="k", lw=2.2, label="tier A/B (solid)"), plt.Line2D([], [], color="k", lw=1.2, ls=(0, (3, 2)), label="tier C (dashed)")]
    ax.legend(handles=handles, loc="lower left", fontsize=8)
    ax.set_title(f"North celestial pole view (centre Dec +90, ring Dec 0). {len(sk)} meteors, sun lon {lam_mid:.1f} deg\n"
                 "arrow = observed, thin line = back-projected great circle, star = active radiant (mid-night)", fontsize=10)
    plt.tight_layout(); plt.savefig(os.path.join(a.out, "night_sky_polar.png"), dpi=100); plt.close()
print("\nwritten to", a.out)
