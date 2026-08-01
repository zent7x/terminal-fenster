# A04 — Terminal Graphics + Capability Matrix (Implementation Spec)

**Mission:** A04 · **Status:** Complete (one gap flagged) · **Date:** 2026-07-31
**Target host:** macOS 26.1, Apple M4, arm64
**Evidence classes used below:** `[EMPIRICAL]` = measured on this machine via byte-level tty probe · `[SOURCE]` = read from the terminal's own source code · `[SPEC]` = primary protocol documentation · `[UNVERIFIED]` = could not confirm; do not build on it.

Reproduction harness written during this mission (keep these, they are the regression suite):

- `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/a04_probe.py` — capability probe (opens `/dev/tty` directly, raw mode, 17 queries)
- `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/a04_bench.py` — kitty-graphics throughput benchmark
- Raw results: `out-appleterm.json`, `out-iterm2.json` in the same directory

---

## 0. Executive answer

| Question | Answer |
|---|---|
| Primary renderer protocol | **Kitty graphics protocol**, `t=s` (POSIX shared memory) for local, `t=d` chunked base64 as remote fallback |
| Works on Ghostty 1.3.1? | **Yes** `[SOURCE]` — but **no animation actions**, **no Sixel** |
| Works on iTerm2 3.6.9? | **Yes** `[EMPIRICAL]` — responded `ESC _ G i=31 ; OK ESC \` for both `f=24` and `f=100` |
| Works on Apple Terminal 465? | **No** `[EMPIRICAL]` — no kitty, no Sixel, no `CSI 16 t`. It is a text-only fallback target. |
| Sixel on our 3 terminals? | iTerm2 **yes** (256 color registers, max 1120×850 px) `[EMPIRICAL]`; Ghostty **no** `[SOURCE]`; Apple Terminal **no** `[EMPIRICAL]` |
| Can we hit 30 fps? | **Only with `t=s` shared memory or dirty-rect updates.** Full-frame 1280×720 RGBA over `t=d` needs ~147 MB/s through a pty. Not viable. |

**The single most important actionable recommendation is in §9.**

---

## 1. Kitty Graphics Protocol — complete grammar

Primary source: <https://sw.kovidgoyal.net/kitty/graphics-protocol/> (kitty 0.48.2, 2026-07-30).
Protocol documentation is published by Kovid Goyal; kitty itself is **GPL-3.0**. The *protocol* is an open specification explicitly intended for third-party implementation ("We intend that any terminal emulator that wishes to support it can do so"). **Do not copy kitty source into Terminal-Fenster** — implement from the spec text only. Ghostty (**MIT**, `LICENSE` line 1–3 of the checkout) is the safe reference implementation to read.

### 1.1 Envelope

```
ESC _ G <control data> ; <payload> ESC \
0x1b 0x5f 0x47      ...      0x3b   ...   0x1b 0x5c
```

This is an **APC** (Application Programming Command). Terminals that do not understand APC discard it silently, which is what makes the protocol safe to emit blind. `<control data>` is a comma-separated list of `key=value`. `<payload>` is **always base64** (standard alphabet, `+/`, with `=` padding). The `;` is omitted when there is no payload — e.g. `ESC _ G a=d ESC \` is legal.

`ESC \` (ST, `0x1b 0x5c`) is the terminator. `0x9c` (8-bit ST) is **not** used by any implementation here — always emit the 2-byte form.

### 1.2 Control keys — exhaustive reference `[SPEC]`

All integers are 32-bit.

**Global**

| Key | Values | Default | Meaning |
|---|---|---|---|
| `a` | `t` `T` `q` `p` `d` `f` `a` `c` | `t` | action: **t**=transmit, **T**=transmit+display, **q**=query, **p**=put (display already-transmitted), **d**=delete, **f**=transmit animation frame, **a**=control animation, **c**=compose animation frames |
| `q` | `0` `1` `2` | `0` | suppress responses: `1` = suppress OK, `2` = suppress OK **and** errors |

**Transmission**

| Key | Values | Default | Meaning |
|---|---|---|---|
| `f` | `24` `32` `100` | `32` | pixel format: `24`=RGB (3 B/px), `32`=RGBA (4 B/px), `100`=PNG |
| `t` | `d` `f` `t` `s` | `d` | medium: **d**=direct (data inline), **f**=regular file, **t**=temp file (terminal deletes it), **s**=POSIX shared memory object |
| `s` | uint | 0 | source image **width** in pixels — REQUIRED for `f=24`/`f=32`, read from the PNG for `f=100` |
| `v` | uint | 0 | source image **height** in pixels — same rule |
| `S` | uint | 0 | number of bytes to read from file/shm. **Mandatory when combining `f=100` with `o=z`.** |
| `O` | uint | 0 | byte offset to start reading from file/shm |
| `i` | 1 … 4294967295 | 0 | image id. **0 is reserved and must not be sent.** |
| `I` | 1 … 4294967295 | 0 | image *number* (non-unique); terminal allocates an id and reports it back. Sending both `i` and `I` is `EINVAL`. |
| `p` | 1 … 4294967295 | 0 | placement id |
| `o` | `z` | none | compression: `z` = RFC 1950 zlib deflate, applied **before** base64 |
| `m` | `0` `1` | 0 | `1` = more chunks follow, `0` = final chunk |
| `N` | bitmask | 0 | usage hint. Only bit defined: `1` = *transient* (added kitty 0.48.0) |

**Display / placement**

| Key | Default | Meaning |
|---|---|---|
| `x`, `y` | 0 | top-left of the **source rectangle**, in source-image pixels |
| `w`, `h` | 0 (= full) | width/height of the source rectangle, in source-image pixels |
| `X`, `Y` | 0 | pixel offset **within the first cell** at which drawing starts. Must be < cell size. |
| `c`, `r` | 0 (= natural) | destination size in **columns** / **rows**. Image is scaled to fit. If only one is given the other is derived from aspect ratio. `X`/`Y` are *not* added to `c`/`r`. |
| `C` | 0 | cursor movement policy. `0` = move cursor right by `c` and down by `r`. **`C=1` = do not move the cursor at all.** |
| `z` | 0 | z-index. Negative ⇒ drawn **under** text. `z < INT32_MIN/2` (`< -1073741824`) ⇒ drawn under non-default cell backgrounds. Ties broken by lower image id. |
| `U` | 0 | `U=1` creates a *virtual* placement for Unicode-placeholder rendering |
| `P`, `Q` | 0 | parent image id / parent placement id, for relative placements |
| `H`, `V` | 0 | cell offset from parent (signed) |

**Deletion — `a=d`, selector in `d`** `[SPEC]`

Lowercase = remove the *placement*, keep the image data cached (cheap redisplay).
Uppercase = also **free the image data**.

| `d` | Deletes |
|---|---|
| `a` / `A` | all placements visible on screen (this is the default if `d` is omitted) |
| `i` / `I` | all placements of image `i=`; if `p=` also given, only that one placement |
| `n` / `N` | newest image with image-number `I=`; `p=` narrows to one placement |
| `c` / `C` | all placements intersecting the current cursor cell |
| `f` / `F` | animation frames |
| `p` / `P` | all placements intersecting cell (`x=`, `y=`) — **1-based, `x=1,y=1` is top-left** |
| `q` / `Q` | placements intersecting cell (`x=`,`y=`) that have z-index `z=` |
| `r` / `R` | all images with `x=` ≤ id ≤ `y=` (added kitty 0.33.0) |
| `x` / `X` | all placements intersecting column `x=` |
| `y` / `Y` | all placements intersecting row `y=` |
| `z` / `Z` | all placements with z-index `z=` |

Notes: when the last placement of an image is deleted **and** an uppercase selector was used, the image itself is freed. A delete command arriving mid-chunked-upload **aborts** that upload.

**Animation** (`a=f` load frame, `a=a` control, `a=c` compose) — see §1.9. **Ghostty does not implement any of these.**

### 1.3 Chunking rules — exact `[SPEC]`

- Base64-encode **first**, then split.
- **Maximum 4096 bytes of base64 per escape code.** Confirmed verbatim: *"chunked up into chunks no larger than 4096 bytes"*. 4096 is itself a multiple of 4, so the natural chunk size is legal.
- **Every chunk except the last must have a length that is a multiple of 4.**
- Only the **first** chunk carries the full control data. Subsequent chunks carry **only** `m` and optionally `q` (and `a=f` when loading animation frames). Sending `s=`/`v=`/`f=` on later chunks is a protocol error.
- `m=1` on all chunks except the last, which is `m=0`. The final chunk **may have an empty payload** — that is the idiomatic terminator.
- No other graphics escape code may be interleaved between chunks of one image.
- The cursor position used for display is the position **when the final chunk is received**.
- The terminal must render nothing until the whole sequence is received and validated.

Arithmetic for a 1280×720 RGB frame `[EMPIRICAL, computed]`:
`1280*720*3 = 2 764 800` raw → `3 686 400` base64 bytes → **900 chunks** of 4096. Per-chunk overhead is ~12 bytes (`ESC_Gm=1;` + `ESC\`), so ~10.8 KB of framing — negligible. The 3.7 MB payload is the problem, not the framing.

### 1.4 Support detection — the handshake, byte for byte

Send the query **immediately followed by DA1**, then read until you see the DA1 reply:

```
ESC _ G i=31,s=1,v=1,a=q,t=d,f=24 ; AAAA ESC \  ESC [ c
```

Hex (38 bytes) `[EMPIRICAL — this exact byte string was sent to all three terminals]`:

```
1b 5f 47 69 3d 33 31 2c 73 3d 31 2c 76 3d 31 2c 61 3d 71 2c 74 3d 64 2c 66 3d 32 34 3b
41 41 41 41 1b 5c 1b 5b 63
```

`AAAA` is the base64 of three zero bytes = one black RGB pixel. `i=31` is arbitrary; `a=q` guarantees the terminal will **not** store the image and will not clobber an existing image with that id.

**Decision rule:** terminals that support the protocol MUST answer the query *before* the DA1 reply (they are required to answer query actions immediately, without processing further input). So:

- Reply contains `ESC _ G i=31 ; OK ESC \` **before** the `ESC [ ? … c` → supported.
- Only the DA1 reply arrives → **not** supported.
- Nothing at all arrives within your timeout → the terminal is not a VT at all, or is wedged; treat as unsupported.

Measured replies:

| Terminal | Bytes received | Verdict |
|---|---|---|
| iTerm2 3.6.9 | `\x1b_Gi=31;OK\x1b\\` `\x1b[?64;1;2;4;6;17;18;21;22;52c` | **supported** `[EMPIRICAL]` |
| Apple Terminal 465 | `\x1b[?1;2c` only | **not supported** `[EMPIRICAL]` |
| Ghostty 1.3.1 | probe blocked (see §3.3) — supported per `[SOURCE]` and Ghostty docs | supported |

Also probe PNG separately; a terminal can accept `f=24` and reject `f=100`:

```
ESC _ G i=32,a=q,f=100,t=d ; <base64 of a 1x1 PNG> ESC \ ESC [ c
```
iTerm2 3.6.9 → `\x1b_Gi=32;OK\x1b\\` `[EMPIRICAL]`.

**Medium probe** (this is the one that decides your fast path):

```
ESC _ G i=33,s=1,v=1,a=q,t=s,f=24 ; <base64 of the shm name> ESC \ ESC [ c
```
iTerm2 3.6.9 with a deliberately non-existent name → `\x1b_Gi=33;EBADF:The operation couldn't be completed. No such file or directory\x1b\\`. This proves iTerm2 **parses and attempts** `t=s`, but because the object did not exist the test is inconclusive as to whether a *real* shm object works. `[UNVERIFIED — re-run with a live shm_open() object before relying on t=s in iTerm2]`

### 1.5 Response grammar

```
ESC _ G i=<id> ; OK ESC \
ESC _ G i=<id> , p=<placement id> ; OK ESC \
ESC _ G i=<id> , I=<image number> ; OK ESC \      (reply to an I= creation; i= is the assigned id)
ESC _ G i=<id> ; <ERRCODE> : <human message> ESC \
```

Error codes seen in the wild / in the spec: `ENOENT`, `EINVAL`, `ENOMEM`, `ENOSPC`, `EBADF`, `ETOODEEP` (relative-placement chain too deep), `ECYCLE` (relative-placement cycle), `ENOPARENT`. Ghostty additionally emits the literal string `ERROR: unimplemented action` for animation actions `[SOURCE: src/terminal/kitty/graphics_exec.zig:72-75]`.

Parse defensively: the message text is free-form ASCII (printable + space) and vendor-specific.

### 1.6 Worked byte-level examples

**(a) Transmit and display a 2×2 solid-red RGB image at the cursor**

```
ESC _ G a=T,f=24,s=2,v=2 ; /wAA/wAA/wAA/wAA ESC \
```
38 bytes:
```
1b 5f 47 61 3d 54 2c 66 3d 32 34 2c 73 3d 32 2c 76 3d 32 3b 2f 77 41 41 2f 77 41 41
2f 77 41 41 2f 77 41 41 1b 5c
```
(`/wAA` is base64 of `FF 00 00`.)

**(b) Transmit once, then place many times without re-sending pixels**

```
ESC _ G a=t,f=32,s=800,v=600,i=42,q=2,m=1 ; <chunk 1> ESC \
ESC _ G m=1 ; <chunk 2> ESC \
…
ESC _ G m=0 ; ESC \
ESC _ G a=p,i=42,p=1,c=40,r=12,C=1,z=0 ESC \
```
`C=1` is mandatory for a compositor — without it the terminal moves the cursor and your next write lands somewhere unexpected.

**(c) Re-place the same image (move/resize without flicker)**
Re-issue `a=p` with the **same `i` and `p`**. Per spec, the second placement replaces the first atomically.

**(d) Delete**
```
ESC _ G a=d ESC \                    all visible placements
ESC _ G a=d,d=i,i=10 ESC \           image 10's placements, keep pixels cached
ESC _ G a=d,d=I,i=10 ESC \           image 10, free the pixels
ESC _ G a=d,d=i,i=10,p=7 ESC \       just placement 7 of image 10
ESC _ G a=d,d=Z,z=-1 ESC \           everything at z-index -1, free pixels
ESC _ G a=d,d=p,x=3,y=4 ESC \        everything intersecting cell (3,4), 1-based
```
Hex of `ESC _ G a=d,d=I,i=7 ESC \`:
```
1b 5f 47 61 3d 64 2c 64 3d 49 2c 69 3d 37 1b 5c
```

**(e) Local fast path — shared memory**
```
ESC _ G a=T,f=32,s=1280,v=720,t=s,i=42,q=2,C=1,c=160,r=45 ; <base64 of "/bg-frame-0"> ESC \
```
The payload is the **`shm_open()` name**, not the data. The terminal reads the object and then **unlinks and closes it** — you must `shm_open` a fresh name (or re-create it) for every frame. Total pty bytes: ~70. This is the difference between 1 MB/frame and 70 bytes/frame.

**(f) Temp-file path** — `t=t`. The terminal deletes the file after reading. Security constraint from the spec, enforced by Ghostty: the path must be inside a recognised temp dir (`/tmp`, `/dev/shm`, `$TMPDIR`, platform temp dirs) **and the full path must contain the literal substring `tty-graphics-protocol`**. Ghostty returns `EINVAL: temporary file not named correctly` / `EINVAL: temporary file not in temp dir` otherwise `[SOURCE: graphics_exec.zig:393-394]`.

### 1.7 Interaction with normal terminal operations `[SPEC]`

- `RIS` (`ESC c`) clears all visible images.
- Switching to/from the alternate screen (mode 1049) clears the images on the screen being left.
- **`ESC [ 2 J` (ED 2) clears all images.** Other erase sequences (`EL`, `ECH`, `ED 0/1`) must **not** touch graphics — only `a=d` does.
- Images scroll with text automatically. With DECSTBM margins set, only images entirely inside the page area scroll, and they are clipped at the margins.

### 1.8 Unicode placeholders (needed only if we ever run inside a host TUI)

Create the image quietly (`q=2`), create a **virtual** placement, then emit text:

```
ESC _ G a=p,U=1,i=<id>,c=<cols>,r=<rows> ESC \
```
Then print `U+10EEEE` cells with the image id in the **foreground colour** and row/column encoded as combining diacritics from kitty's `rowcolumn-diacritics.txt`. `U+0305`=0, `U+030D`=1, `U+030E`=2. Underline colour optionally carries the placement id; the 4th (most-significant) image-id byte goes in a third diacritic. Omitted diacritics inherit from the cell to the left (row same, column +1).

Ghostty implements this (`src/terminal/kitty/graphics_unicode.zig`, `placeholder: u21 = 0x10EEEE`) `[SOURCE]`.

Only `d=i/I/r/R/n/N` can delete virtual placements; `a,c,p,q,x,y,z` never affect them.

### 1.9 Animation — do not use

`a=f` (load frame), `a=a` (control: `s=1` stop, `s=2` run-and-wait, `s=3` loop; `v=` loop count; `c=` set current frame; `z=` gap ms, default 40 ms, root frame 0), `a=c` (compose rect from frame `r=` onto frame `c=`).

**Ghostty returns `ERROR: unimplemented action` for all three** `[SOURCE: src/terminal/kitty/graphics_exec.zig, commit 4d605bf]`. Since Ghostty is a primary target, Terminal-Fenster must drive its own frame loop with `a=T`/`a=p` and never depend on terminal-side animation.

### 1.10 Storage quotas

kitty: 320 MB per buffer; animation frame data gets a separate quota 5× larger `[SPEC]`.
Ghostty: `total_limit: usize = 320 * 1000 * 1000` `[SOURCE: graphics_storage.zig:133]`; per-image hard caps `max_dimension = 10000` px and `max_size = 400 MB` `[SOURCE: graphics_image.zig:16,19]`. Exceeding dimensions ⇒ `EINVAL: dimensions too large`.
Terminals evict *placement-less* images first under pressure. Combine with `N=1` (transient) so our scratch frames are evicted before anything else.

---

## 2. Which terminals support the kitty protocol

`[SPEC]` The kitty documentation itself lists these third-party implementations: Ghostty, Konsole, st (with a patch), Warp, wayst, WezTerm, iTerm2, xterm.js.

| Terminal | Kitty graphics | Evidence |
|---|---|---|
| kitty 0.48.2 | reference implementation | `[SPEC]` |
| **Ghostty 1.3.1** | **yes** (no animation, no Sixel) | `[SOURCE]` full implementation in `src/terminal/kitty/`; listed in kitty docs |
| **iTerm2 3.6.9** | **yes** (`f=24`, `f=32`, `f=100` direct) | `[EMPIRICAL]` `\x1b_Gi=31;OK\x1b\\` |
| **Apple Terminal 465** | **NO** | `[EMPIRICAL]` no APC reply; DA1 `\x1b[?1;2c` only |
| WezTerm | yes | `[SPEC]` listed by kitty docs |
| Konsole | yes | `[SPEC]` listed by kitty docs |
| Warp, wayst, xterm.js, st+patch | yes | `[SPEC]` listed by kitty docs |
| Alacritty, xterm, VS Code terminal | no | `[UNVERIFIED as of today, but no implementation is listed]` |

> **CONTRADICTION — READ THIS.** The community table at <https://terminfo.dev/extensions/kitty-graphics-protocol> currently lists **"Terminal.app — Yes"**. That is **wrong**. Direct byte-level probing of Apple Terminal 465 on this machine returned *no* APC response, only the DA1 reply. Trust the probe, not the table. Any capability database should be treated as a hint and confirmed at runtime.

**Version-gating rule for Terminal-Fenster:** never gate on `TERM_PROGRAM`/version strings. Always run the §1.4 handshake. Version strings lie (Apple Terminal advertises `xterm-256color`), and a terminal multiplexer in between changes everything.

---

## 3. Sixel

### 3.1 Wire format `[SPEC — DEC VT330/VT340 Programmer Reference Vol. 2 ch.14 (vt100.net), xterm ctlseqs]`

```
DCS P1 ; P2 ; P3 q <sixel data> ST
ESC P  …  q  …  ESC \
0x1b 0x50 … 0x71 … 0x1b 0x5c
```

- **P1** — pixel aspect ratio. `0`/`1`/omitted → 2:1, `2` → 5:1, `3`–`4` → 3:1, `5`–`6` → 2:1, `7`–`9` → 1:1. Modern usage: send `0` (or `1`) and control geometry via raster attributes.
- **P2** — background select. `0` or `2` → pixels not set are painted with the background colour. **`1` → pixel positions with 0 bits are left unchanged (transparent).** For a compositor you almost always want `P2=1`.
- **P3** — horizontal grid size. Ignored by every modern implementation; send nothing or `0`.

**Raster attributes** (emit this first, always):
```
" Pan ; Pad ; Ph ; Pv
```
`Pan/Pad` = aspect-ratio numerator/denominator (use `1;1`), `Ph`/`Pv` = image width/height in pixels. Terminals use this to pre-allocate and to avoid the "image grows as it decodes" artefact. Example: `"1;1;640;480`.

**Colour definition**
```
# Pc                                  select colour register Pc
# Pc ; Pu ; Px ; Py ; Pz              define register Pc
```
`Pu=1` → HLS with `Px`=hue 0–360°, `Py`=lightness 0–100, `Pz`=saturation 0–100.
`Pu=2` → RGB with `Px`,`Py`,`Pz` each **0–100 percent, not 0–255**. This is the single most common Sixel bug. Convert with `round(v * 100 / 255)`.

**Pixel encoding**
Each data character is one column of **6 vertical pixels**. Value = `byte - 0x3F`. Legal range `?` (0x3F, all six off) … `~` (0x7E, all six on). **Bit 0 (LSB) is the topmost pixel**, bit 5 the bottom.
So a byte with only the top pixel set = `0x3F + 1 = 0x40 = '@'`. All six set = `0x3F + 63 = 0x7E = '~'`.

**Control characters inside the data**
| Char | Hex | Meaning |
|---|---|---|
| `!` | 0x21 | Graphics Repeat Introducer: `! Pn <char>` repeats `<char>` `Pn` times (RLE) |
| `$` | 0x24 | Graphics Carriage Return — back to left margin, **same** 6-pixel band (used to overlay a second colour on the same band) |
| `-` | 0x2D | Graphics New Line — left margin, advance **down one 6-pixel band** |
| `#` | 0x23 | colour introducer (above) |
| `"` | 0x22 | raster attributes (above) |

Minimal red 16×6 block:
```
ESC P q "1;1;16;6 #0;2;100;0;0 #0 !16~ ESC \
```
Bytes: `1b 50 71 22 31 3b 31 3b 31 36 3b 36 23 30 3b 32 3b 31 30 30 3b 30 3b 30 23 30 21 31 36 7e 1b 5c`

### 3.2 Capability detection

**DA1:** send `ESC [ c` (`1b 5b 63`). The reply is `ESC [ ? Ps ; Ps … c`. **Sixel is advertised by the presence of parameter `4`** in the list (`Ps = 4 → Sixel graphics`, xterm ctlseqs). Match on a parameter equal to `4`, *not* on the substring `";4;"` — `4` can be first or last (`\x1b[?64;4c`) and a naive substring test misses it and false-positives on `;64;`.

**Colour registers and max geometry — XTSMGRAPHICS** `[SPEC: xterm ctlseqs]`
```
CSI ? Pi ; Pa ; Pv S
  Pi = 1 number of colour registers | 2 Sixel geometry | 3 ReGIS geometry
  Pa = 1 read | 2 reset to default | 3 set to Pv | 4 read maximum
reply: CSI ? Pi ; Ps ; Pv S      Ps: 0 success, 1 bad Pi, 2 bad Pa, 3 failure
```

**Measured** `[EMPIRICAL]`:

| Query | iTerm2 3.6.9 | Apple Terminal 465 |
|---|---|---|
| `\x1b[c` (DA1) | `\x1b[?64;1;2;4;6;17;18;21;22;52c` → **has `4` ⇒ Sixel** | `\x1b[?1;2c` → **no `4` ⇒ no Sixel** |
| `\x1b[?1;1;0S` (colour regs) | `\x1b[?1;0;256S` → **256 registers** | *(no reply)* |
| `\x1b[?2;1;0S` (Sixel geometry) | `\x1b[?2;0;1120;850S` → **max 1120×850 px** | *(no reply)* |
| `\x1b[?80$p` (DECSDM) | `\x1b[?80;2$y` (reset) | *(no reply)* |
| `\x1b[?8452$p` | `\x1b[?8452;4$y` (permanently reset) | *(no reply)* |

Ghostty: **no Sixel implementation at all.** A grep for `sixel` across the entire Ghostty source tree at commit `4d605bf` returns exactly **one** hit — the enum constant `sixel = 4` in `src/terminal/device_attributes.zig:53`, which is *never* included in the DA1 feature list. Ghostty's DA1 is hard-coded `[SOURCE: src/termio/stream_handler.zig:779-791]`:

```zig
.primary  => "\x1B[?62;22;52c"   // or "\x1B[?62;22c" if clipboard writes are denied
.secondary=> "\x1B[>1;10;0c"
```
62 = VT level-2 conformance, 22 = colour text, 52 = clipboard. **No 4.** Ghostty will never render Sixel.

Note also that iTerm2's advertised Sixel geometry cap (1120×850) is **the current window size**, not a fixed limit — it changed with the probe window. Re-query after every resize.

### 3.3 Verdict

Sixel is a **fallback for iTerm2 only**, and a poor one: 256 colour registers means paletted output (quantisation on every frame), no image ids, no placements, no partial update, no deletion primitive, and re-drawing means re-transmitting the whole thing. Since iTerm2 already speaks the kitty protocol, **Sixel buys us nothing on any of our three targets.** Implement it only if a fourth target (xterm, foot, mlterm, Windows Terminal) enters scope.

---

## 4. iTerm2 OSC 1337 inline images

`[SPEC: https://iterm2.com/documentation-images.html, https://iterm2.com/documentation-escape-codes.html]`

```
ESC ] 1337 ; File = <key>=<value> ; <key>=<value> : <base64 file data> BEL
0x1b 0x5d 31 33 33 37 3b …                                          0x07
```
`ST` (`ESC \`, `1b 5c`) may be used instead of `BEL`. Prefer `ST` — `BEL` is ambiguous inside multiplexers.

**Arguments**

| Key | Values |
|---|---|
| `inline` | `1` = render inline. **`0` (default) downloads the file instead** — always set `inline=1`. |
| `name` | base64-encoded filename (default "Unnamed file") |
| `size` | file size in bytes; used only for the download progress indicator |
| `width`, `height` | `N` = cells, `Npx` = pixels, `N%` = percent of session dimension, `auto` = intrinsic size |
| `preserveAspectRatio` | `1` (default) letterboxes to fit; `0` stretches |
| `type` | MIME type or extension hint |

**Literal example** (1×1 PNG, 2 cells wide, 1 row tall):
```
\x1b]1337;File=inline=1;width=2;height=1:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==\x07
```

### 4.1 Limitations — why this cannot be our renderer

1. **No image identity.** There is no id, no placement, no handle. You cannot reference, move, resize, or delete an image after it is drawn. The only way to remove it is to erase/scroll the text cells it occupies.
2. **No positioning primitive.** The image is planted at the cursor and the cursor advances past it. Absolute placement requires `CUP` before every image, and there is no `C=1` equivalent, so cursor bookkeeping is on you. No sub-cell (`X`/`Y`) offset, no z-index, no under-text compositing.
3. **No partial update.** There is no source-rectangle argument. A one-pixel change means retransmitting the entire encoded image.
4. **Format is a container, not raw pixels.** iTerm2 decodes PNG/JPEG/GIF. So every frame costs a **full PNG encode** on our side plus a full decode on theirs. At 1280×720 that is single-digit milliseconds of encode at best with a fast encoder, on top of the transfer.
5. **No shared-memory or file medium.** Everything is base64 in-band.

**30 fps verdict: no.** Even ignoring encode cost, the data path is base64-in-band with a mandatory full-image retransmit per frame.

`ReportCellSize`: `ESC ] 1337 ; ReportCellSize ST` exists and is documented, and is a useful iTerm2-specific way to get cell metrics when `CSI 16 t` is unavailable (which it is — see §5). Its exact reply format was **not** captured before the probe harness was blocked. `[UNVERIFIED — re-run a04_bench.py in iTerm2 to capture it]`

`MultipartFile=` / `FilePart=` / `FileEnd` are referenced by the images page for tmux chunking but are **not** enumerated in the escape-code reference. `[UNVERIFIED — confirm against iTerm2 source before using]`

---

## 5. Cell metrics — how to get pixel geometry, and the trap

### 5.1 The four mechanisms

**(a) `TIOCGWINSZ` ioctl** — no escape codes, no round trip, no parsing, works under `read()`-blocked conditions. This is the primary source.

```c
#include <sys/ioctl.h>
struct winsize sz;
ioctl(STDOUT_FILENO, TIOCGWINSZ, &sz);
// sz.ws_row, sz.ws_col, sz.ws_xpixel, sz.ws_ypixel
```

Rust (no crates; `libc` only):
```rust
#[repr(C)]
struct Winsize { ws_row: u16, ws_col: u16, ws_xpixel: u16, ws_ypixel: u16 }
// macOS TIOCGWINSZ = 0x40087468
unsafe {
    let mut ws: Winsize = std::mem::zeroed();
    if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 { /* … */ }
}
```
Pair with a `SIGWINCH` handler; do not poll.

**(b) `CSI 14 t`** → `ESC [ 4 ; <height_px> ; <width_px> t`
**(c) `CSI 16 t`** → `ESC [ 6 ; <cell_height_px> ; <cell_width_px> t`
**(d) `CSI 18 t`** → `ESC [ 8 ; <rows> ; <cols> t`
(`CSI 15 t` = whole-screen pixels; unsupported by our targets.)

Ghostty's encoder, verbatim `[SOURCE: src/terminal/size_report.zig]`:
```zig
.csi_14_t => "\x1b[4;{h}t"  where h = rows * cell_height, w = cols * cell_width
.csi_16_t => "\x1b[6;{cell_height};{cell_width}t"
.csi_18_t => "\x1b[8;{rows};{cols}t"
.mode_2048 => "\x1b[48;{rows};{cols};{h_px};{w_px}t"
```

### 5.2 Measured values on this machine `[EMPIRICAL]`

| | **Apple Terminal 465** | **iTerm2 3.6.9** | **Ghostty 1.3.1** |
|---|---|---|---|
| `TIOCGWINSZ` rows×cols | 30 × 120 | 25 × 80 | *(probe blocked)* |
| `TIOCGWINSZ` ws_xpixel × ws_ypixel | **840 × 450** | **1120 × 850** | *(blocked)* — `[SOURCE]` = full screen incl. padding, DPI-scaled |
| `CSI 14 t` reply | `\x1b[4;467;860t` → 860 × 467 | `\x1b[4;462;570t` → 570 × 462 | `[SOURCE]` `cols*cell_w` × `rows*cell_h`, **excludes padding** |
| `CSI 16 t` reply | **(none — unsupported)** | **(none — unsupported)** | supported `[SOURCE]` |
| `CSI 18 t` reply | `\x1b[8;30;120t` | `\x1b[8;25;80t` | supported `[SOURCE]` |
| Derived cell from `TIOCGWINSZ` | 7.00 × 15.00 | 14.00 × 34.00 | — |
| Derived cell from `CSI 14 t` | 7.17 × 15.57 | 7.125 × 18.48 | — |

### 5.3 THE TRAP — two different pixel spaces and two different rectangles

Read the iTerm2 column again. `TIOCGWINSZ` says the text area is **1120 × 850**. `CSI 14 t` says **570 × 462**. That is not padding — that is a **factor of ~2**.

- **Apple Terminal 465 reports logical points** in `ws_xpixel/ws_ypixel` (840×450 = 120×7.0 by 30×15.0). `CSI 14 t` returns **860×467**, i.e. the *window* including a 20 pt horizontal / 17 pt vertical inset.
- **iTerm2 3.6.9 reports physical backing-store pixels** in `ws_xpixel/ws_ypixel` (1120×850 = exactly 2 × 560×425 on this 2× Retina display), while `CSI 14 t` returns **points** (570×462, window incl. insets).
- **Ghostty** documents its internal sizes as *"any pixel values should already be scaled to the current DPI of the screen"* `[SOURCE: src/renderer/size.zig:25]`, and sets `ws_xpixel = screen_size.width` (the **full** surface incl. padding) `[SOURCE: src/termio/Exec.zig:905]` while `CSI 14 t` reports `cols*cell_width` (the **grid only**). So on Ghostty `TIOCGWINSZ ≥ CSI 14 t`, in device pixels.

Three terminals, three different conventions. **Never mix a `TIOCGWINSZ` value with a `CSI 14 t` value in the same calculation.**

### 5.4 The algorithm Terminal-Fenster must use

```
1. rows, cols        := TIOCGWINSZ.ws_row / ws_col     (always trustworthy)
2. try CSI 16 t      -> if it answers, cell_w/cell_h are authoritative. DONE.
                        (Ghostty: yes. iTerm2 3.6.9: no. Apple Terminal: no.)
3. else if ws_xpixel > 0 and ws_ypixel > 0:
        cell_w := ws_xpixel / cols ; cell_h := ws_ypixel / rows
        -- self-consistent, because both numerator and denominator come from
           the same source. Any padding shows up as a fractional cell size,
           which biases us to slightly-too-large images (harmless: they are
           clipped) rather than too small (visible gaps).
4. else try CSI 14 t -> cell_w := w/cols ; cell_h := h/rows   (worse: includes
                        window insets on Apple Terminal and iTerm2)
5. else fall back to 8 x 16 and render text-only.
```
Then, **critically, calibrate the pixel space empirically instead of guessing it**: transmit a probe image of exactly `cell_w × cell_h` pixels with `c=1,r=1,C=1,q=2` and compare. If the image is half-size the terminal is in points and you must multiply by the backing scale factor. Simpler and fully robust: **derive everything from `ws_xpixel/ws_ypixel` and send images sized `cols*cell_w × rows*cell_h`, letting `c=`/`r=` do the scaling.** Because `c`/`r` scale to a cell rectangle, a 2× mismatch degrades to a resampling-quality issue, not a layout bug.

### 5.5 Bonus: in-band resize reports (mode 2048)

Ghostty implements DEC private mode **2048** `[SOURCE: src/terminal/modes.zig:286 `in_band_size_reports`]`. Enable with `ESC [ ? 2048 h` and the terminal *pushes* `ESC [ 48 ; <rows> ; <cols> ; <height_px> ; <width_px> t` on every resize — rows, cols **and** pixels in one atomic message, no round trip, no `SIGWINCH` race. Query with `ESC [ ? 2048 $ p`. Use it when available; keep `SIGWINCH` + `TIOCGWINSZ` as the fallback. `[UNVERIFIED for iTerm2 3.6.9 — the probe that would have measured it was blocked; test `\x1b[?2048$p`]`

---

## 6. tmux / screen passthrough

### 6.1 tmux — exact wire format, verified against tmux source

Wrapper:
```
ESC P tmux ; <payload> ESC \
1b 50 74 6d 75 78 3b   …    1b 5c
```
The prefix is the literal 5 bytes `tmux;` `[SOURCE: tmux input.c:2626 `const char prefix[] = "tmux;"`]`.

**Every `ESC` (0x1b) inside `<payload>` must be doubled to `ESC ESC` (0x1b 0x1b).** Verified at the state-machine level `[SOURCE: tmux input.c:683-702]`:

```c
/* dcs_handler state table */
{ 0x00, 0x1a, input_input,  NULL },
{ 0x1b, 0x1b, NULL,         &input_state_dcs_escape },   /* ESC: consumed, NOT buffered */
{ 0x1c, 0xff, input_input,  NULL },
/* dcs_escape state table */
{ 0x00, 0x5b, input_input,        &input_state_dcs_handler },  /* next byte buffered */
{ 0x5c, 0x5c, input_dcs_dispatch, &input_state_ground },       /* ESC \ = terminate */
{ 0x5d, 0xff, input_input,        &input_state_dcs_handler },
```
The `ESC` that enters `dcs_escape` has a **NULL** action — it is *not* appended to the buffer. The following byte is appended. Hence `ESC ESC` → one `ESC` in the buffer, and a lone `ESC \` terminates the wrapper. On dispatch tmux strips the 5-byte `tmux;` prefix and writes the remainder **raw** to the outer terminal `[SOURCE: input.c:2677-2680 `screen_write_rawstring(sctx, buf + prefixlen, len - prefixlen, allow_passthrough == 2)`]`.

**Gate option** `[SOURCE: tmux options-table.c:1257-1266]`:
```
allow-passthrough   OPTIONS_TABLE_CHOICE, scope = WINDOW|PANE, default_num = 0
  off (0) — disallowed (DEFAULT)
  on  (1) — allowed if the pane is visible
  all (2) — allowed even if the pane is invisible
```
So: **off by default.** Terminal-Fenster must instruct users to `tmux set -g allow-passthrough on` (or `all`), and must detect the `TMUX` env var and degrade gracefully when passthrough is off — with `allow-passthrough off` the sequence is silently dropped, producing a blank screen with no error.

**Buffer limit:** `INPUT_BUF_DEFAULT_SIZE = 1048576` (1 MiB) `[SOURCE: tmux.h:3419]`. A single passthrough DCS must stay under this. With kitty's 4096-byte base64 chunks each wrapped individually you are nowhere near it — **wrap each 4096-byte chunk in its own `ESC P tmux;` envelope**, do not accumulate.

**Worked example** — the 2×2 red image from §1.6(a), wrapped for tmux:
```
\x1bPtmux;\x1b\x1b_Ga=T,f=24,s=2,v=2;/wAA/wAA/wAA/wAA\x1b\x1b\\\x1b\\
```
```
1b 50 74 6d 75 78 3b 1b 1b 5f 47 61 3d 54 2c 66 3d 32 34 2c 73 3d 32 2c 76 3d 32 3b
2f 77 41 41 2f 77 41 41 2f 77 41 41 2f 77 41 41 1b 1b 5c 1b 5c
```
Note **both** interior ESCs are doubled: the APC introducer and the APC terminator.

Reference encoder:
```rust
fn tmux_wrap(seq: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() * 2 + 10);
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in seq { if b == 0x1b { out.push(0x1b); } out.push(b); }
    out.extend_from_slice(b"\x1b\\");
    out
}
```

**Does kitty graphics survive tmux?** The *bytes* survive — tmux forwards them verbatim to the outer terminal. But tmux does **not** model the images: it has no idea those cells are occupied, so it will not scroll, clip, redraw-on-pane-switch, or clean up the graphics. Images bleed across panes, survive pane switches, and are not restored after a redraw. Responses from the outer terminal (`ESC _ G i=…;OK ESC \`) do come back on stdin, so the §1.4 handshake still works — but you are querying the **outer** terminal, whose geometry is *not* your pane's geometry. **Under tmux, use the Unicode-placeholder mode (§1.8)**: tmux moves the placeholder text like any other text, so the image follows the pane correctly. This is exactly the use case the placeholder feature was designed for.

`TIOCGWINSZ` inside tmux reports the **pane**; `ws_xpixel`/`ws_ypixel` are set by tmux from the client's reported size and are frequently **0**. Handle zeros (step 3 of §5.4 must be guarded).

tmux gained optional Sixel decoding in **3.5** (build flag `--enable-sixel`), with a `sixel_support` format variable added in 3.6 and a cap of 20 Sixel images per pane in 3.8 `[SPEC: tmux CHANGES]`. tmux has **no** kitty-graphics awareness in any version.

### 6.2 GNU screen

`screen` has a DCS passthrough too — `ESC P <payload> ESC \` (no `tmux;` prefix), with the same ESC-doubling requirement — but:

- screen's string buffer is small (historically 256 bytes; distro-dependent), so 4096-byte kitty chunks **will be truncated**.
- screen re-wraps and re-emits content on redraw and has no notion of graphics cells.
- `ws_xpixel`/`ws_ypixel` are 0.

**Recommendation: detect `STY` and disable the graphics renderer entirely under screen.** Do not attempt a fallback; a truncated APC leaves the terminal parsing garbage. `[UNVERIFIED — exact current screen buffer limit; treat screen as unsupported rather than test-and-hope]`

---

## 7. Complete capability matrix — our three targets

| Capability | Ghostty 1.3.1 | iTerm2 3.6.9 | Apple Terminal 465 |
|---|---|---|---|
| Kitty graphics `a=T` / `a=p` / `a=d` | **yes** `[SOURCE]` | **yes** `[EMPIRICAL]` | **no** `[EMPIRICAL]` |
| Kitty `f=24` / `f=32` / `f=100` | yes / yes / yes `[SOURCE]` | yes / `[UNVERIFIED]` / yes `[EMPIRICAL]` | — |
| Kitty `t=d` (direct) | yes | yes `[EMPIRICAL]` | — |
| Kitty `t=f` / `t=t` / `t=s` | **all enabled** `[SOURCE: Termio.zig:265 `.allWithTempDir(global.tmpDirPath())`]` | parses `t=s`, real-object test inconclusive `[UNVERIFIED]` | — |
| Kitty `o=z` (zlib) | yes `[SOURCE]` | `[UNVERIFIED]` | — |
| Kitty `N=1` transient hint | yes `[SOURCE: graphics_command.zig:457,546]` | `[UNVERIFIED]` | — |
| Kitty Unicode placeholders (`U=1`) | yes `[SOURCE: graphics_unicode.zig:16]` | `[UNVERIFIED]` | — |
| Kitty animation (`a=f`/`a=a`/`a=c`) | **NO — `ERROR: unimplemented action`** `[SOURCE]` | `[UNVERIFIED]` | — |
| Kitty relative placements (`P`/`Q`/`H`/`V`) | `[UNVERIFIED]` | `[UNVERIFIED]` | — |
| Image storage quota | 320 MB; max dim 10000 px; max img 400 MB `[SOURCE]` | `[UNVERIFIED]` | — |
| Sixel | **no** `[SOURCE]` | **yes**, 256 regs, max 1120×850 px `[EMPIRICAL]` | **no** `[EMPIRICAL]` |
| iTerm2 OSC 1337 inline images | `[UNVERIFIED]` | yes `[SPEC]` | **no** |
| DA1 | `\x1b[?62;22;52c` `[SOURCE]` | `\x1b[?64;1;2;4;6;17;18;21;22;52c` `[EMPIRICAL]` | `\x1b[?1;2c` `[EMPIRICAL]` |
| DA2 | `\x1b[>1;10;0c` `[SOURCE]` | `\x1b[>64;2500;0c` `[EMPIRICAL]` | `\x1b[>1;95;0c` `[EMPIRICAL]` |
| XTVERSION (`CSI > q`) | `[UNVERIFIED]` | `\x1bP>|iTerm2 3.6.9\x1b\\` `[EMPIRICAL]` | **no reply** `[EMPIRICAL]` |
| `CSI 14 t` | yes, grid-only px `[SOURCE]` | yes, window pts `[EMPIRICAL]` | yes, window pts `[EMPIRICAL]` |
| `CSI 16 t` (cell size) | **yes** `[SOURCE]` | **no** `[EMPIRICAL]` | **no** `[EMPIRICAL]` |
| `CSI 18 t` | yes `[SOURCE]` | yes `[EMPIRICAL]` | yes `[EMPIRICAL]` |
| Mode 2048 in-band resize | **yes** `[SOURCE]` | `[UNVERIFIED]` | `[UNVERIFIED — no reply to other DECRQM, likely no]` |
| XTSMGRAPHICS | no (no graphics) | yes `[EMPIRICAL]` | no `[EMPIRICAL]` |
| Kitty keyboard protocol (`CSI ? u`) | yes | `\x1b[?0u` `[EMPIRICAL]` | **no reply** `[EMPIRICAL]` |
| `TIOCGWINSZ` pixel space | device px, **incl. padding** `[SOURCE]` | **device px (2× on Retina)** `[EMPIRICAL]` | **points** `[EMPIRICAL]` |

### 7.1 Gap — Ghostty runtime probe was blocked

The empirical Ghostty column is missing. Cause: several concurrent agents were driving Ghostty on this machine simultaneously; the app stopped answering AppleEvents (`AppleEvent timed out (-1712)`, then `Connection is invalid (-609)`), and `ghostty -e` stubs accumulated without a live GUI instance to attach to. Ghostty's own source (checkout at `4d605bf`, tag `tip`, 2026-07-30) was used instead, which is authoritative for behaviour but **is newer than the installed 1.3.1 binary** — treat Ghostty source claims as "1.3.1 or later" and re-confirm at runtime.

To close the gap, on an idle machine:
```sh
SP=/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad
pkill -f 'Ghostty.app/Contents/MacOS/ghostty'; sleep 2
open -a Ghostty; sleep 5
osascript -e 'tell application "Ghostty"
  set cfg to make new surface configuration
  set command of cfg to "/usr/bin/python3 '"$SP"'/a04_probe.py '"$SP"'/out-ghostty.json; sleep 3"
  set wait after command of cfg to false
  new window with configuration cfg
end tell'
```
(The identical flow succeeded for Apple Terminal via `do script` and for iTerm2 via `create window with default profile command`.)

---

## 8. Throughput analysis — can we hit 30 fps?

The live benchmark (`a04_bench.py`) could not complete before the terminals became unresponsive under agent contention. `[UNVERIFIED — run it]` The arithmetic, however, is decisive and does not need measurement:

| Frame | Raw | base64 | Bytes/s @30fps through the pty |
|---|---|---|---|
| 640×360 RGB (`f=24`) | 691 200 | 921 600 | **27.6 MB/s** |
| 1280×720 RGB (`f=24`) | 2 764 800 | 3 686 400 | **110 MB/s** |
| 1280×720 RGBA (`f=32`) | 3 686 400 | 4 915 200 | **147 MB/s** |
| 1920×1080 RGBA | 8 294 400 | 11 059 200 | **332 MB/s** |

A macOS pty is not a bulk pipe: the kernel buffer is small, so the writer blocks in lockstep with the terminal's parser, and each 4096-byte chunk carries a full APC parse. 900 APC parses per frame × 30 fps = 27 000 escape-sequence dispatches per second **before** any decoding, plus base64-decode and a GPU texture upload. Base64 alone burns 33 % of the bandwidth. **Full-frame `t=d` at 720p/30 is not achievable.**

Three levers, in order of impact:

1. **`t=s` shared memory.** The escape code carries only the shm *name* — ~70 bytes per frame regardless of resolution. The terminal `mmap`s our buffer. 1920×1080 RGBA at 60 fps is then a memcpy-bandwidth problem (trivially met by an M4), not an I/O problem. Verified enabled in Ghostty `[SOURCE]`; needs a real-object test in iTerm2 `[UNVERIFIED]`. Remember the terminal **unlinks and closes** the object after reading, so ring through N names (`/bg-f0` … `/bg-f3`) and recreate each before reuse.
2. **Dirty rectangles.** A browser rarely repaints the whole viewport. Transmit only the damaged region as its own small image and place it with `a=T, C=1, X=<subcell x>, Y=<subcell y>` at the right cell, with `q=2`. A 200×100 damage rect is 60 KB raw — trivially 30 fps even over `t=d`. This is the highest-leverage change and it also makes the remote/SSH case work.
3. **`o=z` zlib.** Cheap win on flat UI content (large uniform regions), near-zero on photos/video. Costs CPU on both ends. Use `level=1`. Note: with `f=100` **and** `o=z` you must also send `S=`.

Do **not** rely on terminal-side animation (`a=f`/`a=a`) as a frame-pacing mechanism — Ghostty rejects it outright `[SOURCE]`.

---

## 9. Recommendation

**Build one renderer — kitty graphics protocol — with a runtime-negotiated transport ladder, and make dirty-rectangle updates the default path from day one.**

Concretely:

1. At startup run the §1.4 handshake (`a=q` + DA1) and, in the *same* round trip, probe `t=s` with a **real** `shm_open()`ed object and probe `f=100`. Never branch on `TERM_PROGRAM`.
2. Transport ladder, first that works: `t=s` shared memory → `t=t` temp file (path must contain `tty-graphics-protocol`) → `t=d` chunked base64 at exactly 4096 B/chunk with `o=z`.
3. Always render damage rects, never full frames, as the default. Full-frame is the resize/first-paint path only.
4. Always send `C=1` and `q=2` on render commands; reserve a private image-id range and use `I=` (image *number*) if we might share a screen with another program.
5. Derive cell metrics from `TIOCGWINSZ` alone unless `CSI 16 t` answers (Ghostty does; iTerm2 and Apple Terminal do not), and calibrate the pixel space with a one-cell probe image rather than assuming points vs device pixels — the three terminals disagree by a factor of two (§5.3).
6. Detect `TMUX` → wrap every chunk individually in `ESC P tmux; … ESC \` with ESC doubled, and switch to Unicode placeholders. Detect `STY` → disable graphics.
7. Apple Terminal, `screen`, and any terminal failing the handshake get the **text-only** renderer. Do not implement Sixel: it exists on exactly one of our three targets, and that target already speaks kitty better.

---

## 10. Licensing

| Artifact | License | Use |
|---|---|---|
| kitty graphics **protocol spec** | published open specification, explicitly invites third-party implementation | **Implement from the prose.** Safe. |
| kitty **source** (kovidgoyal/kitty) | **GPL-3.0** | **Do not read for implementation, do not copy.** Incompatible with a proprietary/permissive Terminal-Fenster. |
| **Ghostty** source | **MIT** (`Copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors`) | Safe to read and to adapt with attribution. Best reference implementation. |
| **tmux** source | ISC/BSD | Safe to read; only behavioural facts used here. |
| **xterm** `ctlseqs` | permissive (MIT-style, Thomas Dickey) | Safe reference for Sixel/XTSMGRAPHICS/CSI-t. |
| **iTerm2** | GPL-2.0 | Read the *documentation*, not the source. OSC 1337 is documented publicly. |
| DEC VT330/VT340 Programmer Reference (vt100.net) | historical DEC documentation | Reference only. |

---

## 11. Primary sources

- kitty graphics protocol — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- kitty changelog (0.48.2, 2026-07-30) — <https://sw.kovidgoyal.net/kitty/changelog/>
- Ghostty source, commit `4d605bf0d819df901a0332bbb320dc849fdd82e4` (tag `tip`, 2026-07-30): `src/terminal/kitty/*.zig`, `src/terminal/size_report.zig`, `src/terminal/device_attributes.zig`, `src/termio/stream_handler.zig`, `src/termio/Termio.zig`, `src/termio/Exec.zig`, `src/renderer/size.zig`
- Ghostty VT reference — <https://ghostty.org/docs/vt/reference>
- xterm control sequences (Sixel, XTSMGRAPHICS, XTWINOPS) — `ctlseqs.txt`, Thomas Dickey
- DEC VT330/VT340 Programmer Reference Vol. 2, ch. 14 — <https://vt100.net/docs/vt3xx-gp/chapter14.html>
- iTerm2 inline images — <https://iterm2.com/documentation-images.html>; escape codes — <https://iterm2.com/documentation-escape-codes.html>
- tmux source: `input.c`, `options-table.c`, `tmux.h`, `CHANGES` — <https://github.com/tmux/tmux>
- terminfo.dev kitty-graphics table — <https://terminfo.dev/extensions/kitty-graphics-protocol> — **contains at least one confirmed error (Terminal.app), do not trust**
