# C05 — Capability detector hardening

**Scope:** audit of `crates/bg-term/src/caps.rs` (373 lines) for failure modes under slow
terminals, partial responders, concurrent user input, multiplexers, and SSH.

**Verdict:** the detector's *philosophy* is right — ask the terminal, treat silence as a
negative, keep raw replies for `doctor`. The *mechanism* has two defects that silently
produce wrong geometry on real, currently-shipping terminals, plus a startup cost of up to
1.8 s that is entirely avoidable. Nine further issues are ranked below.

The single highest-impact finding is **F1**: on iTerm2 3.6.9 the page renders at roughly
half resolution and about half the window becomes unclickable. This is not a hypothetical
— it falls out of two measured numbers that disagree by 2×.

I own only this file. Every fix below is a sketch for the commander; I have not touched
`crates/`, `apps/cli/`, or `apps/engine/`.

---

## 1. Evidence base

Four independent sources. Everything numeric below traces to one of them.

**(a) Two purpose-built harnesses.** Because the agent shell has no controlling terminal, I
drove the detector against scripted PTYs. Both are single-file `rustc` builds with no
crates (the volume is at 99%, 5.4 GiB free — `df -h`, so no cargo target dir was created).

| File | What it does |
|---|---|
| `scratchpad/detector_harness.rs` → `baseline.log` | `read_reply`, `query`, `ends_with_any`, `parse_two_param_t`, `parse_da1_has_sixel`, `parse_decrqm_supported` copied **verbatim** from `caps.rs:92-251`. Only change: `libc::` → local `extern "C"`, and `query` takes an explicit out-fd. |
| `scratchpad/fix_harness.rs` → `fix.log` | The proposed `sieve` + pipelined `handshake`, measured on the same PTY scripts. |
| `scratchpad/geom.rs` → `geom.log` | Geometry-source selection, replayed against three real terminal captures. |

**(b) A real multiplexer.** `tmux` is not installed on this machine; `/usr/bin/screen` is.
I ran the probe inside a **detached** screen session (`screen -dmS`, no client attached):

```
isatty(0)=true   TERM="screen"   STY="44826.bgtest2"
rtt=0ms  da1_seen=true   raw_reply=\e[?1;2c
kitty_gfx=false sixel=false kitty_kbd=false pixel_mouse=false
window_px=None cell_px=None
TIOCGWINSZ rc=0 rows=24 cols=80 xpixel=0 ypixel=0
```

**(c) Prior-mission terminal captures** already present in the shared scratchpad
(`out-iterm2.json`, `out-appleterm.json`, `term-out.txt`, `iterm-out.txt`, `wsz-term.txt`).
Provenance: another agent, timestamps 21:00–21:11. I treat these as measurements, not
assertions, because each records the exact bytes sent and received.

