#!/usr/bin/env python3
"""SPIKE: prove the Kitty graphics protocol path end-to-end in a real terminal.

Emits a PNG via the Kitty graphics protocol APC sequence:
    ESC _ G <control-data> ; <base64-payload> ESC \\

Control keys used:
    a=T   action = transmit AND display immediately
    f=100 payload format = PNG (Kitty decodes it for us)
    q=2   quiet: suppress both OK and error replies (we are not reading them here)
    m=1/0 more-chunks flag; payload must be split into <=4096-byte base64 chunks

Run this inside Ghostty/kitty/WezTerm. If the terminal supports the protocol the image
appears inline; if it does not, the escape sequence is swallowed and nothing is drawn.
"""
import base64
import os
import sys

CHUNK = 4096  # Kitty spec: max 4096 bytes of base64 payload per escape sequence


def emit_png(path: str) -> int:
    with open(path, "rb") as fh:
        payload = base64.standard_b64encode(fh.read())

    out = sys.stdout.buffer
    chunks = [payload[i:i + CHUNK] for i in range(0, len(payload), CHUNK)]
    for idx, chunk in enumerate(chunks):
        first = idx == 0
        more = 1 if idx < len(chunks) - 1 else 0
        if first:
            control = f"a=T,f=100,q=2,m={more}"
        else:
            # Continuation chunks carry only the m= key.
            control = f"m={more}"
        out.write(b"\x1b_G" + control.encode("ascii") + b";" + chunk + b"\x1b\\")
    out.flush()
    return len(chunks)


if __name__ == "__main__":
    png = sys.argv[1] if len(sys.argv) > 1 else "out/example-com.png"
    if not os.path.exists(png):
        print(f"missing {png}", file=sys.stderr)
        sys.exit(1)
    size = os.path.getsize(png)
    print(f"BLACKGLASS KITTY GRAPHICS PROOF -- TERM={os.environ.get('TERM')} "
          f"TERM_PROGRAM={os.environ.get('TERM_PROGRAM')}")
    print(f"transmitting {png} ({size} bytes) ...")
    n = emit_png(png)
    print(f"\nsent {n} chunk(s). If you see a rendered web page above, the protocol works.")
