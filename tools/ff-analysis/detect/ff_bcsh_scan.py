"""Per-block noise statistics of the sky region, to see when the ISP setting (BCSH on/off) changed.
usage: python ff_bcsh_scan.py <out.json> <from YYYYMMDD_HHMMSS> <to YYYYMMDD_HHMMSS> <step> <fits root>...
out rows: [time, GAINDB, median stdpixel, mean stdpixel, median avepixel, fraction of (max-ave) > 12]
BCSH on ~ stdpixel 1.4-1.8, fraction 0.0;  BCSH off ~ stdpixel 2.1-3.1, fraction 0.03-0.18 (docs 9).
The sky region (rows 40-250, cols 100-540) is this camera's view; change `r` for another one.
"""
import glob, os, json, sys
import numpy as np
from astropy.io import fits
OUT, FROM, TO, STEP, ROOTS = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5:]
fs = [p for root in ROOTS for p in glob.glob(f'{root}/**/FF_*.fits', recursive=True)]
byname = {os.path.basename(p): p for p in fs}
names = sorted(n for n in byname if FROM <= n[3:18] <= TO)
names = names[::STEP]
out = []
for n in names:
    try:
        with fits.open(byname[n], memmap=False) as h:
            mx = h[0].data.astype(float); av = h[2].data.astype(float); sd = h[3].data.astype(float)
            g = h[0].header.get('GAINDB')
    except Exception:
        continue
    r = (slice(40, 250), slice(100, 540))
    out.append((n[3:18], g, float(np.median(sd[r])), float(np.mean(sd[r])), float(np.median(av[r])), float(np.mean((mx - av)[r] > 12))))
json.dump(out, open(OUT, 'w'))
print(len(out))
