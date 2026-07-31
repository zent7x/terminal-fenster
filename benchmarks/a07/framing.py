import base64, zlib
import numpy as np
from PIL import Image

# 1. base64 alphabet safety check
alpha=set(base64.b64encode(bytes(range(256))*3))
bad=[c for c in alpha if c in (0x0a,0x0d,0x09,0x11,0x13,0x1b,0x00,0x7f)]
print("base64 output alphabet:", "".join(sorted(chr(c) for c in alpha)))
print("dangerous bytes present in b64 alphabet:", bad, "->", "SAFE" if not bad else "UNSAFE")
print()

# 2. exact kitty framing overhead
def kitty_cost(payload_bytes, first_ctrl_len=48):
    """payload_bytes = raw compressed bytes; returns total bytes on the wire."""
    b64 = (payload_bytes + 2)//3*4
    nchunks = (b64 + 4095)//4096
    # first chunk: ESC _ G <ctrl> ; <=4096 ESC \    = 3 + ctrl + 1 + data + 2
    # later chunks: ESC _ G m=1 ; data ESC \       = 3 + 4 + 1 + data + 2  (m=0 on last)
    overhead = (3+first_ctrl_len+1+2) + (nchunks-1)*(3+3+1+2)
    return b64 + overhead, nchunks, overhead

print("=== Kitty escape framing overhead (f=24, o=z, direct transmission) ===")
print(f"{'compressed B':>13} {'b64 B':>9} {'chunks':>7} {'frame ovh B':>12} {'wire B':>10} {'ovh %':>7}")
for c in (500, 5_000, 50_000, 150_000, 290_000, 400_000):
    w,n,o = kitty_cost(c)
    print(f"{c:>13} {(c+2)//3*4:>9} {n:>7} {o:>12} {w:>10} {100*o/w:6.2f}%")
print()

# 3. wire cost of a scroll re-placement (a=p with source rect) - no payload
sc = b"\x1b_Ga=p,i=1,p=1,x=0,y=1234,w=1440,h=900,c=180,r=53,q=2\x1b\\"
print(f"=== scroll re-placement escape (source-rect crop), NO pixel data ===")
print(f"    bytes: {len(sc)}  -> at 60fps = {len(sc)*60} B/s = {len(sc)*60/1024:.1f} KiB/s")
print(f"    literal: {sc!r}")
print()

# 4. achievable fps by link speed for each strategy
strategies = {
 "full frame zlib-1 rgb24 (wiki, worst)": 294883,
 "full frame zlib-1 rgb24 (gh, best)":    168934,
 "full frame PNG f=100 L1 (wiki)":        435851,
 "typing dirty-tile update (1 tile)":     426,      # measured 568 incl b64+ctrl
 "scroll via source-rect re-place":       len(sc),
}
links = [("SSH LAN 1Gbit", 125e6), ("100 Mbit", 12.5e6), ("25 Mbit", 3.125e6),
         ("10 Mbit", 1.25e6), ("5 Mbit", 625e3), ("1.5 Mbit", 187.5e3)]
print("=== Max FPS by link (wire bytes incl base64 + framing) ===")
hdr = f"{'strategy':<40}" + "".join(f"{n:>14}" for n,_ in links)
print(hdr); print("-"*len(hdr))
for name,c in strategies.items():
    w,_,_ = kitty_cost(c)
    row=f"{name:<40}"
    for _,bps in links:
        fps = bps/w
        row += f"{fps:>13.1f} " if fps<1000 else f"{'>1000':>13} "
    print(row)
