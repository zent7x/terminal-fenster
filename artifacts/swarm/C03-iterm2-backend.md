# C03 — iTerm2 OSC 1337 Inline Image Backend: Specification and Frame-Rate Verdict

**Mission:** C03 · **Date:** 2026-07-31 · **Target host:** macOS 26.1, Apple M4, arm64
**Subject:** iTerm2 3.6.9 (`~/Applications/iTerm.app`, `CFBundleShortVersionString` = `3.6.9`, `CFBundleVersion` = `3.6.9`)

**Evidence classes.** `[SRC]` = read from iTerm2's own published source at `github.com/gnachman/iTerm2@master` (2026-07-31T11:59:22Z), which is authoritative for behaviour. `[BIN]` = extracted from the installed 3.6.9 binary with `strings(1)`, which is authoritative for *this* build. `[MEAS]` = measured on this machine. `[SPEC]` = iTerm2's published documentation. `[UNVERIFIED]` = could not confirm; do not build on it.

**Licence note (Hard Rule 4).** iTerm2 is GPL-2.0 (`gh api repos/gnachman/iTerm2 --jq .license.spdx_id` → `GPL-2.0`). Everything below was read to establish *behaviour*, which is fact and not copyrightable. **No iTerm2 code is to be copied into Terminal-Fenster.** The OSC 1337 wire format is separately published as documentation at `iterm2.com` and is safe to implement from.

---

## 0. Executive answer

| Question from the mission | Answer |
|---|---|
| Exact format? | `ESC ] 1337 ; File = k=v ; … : <base64 of a PNG/JPEG/GIF file> ST`. Full grammar in §1. |
| Can images be **replaced in place**, or only appended? | **Replaced in place — yes.** The image is written into ordinary grid cells, so re-drawing at the same cursor position overwrites it. `[SRC]` This is the good news and it is the only good news. |
| Does every frame scroll? | **Only if you let it.** Rows after the first are advanced by a real linefeed with **no scroll guard on the OSC 1337 path**, so an image that overruns the bottom margin scrolls the screen. Home the cursor and size the image to fit and it does not scroll. `[SRC]` §2.3 |
| Partial / positioned updates? | **Yes, at cell granularity**, via `CUP` before a small image. No source-rectangle, no image id, no placement, no delete. Sub-cell alignment is possible through four **undocumented** inset keys. §3 |
| Can it sustain interactive frame rates? | **No.** Not from the transport — from the **encoder**. OSC 1337 carries a *container format*, so every frame costs a full PNG/JPEG encode. Measured: **73.8 ms** to PNG-encode one content-rich 2482×814 frame on this M4 — 4.4× the entire 16.65 ms frame budget, for a **5.09 MB** base64 payload. §5 |
| What is the fallback? | **There is no need for this backend at all.** iTerm2 3.6.9 implements the *full* kitty graphics protocol — placements, z-index, unicode placeholders, deletes, and both `t=t` temp-file and `t=s` **shared-memory** transfer. `[BIN]` §6 |

> ### Recommendation
> **Delete `Backend::Iterm2` from the backend ladder.** It is dead code on every terminal in scope, and the version that would activate it is strictly worse than the kitty backend already built. The ladder becomes **Kitty → Unicode half-block**, with Sixel and OSC 1337 explicitly out of scope. Full argument and the exact `caps.rs` change in §7.

---

## 1. The wire format, exactly

### 1.1 Envelope

```
ESC ] 1337 ; File = <arg> ; <arg> ; … : <base64 file data> ST
0x1b 0x5d  '1337'  ';'                       ':'            0x1b 0x5c
```

`BEL` (`0x07`) is accepted in place of `ST` `[SPEC]`. **Prefer `ST`** — `BEL` is ambiguous inside multiplexers and inside our own framing.

The payload is a **base64-encoded image file**, not pixels. iTerm2 decodes it through `iTermImage` / ImageIO, so PNG, JPEG and GIF all work `[SRC: sources/InlineImages/iTermImage.m]`.

### 1.2 Complete argument list