**(d) Primary specs.** kitty graphics protocol
(<https://sw.kovidgoyal.net/kitty/graphics-protocol/>), xterm `ctlseqs` XTWINOPS
(<https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>), tmux FAQ
(<https://github.com/tmux/tmux/wiki/FAQ>).

### 1.1 Correction to the briefed terminal matrix

The mission brief and the `caps.rs:12-17` header table both state iTerm2 is **UNVERIFIED**
because TCC blocks automation. That is now stale: `out-iterm2.json` and `iterm-out.txt`
contain a complete, successful iTerm2 3.6.9 capture (marker file `marker-iterm2.txt` shows
`rc=0`). The corrected matrix, with the numbers that matter for this audit:

| | Ghostty 1.3.1 | Apple Terminal 465 | iTerm2 3.6.9 | GNU screen |
|---|---|---|---|---|
| kitty graphics | yes | **no** (`\e[?1;2c` only) | **yes** (`\e_Gi=31;OK\e\`) | no |
| DA1 | `?62;22;52c` | `?1;2c` | `?64;1;2;4;6;17;18;21;22;52c` → **sixel** | `?1;2c` |
| kitty keyboard | yes | no reply | **yes** (`\e[?0u`; push/pop works) | no |
| DECRQM ?1016 | supported | no reply | `;4$y` = **permanently reset** | no reply |
| `CSI 14t` | `4;851;2482t` | `4;467;860t` | `4;462;570t` | no reply |
| `CSI 16t` | `6;37;17t` | **no reply** | **no reply** | no reply |
| `TIOCGWINSZ` px | 2488×858 | 840×450 | **1120×850** | **0×0** |
| grid | 146×23 | 120×30 | 80×25 | 80×24 |

Two things jump out. iTerm2 supports kitty graphics *and* advertises sixel, so it is a
first-class target, not a fallback case. And iTerm2's `CSI 14t` (570×462) disagrees with
its own `TIOCGWINSZ` (1120×850) by a factor of ~1.96 — Retina points versus device pixels.
The `caps.rs:188-190` comment asserting iTerm2 supports kitty graphics is now backed by
`out-iterm2.json`; it should cite it.

---

## 2. Findings

### F1 — CRITICAL: viewport and cell size are taken from different coordinate spaces

`viewport_px()` (`caps.rs:73-88`) unconditionally prefers `CSI 14t` over `TIOCGWINSZ`. Cell
size (`caps.rs:176-180`) prefers `CSI 16t`, falling back to `TIOCGWINSZ / grid`. On a
terminal that answers `CSI 14t` but **not** `CSI 16t`, those two paths draw from different
sources — and on iTerm2 those sources are in different units.

Neither value is individually wrong. The *pair* is incoherent:

```
iTerm2 3.6.9, measured (out-iterm2.json)
  viewport  <- CSI 14t     =  570 x 462   (Retina points)
  cell      <- ioctl/grid  =  1120/80 x 850/25 = 14 x 34   (device pixels)
  implied grid = 570/14 x 462/34 = 40 x 13   ... but the real grid is 80 x 25
```

Consequences, following `apps/cli/src/main.rs:252-254` and `PointerMap::to_page`
(`main.rs:712-727`), given `pixel_mode=false` on iTerm2 (DECRQM 1016 returns
*permanently reset*, correctly detected):

- Page is rendered at `570 × (462-34) = 570×428` into a window whose real pixel area is
  `1120×850`. **Roughly quarter-area, half-linear resolution.**
- `to_page` computes `py = row*34 + 17` and discards any point with `py >= page_h = 428`.
  That kills every row from 13 upward: **12 of 25 rows (48% of window height) are dead to
  the mouse.**
- `px = col*14 + 7` is clamped by `px.min(page_w-1) = 569`. Every column from 42 upward
  collapses onto the single rightmost page pixel: **39 of 80 columns (49% of width) are
  unaddressable.**

The browser is usable only in the top-left quadrant, and nothing logs an error.

Ghostty escapes this because it answers `CSI 16t`, and Apple Terminal escapes it because
its two sources agree to within padding. iTerm2 is the case that breaks, and it is one of
the most widely used macOS terminals.

**Fix — pick the pixel source that lives in the same space as the cells.** The discriminator
is objective and needs no per-terminal special-casing: the correct source divides the
character grid *evenly*. Verified against all three captures:

```
Ghostty:        2482 = 146*17 exact,  851 = 23*37 exact   -> CSI 14t wins
Apple Terminal:  840 = 120*7  exact,  450 = 30*15 exact   -> ioctl wins (860x467 has padding)
iTerm2:         1120 =  80*14 exact,  850 = 25*34 exact   -> ioctl wins (570x462 is points)
```

```rust
/// Residual distance from `px` being an exact multiple of the grid. Lower is better;
/// None when the implied cell is not a plausible character cell.
fn residual(px: (u16, u16), grid: (u16, u16)) -> Option<(u32, (u16, u16))> {
    let ((w, h), (cols, rows)) = (px, grid);
    if w == 0 || h == 0 || cols == 0 || rows == 0 { return None; }
    let (cw, ch) = (w / cols, h / rows);
    if !(3..=64).contains(&cw) || !(5..=128).contains(&ch) { return None; }
    Some(((w % cols) as u32 + (h % rows) as u32, (cw, ch)))
}

pub fn choose_geometry(
    queried_cell: Option<(u16, u16)>,  // CSI 16 t
    window_px:    Option<(u16, u16)>,  // CSI 14 t
    ioctl_px:     (u16, u16),          // TIOCGWINSZ
    grid:         (u16, u16),          // (cols, rows)
) -> Option<Geom> {
    let mut cands: Vec<(u32, Geom)> = Vec::new();
    if let Some((cw, ch)) = queried_cell {
        // Cell size is known exactly: the right viewport is the one nearest cell*grid.
        let want = (cw as u32 * grid.0 as u32, ch as u32 * grid.1 as u32);
        for (px, src) in [(window_px, "CSI 14t+16t"), (Some(ioctl_px), "ioctl+16t")] {
            if let Some((w, h)) = px.filter(|p| p.0 > 0 && p.1 > 0) {
                let err = (w as i64 - want.0 as i64).unsigned_abs() as u32
                        + (h as i64 - want.1 as i64).unsigned_abs() as u32;
                cands.push((err, Geom { viewport: (w as u32, h as u32), cell: (cw, ch), source: src }));
            }
        }
        if cands.is_empty() {
            return Some(Geom { viewport: want, cell: (cw, ch), source: "CSI 16t * grid" });
        }
    } else {
        for (px, src) in [(window_px, "CSI 14t/grid"), (Some(ioctl_px), "ioctl/grid")] {
            if let Some(p) = px {
                if let Some((r, cell)) = residual(p, grid) {
                    cands.push((r, Geom { viewport: (p.0 as u32, p.1 as u32), cell, source: src }));
                }
            }
        }
    }
    cands.sort_by_key(|(e, _)| *e);
    cands.first().map(|(_, g)| *g)
}
```

Measured result (`geom.log`) — the `source` string is worth surfacing in `doctor`:

```
Ghostty        CURRENT viewport=(2482,851) cell=(17,37)  FIXED same, via CSI 14t+16t
Apple Terminal CURRENT viewport=(860,467)  cell=(7,15)   FIXED (840,450) via ioctl/grid
iTerm2         CURRENT viewport=(570,462)  cell=(14,34)  FIXED (1120,850) via ioctl/grid
                                                          -> implies 80x25 cells vs real 80x25
GNU screen     CURRENT viewport=(640,384)  cell=None     FIXED GEOMETRY UNKNOWN
```

### F2 — CRITICAL: a late `CSI 14t` reply is parsed as the cell size

`parse_two_param_t` (`caps.rs:221-230`) accepts any `CSI <a>;<b>;<c> t` and returns
`(b, c)`, **discarding the leading parameter**. Per xterm `ctlseqs`, that leading parameter
is the only thing distinguishing the replies:

| Query | Reply |
|---|---|
| `CSI 14 t` (window px) | `CSI **4** ; height ; width t` |
| `CSI 16 t` (cell px) | `CSI **6** ; height ; width t` |
| `CSI 18 t` (text area cells) | `CSI **8** ; height ; width t` |

Because each probe gets its own independent deadline, a terminal that is merely slow — a
congested SSH link, a compositor stall — can have its `CSI 14t` answer arrive after the
14t deadline expires, landing in the buffer of the *next* probe, which is `CSI 16t`.

Reproduced end to end (`baseline.log`, E3; terminal answers 14t at 380 ms against a 300 ms
deadline):

```
window_px probe reply = ""
cell_px   probe reply = "\e[4;851;2482t"
=> window_px = None   cell = Some((2482, 851))
=> CLI would compute page_h = 851 - 851 = 1px  (cell_w=2482)
```

A **1-pixel-tall page**, from one late reply. And `parse_two_param_t` also happily accepts
an `18t` reply as pixels (`\e[8;23;146t` → `Some((23,146))`), which matters because both
Apple Terminal and iTerm2 do answer `CSI 18t`.

Note this is not purely a slow-link problem: it is latent in *any* ordering assumption
that probes are independent. F4's pipelining makes interleaving the normal case, so F2
must be fixed together with F4, not after it.

**Fix:** match on the leading parameter, and never accept an unlabelled triple.

```rust
enum WinOp { WindowPx { w: u16, h: u16 }, CellPx { w: u16, h: u16 }, TextChars { w: u16, h: u16 } }

fn parse_winop(params: &[u32]) -> Option<WinOp> {
    let [kind, h, w] = params else { return None };
    let (w, h) = (u16::try_from(*w).ok()?, u16::try_from(*h).ok()?);
    match kind {
        4 => Some(WinOp::WindowPx { w, h }),
        6 => Some(WinOp::CellPx  { w, h }),
        8 => Some(WinOp::TextChars { w, h }),
        _ => None,
    }
}
```

### F3 — HIGH: the reply reader eats user keystrokes, and single characters desync the chain

This is the race the mission asked about, and it has three distinct edges.

**(i) Keystrokes are silently destroyed.** `read_reply` (`caps.rs:92-114`) reads whatever
is available into `buf`; `detect` parses `buf` and drops it. Any byte the user typed during
detection is gone. Typeahead is the common case: the user runs `blackglass open …` and
keeps typing, or a shell wrapper leaves bytes queued before we enter raw mode. Measured
(`baseline.log`, E2 — user types `you` 40 ms in):

```
kitty_graphics    300ms  reply=you
user bytes typed: 3  -> delivered to input::Decoder: 0  (all discarded)
```

**(ii) A single ordinary character terminates a probe early.** The completion predicates are
`ends_with_any(b, b"c" | b"u" | b"t" | b"y")` (`caps.rs:157,163,168,175,183`). Each fires on
one keystroke:

```
ends_with_any("c") = true   ends_with_any("t") = true
ends_with_any("u") = true   ends_with_any("y") = true
```

`c`, `t`, `u` and `y` are common English letters; typing "you" or "cut" during startup is
entirely ordinary. When a probe exits early on a stray letter, the terminal's real reply
arrives afterwards and lands in the *next* probe's buffer — the same misattribution as F2,
now triggered by typing rather than latency.

**(iii) The parsers accept forged input.** `parse_da1_has_sixel` (`caps.rs:213-218`) splits
on the literal `"[?"` with **no ESC required**. Measured:

```
parse_da1_has_sixel(user typed "[?4;c")   = true    <- forged sixel claim, no ESC involved
parse_da1_has_sixel("hello" + real DA1)   = true
parse_decrqm_supported(user typed "\e[?1;1$y") = true
```

A user typing `[?4;c` makes BlackGlass believe the terminal does sixel and select the sixel
backend on a terminal that cannot render it. Low-probability, but it is a correctness hole
reachable from ordinary input, and the mitigation is free.

**Fix — never discard bytes; demultiplex them.** Replace "read until the last byte looks
right" with a real ECMA-48 tokenizer that extracts *identified* replies and returns
everything else as pending user input, to be replayed into `input::Decoder`. A truncated
tail is left buffered rather than guessed at.

```rust
/// Length of the complete escape sequence at b[0] (which must be ESC), or None if truncated.
fn seq_len(b: &[u8]) -> Option<usize> {
    if b.len() < 2 { return None; }
    match b[1] {
        b'[' => { // CSI: params 0x30-0x3F, intermediates 0x20-0x2F, final 0x40-0x7E
            let mut i = 2;
            while i < b.len() && (0x30..=0x3f).contains(&b[i]) { i += 1; }
            while i < b.len() && (0x20..=0x2f).contains(&b[i]) { i += 1; }
            (i < b.len() && (0x40..=0x7e).contains(&b[i])).then_some(i + 1)
        }
        b'P' | b'X' | b']' | b'^' | b'_' => { // string seqs, terminated by ST or BEL
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 { return Some(i + 1); }
                if b[i] == 0x1b {
                    return (i + 1 < b.len() && b[i + 1] == b'\\').then_some(i + 2);
                }
                i += 1;
            }
            None
        }
        b'O' => (b.len() >= 3).then_some(3), // SS3 -- a user key
        _    => Some(2),                     // ESC <char> = Alt+key -- a user key
    }
}

/// Returns None when the sequence is user input rather than a reply we asked for.
fn classify(s: &[u8], probe_id: u32) -> Option<Reply> {
    if s.len() >= 2 && s[1] == b'[' {
        // ... split private-intro / params / intermediates / final ...
        return match (priv_intro, intermediates, final_byte) {
            (Some(b'?'), [],     b'c') => Some(Reply::Da1 { params }),
            // CSI ? <flags> u is the kitty keyboard *reply*; a kitty key EVENT is
            // CSI <n>;<m> u with no private intro. The '?' is what separates them.
            (Some(b'?'), [],     b'u') => Some(Reply::KittyKeyboard { flags: params[0] }),
            (Some(b'?'), [b'$'], b'y') => Some(Reply::Decrpm { mode: params[0], value: params[1] }),
            (None,       [],     b't') => parse_winop(&params).map(Reply::from), // F2
            _ => None, // arrows, SGR mouse, function keys, kitty key events -> user input
        };
    }
    if s.starts_with(b"\x1b_G") {
        // Require OUR probe id and an exact "OK" status (see F9).
        let (ctrl, status) = split_once(&s[3..s.len()-2], b';')?;
        let id = kv(ctrl, "i=")?;
        return (id == probe_id).then(|| Reply::KittyGraphics { ok: status.trim() == "OK" });
    }
    None
}

pub struct Sieved { pub replies: Vec<Reply>, pub user: Vec<u8>, pub consumed: usize }

pub fn sieve(buf: &[u8], probe_id: u32) -> Sieved {
    let mut out = Sieved::default();
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b {
            let start = i;
            while i < buf.len() && buf[i] != 0x1b { i += 1; }
            out.user.extend_from_slice(&buf[start..i]);
            out.consumed = i;
            continue;
        }
        let Some(n) = seq_len(&buf[i..]) else { break }; // truncated: keep the tail, don't guess
        match classify(&buf[i..i + n], probe_id) {
            Some(r) => out.replies.push(r),
            None    => out.user.extend_from_slice(&buf[i..i + n]), // user's Escape/arrows
        }
        i += n;
        out.consumed = i;
    }
    out
}
```

The pending bytes must then be handed to the input decoder before the first real read.
`input::Decoder::decode` already accepts an arbitrary byte slice, so this is a one-line
handoff in the CLI:

```rust
let probe = caps::detect(guard.fd(), 300);
let mut decoder = input::Decoder::new(probe.sgr_pixel_mouse);
for ev in decoder.decode(&probe.pending_user_input) { /* replay typeahead */ }
```

Measured with the fix (`fix.log`), keystrokes injected mid-handshake:

```
user typed "you"                        -> recovered "you"
user typed Up-arrow then 'c' (\e[Ac)    -> recovered "\e[Ac"
user typed bracketed paste \e[200~hello\e[201~ -> recovered intact
sieve("[?4;c")            -> replies=[]  user="[?4;c"        (forgery rejected)
sieve("\e[27;5u")         -> replies=[]  user="\e[27;5u"     (kitty key event, not a reply)
sieve("\e[?62;22")        -> replies=[]  consumed=0          (truncated tail kept)
sieve("\e_Gi=99;OK\e\\")  -> UnsolicitedSeq                  (foreign image id ignored)
```

### F4 — HIGH: six sequential deadlines cost up to 1.8 s, and six round trips over SSH

`detect` issues six probes, each with its own full `deadline_ms`. Every unanswered probe
costs the whole deadline. Measured against a DA1-only terminal (the Apple Terminal shape)
at the CLI's `300` (`main.rs:118,241`):

```
kitty_graphics 300ms   da1 0ms   kitty_keyboard 299ms
window_px      300ms   cell_px 300ms   sgr_pixel_mouse 299ms
TOTAL DETECT WALL TIME: 1498ms
```

For context, the project's own measured engine startup is 212 ms to ready and 366 ms to
first frame. **Detection is four times the cost of starting Chromium**, spent entirely on
`poll` timeouts. On a partially-answering terminal over SSH it is worse: the probes are
serialised, so the cost is *six* round trips, not one. At a 150 ms transcontinental RTT
that is ~0.9 s of pure latency even when every probe answers.

**Fix — pipeline every query behind a DA1 sentinel.** This is exactly what the kitty
graphics spec prescribes: send the query, follow it with a primary device attributes
request, and if DA1 comes back without the query's answer, the feature is absent. The spec
states terminals supporting the protocol *must* reply to query actions immediately without
processing other input, which is what makes DA1 a sound sentinel. It converts "no reply"
from a timeout guess into a **positive negative result**, and collapses six deadlines into
one round trip.

This is not speculative for this codebase: the project's own capability-matrix mission
already used it. `out-iterm2.json` records
`sent: b'\x1b_Gi=31,...,a=q,t=d,f=24;AAAA\x1b\\\x1b[c'` — query plus DA1 in one write — and
it worked on iTerm2, Apple Terminal and (per the brief) Ghostty.

```rust
fn build_queries() -> Vec<u8> {
    let mut q = Vec::new();
    q.extend_from_slice(&kitty::support_query(PROBE_ID)); // graphics
    q.extend_from_slice(b"\x1b[?u");        // kitty keyboard
    q.extend_from_slice(b"\x1b[14t");       // window px
    q.extend_from_slice(b"\x1b[16t");       // cell px
    q.extend_from_slice(b"\x1b[?1016$p");   // DECRQM SGR-pixels
    q.extend_from_slice(b"\x1b[c");         // DA1 -- MUST BE LAST: it is the sentinel
    q
}

pub fn handshake(in_fd: RawFd, out_fd: RawFd, deadline: Duration) -> Probe {
    let mut p = Probe::default();
    write_all_eintr(out_fd, &build_queries())?;      // see F6, F5
    let start = Instant::now();
    let mut buf = Vec::new();
    while start.elapsed() < deadline {
        match poll_read(in_fd, deadline - start.elapsed(), &mut buf) {
            PollRead::Interrupted => continue,       // F5: EINTR is not a negative result
            PollRead::Timeout | PollRead::Eof => break,
            PollRead::Data => {}
        }
        let s = sieve(&buf, PROBE_ID);
        if s.replies.iter().any(|r| matches!(r, Reply::Da1 { .. })) {
            p.rtt = start.elapsed();                 // free, honest link-latency measurement
            apply(&mut p, s);
            return p;
        }
    }
    p.rtt = start.elapsed();
    apply(&mut p, sieve(&buf, PROBE_ID));            // deadline hit: use whatever arrived
    p
}
```

Measured (`fix.log`), same PTY scripts as the baseline:

| Scenario | Current | Pipelined |
|---|---|---|
| Ghostty, quiet | — | **0 ms** |
| Ghostty, user typing mid-probe | keystrokes lost | **60 ms**, keystrokes recovered |
| Apple Terminal (DA1 only) | **1498 ms** | **0 ms** |
| Apple Terminal over 120 ms-RTT SSH | ~1.5 s + 6 RTT | **121 ms** (one RTT) |

The `rtt` field is a useful by-product: it is a *direct measurement* of link latency, far
better than the `SSH_CONNECTION` proxy (see F8), and it can drive compression level and
frame pacing.

### F5 — MEDIUM: a signal during a probe is read as "terminal does not support it"

`read_reply` treats `poll() <= 0` and `read() <= 0` as terminal conditions
(`caps.rs:101-107`). `poll` returning `-1` with `EINTR` is not a timeout — it means a signal
arrived — but the code `break`s and reports an empty reply.

Today this is *latent*: `TtyGuard::acquire` installs handlers only for SIGINT/TERM/HUP/QUIT
(which re-raise and die), and SIGWINCH's default disposition is ignore, so nothing currently
interrupts the poll. It goes live the moment anyone installs a SIGWINCH handler — which is
precisely what F11 (re-detect on resize) requires. Demonstrated by installing one
(`baseline.log`, E4):

```
SIGWINCH delivered: true
probe returned after 52ms with reply=""
=> DA1 LOST even though the terminal answered at ~100ms
```

Losing DA1 means losing the sentinel in the F4 design, so this must be fixed *before*
F11 lands. Fix is the standard retry, shown in the `handshake` sketch above: on `EINTR`,
`continue` rather than `break`, recomputing the remaining deadline each pass. `n == 0` is
the only genuine timeout. With the fix in place the same scenario yields
`SIGWINCH delivered=true  da1_seen=true  kitty_gfx=true`.

### F6 — MEDIUM: queries are written to `stdout`, not to the terminal

`query` writes to `io::stdout()` (`caps.rs:117`) but reads from `fd`. When stdout is
redirected — `blackglass doctor > report.txt`, a CI log, a tee — the escape sequences go
into the file and never reach the terminal, while detection still waits the full deadline on
each probe for replies that cannot arrive. `cmd_doctor` checks `isatty(stdin)`
(`main.rs:138`) but never checks stdout, so this path is reachable in exactly the situation
where a user is capturing diagnostics. Measured (`baseline.log`, E5):

```
probes answered: 0/6
wall time wasted: 1800ms
bytes that went to the redirect instead of the terminal: 61
```

The output is a `doctor` report claiming the terminal supports nothing, plus 61 bytes of
escape garbage in the user's log file. Fix: write to the same fd that is being read (or open
`/dev/tty` explicitly), and propagate write errors instead of `let _ =` — a failed write
currently still costs a full deadline of waiting.

### F7 — MEDIUM: unknown geometry is fabricated rather than reported

`viewport_px()` ends with `Some((cols * 8, rows * 16))` (`caps.rs:84-86`). This converts
"I could not determine the size" into a confident, wrong answer, and because it returns
`Some`, the CLI's guard at `main.rs:244-250` ("could not determine terminal pixel size")
can never fire. `c.cell.unwrap_or((8, 16))` at `main.rs:252` does the same for cell size.

The measured screen session hits exactly this: `CSI 14t`/`16t` unanswered *and*
`TIOCGWINSZ` pixels `0×0`, so all three sources fail and BlackGlass would render an
`80*8 × 24*16 = 640×384` page bearing no relation to the real window, with every pointer
coordinate wrong.

Fix: let `choose_geometry` (F1) return `None` and make the CLI fail with an actionable
message. An honest "I cannot determine your terminal's pixel geometry; try
`BLACKGLASS_CELL=WxH`" beats a silently mis-scaled browser. If a fabricated default is kept
for the Unicode backend (where cell-exactness matters less), it must be flagged in
`Capabilities` — e.g. `geometry_confidence: Measured | Derived | Assumed` — and printed by
`doctor`, so the failure is visible rather than inferred.

### F8 — MEDIUM: multiplexers answer on their own behalf; env-derived signals go stale

**Measured, not assumed.** The detached screen session answered DA1 in 0 ms with
`\e[?1;2c` **while no client was attached** — there was no outer terminal to consult. This
is the structural point for both screen and tmux: a multiplexer is a terminal emulator, so
every capability reply describes *it*, not the terminal the user is looking at. Under a
multiplexer, `c.sixel` is the multiplexer's sixel support, and a sixel-capable outer
terminal will be reported as incapable.

The good news is that DA1 still arrives, so the F4 sentinel remains sound inside a
multiplexer — detection terminates fast rather than hanging.

For tmux specifically, per the tmux FAQ: tmux does not forward escape sequences it does not
understand, and passthrough requires wrapping the sequence in a DCS form prefixed with
`tmux;` with every `\033` **doubled**, terminated by ST — and as of tmux 3.3 the
`allow-passthrough` option must be enabled. So the current code's raw APC graphics probe is
filtered by tmux and `kitty_graphics` comes back false even on Ghostty. That is a
fail-closed false negative: correct, but it costs the user the entire product.

`mux_label` (`main.rs:189-197`) already tells the user about `allow-passthrough`, which is
the right instinct. Two concrete improvements:

```rust
/// tmux swallows sequences it does not understand. Wrap ours, doubling every ESC.
fn tmux_wrap(inner: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in inner {
        if b == 0x1b { out.push(0x1b); } // ESC must be doubled
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
}
```

and gate it on the option actually being on, rather than hoping:

```rust
fn tmux_passthrough_enabled() -> bool {
    std::process::Command::new("tmux")
        .args(["show", "-gv", "allow-passthrough"])
        .output().ok()
        .map(|o| matches!(String::from_utf8_lossy(&o.stdout).trim(), "on" | "all"))
        .unwrap_or(false)
}
```

**UNVERIFIED, and the one thing I could not test:** whether tmux forwards the outer
terminal's *reply* back to the pane. Wrapping only guarantees the query goes out. tmux is
not installed here, so I could not measure the return path, and I decline to guess. This is
the highest-value remaining measurement in this area — see §4.

A related trap worth designing against: **environment variables go stale across
reattach.** `TERM_PROGRAM`, `TERM` and `COLORTERM` are captured when a pane is created. If a
user starts a tmux/screen session from Apple Terminal, detaches, and reattaches from
Ghostty, those variables still describe Apple Terminal while the real terminal is Ghostty.
`iterm2_images` (`caps.rs:191`) is derived purely from `TERM_PROGRAM` and is therefore
wrong in exactly that scenario. The query-based approach is immune; the env-derived
heuristic is not. Note the captures show a better identifier exists: `XTVERSION`
(`CSI > 0 q`) returned `\eP>|iTerm2 3.6.9\e\` on iTerm2 — a live, authoritative
name-and-version that survives SSH and cannot go stale. Apple Terminal does not answer it,
which is itself a useful signal. Worth adding to the pipelined batch, since it costs
nothing once queries are pipelined.

### F9 — LOW/MEDIUM: the kitty graphics probe accepts a substring and ignores the image id

`c.kitty_graphics = find(&reply, b"OK").is_some() && find(&reply, b"_G").is_some()`
(`caps.rs:146`). Two weaknesses. The status is matched as a *substring* anywhere in the
reply rather than parsed as the status field, and the image id we sent (`31`) is never
checked, so a stale APC response left by another program in the same terminal — or any
error text happening to contain `OK` — reads as support.

The captures show kitty error payloads carry arbitrary text: `out-iterm2.json` has
`\e_Gi=33;EBADF:The operation couldn't be completed. No such file or directory\e\`,
including a UTF-8 right single quote. Text under someone else's control should not be
substring-matched for a capability decision. The `classify` sketch in F3 parses
`i=<id>;<status>`, requires `id == PROBE_ID`, and requires `status.trim() == "OK"`.

### F10 — LOW: probing happens on the user's primary screen

`detect` runs at `main.rs:241`; `enable_input_protocols`, which sends `\x1b[?1049h` to enter
the alternate screen, runs at `main.rs:263`. So all probe traffic — including the APC
graphics payload — is emitted onto the user's real scrollback. Terminals that do not
understand APC generally swallow it, but that is not guaranteed, and a terminal that echoes
it prints garbage over the user's prompt. `detect` already cleans up its probe image
(`caps.rs:148-154`), which shows the concern is understood; extending it to enter the
alternate screen before probing and leave it after (`doctor` already restores before
printing) closes the remaining gap.

### F11 — LOW: no re-detection on resize

Detection is one-shot. `WinSize`, `window_px` and `cell` all change when the user resizes
the window or drags it between displays with different DPI — the latter changes the
device-pixel ratio, which per F1 changes the relationship between `CSI 14t` and
`TIOCGWINSZ`. No SIGWINCH handler is installed, so the resize is invisible and the geometry
silently goes stale. The fix is a debounced re-query of `CSI 14t`/`16t` plus `TIOCGWINSZ` on
SIGWINCH, feeding `choose_geometry` again — but it depends on F5 being fixed first, since
installing the handler is what makes EINTR live.

### F12 — LOW: documentation drift

The `caps.rs:12-17` matrix lists only Ghostty and Apple Terminal and predates the iTerm2
capture. Given iTerm2 supports kitty graphics *and* advertises sixel, it belongs in the
table. The `caps.rs:188-190` claim that iTerm2 supports kitty graphics is correct and now
has a citation (`out-iterm2.json`, `KITTY_GFX_QUERY_RGB` → `\e_Gi=31;OK\e\`); the comment
should point at it. Adding per-probe elapsed-ms to `raw_replies` would also let `doctor`
show *how slow* a terminal was, not just what it said — directly useful for the
slow-terminal failure mode and cheap to assert on in CI.

---

## 3. Suggested test vectors

All of these are pure-function tests needing no terminal, and all are drawn from bytes that
were actually observed.

```rust
// F2 -- leading parameter must be honoured
assert_eq!(parse_winop(&[4, 851, 2482]), Some(WinOp::WindowPx { w: 2482, h: 851 }));
assert_eq!(parse_winop(&[6,  37,   17]), Some(WinOp::CellPx   { w: 17,   h: 37  }));
assert_eq!(parse_winop(&[8,  30,  120]), Some(WinOp::TextChars{ w: 120,  h: 30  })); // Apple Terminal
assert_eq!(parse_winop(&[4, 462,  570]).map(is_cell), Some(false)); // iTerm2 14t is NOT a cell size

// F1 -- geometry selection, from the three real captures
assert_eq!(choose_geometry(Some((17,37)), Some((2482,851)), (2488,858), (146,23)).unwrap().cell, (17,37));
assert_eq!(choose_geometry(None, Some((860,467)), (840,450), (120,30)).unwrap().viewport, (840,450));
assert_eq!(choose_geometry(None, Some((570,462)), (1120,850), (80,25)).unwrap().viewport, (1120,850));
assert_eq!(choose_geometry(None, None, (0,0), (80,24)), None); // measured GNU screen

// F3 -- the sieve preserves input and rejects forgeries
assert_eq!(sieve(b"hi\x1b[Athere\x1b[?62;22;52c", 31).user, b"hi\x1b[Athere");
assert!(sieve(b"[?4;c", 31).replies.is_empty());              // no ESC -> not a reply
assert!(sieve(b"\x1b[27;5u", 31).replies.is_empty());         // kitty key event, not a reply
assert_eq!(sieve(b"\x1b[?62;22", 31).consumed, 0);            // truncated tail is kept
assert!(matches!(sieve(b"\x1b_Gi=99;OK\x1b\\", 31).replies[0], Reply::UnsolicitedSeq(_)));

// F9 -- real iTerm2 error payload must not read as support
let err = b"\x1b_Gi=33;EBADF:The operation couldn\xe2\x80\x99t be completed.\x1b\\";
assert!(!matches!(classify(err, 33), Some(Reply::KittyGraphics { ok: true })));
```

The PTY harnesses in `scratchpad/` are CI-able as-is: they need no display, no Chromium, and
no real terminal, which matters given the lock-screen constraint. Folding
`detector_harness.rs`'s scripted-terminal idea into `tests/` would give regression coverage
for E1–E5 (timeout stacking, keystroke race, late-reply misattribution, EINTR, stdout
redirect) that no unit test on the parsers alone can provide.

---

## 4. What I could not verify

- **tmux end-to-end.** Not installed; only `/usr/bin/screen` is available. The screen result
  is real and the structural conclusion carries, but the specific question of whether tmux
  forwards the outer terminal's *reply* back into the pane is **UNVERIFIED**. Installing
  tmux and re-running `fixharness --real-tty` inside it, attached to Ghostty, would settle
  it in one command and is the cheapest high-value measurement outstanding.
- **Ghostty live re-measurement.** The machine is at a lock screen, so I relied on the
  brief's Ghostty numbers and the `caps.rs` header table rather than re-measuring. The
  iTerm2 and Apple Terminal numbers are from prior-mission JSON captures that record exact
  sent/received bytes.
- **Whether iTerm2's `CSI 14t` is truly points rather than device pixels.** The 1.96× ratio
  against `TIOCGWINSZ` and the exact-division test both point that way and the fix does not
  depend on the interpretation being right — `choose_geometry` picks the self-consistent
  source either way — but I did not confirm the cause with an iTerm2 source or a
  display-scale change.
- **The workspace test suite.** I did not run `cargo test`; at 99% disk I avoided creating
  build artifacts. No claim here depends on it.

---

## 5. Recommended order

F5 and F2 first — they are small, self-contained, and F4 depends on both (pipelining makes
interleaved replies routine, and EINTR-safety protects the sentinel). Then F3 and F4
together, since they share the `sieve`. Then F1, which is the highest user-visible impact
but is a pure function over values the earlier work already produces correctly. F6 is a
two-line fix worth taking at any point. F7 follows F1 naturally. F8, F10–F12 are hardening
and documentation.
