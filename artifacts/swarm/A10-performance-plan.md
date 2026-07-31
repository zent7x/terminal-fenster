# A10 — BlackGlass Performance Measurement Plan

**Status:** Implementation spec, ready to build
**Date:** 2026-07-31
**Target:** macOS 26.1 (build 25B78), Apple M4 (4 P-core + 6 E-core), 24 GiB, arm64
**Engine (per ADR-0001):** Electron 43.2.0 / Chromium 150.0.7871.129, OSR bitmap `paint` path, BGRA8888 non-strided
**Author's rule for this doc:** every number below marked *[measured]* was produced on this machine during this session, with the exact program shown. Everything else is marked *[doc]* (primary-source documented) or **UNVERIFIED**.

---

## 0. Environment ground truth (measured this session)

These are inputs to every downstream calculation. Do not re-derive them; re-measure only if hardware changes.

| Fact | Value | How obtained |
|---|---|---|
| `mach_timebase_info` | `numer=125, denom=3` (24 MHz tick, **41.667 ns/tick**) | *[measured]* `tb.c` below |
| `mach_absolute_time()` call cost | **5.21 ns** | *[measured]* 1e6-iteration loop |
| `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` call cost | **9.27 ns** | *[measured]* 1e6-iteration loop |
| Minimum observable clock delta | **41 ns** (= 1 tick) | *[measured]* 200k-sample min-nonzero-delta |
| `CLOCK_UPTIME_RAW` ≡ `CLOCK_MONOTONIC_RAW` ≡ `mach_absolute_time()*125/3` | agree to ≤83 ns (call overhead) | *[measured]* `clocks.c` |
| `CLOCK_MONOTONIC` (POSIX) | **7.35 s offset** from the above, µs granularity | *[measured]* — **never use it** |
| Node `process.hrtime.bigint()` | **same timeline as `CLOCK_UPTIME_RAW`, zero offset** | *[measured]* interleave proof, §1.2 |
| `getrusage.ru_maxrss` units on macOS | **bytes** (not KiB as on Linux) | *[measured]* 1261568 for a 1.2 MB process |
| `proc_pid_rusage(RUSAGE_INFO_V6)` | works, no sudo, any own-uid pid | *[measured]* `ru.c` |
| `ri_instructions` / `ri_cycles` | populated, no sudo | *[measured]* 464e9 instr on a live shell |
| `powermetrics` | **requires sudo** — "powermetrics must be invoked as the superuser" | *[measured]* |
| `/usr/bin/time -l` | reports `instructions retired`, `cycles elapsed`, `peak memory footprint` — no sudo | *[measured]* |
| `sample`, `spindump`, `xctrace record`+`export` | work headless, no sudo, own-uid targets | *[measured]* (Instruments 26.0 / 17C52) |
| `kern.clockrate` | `hz=100, tick=10000` → **`ps`/`times` CPU accounting is 10 ms-granular** | *[measured]* |
| `hw.pagesize` | **16384** (16 KiB) — RSS quantum | *[measured]* |
| Display | Built-in Liquid Retina, 2880×1864, **60 Hz** (M4 Air, no ProMotion) | *[measured]* `system_profiler` |
| Chassis | **MacBook Air M4 — fanless.** Thermal throttling is a first-class confound | inherent |
| Battery state during this session | 18%, discharging | *[measured]* `pmset -g ps` — see §8.4 |

### 0.1 THE headline measurement: PTY write is the bottleneck, not Chromium

The single most important number produced this session. A real `posix_openpt` PTY, `cfmakeraw` + `~OPOST` on the slave, a dedicated draining reader thread on the master, 51 iterations, first discarded:

```
payload                  bytes    chunk    p50        p95        p99        p50→fps
1440x900 b64-RGBA      6912064     1024   10.854ms   16.637ms   20.080ms     92.1
1440x900 b64-RGBA      6912064     8192   33.365ms   40.844ms   44.542ms     30.0
1440x900 b64-RGBA      6912064    65536   35.795ms   42.298ms   43.554ms     27.9
720x450  b64-RGBA      1728000     1024    2.557ms    4.913ms    5.067ms    391.1
720x450  b64-RGBA      1728000     8192    8.186ms   10.513ms   11.029ms    122.2
720x450  b64-RGBA      1728000    65536    9.514ms   11.732ms   12.324ms    105.1
half-block text         400000     1024    0.987ms    1.352ms    1.691ms   1013.2
half-block text         400000     8192    2.243ms    2.716ms    2.853ms    445.7
small delta              40000     1024    0.138ms    0.244ms    0.275ms   7263.9
small delta              40000    65536    0.211ms    0.300ms    0.402ms   4745.0
```
*[measured]* — `pty3.c`, reproduced across three independent runs.

Three consequences that the whole plan is built around:

1. **`write()` alone eats the entire 16.67 ms frame budget at full resolution.** Chromium delivers 60 fps (ADR-0001: gap p50 16.65 ms). Our own `write()` of one full-res base64 frame is 10.8 ms p50 / 20.1 ms p99. There is no room left. Damage-driven partial updates are not an optimization, they are load-bearing.
2. **1 KiB chunks are 3.3× faster than 8 KiB or 64 KiB chunks on a macOS PTY.** This is counter-intuitive and inverts the usual "bigger writes are better" instinct. Hypothesis (**UNVERIFIED**, needs `dtrace` on `ptcwrite`): macOS clists / `TTYHIWAT` cause large writes to sleep-and-wake repeatedly. `chunk_bytes` must be a tunable in the harness with a sweep, not a hardcoded constant.
3. For comparison, an ordinary **pipe** hits 5.5–7.25 GB/s *[measured]* `wr.c`, ~20× the PTY's 283–866 MB/s. Do **not** prototype the output path over a pipe and extrapolate; the result will be off by a factor of 20.

---

## 1. Clock discipline (the foundation)

Every timestamp in BlackGlass, in every language, in every process, must be on **one** timeline. Cross-process latency arithmetic is otherwise meaningless.

### 1.1 The chosen clock

**`mach_absolute_time()` scaled by `numer/denom` (= `× 125 / 3` on this machine), equivalently `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`.** Units: nanoseconds since boot, excluding sleep.

Rejected alternatives and why:
- `CLOCK_MONOTONIC` — 7.35 s offset from the chosen timeline and only microsecond-granular *[measured]*.
- `CLOCK_REALTIME` / `Date.now()` — subject to NTP steps and slew; forbidden anywhere on a latency path. Use it once, at harness start, to write a single `(uptime_ns, realtime_ns)` anchor pair into the run manifest for human-readable correlation.
- `mach_continuous_time()` — includes sleep. It read **identical** to `mach_absolute_time()` on this machine *[measured]*, because the machine has not slept since boot. On a laptop that lids, they diverge. See §1.3.

### 1.2 Node and Rust are already on the same clock — proof

libuv's `uv__hrtime()` on Darwin is `mach_continuous_time() * timebase.numer / timebase.denom` *[doc]* (libuv `src/unix/darwin.c`, v1.x, MIT). `process.hrtime.bigint()` is a thin wrapper over it. Interleaved reads *[measured]*:

```
C   CLOCK_UPTIME_RAW      51885793341333
Node process.hrtime.bigint 51885811597750     <-- lands strictly between
C   CLOCK_UPTIME_RAW      51885816189791
```

**Conclusion: a `u64` nanosecond timestamp taken in Rust with `mach_absolute_time()*125/3` and one taken in Node with `process.hrtime.bigint()` are directly subtractable with no calibration constant.** This is what makes the t0→t4 chain in §3 tractable.

### 1.3 The one hazard

`mach_absolute_time()` (Rust side, if you use it directly) stops during system sleep; `mach_continuous_time()` (Node side) does not. On this machine the delta is currently 0 *[measured]*, so they are interchangeable — **until the lid closes mid-soak.** During the 8 h soak (§7) the machine *will* be idle and *may* sleep.

**Mitigation, mandatory:**
- Run every soak under `caffeinate -dimsu ./harness ...` *[measured: `/usr/bin/caffeinate` present]*.
- On the Rust side call **`mach_continuous_time() * numer / denom`** — literally the same expression libuv uses *[doc: libuv `src/unix/darwin.c`]* — so Node and Rust agree by construction, sleep or no sleep. Do **not** substitute `clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW)` on the assumption that it is continuous: on this machine it reads identically to `CLOCK_UPTIME_RAW` *[measured]*, but that proves nothing because `mach_continuous_time() − mach_absolute_time() == 0` here (the machine has not slept since boot). **Whether `CLOCK_MONOTONIC_RAW` on Darwin includes sleep is UNVERIFIED**, and matching libuv's exact expression makes the question moot.
- The harness emits `con_ns - abs_ns` at start and end of every run into the manifest. If it changed, the run is **invalid** and must be discarded, not corrected.

