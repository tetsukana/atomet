"""Plot detection rate vs peak excess A from ff_inject --csv output.
usage: python ff_inject_plot.py inject.csv out.png   (one panel per speed, one line per variant, 'any tier' solid / tier A dashed)
"""
import csv, sys, collections
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

rows = list(csv.DictReader(open(sys.argv[1])))
agg = collections.defaultdict(lambda: [0, 0, 0])  # (variant, speed, amp) -> trials, hitA, hitAny
for r in rows:
    k = (r["variant"], float(r["speed"]), float(r["amp"]))
    n = int(r["trials"]); a = int(r["hit_A"]); b = int(r["hit_B"]); c = int(r["hit_C"])
    agg[k][0] += n; agg[k][1] += a; agg[k][2] += a + b + c
variants = sorted({k[0] for k in agg}); speeds = sorted({k[1] for k in agg})
fig, axes = plt.subplots(1, len(speeds), figsize=(4 * len(speeds), 3.6), sharey=True, squeeze=False)
for ax, sp in zip(axes[0], speeds):
    for i, v in enumerate(variants):
        pts = sorted((k[2], d) for k, d in agg.items() if k[0] == v and k[1] == sp)
        x = [p[0] for p in pts]
        ax.plot(x, [100 * p[1][2] / p[1][0] for p in pts], "-o", color=f"C{i}", label=v)
        ax.plot(x, [100 * p[1][1] / p[1][0] for p in pts], "--", color=f"C{i}")
    ax.set_xscale("log"); ax.set_title(f"{sp:g} px/frame"); ax.set_xlabel("peak excess A (ADU)"); ax.grid(alpha=.3)
axes[0][0].set_ylabel("detection rate (%)  solid=any tier, dashed=tier A"); axes[0][0].legend(fontsize=7)
fig.tight_layout(); fig.savefig(sys.argv[2], dpi=130)