This is not the documented list — it is the list iTerm2 3.6.9 actually parses, read from `executeFileCommandWithValue:` `[SRC: sources/VT100/VT100Terminal.m:3904–4005]` and cross-checked against the shipped binary's string table `[BIN]`.

| Key | Values | Default | Notes |
|---|---|---|---|
| `inline` | bool | **`no`** | **Must be `1`.** Left at the default the file is *downloaded*, not displayed. |
| `name` | base64 filename | `Unnamed file` | Cosmetic. Base64-decoded with UTF-8. |
| `size` | integer | `0` | Progress indicator only. |
| `width` | `auto` \| `N` \| `Npx` \| `N%` | `auto` | Bare `N` means **cells**. |
| `height` | `auto` \| `N` \| `Npx` \| `N%` | `auto` | Bare `N` means **cells**. |
| `preserveAspectRatio` | bool | `yes` | Set **`0`** for a browser surface; `1` letterboxes with background-colour bars and you lose viewport edges. |
| `type` | MIME / `.ext` / language | auto-detect | Only affects the text-document path. |
| `mode` | `regular` \| `wide` | `regular` | Text documents only (`forceWide`). |
| `insetTop` | float, fraction of a cell | `0` | **Undocumented.** |
| `insetLeft` | float, fraction of a cell | `0` | **Undocumented.** |
| `insetBottom` | float, fraction of a cell | `0` | **Undocumented.** |
| `insetRight` | float, fraction of a cell | `0` | **Undocumented.** |

The four `inset*` keys are absent from `iterm2.com/documentation-images.html` and from every third-party write-up checked, but they are parsed by `executeFileCommandWithValue:` and passed through to `ImageCharForNewImage(name, width, height, preserveAspectRatio, insets)`, whose header comment states insets are "a fraction of cell size … in [0, 1] … multiplied by cell width and height before rendering" `[SRC: sources/ScreenChar/ScreenChar.h:629–635]`. **This is the only sub-cell positioning primitive OSC 1337 has**, and because it is undocumented it must be treated as unstable.

Parsing is forgiving in a way worth knowing: arguments are split on `;`, then on the *first* `=`; a token with no `=` becomes a key with an empty value `[SRC]`. Unknown keys are ignored silently, so feature-probing by sending an unknown key tells you nothing.

`doNotMoveCursor` is **not** an OSC 1337 key in iTerm2. It is a WezTerm extension (`wezterm.org/imgcat.html`). It does appear in the iTerm2 3.6.9 binary — as `executeDisplay(image): cursorMovementPolicy=doNotMoveCursor` `[BIN]` — but that string lives in `KittyImageController.swift` and is iTerm2's handling of the **kitty** protocol's `C=1`, not of `File=`. Do not send it to iTerm2 expecting an effect.

### 1.3 Multipart form (for tmux)

```
ESC ] 1337 ; MultipartFile = <same args> ST
ESC ] 1337 ; FilePart = <base64 chunk> ST      (repeated)
ESC ] 1337 ; FileEnd ST
```

Documented chunk ceilings: 256 bytes for older tmux, 1,048,576 bytes for newer tmux and iTerm2 `[SPEC]`. All three names are present in the 3.6.9 dispatch table `[BIN]`. A 256-byte ceiling means a 143 KB frame becomes **559 separate OSC sequences**; this path is for file transfer, not for animation.

### 1.4 Cell metrics — closing an A04 gap

`ESC ] 1337 ; ReportCellSize ST` replies `OSC 1337 ; ReportCellSize=<height>;<width> ST` on older builds and `…=<height>;<width>;<scale> ST` on newer ones, in **points**, with `scale` being the backing-store ratio (`1.0` non-Retina, `2.0` Retina) `[SPEC]`. This matters because A04 §5.3 measured that iTerm2's `TIOCGWINSZ` reports **device pixels** while `CSI 14 t` reports **points** — a factor-of-two disagreement on this Retina display. A04 flagged the reply format `[UNVERIFIED]`; the documented format above closes it, but the **live reply from 3.6.9 is still uncaptured** `[UNVERIFIED — the probe in §8 captures it]`.

