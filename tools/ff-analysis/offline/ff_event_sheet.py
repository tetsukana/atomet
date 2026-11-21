"""Contact sheet of tier events in the validation window (paged by --from/--to index).
usage: python night_sheet.py <tiers e.g. AB> <out.png> <from_idx> <to_idx> <dir with cache_n18.jsonl / cache_n19.jsonl>
Index = position in the time-ordered event list of those tiers (so sheets can be paged)."""
import sys, json
sys.path.insert(0, 'E:/source/atomet/tools/ff-analysis')
import numpy as np
import ff_offline as F
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

tiers, out = sys.argv[1], sys.argv[2]
lo, hi = int(sys.argv[3]), int(sys.argv[4])
cdir = sys.argv[5]
T0 = sys.argv[6] if len(sys.argv) > 6 else "20261118_172430"
T1 = sys.argv[7] if len(sys.argv) > 7 else "20261119_014240"
items = []
for c in (cdir + "/cache_n18.jsonl", cdir + "/cache_n19.jsonl", cdir + "/cache_n19b.jsonl"):
    for l in open(c, encoding="utf-8"):
        r = json.loads(l)
        if "error" in r or not (T0 <= r["block"] <= T1):
            continue
        for e in F.tier_events(r):
            if e["tier"] in tiers:
                items.append((r["block"], e, r["path"]))
items.sort(key=lambda t: t[0])
total = len(items)
items = list(enumerate(items))[lo:hi]
cols = 6
nr = (len(items) + cols - 1) // cols
fig, axs = plt.subplots(nr, cols, figsize=(cols * 3.2, nr * 3.4))
for ax in np.atleast_1d(axs).flat:
    ax.axis("off")
for ax, (k, (b, e, p)) in zip(np.atleast_1d(axs).flat, items):
    mp, mf, av, sd, fps = F.read_block(p)
    cx, cy = int((e["x0"] + e["x1"]) / 2), int((e["y0"] + e["y1"]) / 2)
    x0 = max(0, min(540, cx - 50))
    y0 = max(0, min(260, cy - 50))
    d = mp.astype(float)[y0:y0 + 100, x0:x0 + 100] - av.astype(float)[y0:y0 + 100, x0:x0 + 100]
    ax.imshow(d, cmap="gray", vmin=0, vmax=max(40, np.percentile(d, 99.8)), extent=(x0, x0 + 100, y0 + 100, y0))
    ax.plot([e["x0"], e["x1"]], [e["y0"], e["y1"]], "r-", lw=0.5, alpha=.5)
    ax.set_title(f"#{k} {b[9:11]}:{b[11:13]}:{b[13:15]} [{e['tier']}]\nL{e['length']:.0f} px{e['pixels']} r{e['corr']:.2f} f{e['span']:.0f}", fontsize=7)
    ax.axis("on")
    ax.tick_params(labelsize=5)
plt.tight_layout()
plt.savefig(out, dpi=95)
print(f"{len(items)} of {total} events -> {out}")
