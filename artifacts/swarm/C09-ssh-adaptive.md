# C09 — SSH Adaptive Transport

**Mission:** specify in-band bandwidth/latency measurement, the quality/fps adaptation ladder,
frame-dropping policy, and reconnect, for Terminal-Fenster over SSH.

**Status:** design spec. Every number below is either measured on this machine, taken from a
named prior artifact, or computed from a formula that is validated against our one real
end-to-end data point in §3.1. Nothing here has been run over an actual WAN — see §10.

**Relationship to prior work.** A07 §5 already sketched in-band measurement, a rung ladder, and
ACK pacing. This document does not replace it; it (a) replaces the ACK mechanism with one that
does not conflate RTT with bandwidth, (b) replaces `max_in_flight = 1` with a byte-credit rule
that fixes a 6.3× throughput loss on fat high-latency links, and (c) adds the drop policy and
reconnect design A07 left open. Where I depart from A07 I say so explicitly and show the math.
B07 owns local frame scheduling (damage union, visibility gating); C09 consumes B07's output and
never re-derives it. B08 owns crash recovery; §7 covers only transport reconnect and defers
process death to B08.

---

## 0. TL;DR

1. **Do not time a DA1 round trip.** A trailing `ESC [ c` measures `RTT + serialisation +
   parse` as one scalar. On a 100 Mbit link with 100 ms RTT that reports 536 KB/s against a real
   12.5 MB/s — a **23× underestimate** that would pin the ladder to its bottom rung on the
   fastest link we support. Use **marker-bracketed drain timing** (§2.2): one identified kitty
   `a=q` query before the frame and one after, in the same write. The difference of the two reply
   timestamps cancels RTT exactly. Cost: 86 bytes out, 40 back — 0.15% on our real frame.
2. **Pace on in-flight *bytes*, not in-flight frames.** A07's `max_in_flight = 1` caps a
   100 Mbit / 100 ms-RTT link at **9.6 fps** when the link can carry 60. A credit of
   `drain_rate × 100 ms` recovers the full 60 fps, and degenerates to exactly `max_in_flight = 1`
   on links ≤ 5 Mbit, so it subsumes A07's rule rather than contradicting it (§4).
3. **Our 54 KB frame is example.com — a near-blank page. Do not plan capacity on it.** Scaling
   A07's measured Wikipedia render to our viewport gives **637 KB on the wire**, 11.4× larger.
   Both are carried through every table below.
4. **Drop frame rate before resolution.** A browser is read, not watched. Downscaling blurs text,
   which is the entire payload; a slideshow of sharp text beats smooth mush (§5.2).
5. **Change `q=2` to `q=1` on the frame path** (`crates/tf-term/src/kitty.rs:130`). `q=2`
   suppresses errors as well as acknowledgements, so a terminal that rejects a frame tells us
   nothing and the screen silently diverges forever. This is C01 defect D1, and adaptive
   transport is where it actually bites.

---

## 1. What the transport must deliver

| requirement | source | binding constraint |
|---|---|---|
| never let queued pixels delay the visual response to a keystroke | A07 §4.4 | in-flight byte credit (§4.2) |
| never exceed 4096 B of base64 per escape chunk | kitty spec; `kitty.rs:26` | fixed, do not "optimise" |
| survive a cooked pty | A07 §4.2 (base64 ∩ dangerous set = ∅) | already satisfied |
| restore the terminal on any exit | B08 | reconnect must not break the nanny |
| work when the far end is Ghostty, and degrade when it is Apple Terminal | measured matrix | §7.4 re-probe on reattach |

The transport is a **rate limiter with a measurement loop**, sitting between B07's frame
scheduler and the pty. It never decides *what* to draw (B07 does) — only *how big*, *how often*,
and *whether to send at all*.

---

## 2. In-band measurement

### 2.1 Why a DA1 round trip is the wrong instrument

The mission names timing `ESC [ c` as one option. It is cheap and universally answered, and
those are its only virtues. A single trailing query produces one number that fuses three
independent quantities:

```
t_reply − t_write  =  RTT  +  (wire_bytes / link_rate)  +  terminal_parse_time
                      ↑ fixed        ↑ what we want          ↑ varies with terminal
```

You cannot recover the middle term from one sample. The error is not academic. Take our real
frame (56,051 B on the wire, §3.3) on a 100 Mbit link at 100 ms RTT:

| quantity | value |
|---|---|
| true serialisation time | 4.5 ms |
| measured DA1 round trip | ≈ 104.5 ms |
| inferred "goodput" if you divide | 536 KB/s |
| actual link rate | 12,500 KB/s |
| **error** | **23× low** |

Feed that into any budget calculation and the controller concludes it is on a bad DSL line and
parks at the bottom rung. The faster and more distant the link, the worse the lie — which is
precisely backwards. A07 §5.2 already recognised this for the *startup* probe and solved it with
a two-point regression across payload sizes. The insight generalises: **never use an absolute
timestamp where a difference of two will do.** §2.2 applies it to every frame, continuously, for
free.

Two further defects rule DA1 out as a steady-state ACK. It carries **no identity** — every reply
is the same fixed string, so with more than one query outstanding you cannot say which frame a
reply belongs to, and a DA1 emitted by any other program sharing the terminal is
indistinguishable from ours. And it is answered by terminals that do not support graphics at all,
so a DA1 reply is not evidence the frame was accepted.

### 2.2 Marker-bracketed drain timing (MBDT)