Darwin `clockid_t` values, for reference *[measured: SDK `usr/include/_time.h` on MacOSX.sdk, cross-checked by compiling]*: `CLOCK_REALTIME=0`, `CLOCK_MONOTONIC_RAW=4`, `CLOCK_MONOTONIC_RAW_APPROX=5`, `CLOCK_MONOTONIC=6`, `CLOCK_UPTIME_RAW=8`, `CLOCK_UPTIME_RAW_APPROX=9`, `CLOCK_PROCESS_CPUTIME_ID=12`, `CLOCK_THREAD_CPUTIME_ID=16`.

### 1.4 Rust: the canonical clock module

`crates/bg-bench/src/clock.rs`:

```rust
//! One clock for the whole product. Matches Node's process.hrtime.bigint() exactly.
use std::sync::OnceLock;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MachTimebaseInfo { numer: u32, denom: u32 }

// Darwin clockid_t values, verified against MacOSX.sdk usr/include/_time.h [measured].
const CLOCK_UPTIME_RAW: u32 = 8;

unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    fn mach_absolute_time() -> u64;
    fn mach_continuous_time() -> u64;
    fn clock_gettime_nsec_np(clock_id: u32) -> u64;
}

/// Nanoseconds on EXACTLY the timeline Node's `process.hrtime.bigint()` uses.
/// This is libuv's `uv__hrtime` expression verbatim (src/unix/darwin.c): agreement is
/// by construction, not by coincidence, and it survives system sleep.
/// Cost [measured]: mach_continuous_time 5.37 ns, the full `*125/3` expression 8.36-8.50 ns.
/// (mach_absolute_time is 5.33 ns; the 3 ns delta is the mul/div, not the syscall.)
/// At 8 timestamps/frame that is 68 ns of a 16,670,000 ns budget: 0.0004%.
#[inline(always)]
pub fn now_ns() -> u64 {
    let (n, d) = timebase();   // OnceLock: one relaxed atomic load, already in cache
    unsafe { mach_continuous_time() } * n as u64 / d as u64
}

/// ~5.2 ns/call [measured]. Use ONLY inside a single burst where sleep is impossible
/// (e.g. encode microbenchmarks). Never for cross-process arithmetic.
#[inline(always)]
pub fn now_ticks() -> u64 { unsafe { mach_absolute_time() } }

pub fn timebase() -> (u32, u32) {
    static TB: OnceLock<(u32, u32)> = OnceLock::new();
    *TB.get_or_init(|| {
        let mut i = MachTimebaseInfo::default();
        unsafe { mach_timebase_info(&mut i) };
        (i.numer, i.denom) // (125, 3) on Apple M4 [measured]
    })
}

#[inline(always)]
pub fn ticks_to_ns(t: u64) -> u64 {
    let (n, d) = timebase();
    // 125/3 fits u64 for any plausible uptime: u64::MAX/125 ≈ 1.5e17 ticks ≈ 200 years.
    t * n as u64 / d as u64
}

/// Emit into every run manifest. Nonzero at end-of-run => machine slept => DISCARD RUN.
pub fn sleep_skew_ns() -> i64 {
    let (a, c) = unsafe { (mach_absolute_time(), mach_continuous_time()) };
    ticks_to_ns(c).wrapping_sub(ticks_to_ns(a)) as i64
}

/// Single wall-clock anchor for the manifest. Never on a latency path.
pub fn realtime_anchor() -> (u64, u64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let rt = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
    (now_ns(), rt)
}
```

### 1.5 Node: the matching module

`apps/engine/src/bench/clock.js`:

```js
'use strict';
// process.hrtime.bigint() === libuv uv_hrtime() === mach_continuous_time()*125/3
// on Darwin. Verified identical to Rust clock::now_ns() [measured 2026-07-31].
const now = () => process.hrtime.bigint();          // BigInt ns
const nowNum = () => Number(process.hrtime.bigint()); // f64 ns; exact to 2^53 ns ≈ 104 days uptime
module.exports = { now, nowNum,
  realtimeAnchor: () => ({ up_ns: process.hrtime.bigint().toString(),
                           rt_ns: String(BigInt(Date.now()) * 1000000n),
                           perf_time_origin_ms: performance.timeOrigin }) };
```

`Number()` is safe: `2^53 ns = 104 days` of uptime. Add an assertion that `now() < 2n**53n` at startup and fail loudly if the machine has been up longer.

---

## 2. Benchmark harness architecture

### 2.1 Design constraints

- **Zero allocation, zero formatting, zero locking on the hot path.** A `println!` in a paint handler is a 1–10 µs syscall that will itself show up as jitter.
- **Always-compiled, runtime-gated.** `BG_TRACE=1` env var. Trace-disabled cost must be one predictable-branch load. Do *not* use `#[cfg(feature)]` for this — the measured build must be bit-identical to the shipped build, or you are measuring a different program.
- **Binary records, decoded offline.** No JSON on the hot path.
- **Every process writes its own file.** Correlation happens offline via §1's shared clock.

### 2.2 The record format

Fixed 32 bytes, naturally aligned, no padding surprises:

```rust
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ev {
    pub t_ns:  u64,   // clock::now_ns()
    pub seq:   u64,   // frame or input sequence number; ties phases together
    pub aux:   u64,   // payload-dependent: byte count, queue depth, dirty-rect area...
    pub kind:  u16,   // EvKind discriminant
    pub pid:   u16,   // low 16 bits of getpid(), so files can be concatenated
    pub _pad:  u32,
}
const _: () = assert!(std::mem::size_of::<Ev>() == 32);
```

At 60 fps × 8 events/frame × 8 h = **13.8 M records = 442 MiB**. Acceptable. At 1 kHz input replay it is larger; the ring drops oldest and increments a `dropped` counter that must be reported (a harness that silently drops samples produces a lying p99).

```rust
#[repr(u16)]
pub enum EvKind {
    InStdinRead      = 1,  // t0
    InParsed         = 2,
    InIpcSend        = 3,
    InIpcRecv        = 4,  // Node side
    InDispatched     = 5,  // t1: sendInputEvent() returned
    RendererSawEvent = 6,  // from pixel-stamp, §3.4
    PaintEvent       = 7,  // t2: Electron 'paint' fired
    FrameIpcSend     = 8,
    FrameIpcRecv     = 9,
    EncodeBegin      = 10,
    EncodeEnd        = 11, // t3
    WriteBegin       = 12,
    WriteEnd         = 13, // t4: write()/writev() returned
    TermAckRecv      = 14, // t5a: kitty OK or DSR reply, §3.5
    PhotonObserved   = 15, // t5b: offline, from capture, §3.6
    FrameCoalesced   = 16, // a paint we deliberately discarded
    QueueDepth       = 17, // aux = depth, sampled at enqueue
    IdleSample       = 18,
    MemSample        = 19,
}
```

### 2.3 Ring buffer + flusher

```rust
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

const CAP: usize = 1 << 20; // 1,048,576 records = 32 MiB, power of two

pub struct Trace {
    buf:     Box<UnsafeCell<[Ev; CAP]>>,
    head:    AtomicU64,   // producer-only monotonic counter
    dropped: AtomicU64,
    on:      AtomicBool,
}
unsafe impl Sync for Trace {}

impl Trace {
    #[inline(always)]
    pub fn rec(&self, kind: EvKind, seq: u64, aux: u64) {
        // Trace-off cost: one relaxed atomic load + predictable branch. ~1 ns.
        if !self.on.load(Ordering::Relaxed) { return; }
        let i = self.head.fetch_add(1, Ordering::Relaxed);
        let slot = (i as usize) & (CAP - 1);
        unsafe {
            (*self.buf.get())[slot] = Ev {
                t_ns: crate::clock::now_ns(), seq, aux,
                kind: kind as u16, pid: std::process::id() as u16, _pad: 0,
            };
        }
    }
}

pub static TRACE: /* lazily inited */ ... ;

#[macro_export]
macro_rules! ev {
    ($k:expr, $seq:expr, $aux:expr) => { $crate::trace::TRACE.rec($k, $seq, $aux) };
}
```

A **dedicated flusher thread**, pinned to nothing, wakes every 250 ms, snapshots `head`, and `write`s the new region to `run-<pid>.bgtrace` with `O_APPEND`. It must **never** be the same thread that does encoding or I/O. Verify the flusher's own cost with `proc_pid_rusage` on its thread (`thread_info` / `THREAD_BASIC_INFO`).

Single-producer is the common case (one paint thread). Where two threads record (paint thread + write thread), `fetch_add` on `head` is already MPSC-safe for slot claiming; the only hazard is the flusher reading a slot whose write has not landed. Fix: producer publishes a per-slot `t_ns != 0` sentinel last, and the flusher stops at the first zero. Simpler alternative: one ring per thread, `thread_local!`, concatenated offline. **Prefer the per-thread ring** — it is trivially correct and removes the atomic contention entirely.

### 2.4 Node-side trace

Node needs the same file format so the offline decoder is shared. Use a preallocated `Buffer` and `DataView`, `fs.writeSync` from a `setInterval`:

