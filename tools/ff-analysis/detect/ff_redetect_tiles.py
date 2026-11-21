"""Contact sheet of the events in a ff_redetect --dump jsonl (tier A, then B, then C).
usage: python ff_redetect_tiles.py <redetect.jsonl> <out.png> <fits root>...
Tile label: N<number> <tier> <hh>-<mm:ss> L<length px> f<frames>. The N numbers are the ones
ff_redetect_sanson.py takes in INSECT.
"""
import glob, json, math, os, sys
import numpy as np
from astropy.io import fits
from PIL import Image, ImageDraw
JSONL, OUT, ROOTS = sys.argv[1], sys.argv[2], sys.argv[3:]
ev = []
for l in open(JSONL, encoding='utf8'):
    d = json.loads(l)
    for m in d.get('meteors', []):
        ev.append((d['name'], d.get('gain_db', 0), m))
ev.sort(key=lambda e: ({'A': 0, 'B': 1, 'C': 2}[e[2]['tier']], e[0]))
def find(n):
    for root in ROOTS:
        g = glob.glob(f'{root}/**/{n}', recursive=True)
        if g:
            return g[0]
T, R, cols = 200, 50, 8
COL = {'A': (60, 200, 60), 'B': (230, 180, 50), 'C': (230, 60, 60)}
tiles = []
for i, (n, g, m) in enumerate(ev, 1):
    with fits.open(find(n), memmap=False) as h:
        mx = h[0].data.astype(float); av = h[2].data.astype(float)
    d = np.clip((mx - av) * 4, 0, 255)
    cx = int((m['x0'] + m['x1']) / 2); cy = int((m['y0'] + m['y1']) / 2)
    x0 = min(max(cx - R, 0), 640 - 2 * R); y0 = min(max(cy - R, 0), 360 - 2 * R)
    im = Image.fromarray(d[y0:y0 + 2 * R, x0:x0 + 2 * R].astype(np.uint8)).resize((T, T), Image.NEAREST).convert('RGB')
    dr = ImageDraw.Draw(im); s = T / (2 * R); col = COL[m['tier']]
    dr.line([((m['x0'] - x0) * s, (m['y0'] - y0) * s), ((m['x1'] - x0) * s, (m['y1'] - y0) * s)], fill=col, width=1)
    dr.rectangle([0, 0, T, 13], fill=(0, 0, 0))
    dr.text((3, 1), 'N%d %s %s-%s L%.0f f%d' % (i, m['tier'], n[12:14], n[14:16] + ':' + n[16:18], m['length'], m['frames']), fill=col)
    dr.rectangle([0, 0, T - 1, T - 1], outline=col, width=2)
    tiles.append(im)
rows = math.ceil(len(tiles) / cols)
sheet = Image.new('RGB', (cols * T, rows * T), (0, 0, 0))
for i, im in enumerate(tiles): sheet.paste(im, ((i % cols) * T, (i // cols) * T))
sheet.save(OUT); print(len(tiles), sheet.size)
for i, (n, g, m) in enumerate(ev, 1):
    print(i, m['tier'], n[3:18], '%.1fdB' % g, 'L%.0f f%d v%.1f px%d lin%.0f' % (m['length'], m['frames'], m['speed'], m['pixels'], m['lin']))