Consequence for this backend: `width=Npx` is interpreted as **device pixels** — `executeFileCommandWithValue:` passes the raw number through and `getRequestedWidthInPoints:` computes `ceil(width / (cellSize.width * scaleFactor))` where `cellSize` is in points `[SRC]`. Sizing in **cells** (`width=<cols>`) avoids the entire points-versus-pixels trap and is what a renderer should do.

---

## 2. What actually happens inside the terminal — the crux

Everything in this section comes from `writeBaseCharacter:toGrid:width:height:decodedImage:` and `writeImage:toGrid:` in `sources/InlineImages/VT100InlineImageHelper.m:265–298, 532–560` `[SRC]`.

### 2.1 The image becomes grid cells

For an image sized `width` × `height` **cells**, iTerm2 allocates one image code and then, for each row `y` and each column `x` in `[cursorX, cursorX + width)`, writes a `screen_char_t` into the grid whose `image` bit is set and which carries the code plus that cell's `(x, y)` offset inside the image (`SetPositionInImageChar`). Columns past the right edge of the grid are simply dropped (`x < screenWidth`).

**This is the decisive fact for the mission's crux question.** An OSC 1337 image is not a floating overlay in a separate display list. It is ordinary screen content occupying ordinary cells. Anything that writes to those cells — another image, text, `ED`, `EL` — replaces it. There is no append-only display list, and therefore **no inherent requirement to scroll**.

### 2.2 Where the cursor ends up

Row advance is `[self.delegate inlineImageAppendLinefeed]`, executed once before each row after the first. After the loop, `grid.cursorX = grid.cursorX + width`. So for an image drawn with the cursor at row `R`, column `C`:

- final cursor row = `R + height - 1` — **the last row of the image, not the line below it**
- final cursor column = `C + width`

If `width == cols`, the cursor is left at the right edge in the pending-wrap state. The next printable character wraps, and if you are on the bottom row that wrap scrolls. **Always `CUP` immediately after a frame; never emit a printable byte or newline after one.**

### 2.3 The scroll hazard, precisely

```
for (int y = 0; y < height; y++) {
    if (y > 0) {
        if (_sixelData && self.sixelDisplayMode && [self.delegate inlineImageLinefeedWouldScroll]) break;
        [self.delegate inlineImageAppendLinefeed];
    }
    ...
}
```

Note the guard condition. `inlineImageLinefeedWouldScroll` — a predicate that exists precisely to prevent this — is consulted **only when the payload is Sixel data and Sixel display mode is on**. On the OSC 1337 base64 path both conjuncts are false, so the guard never fires and the linefeed is unconditional.

Therefore: **an image of `H` cells drawn with the cursor on 1-based row `R` scrolls the screen by `max(0, R + H - 1 - rows)` lines.** An image that exactly fills the grid from the home position (`R = 1`, `H = rows`) does *not* scroll, because the final linefeed moves from row `rows - 1` to row `rows` and a linefeed only scrolls when the cursor is *already* at the bottom margin.

This answers the mission's framing directly. It is **not** true that "every frame appends and scrolls" — that is the naive `imgcat` behaviour, where the image is emitted at the end of accumulated output and each one lands at the bottom. Under cursor control the surface is stable.

### 2.4 The in-place replacement recipe

Per frame, and nothing else:

```
ESC [ ? 25 l                     (hide cursor, once at startup)
ESC [ ? 2026 h                   (begin synchronized update — iTerm2 supports mode 2026)
ESC [ 1 ; 1 H                    (home)
ESC ] 1337 ; File = inline=1 ; width=<cols> ; height=<rows> ;
             preserveAspectRatio=0 ; size=<n> : <base64> ESC \
ESC [ ? 2026 l                   (end synchronized update)
```

Do **not** append a newline. Do **not** use `height=<rows+1>`. If you want a status line, use `height=<rows-1>` and own the last row yourself.

Two residual risks. First, on a grid **resize** the cells are re-flowed and the geometry assumption breaks for one frame; redraw on `SIGWINCH` before anything else. Second, `terminalWillReceiveInlineFileNamed:ofSize:` routes through `inlineImageConfirmBigDownloadWithBeforeSize:afterSize:name:` `[BIN]`, i.e. iTerm2 can raise a **modal confirmation** for a large inline file. Frames of 100–300 KB are very likely under any sane threshold, but the threshold itself is `[UNVERIFIED]` and a modal in the middle of a render loop would be fatal.

