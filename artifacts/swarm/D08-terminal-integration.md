# D08 — tmux, Multiplexers, and Terminal Splits

**Mission:** specify integration with tmux and terminal splits *without silently rewriting user
config*. Cover `allow-passthrough` detection, pane resize handling, and per-terminal adapters.

**Status:** every claim below marked *Measured* was produced on this machine against a real
`tmux 3.7b` on macOS 26.1 (Apple M4). Claims I could not test are marked **UNVERIFIED** and are
not relied upon.

---

## 0. Environment change I made — please review

`tmux` was **not installed** on this machine. The mission is a tmux specification, and writing one
from memory would have been guesswork, so I installed it:

```
brew install tmux          # HOMEBREW_NO_AUTO_UPDATE=1
→ tmux 3.7b + libevent 2.1.13, ncurses 6.6, utf8proc 2.11.3
→ 14.5 MB total, 10.6 s wall
```

Disk was at 99 % (6.4 GiB free) before and after; 14.5 MB is 0.2 % of remaining headroom, well
inside the "no multi-GB builds, no large binaries" rule. **It is still an unrequested change to the
commander's machine.** `brew uninstall tmux libevent ncurses utf8proc` reverses it. My
recommendation is to keep it: it is the only way to run the CI matrix in §11, and it is a build-time
test dependency, not a runtime one.

All probe scripts live in the session scratchpad
(`.../scratchpad/{ptyprobe,replyprobe2,control,attached,roundtrip,sizeprobe,final,final2,frameprobe}.py`)
and every result below is reproducible from them. I left no tmux servers or sockets running.

### 0.1 Headline results

| # | Finding | Impact |
|---|---------|--------|
| 1 | `allow-passthrough` is a **window** option, default **`off`**, added in tmux **3.3**; the `all` state added in **3.4** | Every tmux user hits this on first run |
| 2 | With it off, graphics sequences are **silently swallowed** — zero bytes, no error | Blank pane, no diagnostic |
| 3 | `tmux show-options` answers this question **wrongly in three different ways** | The obvious detector nags correctly-configured users |
| 4 | `wrap_tmux` (`crates/tf-term/src/kitty.rs:241`) is **byte-for-byte correct**, now verified end-to-end | Closes C01's open item D |
| 5 | APC replies **do** round-trip through tmux; CSI replies **do not** | Kitty detection works inside tmux; DA1-style detection cannot |
| 6 | tmux answers DA1 as `ESC[?1;2;4c` — **param 4 = sixel** | `parse_da1_has_sixel` is a **false positive** inside tmux |
| 7 | `TIOCGWINSZ` inside a pane is **pixel-exact**; `CSI 16t` is confidently **wrong** when the outer terminal reports no pixels | Inverts caps.rs's preference order inside tmux |
| 8 | Window switch delivers **no signal at all** — no SIGWINCH, no focus event, no output | Image loss on window switch is undetectable without polling |

---

## 1. What `allow-passthrough` actually is

tmux parses everything a pane writes. Sequences it does not understand — including the Kitty
graphics APC and sixel DCS — are dropped, because tmux maintains its own screen model and cannot
represent them. The escape hatch is a DCS wrapper:

```
ESC P tmux; <payload, every ESC doubled> ESC \
```

tmux unwraps it, halves the doubled ESCs, and writes the result straight to the outer terminal —
**but only if `allow-passthrough` permits it.**

*Measured*, from the `CHANGES` file shipped with tmux 3.7b (`/opt/homebrew/opt/tmux/CHANGES`):

| Line | Text | Section | Version |
|------|------|---------|---------|
| 819 | "Add an option (default off) to control the passthrough escape sequence." | `CHANGES FROM 3.2a TO 3.3` | **3.3** |
| 770 | "Add a third state \"all\" to allow-passthrough to work even in invisible panes." | `CHANGES FROM 3.3a TO 3.4` | **3.4** |

So:

| tmux version | `allow-passthrough` | Consequence for Terminal-Fenster |
|--------------|--------------------|-----------------------------|
| < 3.3 | does not exist | Graphics impossible. Unicode fallback only. |
| 3.3 | `off` \| `on`, default `off` | Works once enabled; `on` drops output from invisible panes |
| ≥ 3.4 | `off` \| `on` \| `all`, default `off` | Adds `all` (forwards even when invisible) |

### 1.1 The three values, measured

Probe `final.py` §E: our pane was pushed into a background window
(`#{window_active}` = 0), then emitted a wrapped Kitty query.

| Value | Query reached the outer terminal? |
|-------|-----------------------------------|
| `off` | **No** |
| `on` | **No** (pane not visible) |
| `all` | **Yes** |

`on` gates on visibility; `all` does not. §7 explains why we still recommend `on`.

---

## 2. Failure mode: silence

Probe `ptyprobe.py` owned the outer pty, so bytes read on the master fd are exactly what a real
terminal emulator would have received.

Pane emitted `wrap_tmux(kitty_support_query)`:

```
ESC Ptmux; ESC ESC _Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA ESC ESC \ ESC \
```

| `allow-passthrough` | Outer-pty bytes | Bare Kitty APC present | Wrapper leaked | Doubled ESC left |
|---------------------|-----------------|------------------------|----------------|------------------|
| `off` | 620 | **No — swallowed** | No | No |
| `on` | 724 | **Yes** | No | No |
| `all` | 724 | **Yes** | No | No |