```js
const CAP = 1 << 18, REC = 32;
const buf = Buffer.allocUnsafe(CAP * REC);
let head = 0, dropped = 0;
const ON = process.env.BG_TRACE === '1';
const PID = process.pid & 0xffff;

function ev(kind, seq, aux) {
  if (!ON) return;
  const i = head++ % CAP, o = i * REC;
  buf.writeBigUInt64LE(process.hrtime.bigint(), o);
  buf.writeBigUInt64LE(BigInt(seq), o + 8);
  buf.writeBigUInt64LE(BigInt(aux), o + 16);
  buf.writeUInt16LE(kind, o + 24);
  buf.writeUInt16LE(PID, o + 26);
  buf.writeUInt32LE(0, o + 28);
}
```

`process.hrtime.bigint()` allocates a BigInt — roughly 40–80 ns and GC pressure. **UNVERIFIED on Node 24.11.1; measure it before trusting it inside the `paint` handler.** If it is too costly, the fallback is a tiny N-API addon exposing `clock_gettime_nsec_np` as a `double` (exact below 2^53 ns).

### 2.5 Harness CLI surface

```
bgbench latency   --url <U> --viewport WxH --n 2000 --warmup 200 --input-rate 20
bgbench pacing    --url <U> --viewport WxH --duration 60s --repeat 3
bgbench startup   --mode cold|warm --n 30 --url <U>
bgbench idle      --url <U> --duration 120s
bgbench soak      --url <U> --duration 8h --sample-interval 30s
bgbench encode    --frames <dir-of-bgra> --codec kitty|iterm2|halfblock --n 500
bgbench sweep     --param chunk_bytes --values 512,1024,2048,4096,8192,65536
bgbench decode    run-*.bgtrace --out summary.json
```

Every subcommand writes `manifest.json` containing: git SHA, `rustc -Vv`, `node -v`, `process.versions.electron`/`.chrome`, `sw_vers`, `TERM`/`TERM_PROGRAM`/`TERM_PROGRAM_VERSION`, terminal version from XTVERSION (§3.5), viewport, `pmset -g ps` output, `pmset -g therm` before *and* after, `sleep_skew_ns` before and after, `sysctl -n hw.model machdep.cpu.brand_string`, and every tunable's value. **A result without its manifest is not a result.**

---

## 3. Input-to-photon latency

### 3.1 The chain and where each stamp lives

| Stamp | Meaning | Process | Exact call site |
|---|---|---|---|
| **t0** | Input byte(s) read from stdin | Rust TUI | immediately after `read(0, ...)` / `poll` wake returns |
| t0b | Key decoded to a semantic event | Rust TUI | after the CSI/kitty-keyboard parser emits |
| t0c | IPC frame written to engine | Rust TUI | after `write()` on the control socket returns |
| t0d | IPC frame parsed | Node main | in the socket `data` handler |
| **t1** | `sendInputEvent` dispatched | Node main | the statement after `wc.sendInputEvent(e)` returns |
| t1b | Renderer *observed* the event | Renderer | pixel-stamp, §3.4 — the only way to see inside |
| **t2** | Paint event received | Node main | first statement inside the `'paint'` handler |
| t2b | Bitmap materialized | Node main | after `image.getBitmap()` returns (this is a **copy**, measure it) |
| t2c | Frame bytes arrive at renderer-side | Rust TUI | after reading the frame off the IPC channel |
| **t3** | Encode complete | Rust TUI | after the escape-sequence buffer is fully built |
| **t4** | `write()`/`writev()` to terminal fd returns | Rust TUI | after the last chunk's `write` returns |
| *t5a* | Terminal acknowledged ingest | Rust TUI | on reading the ACK reply, §3.5 |
| *t5b* | Photons | external | offline capture, §3.6 |

**Controllable latency := t4 − t0.** This is the number BlackGlass owns, the number we regress against, and the number we publish. Everything past t4 is the terminal's and the compositor's.

### 3.2 Why this is honest and what it hides

t4 − t0 does **not** include: the terminal's escape-sequence parse, its texture upload, its own render, `CATransaction` commit, WindowServer composite, and panel scan-out. On a 60 Hz panel *[measured]* the compositor alone contributes a uniform 0–16.67 ms of phase, mean 8.33 ms, that no amount of our engineering removes.

The report must always state, verbatim:

> Controllable latency (t4 − t0) is X ms p50 / Y ms p99. Terminal present latency is not observable from inside the pipe; it is bounded below by the ingest ACK round trip (§3.5, measured Z ms) and above by the capture measurement (§3.6, measured W ms). End-to-end input-to-photon is therefore in [X + Z_lower, X + W].

Never publish a single "input to photon" number derived only from t4 − t0. That is the mistake this section exists to prevent.

### 3.3 Deliberately excluded from t0

