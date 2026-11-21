"""Sanson (sinusoidal) projection of the events in a ff_redetect --dump jsonl: back-projected great circles,
active radiants and the shower association (same rule as ff_showers.py).
usage: python ff_redetect_sanson.py <redetect.jsonl> <out.png> <fits root>...
env INSECT=16,17,18  event numbers (N numbers of ff_redetect_tiles.py) to leave out: insects are valid
                     detections, but their direction says nothing about the radiant
env CSV=<showers.csv>  env TZ_HOURS=9 (block names are local time)
One night only (WCS borrowing compares times of day); the FITS headers must carry a WCS.
"""
import glob, json, math, os, sys
from datetime import datetime, timedelta, timezone
import numpy as np
import matplotlib; matplotlib.use('Agg')
import matplotlib.pyplot as plt
from astropy.io import fits
from astropy.wcs import WCS
JSONL, OUT, ROOTS = sys.argv[1], sys.argv[2], sys.argv[3:]
CSV = os.environ.get('CSV', os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'overlay_rootfs', 'usr', 'share', 'atomet', 'showers.csv'))
ROT = 360.0 / 86164.0905
JST = timezone(timedelta(hours=float(os.environ.get('TZ_HOURS', '9'))))
def vec(ra, dec):
    ra, dec = math.radians(ra), math.radians(dec)
    return np.array([math.cos(dec)*math.cos(ra), math.cos(dec)*math.sin(ra), math.sin(dec)])
def radec(v): return math.degrees(math.atan2(v[1], v[0])) % 360, math.degrees(math.asin(max(-1, min(1, v[2]))))
def w180(x): return (x + 180) % 360 - 180
def solar_lon(t):
    n = (t - datetime(2000,1,1,12,tzinfo=timezone.utc)).total_seconds()/86400
    l = 280.460+0.9856474*n; g = math.radians(357.528+0.9856003*n)
    return (l+1.915*math.sin(g)+0.020*math.sin(2*g)) % 360
showers = []
for line in open(CSV, encoding='utf-8'):
    if line.startswith('#') or '|' not in line: continue
    f = [s.strip() for s in line.split('|')]
    if len(f) < 10: continue
    try: mx, ra, dec = float(f[4]), float(f[6]), float(f[8])
    except ValueError: continue
    showers.append(dict(code=f[1], begin=float(f[3]) if f[3] else mx-5, max=mx, end=float(f[5]) if f[5] else mx+5,
                        ra=ra, dra=float(f[7] or 0), dec=dec, ddec=float(f[9] or 0)))
def find(n):
    for root in ROOTS:
        g = glob.glob(f'{root}/**/{n}', recursive=True)
        if g:
            return g[0]
def tsec(n): t = n[12:18]; return int(t[:2])*3600+int(t[2:4])*60+int(t[4:6])
# headers with a WCS, for borrowing
wcs_files = []
for l in open(JSONL, encoding='utf8'):
    d = json.loads(l)
    if 'meteors' in d: pass
ev = []
for l in open(JSONL, encoding='utf8'):
    d = json.loads(l)
    for m in d.get('meteors', []): ev.append((d['name'], m))
ev.sort(key=lambda e: ({'A':0,'B':1,'C':2}[e[1]['tier']], e[0]))
allf = sorted(set(os.path.basename(p) for root in ROOTS for p in glob.glob(f'{root}/**/FF_*.fits', recursive=True)))
cache = {}
def hdr(n):
    if n not in cache:
        try: cache[n] = fits.getheader(find(n))
        except Exception: cache[n] = None
    return cache[n]
def get_wcs(n):
    h = hdr(n)
    if h is not None and 'CRVAL1' in h: return h, 0.0
    for f in sorted(allf, key=lambda f: abs(tsec(f)-tsec(n)))[:60]:
        h = hdr(f)
        if h is not None and 'CRVAL1' in h: return h, ROT*(tsec(n)-tsec(f))
    return None, 0
mets = []; skipped = 0
INSECT = {int(x) for x in os.environ.get('INSECT', '').split(',') if x}
for i, (n, m) in enumerate(ev, 1):
    if i in INSECT: continue
    h, rot = get_wcs(n)
    if h is None: skipped += 1; continue
    w = WCS(h); fps = float(h.get('FPS', 25.0))
    p = lambda x, y: tuple(float(a) for a in w.wcs_pix2world([[x+1, y+1]], 1)[0])
    (ra0, d0), (ra1, d1) = p(m['x0'], m['y0']), p(m['x1'], m['y1'])
    ra0 = (ra0+rot) % 360; ra1 = (ra1+rot) % 360
    v0, v1 = vec(ra0, d0), vec(ra1, d1)
    ang = math.degrees(math.acos(float(np.clip(v0@v1, -1, 1))))
    when = datetime.strptime(n[3:18], '%Y%m%d_%H%M%S').replace(tzinfo=JST) + timedelta(seconds=m['f0']/fps)
    mets.append(dict(i=i, tier=m['tier'], n=n, m=m, when=when, v0=v0, v1=v1, L=ang, borrowed=bool(rot)))