---

## 3. Partial and positioned updates

**Positioned: yes, at cell granularity.** `CUP` to the target cell, then emit an image sized to the damage rectangle in cells. The image lands at the cursor. There is no `C=`-style "don't move the cursor", so you re-home after every write — cheap, but it means the cursor is not usable for anything else.

**Sub-cell: only via the undocumented `inset*` keys** (§1.2), which shrink the image inside its cell box by a fraction of a cell on each edge. That is enough to align a damage rect that does not fall on cell boundaries, at the cost of depending on an undocumented key.

**Partial in the sense of a source rectangle: no.** There is no argument that selects a region of a previously transmitted image. Every update retransmits a complete, independently encoded image file.

**Deletion: no primitive.** There is no image id, no handle, no `a=d` equivalent. You remove an image by erasing or overwriting the cells it occupies.

**Compositing: no.** No z-index, no alpha-over-text, no ordering control. Later writes to a cell win.

This is a workable dirty-rectangle scheme and it is the *only* configuration in which OSC 1337 is not absurd — a 200×100 damage rect is ~4 KB of JPEG. It is also exactly the scheme A04 §8 already recommends for the kitty backend, where it composes with `a=T, C=1, X=, Y=, q=2` and does not need a per-rect image allocation.

---

## 4. Image lifetime, and why it is not the disaster it first looks like

Each `File=` allocates a fresh image code (`ImageCharForNewImage`), registers the decoded image in a process-wide `ImageRegistry` keyed by code, and sets an `iTermImageMark` on the line below the image `[SRC]`. `iTermImageMark.dealloc` calls `ReleaseImage(code)`, which removes the registry entry `[SRC: sources/Marks/iTermImageMark.m:62–67, sources/InlineImages/ImageRegistry.swift:86–95]`. The registry also caches `imageInfo.dictionary()` per code for state restoration — a second retained copy of the payload.

At 60 fps that would be a fatal leak, except that iTerm2 has a sweeper: `releaseOverwrittenImagesIn:` walks the interval tree above the top of the screen and, for each image mark, scans backwards up to the image's height in lines checking whether **any cell still carries that image code**; if none does, the mark is dropped and the image freed `[SRC: sources/VT100Screen/VT100ScreenState.m:312–400]`. Overwriting a full-viewport image in place therefore *does* reclaim the previous frame.

Two caveats that matter at frame rates:

The sweep is **O(resident marks × rows × cols)** per state sync, on iTerm2's main thread, competing with rendering. At 60 fps you are asking iTerm2 to run a full-grid cell scan sixty times a second purely to garbage-collect your own frames.

The sweep **bails out entirely if the grid size changed** — the comment is explicit that images then "stay in memory until they exit scrollback history" `[SRC]`. In the alternate screen there is no scrollback, so a resize storm leaks for the life of the session.

Separately, image codes are `unichar` — a **16-bit code space** (`ReleaseImage(unichar code)`, `GetImageInfo(unichar code)`) `[SRC: sources/ScreenChar/ScreenChar.h:643–650]`. At 60 fps that space is exhausted in about 18 minutes of continuous rendering; whether allocation reuses freed codes safely was not established `[UNVERIFIED]`.

None of this is a reason to choose a different backend on its own. It is listed because it is the class of problem you only find after shipping.

---

## 5. Throughput — the actual killer

The transport is not what kills OSC 1337. The **encoder** is.

OSC 1337 carries a container format, so there is no equivalent of kitty's `f=32` raw-RGBA path and no equivalent of `t=s` shared memory. Every frame must be fully encoded by us and fully decoded by iTerm2.

### 5.1 Measured encode cost on this machine `[MEAS]`

Harness: `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/c03/enc_bench.py` and `enc_bench2.py`, Python 3.14.2, Pillow 11.3.0, median of 7. Frame size 2482×814 — the measured Ghostty viewport from the mission brief, and a fair stand-in for a full-screen terminal on this display.

