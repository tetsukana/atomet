import json, glob, sys
import numpy as np
from astropy.io import fits
import matplotlib; matplotlib.use("Agg")
import matplotlib.pyplot as plt
root, out = sys.argv[1], sys.argv[2]
items=[]
for h in sys.argv[3:]:
    for p in sorted(glob.glob(f"{root}/{h}/*.json")):
        j=json.load(open(p))
        for m in j.get("ff_meteors",[]): items.append((p,m))
n=len(items); cols=6; rows=(n+cols-1)//cols
fig,axs=plt.subplots(rows,cols,figsize=(cols*3.2,rows*3.4))
for ax in axs.flat: ax.axis('off')
for ax,(p,m) in zip(axs.flat,items):
    with fits.open(p.replace('.json','.fits')) as f:
        mx=f[0].data.astype(float); av=f[2].data.astype(float)
    cx=int((m['x0']+m['x1'])/2); cy=int((m['y0']+m['y1'])/2)
    x0=max(0,min(640-100,cx-50)); y0=max(0,min(360-100,cy-50))
    d=(mx-av)[y0:y0+100,x0:x0+100]
    ax.imshow(d,cmap='gray',vmin=0,vmax=max(40,np.percentile(d,99.8)),origin='upper',extent=(x0,x0+100,y0+100,y0))
    ax.plot([m['x0'],m['x1']],[m['y0'],m['y1']],'r-',lw=0.5,alpha=.5)
    ax.set_title(p.split('/')[-1][3:-5].replace('20261114_','')+f"\nL{m['length']:.0f} f{m['frames']} c{m['time_corr']:.2f}",fontsize=7)
    ax.axis('on'); ax.tick_params(labelsize=5)
plt.tight_layout(); plt.savefig(out,dpi=110)
print(n)