print('events', len(mets), 'no wcs', skipped, 'borrowed', sum(t['borrowed'] for t in mets))
TOL_BASE, TOL_CAP = 3.0, 15.0
for t in mets:
    t['lam'] = solar_lon(t['when'].astimezone(timezone.utc))
    t['tol'] = min(TOL_BASE + 3*math.degrees(1.0/max(t['m']['length'], 1.0)), TOL_CAP)
    nn = np.cross(t['v0'], t['v1']); nn /= np.linalg.norm(nn); t['nn'] = nn
    best = None
    for s in showers:
        d = w180(t['lam']-s['max'])
        if d < w180(s['begin']-s['max']) or d > w180(s['end']-s['max']): continue
        r = vec((s['ra']+s['dra']*d) % 360, max(-90, min(90, s['dec']+s['ddec']*d)))
        off = math.degrees(math.asin(min(1, abs(float(r@nn)))))
        behind = math.acos(np.clip(r@t['v1'], -1, 1)) > math.acos(np.clip(r@t['v0'], -1, 1))
        if behind and off <= t['tol'] and (best is None or off < best[1]): best = (s['code'], off)
    t['best'] = best
hits = {}
for t in mets:
    k = t['best'][0] if t['best'] else 'sporadic'; hits[k] = hits.get(k, 0)+1
print('summary', hits, 'lambda %.2f-%.2f' % (min(t['lam'] for t in mets), max(t['lam'] for t in mets)))
for t in mets:
    print('N%d %s %s L%.1fdeg %s' % (t['i'], t['tier'], t['n'][3:18], t['L'], ('%s(%.1f)' % t['best']) if t['best'] else '-'))
ras = np.array([radec(t['v0'])[0] for t in mets])
c = math.degrees(math.atan2(np.sin(np.radians(ras)).mean(), np.cos(np.radians(ras)).mean())) % 360
def sans(ra, dec):  # sinusoidal (Sanson-Flamsteed), RA increasing to the left
    return -w180(ra-c)*math.cos(math.radians(dec)), dec
codes = sorted({t['best'][0] for t in mets if t['best']})
col = {k: plt.cm.tab10.colors[i % 10] for i, k in enumerate(codes)}
fig, ax = plt.subplots(figsize=(13, 9))
for dec in range(-60, 91, 30):  # parallels
    xs = [sans(c+dx, dec) for dx in np.linspace(-180, 180, 200)]
    ax.plot([a for a, b in xs], [b for a, b in xs], color='0.85', lw=0.5)
for dra in range(-150, 151, 30):  # meridians
    xs = [sans(c+dra, d) for d in np.linspace(-90, 90, 100)]
    ax.plot([a for a, b in xs], [b for a, b in xs], color='0.85', lw=0.5)
    x, y = sans(c+dra, -62); ax.text(x, y, '%.0f°' % ((c+dra) % 360), ha='center', fontsize=7, color='0.4')
for t in mets:
    k = t['best'][0] if t['best'] else None; cc = col.get(k, '0.5')
    e1 = np.cross(t['nn'], t['v0'])
    pts = np.array([radec(math.cos(math.radians(dd))*t['v0']+math.sin(math.radians(dd))*e1) for dd in np.linspace(-70, t['L']+10, 200)])
    xy = np.array([sans(r, d) for r, d in pts])
    br = np.where(np.abs(np.diff(xy[:, 0])) > 25)[0]+1
    for xs, ys in zip(np.split(xy[:, 0], br), np.split(xy[:, 1], br)): ax.plot(xs, ys, '-', color=cc, lw=0.6, alpha=0.5)
    a0, a1 = sans(*radec(t['v0'])), sans(*radec(t['v1']))
    ax.annotate('', xy=a1, xytext=a0, arrowprops=dict(arrowstyle='->', color=cc, lw=1.8))
lam_mid = float(np.mean([t['lam'] for t in mets]))
for s in showers:
    d = w180(lam_mid-s['max'])
    if d < w180(s['begin']-s['max']) or d > w180(s['end']-s['max']): continue
    ra = (s['ra']+s['dra']*d) % 360; dec = s['dec']+s['ddec']*d
    x, y = sans(ra, dec)
    ax.plot(x, y, '*', ms=15 if s['code'] in codes else 9, color=col.get(s['code'], 'w'), mec='k', mew=0.6, zorder=5)
    ax.annotate(s['code'], (x, y), xytext=(4, 4), textcoords='offset points', fontsize=8, weight='bold' if s['code'] in codes else None)
ax.set_xlim(-150, 150); ax.set_ylim(-65, 92); ax.set_aspect('equal'); ax.set_xlabel('(RA-center)*cos(Dec), RA increases to the left'); ax.set_ylabel('Dec')
ax.set_title('FF meteors (%d, tiers A/B/C)  Sanson projection  center RA %.0f°  λ☉=%.1f°  ' % (len(mets), c, lam_mid)
             + ', '.join('%s:%d' % kv for kv in hits.items()), fontsize=9)
plt.tight_layout(); plt.savefig(OUT, dpi=100)
