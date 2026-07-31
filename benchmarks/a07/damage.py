import zlib, io
from PIL import Image
import numpy as np

CELL_W, CELL_H = 8, 17          # typical monospace cell at 1440x900 -> 180x52 grid
def tiles_changed(a, b, tw, th):
    """Return (n_changed, n_total, changed_px_bytes) using a tw x th tile grid."""
    A=np.asarray(a,dtype=np.uint8); B=np.asarray(b,dtype=np.uint8)
    h,w,_=A.shape
    ty=(h+th-1)//th; tx=(w+tw-1)//tw
    changed=0
    boxes=[]
    for j in range(ty):
        for i in range(tx):
            sa=A[j*th:(j+1)*th, i*tw:(i+1)*tw]
            sb=B[j*th:(j+1)*th, i*tw:(i+1)*tw]
            if not np.array_equal(sa,sb):
                changed+=1; boxes.append((i,j,sb))
    return changed, tx*ty, boxes

def cost_zlib(boxes, tw, th):
    """bytes if each changed tile sent as its own kitty f=24 o=z payload (+b64 +~60B ctrl)"""
    tot=0
    for i,j,sb in boxes:
        raw=np.ascontiguousarray(sb).tobytes()
        c=zlib.compress(raw,1)
        tot += int(len(c)*4/3) + 70   # base64 expansion + control data overhead
    return tot

def cost_merged(boxes, tw, th, img):
    """bytes if changed tiles merged into one bounding rect"""
    if not boxes: return 0
    xs=[b[0] for b in boxes]; ys=[b[1] for b in boxes]
    x0,x1=min(xs)*tw,(max(xs)+1)*tw; y0,y1=min(ys)*th,(max(ys)+1)*th
    crop=np.asarray(img)[y0:y1,x0:x1]
    c=zlib.compress(np.ascontiguousarray(crop).tobytes(),1)
    return int(len(c)*4/3)+70, (x1-x0,y1-y0)

def full_cost(img):
    c=zlib.compress(np.asarray(img).tobytes(),1)
    return int(len(c)*4/3)+70

tall=Image.open("/tmp/dmg/tall_wiki.png").convert("RGB")
print("=== SCROLL (1440x900 viewport, wikipedia article) ===")
print(f"{'scroll dy':>10} {'tiles chg':>10} {'/total':>8} {'%':>6} {'tiled B':>10} {'merged B':>10} {'full B':>10} {'saving':>8}")
base=tall.crop((0,0,1440,900))
fullB=full_cost(base)
for dy in (1,3,10,40,100,300,900):
    cur=tall.crop((0,dy,1440,900+dy))
    n,t,boxes=tiles_changed(base,cur,CELL_W*8,CELL_H*4)   # 64x68 px tiles
    tb=cost_zlib(boxes,64,68)
    mb,dim=cost_merged(boxes,64,68,cur)
    fb=full_cost(cur)
    print(f"{dy:>10} {n:>10} {t:>8} {100*n/t:5.1f}% {tb:>10} {mb:>10} {fb:>10} {100*(1-min(tb,mb)/fb):7.1f}%")

print()
print("=== TYPING (same page, chars added to a text box) ===")
print(f"{'transition':>14} {'tiles chg':>10} {'/total':>8} {'%':>6} {'tiled B':>10} {'merged B':>10} {'full B':>10} {'saving':>8}")
for a,b in (("0","1"),("1","2"),("2","40")):
    A=Image.open(f"/tmp/dmg/type_{a}.png").convert("RGB")
    B=Image.open(f"/tmp/dmg/type_{b}.png").convert("RGB")
    n,t,boxes=tiles_changed(A,B,64,68)
    tb=cost_zlib(boxes,64,68); mb,dim=cost_merged(boxes,64,68,B); fb=full_cost(B)
    print(f"{a+'->'+b:>14} {n:>10} {t:>8} {100*n/t:5.1f}% {tb:>10} {mb:>10} {fb:>10} {100*(1-min(tb,mb)/fb):7.1f}%")

print()
print("=== TILE SIZE SWEEP (scroll dy=3, the worst realistic case) ===")
cur=tall.crop((0,3,1440,903))
for tw,th in ((16,17),(32,34),(64,68),(128,136),(240,180),(1440,900)):
    n,t,boxes=tiles_changed(base,cur,tw,th)
    tb=cost_zlib(boxes,tw,th)
    print(f"  tile {tw:>4}x{th:<4} changed {n:>5}/{t:<5} ({100*n/t:5.1f}%)  tiled cost {tb:>9} B  ({tb/1024:6.1f} KiB)")
