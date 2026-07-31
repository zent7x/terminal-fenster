import zlib
from PIL import Image
import numpy as np

tall=Image.open("/tmp/dmg/tall_wiki.png").convert("RGB")
base=tall.crop((0,0,1440,900))
def z(a,lvl=1): return len(zlib.compress(np.ascontiguousarray(a).tobytes(),lvl))
def b64(n): return int(n*4/3)

fullbase=b64(z(np.asarray(base)))
print("=== SCROLL-BLIT: only send newly exposed strip ===")
print(f"{'dy':>6} {'strip HxW':>12} {'strip b64 B':>12} {'full b64 B':>11} {'saving':>8}")
for dy in (1,3,10,17,34,100,300):
    cur=np.asarray(tall.crop((0,dy,1440,900+dy)))
    strip=cur[900-dy:900]            # newly exposed rows at the bottom
    sb=b64(z(strip))+70
    fb=b64(z(cur))+70
    print(f"{dy:>6} {str(strip.shape[:2]):>12} {sb:>12} {fb:>11} {100*(1-sb/fb):7.1f}%")

print()
print("=== INTER-FRAME XOR DELTA + zlib (no blit compensation) ===")
A=np.asarray(base,dtype=np.uint8)
for dy in (0,1,3,10,100):
    B=np.asarray(tall.crop((0,dy,1440,900+dy)),dtype=np.uint8)
    d=np.bitwise_xor(A,B)
    print(f"  dy={dy:<4} xor+zlib {b64(z(d)):>9} B   raw frame zlib {b64(z(B)):>9} B   ratio {z(d)/z(B):.2f}")

print()
print("=== TYPING: XOR delta ===")
for a,b in (("0","1"),("2","40")):
    X=np.asarray(Image.open(f"/tmp/dmg/type_{a}.png").convert("RGB"),dtype=np.uint8)
    Y=np.asarray(Image.open(f"/tmp/dmg/type_{b}.png").convert("RGB"),dtype=np.uint8)
    d=np.bitwise_xor(X,Y)
    print(f"  {a}->{b}: xor+zlib {b64(z(d)):>8} B   full {b64(z(Y)):>8} B   saving {100*(1-z(d)/z(Y)):5.1f}%")

print()
print("=== CROSSOVER: dirty-rect vs full-frame by damage fraction ===")
print("  synthetic: damage N random 64x68 tiles of the wiki frame, compare costs")
rng=np.random.default_rng(7)
F=np.asarray(base,dtype=np.uint8).copy()
tx,ty=1440//64, 900//68
tot=tx*ty
print(f"{'dmg tiles':>10} {'dmg %':>7} {'tiled b64 B':>12} {'full b64 B':>11} {'winner':>10}")
for k in (1,4,8,16,32,48,64,96,128,tot):
    idx=rng.choice(tot,size=min(k,tot),replace=False)
    cost=0
    for t in idx:
        i,j=t%tx,t//tx
        sub=F[j*68:(j+1)*68, i*64:(i+1)*64]
        cost+=b64(z(sub))+70
    w = "TILED" if cost<fullbase else "FULL"
    print(f"{k:>10} {100*k/tot:6.1f}% {cost:>12} {fullbase:>11} {w:>10}")