With `on`, the sequence emerged correctly unwrapped and ESC-undoubled:

```
ESC _Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA ESC \
```

Two conclusions:

1. **`wrap_tmux` at `crates/tf-term/src/kitty.rs:241` is correct.** C01 §"tmux passthrough
   end-to-end" flagged it as uncalled and therefore unverified. It is now verified against real
   tmux. The function is still uncalled — wiring it up is the work, not fixing it.
2. **With passthrough off there is no error signal whatsoever.** tmux does not warn, does not log,
   does not reply. The pane simply stays blank. This is precisely why detection has to be explicit:
   we cannot discover the problem by watching for failure, because failure looks identical to
   success-with-a-black-page.

---

## 3. Detection — and why the obvious approach is wrong

### 3.1 Three ways `show-options` lies

Probe `mx` (§"Resolve option inheritance semantics"). All *Measured*.

**Lie 1 — `-gv` misses window overrides.** With the window option set to `on`:

```
tmux show-options -wv allow-passthrough   → on     (correct)
tmux show-options -gv allow-passthrough   → off    (WRONG — false negative)
```

**Lie 2 — untargeted `-wv` resolves against the wrong window.** Three windows set to `on`, `off`,
`all`; current window was #2:

```
tmux show-options -wv allow-passthrough        → all   (whatever the CURRENT window is)
tmux show-options -wv -t %0 allow-passthrough  → on    (ours)
```

Terminal-Fenster may be running in a pane whose window is not the active one. Untargeted queries answer a
question about someone else's window.

**Lie 3 — the fatal one — `show-options` does not resolve inheritance.** This is the worst because
it fires against users who have *already configured tmux correctly*:

| State | `show -wv` | `show -wv -t %0` | `display-message -p -t %0` |
|-------|-----------|------------------|----------------------------|
| pristine default | `` (empty) | `` (empty) | **`off`** |
| `set -g allow-passthrough on` | `` (empty) | `` (empty) | **`on`** |
| `set -g on` + window override `off` | — | — | **`off`** (override wins) |
| sibling window, same global | — | — | **`on`** (inherits) |

A user with `set -g allow-passthrough on` in `~/.tmux.conf` — the correct, recommended
configuration — gets an **empty string** from every `show-options` form. A detector that treats
"not `on`" as "off" would tell that user to fix something already fixed, every single launch.

### 3.2 The one correct query

```sh
tmux display-message -p -t "$TMUX_PANE" '#{allow-passthrough}'
```

`display-message` expands the format in the target pane's context and walks the full inheritance
chain (window-local → global-window → built-in default). *Measured* correct in all four states
above.

Three-way result, all with `rc=0`:

| Output | Meaning | Action |
|--------|---------|--------|
| `on` / `all` | Passthrough permitted | Proceed to protocol probe |
| `off` | Explicitly or by default disabled | Print §4.1, fall back to Unicode |
| `` (empty) | Format unknown ⇒ tmux < 3.3 | Print §4.2, fall back to Unicode |

The empty case is *Measured*: `display-message -p '#{nonexistent_option}'` returns empty with
`rc=0`, so an unknown format is indistinguishable from a missing option — which is exactly the
tmux < 3.3 situation we want to catch.

### 3.3 Config query is necessary but not sufficient

Probe `roundtrip.py` ran the complete production sequence: pane (raw mode) emits the wrapped query,
the harness acts as the outer terminal and answers `ESC_Gi=31;OK ESC\`, and we check the pane's
stdin.

| `allow-passthrough` | Query reached terminal | Terminal answered | Reply on pane stdin | Verdict |
|---------------------|------------------------|-------------------|---------------------|---------|
| `off` | No | No | `''` | **indistinguishable from a terminal with no Kitty support** |
| `on` | Yes | Yes | `ESC_Gi=31;OK ESC\` | **Kitty graphics confirmed** |

The protocol probe is ground truth for *does it work*. It cannot tell us *why not*. The config query
cannot prove it works — tmux may permit passthrough to a terminal that has no graphics support at
all. **We need both**, and they answer different questions:

- protocol probe → **capability** (what we act on)
- config query → **explanation** (what we print)

### 3.4 Detection algorithm

```
in_tmux ← $TMUX is set                       (already in caps.rs:134)

if not in_tmux:
    existing caps::detect() path, unchanged
    return

pane ← $TMUX_PANE                            (Measured present: e.g. "%0")
if pane is empty or the tmux binary is not on PATH:
    → TmuxState::Unknown          # $TMUX set but we cannot ask; see §3.5
else:
    v ← `tmux display-message -p -t $pane '#{allow-passthrough}'`   # ~21 ms, once
    match v:
        "on" | "all" → TmuxState::Enabled(v)
        "off"        → TmuxState::Disabled
        ""           → TmuxState::Unsupported     # tmux < 3.3
        _            → TmuxState::Unknown

# Protocol probe is attempted whenever passthrough is not known-disabled.
# Every graphics query is wrapped with wrap_tmux() when in_tmux.
if state is Disabled or Unsupported:
    print §4.1 / §4.2 to stderr, before raw mode
    backend ← Unicode
else:
    backend ← result of the wrapped protocol probe (may still be Unicode)
    if state is Enabled and probe found nothing:
        print §4.3    # passthrough is fine; the terminal genuinely lacks graphics
