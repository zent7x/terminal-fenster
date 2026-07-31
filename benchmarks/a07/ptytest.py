import os, pty, termios, tty, select, time, sys

# Test: what does the pty line discipline do to bytes in the OUTPUT direction (slave->master)?
def run(setraw):
    pid, fd = pty.fork()
    if pid == 0:
        # child writes a byte census to its pty slave
        data = bytes(range(256))
        os.write(1, data)
        os._exit(0)
    if setraw:
        try: tty.setraw(fd)
        except Exception as e: pass
    time.sleep(0.4)
    out=b""
    while True:
        r,_,_=select.select([fd],[],[],0.3)
        if not r: break
        try:
            c=os.read(fd,65536)
        except OSError: break
        if not c: break
        out+=c
    os.waitpid(pid,0)
    return out

for setraw in (False,True):
    out=run(setraw)
    sent=bytes(range(256))
    mode = "RAW (cfmakeraw on master)" if setraw else "DEFAULT (cooked/OPOST)"
    print(f"--- {mode}: sent 256 bytes, got {len(out)}")
    if out!=sent:
        # find mangled bytes
        i=0;j=0;probs=[]
        while i<len(sent) and j<len(out):
            if sent[i]==out[j]: i+=1;j+=1
            else:
                probs.append((i,hex(sent[i]),out[j:j+3].hex()))
                # try to resync
                j+=1
                if len(probs)>12: break
        for p in probs[:12]: print(f"    byte idx {p[0]} (0x{int(p[1],16):02x}) -> got {p[2]}")
    else:
        print("    IDENTICAL - no mangling")
