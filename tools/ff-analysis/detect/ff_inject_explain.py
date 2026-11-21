"""Explainer + results figure for ff_inject. usage: python ff_inject_explain.py inject.csv out.png [variant]"""
import csv, sys, collections
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch
plt.rcParams["font.family"] = ["Yu Gothic", "Meiryo", "sans-serif"]

var = sys.argv[3] if len(sys.argv) > 3 else "new"
rows = [r for r in csv.DictReader(open(sys.argv[1])) if r["variant"] == var]
fig = plt.figure(figsize=(15, 10.5))
gs = fig.add_gridspec(2, 3, height_ratios=[1, 1.15], hspace=0.2, wspace=0.28)

# ---- top: what the test does
ax = fig.add_subplot(gs[0, :]); ax.axis("off"); ax.set_xlim(-2, 102); ax.set_ylim(-1, 29)
def box(x, w, title, body, c):
    ax.add_patch(FancyBboxPatch((x, 2), w, 25, boxstyle="round,pad=0.3", fc=c, ec="#555", lw=1))
    ax.text(x + w / 2, 24, title, ha="center", va="center", fontsize=13, weight="bold")
    ax.text(x + w / 2, 12.5, body, ha="center", va="center", fontsize=10.5, linespacing=1.6)
box(0, 21, "① 実際の夜のデータ", "保存済みの FF ブロック\n(流星が写っていない)\n本物のノイズが入っている", "#e8eef7")
box(26, 21, "② 人工の流星を足す", "明るさ A・長さ L・速さ v\n位置と向きは乱数\nで線を 1 本だけ書き込む", "#fdf0d5")
box(52, 21, "③ 検出器にかける", "いつもの検出器\n(閾値は本番と同じ)\nに通す", "#e6f2e6")
box(78, 22, "④ 見つけたか数える", "足した線の両端 15px 以内に\n検出が出たら「成功」\n出なければ「見逃し」", "#f7e3e3")
for x in (22.5, 48.5, 74.5):
    ax.annotate("", xy=(x + 3, 14.5), xytext=(x, 14.5), arrowprops=dict(arrowstyle="->", lw=2))
ax.text(50, 0, "答えがわかっている流星で試すので、本物では測れない「見逃し率」が分かる。"
        "これを 128 通りの (A,L,v) × 150 ブロックで繰り返した。", ha="center", fontsize=11, color="#333")

# ---- aggregate
by_amp = collections.defaultdict(lambda: np.zeros(4))
cell = collections.defaultdict(lambda: np.zeros(2))
for r in rows:
    a = float(r["amp"]); n = int(r["trials"]); h = [int(r[k]) for k in ("hit_A", "hit_B", "hit_C", "miss")]
    by_amp[a] += h
    cell[(a, float(r["len"]), float(r["speed"]))] += [n - h[3], n]
amps = sorted(by_amp)

# ---- bottom-left: rate vs brightness
ax1 = fig.add_subplot(gs[1, 0])
anyr = [100 * (1 - by_amp[a][3] / by_amp[a].sum()) for a in amps]
Ar = [100 * by_amp[a][0] / by_amp[a].sum() for a in amps]
ax1.plot(amps, anyr, "-o", color="#1f77b4", label="検出できた(どの段階でも)")
ax1.plot(amps, Ar, "--s", color="#d62728", label="確度が最高の段階 A で")
ax1.axvline(15, color="gray", ls=":"); ax1.text(15.5, 5, "段階Aの閾値 15", fontsize=9, color="gray")
ax1.axvline(12, color="gray", ls=":"); ax1.text(6.2, 15, "段階B 12", fontsize=9, color="gray")
ax1.set_xscale("log"); ax1.set_xticks(amps); ax1.set_xticklabels([f"{a:g}" for a in amps])
ax1.set_xlabel("流星の明るさ A（背景より何段階明るいか）"); ax1.set_ylabel("検出率 (%)"); ax1.set_ylim(-3, 103)
ax1.set_title("明るさごとの検出率", fontsize=12); ax1.grid(alpha=.3); ax1.legend(fontsize=9, loc="center right")

# ---- bottom: heatmaps L x v
lens = sorted({k[1] for k in cell}); sp = sorted({k[2] for k in cell})
for i, a in enumerate((14, 100)):
    axh = fig.add_subplot(gs[1, 1 + i])
    m = np.array([[100 * cell[(a, l, v)][0] / cell[(a, l, v)][1] for v in sp] for l in lens])
    axh.imshow(m, cmap="RdYlGn", vmin=0, vmax=100, origin="lower", aspect="auto")
    for yi in range(len(lens)):
        for xi in range(len(sp)):
            axh.text(xi, yi, f"{m[yi, xi]:.0f}%", ha="center", va="center", fontsize=11, weight="bold")
    axh.set_xticks(range(len(sp))); axh.set_xticklabels([f"{v:g}" for v in sp])
    axh.set_yticks(range(len(lens))); axh.set_yticklabels([f"{l:g}" for l in lens])
    axh.set_xlabel("速さ v（px/フレーム）"); axh.set_ylabel("長さ L（px）")
    axh.set_title(f"A={a}（{'ぎりぎり暗い' if a == 14 else '十分明るい'}）の検出率", fontsize=12)
fig.suptitle(f"人工流星で測った FF 検出の見逃し率（2026-11-21 夜・変種 {var}）", fontsize=16, weight="bold", y=0.98)
fig.savefig(sys.argv[2], dpi=110, bbox_inches="tight")
