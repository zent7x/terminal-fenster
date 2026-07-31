#!/usr/bin/env python3
"""Terminal capability probe -- run INSIDE the terminal under test.

Screenshot verification is unavailable when the display is asleep, and it is not
CI-able anyway. Instead we use each protocol's own query/response handshake, which is
machine-readable, deterministic, and provable. This is the prototype of the C05 detector.

    ghostty -e python3 term-probe.py --out /tmp/ghostty.json

Every probe writes a query to stdout and reads the reply from stdin in raw mode with a
deadline. A terminal that does not implement a feature simply never replies, so the
timeout IS the negative result -- but note a slow terminal can look like an absent
feature, so the deadline is generous and reported alongside the verdict.
"""
import argparse
import json
import os
import select
import sys
import termios
import tty
import base64

DEADLINE = 1.0  # seconds per probe


def read_reply(deadline=DEADLINE, terminators=(b"\x1b\\", b"\x07")):
    """Collect bytes until a terminator appears or the deadline expires."""
    buf = b""
    end = __import__("time").monotonic() + deadline
    while True:
        remain = end - __import__("time").monotonic()
        if remain <= 0:
            break
        r, _, _ = select.select([sys.stdin.fileno()], [], [], remain)
        if not r:
            break
        chunk = os.read(sys.stdin.fileno(), 4096)
        if not chunk:
            break
        buf += chunk
        if any(t in buf for t in terminators):
            break
        # CSI replies end in a final byte in @-~ ; accept common ones
        if buf.endswith((b"u", b"c", b"t", b"S", b"R")) and buf.startswith(b"\x1b["):
            break
    return buf


def q(sequence, deadline=DEADLINE, terminators=(b"\x1b\\", b"\x07")):
    os.write(sys.stdout.fileno(), sequence)
    return read_reply(deadline, terminators)


def probe_kitty_graphics():
    """Definitive Kitty graphics support test.

    Transmit a 1x1 RGB pixel (f=24, s=1, v=1) with a known image id and ask for a
    response. A conforming terminal replies  ESC _ G i=<id>;OK ESC \\ .
    """
    payload = base64.standard_b64encode(b"\x00\x00\x00")  # one black RGB pixel
    seq = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;" + payload + b"\x1b\\"
    reply = q(seq)
    ok = b"OK" in reply and b"_G" in reply
    # Clean up any stored image so we do not leak state into the user's terminal.
    if ok:
        os.write(sys.stdout.fileno(), b"\x1b_Ga=d,d=I,i=31\x1b\\")
    return {"supported": ok, "raw": reply.decode("latin-1")}


def probe_sixel():
    """Primary DA (CSI c). Reply is CSI ? <params> c ; parameter '4' means sixel."""
    reply = q(b"\x1b[c", terminators=(b"c",))
    txt = reply.decode("latin-1")
    params = ""
    if "[?" in txt:
        params = txt.split("[?", 1)[1].split("c", 1)[0]
    caps = params.split(";") if params else []
    return {"supported": "4" in caps, "da1": txt, "params": caps}


def probe_kitty_keyboard():
    """CSI ? u  -> reply CSI ? <flags> u if the keyboard protocol is implemented."""
    reply = q(b"\x1b[?u", terminators=(b"u",))
    txt = reply.decode("latin-1")
    supported = txt.startswith("\x1b[?") and txt.endswith("u")
    flags = None
    if supported:
        try:
            flags = int(txt[3:-1])
        except ValueError:
            flags = None
    return {"supported": supported, "flags": flags, "raw": txt}


def probe_pixel_size():
    """CSI 14 t -> CSI 4 ; height ; width t   (window size in PIXELS)."""
    reply = q(b"\x1b[14t", terminators=(b"t",))
    txt = reply.decode("latin-1")
    out = {"raw": txt, "height": None, "width": None}
    if ";" in txt:
        parts = txt.strip("\x1b[t").split(";")
        if len(parts) == 3:
            try:
                out["height"], out["width"] = int(parts[1]), int(parts[2])
            except ValueError:
                pass
    return out


def probe_cell_size():
    """CSI 16 t -> CSI 6 ; height ; width t   (single cell size in PIXELS)."""
    reply = q(b"\x1b[16t", terminators=(b"t",))
    txt = reply.decode("latin-1")
    out = {"raw": txt, "cellHeight": None, "cellWidth": None}
    if ";" in txt:
        parts = txt.strip("\x1b[t").split(";")
        if len(parts) == 3:
            try:
                out["cellHeight"], out["cellWidth"] = int(parts[1]), int(parts[2])
            except ValueError:
                pass
    return out


def probe_ioctl_winsize():
    """TIOCGWINSZ gives rows/cols and, on some platforms, pixel dims."""
    import fcntl
    import struct
    try:
        packed = fcntl.ioctl(sys.stdout.fileno(), termios.TIOCGWINSZ, b"\x00" * 8)
        rows, cols, xpix, ypix = struct.unpack("HHHH", packed)
        return {"rows": rows, "cols": cols, "xpixel": xpix, "ypixel": ypix}
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}


def probe_secondary_da():
    reply = q(b"\x1b[>c", terminators=(b"c",))
    return reply.decode("latin-1")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    result = {
        "env": {
            "TERM": os.environ.get("TERM"),
            "TERM_PROGRAM": os.environ.get("TERM_PROGRAM"),
            "TERM_PROGRAM_VERSION": os.environ.get("TERM_PROGRAM_VERSION"),
            "COLORTERM": os.environ.get("COLORTERM"),
            "TMUX": os.environ.get("TMUX"),
        }
    }
    try:
        tty.setraw(fd)
        result["ioctl"] = probe_ioctl_winsize()
        result["kittyGraphics"] = probe_kitty_graphics()
        result["sixel"] = probe_sixel()
        result["kittyKeyboard"] = probe_kitty_keyboard()
        result["pixelSize"] = probe_pixel_size()
        result["cellSize"] = probe_cell_size()
        result["secondaryDA"] = probe_secondary_da()
    finally:
        # RAII discipline: the terminal MUST be restored even on failure.
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)

    with open(args.out, "w") as fh:
        json.dump(result, fh, indent=2)
    print(f"\r\nwrote {args.out}\r")


if __name__ == "__main__":
    main()