Send, in one write to the pty:

```
[ marker A ] [ frame chunks … ] [ marker B ]
```

A terminal is a strictly sequential byte-stream parser, so it emits A's reply on reaching A and
B's reply on reaching B — that is, after it has consumed every byte of the frame between them.
Both replies traverse the identical return path.

```
RTT_est     =  t(reply A)  −  t(write A)          ← fixed cost, payload-free
drain_time  =  t(reply B)  −  t(reply A)          ← RTT cancels; pure absorption time
drain_rate  =  frame_wire_bytes / drain_time      ← bytes/s
```

`drain_rate` is the quantity to rate-limit on, and it is deliberately *not* "bandwidth": it is
`min(network delivery rate, terminal absorption rate)`. If the far terminal's escape parser or
image decoder is the bottleneck — plausible, and A07 §4.3 flags it as unmeasured — MBDT catches
that too and throttles correctly. Call it drain rate everywhere in the code; calling it bandwidth
will mislead whoever reads it next.

Bracketing both ends in a **single write** matters. It makes writer-idle contamination
structurally impossible: no gap can open between A and the frame, because the kernel receives
them together. An interleaved single-marker scheme (marker N closes frame N and opens frame N+1)
halves marker traffic but only yields valid samples while the pipe is continuously saturated, and
requires idle detection to discard the rest. Bracketing is simpler and the traffic it saves is
rounding error. Use bracketing.

### 2.3 The marker primitive already exists

`crates/tf-term/src/kitty.rs:225` is exactly the marker:

```rust
pub fn support_query(id: u32) -> Vec<u8>     // ESC _ G i=<id>,s=1,v=1,a=q,t=d,f=24;AAAA ESC \
```