**Near-blank page** — the real captured `apps/engine/spike/out/example-com.png`, upscaled. This is the *optimistic* end and it flatters PNG badly:

| Codec | median ms | bytes | base64 bytes | MB/s @30 | MB/s @60 |
|---|---:|---:|---:|---:|---:|
| PNG `compress_level=1` | 7.02 | 107,455 | 143,276 | 4.3 | 8.6 |
| PNG `compress_level=6` | 9.66 | 49,509 | 66,012 | 2.0 | 4.0 |
| JPEG q=75 | 2.24 | 42,839 | 57,120 | 1.7 | 3.4 |

**Content-rich page** — a photo-like frame, which is what any page with an image, video, map or gradient looks like. This is the honest case:

| Codec | median ms | bytes | base64 bytes | MB/s @30 | MB/s @60 |
|---|---:|---:|---:|---:|---:|
| PNG `compress_level=1` | **73.80** | 3,817,685 | 5,090,248 | **152.7** | **305.4** |
| PNG `compress_level=6` | **270.10** | 2,247,476 | 2,996,636 | 89.9 | 179.8 |
| JPEG q=75 | 6.38 | 195,916 | 261,224 | 7.8 | 15.7 |
| JPEG q=85 | 6.72 | 259,787 | 346,384 | 10.4 | 20.8 |

Read the PNG row against the budget. The engine delivers frames at a measured p50 gap of **16.65 ms**. A single PNG encode of one content-rich frame takes **73.8 ms** — a **13 fps ceiling from encode alone**, before base64, before the write, before iTerm2 decodes anything. At `compress_level=6` it is **3.7 fps**. PNG is not a candidate.

JPEG at 6.4 ms is the only survivable option, and it costs the thing a browser surface can least afford: chroma subsampling turns coloured text into mush, and there is no alpha. `q=90, subsampling=0` measured 3.74 ms / 96,572 base64 bytes on the near-blank frame — better, but on content-rich frames that configuration was not measured `[UNVERIFIED]`.

Pillow's PNG encoder is zlib-based, as most are. A hand-tuned encoder (fpng, libspng) would improve on it — plausibly 2–4× `[UNVERIFIED — not benchmarked]` — which still leaves PNG at roughly 20–35 ms per content-rich frame, i.e. still over budget at 30 fps.

### 5.2 The receiving side

For every frame iTerm2 must base64-decode, run a full image decode to a ~8 MB bitmap, allocate an `NSImage` and a registry entry, write `cols × rows` cells into the grid, insert an interval-tree mark, and run the overwritten-image sweep of §4 — all on its main thread. This was not measured `[UNVERIFIED — see §8]`, but it is strictly additive to the encode cost above.

### 5.3 Verdict

**30 fps: no, for any content that is not a near-blank page.** JPEG at reduced resolution could reach 30 fps on paper (7.8 MB/s), which is within what a pty on this machine can carry, but only by giving up text fidelity and alpha, and only if iTerm2's decode fits in the remaining budget.

**60 fps: no, under any configuration.**

Compare: the kitty backend on the same terminal, using `t=s`, sends roughly **70 bytes per frame** regardless of resolution (A04 §8) and does **zero** encoding.

---

## 6. iTerm2 3.6.9 speaks the full kitty protocol — this is the decisive fact

The installed 3.6.9 binary contains a complete kitty graphics implementation `[BIN]`:

| Kitty capability | Evidence from the 3.6.9 binary |
|---|---|
| Controller, renderer, command parser | `KittyImageController.swift`, `KittyImageRenderer.swift`, `KittyImageCommand.swift` (paths embedded as `/Users/gnachman/git/iterm2-alt6/sources/…`) |
| Placements with ids and parents | `executeDisplay(image): appending placement - imageID=`, `parentPlacement=`, `ERROR - adding would form cycle` |
| Delete commands | `executeDeleteImage: case 'a/A' - deleting all placements`, `case '' - deleting ALL images and placements` |
| **Shared-memory transfer (`t=s`)** | `executeTransmitSharedMemory: shm name=`, `read <n> bytes from shared memory` |
| **Temp-file transfer (`t=t`)** | `executeTransmitTemporaryFile(_:payload:query:)` |
| `C=1` do-not-move-cursor | `executeDisplay(image): cursorMovementPolicy=doNotMoveCursor` |
| Z-index and source/dest rects | `drawKittyImagesInRange: zIndexRange=[…]`, `destFrame=… sourceFrame=…` |
| Unicode placeholders | `Decoded kitty unicode placeholder: imageID=… placementID=… row=… col=…` |

