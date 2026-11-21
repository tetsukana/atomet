import json, glob, sys
root = sys.argv[1]
for h in sys.argv[2:]:
    for p in sorted(glob.glob(f"{root}/{h}/*.json")):
        j = json.load(open(p))
        ffm = j.get("ff_meteors", [])
        rt = j.get("realtime_events", [])
        print(p.split('/')[-1], j["reason"], f"fps={j['fps']:.1f}", f"dur={j['duration_s']:.1f}", f"drop={j['dropped']}", f"wcs={j['wcs']}")
        for m in ffm:
            sky = m.get("sky", {})
            print(f"   FF len={m['length']:.0f} fr={m['frames']} f0={m['f0']:.0f}-{m['f1']:.0f} spd={m['speed']:.2f} px={m['pixels']} corr={m['time_corr']:.2f} "
                  f"({m['x0']:.0f},{m['y0']:.0f})->({m['x1']:.0f},{m['y1']:.0f}) fill={m['pixels']/m['length']:.1f} shower={sky.get('shower')} deg_s={sky.get('speed_deg_s',0):.1f}")
        for e in rt:
            print(f"   RT {e['type']} id={e['id']} frames={e['frames']} spd={e['speed']} short={e.get('short')} traj={e['trajectory'][:1]}..{e['trajectory'][-1:]}")