Keyboard-to-stdin latency (USB/Bluetooth HID poll, IOKit, terminal's key handling, PTY line discipline) precedes t0 and is unmeasurable from inside our process. Bluetooth keyboards add a documented 10–30 ms that will swamp everything else. **All latency runs must use a wired keyboard or, better, synthetic input written directly into the PTY master by the harness** (§3.7) so this term is exactly zero and reproducible.

### 3.4 Pixel-stamps: seeing inside the renderer without CDP

The gap t1 → t2 is a black box: `sendInputEvent` goes in, a bitmap comes out, and we cannot tell whether 12 ms was spent in event routing, in JS, in layout, or in raster. CDP `Page.screencastFrame` carries a real `metadata.timestamp` documented as *"Frame swap timestamp"* (`Network.TimeSinceEpoch`, float seconds) *[doc: chromedevtools.github.io/devtools-protocol/tot/Page/]* — but ADR-0001 rejected screencast for production precisely because of its encode/JSON cost, and attaching it changes what we are measuring.

**Use pixel-stamps instead.** On benchmark pages only, inject:

```js
// bgstamp.js — injected via wc.executeJavaScript() before the run.
(() => {
  const S = document.createElement('canvas');
  S.width = 16; S.height = 1;
  S.style.cssText = 'position:fixed;left:0;top:0;width:16px;height:1px;z-index:2147483647;image-rendering:pixelated';
  document.documentElement.appendChild(S);
  const g = S.getContext('2d', { willReadFrequently: false });
  let seq = 0, lastInputNs = 0;

  // Renderer-side observation of the input event.
  for (const t of ['keydown', 'mousedown', 'wheel', 'pointerdown']) {
    addEventListener(t, () => {
      // performance.now() is ms-float since timeOrigin; convert to the shared ns timeline.
      lastInputNs = Math.round((performance.timeOrigin + performance.now()) * 1e6);
    }, { capture: true, passive: true });
  }

  function frame() {
    seq++;
    // Encode seq (48 bits) and a "has input" marker into pixels 0..7, blue channel.
    const d = g.createImageData(16, 1);
    for (let i = 0; i < 8; i++) {
      const byte = Number((BigInt(seq) >> BigInt(8 * i)) & 0xffn);
      d.data[i * 4 + 0] = 0; d.data[i * 4 + 1] = 0;
      d.data[i * 4 + 2] = byte; d.data[i * 4 + 3] = 255;
    }
    // High-contrast luminance flip in pixels 8..15 for the external camera (§3.6):
    const lum = (seq & 1) ? 255 : 0;
    for (let i = 8; i < 16; i++) {
      d.data[i*4] = lum; d.data[i*4+1] = lum; d.data[i*4+2] = lum; d.data[i*4+3] = 255;
    }
    g.putImageData(d, 0, 0);
    // Report (seq, rAF time, last input time) out-of-band, off the pixel path.
    window.__bgstamp && window.__bgstamp(seq, Math.round((performance.timeOrigin + performance.now()) * 1e6), lastInputNs);
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();
```

On the harness side, read the stamp out of the BGRA buffer **before** any downscaling. Frame is BGRA8888, non-strided, `w*h*4` bytes *[ADR-0001, verified: 1440×900 = 5 184 000 bytes exactly]*, so the blue byte of pixel *i* in row 0 is at offset `i*4 + 0`:

```rust
/// BGRA: byte order is B,G,R,A. Blue is index 0. [ADR-0001, verified against a pure-red page]
#[inline]
fn read_stamp(bgra: &[u8]) -> u64 {
    let mut seq = 0u64;
    for i in 0..8 { seq |= (bgra[i * 4] as u64) << (8 * i); }
    seq
}
```

Now `t2 − rAF_ns(seq)` is the exact compositor→our-process transport cost, and `rAF_ns(seq) − t1` is the exact in-renderer cost, with no CDP and no extra encode on the hot path. This decomposes the single biggest opaque term in the chain.

Two cautions: (a) a 16×1 canvas repainting every frame forces a paint the page might not otherwise do — it changes what you measure. Use it in `latency` and `pacing` runs, **never** in `idle` runs (§6), where its raf loop would destroy the whole measurement. (b) If the TUI downsamples before the stamp is read, the bytes are destroyed. Read first, downsample second.

### 3.5 Bounding the terminal from below: the ingest ACK

Two mechanisms, exact bytes:

**(a) Kitty graphics protocol response — Ghostty 1.3.1.** *[doc: sw.kovidgoyal.net/kitty/graphics-protocol/]*
The protocol is APC-wrapped: prefix `ESC _` = `0x1B 0x5F`, terminator `ESC \` = `0x1B 0x5C`.
On a successful transmit the terminal replies `ESC _ G i=<id>;OK ESC \`, i.e. `0x1B 0x5F 0x47 "i=31;OK" 0x1B 0x5C`, unless suppressed by `q=`. `q=1` suppresses `OK`; `q=2` suppresses errors too.

Zero-cost capability probe (a 1×1 24-bit-RGB query that transmits nothing):
```
1B 5F 47 69 3D 33 31 2C 73 3D 31 2C 76 3D 31 2C 61 3D 71 2C 74 3D 64 2C 66 3D 32 34 3B 41 41 41 41 1B 5C
ESC _  G  i  =  3  1  ,  s  =  1  ,  v  =  1  ,  a  =  q  ,  t  =  d  ,  f  =  2  4  ;  A  A  A  A  ESC \
```
(`AAAA` = base64 of three zero bytes = one RGB pixel.) Expected reply `ESC_Gi=31;OK ESC\`.

**Latency-probe policy:** normal frames carry `q=2` so the hot path never has to parse responses. Every *N*-th frame (N=60, i.e. ~1 Hz) is sent with `q=0`; the reader thread stamps `TermAckRecv` when the `OK` arrives. `t5a − t4` is then the terminal's **ingest + parse + (usually) texture-upload** time. It is a *lower bound* on present latency, not present latency.

**(b) DSR — universal fallback, works on iTerm2 and Apple Terminal.**
Request `ESC [ 5 n` = `0x1B 0x5B 0x35 0x6E`; reply `ESC [ 0 n` = `0x1B 0x5B 0x30 0x6E`.
Because terminals process their input stream strictly in order, a DSR issued immediately after the frame bytes cannot be answered until the frame bytes have been consumed. This bounds ingest on any VT-compatible terminal.
iTerm2's inline-image protocol (OSC 1337) has **no acknowledgement of its own** *[doc: iterm2.com/documentation-images.html — the spec documents no response]*, so DSR is the only option there.

**(c) Wrap frames in synchronized output (DEC private mode 2026)** so the terminal never presents a half-written frame — this removes tearing as a confound:
```
BSU  ESC [ ? 2 0 2 6 h   = 1B 5B 3F 32 30 32 36 68
ESU  ESC [ ? 2 0 2 6 l   = 1B 5B 3F 32 30 32 36 6C
```
Query support with DECRQM: `ESC [ ? 2 0 2 6 $ p` = `1B 5B 3F 32 30 32 36 24 70`; reply is DECRPM `ESC [ ? 2026 ; <Ps> $ y` where `Ps` 1 or 2 means supported. **UNVERIFIED against Ghostty 1.3.1 / iTerm2 3.6.9 on this machine** — run the probe in §9.4 before relying on it.

**(d) Terminal identification** for the manifest: XTVERSION `ESC [ > 0 q` = `1B 5B 3E 30 71`, reply is `DCS > | <name and version> ST`. Primary DA `ESC [ c` = `1B 5B 63` as fallback.

### 3.6 Bounding the terminal from above: capture

Ranked by fidelity:

1. **External high-speed camera (recommended ground truth).** An iPhone at 240 fps slow-mo gives **4.17 ms** quantization. Frame the terminal window and a second display (or an LED) driven by the harness at t0. Count frames between the t0 marker and the first frame showing the §3.4 luminance flip. Do this **once per release** as a calibration, not per-CI-run. Cost: ~30 min of manual work. Value: the only number in this entire document that is genuinely end-to-end.
2. **ScreenCaptureKit.** `SCStreamConfiguration.minimumFrameInterval` + `CMSampleBuffer` presentation timestamps. **Capped by the panel: 60 Hz on this MacBook Air *[measured]*, so ±16.67 ms** — barely better than the whole frame budget, and it measures WindowServer composite, still not scan-out. Also, SCK capture itself consumes GPU and perturbs the thing being measured. Use only as a cross-check.
3. **`screencapture(1)` / periodic grabs.** Far too slow. Do not use.

The honest ceiling: `photon ≤ t5b_observed`, quantized to the capture interval, and we report the quantization explicitly.

### 3.7 Synthetic input injection (mandatory for reproducibility)

Do **not** benchmark with a human pressing keys. The harness opens a PTY pair (`posix_openpt` / `grantpt` / `unlockpt` / `ptsname`), runs BlackGlass on the slave, and writes input bytes to the master at a controlled rate. t0 is then the TUI's `read()` return, and the harness *also* knows exactly when it wrote the byte, giving a free sanity check on PTY input latency:

```rust
let m = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
unsafe { libc::grantpt(m); libc::unlockpt(m); }
let slave_name = unsafe { std::ffi::CStr::from_ptr(libc::ptsname(m)) };
// spawn blackglass with stdin/stdout/stderr = open(slave_name), setsid, TIOCSCTTY
// then: write(m, b"j", 1) at a controlled cadence; record inject_ns
```

Inject at **20 Hz** (50 ms apart) for latency runs. This is fast enough for 2000 samples in 100 s and slow enough that each keystroke's work fully drains before the next, so you measure latency, not queueing. Run a separate `--input-rate 200` condition to characterize behavior under input flood, and report it as a *distinct* number.

Set the PTY to raw with `cfmakeraw()` and clear `OPOST` on the slave. If you skip this, `ONLCR` will translate `\n` to `\r\n` inside your image payloads and corrupt frames — and you will spend a day thinking the encoder is broken.

---

## 4. Frame pacing metrics

### 4.1 Definitions (pin these down or the numbers are not comparable)

- **`paint_fps`** — count of `EvKind::PaintEvent` per wall second. What Chromium hands us. ADR-0001 baseline: 60.2 fps, gap p50 16.65 ms.
- **`present_fps`** — count of `WriteEnd` per wall second. What actually reaches the terminal. **This is the number that matters**; `paint_fps` without it is vanity.
- **`coalesce_ratio` = 1 − present_fps / paint_fps.** Frames we deliberately dropped because a newer one arrived while we were still encoding or writing. Deliberate coalescing is *correct behavior* — it is how you stay responsive when the PTY is the bottleneck (§0.1) — but it must be counted, named, and reported, never hidden.
- **`drop_count`** — paints for which no `WriteEnd` with that `seq` exists **and** which were not deliberately coalesced (no matching `FrameCoalesced`). These are bugs.
- **`queue_depth`** — number of frames buffered between the `paint` handler and the writer, sampled at every enqueue (`EvKind::QueueDepth`, `aux` = depth). Report p50/max. A queue depth that grows monotonically means the writer is permanently behind and latency is unbounded; **the harness must fail the run** if `max(queue_depth) > 3` under a steady 60 fps load.
- **`bytes_per_frame`** — `aux` on `WriteEnd`. Report p50/p95/p99 and total bytes/second. Cross-check against §0.1: `bytes_per_second / 866e6` is a hard floor on PTY occupancy at 1 KiB chunks.
- **`encode_ms`** — `EncodeEnd − EncodeBegin`, p50/p95/p99.
- **`write_ms`** — `WriteEnd − WriteBegin`, p50/p95/p99. Expect this to dominate at full resolution (§0.1).
- **`interframe_gap_ms`** — successive `WriteEnd` deltas. Report p50/p95/p99 **and the standard deviation**, because pacing *smoothness* is perceptually distinct from pacing *rate*. 60 fps with a 30 ms stutter every second looks worse than a steady 45 fps.
- **`damage_area_ratio`** — `dirtyRect.width*height / (w*h)`. ADR-0001 established that Electron OSR does report partial damage. Given §0.1, exploiting it is the primary optimization lever; this metric measures whether we are.

### 4.2 Frame-drop attribution

A dropped frame is useless without a cause. Classify each drop by the phase that overran, using the trace:

| Condition | Attributed cause |
|---|---|
| `EncodeBegin(n+1) < EncodeEnd(n)` | encoder-bound |
| `WriteBegin(n+1) < WriteEnd(n)` | **PTY-bound** (expect this to be the common case) |
| `PaintEvent(n+1) − PaintEvent(n) > 1.5 × target` | engine-bound |
| `FrameIpcRecv(n) − FrameIpcSend(n) > 2 ms` | IPC-bound |

Emit the histogram of causes with every pacing run.

### 4.3 Test corpus (fixed, versioned, offline)

Network variance destroys reproducibility. Serve all pages from a local `bgbench serve` on `127.0.0.1` with fixed content, and commit the corpus:

| Case | Page | Exercises |
|---|---|---|
| `static-text` | `about:blank` + a paragraph | idle path, §6 |
| `canvas-full` | full-viewport canvas fill + text (ADR-0001's page) | worst-case full damage |
| `canvas-corner` | 80×80 canvas repaint in one corner | damage-tracking effectiveness |
| `scroll-long` | 20 000-line document, programmatic scroll | full-damage scroll, realistic |
| `video-h264` | local 1080p30 h264 `<video>` | decode + composite path |
| `webgl-cube` | rotating cube | GPU path |
| `real-github` | a captured, frozen snapshot of a real page | web-compat realism |

Two viewports minimum: **1440×900** (ADR-0001's, 5 184 000 B/frame) and **720×450** (1 296 000 B/frame). §0.1 shows a 4× difference in write cost; conclusions do not transfer between them.

---

## 5. Cold vs warm start

### 5.1 Definitions

- **Cold start** — no BlackGlass process running; Chromium's on-disk caches (code cache, GPU shader cache, HTTP cache) purged; the target page has never been loaded. Ends at **first pixel present**.
- **Warm start** — a BlackGlass engine process is already resident; measure `new BrowserWindow` → first pixel of a page that is already in cache.
- **First pixel** — the first `WriteEnd` whose payload contains non-background pixels for the target page. Not `did-finish-load`, not the first `paint` (which is often a blank white frame — count it separately as `first_paint` and report both).

### 5.2 The zero-instrumentation spawn-time trick

`proc_pid_rusage(RUSAGE_INFO_V6).ri_proc_start_abstime` gives the **exact `mach_absolute_time` at which the process was created** *[measured: `ri_proc_start_abstime=1204382388322` on a live shell, converts to the correct 27-minute-old start]*. This means t_spawn requires **no instrumentation inside the child at all**, and it correctly excludes shell/fork overhead the parent would otherwise attribute to us.

```rust
#[repr(C)] pub struct RusageInfoV6 { /* ... 40+ fields, see <sys/resource.h> ... */ }
const RUSAGE_INFO_V6: i32 = 6;
unsafe extern "C" { fn proc_pid_rusage(pid: i32, flavor: i32, buf: *mut RusageInfoV6) -> i32; }

pub fn proc_start_ns(pid: i32) -> Option<u64> {
    let mut ri = std::mem::MaybeUninit::<RusageInfoV6>::zeroed();
    if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V6, ri.as_mut_ptr()) } != 0 { return None; }
    Some(crate::clock::ticks_to_ns(unsafe { ri.assume_init() }.ri_proc_start_abstime))
}
```

Beware: this is on the **absolute** (non-continuous) timeline. Convert consistently, and see §1.3.

Cold start is then `first_pixel_ns − proc_start_ns(main_pid)`, plus a per-child-process breakdown (`proc_start_ns` of the GPU process, the renderer, each utility process) that shows exactly where the 2104 ms in ADR-0001's `example.com` measurement goes.

### 5.3 Cold-start procedure

```sh
#!/bin/zsh
# bench/cold-start.sh — n cold starts with caches purged. Run under `caffeinate -dimsu`.
set -euo pipefail
N=${1:-30}
APPSUP="$HOME/Library/Application Support/blackglass"
CACHE="$HOME/Library/Caches/blackglass"
for i in $(seq 1 $N); do
  pkill -f 'blackglass|Electron' 2>/dev/null || true
  sleep 1
  rm -rf "$APPSUP/Cache" "$APPSUP/Code Cache" "$APPSUP/GPUCache" "$CACHE"
  sync
  # `purge` drops the unified buffer cache; on macOS 26 it may require sudo — check and
  # record whether it ran, because a warm UBC and a cold UBC are different experiments.
  /usr/sbin/purge 2>/dev/null && P=1 || P=0
  BG_TRACE=1 BG_RUN="cold-$i" ./target/release/blackglass --url "$URL" --exit-after-first-pixel
  echo "{\"run\":$i,\"purged\":$P}" >> cold-meta.jsonl
done
```

Because `purge` may need sudo, **record whether it succeeded per run**. Runs with and without a purged unified buffer cache must be analyzed as separate populations. n=30, discard the first 3.

### 5.4 Reference points to publish alongside

Cold start is only meaningful relative to something. Measure, on the same machine, with the same script: `open -na Ghostty.app` to a shell prompt, `/Applications/Safari.app` to first paint of the same local URL, and bare `electron --version`. **UNVERIFIED — none of these have been measured yet.**

---

## 6. Idle cost

The claim to be proven: *a static page costs approximately zero CPU.* This is the claim most likely to be false in a browser (Chromium has timers, GC, compositor heartbeats, network keep-alives) and it is the one that determines whether BlackGlass is usable in a background tmux pane.

### 6.1 Method: `proc_pid_rusage` deltas, not sampling

`ps` is unusable here: `kern.clockrate` is `hz=100` *[measured]*, so `ps` CPU accounting is quantized to **10 ms**, and `%CPU` is a decayed average, not an integral. Over 120 s of near-idle you would be measuring quantization noise.

`proc_pid_rusage` gives `ri_user_time` and `ri_system_time` in **nanoseconds** *[measured]*, and `ri_instructions` / `ri_cycles` as exact counters *[measured: 464 365 971 370 instructions on a live shell]*. Take the delta across the window. This is exact, needs no sudo, and works on every process in the tree.

```rust
pub struct IdleSample { pub pid: i32, pub user_ns: u64, pub sys_ns: u64,
                        pub instructions: u64, pub cycles: u64, pub phys_footprint: u64,
                        pub idle_wkups: u64, pub interrupt_wkups: u64 }

// Metric: cpu_fraction = (Δuser_ns + Δsys_ns) / Δwall_ns, summed over the whole process tree.
// Secondary: instructions_per_second — immune to frequency scaling, which cpu_fraction is not.
// Tertiary: ri_pkg_idle_wkups per second — the metric that actually predicts battery drain.
```

`ri_pkg_idle_wkups` deserves emphasis: on Apple Silicon, **wakeups matter more than cycles** for battery. A process burning 0.1% CPU in one 10 ms block per second is far cheaper than one burning 0.1% spread over 1000 wakeups. Report both.

### 6.2 Procedure

```sh
# bench/idle.sh
caffeinate -dimsu ./target/release/bgbench idle --url http://127.0.0.1:8099/static-text --duration 120s
```

The harness:
1. Loads the page, waits for quiescence — defined as **no `paint` event for 3 consecutive seconds**. Discard everything before that point.
2. Enumerates the process tree (main, GPU, renderer, network, each utility). `pgrep -P <pid>` recursively, or `proc_listchildpids`.
3. Samples every process at t=0 and t=120 s. Delta, not average.
4. Also samples at 5 s intervals so a slow leak or a periodic wakeup storm is visible as a shape, not just an endpoint.

**Critically: no pixel-stamp canvas, no rAF loop, no animated corpus page.** The §3.4 stamp injects a 60 Hz repaint and would turn a 0.1% idle measurement into a 25% one. The harness must assert `BG_STAMP` is unset in `idle` mode and refuse to run otherwise.

### 6.3 Pass/fail

| Metric | Budget | Rationale |
|---|---|---|
| Total tree `cpu_fraction` over 120 s idle | **< 0.5 %** of one core | ~0.05 % of the 10-core machine; invisible in `top` |
| BlackGlass Rust TUI process alone | **< 0.05 %** | it should be blocked in `poll()` |
| `write()` syscalls during idle | **0** after quiescence | any output on a static page is a bug |
| `ri_pkg_idle_wkups` per second, whole tree | **< 20/s** | above this, battery impact becomes user-visible |

A single-run cross-check that requires no harness at all:
```sh
/usr/bin/time -l ./target/release/blackglass --url ... --idle-then-exit 120
# -> "instructions retired", "cycles elapsed", "peak memory footprint" [measured: works, no sudo]
```

### 6.4 On `powermetrics`

`powermetrics` gives per-process energy impact and is the *right* tool, but it **requires sudo** *[measured: "powermetrics must be invoked as the superuser"]*, which makes it unusable in unattended CI. Do not put it on the critical path. If a human wants a one-off energy number:
```sh
sudo powermetrics -n 12 -i 10000 --samplers tasks --show-process-energy | grep -i blackglass
```
`proc_pid_rusage`'s `ri_billed_energy` / `ri_serviced_energy` fields are populated without sudo *[measured: 51 461 863 on a live shell]* and are the sudo-free proxy — but **their units are UNVERIFIED** (nanojoules is the common assumption; do not publish absolute energy figures from this field, only ratios between conditions measured identically).

---

## 7. Memory and leak detection over an 8 h soak

### 7.1 What to sample

Per process in the tree, every 30 s:

| Field | Source | Why |
|---|---|---|
| `ri_phys_footprint` | `proc_pid_rusage` | **the primary metric.** This is what macOS's own memory limits and jetsam use; it counts dirty + compressed + IOKit mappings and excludes shared clean pages. RSS over-counts shared framework pages across the 5+ Chromium processes and will make the total look like ~2× reality. |
| `ri_resident_size` | `proc_pid_rusage` | secondary, for comparison |
| `ri_lifetime_max_phys_footprint` | `proc_pid_rusage` | free high-water mark |
| `ri_user_time`, `ri_system_time` | `proc_pid_rusage` | detects CPU creep |
| compressed / swap | `vm_stat` (global) | detects whole-machine pressure |
| JS heap | `process.memoryUsage()` + `v8.getHeapStatistics()` in the engine | separates a JS leak from a native one |
| Rust allocator | `mallinfo`-equivalent, or an instrumented `GlobalAlloc` counting live bytes | separates a Rust leak from Chromium's |

*[measured: `ri_phys_footprint=411 010 584` vs `ri_resident_size=769 507 328` on a live shell — a 1.87× difference. Choosing the wrong one changes every conclusion.]*

### 7.2 Soak workload

8 h is only meaningful if it does work. Loop, with a 45 s period: load page A → scroll to bottom → load page B → resize viewport → open/close a second tab → return to A. Resize and tab lifecycle are where OSR leaks live (unreleased `NativeImage` backing stores, un-`destroy()`ed `BrowserWindow`s). A soak of a single static page for 8 h proves almost nothing.

### 7.3 Leak vs cache — the decision rule

Chromium's footprint legitimately rises for hours as caches fill and then plateaus. A rule that flags any growth is useless.

1. **Discard the first 2 h** as cache-fill.
2. On the remaining 6 h, fit OLS of `phys_footprint` on time, per process and for the tree total.
3. Declare a **leak** iff *all three* hold:
   - slope > **1 MiB/h** for the tree total (or > 256 KiB/h for a single process), **and**
   - the 95 % CI lower bound on the slope is > 0, **and**
   - Mann–Kendall trend test on the 6 h series gives p < 0.01 (non-parametric — footprint series are not Gaussian and have step changes).
4. Sanity gate: `mean(last hour) / mean(hour 3) > 1.05`. A statistically significant 200 KiB/h slope on a 2 GiB process is noise, not a leak.
5. **Separately**, flag a hard fail if tree `phys_footprint` ever exceeds **4 GiB**, or if the machine enters swap (`vm_stat` swapouts increasing), regardless of slope.

Rationale for 1 MiB/h: over an 8 h workday it is 8 MiB — genuinely invisible. At 10× that (10 MiB/h = 80 MiB/day) a week-long tmux session becomes a problem. The threshold sits an order of magnitude below the level of user harm.

### 7.4 Sampler

```sh
# bench/soak-sample.sh — external sampler, so a hung engine still produces data.
# Usage: soak-sample.sh <root-pid> <out.jsonl> <interval-s>
ROOT=$1; OUT=$2; IVL=${3:-30}
while kill -0 "$ROOT" 2>/dev/null; do
  TS=$(./target/release/bgbench now-ns)
  # pgrep -P is not recursive; walk the tree.
  PIDS=$(./target/release/bgbench tree "$ROOT")
  for p in $PIDS; do ./target/release/bgbench rusage "$p"; done \
    | jq -c --arg ts "$TS" '. + {t_ns:$ts}' >> "$OUT"
  /usr/bin/vm_stat | jq -R -s -c --arg ts "$TS" '{t_ns:$ts, vm_stat:.}' >> "$OUT"
  sleep "$IVL"
done
```

Run the whole soak under `caffeinate -dimsu`, on **AC power**, and assert `sleep_skew_ns` is unchanged at the end (§1.3). A soak that slept is void.

### 7.5 Complementary leak tools

- `xctrace record --template 'Leaks' --attach <pid>` — works headless, no sudo *[measured: `xctrace record`/`export` verified against `/bin/sleep`]*. Finds unreachable-allocation leaks in the Rust process. Will be noisy against Chromium's arenas.
- `leaks <pid>` — same caveat, but zero setup.
- `MallocStackLogging=1` + `malloc_history` for attributing a growth to a call site. High overhead; use only after a leak is confirmed, never during the measurement soak.
- `heap <pid>` for a class/size breakdown at a point in time.

---

## 8. Statistical rigor

### 8.1 Sample counts and warmup

| Measurement | n | Warmup discarded | Repetitions |
|---|---|---|---|
| Input→t4 latency | 2000 | 200 | 3 sessions, different days |
| Frame pacing | 60 s @ target fps (≈3600 frames) | first 3 s | 3 |
| Encode microbench | divan/criterion default (≥100 samples) | tool-managed | 1 |
| Cold start | 30 | 3 | 2 |
| Warm start | 100 | 10 | 2 |
| Idle | 120 s window | 3 s of no-paint quiescence | 3 |
| Soak | 8 h @ 30 s = 960 samples | first 2 h (240 samples) | 1 per release |

### 8.2 Statistics

- **Report p50, p95, p99, p99.9, min, max. Never report a mean alone, and never report a mean without a standard deviation next to it.** Latency distributions are right-skewed and multimodal (60 Hz phase + GC + thermal); the mean is a number that describes no actual frame.
- Use **`hdrhistogram` 7.6.0** (MIT/Apache-2.0) for recording — constant-time, allocation-free `record()`, exact percentiles at configurable precision, and it merges across threads and runs. Configure `Histogram::<u64>::new_with_bounds(1_000, 60_000_000_000, 3)` (1 µs floor, 60 s ceiling, 3 significant digits).
- **Publish the full HDR percentile CSV**, not just the five headline numbers. `hdrhistogram` emits the standard format that plots directly on a log-percentile axis, which is the only chart on which a tail regression is visible.
- **Comparing two builds:** do **not** use a t-test on latency samples (non-normal, autocorrelated). Use a **two-sided Mann–Whitney U** on the raw samples for the location shift, plus a **bootstrap 95 % CI on the p99 difference** (10 000 resamples). A change is a regression only if the p99 CI excludes zero *and* the shift exceeds the run-to-run variance established in §8.3.
- **Effect size gate:** with n=2000, trivially small differences become "significant". Require |Δp50| > 0.5 ms or |Δp99| > 2 ms *in addition to* statistical significance before calling anything a regression.

### 8.3 Establishing the noise floor first

**Before any A/B comparison is trusted, run the identical build against itself three times on three different days and publish the resulting spread.** If build-vs-itself p99 varies by 4 ms, then a 3 ms "improvement" is not real. This step is skipped in most performance work and is the reason most performance work is wrong. Bake it into the harness as `bgbench noise-floor` and refuse to print a comparison verdict if a noise-floor run older than 30 days is not on file.

### 8.4 Reproducibility controls (this machine specifically)

The M4 MacBook Air is **fanless**. Sustained load throttles, and throttling is a function of ambient temperature, chassis contact, and prior workload. Additionally, `pmset -g ps` showed **18 % battery, discharging** during this session *[measured]* — macOS reduces performance on low battery.

Mandatory preconditions, checked and recorded by the harness, run refused if violated:
1. **AC power connected, battery ≥ 80 %.** `pmset -g ps` parsed, recorded in the manifest.
2. **`pmset -g therm` clean before and after.** *[measured: works without sudo; reports "No thermal warning level has been recorded"]*. Any recorded thermal or performance warning voids the run.
3. **≥ 90 s idle cooldown between runs**, and **interleave conditions** (A,B,A,B,…) rather than blocking (A×30 then B×30). Blocked designs alias thermal drift directly onto the condition — this is the single most common way to fabricate a performance result on a fanless laptop.
4. `caffeinate -dimsu` wrapping everything.
5. Close other applications; record `ps -A -o pcpu | awk '{s+=$1} END {print s}'` before the run and abort if system-wide CPU exceeds 15 %.
6. Fixed `TERM`, terminal window size, font, and font size — cell dimensions change bytes/frame and therefore §0.1's write cost. Record the pixel cell size from `TIOCGWINSZ` (`ws_xpixel`/`ws_ypixel`).
7. Same Electron build (43.2.0 / Chromium 150.0.7871.129), pinned, hash recorded.

### 8.5 CI integration

Full latency/pacing runs are too noisy for per-PR CI on shared hardware. Structure:
- **Per PR:** `bgbench encode` microbenchmarks only (divan, hermetic, no PTY, no engine) + a 30 s idle check. These are stable enough to gate on.
- **Nightly, on this dedicated M4:** the full suite, results appended to a time series, alerting on a **7-day rolling p99 regression** rather than on any single night.
- **Per release:** the 8 h soak and the §3.6 camera calibration.

---

## 9. Exact instrumentation, per metric

### 9.1 Rust: the write path, instrumented

```rust
use crate::{ev, trace::EvKind::*, clock};

const CHUNK: usize = 1024; // [measured] 3.3x faster than 8K/64K on a macOS PTY. Tunable.

pub fn present(seq: u64, frame_bgra: &[u8], out: &mut std::fs::File, enc: &mut Encoder) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    ev!(EncodeBegin, seq, frame_bgra.len() as u64);
    let payload = enc.encode(frame_bgra);          // -> &[u8], into a reused buffer, no alloc
    ev!(EncodeEnd, seq, payload.len() as u64);

    ev!(WriteBegin, seq, payload.len() as u64);
    let fd = out.as_raw_fd();
    let mut off = 0usize;
    while off < payload.len() {
        let n = (payload.len() - off).min(CHUNK);
        let r = unsafe { libc::write(fd, payload[off..].as_ptr() as *const _, n) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted { continue; }
            if e.kind() == std::io::ErrorKind::WouldBlock {
                // Non-blocking fd: this is backpressure, and it is DATA. Count it.
                ev!(QueueDepth, seq, u64::MAX); // sentinel: EAGAIN stall
                poll_writable(fd)?;
                continue;
            }
            return Err(e);
        }
        off += r as usize;
    }
    ev!(WriteEnd, seq, payload.len() as u64);
    Ok(())
}
```

Note `writev` is *not* used: with `CHUNK = 1024` the whole point is small writes, and `writev` would coalesce them back into the slow regime. Sweep `CHUNK` with `bgbench sweep --param chunk_bytes` and re-derive per terminal — the optimum is a property of the *terminal's* read loop, not just the kernel's, so Ghostty and iTerm2 may differ.

### 9.2 Node: the engine side, instrumented

```js
const { ev } = require('./bench/trace');
const K = require('./bench/kinds');

wc.on('paint', (details, dirtyRect, image) => {
  const seq = ++frameSeq;
  ev(K.PaintEvent, seq, dirtyRect.width * dirtyRect.height);
  const bmp = image.getBitmap();           // BGRA, w*h*4, non-strided [ADR-0001]
  ev(K.PaintBitmap, seq, bmp.length);      // getBitmap() is a COPY. This delta is the copy cost.
  enqueueToTui(seq, dirtyRect, bmp);
});

// Input dispatch. NOTE: "The BrowserWindow containing the contents needs to be focused
// for sendInputEvent() to work" [doc: electronjs.org/docs/latest/api/web-contents].
// With show:false OSR windows this is a real hazard -- assert delivery via the pixel-stamp
// (§3.4) rather than assuming the event landed.
function dispatchKey(seq, key) {
  wc.sendInputEvent({ type: 'keyDown', keyCode: key });
  wc.sendInputEvent({ type: 'char',    keyCode: key });
  wc.sendInputEvent({ type: 'keyUp',   keyCode: key });
  ev(K.InDispatched, seq, 0);   // t1
}
```

Chromium flags worth a *characterization* run (not shipping defaults), all verified against primary source:

| Flag | Source file | Effect |
|---|---|---|
| `--disable-frame-rate-limit` | `components/viz/common/switches.cc` (`kDisableFrameRateLimit`) *[doc]* | removes begin-frame limiting in cc + display scheduler; implies `--disable-gpu-vsync`. Use to find the engine's ceiling. |
| `--run-all-compositor-stages-before-draw` | `components/viz/common/switches.cc` (`kRunAllCompositorStagesBeforeDraw`) *[doc]* | disables pipelining; each stage completes before the next. Removes pipelining jitter → deterministic frames for A/B comparison. |
| `--disable-gpu-vsync` | `ui/gl/gl_switches.cc` (`kDisableGpuVsync`) *[doc]* | disables vsync on present/SwapBuffers. |
| `--show-fps-counter` | `cc/base/switches.cc` (`kShowFPSCounter`) *[doc]* | draws Chromium's own FPS HUD **into the frame** — cross-checks our `paint_fps` against the compositor's own count. |
| `--show-surface-damage-rects` | `cc/base/switches.cc` (`kShowSurfaceDamageRects`) *[doc]* | visualizes damage; validates `damage_area_ratio` visually. |
| `--enable-gpu-benchmarking` | `cc/base/switches.cc` (`kEnableGpuBenchmarking`) *[doc]* | exposes `chrome.gpuBenchmarking` in-page. |

`--disable-frame-rate-limit` is **UNVERIFIED in Electron OSR mode specifically** — OSR frame production goes through `setFrameRate()`/`BeginFrame` control rather than the display scheduler, so the flag may be inert. Test before drawing conclusions from it.

Set via `app.commandLine.appendSwitch('disable-frame-rate-limit')` **before** `app.whenReady()`.

### 9.3 Offline decoder

```rust
// bgbench decode run-*.bgtrace --out summary.json
// 1. mmap every file, reinterpret as &[Ev] (repr(C), 32B, same-endian, same machine).
// 2. Concatenate, stable-sort by t_ns. Files from different pids interleave correctly
//    because §1 guarantees one clock.
// 3. Group by seq -> per-frame/per-input phase table.
// 4. Feed each phase delta into its own hdrhistogram::Histogram<u64>.
// 5. Emit summary.json + per-phase percentile CSVs + the drop-cause histogram (§4.2).
let f = std::fs::File::open(p)?;
let m = unsafe { memmap2::Mmap::map(&f)? };
assert_eq!(m.len() % 32, 0, "truncated trace: flusher died mid-write");
let evs: &[Ev] = unsafe {
    std::slice::from_raw_parts(m.as_ptr() as *const Ev, m.len() / 32)
};
```

`memmap2` is MIT/Apache-2.0.

### 9.4 Terminal capability + ACK probe (run before every session; result → manifest)

```sh
#!/bin/zsh
# bench/term-probe.sh -- must run INSIDE the terminal under test, on its real tty.
# Apple Terminal (this shell) will fail the graphics probes; that is the expected result.
exec 3<>/dev/tty
old=$(stty -g); stty raw -echo min 0 time 3   # time is in 1/10 s -> 300 ms timeout

probe() {  # $1 = label, $2 = bytes to send (printf-escaped)
  printf "$2" >&3
  r=$(dd bs=1 count=256 <&3 2>/dev/null | od -An -tx1 | tr -d ' \n')
  print -r -- "{\"probe\":\"$1\",\"reply_hex\":\"$r\"}"
}

probe xtversion   '\033[>0q'
probe primary_da  '\033[c'
probe dsr         '\033[5n'                                  # expect 1b5b306e
probe decrqm_2026 '\033[?2026$p'                             # synchronized output support
probe kitty_gfx   '\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\033\\'  # expect 1b5f4769...3b4f4b1b5c
probe kitty_kbd   '\033[?u'

stty "$old"; exec 3>&-
```

Reply byte references:
- DSR reply: `1b 5b 30 6e` (`ESC [ 0 n`)
- Kitty graphics OK: `1b 5f 47 69 3d 33 31 3b 4f 4b 1b 5c` (`ESC _ G i=31;OK ESC \`)
- XTVERSION reply: `DCS > | <name> ST`, i.e. begins `1b 50 3e 7c`
- DECRPM for 2026: `ESC [ ? 2026 ; Ps $ y`, i.e. begins `1b 5b 3f 32 30 32 36 3b`

**Status of these probes against Ghostty 1.3.1 / iTerm2 3.6.9: UNVERIFIED.** The current session is Apple Terminal 465 (`TERM_PROGRAM=Apple_Terminal`, `TERM_PROGRAM_VERSION=465` *[measured]*), which supports none of the graphics protocols. iTerm2 is installed at `~/Applications/iTerm.app`, not `/Applications` *[measured]*. Ghostty is at `/Applications/Ghostty.app`, version 1.3.1 *[measured]*; on macOS it must be launched with `open -na Ghostty.app --args ...`, not from the CLI *[measured, from `ghostty --help`]*. Ghostty exposes no benchmarking `+action` *[measured: the full action list is version/help/list-fonts/list-keybinds/list-themes/list-colors/list-actions/ssh-cache/edit-config/show-config/validate-config/show-face/crash-report/boo/new-window]* — there is no built-in frame-timing hook to borrow.

Run:
```sh
open -na Ghostty.app --args -e /path/to/bench/term-probe.sh
```
and have the script tee its JSON to a file, since stdout dies with the window.

### 9.5 The exact probe programs used to produce §0

All five are ~40 lines of C, compile with plain `clang -O2`, no dependencies. They should be committed to `benchmarks/probes/` verbatim so §0's numbers are re-derivable on any future machine:

- `tb.c` — timebase, clock resolution, per-call cost, `getrusage`
- `clocks.c` — the five-clock comparison that proves §1.2
- `ru.c` — `proc_pid_rusage(RUSAGE_INFO_V6)` field dump
- `wr.c` — pipe throughput reference
- `pty3.c` — **the PTY latency matrix of §0.1; the most important program in this list**

`pty3.c` in full (it is the one whose result drives the architecture):

```c
#define _DARWIN_C_SOURCE 1
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <string.h>
#include <termios.h>
#include <pthread.h>
static inline uint64_t now(void){ return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }
static int master_fd; static volatile int stop_ = 0;
static void* drain(void*a){ char b[1<<16]; while(!stop_){ ssize_t r=read(master_fd,b,sizeof b); if(r<=0) break; } return 0; }
static int cmp(const void*a,const void*b){ uint64_t x=*(uint64_t*)a,y=*(uint64_t*)b; return x<y?-1:x>y; }
int main(void){
  int m = posix_openpt(O_RDWR|O_NOCTTY); grantpt(m); unlockpt(m); master_fd = m;
  int s = open(ptsname(m), O_RDWR|O_NOCTTY);
  struct termios t; tcgetattr(s,&t); cfmakeraw(&t); t.c_oflag &= ~OPOST; tcsetattr(s,TCSANOW,&t);
  pthread_t th; pthread_create(&th,0,drain,0);
  size_t sizes[4] = { 6912064, 1728000, 400000, 40000 };
  size_t chunks[3] = { 1024, 8192, 65536 };
  char *buf = malloc(8<<20); memset(buf,'A',8<<20);
  for (int z=0; z<4; z++) for (int k=0; k<3; k++) {
    size_t N=sizes[z], chunk=chunks[k]; int iters=51; uint64_t sam[51];
    for (int i=0;i<iters;i++){ uint64_t a=now(); size_t w=0;
      while(w<N){ size_t c=(N-w<chunk)?N-w:chunk; ssize_t r=write(s,buf+w,c); if(r>0)w+=r; else break; }
      sam[i]=now()-a; }
    qsort(sam+1, iters-1, sizeof(uint64_t), cmp);   /* discard sample 0 as warmup */
    printf("N=%7zu chunk=%5zu  p50=%7.3f ms  p95=%7.3f ms  p99=%7.3f ms\n", N, chunk,
      sam[1+(iters-1)/2]/1e6, sam[1+(int)((iters-1)*0.95)]/1e6, sam[iters-1]/1e6);
  }
  stop_=1; close(s); return 0;
}
```

### 9.6 Encode microbenchmarks (hermetic, CI-safe)

Use **divan 0.1.21** (MIT/Apache-2.0) — lower ceremony than criterion and it reports allocation counts, which matters because an allocation in the encode path is itself a defect:

```rust
#[divan::bench(args = [(1440,900), (720,450), (320,200)])]
fn encode_kitty_rgba(b: divan::Bencher, (w,h): (u32,u32)) {
    let src = vec![0u8; (w*h*4) as usize];
    let mut enc = Encoder::kitty(w, h);
    b.counter(divan::counter::BytesCount::new(src.len()))
     .bench_local(|| divan::black_box(enc.encode(divan::black_box(&src))));
}
```

Benchmark separately, because these are separately optimizable and separately regressible: BGRA→RGB channel swap, downsample/resample to cell grid, half-block / sextant quantization, base64 encode (compare `base64-simd` 0.8.0, MIT), zlib for kitty `o=z`, and total escape-sequence assembly.

---

## 10. Pre-registered budgets

Registering these *before* measuring prevents the retroactive-goalpost failure mode.

| Metric | Target | Hard fail | Basis |
|---|---|---|---|
| Controllable latency t4−t0, p50 | ≤ 16 ms | > 33 ms | one 60 Hz frame |
| Controllable latency t4−t0, p99 | ≤ 33 ms | > 66 ms | two frames |
| `present_fps`, `canvas-corner` @1440×900 | ≥ 55 | < 30 | near-native |
| `present_fps`, `canvas-full` @1440×900 | ≥ 24 | < 15 | §0.1 makes 60 implausible at full damage |
| `write_ms` p99 | ≤ 16 ms | > 33 ms | must not exceed one frame |
| `max(queue_depth)` under steady load | ≤ 2 | > 3 | unbounded latency otherwise |
| Idle CPU, whole tree, 120 s | < 0.5 % of one core | > 2 % | §6.3 |
| Idle wakeups/s | < 20 | > 100 | battery |
| Cold start to first pixel, local page | ≤ 1200 ms | > 2500 ms | ADR-0001 measured 2104 ms to first paint on remote `example.com` — network-inclusive, so a local-page target of 1200 ms is a genuine tightening |
| Warm new-tab to first pixel | ≤ 250 ms | > 600 ms | must feel instant |
| 8 h soak footprint slope | < 1 MiB/h | > 10 MiB/h | §7.3 |
| Peak tree `phys_footprint` | ≤ 1.5 GiB | > 4 GiB | must coexist with a real workstation |

---

## 11. Licenses of everything referenced

| Thing | License | How we use it |
|---|---|---|
| Kitty graphics protocol | Protocol **specification**, GPL-3.0 implementation | We implement from the published spec only. **Do not read or copy kitty's source.** Behavior-from-docs only. |
| iTerm2 inline images protocol (OSC 1337) | Protocol spec, published on iterm2.com | Spec only. |
| Ghostty 1.3.1 | MIT | Reference behavior; no code copied. |
| Electron 43.2.0 | MIT | Runtime dependency (Chromium beneath it is BSD-3-Clause). |
| Chromium switch names | BSD-3-Clause | We use flag *strings*, not code. |
| libuv | MIT | Facts about `uv_hrtime` only; Node links it, we do not vendor it. |
| Chrome DevTools Protocol spec | BSD-3-Clause | Spec only; §3.4 avoids CDP on the hot path. |
| `hdrhistogram` 7.6.0 | MIT OR Apache-2.0 | Direct dependency |
| `divan` 0.1.21 | MIT OR Apache-2.0 | Dev-dependency |
| `criterion` 0.8.2 | Apache-2.0 OR MIT | Dev-dependency (alternative to divan) |
| `base64-simd` 0.8.0 | MIT | Candidate dependency; benchmark before adopting |
| `memmap2` | MIT OR Apache-2.0 | Dev/tooling dependency |
| `quanta` 0.12.6 | MIT | **Rejected** — §1.4 is 20 lines and avoids a dependency on the most safety-critical code in the harness |
| `xctrace` / Instruments 26.0 | Apple, Xcode license | Local developer tooling only; never redistributed, never in CI images |

All crate versions and licenses *[measured]* via the crates.io API this session.

---

## 12. Explicitly UNVERIFIED — do not build on these without testing

1. **Ghostty 1.3.1 and iTerm2 3.6.9 responses to every probe in §9.4.** All escape-sequence *syntax* above is from primary specs; none of it has been round-tripped against these two terminals on this machine. The current shell is Apple Terminal, which cannot test any of it.
2. **Why 1 KiB PTY chunks beat 64 KiB by 3.3×.** The effect is measured and reproducible; the mechanism (macOS clist / `TTYHIWAT` sleep-wake) is a hypothesis. Confirm with `sudo dtrace -n 'fbt::ptcwrite:entry,fbt::ttwrite:entry { @[probefunc] = count(); }'` before generalizing to other chunk sizes or other terminals.
3. **Whether a *real terminal* drains its PTY as fast as `pty3.c`'s dedicated reader thread.** §0.1 is an upper bound on achievable throughput. A terminal that parses escape sequences between reads will be slower, possibly much slower. Re-run the matrix against a live Ghostty and iTerm2 and treat *those* numbers as the real budget.
4. **`--disable-frame-rate-limit` efficacy in Electron OSR mode** (§9.2).
5. **`process.hrtime.bigint()` cost inside the `paint` handler on Node 24.11.1** (§2.4) — BigInt allocation may be too expensive for the hot path.
6. **Units of `ri_billed_energy` / `ri_serviced_energy`** (§6.4).
7. **Whether `/usr/sbin/purge` requires sudo on macOS 26.1** (§5.3) — the script handles both, but the analysis must know which happened.
8. **Baseline comparison numbers for §5.4** — nothing has been measured for Ghostty, Safari, or bare Electron startup.
9. **ScreenCaptureKit's actual achievable capture cadence** and its perturbation of the system under test (§3.6).
10. Whether the §3.4 pixel-stamp survives the renderer's own scaling at non-1.0 `deviceScaleFactor`. Force `deviceScaleFactor: 1` in benchmark runs and verify by reading back a known counter value.
11. **Whether Darwin's `CLOCK_MONOTONIC_RAW` (id 4) includes system sleep.** It read identically to `CLOCK_UPTIME_RAW` (id 8) here *[measured]*, but this machine has not slept since boot (`mach_continuous_time() − mach_absolute_time() == 0` *[measured]*), so the test was vacuous. §1.4 sidesteps the question entirely by using libuv's exact expression; do not reintroduce `CLOCK_MONOTONIC_RAW` into the clock path without closing this first. Test procedure: record both clocks, close the lid for 60 s, reopen, record again, compare deltas.

---

## 13. Build order

1. `crates/bg-bench/src/clock.rs` (§1.4) + a test asserting agreement with Node to < 1 µs. Nothing else is trustworthy until this exists.
2. `trace.rs` ring + flusher (§2.3) + `decode` (§9.3).
3. Commit the five C probes to `benchmarks/probes/` (§9.5); wire `pty3` output into CI as a machine-characterization step.
4. `bgbench sweep --param chunk_bytes` against real Ghostty and real iTerm2. **This closes UNVERIFIED #3, which is the largest open risk in the whole plan.**
5. §9.4 terminal probe → manifest.
6. `bgbench idle` and `bgbench soak` (cheapest to build, longest to run — start them early and let them run overnight).
7. Pixel-stamp (§3.4) and the full latency chain.
8. `bgbench noise-floor` (§8.3) before anyone is allowed to publish a comparison.
9. Camera calibration (§3.6) once, at first release candidate.