It is 43 bytes at a 10-digit id and elicits `ESC _ G i=<id>;OK ESC \` (20 bytes). Three
properties make it the right choice over DA1:

- **It echoes the id back.** That is a 32-bit correlator, free. Replies are self-labelling, so
  out-of-order or stale replies are unambiguous rather than merely suspicious.
- **`a=q` neither stores nor displays the image.** Per the spec text quoted in A07 §5.2, query
  "will not replace an existing image with the same id, nor will it store the image" — so
  rotating ids leak no terminal memory and cannot disturb the page surface at
  `PAGE_IMAGE_ID = 1000`.
- **It is verified against a real terminal.** `kitty.rs:362` asserts the exact byte shape that
  Ghostty 1.3.1 answered with `OK`. The marker is not a new protocol surface; it is the handshake
  we already ship, reused.

**Id allocation.** Markers occupy `0x7000_0000 .. 0x7FFF_FFFF`, encoded
`0x7000_0000 | (epoch << 16) | seq`, where `epoch` is a 12-bit reconnect generation (§7.3) and
`seq` a 16-bit per-epoch counter. A reply from a previous epoch is therefore recognisable on
sight and discarded rather than corrupting the estimator. This does not collide with
`PAGE_IMAGE_ID` and does not depend on fixing C01 D8.

**Marker `q` value.** `support_query` omits `q`, so it defaults to `q=0` and replies. Leave it.
The *frame* keeps a non-replying `q` (§2.5), so a healthy frame produces exactly two replies —
one per marker — and nothing else.

### 2.4 Sampling rules

| rule | value | why |
|---|---|---|
| frames that produce a drain sample | wire ≥ 8 KB | below that, marker overhead is 38% of a 1-cell update (§G) and the sample is dominated by quantisation |
| frames below 8 KB | single trailing marker | liveness + RTT only; no drain sample |
| `RTT_ewma` | α = 0.25 | responsive; RTT shifts on route changes |
| `drain_rate_ewma` | α = 0.20 | matches A07 §5.3 |
| outlier rejection | discard sample if `drain_time` < 1 ms or > 20× EWMA | clock granularity at one end, stall/suspend at the other |
| epoch mismatch | discard silently | §7.3 |
| cold start | bracketed pair around a 64 KB and a 512 KB dummy `a=q`, per A07 §5.2 | one estimate before the first real frame |

### 2.5 Frame acknowledgement and the `q=2` problem

`crates/tf-term/src/kitty.rs:130` emits `a=T,f=24,t=d,q=2` on every frame. The kitty `q` key has
three levels: `q=0` reply always, `q=1` suppress success but **report errors**, `q=2` suppress
everything including errors.

`q=2` means a terminal that refuses a frame — out of image memory, malformed chunk, a truncated
transfer left by a dropped connection — says nothing, and the client believes the screen is
correct when it is stale. There is no other channel that would tell us. **Set `q=1`.** The cost
is zero bytes on the happy path (errors are, by construction, rare) and the benefit is that §6.4's
"repair on error" rule becomes implementable at all. C01 filed this as defect D1 on general
principle; adaptive transport is the concrete reason it must be fixed.

With `q=1`, a healthy frame still produces exactly two replies (the markers). An error adds a
third, tagged with the frame's own image id — trivially distinguishable from marker ids by range.

### 2.6 Blocking dependency: there is no reply demultiplexer

This is the one thing that stops C09 being implementable today, and it is a core-file change I am
not making.

`crates/tf-term/src/caps.rs:92-120` reads terminal replies with a blocking `read_reply` against a
deadline. That is correct for a one-shot startup handshake and unusable in steady state, because
in steady state marker replies arrive **interleaved with the user's keystrokes on the same fd**.
`crates/tf-term/src/input.rs` has no APC branch at all (C01 defect D3), so today an incoming
`ESC _ G i=…;OK ESC \` is decoded as an Escape keypress followed by literal text typed into the
page.

What is needed is a splitter in the input path: a state machine that recognises APC
(`ESC _ … ESC \`) and CSI-response terminators, routes those bytes to the transport's reply
channel, and passes everything else through to the existing key decoder unchanged. C01 D3 already
argues this is required for security (only `q=2` currently prevents a hostile page from
synthesising keystrokes); C09 needs it for function. **One fix, two problems.** Specified for the
commander in §9.

---

## 3. Bandwidth math from our real frame

### 3.1 The wire formula, validated

I reimplemented `encode_rgb_frame`'s framing exactly as written at `kitty.rs:124-158` and solved
for the deflate size that reproduces our measured wire byte count:

```
wire = 3 + len(control) + 1 + 2 + 9·(chunks−1) + base64_len
base64_len = ceil(C/3)·4 ,  chunks = ceil(base64_len / 4096)
control    = "a=T,f=24,t=d,q=2,o=z,s=2482,v=814,i=1000,m=0"   (44 B)
```

| quantity | value |
|---|---|
| measured wire bytes (end-to-end run, Ghostty 1.3.1) | **53,999** |
| deflate output that reproduces it | **40,372 B** |
| chunks | 14 |
| raw RGB (2482×814×3) | 6,061,044 B |
| zlib level-1 ratio on RGB | **150.1 : 1** |
| BGRA → wire | 149.7 : 1 |

The formula reproduces 53,999 exactly, so every projection below rests on validated arithmetic
rather than an estimate of framing overhead.

**A 32-byte discrepancy worth one line.** The brief states the BGRA frame was 8,081,424 B, but
2482 × 814 × 4 = **8,081,392** — 32 bytes (8 pixels) less. It cannot have been 8,081,424 on the
real run: `bgra_to_rgb` consumes `chunks_exact(4)` and `encode_rgb_frame` asserts
`rgb.len() == w·h·3` (`kitty.rs:108`), so a buffer of 8,081,424 would have produced a 6,061,068-byte
RGB vector and **panicked**. It did not. The figure in the brief is a transcription slip, not
stride padding. No action needed; recorded so nobody later "fixes" the assert.

### 3.2 Our 54 KB frame is not a typical page

example.com is white space and a paragraph of text. A 150:1 zlib ratio is what a near-blank page
compresses to, and planning capacity on it would be self-deception. A07 §2.3 measured real pages
at 1440×900 with the identical codec (`f=24 o=z`, level 1). Scaling by our pixel count
(2,020,348 / 1,296,000 = 1.559):

| page | A07 @1440×900 | scaled to 2482×814 | wire B | on-wire B |
|---|---|---|---|---|
| **example.com (ours, measured)** | — | **40,372** | **53,999** | **56,051** |
| GitHub repo page | 168,934 | 263,353 | 351,955 | 365,061 |
| Hacker News | 205,595 | 320,504 | 428,326 | 444,302 |
| **Wikipedia article (worst)** | 294,883 | 459,696 | 614,319 | **637,141** |

Both extremes are carried through everything that follows. Treat **Wikipedia-class as the design
point** and example.com as the best case.

### 3.3 On-wire overhead above the escape sequence

`wire_bytes` (`kitty.rs:161`) counts escapes and base64 but stops at the pty. Over SSH:

- **SSH binary packet.** With `chacha20-poly1305@openssh.com`: 4 B encrypted length + 1 B padding
  length + ~8 B padding + 16 B Poly1305 tag, plus a 9-byte `SSH_MSG_CHANNEL_DATA` header
  (type + recipient channel + length) ≈ **38 B per 32,768-byte packet** (`CHAN_SES_PACKET_DEFAULT`,
  cited in A07 §4.4). ≈ 0.12%.
- **TCP/IP.** MSS 1448 (1500 MTU − 20 IPv4 − 20 TCP − 12 timestamp option), 52 B of header per
  segment ≈ **3.6%**.

Combined factor **1.038** at our frame size. Accounting is at the IP layer; add ~1.8% more if you
measure at the Ethernet frame. Every "on-wire" column below includes this.

### 3.4 Achievable fps at 1, 10, and 100 Mbit — the mission's question

Serialisation only; pacing and RTT effects come in §4. Capped at 60 fps because that is the
measured engine ceiling (p50 frame gap 16.65 ms).

| content (2482×814, zlib-1) | on-wire B | **1 Mbit** | **10 Mbit** | **100 Mbit** |
|---|---|---|---|---|
| **example.com (our measured frame)** | 56,051 | **2.23 fps** | **22.30 fps** | **60 fps** (link allows 223) |
| GitHub repo page | 365,061 | 0.34 | 3.42 | 34.24 |
| Hacker News | 444,302 | 0.28 | 2.81 | 28.13 |
| **Wikipedia article** | 637,141 | **0.20 fps** | **1.96 fps** | **19.62 fps** |

Time to transmit one full frame — the number a user actually feels when a page paints:

| content | 1 Mbit | 10 Mbit | 100 Mbit |
|---|---|---|---|
| example.com @1.00 | 448 ms | 45 ms | 4 ms |
| example.com @0.50 | 113 ms | 11 ms | 1 ms |
| Wikipedia-class @1.00 | **5,097 ms** | 510 ms | 51 ms |
| Wikipedia-class @0.50 | 1,275 ms | 127 ms | 13 ms |

### 3.5 What the user experiences at each link speed

The framing that matters: **a browser is not a video stream.** For most of a session the correct
frame rate is zero — a rendered page is static until the user acts. The fps column governs
*transients* (scroll, video, animation, a loading spinner), while the paint-latency table governs
the dominant experience. Judge each link on both.

**1 Mbit — "usable for reading, unusable for scrolling."** A real page paints in **5.1 s** at full
scale, 1.3 s at half. Once painted, reading costs nothing and typing is fine: a single-cell damage
update is 225 B on the wire, which is 1.8 ms — the ladder's bottom rungs are effectively free at
any link speed (§5.3). Scrolling is the cliff. A07 §3.1 measured that a **one-pixel scroll dirties
78% of the screen**, so scrolling degenerates to full frames at 0.20 fps — a five-second-per-step
slideshow. Verdict: acceptable for read-and-click if and only if scroll uses the source-rect
re-placement path (A07 §3.7, 55 B). Without that path, 1 Mbit is not shippable. Downscale to 0.33
for a 1.80 fps scroll if the page must animate at all.

**10 Mbit — "the realistic floor for a good experience."** Real pages paint in **510 ms**, which
reads as a fast page load. Scrolling and light animation run at **1.96 fps** full-scale — still a
slideshow — or **17.99 fps** at 0.33 scale. Our blank-page frame hits 22 fps. Typing, clicking,
and reading are indistinguishable from local. Verdict: good browsing, poor motion. This is the
link speed to target and demo on.

**100 Mbit — "indistinguishable from local for everything except video."** Real pages paint in
**51 ms**, below the threshold of noticing. Scroll and animation run at **19.62 fps** full-scale
and hit the 60 fps engine cap at 0.5 scale. Our measured frame saturates the engine outright.
Verdict: fully interactive. The residual gap versus local is fullscreen video, which at 19.6 fps
looks like a bad video call — and no kitty-legal codec fixes that, because the format has no
quality knob (A07 §5.4). Fullscreen video over SSH needs topology (b′) and WebP; treat it as out
of scope, not as a bug.

**The caveat that governs all three.** Every figure above is serialisation. §4 shows that with
naïve pacing the 100 Mbit column collapses to 9.6 fps the moment RTT reaches 100 ms — so on this
axis the transport design, not the link, is the binding constraint.

---

## 4. Pacing: byte credit, not frame count

### 4.1 Where `max_in_flight = 1` breaks

A07 §5.3 requires at most one frame in flight, reasoning that a 2 MiB SSH channel window filled
with pixels stalls interactivity. The concern is real; the mechanism deserves restating precisely,
because getting it wrong leads to the wrong fix.

Frames travel server→client; keystrokes travel client→server. TCP is full-duplex, so **a keystroke
is never itself queued behind pixel data.** What queues is the *response*: the frame showing the
effect of the keystroke sits behind every pixel already committed to the downstream window. Fill
2 MiB at 5 Mbit and the user waits 3.4 s to see their own character. Same conclusion as A07, but
the quantity to bound is **downstream bytes outstanding**, not frames outstanding — and those two
differ enormously once frames get small or links get fast.

Strict `max_in_flight = 1` forces `fps = 1 / (drain + RTT)`, injecting a full idle RTT between
every pair of frames. Our 54 KB frame:

| RTT | 1 Mbit | 10 Mbit | 100 Mbit | 1 Gbit |
|---|---|---|---|---|
| 5 ms | 2.21 | 20.06 | 60.00 | 60.00 |
| 20 ms | 2.13 | 15.42 | 40.84 | 48.90 |
| 50 ms | 2.01 | 10.54 | 18.35 | 19.82 |
| 100 ms | 1.82 | 6.90 | **9.57** | 9.96 |
| 150 ms | 1.67 | 5.13 | 6.47 | 6.65 |
| 250 ms | 1.43 | 3.39 | 3.93 | 3.99 |

At 100 Mbit / 100 ms the link carries 223 fps, the engine can produce 60, and the pacer delivers
**9.57** — a **6.3× loss**, entirely self-inflicted. At 1 Gbit / 250 ms it is 15× off. The rule
costs nothing at 1 Mbit (2.21 vs 2.23) because there the link genuinely is the constraint; it is
catastrophic exactly where Terminal-Fenster should look best.

### 4.2 The rule

Bound **outstanding downstream bytes**, not frames:

```
credit_bytes = max( one_frame , drain_rate_ewma × LAG_MS / 1000 )
may_send(frame) = (bytes_outstanding + frame_wire_bytes) ≤ credit_bytes
```

`bytes_outstanding` decrements when marker B for that frame replies. The `max(one_frame, …)`
floor guarantees forward progress on any link, however slow.

| RTT | 1 Mbit | 10 Mbit | 100 Mbit | 1 Gbit |
|---|---|---|---|---|
| 5 ms | 2.21 | 22.30 | 60.00 | 60.00 |
| 20 ms | 2.13 | 22.30 | 60.00 | 60.00 |
| 50 ms | 2.01 | 21.09 | 60.00 | 60.00 |
| 100 ms | 1.82 | **13.81** | **60.00** | 60.00 |
| 250 ms | 1.43 | 6.78 | 60.00 | 60.00 |

`LAG_MS = 100`, our 54 KB frame. The 100 Mbit column is fully recovered at every RTT. The 1 Mbit
column is **unchanged** — at 125 KB/s the credit is 12.5 KB, below one frame, so the floor
engages and the rule degenerates to precisely `max_in_flight = 1`. **The byte-credit rule
subsumes A07 §5.3 rather than contradicting it**; A07's rule is its low-bandwidth special case.

### 4.3 `LAG_MS` is the interactivity knob, and it is the only one

`LAG_MS` has a direct physical meaning: **the maximum staleness the transport is willing to have
queued, and therefore the worst-case latency it adds to input echo.** That makes it the one
parameter to reason about, and it should be set from a human threshold, not tuned empirically.

A07 §5.5 puts the point where remote echo "feels broken" at ~150 ms RTT. Budgeting the transport
a fraction of that:

| `LAG_MS` | worst-case added echo | pipelining at 100 Mbit | verdict |
|---|---|---|---|
| 33 | 33 ms | 7 frames | conservative; leaves throughput on the table below 25 Mbit |
| **100** | **100 ms** | **22 frames** | **default** — under the 150 ms threshold, recovers all fast links |
| 250 | 250 ms | 55 frames | perceptibly laggy typing; only for a "throughput" mode |

Ship `LAG_MS = 100`. Expose it as `--lag-budget-ms` for the one user who wants a smooth-scrolling
demo over a satellite link and does not care about typing.

One consequence to accept honestly: on a slow link the credit floor means a single Wikipedia-class
frame (637 KB) is already 5.1 s of queued pixels at 1 Mbit, far above any `LAG_MS`. Bytes cannot
be recalled once written. The only defence is not to generate them — which is what the ladder in
§5 is for. Pacing bounds queueing; the ladder bounds frame size; neither substitutes for the
other.

---

## 5. The adaptation ladder

### 5.1 Budget

```
target_interval = 1 / target_fps
budget_bytes    = drain_rate_ewma × target_interval × 0.70
```

The 0.70 margin is A07 §5.3's and I keep it: it leaves headroom so a frame that compresses worse
than predicted does not immediately overrun the credit.

### 5.2 Drop frame rate before resolution

The kitty-legal codec has no quality knob (A07 §2.2 — `f` is 24/32/100, `o` is only `z`), so the
levers in topology (a) are **frame rate** and **render scale**. A07 §5.4 lists both without
ordering them. The order matters and it is not the video-codec instinct:

**Hold scale at 1.00 as long as possible; spend frame rate first.** A web page is text, and text
is exactly the content that survives frame-rate reduction and does not survive resampling.
Rendering at 0.5 and letting the terminal upscale into the same cell rectangle blurs every glyph
on screen permanently, for as long as the page is displayed — whereas a low frame rate costs
nothing at all while the page is static, which is most of the time. A sharp slideshow beats smooth
mush. Only drop scale when frame rate has bottomed out and the page still cannot animate.

### 5.3 Rungs

| rung | strategy | typical on-wire | source |
|---|---|---|---|
| **R0** | scroll: source-rect re-placement | **55 B** | A07 §3.7 |
| **R1** | damage rect, 1 cell (17×37 px) | **225 B** | computed, §D |
| **R2** | damage rect, 10 cells / one word | 306 B | computed |
| **R3** | damage rect, 2 text lines | 810 B | computed |
| **R4** | damage rect, one paragraph | 2,875 B | computed |
| **R5** | damage rect, full-width 2 rows | 5,200 B | computed |
| **R6** | full frame, scale 1.00 | 56 KB (blank) – 637 KB (Wikipedia) | §3.2 |
| **R7** | full frame, scale 0.75 | 32 KB – 358 KB | pixel-linear estimate |
| **R8** | full frame, scale 0.50 | 14 KB – 159 KB | pixel-linear estimate |
| **R9** | full frame, scale 0.33 | 6 KB – 69 KB | pixel-linear estimate |

R0–R5 are damage-driven and chosen by B07's union, not by the controller — they are listed to
make the crucial point that **the bottom of the ladder is free on every link we support**: a
1-cell update is 1.8 ms even at 1 Mbit. Typing never degrades. The controller only picks among
R6–R9, and only when B07 reports damage exceeding the ~35% crossover where tiling loses to a full
frame (A07 §3.4).

Scaled sizes assume compressed bytes fall linearly with pixel count. This is **conservative**:
downsampling removes high-frequency detail, so real deflate output should be somewhat smaller than
`scale²`. Unmeasured — flagged in §10.

### 5.4 Selection thresholds

Given `drain_rate_ewma`, walk scales 1.00 → 0.75 → 0.50 → 0.33 and, at each, take the highest fps
in {60, 30, 15, 8, 4, 2, 1} that fits the budget. First hit wins — which encodes §5.2's ordering.
For the Wikipedia-class design point:

| measured drain rate | equivalent link | selected rung |
|---|---|---|
| ≥ 100 MB/s | 1 Gbit | scale 1.00 @ 60 fps |
| ≥ 12.5 MB/s | 100 Mbit | scale 1.00 @ 8 fps |
| ≥ 3.13 MB/s | 25 Mbit | scale 1.00 @ 2 fps |
| ≥ 1.25 MB/s | 10 Mbit | scale 1.00 @ 1 fps |
| ≥ 625 KB/s | 5 Mbit | scale 0.75 @ 1 fps |
| ≥ 125 KB/s | 1 Mbit | scale 0.33 @ 1 fps |
| < 125 KB/s | sub-1 Mbit | static-only: R0–R5 damage, full frames on navigation only |

Read this as *animation capability*, not page-load capability — a static page still paints at the
latencies in §3.4 regardless of the rung.

RTT gates the ladder independently of bandwidth:

| `RTT_ewma` | class | additional constraint |
|---|---|---|
| < 30 ms | LAN | none |
| 30–100 ms | continental | none (byte credit handles it) |
| 100–250 ms | intercontinental | cap 30 fps; smoother motion is wasted under this much input lag |
| > 250 ms | satellite / cellular edge | **static-only**: suppress continuous animation entirely, R0–R5 plus navigation keyframes. Above A07 §5.5's 150 ms threshold the session is a remote-control experience, and spending the link on animation actively hurts it. |

### 5.5 Hysteresis

Fast down, slow up — A07 §5.4's policy, with concrete triggers:

**Down 2 rungs immediately** on any of: a frame whose wire size exceeds `budget_bytes`; a drain
sample above 2× `drain_rate_ewma`; a marker timeout (§7.1); an error reply from the terminal
(requires `q=1`, §2.5).

**Up 1 rung** only when *all* hold: ≥ 20 consecutive frames sent; ≥ 2 s elapsed since the last
rung change; p95 drain over that window ≤ 60% of budget; zero drops and zero timeouts in the
window. The dual frame-count-and-time condition matters — 20 frames is 0.7 s at 30 fps but 20 s at
1 fps, and ramping on frame count alone makes the recovery time on slow links absurd.

---

## 6. Frame-dropping policy

### 6.1 Queue depth is exactly one

The transport holds **one pending frame**. A new frame arriving while one is pending replaces it;
the older frame's damage rectangle is unioned into the replacement. This is B07's local rule
(`§4.1` rectangle union) extended across the link, and it is the only correct policy: over a slow
link the engine will always outrun the transport, and the user wants the *newest* state, never a
backlog of history. Queue-and-drain would render a stale slideshow that lags further behind by the
second.

### 6.2 Damage may be coalesced but never discarded

A dropped *frame* is harmless — the newer one supersedes it. A dropped *damage rectangle* is a
permanent defect: the terminal keeps showing stale pixels in that region with nothing to correct
it, because the next frame only carries its own damage. Therefore:

- Coalescing a pending damage update into a newer one **must** union the rectangles.
- If the union exceeds ~35% of viewport area, promote to a full frame (A07 §3.4 crossover — below
  that, tiling wins; above it, tiling costs more than the whole screen).
- If the union is ever lost or its validity is in doubt (epoch change, error reply, resize),
  escalate to a keyframe (§6.4). Never guess.

### 6.3 Priority classes

| class | trigger | treatment |
|---|---|---|
| **urgent** | first frame after any input event | may exceed `budget_bytes` by 1.5×; never deferred behind a non-urgent pending frame |
| **normal** | ordinary repaint | full ladder + credit rules |
| **speculative** | animation/video repaint with no input in the last 500 ms | dropped outright whenever credit is unavailable; no union tracking beyond the current rect |

Urgent frames get a budget override rather than a credit override. Overriding the *credit* would
push bytes into a window that is already the problem; overriding the *budget* only permits a
larger single frame once the pipe has room. The distinction is the whole point of §4.

There is no mechanism to cancel bytes already written — SSH offers none. Interactivity is
therefore protected by prevention (small `LAG_MS`), not by preemption. Do not add a "cancel
in-flight frame" feature; it cannot exist.

### 6.4 Repair

A keyframe — full frame, scale 1.00, credit and budget rules waived — is sent on exactly these
events, and no others:

1. Reconnect (§7).
2. Viewport resize.
3. An error reply from the terminal for a frame image id (needs `q=1`).
4. Coalescing overflow: union exceeded 35% and the full-frame promotion itself was dropped.

Note there is deliberately **no periodic paranoia keyframe**. With `q=1` the terminal tells us
when it rejects something, so blind retransmission buys nothing and costs 637 KB. If `q=2` is kept
instead of adopting `q=1`, a periodic keyframe becomes mandatory — a 637 KB tax every N seconds
purely to compensate for having silenced the error channel. That trade is the practical argument
for §2.5.

### 6.5 Accounting

Every drop increments a counter by class, and the transport reports `sent`, `coalesced`,
`dropped_speculative`, `rung_changes`, `timeouts`, `drain_rate_p50/p95`, `rtt_p50/p95` to B07's
existing dropped-frame accounting (B07 §6). A drop rate that is high *and* stable is healthy — it
means the pacer is doing its job. A rising *coalesced* count with a flat *sent* count means the
engine is repainting faster than the link can carry, which is the expected steady state on a slow
link and not an error to surface to the user.

---

## 7. Reconnect

### 7.1 Detection

Three independent signals, whichever fires first:

| signal | threshold | note |
|---|---|---|
| marker timeout | no reply within `max(5 × RTT_ewma, 750 ms)`, 3 consecutive | the primary signal; it is already instrumented for free by §2 |
| pty read error | `read()` → 0, `EIO`, or `EPIPE` | ssh exited or the master closed |
| SSH transport | `ServerAliveInterval 5`, `ServerAliveCountMax 3` | a 15 s backstop for silent black holes |

Marker timeouts detect a stall far faster than SSH keepalives, which is why the measurement loop
doubles as the liveness loop. On the first timeout, before declaring a drop, drop 2 rungs — a
severe slowdown and a dead link look identical at first, and degrading costs nothing if the link
recovers.

### 7.2 Two classes of reconnect, and the architectural gap

| class | what survived | recovery |
|---|---|---|
| **R-A: transport only** | engine alive, page state intact | resync preamble + re-probe + keyframe (§7.4). Sub-second. |
| **R-B: engine died** | nothing | B08's session restoration; C09 only re-establishes the pipe |

**R-A is not currently possible, and this is a real architectural gap.** `apps/cli/src/main.rs:404-411`
spawns the engine as a *child* of the CLI, and `main.rs:670` kills it on exit. When SSH drops, the
session leader dies, SIGHUP propagates, and the engine dies with it — so every reconnect is R-B and
the user loses their tabs, scroll position, and form state on any network blip.

The fix is available cheaply because the socket already exists: `main.rs:399` binds a Unix socket
at a private 0700 path. If the engine is detached (`setsid`, survives SIGHUP) and the CLI *adopts*
an existing socket when one is live rather than always spawning, R-A falls out. That is a
core-file change; specified in §9, not made here. Until then, document honestly that SSH drops
lose page state.

### 7.3 Epoch

Every reconnect increments a 12-bit `epoch`, encoded in the marker id (§2.3). On epoch change:

- All outstanding `bytes_outstanding` accounting resets to zero (those bytes are gone with the
  connection).
- Any marker reply bearing a prior epoch is discarded silently — it is a ghost from the old
  connection, and feeding it to the estimator would produce a wildly wrong drain sample at exactly
  the moment the controller is most vulnerable.
- `drain_rate_ewma` and `RTT_ewma` are **discarded, not carried over.** A reconnect frequently
  means a different path — Wi-Fi to cellular, a new route, a different exit. Reusing the old
  estimate to pick a rung is the single easiest way to blast a 637 KB frame into a link that
  cannot take it. Cold-start the estimator (§2.4) and ramp.

### 7.4 The resync preamble

The far terminal may be in an arbitrary parser state: the drop could have landed mid-APC, or
between chunks of a 14-chunk transfer with the terminal still awaiting `m=0`. Send these three
sequences, in this order, before anything else:

```
1.  ESC \                      terminate any partial APC/DCS string left mid-sequence
2.  ESC _ G q=1,m=0 ; ESC \    close any dangling chunked transfer
3.  ESC _ G a=d,d=A ESC \      delete all images  (kitty.rs:219)
```

Step 1 is required because if the drop landed inside an APC, the terminal is consuming bytes
looking for a string terminator and will swallow whatever we send next — including step 2. A lone
ST outside a string state is ignored by conforming terminals. Step 2 closes a transfer left open
mid-chunk; the truncated image will fail to decode, and with `q=1` we will see that error and can
ignore it by id. Step 3 clears stale surfaces so the old frame cannot linger under the new one.

Steps 1 and 2 are **UNVERIFIED** against Ghostty — reproducing a mid-transfer drop needs a real
SSH session, which the environment blocks (§10). They are cheap, idempotent, and safe to send
unconditionally even if a given terminal ignores them.

Then, in order:

4. **Re-run `caps::detect()`** (`caps.rs:128`). Do not assume capabilities survived. A user who
   reattaches a tmux session from Ghostty to Apple Terminal has moved from kitty graphics to the
   Unicode half-block fallback, and rendering kitty escapes into Apple Terminal paints garbage
   across their screen. The far end after a reconnect is a **different terminal until proven
   otherwise.**
5. **Re-read geometry** (`CSI 14t` / `CSI 16t`). `TIOCGWINSZ` returns zeroes through most SSH
   sessions (A07 §5.1), so the CSI queries are mandatory. If geometry changed, resize the OSR
   surface before drawing.
6. **Cold-start the drain estimator** (§2.4).
7. **Send a keyframe.** All damage state is invalid by definition.

### 7.5 Backoff

`0, 0.5, 1, 2, 4, 8, 15, 30 s`, capped at 30, with ±20% jitter. Reset to 0 after 60 s of a stable
connection — not immediately on connect, or a link that flaps every 10 s reconnects at full rate
forever. Attempts are unbounded in an interactive session (the user can Ctrl-C); bound them at 10
in any non-interactive mode.

**The terminal-restoration guarantee outranks reconnection.** If the CLI is killed while
reconnecting, B08's restore nanny must still fire. Reconnect logic must never install a signal
handler or hold a lock that could prevent teardown — the user's shell being left in raw mode with
a stale image over it is a worse outcome than any number of lost frames.

---

## 8. Verification

All of this is testable without a WAN, which matters because the environment cannot provide one
(§10).

| # | test | method | pass criterion |
|---|---|---|---|
| V1 | wire formula matches the encoder | property test: random `C`, compare `encode_rgb_frame().wire_bytes` to the closed form | exact equality, all sizes |
| V2 | MBDT cancels RTT | pty pair + a synthetic terminal that replies after a programmed delay + rate | recovered `drain_rate` within 5% of programmed, across RTT 0–500 ms |
| V3 | DA1 comparison | same harness, trailing-DA1 estimator | reproduces the 23× error of §2.1 — proves the fix is load-bearing, not decorative |
| V4 | credit rule ≡ A07 at low bandwidth | simulate 1 Mbit | `max_in_flight` collapses to 1 |
| V5 | credit rule recovers fast links | simulate 100 Mbit / 100 ms | ≥ 55 fps (vs 9.57 for frame-count pacing) |
| V6 | damage never lost | fuzz: random damage rects, random drops | union of all sent rects ⊇ union of all generated rects, always |
| V7 | epoch isolation | inject a stale-epoch reply mid-stream | discarded; estimator unperturbed |
| V8 | resync preamble | write a truncated chunked transfer to Ghostty, then the preamble, then a frame | frame renders correctly — **needs a real terminal; run manually** |
| V9 | ladder monotonicity | sweep drain rate 10 KB/s → 1 GB/s | selected rung is monotone; no oscillation at boundaries |

V2, V3, V4, V5 need only a pty pair and a mock terminal — a few hundred lines, no network, no
Electron, and CI-able on any machine. **Build the mock terminal first**; it is the highest-leverage
test asset in this whole document, and every threshold in §5 becomes empirical rather than
asserted the moment it exists.

---

## 9. Changes required in core files (for the commander — I did not make these)

Ordered by whether C09 is implementable without them.

**Blocking:**

1. `crates/tf-term/src/input.rs` — **add a terminal-reply demultiplexer.** A state machine
   recognising APC (`ESC _ … ESC \`) and CSI responses, routing them to a transport channel and
   passing all other bytes to the existing key decoder. Without this, marker replies are decoded
   as keystrokes and MBDT cannot exist. Also closes C01 defect D3 (keystroke injection). One fix,
   two problems.
2. `crates/tf-term/src/kitty.rs:130` — **`q=2` → `q=1`** on the frame path. Restores the error
   channel that §6.4's repair rule and §5.5's down-trigger both depend on. C01 defect D1.

**Needed for full function:**

3. `apps/cli/src/main.rs:404-411, 670` — **detach the engine and adopt an existing socket.** Turns
   every reconnect from R-B (lose all page state) into R-A (sub-second, state intact). See §7.2.
4. `crates/tf-proto/src/lib.rs` — transport telemetry in the event stream: `drain_rate_ewma`,
   `rtt_ewma`, `rung`, `bytes_outstanding`, drop counters by class. Needed for §6.5 and for any
   field diagnosis of a slow link.
5. `crates/tf-term/src/caps.rs` — expose `detect()` for **re-invocation on reconnect** (§7.4 step
   4). It is currently shaped as a startup-only path.

**Nice to have:**

6. `crates/tf-term/src/kitty.rs` — a `marker(id)` alias for `support_query(id)`. Purely naming;
   the function is already exactly right, and a caller reading `support_query` in the frame hot
   path will reasonably wonder why capability detection runs every frame.

---

## 10. UNVERIFIED

Stated plainly, because several of these are load-bearing.

1. **No measurement over a real WAN.** Every fps figure is computed from a validated wire formula
   and a link rate, not observed. The formula is exact (§3.1); the link model (§3.3) is standard
   but unconfirmed for our traffic pattern.
2. **Downscaled compression is assumed pixel-linear.** Real deflate output at scale 0.5 should be
   *smaller* than `0.25 × full`, so §5.3's R7–R9 sizes are conservative — but by an unmeasured
   margin. One afternoon with the existing encoder settles it.
3. **Wikipedia-class sizes are A07's 1440×900 measurements scaled by pixel count.** Compression
   ratio is not perfectly scale-invariant. Direction of error unknown.
4. **The resync preamble (§7.4 steps 1–2) has not been executed against any terminal.** Reproducing
   a mid-transfer drop needs a live SSH session; the machine is at a lock screen and Chromium
   children fail under the agent sandbox.
5. **Terminal absorption rate is unmeasured** for every terminal (A07 §4.3 flags the same gap). If
   a terminal's image decode is slower than the network, MBDT reports that correctly and the
   controller adapts — but we do not know how often that case dominates.
6. **iTerm2 remains unverified** (macOS TCC blocks automation). Its 1 MiB inline-image limit is
   documented but untested by us.
7. **`ESC _ G q=1,m=0 ; ESC \` with an empty final chunk** may not be accepted by all terminals as
   a transfer terminator. It is harmless if ignored, but if it is *not* a valid terminator, a
   dangling transfer survives the preamble and step 3's `a=d` may itself be swallowed.
8. **tmux interaction with markers is untested.** A07 §6 covers passthrough for frames; whether
   marker replies survive `allow-passthrough` cleanly, and with what added latency, is unknown.

---

## 11. Recommendation

**Build the mock-terminal pty harness (§8, V2–V5) before writing any adaptive code.**

Everything specified here is a control loop, and its two central claims are quantitative: that
marker-bracketed drain timing recovers the true rate where a DA1 round trip is 23× wrong, and that
byte-credit pacing recovers 6.3× of throughput that frame-count pacing discards on fast,
high-latency links. Both are simulatable today with a pty pair and a few hundred lines — no
network, no Electron, no Chromium, nothing the sandbox or the 9 GiB of free disk blocks. That
harness turns every threshold in §5 from an asserted number into a measured one, and it is the
only way to test the pathological cases (250 ms RTT, mid-frame disconnect, stale-epoch replies)
that will otherwise be discovered by a user on a train.

If only one line of core code changes first, make it `q=2` → `q=1` at `kitty.rs:130`. It is a
one-character edit that restores the error channel the entire repair path depends on, and it is
the difference between a screen that self-corrects and one that silently diverges.
