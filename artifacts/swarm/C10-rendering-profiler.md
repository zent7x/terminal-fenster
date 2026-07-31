# C10 — Rendering Profiler: stage model, probe placement, and output format

**Mission:** specify the profiler and its output format — per-stage timings (capture, IPC,
BGRA→RGB, deflate, base64, write, terminal ack), counters, a machine-readable JSONL schema,
and a human summary; and say exactly where in the *existing* code each probe point goes.

**Status of this document:** specification + measured justification. No core file was edited.
Every change this spec requires in commander-owned code is collected in §11 as a diff-shaped
description, per the file-ownership rule.

**Relationship to A10 (`A10-performance-plan.md`).** A10 owns the *methodology*: the shared
`mach_continuous_time`-based clock (§1.4/1.5), the 32-byte binary `Ev` record and lock-free
ring (§2.2/2.3), the t0→t5b latency chain (§3.1), the pacing definitions (§4.1), the
pre-registered budgets (§10). **This document does not restate or contradict any of it.**
C10 adds the three things A10 leaves open:

1. A **stage decomposition** finer than A10's `EncodeBegin`/`EncodeEnd` pair. A10 §9 says
   BGRA→RGB, base64 and zlib are "separately optimizable and separately regressible" but
   does not place probes for them. §1 below shows measured numbers proving that without
   that decomposition the profiler reports the *wrong stage* as the cost centre.
2. The **JSONL interchange schema** (§5) — A10's binary ring is the hot-path sink; JSONL is
   what the ring decodes *into*, what CI diffs, and what humans and `jq` read.
3. **Exact probe sites** by file, function and line in the code that exists today (§3).

---

## 0. Headline findings (measured, this machine, 2026-07-31)

