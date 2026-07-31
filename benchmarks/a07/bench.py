import io, zlib, time, os
from PIL import Image
import numpy as np, qoi

FPS=30
def kb(n): return n/1024
def fmt(n): return f"{n/1024:8.1f} KiB"

rows=[]
for name in ["wiki","hn","gh"]:
    p=f"/tmp/bgshots/{name}.png"
    im=Image.open(p).convert("RGB")
    w,h=im.size
    rgb=im.tobytes()
    rgba=im.convert("RGBA").tobytes()
    raw24=len(rgb); raw32=len(rgba)
    res={"name":name,"size":(w,h),"raw24":raw24,"raw32":raw32}

    # zlib on raw RGB at various levels
    for lvl in (1,6,9):
        t=time.perf_counter(); c=zlib.compress(rgb,lvl); dt=time.perf_counter()-t
        res[f"zlib24_L{lvl}"]=(len(c),dt)
    # zlib on RGBA L1 (kitty f=32 o=z)
    t=time.perf_counter(); c=zlib.compress(rgba,1); dt=time.perf_counter()-t
    res["zlib32_L1"]=(len(c),dt)

    # PNG (kitty f=100)
    for lvl in (1,6):
        b=io.BytesIO(); t=time.perf_counter(); im.save(b,"PNG",compress_level=lvl); dt=time.perf_counter()-t
        res[f"png_L{lvl}"]=(len(b.getvalue()),dt)

    # JPEG
    for q in (50,70,85,95):
        b=io.BytesIO(); t=time.perf_counter(); im.save(b,"JPEG",quality=q); dt=time.perf_counter()-t
        res[f"jpeg_q{q}"]=(len(b.getvalue()),dt)

    # WebP lossy + lossless
    for q in (60,80):
        b=io.BytesIO(); t=time.perf_counter(); im.save(b,"WEBP",quality=q,method=2); dt=time.perf_counter()-t
        res[f"webp_q{q}"]=(len(b.getvalue()),dt)
    b=io.BytesIO(); t=time.perf_counter(); im.save(b,"WEBP",lossless=True,quality=20,method=1); dt=time.perf_counter()-t
    res["webp_lossless"]=(len(b.getvalue()),dt)

    # QOI
    arr=np.array(im)
    t=time.perf_counter(); q_=qoi.encode(arr); dt=time.perf_counter()-t
    res["qoi"]=(len(q_),dt)
    # QOI + zlib (QOI is not entropy coded)
    t=time.perf_counter(); qz=zlib.compress(q_,1); dt=time.perf_counter()-t
    res["qoi_zlib1"]=(len(qz),dt)
    rows.append(res)

print(f"{'page':6} {'codec':16} {'bytes':>10} {'KiB':>9} {'ratio':>7} {'enc ms':>8} {'MB/s@30fps':>11} {'b64 KiB':>9}")
print("-"*84)
for r in rows:
    raw=r["raw24"]
    print(f"{r['name']:6} {'RAW rgb24':16} {raw:10d} {kb(raw):9.1f} {1.0:7.2f} {'-':>8} {raw*30/1e6:11.1f} {kb(raw*4/3):9.1f}")
    for k in ["zlib24_L1","zlib24_L6","zlib24_L9","zlib32_L1","png_L1","png_L6","jpeg_q50","jpeg_q70","jpeg_q85","jpeg_q95","webp_q60","webp_q80","webp_lossless","qoi","qoi_zlib1"]:
        n,dt=r[k]
        print(f"{'':6} {k:16} {n:10d} {kb(n):9.1f} {raw/n:7.2f} {dt*1000:8.1f} {n*30/1e6:11.2f} {kb(n*4/3):9.1f}")
    print()
