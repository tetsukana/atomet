"""Open FF blocks one by one in DS9 (4 extensions as 4 tiled frames).

usage: python ff_ds9.py <ff detections dir> [hour ...] [--all]
  e.g. python ff_ds9.py D:/ff/detections/2026/11/14 01 02 03 04 05

Each block opens in a DS9 window; close it (File > Exit) to get the next one.
Ctrl+C in the terminal stops the loop.  By default only blocks with an FF
detection are shown; --all includes the others.

Frames: 1 MAXPIXEL, 2 MAXFRAME (linear 0-255, rainbow: colour = time),
        3 AVEPIXEL, 4 STDPIXEL.
Regions (loaded into every frame): red = FF line, green arrow = direction
(earliest end -> latest end), cyan = realtime events, plus a text label with
speed.  Display is flipped (-orient y) so it matches the camera view.

Reading speed: put the pointer on the streak in the MAXFRAME frame (or use
Analysis > Pixel Table): the pixel value there is the frame number (25 fps by
default: 1 frame = 0.04 s).
"""
import glob
import json
import os
import subprocess
import sys
import tempfile

DS9 = r"C:\SAOImageDS9\ds9.exe"

args = [a for a in sys.argv[1:] if not a.startswith("--")]
show_all = "--all" in sys.argv
root, hours = args[0], args[1:]
if not hours:
    hours = sorted(d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d)))
jsons = []
for h in hours:
    jsons += sorted(glob.glob(os.path.join(root, h, "*.json")))


def regions(j):
    # DS9 image coords are 1-based; our pixel (x, y) -> (x+1, y+1).
    lines = ["# Region file format: DS9 version 4.1",
             "global width=1 font=\"helvetica 10 normal\"",
             "image"]
    fps = j.get("fps") or 25.0
    for m in j.get("ff_meteors", []):
        x0, y0, x1, y1 = m["x0"] + 1, m["y0"] + 1, m["x1"] + 1, m["y1"] + 1
        lines.append(f"# vector({x0:.1f},{y0:.1f},{m['length']:.1f},"
                     f"{__import__('math').degrees(__import__('math').atan2(y1 - y0, x1 - x0)):.1f}) vector=1 color=red")
        lines.append(f"line({x0:.1f},{y0:.1f},{x1:.1f},{y1:.1f}) # line=0 1 color=red")
        lines.append(f"# text({(x0 + x1) / 2:.0f},{max(y0, y1) + 12:.0f}) text={{FF {m['speed']:.2f}px/f = {m['speed'] * fps:.0f}px/s f{m['f0']:.0f}-{m['f1']:.0f}}} color=red")
    for e in j.get("realtime_events", []):
        if e["type"] != "meteor":
            continue
        t = e["trajectory"]
        lines.append(f"line({t[0][0] + 1:.1f},{t[0][1] + 1:.1f},{t[-1][0] + 1:.1f},{t[-1][1] + 1:.1f}) # line=0 1 color=cyan")
    return "\n".join(lines) + "\n"


tmp = tempfile.mkdtemp(prefix="ff_ds9_")
n_total = len(jsons)
for k, p in enumerate(jsons, 1):
    j = json.load(open(p, encoding="utf-8"))
    if not show_all and not j.get("ff_meteors"):
        continue
    fits_path = os.path.abspath(p[:-5] + ".fits")
    reg = os.path.join(tmp, os.path.basename(p)[:-5] + ".reg")
    with open(reg, "w") as f:
        f.write(regions(j))
    print(f"[{k}/{n_total}] {os.path.basename(fits_path)}", flush=True)
    cmd = [
        DS9, "-multiframe", fits_path,
        "-tile", "grid", "-tile", "mode", "grid",
        "-orient", "y", "-zoom", "to", "fit",
        "-width", "1400", "-height", "800",
        "-frame", "1", "-scale", "zscale", "-cmap", "grey",
        "-frame", "2", "-scale", "linear", "-scale", "limits", "0", "255", "-cmap", "rainbow",
        "-frame", "3", "-scale", "zscale", "-cmap", "grey",
        "-frame", "4", "-scale", "zscale", "-cmap", "grey",
        "-regions", "load", "all", reg,
        "-frame", "2",
        "-title", os.path.basename(fits_path),
    ]
    try:
        subprocess.run(cmd, check=False)
    except KeyboardInterrupt:
        break