```

### 3.5 `TmuxState::Unknown`

`$TMUX` is set but we could not resolve the option — the tmux client binary is absent from `PATH`
(common in restricted images and in `sudo` environments), or the socket is unreachable.

Do **not** guess. Attempt the wrapped probe anyway: it is harmless if passthrough is off (the bytes
are swallowed) and authoritative if it is on. If the probe returns nothing, print §4.4, which names
both possible causes rather than asserting one.

---

## 4. The exact messages

All messages go to **stderr, before `TtyGuard::acquire`** — i.e. before raw mode and before
`ESC[?1049h`. Printing after entering the alternate screen would put the text on a buffer that is
discarded on exit, and printing in raw mode with `OPOST` off renders without line breaks (the same
trap `tty.rs:124` already documents for the panic hook).

Wrapped to 79 columns. No colour when `NO_COLOR` is set or stderr is not a tty.

### 4.1 Passthrough is `off` — the message the mission asks for

```
terminal-fenster: tmux is discarding the graphics escape sequences, so the page
            cannot be rendered.

  cause   The tmux window option `allow-passthrough` is off. This is tmux's
          default. With it off, tmux drops the sequences that carry page
          pixels before your terminal ever sees them, silently and with no
          error of its own.

  fix     For this tmux window, effective immediately:

              tmux set -w allow-passthrough on

          For every window, permanently, add to ~/.tmux.conf:

              set -g allow-passthrough on

          then reload it with:

              tmux source-file ~/.tmux.conf

  note    Terminal-Fenster does not modify your tmux configuration.

  meanwhile
          Falling back to the Unicode half-block renderer. It works through
          tmux unmodified, at roughly one quarter of the vertical detail.
          Run `terminal-fenster doctor` to re-check after changing the setting.
```

Design notes, each load-bearing:

- **Names the option and the value.** The user can search for `allow-passthrough` and find the tmux
  manual. A message that says only "graphics unavailable" sends them nowhere.
- **Says tmux's default is off.** Pre-empts "why is my terminal broken" — nothing is broken.
- **Explains the silence.** The pane looked blank with no error; saying so out loud stops the user
  hunting for a log that does not exist.
- **Two fixes, ordered by commitment.** `tmux set -w …` is *Measured* to take effect immediately on
  the current window, so the user can retry within seconds. The `~/.tmux.conf` line is *Measured*
  to apply to both existing and new windows (§3.1 table).
- **`set -w` in the transient fix, `set -g` in the permanent one.** Not interchangeable: `-w` sets
  the window-local override; `-g` sets the global-window default that other windows inherit.
- **States that we will not edit their config.** This is the mission's explicit requirement and it
  is worth saying, because the natural next question is "why doesn't it just fix itself".
- **Says what happens now.** We degrade, we do not abort. The Unicode backend is already the
  measured floor (`caps.rs:335`, `unicode_is_the_floor_not_an_error`).
- **Does not name the outer terminal.** We *cannot* know it: §8.1 shows tmux overwrites
  `TERM_PROGRAM` to `tmux` and `TERM` to `tmux-256color`, and with passthrough off we cannot ask the
  terminal either. Writing "your terminal" is honest; guessing "Ghostty" from a stale env var is not.
- **No emoji, no colour-dependence, no box drawing.** It must survive `2>&1 | tee`, CI logs, and a
  7-bit ssh session.

### 4.2 tmux older than 3.3

```
terminal-fenster: this tmux is too old to pass graphics through.

  cause   tmux <version> has no `allow-passthrough` option; it was added in
          tmux 3.3. Without it there is no supported way to send image data
          from a pane to the terminal underneath.

  fix     Upgrade tmux to 3.3 or newer, or run Terminal-Fenster outside tmux.

  meanwhile
          Falling back to the Unicode half-block renderer.
```

`<version>` comes from `tmux -V`, which *Measured* prints `tmux 3.7b` — note the trailing letter,
so the parser must handle `<major>.<minor><suffix>` and must not choke on a missing suffix.

### 4.3 Passthrough is on but the terminal has no graphics

```
terminal-fenster: tmux passthrough is enabled, but the terminal underneath did not
            answer the Kitty graphics query.

  This is not a tmux problem. The outer terminal appears not to support the
  Kitty graphics protocol. Using the Unicode half-block renderer.

  `terminal-fenster doctor` outside tmux will confirm what the terminal supports.
```

Worth separating from §4.1: it stops the user editing tmux config that is already correct. The
"run doctor outside tmux" advice matters because outside tmux we *can* identify the terminal.

### 4.4 `TmuxState::Unknown`

```
terminal-fenster: $TMUX is set but the tmux client could not be queried, so the
            state of `allow-passthrough` is unknown.

  The graphics probe found no terminal response. That means either passthrough
  is off, or the terminal has no Kitty graphics support. We cannot tell which
  from inside this pane.

  If graphics are expected, check:  tmux show -w allow-passthrough

  Using the Unicode half-block renderer.
```

This message deliberately refuses to pick a cause. Both are live possibilities and §3.3 measured
that they are observationally identical.

### 4.5 Suppression

The message prints **once per process**, not per frame or per tab. `TERMINAL_FENSTER_QUIET_TMUX=1`
suppresses §4.1–§4.4 for users who have decided to live with the Unicode backend. `doctor` always
prints them regardless, since diagnosis is its entire job.

---

## 5. Frame path through tmux

### 5.1 Wrap the whole batch, not each chunk

A real frame is many Kitty escape sequences: `encode_rgb_frame` (`kitty.rs:100`) chunks base64
payload at `MAX_CHUNK = 4096`. A 200×200 incompressible test frame produced **40 chunks,
160,468 bytes**. Probe `frameprobe.py` pushed that through real tmux both ways:

| Strategy | Wire bytes | Wrapper overhead | Bytes at outer pty | APCs intact | Wrapper leaked | Doubled ESC left |
|----------|-----------|------------------|--------------------|-------------|----------------|------------------|
| One DCS around all 40 | 160,557 | **+89** | 160,852 | **40/40** | 0 | 0 |
| One DCS per chunk | 160,908 | +440 | 163,543 | 40/40 | 0 | 0 |

Both are correct. **Wrap the whole batch**: 89 bytes instead of 440, one wrapper instead of 40, and
fewer bytes on the outer pty. A single DCS carrying 160 KB survived tmux 3.7b intact.

**UNVERIFIED:** whether tmux 3.3–3.6 accept a DCS this large, and whether any intermediate ssh/pty
layer imposes a lower bound. The per-chunk strategy is the safe fallback and is measured to work, so
keep it behind a flag (`TERMINAL_FENSTER_TMUX_WRAP=per-chunk`) rather than deleting the code path.

### 5.2 Escape doubling cost is negligible

Base64 output contains no `0x1b`, so only the framing escapes double. Per chunk: `ESC_G` opener and
`ESC\` terminator = 2 ESC = 2 extra bytes. On the measured real frame from the mission brief
(53,999 wire bytes, ~14 chunks) that is ~28 bytes, **0.05 %**. Passthrough wrapping is not a
bandwidth concern, which matters for the C09 SSH budget.

### 5.3 Ordering constraint

The DCS wrapper is not re-entrant: a wrapped payload must be written to the tty as one contiguous
run. If the compositor interleaves a cursor move or a status-bar update between chunks of a wrapped
frame, tmux sees a malformed DCS. The frame writer must hold the tty for the duration of one wrapped
batch. This is a constraint on the C06 compositor, not on the encoder.

---

## 6. Pane resize handling

### 6.1 tmux propagates pixel geometry exactly

This is the happiest finding in the document. Probe `sizeprobe.py` set the outer pty to the real
Ghostty numbers (146×23 cells, 2482×851 px ⇒ 17×37 cell) and read `TIOCGWINSZ` from inside the pane:

```
initial:      rows=22 cols=146 xpixel=2482 ypixel=814
SIGWINCH #1:  rows=22 cols=73  xpixel=1241 ypixel=814     (after split-window -h)
SIGWINCH #2:  rows=22 cols=60  xpixel=1020 ypixel=814     (after resize-pane -x 60)
```

Every number is exact: 146 × 17 = 2482, 73 × 17 = 1241, 60 × 17 = 1020, 22 × 37 = 814. tmux derives
the cell size from its own pty and reports each pane's pixel size as `cols × cell_w` by
`rows × cell_h`. The one row lost is the status bar (23 → 22).

**Cross-check:** the mission brief records a real end-to-end Ghostty run producing a frame of
**2482×814**. That is exactly `146 cols × 17` by `22 rows × 37`. The harness geometry and the real
measured frame agree to the pixel, which is good evidence the harness models reality.

**SIGWINCH is delivered on every pane resize, with correct pixel geometry already in place.** The
existing `window_size()` (`tty.rs:205`) needs no change; it needs a SIGWINCH handler to call it.

### 6.2 Resize policy

1. **SIGWINCH → coalesce → re-query → resize the page.** Dragging a tmux pane divider emits a burst.
   Debounce ~80 ms after the last SIGWINCH before telling the engine to resize; a Chromium
   `setSize` per event would thrash the OSR pipeline.
2. **Re-read `TIOCGWINSZ`, not `CSI 14t`, when in tmux.** See §6.3.
3. **Full repaint after resize, never damage.** Kitty image geometry is bound to the placement;
   after a resize the previous placement is invalid. Delete the image (`delete_image(PAGE_IMAGE_ID)`,
   `kitty.rs:211`) and transmit a fresh full frame.
4. **Clamp to the pane, not the window.** After a vertical split the pane is ~half the columns; the
   ioctl already reports the pane's own size, so simply using it is correct.
5. **Zero-size panes.** tmux permits panes small enough that a browser viewport is meaningless.
   Below a floor (proposal: 20 cols × 5 rows) suspend frame production and show a one-line
   "pane too small" notice rather than shipping degenerate frames.

### 6.3 Inside tmux, trust the ioctl over `CSI 14t`/`16t` — the reverse of the current rule

`caps.rs:73` documents, correctly, that outside tmux `CSI 14t` beats `TIOCGWINSZ` because Apple
Terminal's ioctl excludes padding. **Inside tmux that preference inverts.** Probe `final.py` §D and
`final2.py` §F:

| Outer terminal reports pixels? | ioctl inside pane | `CSI 16t` inside pane | `CSI 14t` inside pane | Truth |
|-------------------------------|-------------------|----------------------|----------------------|-------|
| **Yes** (2482×851, cell 17×37) | `2482×814`, cell 17×37 | `ESC[6;37;17t` → 17×37 | `ESC[4;814;2482t` | ✅ both correct |
| **No** (xpixel=0, ypixel=0) | `2336×704`, cell **16×32** | `ESC[6;32;16t` → **16×32** | `ESC[4;704;2336t` | ❌ both fabricated |

When the outer terminal reports no pixel geometry — the Apple Terminal case, per the mission's
measured matrix — **tmux invents a 16×32 cell and reports the invention with total confidence
through both channels.** 2336 = 146 × 16 and 704 = 22 × 32.

This is strictly worse than the non-tmux case. Outside tmux, Apple Terminal simply does not answer
`CSI 16t` (*Measured*: no reply), so `caps.rs` knows it does not know and can degrade deliberately.
Inside tmux, the same terminal produces a plausible, precise, wrong answer. **tmux converts "unknown"
into "confidently wrong."**

**Mitigation (heuristic, and must be labelled as such):** if `in_tmux` and the derived cell is
exactly `(16, 32)`, treat it as *unverified* rather than *known*, and take the same conservative path
used when cell size is unavailable. 16×32 is a plausible real cell size on some displays, so this
will occasionally mis-flag a correct reading — the cost is a conservative layout, which is much
cheaper than mouse coordinates that are silently off by 6 % horizontally and 16 % vertically.

Report the flag in `doctor` output so the user can see which value is in play.

---

## 7. Visibility: the invisible-pane problem

### 7.1 tmux tells us nothing

If Terminal-Fenster is in window 0 and the user presses `prefix 1`, our frames stop reaching the terminal
(with `allow-passthrough on`, §1.1). Worse, tmux **cannot restore the image** when the user returns:
tmux redraws a pane from its own screen model, and passthrough bytes were never in that model — they
went straight to the terminal. The image is simply gone.

So we must know when we become visible again. *Measured*, we get no help:

| Candidate signal | Result on window switch |
|------------------|-------------------------|
| SIGWINCH | **Not delivered** (probe `bg_sw`: only the initial size line, zero SIGWINCH across 3 switches) |
| Focus in/out (`CSI I` / `CSI O`, mode 1004 enabled) | **Not delivered** (probe `final2.py` §G: 0 bytes across 3 switches) |
| Pane output | Dropped by definition |

*(Caveat: the probe's fake outer terminal never generated real focus events, so whether tmux
**forwards** genuine terminal focus events is **UNVERIFIED**. What is verified is the thing we
actually need: a tmux **window switch** produces no focus event.)*

### 7.2 Why not just recommend `all`

`all` forwards passthrough from invisible panes (*Measured*, §1.1) and looks like a one-line fix.
It is a trap: with `all`, a background Terminal-Fenster paints its frames onto the terminal **while the
user is looking at a different tmux window**. Graphics land over unrelated content. `all` exists for
programs that emit one image and stop, not for something repainting at 60 fps.

**Recommend `on`. Never recommend `all`.** Then make ourselves correct under `on`.

### 7.3 Policy

Polling is the only mechanism available, and it is not cheap. *Measured*, warm, 40 iterations:

| Command | Cost per call |
|---------|---------------|
| `tmux display-message -p -t %0 '#{window_active}'` | **20.76 ms** |
| `tmux show-options -wv -t %0 allow-passthrough` | **14.70 ms** |

That is fork + exec + socket round-trip. At 4 Hz a visibility poll costs ~8 % of a core — far too
much to pay continuously for a condition that is rare.

**Event-driven first, poll as a backstop:**

1. **Repaint on any input.** Key, mouse, or paste arriving after ≥1 s of silence triggers a full
   repaint instead of a damage update. Returning to a tmux window is followed almost immediately by
   an interaction, so the common case is fixed at the cost of one full frame.
2. **Repaint on SIGWINCH.** Already required by §6.2 and free.
3. **Low-frequency backstop poll at 1 Hz** (~2 % of a core), only when `in_tmux`. Detect a
   `false → true` transition in `#{window_active}` and force a full repaint. Covers "user switched
   back and is just reading".
4. **Never poll when not in tmux.** Zero cost outside a multiplexer.
5. Make it tunable: `TERMINAL_FENSTER_TMUX_POLL_HZ` (`0` disables, default `1`).

**UNVERIFIED alternative worth investigating:** tmux ≥ 3.2 supports format subscriptions via
`refresh-client -B` in control mode, which would replace polling with a push notification and
eliminate the per-poll fork. It requires a persistent control-mode client, which is a larger design
change. If §7.3's poll shows up in profiling, this is the escape hatch.

---

## 8. What else tmux breaks in capability detection

### 8.1 The outer terminal's identity is destroyed

*Measured* from inside a pane (probe `replyprobe2.py` §Q3):

| Variable | Outside tmux (this machine) | Inside tmux |
|----------|------------------------------|-------------|
| `TERM` | `xterm-ghostty` | `tmux-256color` |
| `TERM_PROGRAM` | `ghostty` / `Apple_Terminal` | **`tmux`** |
| `COLORTERM` | `truecolor` | `truecolor` (survives) |
| `TMUX` | unset | `/private/tmp/tmux-501/<sock>,<pid>,0` |
| `TMUX_PANE` | unset | `%0` |

`caps.rs:191` sets `iterm2_images = term_program == "iTerm.app"`. Inside tmux `TERM_PROGRAM` is
`tmux`, so **the iTerm2 heuristic always evaluates false inside tmux**, on iTerm2 included. The
existing comment already calls this heuristic a last resort; inside tmux it is simply dead. Not
urgent — iTerm2 3.6.9 was measured to support Kitty graphics, which the detector prefers anyway —
but `doctor` should say "outer terminal unidentifiable (inside tmux)" rather than implying it looked
and found nothing.

`caps.rs`'s module docstring already says "`$TERM` … survives SSH hops unchanged, tmux rewrites it".
That claim is now *Measured* true.

### 8.2 tmux answers protocol queries itself

Probe `attached.py` §Q2 — pane emits a query, with an attached client and a raw-mode pane:

| Query | Answered by | Reply | Forwarded to outer terminal |
|-------|-------------|-------|-----------------------------|
| `ESC[c` (DA1) | **tmux** | `ESC[?1;2;4c` | No |
| `ESC[>c` (DA2) | **tmux** | `ESC[>84;0;0c` | No |
| `ESC[6n` (DSR) | **tmux** | `ESC[1;1R` | No |
| `ESC[14t` | **tmux** | `ESC[4;814;2482t` | No |
| `ESC[16t` | **tmux** | `ESC[6;37;17t` | No |
| Kitty query, **unwrapped** | nobody | — | No — **silently dropped** |

Two consequences.

**(a) `parse_da1_has_sixel` is a false positive inside tmux.** tmux answers DA1 with
`ESC[?1;2;4c`. Parameter `4` is sixel. Tracing `caps.rs:213`: `split("[?").nth(1)` → `"1;2;4c"`,
`split('c').next()` → `"1;2;4"`, `any(|p| p == "4")` → **true**. So inside tmux,
`c.sixel == true` regardless of what the outer terminal supports, and `best_backend()`
(`caps.rs:56`) will select `Backend::Sixel` whenever Kitty detection fails — which is exactly the
passthrough-off case. On Ghostty, measured to have **no** sixel support (`DA1 = ?62;22;52c`), that
selects a backend the terminal cannot display.

The fix is not to special-case the parser — tmux's DA1 is truthful about *tmux*, which does have
sixel support compiled in. The fix is that **inside tmux, DA1 describes tmux, not the terminal, and
must not be used for backend selection.** When `in_tmux`, sixel support must be established by a
*wrapped* probe of the outer terminal, or treated as unknown. Given passthrough-off already forces
Unicode, the practical rule is: `in_tmux && !passthrough_enabled ⇒ Backend::Unicode`, never Sixel.

**(b) An unwrapped Kitty query is dropped, not answered and not forwarded.** This is the positive
justification for `wrap_tmux`: without the wrapper there is no path at all.

### 8.3 Reply routing is asymmetric — APC returns, CSI does not

Probe `attached.py` §Q1: the harness (as outer terminal) injected a reply and we checked the pane's
stdin, with the pane in raw mode.

| Injected by outer terminal | Reached pane stdin? |
|----------------------------|---------------------|
| `ESC_Gi=31;OK ESC\` (Kitty APC) | **Yes** — `'\x1b_Gi=31;OK\x1b\\'` |
| `ESC[?62;22;52c` (DA1 reply) | **No** — consumed by tmux |

tmux consumes CSI replies because it issued its own queries to the outer terminal and is parsing the
answers. APC replies it does not recognise are passed to the pane.

**This asymmetry is what makes the whole design viable.** Kitty graphics detection round-trips
through tmux (§3.3). Any detection built on CSI replies (DA1 sixel, `CSI 14t`, `CSI 16t`,
DECRQM for SGR-pixels mouse at `caps.rs:183`) **cannot** see the outer terminal from inside tmux —
those answers come from tmux itself.

Note the consequence for SGR-pixel mouse: `parse_decrqm_supported` inside tmux reports tmux's
support for mode 1016, not the terminal's. The existing test
`decrqm_permanently_reset_is_not_support` (`caps.rs:300`) exists precisely because a wrong answer
here collapses every click into the top-left corner. Inside tmux that risk returns through a
different door. **UNVERIFIED:** what tmux 3.7b answers for `ESC[?1016$p`; worth one probe before
enabling pixel mouse inside tmux. Until then, the conservative default inside tmux is cell-accurate
mouse with a documented loss of precision.

---

## 9. Per-terminal adapters

### 9.1 Shape

The multiplexer is a **transport decorator**, not a terminal. Two orthogonal axes:

```
TerminalAdapter   — what the outer terminal can do   (Kitty / Sixel / iTerm2 / Unicode)
TransportAdapter  — how bytes get there              (Direct / TmuxPassthrough / ScreenDrop)
```

Conflating them is how you end up with a `GhosttyInTmux` adapter and then a
`GhosttyInTmuxOverSsh` one. The transport decides framing and whether replies survive; the terminal
decides encoding.

```rust
// DESCRIBED, NOT IMPLEMENTED — crates/tf-term is the commander's file (see §13).
trait Transport {
    /// Wrap one complete, contiguous batch for the wire.
    fn frame(&self, payload: &[u8], out: &mut Vec<u8>);
    /// Can a terminal reply to a query reach us? Governs probe interpretation.
    fn replies_round_trip(&self) -> ReplyPath;
    /// Cell/window geometry source of truth for this transport.
    fn geometry_source(&self) -> GeometrySource;
    /// Does this transport drop output when we are not visible?
    fn drops_when_hidden(&self) -> bool;
}

enum ReplyPath { All, ApcOnly, None }
enum GeometrySource { CsiPreferred, IoctlPreferred }
```

| Transport | `frame` | `replies_round_trip` | `geometry_source` | `drops_when_hidden` |
|-----------|---------|----------------------|-------------------|---------------------|
| `Direct` | identity | `All` | `CsiPreferred` (caps.rs:73) | false |
| `TmuxPassthrough{on}` | `wrap_tmux`, whole batch | **`ApcOnly`** (§8.3) | **`IoctlPreferred`** (§6.3) | **true** |
| `TmuxPassthrough{all}` | `wrap_tmux`, whole batch | `ApcOnly` | `IoctlPreferred` | false |
| `TmuxBlocked` | discard + Unicode | `None` | `IoctlPreferred` | n/a |
| `ScreenDrop` | discard + Unicode | `None` | `IoctlPreferred` | n/a |

Every row is *Measured* except `ScreenDrop` (§10).

### 9.2 Terminal adapters, and what tmux does to each

Combining the mission's measured matrix with §8:

| Outer terminal | Direct backend | Through tmux, passthrough on | Notes |
|----------------|----------------|------------------------------|-------|
| **Ghostty 1.3.1** | Kitty (measured: `ESC_Gi=31;OK`) | Kitty — round-trip *Measured* to work | Cell 17×37 propagates exactly (§6.1) |
| **Apple Terminal 465** | Unicode (no Kitty, no sixel, `DA1 ?1;2c`) | Unicode | **Danger:** tmux fabricates 16×32 (§6.3) |
| **iTerm2 3.6.9** | Kitty (**UNVERIFIED** — TCC blocks automation) | Presumed Kitty, **UNVERIFIED** | `TERM_PROGRAM` heuristic dead inside tmux (§8.1) |

The Ghostty row is now end-to-end verified through tmux. The iTerm2 row remains blocked by macOS TCC
exactly as the mission states; nothing in this work changed that.

### 9.3 Terminal-native splits

Ghostty, iTerm2, and Apple Terminal all split natively, and this needs no work at all: each split is
a separate pty with its own `TIOCGWINSZ` and its own SIGWINCH. §6.2's resize policy covers it
unchanged, and there is no passthrough question because there is no multiplexer. The only
requirement is that resize handling be driven by SIGWINCH rather than assumed constant — which is
required for tmux anyway.

Worth stating explicitly so nobody builds a "split adapter": **terminal splits are not a special
case; tmux is.**

---

## 10. GNU screen and others

`screen` **is** installed here: `/usr/bin/screen`, version **4.00.03 (FAU) 23-Oct-06** — a 2006
build that Apple has shipped unchanged for years. It has no equivalent of `allow-passthrough` and no
graphics-protocol support.

**Recommendation: detect and decline, do not attempt.** `caps.rs:135` already sets `in_screen` from
`$STY`. When set, go straight to Unicode with:

```
terminal-fenster: running under GNU screen, which cannot pass graphics protocols
            through to the terminal. Using the Unicode half-block renderer.
            tmux 3.3+ supports graphics; screen does not.
```

**UNVERIFIED:** whether any newer screen (5.x) added a passthrough mechanism. I did not test screen
end-to-end — the probes above targeted tmux. The claim above is scoped to "no evidence of support";
it is not a proof of absence.

**zellij** is not installed here and is **UNVERIFIED**. It is worth a future probe: zellij is
Rust-native, actively developed, and its graphics story differs from tmux's. Do not ship claims about
it either way.

---

## 11. Test plan

The whole tmux surface is testable **without a display**, which matters because this machine is at a
lock screen. Everything in §1–§8 was produced this way.

### 11.1 Unit tests (no tmux binary needed)

`crates/tf-term` already has `tmux_wrapper_doubles_escapes` (`kitty.rs:371`). Add:

1. Round-trip: `unwrap(wrap_tmux(x)) == x` for payloads containing 0, 1, and many `ESC`.
2. Whole-batch wrap of a multi-chunk frame contains exactly one `ESC Ptmux;` and one terminating
   `ESC \` beyond the doubled inner ones.
3. `#{allow-passthrough}` output parser: `on`/`all`/`off`/`` → `Enabled/Enabled/Disabled/Unsupported`.
4. `tmux -V` parser accepts `tmux 3.7b`, `tmux 3.3`, `tmux next-3.8`; version gate at 3.3.
5. Cell-size heuristic: `in_tmux && cell == (16,32)` ⇒ flagged unverified.
6. DA1 `ESC[?1;2;4c` **must not** select `Backend::Sixel` when `in_tmux`. This is the §8.2 defect
   pinned as a regression test.

### 11.2 Integration tests (tmux binary required, no terminal required)

The pty-harness pattern is the reusable asset: own the outer pty, so bytes on the master fd are
exactly what a terminal would receive. Gate on `command -v tmux`; skip cleanly when absent.

| Test | Asserts | Status |
|------|---------|--------|
| `passthrough_off_swallows` | 0 Kitty APCs reach outer pty | ✅ measured |
| `passthrough_on_emits_bare_apc` | correctly unwrapped, ESC-undoubled | ✅ measured |
| `roundtrip_confirms_kitty` | injected `OK` reaches pane stdin | ✅ measured |
| `off_is_indistinguishable_from_unsupported` | no reply either way — pins §3.3 | ✅ measured |
| `display_message_resolves_inheritance` | `set -g on` ⇒ `on`; `show -wv` ⇒ empty | ✅ measured |
| `multichunk_survives_single_dcs` | 40/40 APCs intact | ✅ measured |
| `resize_delivers_sigwinch_with_pixels` | 146→73→60 cols, xpixel tracks ×17 | ✅ measured |
| `no_pixel_outer_yields_16x32` | fabrication detected and flagged | ✅ measured |
| `window_switch_is_silent` | no SIGWINCH, no focus event | ✅ measured |

Every one of these ran during this mission. They are CI-able on any Linux/macOS runner with tmux
installed and **no display server**.

### 11.3 Manual checks that still need a human

- Actual visual confirmation that a page renders inside tmux in Ghostty. The protocol round-trip is
  verified; pixels on glass are not, because the machine is at a lock screen.
- iTerm2 anything (TCC).
- tmux 3.3/3.4/3.5 behaviour — only 3.7b was tested.

---

## 12. Recommended changes to core files

Per the file-ownership rule I have written nothing outside this document. These are descriptions.

**`crates/tf-term/src/caps.rs`**
1. Add `tmux: TmuxState` to `Capabilities`; populate via §3.4. One `display-message` call, ~21 ms,
   at startup only.
2. Gate `best_backend()` (`:56`): when `in_tmux` and passthrough is not enabled, return
   `Backend::Unicode` directly — never `Sixel`. Fixes §8.2(a).
3. Do not use `c.sixel` for backend selection when `in_tmux`; DA1 describes tmux.
4. Invert the geometry preference in `viewport_px()` (`:73`) when `in_tmux`: prefer `TIOCGWINSZ`
   over `window_px`. Add the 16×32 fabrication flag (§6.3).
5. Wrap every graphics probe with `wrap_tmux` when `in_tmux`. Do **not** wrap CSI probes — tmux
   answers those itself and the wrapper would send them to the wrong parser.
6. `raw_replies` should record the tmux state so `doctor` can show it.

**`crates/tf-term/src/kitty.rs`**
7. `wrap_tmux` (`:241`) is correct — **call it**. Whole-batch, per §5.1.
8. `delete_all` (`:219`) and `delete_image` (`:211`) also need wrapping inside tmux, or teardown
   leaves the image on screen. `RESTORE_SEQ` (`tty.rs:27`) embeds `ESC_Ga=d,d=A ESC\` as a literal
   — **inside tmux that specific byte string will be swallowed**, so the "no stale image over the
   user's shell" guarantee that `tty.rs` is built around silently fails under tmux. This is the most
   user-visible defect in the current code after the missing detection itself.
   Note the constraint: the signal-handler path must stay async-signal-safe, so the tmux-wrapped
   restore sequence must be **precomputed at `acquire()` time** into a static buffer, not built
   during the handler.

**`apps/cli/src/main.rs`**
9. Print §4.1–§4.4 before `TtyGuard::acquire`.
10. SIGWINCH handler + 80 ms debounce (§6.2); full repaint, never damage.
11. 1 Hz visibility poll when `in_tmux` (§7.3), off otherwise.
12. `doctor`: report tmux version, resolved `allow-passthrough`, the query used, whether geometry is
    fabricated, and the round-trip probe result.

---

## 13. Open questions

| # | Question | Why it matters |
|---|----------|----------------|
| 1 | Does tmux 3.3–3.6 accept a single 160 KB DCS? | Decides whether whole-batch wrap is safe as default (§5.1) |
| 2 | What does tmux answer for `ESC[?1016$p`? | Decides pixel-accurate mouse inside tmux (§8.3) |
| 3 | Does tmux forward *genuine* terminal focus events? | Would remove the 1 Hz poll (§7.1) |
| 4 | Is `refresh-client -B` a viable push alternative? | Removes the per-poll 20.76 ms fork (§7.3) |
| 5 | zellij's graphics story | Second-most-likely multiplexer after tmux (§10) |
| 6 | Does tmux 3.7b's sixel support relay to a non-sixel outer terminal? | Affects whether Sixel is ever viable inside tmux (§8.2) |

---

## 14. Summary

The tmux integration reduces to five rules, all measured:

1. **Detect with `tmux display-message -p -t "$TMUX_PANE" '#{allow-passthrough}'`.** Every
   `show-options` form is wrong, and the most natural one is wrong in the direction that nags users
   who already configured tmux correctly.
2. **When off, print §4.1 and degrade to Unicode. Never edit the user's config.** Offer `set -w` for
   now and `set -g` for `~/.tmux.conf`, and say plainly that we will not write it ourselves.
3. **Wrap whole batches with the existing `wrap_tmux`, which is correct and verified.** Wrap the
   teardown sequence too, or images survive exit.
4. **Inside tmux, trust `TIOCGWINSZ`, distrust DA1 and `CSI 16t`.** tmux answers those about itself,
   and when the outer terminal reports no pixels it fabricates a 16×32 cell with total confidence.
5. **Recommend `on`, never `all`**, and handle invisibility with repaint-on-input plus a 1 Hz
   backstop poll.