| # | Finding | Evidence |
|---|---|---|
| **F1** | The number BlackGlass prints today as "encode ms" **excludes the single most expensive CPU stage.** `kitty::bgra_to_rgb` runs in `Renderer::on_frame` (`apps/cli/src/main.rs:837`); the timer starts in `Renderer::present` (`apps/cli/src/main.rs:857`). Measured 2.907 ms p50 invisible vs 0.684 ms p50 visible — the status bar under-reports frame CPU by **5.2×**. | §1.1, §1.2 |
| **F2** | The e2e-verified "0.74 ms encode" is **real but is not the frame cost.** It matches `deflate+base64+assemble` on a page-like frame (measured 0.684 ms p50). True conversion+encode is 3.569 ms p50 / 6.215 ms p99. | §1.2 |
| **F3** | Stage costs are **content-dependent by two orders of magnitude.** Same 2482×814 geometry: page-like `deflate` @L1 = 0.654 ms; incompressible (canvas/video) = 16.067 ms — one stage alone exceeds the entire 16.67 ms frame budget. A single aggregate `encode_ms` cannot express this. | §1.2 |
| **F4** | **Terminal ack via the kitty protocol is unsafe to enable today.** `kitty.rs:132` hardcodes `q=2`; turning it off makes the terminal reply `ESC_Gi=…;OK ESC\`, and `input::Decoder::step()` (`input.rs:176-192`) has no APC state — it decodes that reply as **12 synthetic keystrokes injected into the web page.** Empirically confirmed. | §7 |
| **F5** | **DSR (`CSI 5n`) gives ingest-ack timing with zero changes to the input decoder.** The reply `ESC[0n` already routes through `step_csi` → `Event::Unknown` → ignored at `main.rs:653`. Empirically confirmed. | §7 |
| **F6** | Per-frame copy costs are **real but not dominant**: Node `Buffer.concat` ×2 = 0.383 ms p50; Rust `MessageReader::next_message` under realistic 1 MiB socket reads = 0.766 ms p50. Together ~7 % of budget. Worth probing, not worth pre-emptively optimising. | §1.2 |

---

## 1. Measured justification for the stage list

A profiler that measures the wrong boundaries is worse than none: it produces confident,
wrong optimisation targets. So the stage list below is derived from measurement, not
intuition.

### 1.1 The blind spot in the code as it stands today

```
apps/cli/src/main.rs:828  fn on_frame(&mut self, payload: &[u8], status: &mut Status) {
apps/cli/src/main.rs:837      kitty::bgra_to_rgb(&pixels[..h.expected_payload()], &mut self.rgb);
                              ^^^^ 2.9 ms of work, no timer around it

apps/cli/src/main.rs:848  fn present(&mut self, status: &mut Status, cell_h: u32, rows: u16) {
apps/cli/src/main.rs:857      let t0 = Instant::now();
                              ^^^^ the timer starts HERE, after the expensive part
apps/cli/src/main.rs:861      if let Ok(stats) = kitty::encode_rgb_frame(...)
apps/cli/src/main.rs:881      status.last_encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
```

`status.last_encode_ms` is what the status bar renders (`main.rs:891`) and what the bounded
e2e run logs (`main.rs:468`). It is therefore the number the project currently reasons about,
and it is blind to `bgra_to_rgb`.

This is not a criticism of the code — `present` legitimately times what `present` does. It is
precisely the reason the profiler must define stages **across** function boundaries and
reconcile them against a measured whole (the `residual_ns` check, §5.7).

### 1.2 Stage costs, measured

Method: a release-mode Rust binary linking the shipping `bg-term` crate by path and calling
the real `kitty::bgra_to_rgb`, `b64::encode_into` and `kitty::encode_rgb_frame`. `deflate` is
private in `kitty.rs:70`, so it was reproduced byte-identically (same `ZlibEncoder`, same
`Vec::with_capacity(len/4)`, same `Compression::new(level)`). Geometry 2482×814 — the exact
viewport measured in Ghostty 1.3.1. Corpus is **synthetic** (see caveat below).

Build/run:
```
CARGO_TARGET_DIR=$SCRATCH/sbtarget cargo build --release   # in a scratchpad crate
$SCRATCH/sbtarget/release/stagebench
```

**Page-like content (white background, dark glyph runs, header band), `compress_level = 1`
— the level the shipping code uses (`main.rs:865`):**

```
bytes: bgra=8081392  rgb=6061044  deflate=53351  b64=71136  wire=71343   (bgra/wire = 113x)
  bgra_to_rgb                  p50=   2.907ms  p99=   5.318ms
  deflate                      p50=   0.654ms  p99=   1.210ms
  base64                       p50=   0.026ms  p99=   0.044ms
  assemble                     p50=   0.004ms  p99=   0.073ms
  TOTAL conv+encode_rgb_frame  p50=   3.569ms  p99=   6.215ms
```

**Same content, `compress_level = 6`:**

```
bytes: bgra=8081392  rgb=6061044  deflate=11575  b64=15436  wire=15517   (bgra/wire = 521x)
  bgra_to_rgb                  p50=   2.925ms  p99=   3.132ms
  deflate                      p50=   5.234ms  p99=   5.474ms
  base64                       p50=   0.006ms  p99=   0.007ms
  TOTAL conv+encode_rgb_frame  p50=   8.138ms  p99=   8.352ms
```

**Incompressible noise (stands in for canvas / video / photo), `compress_level = 1`:**

```
bytes: bgra=8081392  rgb=6061044  deflate=6066656  b64=8088876  wire=8106696  (bgra/wire = 1x)
  bgra_to_rgb                  p50=   2.972ms  p99=   3.106ms
  deflate                      p50=  16.067ms  p99=  16.138ms
  base64                       p50=   3.798ms  p99=   4.179ms
  TOTAL conv+encode_rgb_frame  p50=  23.054ms  p99=  24.259ms
```

**IPC copies:**

```
node v24.11.1, frame payload 8,081,392 B
  main.js:97  Buffer.concat([head, bitmap])       p50=0.164ms p99=0.729ms
  main.js:57  Buffer.concat([header, payload])    p50=0.157ms p99=0.429ms
  both concats per frame (excl. toBitmap)         p50=0.383ms p99=0.613ms

bg_proto::MessageReader (wire message 8,081,429 B)
  next_message, whole message pre-fed             p50=0.195ms p99=0.420ms
  fed in 1 MiB chunks (main.rs:455 sock_buf)      p50=0.766ms p99=1.292ms
```

**Method caveats, stated plainly.** (a) The corpus is synthetic; real page content will land
between the page-like and noise rows, and the profiler exists precisely so that guess gets
replaced by measurement against A10 §4.3's committed corpus. (b) The `assemble` row is
computed by subtracting independently-timed `deflate` and `base64` from a separate
`encode_rgb_frame` call, so its resolution is ~0.1 ms; at level 6 it measured *negative*
(−0.008 ms p50), which is method noise, not a result. Assembly is therefore reported as
"below this method's resolution" and is exactly why §3.3 places a **direct** probe inside
`encode_rgb_frame` rather than inferring it. (c) `write()` was not re-measured here; A10 §0.1
already has it (6.9 MB base64 payload at 1 KiB chunks: 10.854 ms p50 / 20.080 ms p99) and
that measurement stands.

### 1.3 What the numbers imply for the stage list

Adding up the page-like L1 path: 0.383 (Node copies) + 0.766 (Rust deframe) + 2.907 (BGRA→RGB)
+ 0.654 (deflate) + 0.026 (base64) ≈ 4.74 ms of CPU before a single byte reaches the terminal,
against A10 §0.1's ~10.9 ms p50 `write()`. So:

- **BGRA→RGB is the largest single CPU stage on realistic content** and is currently unmeasured.
  It must be its own stage.
- **base64 must be separate from deflate**: 0.026 ms on compressible content, 3.798 ms on
  incompressible — a 146× swing that averaging into "encode" destroys.
- **`compress_level` must be recorded per frame.** L1→L6 costs +4.58 ms of CPU and saves
  4.6× wire bytes. Whether that is a win depends entirely on `write_ns`, which is
  terminal-dependent. The profiler's job is to make that trade *measurable*, not to pick.
- **`damage_area_ratio` (A10 §4.1) is the lever these numbers point at**: every stage above
  scales with pixel count, and the engine already reports a dirty rect
  (`main.js:91-94` → `FrameHeader::dirty_*`, `bg-proto/src/lib.rs:23-26`) that the renderer
  currently ignores. The profiler must record both the reported dirty area and the area
  actually processed, so the gap is visible as a number rather than an intention.

---

## 2. The stage model

### 2.1 Canonical stages

Nine stages, each owned by exactly one process, each with an unambiguous begin and end. `seq`
(assigned at `main.js:88`, read back at `bg-proto/src/lib.rs:40`) is the join key across
processes — **it already exists in shipping code, so no protocol change is needed for
correlation.**

| # | Stage id | Process | Begins | Ends | Field |
|---|---|---|---|---|---|
| 1 | `capture` | engine | `'paint'` handler entry | `image.toBitmap()` returns | `capture_ns` |
| 2 | `frame_pack` | engine | after `toBitmap()` | header+bitmap concat done | `frame_pack_ns` |
| 3 | `ipc_write` | engine | `sendMessage` entry | `sock.write` returns (or `drain` fires) | `ipc_write_ns` |
| 4 | `ipc_wire` | both | `ipc_write` end (engine clock) | `read()` returns in core | `ipc_wire_ns` |
| 5 | `deframe` | core | `MessageReader::next_message` entry | message popped | `deframe_ns` |
| 6 | `convert` | core | before `bgra_to_rgb` | after `bgra_to_rgb` | `convert_ns` |
| 7 | `deflate` | core | before `deflate()` in `encode_rgb_frame` | after | `deflate_ns` |
| 8 | `base64` | core | before `b64::encode_into` | after | `base64_ns` |
| 9 | `assemble` | core | before the chunk loop | after the chunk loop | `assemble_ns` |
| 10 | `statusbar` | core | after backend encode | before `write_all` | `statusbar_ns` |
| 11 | `write` | core | before `stdout.write_all` | after `stdout.flush()` returns (**t4**) | `write_ns` |
| 12 | `term_ack` | core | `write` end | DSR/kitty reply read (**t5a**) | `term_ack_ns` |

Stage 4 `ipc_wire` is the only cross-process interval; it is valid **only** because A10 §1.4/1.5
establishes that Rust `clock::now_ns()` and Node `process.hrtime.bigint()` are the same
timeline by construction. If that equivalence test ever fails, `ipc_wire_ns` must be emitted
as `null`, not as a plausible-looking wrong number.

Stage 12 is sampled, not per-frame (§7.3).

For the Unicode backend, stages 7–9 are `null` and a single `halfblock_ns` replaces them
(`unicode::render_half_blocks`, `main.rs:876`).

### 2.2 EvKind extension

A10 §2.2 reserves discriminants 1–19. C10 extends from 20; **no value is redefined.**

```rust
// Extends A10 §2.2 EvKind. Values 1..=19 are A10's and are unchanged.
BitmapBegin      = 20,  // before image.toBitmap()
BitmapEnd        = 21,  // after   image.toBitmap()   -> capture_ns
FramePackEnd     = 22,  // after Buffer.concat([head, bitmap])
IpcWriteBegin    = 23,
IpcWriteEnd      = 24,  // sock.write returned true
IpcDrainEnd      = 25,  // 'drain' fired; backpressure resolved. aux = ns spent blocked
DeframeBegin     = 26,
DeframeEnd       = 27,
ConvertBegin     = 28,  // bgra_to_rgb                     <-- the stage F1 says is invisible
ConvertEnd       = 29,
DeflateBegin     = 30,
DeflateEnd       = 31,  // aux = deflated bytes
B64Begin         = 32,
B64End           = 33,  // aux = base64 bytes
AssembleBegin    = 34,
AssembleEnd      = 35,  // aux = chunk count
HalfblockEnd     = 36,  // aux = output bytes (unicode backend)
StatusBarEnd     = 37,
FlushEnd         = 38,  // t4, refined: after stdout.flush() returns, not write_all
AckRequestSent   = 39,  // DSR "CSI 5n" written
FrameTruncated   = 40,  // counter: short payload dropped
PresentSkipped   = 41,  // counter: present() ran with nothing dirty
CapsQueryBegin   = 42,  // aux = query id (see §3.5)
CapsQueryEnd     = 43,
EngineSpawn      = 44,
EngineConnected  = 45,
EngineReady      = 46,
```

`FrameCoalesced` (16) already exists in A10 and is reused; note §4 requires it to be emitted
on **both** sides, because the engine and the core coalesce independently and currently only
the engine counts it.

---

## 3. Probe placement — exact sites in the code that exists today

Line numbers are against the tree at commit `5215e1e` (`git log -1`). Every site was read
before being listed. "Insert before/after" describes a single statement inserted at that
point; no existing statement is moved unless the row says so.

### 3.1 `apps/engine/src/main.js` (309 lines)

| Line | Function | Probe | Insertion |
|---|---|---|---|
| 82 | `onPaint(_event, dirty, image)` | `PaintEvent` (A10 #7, **t2**) | First statement of the function, **before** the `if (!sock …) return` guard on line 83 — a paint that is dropped because the socket died is still a paint and must be counted. |
| 86 | `onPaint` | `BitmapBegin` / `BitmapEnd` | Bracket `const bitmap = image.toBitmap();`. This call is a full copy of `w*h*4` bytes and is the `capture` stage. |
| 85 | `onPaint` | `aux` for `BitmapEnd` | `size.width * size.height` from line 85, so `damage_area_ratio` can be computed against `dirty.width*dirty.height` (lines 91-94). |
| 96 | `onPaint` | `FrameCoalesced` (A10 #16) | The `if (pendingFrame) stats.coalesced++;` on line 96 is already the coalesce site — emit the event alongside the existing counter bump. Carry the **discarded** frame's seq in `aux`, not the new one. |
| 97 | `onPaint` | `FramePackEnd` | After `pendingFrame = Buffer.concat([head, bitmap]);`. Measured 0.164 ms p50 (§1.2). |
| 52 | `sendMessage(type, payload)` | `IpcWriteBegin` | First statement. Only when `type === T_FRAME`; events are low-rate and must not pollute the frame series. |
| 57 | `sendMessage` | (covered) | The `Buffer.concat([header, payload])` here is the second per-frame copy, 0.157 ms p50. It is inside `ipc_write` by construction; no separate probe — but if `ipc_write_ns` ever exceeds `write`-side expectations, this is the first suspect. |
| 70 | `flushFrame` | `IpcWriteEnd` | Immediately after `const ok = sendMessage(T_FRAME, frame);`, with `aux` = `frame.length` and a boolean `backpressure = !ok` recorded. |
| 75-78 | `flushFrame` | `IpcDrainEnd` | Inside the `sock.once('drain', …)` callback, `aux` = ns since `IpcWriteEnd`. **This is the socket-backpressure signal and there is currently no way to observe it at all.** |
| 69 | `flushFrame` | `QueueDepth` (A10 #17) | At `stats.sent++`. Depth here is 0 or 1 by construction (the design keeps at most one frame in flight, lines 42-49) — record it anyway so A10 §10's `max(queue_depth) ≤ 2` gate has a real series rather than an assumption. |
| 152 | `handleInput(cmd)` | `InIpcRecv` (A10 #4) | First statement. |
| 211/218/221 | `handleInput` | `InDispatched` (A10 #5, **t1**) | After the **last** `wc.sendInputEvent(...)` for the command — for `action:'press'` that is the `keyUp` on line 218, not the `keyDown` on line 211. Timestamping the first call would under-report dispatch by the char-loop on lines 213-215. |
| 268 | `attachReader`, `socket.on('data')` | (timestamp capture) | Capture `now()` once at the top of the `data` callback and attach it to every command parsed from that chunk (line 279). Do **not** call the clock inside the `for(;;)` loop — a chunk can carry many commands and the loop is the hot path. |
| 292 | `app.whenReady` | `EngineReady` | At the existing `sendEvent({t:'ready', …})`. Add `t_ns` and `proc_start_ns` to that event payload so the core can attribute cold start without a second channel. |
| 255 | `handleCommand`, `case 'stats'` | (extend) | The existing `stats` command is the natural pull-mode drain. Extend the reply with the §4 counters. |

**Engine-side sink.** The engine must not `JSON.stringify` on the paint path. Write fixed
32-byte records into a preallocated `Buffer` ring (A10 §2.2 layout, little-endian, so the
Rust decoder reads both files with one struct) and flush on a `setInterval(250)` — never in
`onPaint`. `BG_PROF` unset ⇒ the ring is never allocated and each probe is one
`if (PROF !== null)` against a module-level const.

### 3.2 `apps/cli/src/main.rs` (1039 lines)

| Line | Function | Probe | Insertion |
|---|---|---|---|
| 218 | `cmd_open` | `ProcStart` | First statement, plus `realtime_anchor()` (A10 §1.4) for the manifest. |
| 241 | `cmd_open` | (see §3.5) | `caps::detect(guard.fd(), 300)` — six sequential terminal queries, each with a 300 ms deadline. Worst case 1.8 s of cold start with no visibility today. |
| 402 | `Session::start` | `EngineSpawn` (46→44) | Immediately before `Command::new(&electron)`. |
| 430 | `Session::start` | `EngineConnected` | At the `Ok((s, _)) => break s` arm of the accept loop. `EngineConnected − EngineSpawn` is Electron cold start, isolated from page load. |
| 436 | `Session::send` | `InIpcSend` (A10 #3) | After `self.stream.write_all(&msg)` on line 438. |
| 460 | `Session::run` | (epoch) | `let started = Instant::now();` already exists — replace with `clock::now_ns()` so the JSONL timeline is the shared one, not a process-local `Instant`. |
| 496 | `Session::run` | `InStdinRead` (A10 #2 §3.1, **t0**) | Immediately after the `libc::read` on lines 489-495 returns `r > 0`. This is the canonical t0 and must be taken **before** `decoder.decode`. |
| 497 | `Session::run` | `InParsed` (A10 #2) | After `decoder.decode(&stdin_buf[..r as usize])` returns, once per read, `aux` = event count. |
| 520 | `Session::run` | `FrameIpcRecv` (A10 #9) | After `self.stream.read(&mut sock_buf)` returns `Ok(n)`, `aux` = `n`. Note this fires once per **socket read**, not per frame — an 8 MB frame arrives in ~8 reads of the 1 MiB `sock_buf` (line 455). The decoder attributes the *last* `FrameIpcRecv` before a `DeframeEnd` to that frame. |
| 532 | `Session::run` | `DeframeBegin`/`DeframeEnd` | Bracket the `while let Some(msg) = reader.next_message()` condition. Measured 0.766 ms p50 in the realistic chunked path (§1.2), driven by `to_vec()` at `bg-proto:90` and `drain()` at `bg-proto:91`. |
| 535 | `Session::run` | (join key) | `render.on_frame(&msg.payload, &mut status)` — parse `FrameHeader` (already done on line 538, but only on the first frame) and hoist `h.seq` so every probe from here on carries it. Today the header is parsed twice for frame 1 and zero times afterwards. |
| 829 | `Renderer::on_frame` | `FrameCoalesced` (A10 #16) | At function entry, **if `self.dirty` is already true** — that means the previous frame was converted but never presented, i.e. the core coalesced it. The engine's `stats.coalesced` (`main.js:96`) does not see this; it is a second, independent coalesce point and is currently invisible. |
| 834-836 | `Renderer::on_frame` | `FrameTruncated` | At the `if pixels.len() < h.expected_payload() { return; }` early return. **This is a silent drop today** — no counter, no log. A truncated frame means a framing bug and must be loud. |
| 837 | `Renderer::on_frame` | `ConvertBegin`/`ConvertEnd` | Bracket `kitty::bgra_to_rgb(...)`. **The F1 fix.** `aux` on `ConvertEnd` = `self.rgb.len()`. |
| 849-851 | `Renderer::present` | `PresentSkipped` | At the `if !self.dirty || self.rgb.is_empty() { return; }` early return. At the 16 ms poll timeout (line 478) this fires most iterations; it is the idle-path counter and A10 §6's idle runs need it. |
| 857 | `Renderer::present` | `EncodeBegin` (A10 #10) | Keep the existing `let t0 = Instant::now();` position but change its meaning: it is now the *backend encode* boundary, and `frame_total_ns` is measured from `ConvertBegin`. Do not silently redefine `last_encode_ms` — rename it `backend_encode_ms` so nothing downstream reads a changed number under an old name. |
| 861 | `Renderer::present` | (see §3.3) | `kitty::encode_rgb_frame` — sub-stages are returned via `EncodeStats`, not via a global sink. |
| 876 | `Renderer::present` | `HalfblockEnd` | After `unicode::render_half_blocks(...)`, `aux` = `s.len()`. |
| 881 | `Renderer::present` | `EncodeEnd` (A10 #11, **t3**) | At the existing `status.last_encode_ms = …`. |
| 885-897 | `Renderer::present` | `StatusBarEnd` | After the status-bar bytes are appended (line 897). Three `format!` allocations plus two `sanitize_for_terminal` passes run **every presented frame**; expected sub-0.05 ms but it is on the critical path and should not be assumed free. |
| 899-900 | `Renderer::present` | `WriteBegin` (A10 #12) | Immediately before `stdout.write_all(&self.out)`, `aux` = `self.out.len()`. |
| 901 | `Renderer::present` | `WriteEnd` (A10 #13) / `FlushEnd` (**t4**) | `WriteEnd` after `write_all` returns; `FlushEnd` after `flush()` returns. **t4 is `FlushEnd`, not `WriteEnd`** — `std::io::Stdout` is `LineWriter`-backed, so `write_all` can return with bytes still buffered and a t4 taken at `WriteEnd` would be a lie. |
| 901 | `Renderer::present` | `AckRequestSent` | After `FlushEnd`, on sampled frames only (§7.3): write `\x1b[5n` and flush. |
| 653 | `Session::handle_event` | `TermAckRecv` (A10 #14, **t5a**) | The `Event::Unknown(_)` arm. Match `seq == b"\x1b[0n"` and stamp the ack; everything else stays ignored exactly as today. |
| 32 | `log_line` | **do not reuse** | `log_line` opens the file with `OpenOptions::new().create(true).append(true).open(path)` on **every call** (line 38) — an `open`+`write`+`close` per line. At 60 fps × 12 stages that is 720 syscall triples per second and would dominate what it measures. The profiler needs its own long-lived fd. |

### 3.3 `crates/bg-term/src/kitty.rs` (396 lines)

`bg-term` is a dependency-free library crate (`crates/bg-term/Cargo.toml`: `libc`, `flate2`).
Do **not** give it a tracing dependency or a global sink. Instead return the timings through
the struct that already carries the byte counts.

| Line | Function | Probe | Insertion |
|---|---|---|---|
| 111-115 | `encode_rgb_frame` | `deflate_ns` | Bracket the `if compress_level > 0 { (deflate(rgb, compress_level)?, true) } else { … }` expression. |
| 118-119 | `encode_rgb_frame` | `base64_ns` | Bracket `b64::encode_into(&payload, &mut b64buf)`. |
| 121-156 | `encode_rgb_frame` | `assemble_ns` | Bracket from `let chunks: Vec<&[u8]> = …` (121) through the end of the `for` loop (156). |
| 192-199 | `EncodeStats` | (extend) | Add `deflate_ns: u64`, `base64_ns: u64`, `assemble_ns: u64`. The struct is `Copy` and already carries `raw_bytes`/`deflated_bytes`/`wire_bytes`/`chunks`; three `u64`s keep it 64 bytes and keep the crate testable in isolation. Its existing unit tests (`kitty.rs:288-359`) then extend naturally to assert the timings are populated and monotone. |
| 40 | `bgra_to_rgb` | **none** | Pure function; timed at the call site (`main.rs:837`). Putting a clock inside it would tax every caller including tests. |
| 54 | `bgra_rect_to_rgb` | **none** | Same. Not yet called from `main.rs`; when damage-driven updates land, the same call-site rule applies. |
| 132 | `encode_rgb_frame` | **see §7** | `out.extend_from_slice(b"a=T,f=24,t=d,q=2");` — the hardcoded `q=2`. |

The clock function itself must come from A10 §1.4 (`crates/bg-bench/src/clock.rs`,
`now_ns()`, measured 8.36–8.50 ns/call). Six extra calls per frame inside `encode_rgb_frame`
is ~51 ns against a 16,670,000 ns budget: 0.0003 %.

### 3.4 `crates/bg-proto/src/lib.rs` (284 lines)

No probes inside this crate. `next_message` (line 81) is timed from `main.rs:532` (§3.2).
Two observations the profiler will surface and that belong in the report rather than in a
patch here:

- `bg-proto:90` `self.buf[5..5+len].to_vec()` copies the entire 8 MB frame.
- `bg-proto:91` `self.buf.drain(..5+len)` memmoves whatever remains.

Together with the `extend_from_slice` growth in `feed` (line 73), these account for the
0.766 ms p50 measured in §1.2. A zero-copy `next_message_ref()` returning a borrowed slice is
the obvious fix; it is a core change and is described in §11, not made here.

### 3.5 `crates/bg-term/src/caps.rs` (373 lines) — cold-start attribution

`detect` (line 128) issues six sequential terminal queries, each via `query` (line 116) with a
`deadline_ms` of 300 (passed from `main.rs:241`). On a terminal that does not answer, each
one burns the full deadline: Apple Terminal 465 gives no reply to `CSI 16t`
(measured, per the project brief), so its detect phase alone can cost hundreds of ms.

Place `CapsQueryBegin`/`CapsQueryEnd` around the `read_reply` call inside `query`
(`caps.rs:120`), with `aux` = a small query id so the JSONL can name which probe was slow:

| aux | Query | Site |
|---|---|---|
| 1 | kitty graphics `a=q` | `caps.rs:143` |
| 2 | Primary DA `CSI c` | `caps.rs:157` |
| 3 | kitty keyboard `CSI ?u` | `caps.rs:163` |
| 4 | window px `CSI 14t` | `caps.rs:168` |
| 5 | cell px `CSI 16t` | `caps.rs:175` |
| 6 | DECRQM 1016 | `caps.rs:183` |

Also record whether each query **timed out** vs **replied**, since a timeout is a
capability-detection cost that scales with `deadline_ms`, and a reply is not. That
distinction is what tells the commander whether shortening the deadline is safe.

### 3.6 Files with no probes

`crates/bg-term/src/tty.rs`, `b64.rs`, `unicode.rs`, `input.rs` take no in-crate probes.
`b64::encode_into` and `unicode::render_half_blocks` are timed at their call sites
(`kitty.rs:119`, `main.rs:876`). `input::Decoder::decode` is timed at `main.rs:497`.
`tty.rs` is lifecycle, not steady state — its `acquire` (line 84) and `Drop` (line 176) are
covered by the run manifest's start/end timestamps.

---

## 4. Counters

Counters are monotone `u64`s, snapshotted into every `summary` record and into the `stats`
event (`main.js:255`). Three already exist and are kept; the rest are new.

| Counter | Owner | Source / site | Exists? |
|---|---|---|---|
| `paints_produced` | engine | `stats.produced`, `main.js:84` | **yes** |
| `frames_sent` | engine | `stats.sent`, `main.js:69` | **yes** |
| `coalesced_engine` | engine | `stats.coalesced`, `main.js:96` | **yes** |
| `backpressure_events` | engine | `flushFrame` took the `sock.once('drain')` path, `main.js:75` | no |
| `backpressure_ns_total` | engine | sum of `IpcDrainEnd − IpcWriteEnd` | no |
| `frames_received` | core | messages of type `T_FRAME` popped, `main.rs:534` | no |
| `frames_truncated` | core | the silent drop at `main.rs:835` | no |
| `frames_bad_header` | core | `FrameHeader::parse` returned `None`, `main.rs:829` | no |
| `coalesced_core` | core | `on_frame` entered with `self.dirty` already true | no |
| `frames_presented` | core | `present` reached `write_all`, `main.rs:900` | no |
| `presents_skipped` | core | early return at `main.rs:849` | no |
| `bytes_to_terminal` | core | sum of `self.out.len()` at `main.rs:900` | no |
| `bytes_from_engine` | core | sum of `n` from `main.rs:520` | no |
| `write_partial` | core | `write_all` internally looped (detect via a counting writer) | no |
| `input_events_decoded` | core | length of the `decoder.decode` result, `main.rs:497` | no |
| `input_bytes_read` | core | `r` from `main.rs:496` | no |
| `escape_flushes` | core | `flush_pending_escape` returned `Some`, `main.rs:509` | no |
| `unknown_sequences` | core | `Event::Unknown` at `main.rs:653` (excluding matched acks) | no |
| `ack_requests` / `ack_replies` / `ack_timeouts` | core | §7.3 | no |
| `prof_records_dropped` | both | ring overflow (A10 §2.2) | no |

Two derived identities the decoder must assert (§5.7):

```
paints_produced == frames_sent + coalesced_engine + frames_in_flight_at_exit
frames_received == frames_presented + coalesced_core + frames_truncated + frames_bad_header
                                    + frames_pending_at_exit
```

If either fails, frames are going missing somewhere unaccounted for, and the run is marked
`"integrity": "FAIL"` rather than being quietly reported.

---

## 5. JSONL schema v1

### 5.1 Why JSONL, and where it sits

A10's binary ring is the hot-path sink and stays. JSONL is the **decoded interchange
format**: what `bgprof decode` writes, what CI diffs and archives, what `jq` reads, and what
a human can inspect without a tool. No JSON is ever produced on a frame path.

Design rules, and the reason for each:

1. **One line per frame, not one line per probe.** A frame's twelve stages live on one line,
   so no join is needed to answer "what did frame 4211 cost". At 60 fps a one-hour run is
   216 000 lines (~55 MB), versus ~2.6 M lines for per-probe records.
2. **Every key is always present.** A stage that did not run is `null`, never omitted.
   Omission and "did not happen" are different facts and a consumer cannot distinguish them
   after the fact.
3. **All durations are integer nanoseconds, suffixed `_ns`.** No floats, no milliseconds, no
   unit ambiguity. Humans get milliseconds in the §6 report, machines never do.
4. **All absolute timestamps are `t_*_ns` on A10 §1.4's clock**, and the manifest carries the
   single wall-clock anchor. Nothing else converts to wall time.
5. **Records are self-describing**: every line carries `"r"` (record type) and the file's
   first line carries `"schema"`. A consumer can process a truncated file — which is what a
   crashed run produces, and crashed runs are exactly the ones worth reading.

### 5.2 File layout

```
<outdir>/<run-id>/
  manifest.json          # convenience copy of the meta record
  core.jsonl             # emitted by apps/cli   (pid of blackglass)
  engine.jsonl           # emitted by apps/engine (pid of the Electron main process)
  merged.jsonl           # produced by `bgprof merge`, joined on seq
  report.txt             # §6 human summary
```

`<run-id>` = `<unix-ms>-<git-short-sha>-<label>`. Two files rather than one because the two
processes must never share a lock; they are merged offline on `seq`, exactly as A10 §2.1
prescribes.

### 5.3 Record type `meta` (first line of every file, exactly one)

```json
{"r":"meta","schema":"blackglass.prof/1","v":1,
 "run_id":"1753996800123-5215e1e-pacing",
 "role":"core",
 "pid":48231,
 "git_sha":"5215e1e","dirty_tree":true,
 "argv":["blackglass","open","https://example.com"],
 "t_anchor_ns":184523119884321,"rt_anchor_ns":1753996800123000000,
 "timebase":{"numer":125,"denom":3},
 "clock_agreement_ns":312,
 "host":{"os":"macOS 26.1","arch":"arm64","cpu":"Apple M4","cores_p":4,"cores_e":6},
 "engine":{"electron":"43.2.0","chrome":"150.0.0.0"},
 "terminal":{"program":"ghostty","version":"1.3.1","term":"xterm-ghostty",
             "xtversion":"ghostty 1.3.1","da1":"?62;22;52c",
             "kitty_graphics":true,"sixel":false,"kitty_keyboard":true,
             "sgr_pixel_mouse":true,"synchronized_output":null,
             "cell_px":[17,37],"window_px":[2482,851],"cells":[146,23]},
 "backend":"kitty","compress_level":1,"chunk_bytes":4096,
 "viewport_px":[2482,814],
 "sampling":{"stages":"all","ack_every_n":60,"ring_capacity":262144},
 "budgets_ref":"A10-performance-plan.md#10"}
```

`clock_agreement_ns` is the measured Rust↔Node clock delta (A10 §12 item 1). If it is
`null` or `> 1000`, every `ipc_wire_ns` in the file must be `null` (§2.1).
`synchronized_output` is `null` until the DECRQM 2026 probe runs — A10 §3.5(c) flags it
UNVERIFIED and this schema must be able to say "unknown" rather than guess `false`.

### 5.4 Record type `frame` (one per received frame)

```json
{"r":"frame","seq":4211,
 "t_paint_ns":184529110000000,
 "t_flush_ns":184529124880000,
 "geom":{"w":2482,"h":814},
 "dirty":{"x":0,"y":0,"w":2482,"h":814},
 "damage_area_ratio":1.0,
 "processed_area_ratio":1.0,
 "backend":"kitty","compress_level":1,
 "bytes":{"bgra":8081392,"rgb":6061044,"deflated":53351,"b64":71136,
          "wire":71343,"out_total":71412},
 "chunks":18,
 "stages_ns":{"capture":1180000,"frame_pack":164000,"ipc_write":210000,
              "ipc_wire":940000,"deframe":766000,"convert":2907000,
              "deflate":654000,"base64":26000,"assemble":4000,
              "halfblock":null,"statusbar":31000,"write":10854000,
              "term_ack":null},
 "frame_total_ns":17736000,
 "residual_ns":-1,
 "coalesced_engine":false,"coalesced_core":false,
 "backpressure":false,"queue_depth":1,
 "outcome":"presented"}
```

Field notes:

| Field | Type | Meaning |
|---|---|---|
| `seq` | u32 | From `main.js:88` / `FrameHeader.seq`. The cross-process join key. |
| `t_paint_ns` | u64 \| null | A10 t2. `null` if the frame's engine record is missing (merge failure). |
| `t_flush_ns` | u64 \| null | A10 t4, taken after `stdout.flush()` returns (`main.rs:901`). |
| `dirty` | object | Straight from `FrameHeader.dirty_*` (`bg-proto:23-26`), which the engine fills at `main.js:91-94`. |
| `damage_area_ratio` | f64 | `dirty.w*dirty.h / (w*h)` — what Chromium says changed. |
| `processed_area_ratio` | f64 | What we actually converted and encoded. **Today this is always `1.0`** because `main.rs:837` converts the whole buffer. The gap between these two numbers is the size of the optimisation A10 §0.1 says is load-bearing, expressed as a measurement. |
| `bytes.out_total` | usize | `self.out.len()` at `main.rs:900` — includes the `ESC[H` on line 855 and the status bar, so it exceeds `bytes.wire`. Both are reported; conflating them hides the status bar's cost. |
| `stages_ns.*` | u64 \| null | §2.1. `null` = stage did not run for this frame (e.g. `deflate` when `compress_level == 0`, `halfblock` on the kitty backend, `term_ack` on a non-sampled frame). |
| `frame_total_ns` | u64 | `t_flush_ns − t_paint_ns` when both are known; otherwise `FlushEnd − ConvertBegin` and `"total_basis":"core_only"` is added. |
| `residual_ns` | i64 | `frame_total_ns − Σ(non-null stages_ns)`. See §5.7. |
| `queue_depth` | u8 | A10 §4.1. |
| `outcome` | enum | `"presented"` \| `"coalesced_engine"` \| `"coalesced_core"` \| `"truncated"` \| `"bad_header"` \| `"dropped_socket_closed"`. Exactly one value; every received frame gets a record even if it was never drawn. **A frame that is dropped and not recorded is how a profiler lies about p99.** |

### 5.5 Record type `input` (one per decoded input event)

```json
{"r":"input","iseq":903,"kind":"key","detail":"Char(j)",
 "t_stdin_read_ns":184529100000000,
 "stages_ns":{"decode":18000,"ipc_send":41000,"ipc_wire":880000,"dispatch":121000},
 "t_dispatched_ns":184529102060000,
 "caused_seq":4211,
 "latency_t4_t0_ns":24880000}
```

`caused_seq` is the first frame whose `t_paint_ns` follows `t_dispatched_ns` **and** whose
renderer pixel-stamp (A10 §3.4) matches — never by time proximity alone, which is guesswork.
When the pixel-stamp harness is not running, `caused_seq` and `latency_t4_t0_ns` are `null`.
Emitting a plausible-looking attributed latency without the stamp would be exactly the
fabrication A10 §3.2 warns against.

### 5.6 Record type `note` (sparse; lifecycle and anomalies)

```json
{"r":"note","t_ns":184523119884321,"lvl":"info","tag":"engine_connected",
 "data":{"spawn_to_connect_ns":212000000}}
{"r":"note","t_ns":184523120004000,"lvl":"info","tag":"caps_query",
 "data":{"id":5,"name":"cell_px","replied":false,"elapsed_ns":300112000}}
{"r":"note","t_ns":184529500000000,"lvl":"warn","tag":"backpressure",
 "data":{"seq":4218,"blocked_ns":31400000}}
{"r":"note","t_ns":184530100000000,"lvl":"error","tag":"frame_truncated",
 "data":{"seq":4231,"got":4096,"expected":8081392}}
{"r":"note","t_ns":184531000000000,"lvl":"error","tag":"prof_records_dropped",
 "data":{"count":1284}}
```

`tag` is a closed vocabulary: `proc_start`, `caps_query`, `engine_spawn`,
`engine_connected`, `engine_ready`, `navigate`, `resize`, `backpressure`, `frame_truncated`,
`bad_header`, `unknown_sequence`, `ack_timeout`, `crash`, `prof_records_dropped`,
`clock_disagreement`, `shutdown`. A closed vocabulary is what makes the notes greppable and
alertable instead of a free-text log.

### 5.7 Invariants the decoder must enforce

The decoder refuses to emit a `summary` and marks `"integrity":"FAIL"` if any of these break:

| # | Invariant | Why |
|---|---|---|
| I1 | `meta` is line 1 and appears exactly once | Truncated-file recovery depends on it |
| I2 | `seq` is strictly increasing per file | Out-of-order frames mean a framing bug |
| I3 | Every non-null `stages_ns` value is `≥ 0` | A negative duration means the clock went backwards — with `mach_continuous_time` that is impossible, so it means a probe pair is crossed |
| I4 | `\|residual_ns\| ≤ 0.10 × frame_total_ns` | **The self-check.** A large positive residual means an unprobed stage exists (this is precisely how F1 would have been caught automatically); a large negative residual means stages overlap and are being double-counted |
| I5 | `bytes.rgb == bytes.bgra / 4 * 3` | Pins the BGRA→RGB contract asserted at `kitty.rs:108` |
| I6 | `bytes.b64 == ceil(bytes.deflated/3)*4` | Pins base64 (`b64.rs:12`) |
| I7 | `bytes.wire ≥ bytes.b64` and `chunks == ceil(bytes.b64 / 4096)` | Pins the chunking contract (`kitty.rs:26`, `kitty.rs:121`) |
| I8 | The two counter identities in §4 balance | Frames are not vanishing |
| I9 | `prof_records_dropped == 0` | A run with dropped records cannot produce an honest p99 (A10 §2.2) |
| I10 | `sleep_skew_ns == 0` at end of run (A10 §1.4) | The machine slept; discard the run |

I4 deserves emphasis. It is the one invariant that catches *unknown unknowns*: any future
refactor that moves work into an unprobed function shows up immediately as a residual blowout,
rather than as a mysteriously slow product with a profiler that insists everything is fine.

### 5.8 Record type `summary` (last line)

```json
{"r":"summary","v":1,"integrity":"OK",
 "wall_ns":60002114000,
 "frames":{"received":3611,"presented":3402,"coalesced_engine":142,
           "coalesced_core":61,"truncated":0,"bad_header":0},
 "fps":{"paint":60.2,"present":56.7,"coalesce_ratio":0.058},
 "stages_ms":{
   "convert":{"p50":2.907,"p95":4.410,"p99":5.318,"max":9.02,"share":0.164},
   "deflate":{"p50":0.654,"p95":1.011,"p99":1.210,"max":3.31,"share":0.037},
   "base64":{"p50":0.026,"p95":0.038,"p99":0.044,"max":0.19,"share":0.001},
   "write":{"p50":10.854,"p95":16.637,"p99":20.080,"max":41.2,"share":0.612}},
 "frame_total_ms":{"p50":17.736,"p95":24.9,"p99":31.4,"sd":4.81},
 "interframe_gap_ms":{"p50":17.6,"p95":24.1,"p99":33.0,"sd":5.2},
 "bytes_per_frame":{"p50":71343,"p95":118210,"p99":214880},
 "bytes_per_second":4045000,
 "damage_area_ratio":{"p50":1.0,"p95":1.0},
 "processed_area_ratio":{"p50":1.0,"p95":1.0},
 "residual_ns":{"p50":-1000,"p99":41000,"max_abs_share":0.021},
 "term_ack_ms":{"n":57,"p50":1.9,"p99":4.4,"timeouts":0},
 "counters":{"backpressure_events":3,"presents_skipped":1841,
             "unknown_sequences":0,"prof_records_dropped":0},
 "drop_attribution":{"pty_bound":128,"encoder_bound":9,"engine_bound":5,"ipc_bound":0},
 "budget_verdicts":[
   {"metric":"write_ms_p99","value":20.080,"target":16,"hard_fail":33,"verdict":"WARN"},
   {"metric":"present_fps","value":56.7,"target":55,"hard_fail":30,"verdict":"PASS"},
   {"metric":"max_queue_depth","value":1,"target":2,"hard_fail":3,"verdict":"PASS"}],
 "verdict":"WARN"}
```

`share` is the stage's fraction of summed frame time — the single most useful field for
deciding what to optimise, and the field that today's `last_encode_ms` makes impossible to
compute. `drop_attribution` implements A10 §4.2's table directly. `budget_verdicts` is
generated from A10 §10 by reference, not by a copy that can drift.

**Percentiles are always reported with p50/p95/p99/max and never as a bare mean**, per A10
§8.1. The schema has no `mean` field at all, so nobody can accidentally report one.

---

## 6. Human summary

`bgprof report <run-id>` renders `merged.jsonl` to fixed-width text. Not a dashboard — a
thing that pastes into a PR comment and into this repo's `artifacts/`.

```
BlackGlass render profile   run 1753996800123-5215e1e-pacing        integrity: OK
Ghostty 1.3.1 / kitty backend / 2482x814 / zlib L1 / chunk 4096      60.0 s wall

FRAMES   received 3611   presented 3402   coalesced 203 (5.6%)   truncated 0
FPS      paint 60.2      present 56.7     interframe gap p50 17.6 ms  sd 5.2 ms

STAGE            p50       p95       p99       max     share   budget
  capture       1.180     1.510     2.240     6.11      6.7%   -
  frame_pack    0.164     0.402     0.729     1.98      0.9%   -
  ipc_write     0.210     0.388     0.910     4.02      1.2%   -
  ipc_wire      0.940     1.702     2.880     9.44      5.3%   > 2 ms p99 => IPC-bound
  deframe       0.766     1.104     1.292     3.87      4.3%   -
  convert       2.907     4.410     5.318     9.02     16.4%   *** largest CPU stage
  deflate       0.654     1.011     1.210     3.31      3.7%   -
  base64        0.026     0.038     0.044     0.19      0.1%   -
  assemble      0.004     0.031     0.073     0.42      0.0%   -
  statusbar     0.031     0.044     0.061     0.30      0.2%   -
  write        10.854    16.637    20.080    41.20     61.2%   *** target 16 / fail 33 -> WARN
  ---------------------------------------------------------------------
  frame total  17.736    24.900    31.400    48.90    100.0%
  residual     -0.001     0.019     0.041     0.09      0.2%   OK (<10%)

BYTES    per frame p50 71.3 KB  p99 209.8 KB     4.05 MB/s to terminal
         damage ratio p50 1.00   processed ratio p50 1.00
         >>> the engine reports full-surface damage on this corpus; partial-damage
             encoding would not help here. Re-check on a corpus with local updates.

TERMINAL ingest ack (DSR CSI 5n, n=57): p50 1.9 ms  p99 4.4 ms  timeouts 0

DROPS    pty-bound 128   encoder-bound 9   engine-bound 5   ipc-bound 0

VERDICT  WARN  (write_ms p99 = 20.080 exceeds the 16 ms target; below the 33 ms hard fail)

Controllable latency (t4 - t0) is 21.4 ms p50 / 34.9 ms p99. Terminal present latency
is not observable from inside the pipe; it is bounded below by the ingest ACK round
trip (1.9 ms p50, above) and above by external capture, which was not run. End-to-end
input-to-photon is therefore in [23.3 ms, unbounded-above]. Do not quote t4 - t0 as an
input-to-photon number.
```

Two properties of that output are non-negotiable. **The `residual` row is always printed**,
so a reader can immediately see whether the breakdown accounts for the whole frame. **The
closing paragraph is emitted verbatim** from A10 §3.2 — it is the sentence that stops a t4−t0
number from being quoted as end-to-end latency.

Sub-commands: `bgprof report` (above), `bgprof merge core.jsonl engine.jsonl`,
`bgprof diff <run-a> <run-b>` (Mann–Whitney U + bootstrap p99 CI per A10 §8.2, refusing to
print a verdict without a fresh noise-floor run), `bgprof frames --slowest 20` (worst frames
with full stage breakdown), `bgprof tail` (live one-line-per-second view for interactive use).

---

## 7. Terminal ack — the one stage that needs a code decision

### 7.1 What "ack" can and cannot mean

`term_ack_ns = t5a − t4` is the terminal's **ingest and parse** time. It is a *lower bound*
on present latency, never present latency itself (A10 §3.2, §3.5). The report must never
label it "terminal render time".

### 7.2 The kitty-response path is unsafe today — measured

`kitty.rs:132` writes `a=T,f=24,t=d,q=2` on every frame. Per the kitty graphics protocol
specification, `q=1` suppresses `OK` responses and `q=2` suppresses errors as well
(https://sw.kovidgoyal.net/kitty/graphics-protocol/). So no response arrives today, and A10
§3.5(a) proposes sending every 60th frame with `q=0` to harvest an ack.

That proposal is correct in principle and **unsafe to implement against the current input
decoder.** `input::Decoder::step()` (`input.rs:161-194`) dispatches on `ESC [` (CSI, line 177)
and `ESC O` (SS3, line 178); everything else falls to the `_ =>` arm on line 179, which
decodes `ESC <char>` as Alt+char. There is no APC or DCS state.

Measured, by feeding the shipping decoder the exact bytes a conforming terminal would send:

```
--- kitty OK    ESC _ G i=1000;OK ESC \
    KEY code=Char('_') mods={alt:true}   <-- INJECTED INTO PAGE
    KEY code=Char('G') text="G"          <-- INJECTED INTO PAGE
    KEY code=Char('i') text="i"          <-- INJECTED INTO PAGE
    ... 12 key events total, all forwarded to webContents.sendInputEvent
```

A 12-keystroke burst into the focused element of the page, once per acked frame. On a page
with a text input, that is visible corruption of user data.

The same is true of an **error** response, which `q=1` does *not* suppress:

```
--- kitty ERR   ESC _ G i=1000;EBADF:bad ESC \
    ... 18 key events, all forwarded
```

This is worth flagging beyond profiling: it means the input decoder currently has no defence
against any unsolicited APC, DCS or OSC reply from the terminal. `q=2` is load-bearing
today for correctness, not just for performance.

### 7.3 The DSR path works today with zero decoder changes — measured

Request `ESC [ 5 n`; reply `ESC [ 0 n` (A10 §3.5(b)). Because terminals process their input
stream in order, a DSR issued immediately after the frame bytes cannot be answered until the
frame bytes have been consumed.

Fed to the shipping decoder:

```
--- DSR reply  ESC [ 0 n
    UNKNOWN "\u{1b}[0n"   <-- ignored by handle_event()
    pending bytes still buffered: 0
```

`step_csi` (`input.rs:248`) finds the final byte `n` (0x6e), fails to match paste, focus,
SGR-mouse and kitty-key, then `decode_legacy_csi` (`input.rs:436`) returns `None` for final
byte `n` (line 462), so line 303 emits `Event::Unknown`. `Session::handle_event`
(`main.rs:653`) already discards `Event::Unknown(_)`. **The reply is consumed safely by the
code as it stands.**

Specified ack protocol:

| Aspect | Value | Rationale |
|---|---|---|
| Mechanism | DSR `ESC[5n` | Works on Ghostty, iTerm2 and Apple Terminal; needs no `input.rs` change |
| Cadence | every 60th presented frame (~1 Hz) | Ack cost is one round trip; per-frame would perturb the thing measured |
| Emission site | `main.rs:901`, immediately after `stdout.flush()` returns | Must follow the frame bytes in stream order or the ordering argument collapses |
| Stamp site | `main.rs:653`, `Event::Unknown` arm, matching `b"\x1b[0n"` | Already reached; add a match, change nothing else |
| Timeout | 250 ms → `ack_timeouts += 1`, note `tag:"ack_timeout"`, `term_ack_ns: null` | A missing ack is data, not an error |
| Under tmux | disable | tmux answers DSR itself; the number would describe tmux, not the terminal |
| kitty `q=0` variant | **blocked** on an APC/DCS skip state in `input.rs` (§11.4) | See §7.2 |

One caveat to record in the report: with a 250 ms timeout and 1 Hz cadence, an ack that
arrives late is indistinguishable from one that arrives for the *next* sampled frame. The
sampler therefore refuses to issue a new DSR while one is outstanding, and counts the skip.

---

## 8. Overhead and gating

| Property | Requirement | Basis |
|---|---|---|
| Enable | `BG_PROF=1` env var, checked once at startup into a module-level `Option<&'static Prof>` | A10 §2.1: the measured build must be bit-identical to the shipped build. **Not** a cargo feature. |
| Disabled cost | one predictable, always-not-taken branch per probe | ~0.3 ns each; 40 probes/frame = 12 ns of a 16.67 ms budget |
| Enabled cost | ≤ 0.5 % of frame budget | 40 × `now_ns()` at 8.5 ns = 340 ns = 0.002 % (A10 §1.4 measured) |
| Hot path | no allocation, no formatting, no locking, no syscall | A10 §2.1 |
| Sink | preallocated ring, flushed by a dedicated 250 ms thread | A10 §2.3 |
| Ring overflow | drop oldest, increment `prof_records_dropped`, **fail the run** (I9) | A silent drop produces a lying p99 |
| Log file | separate fd from `BLACKGLASS_LOG`; never reuse `log_line` (`main.rs:32`) | It reopens the file per call |
| stdout | the profiler **never** writes to stdout | stdout is the graphics channel (`main.rs:29-31`) |

CI gating follows A10 §8.4: per-PR runs only the hermetic stage microbenchmarks (`convert`,
`deflate`, `base64`, `assemble` — no PTY, no engine), which are stable enough to gate on;
full-pipeline numbers are nightly with a 7-day rolling p99 comparison.

---

## 9. Tests the profiler itself must pass

A profiler with no tests is an opinion. These are cheap and all are hermetic except T7.

| # | Test | Asserts |
|---|---|---|
| T1 | Synthetic 4×4 frame through the whole pipe with `BG_PROF=1` | Exactly one `frame` record, `outcome:"presented"`, every stage non-null |
| T2 | Known-cost stage: inject a 5 ms sleep into `convert` behind a test flag | `stages_ns.convert ∈ [5.0, 5.5] ms` — proves probes measure the thing they name |
| T3 | Truncated payload (extend `main.rs:1014`'s existing test) | `outcome:"truncated"`, `frames_truncated == 1`, and a `frame` record **exists** |
| T4 | Feed the decoder a JSONL file with a deliberately unprobed 3 ms stage | I4 fires; `integrity:"FAIL"` |
| T5 | Byte identities on random frames | I5, I6, I7 hold for 1000 random geometries |
| T6 | Counter identities with induced coalescing | I8 balances |
| T7 | Rust/Node clock agreement (A10 §12 item 1) | `\|Δ\| < 1 µs`; if it fails, `ipc_wire_ns` is `null` everywhere |
| T8 | `BG_PROF` unset | Zero records written, no file created, and `bgprof report` says so rather than printing an empty table |
| T9 | Decode a truncated `.jsonl` (killed mid-run) | `meta` + partial frames parse; `summary` absent; no panic |
| T10 | DSR ack bytes into the shipping decoder | `Event::Unknown(b"\x1b[0n")`, zero `Event::Key` — the §7.3 result, locked as a regression test |
| T11 | kitty OK bytes into the decoder | Currently produces 12 `Event::Key`. **Assert that** so the day someone adds APC handling, this test forces them to update it deliberately rather than by accident |

T11 is the unusual one: it asserts current, undesirable behaviour. That is the point — it
converts an invisible hazard into a visible, named test that must be consciously changed.

---

## 10. Open items and UNVERIFIED

| # | Item | Status |
|---|---|---|
| U1 | Stage costs on the real A10 §4.3 corpus | **UNVERIFIED.** §1.2 is synthetic. Real pages land between the page-like and noise rows; where exactly is unknown. |
| U2 | `write()` cost with the C10 stage probes attached | **UNVERIFIED.** §1.2 cites A10 §0.1's standalone measurement; the probes have not been run in-process. |
| U3 | DSR round-trip time on Ghostty 1.3.1 / iTerm2 3.6.9 / Apple Terminal 465 | **UNVERIFIED.** No TTY is available in this environment; the *parse* behaviour (§7.3) is verified, the *timing* is not. |
| U4 | kitty `q=0` response format from Ghostty for a real `a=T` transmit | **UNVERIFIED.** Only the `a=q` capability query has been observed to reply (`ESC_Gi=31;OK ESC\`, per project brief and `kitty.rs:5-7`). |
| U5 | Synchronized output (DEC 2026) support | **UNVERIFIED**, inherited from A10 §3.5(c). Schema carries `null` for it. |
| U6 | Node-side ring-buffer flush cost | **UNVERIFIED.** The Rust flusher is costed in A10 §2.3; the JS equivalent is not. |
| U7 | Whether `Instant::now()` (`main.rs:841-843`, `857`) and A10's `clock::now_ns()` agree | Not tested. `Instant` is `mach_absolute_time`-based and does **not** survive sleep, while A10 uses `mach_continuous_time`, which does. Mixing them across a sleep would produce silent skew. §3.2 therefore replaces the `Instant` uses in `Renderer` rather than adding alongside them. |

---

## 11. Changes required in commander-owned files

Per the file-ownership rule these are described, not made. Ordered by value per unit of risk.

**11.1 — `apps/cli/src/main.rs:837` / `:857` / `:881`: fix the F1 blind spot.**
Smallest possible change with the largest correctness payoff, and it is worth doing whether or
not the rest of this spec is built. Bracket `kitty::bgra_to_rgb` with a timer in `on_frame`,
carry the result into `present`, and add it to the reported figure. Rename
`Status::last_encode_ms` → `backend_encode_ms` and add `convert_ms` so no downstream reader
silently gets a changed number under an unchanged name. Effect on the status bar: today's
`0.7ms` becomes `3.6ms`, which is the truth.

**11.2 — `crates/bg-term/src/kitty.rs:192`: extend `EncodeStats` with three `u64` stage fields.**
Populated at `kitty.rs:111-115`, `:118-119`, `:121-156`. Keeps `bg-term` dependency-free and
extends its existing unit tests naturally. Nothing outside `encode_rgb_frame` changes.

**11.3 — `apps/cli/src/main.rs:835`: make the truncated-frame drop loud.**
It currently `return`s with no counter and no log. One counter and one `note` record.

**11.4 — `crates/bg-term/src/input.rs:176`: add APC/DCS skip states to `step()`.**
`ESC _ … ESC \` and `ESC P … ESC \` should be consumed and surfaced as a new
`Event::TerminalReply(Vec<u8>)`, not decoded as Alt+char plus literal keystrokes. This is a
**correctness** fix (§7.2), not a profiling one; it also unblocks the kitty-`q=0` ack variant.
It needs its own tests and should not be bundled with profiler work.

**11.5 — `crates/bg-proto/src/lib.rs:90-91`: add a zero-copy `next_message_ref()`.**
Returns a borrowed `&[u8]` and defers the `drain` to an explicit `consume()`. Saves the 8 MB
`to_vec` per frame; measured headroom 0.766 ms p50 (§1.2). Optimisation, not a prerequisite —
and the profiler should land *first* so the win can be demonstrated rather than asserted.

**11.6 — `apps/engine/src/main.js:97` + `:57`: single-copy frame packing.**
Write the 32-byte header and the 5-byte envelope into one preallocated buffer and `sock.write`
it once, instead of two `Buffer.concat` calls. Measured headroom 0.383 ms p50 (§1.2) — real
but the smallest of the three copy wins. Lowest priority.

**11.7 — `apps/cli/src/main.rs:901` + `:653`: the DSR ack sampler.**
Emit `\x1b[5n` after flush on every 60th presented frame; match `b"\x1b[0n"` in the existing
`Event::Unknown` arm. Roughly fifteen lines, no new decoder state (§7.3).

---

## 12. Sources

| Source | Type | Use |
|---|---|---|
| `artifacts/swarm/A10-performance-plan.md` §1.4, §2.2, §3.1-3.5, §4.1-4.2, §8, §10 | in-repo | Clock, `Ev` record, latency chain, pacing definitions, statistics policy, budgets. C10 extends, never redefines. |
| `docs/adr/ADR-0001-browser-engine.md` | in-repo | 60 fps baseline, BGRA non-strided contract |
| `apps/engine/src/main.js`, `apps/cli/src/main.rs`, `crates/bg-term/src/{kitty,b64,input,caps,tty,unicode}.rs`, `crates/bg-proto/src/lib.rs` | in-repo, read in full or in relevant part | Every line number in §3 |
| https://sw.kovidgoyal.net/kitty/graphics-protocol/ | spec | `q=` semantics (§7.2). Spec only — the kitty *implementation* is GPL-3.0 and must not be read or copied (A10 §11). |
| `flate2` 1.x (MIT OR Apache-2.0), `libc` 0.2 (MIT OR Apache-2.0) | deps | Already in `Cargo.toml`; §1.2's bench added no new dependency |
| Measurements in §1.2, §7.2, §7.3 | this document | Scratchpad crates linking the shipping `bg-term`/`bg-proto` by path; commands shown inline; no repo file created or modified |

**License note:** every measurement here was produced with code that either lives in this
repository already or was written for this document. No third-party source was copied. The
kitty protocol is implemented from its published specification only, consistent with A10 §11.
