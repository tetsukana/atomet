"""Export the avepixel of FF blocks as raw 640x360 u8 files for crates/solver/examples/baseline_solve.rs,
and list the WCS pointing stored in each header (the reference the camera used) for comparison.

usage: python ff_solver_export.py OUT_DIR file1.fits [file2.fits ...]
Writes OUT_DIR/<name>.raw and OUT_DIR/index.tsv (name, JST time, CRVAL1/2, WCSAGE or '-').
"""
import os
import sys

import numpy as np
from astropy.io import fits

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
rows = []
for p in sys.argv[2:]:
    with fits.open(p, memmap=False) as h:
        ave = h[2].data
        hd = h[0].header
        name = os.path.splitext(os.path.basename(p))[0]
        np.clip(ave, 0, 255).astype(np.uint8).tofile(os.path.join(out, name + ".raw"))
        rows.append((name, hd.get("CRVAL1", "-"), hd.get("CRVAL2", "-"), hd.get("WCSAGE", "-"), hd.get("DATE-OBS", "-")))
with open(os.path.join(out, "index.tsv"), "w") as f:
    for r in rows:
        f.write("\t".join(str(x) for x in r) + "\n")
print(len(rows), "frames ->", out)
