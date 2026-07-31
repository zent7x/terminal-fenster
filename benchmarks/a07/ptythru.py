import os, pty, time, select, tty, sys, termios

def bench(total_mb=64, chunk=65536, raw=True):
    pid, fd = pty.fork()
    if pid == 0:
        buf = b"A"*chunk
        n = (total_mb*1024*1024)//chunk
        for _ in range(n):
            try: os.write(1, buf)
            except OSError: break
        os._exit(0)
    if raw:
        try: tty.setraw(fd)
        except Exception: pass
    got=0; t0=time.perf_counter()
    while True:
        r,_,_=select.select([fd],[],[],2.0)
        if not r: break
        try: c=os.read(fd, 1<<20)
        except OSError: break
        if not c: break
        got+=len(c)
    dt=time.perf_counter()-t0
    try: os.waitpid(pid,0)
    except Exception: pass
    return got, dt

for chunk in (4096, 16384, 65536):
    got,dt=bench(48, chunk)
    print(f"pty write chunk {chunk:>6} B: {got/1e6:8.1f} MB in {dt:6.3f}s = {got/dt/1e6:8.1f} MB/s")

# measure per-write syscall cost at kitty's 4096-byte chunk limit
import statistics
pid, fd = pty.fork()
if pid==0:
    os._exit(0)
os.close(fd) if False else None