Corroborated by iTerm2's release notes: file and shared-memory transfer for the kitty image protocol landed in **3.6.1**, and a streaming-decode bug was fixed in 3.5.12. A04 independently measured 3.6.9 replying `ESC _ G i=31 ; OK ESC \` to the kitty support query `[EMPIRICAL, A04 §1]`.

So on iTerm2, our existing kitty backend gets: raw pixels with no encode, ~70-byte frames over `t=s`, real placements, real deletes, sub-cell offsets, z-ordering, and an explicit "do not move the cursor". OSC 1337 offers none of that and costs 74 ms per frame.

`caps.rs:56–66` orders the ladder `Kitty → Iterm2 → Sixel → Unicode`, and `caps.rs:143–146` sets `kitty_graphics` from the protocol query. On iTerm2 the kitty branch therefore wins and `Backend::Iterm2` is **never selected**. It is already dead code on all three measured terminals.

---

## 7. Recommendation

**Do not implement an OSC 1337 rendering backend. Remove `Backend::Iterm2` from the ladder.**

The reasoning in one line: on the only terminal that speaks OSC 1337, a strictly better protocol is already available and already implemented, so the backend can never be selected; and if it were selected it could not hold 30 fps on real content.

I own no core source, so per Hard Rule 1 the change is described rather than made. In `crates/tf-term/src/caps.rs`:

- `Capabilities::best_backend` (lines 56–66): drop the `Iterm2` arm so the ladder reads `Kitty → Unicode`. Drop the `Sixel` arm too, on A04 §3's finding that Sixel buys nothing on any of the three targets.
- `Capabilities::iterm2_images` (line 37) and its heuristic assignment (line 191): keep the **field** as reporting-only for `doctor`, since knowing you are on iTerm2 is still useful, but stop letting it select a backend. The existing comment at lines 187–190 already says this capability is "largely moot"; this makes the code agree with the comment.
- `Backend::Iterm2` in `crates/tf-term/src/lib.rs`: retain the variant only if `doctor` prints it; otherwise remove it so nobody implements against it later.

**Fallback ladder, final form:**

| Terminal | Backend | Basis |
|---|---|---|
| Ghostty 1.3.1 | Kitty | `[EMPIRICAL]`, end-to-end verified in the mission brief |
| iTerm2 3.6.9 | Kitty (`t=s` preferred, `t=d` over SSH) | `[BIN]` + A04 `[EMPIRICAL]` |
| Apple Terminal 465 | Unicode half-block | `[EMPIRICAL]`, A04 |
| Anything else | Kitty if the query answers, else Unicode | detection is a query, never a `$TERM` match |

If a terminal is later found that speaks OSC 1337 but **not** kitty, the correct response is a **single-shot screenshot command** (`terminal-fenster shot <url>` printing one frame and exiting), not a render loop. That is ~40 lines against §1's grammar and it is genuinely useful for CI and for pasting a page into a terminal session. It should never be wired into the frame path. Which terminals are actually in that OSC-1337-only population was not established `[UNVERIFIED]`.

---

## 8. What I could not measure, and the harness left behind

I attempted a live runtime probe of iTerm2 3.6.9 and it **failed**. Reporting it plainly per Hard Rule 5.

AppleScript automation of iTerm2 does work on this machine — `osascript -e 'tell application "iTerm" to get version'` returns `3.6.9`, and a trivial `create window with default profile command` successfully ran a shell command whose environment confirmed `TERM_PROGRAM=iTerm.app`, `TERM_PROGRAM_VERSION=3.6.9`. So A04's note that the iTerm2 automation path works is correct, and the mission brief's "blocked by macOS TCC" is not the obstacle.

The obstacle is that **the machine is at a lock screen and iTerm2 does not drain the pty of a window that is never rendered.** The probe wrote its first full-viewport frame and blocked. Sampled after five minutes:

```
$ /usr/bin/sample 31009 1
798 Thread_…  DispatchQueue_1: com.apple.main-thread
  …
  798 os_write  (in Python) + 124
    798 _Py_write_impl  (in Python) + 144
      798 write  (in libsystem_kernel.dylib) + 8
```

798 of 798 samples in `write(2)` on the tty. The process was killed and the automation cleaned up.

Two honest readings. The narrow one: this measures the lock screen, not OSC 1337, and proves nothing about iTerm2's throughput. The broader one, which stands regardless: **the write path back-pressures hard, and Terminal-Fenster must never issue a frame write from the same thread that drives the engine or handles input.** A blocked terminal must degrade to dropped frames, not to a hung browser. That is a scheduler requirement (B07's territory) and it holds for the kitty backend too.

The probe is complete and correct as far as it goes, and is worth running on an unlocked machine — it would close A04's `ReportCellSize` gap and measure the §5.2 receive-side cost. It is at:

```
/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/c03/c03_iterm_probe.py
```

Run it with the machine **unlocked and iTerm2 frontmost**:

```sh
SP=/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/c03
osascript -e 'tell application "iTerm"
  create window with default profile command "/bin/sh -c '"'"'/Library/Frameworks/Python.framework/Versions/3.14/bin/python3 '"$SP"'/c03_iterm_probe.py '"$SP"'/out.json'"'"'"
end tell'
```

It reports the `ReportCellSize` reply, the cursor position after an image drawn at home, the cursor position after a deliberately overflowing image, and sixty in-place full-viewport frames with p50/p99 write latency and iTerm2's RSS before and after — which is the direct test of §4's garbage-collection cost. **Housekeeping:** the automation attempts left extra iTerm2 windows open (5 at last count); they are harmless but a human may want to close them.

---

## 9. Reproduction

```sh
# Version of the binary these claims are about
defaults read ~/Applications/iTerm.app/Contents/Info.plist CFBundleShortVersionString   # 3.6.9

# The OSC 1337 argument keys this build actually parses
strings -a ~/Applications/iTerm.app/Contents/MacOS/iTerm2 | grep -x -E \
  'preserveAspectRatio|inline|insetTop|insetLeft|insetBottom|insetRight|wide'

# Proof that 3.6.9 implements the kitty protocol with shm and temp-file transfer
strings -a ~/Applications/iTerm.app/Contents/MacOS/iTerm2 | grep -E \
  'executeTransmitSharedMemory|executeTransmitTemporaryFile|executeDeleteImage|cursorMovementPolicy'

# Licence check before any reuse discussion
gh api repos/gnachman/iTerm2 --jq '.license.spdx_id'                                    # GPL-2.0

# The two source files that answer the crux
gh api repos/gnachman/iTerm2/contents/sources/InlineImages/VT100InlineImageHelper.m \
   -H "Accept: application/vnd.github.raw" | sed -n '532,560p'    # writeBaseCharacter
gh api repos/gnachman/iTerm2/contents/sources/VT100/VT100Terminal.m \
   -H "Accept: application/vnd.github.raw" | sed -n '3904,4005p'  # executeFileCommandWithValue

# Encode-cost measurements
python3 /private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/c03/enc_bench.py
python3 /private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/c03/enc_bench2.py
```

**Primary sources**

- iTerm2 inline images — <https://iterm2.com/documentation-images.html>
- iTerm2 proprietary escape codes — <https://iterm2.com/documentation-escape-codes.html>
- iTerm2 source, GPL-2.0 — <https://github.com/gnachman/iTerm2>
- WezTerm's `doNotMoveCursor` extension — <https://wezterm.org/imgcat.html>
- kitty graphics protocol — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- Prior art in this repo: `$REPO/artifacts/swarm/A04-terminal-capability-matrix.md` §4, §5.3, §8
