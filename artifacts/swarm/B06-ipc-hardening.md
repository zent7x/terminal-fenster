# B06 — Wire Protocol Review & IPC Hardening

**Status:** review complete, implementation spec
**Date:** 2026-07-31
**Host:** macOS 26.1, Apple M4 arm64. rustc 1.93.0, Node v24.11.1, Electron 43.2.0 / Chromium 150.
**Scope:** `crates/bg-proto/src/lib.rs`, `apps/engine/src/main.js`. `apps/cli/src/main.rs` is read-only context (it is the only consumer of `bg-proto` and half of every finding lands there).

**Reviewed file state** — these files are under concurrent edit by other agents, so all line numbers below are pinned to these exact revisions:

| File | lines | md5 |
|---|---:|---|
| `crates/bg-proto/src/lib.rs` | 284 | `edbb4c0b2e74e960857f2fc687210fe4` |
| `apps/engine/src/main.js` | 309 | `1520d7ab86e4c69e76508bd6d6bab2ce` |
| `apps/cli/src/main.rs` | 1039 | `bceb2a511097d93ea17ac90b94fcb077` |

**I wrote no code into the repo.** Every snippet here is a proposal for the commander. Where a snippet is marked VERIFIED it was compiled and executed standalone in
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/b06/`
(`proposed.rs`, `outq.rs`, `desync2.rs`, `readerbench.rs`, `overflow.rs`, `thru.rs`, `fdpass.rs`, `proposed-framer.js`, `framer.test.js`, `concat.js`). Those files are outside the repo; copy them in if you want them as tests.

---

## 0. Method, and what "verified" means here

Electron cannot run under the agent Bash sandbox (`bootstrap_look_up ... Permission denied`), and the machine is at a lock screen, so nothing here relies on a screenshot or on a live browser. Instead every claim is either (a) read directly off the source, or (b) reproduced against a **standalone harness that replicates the exact code under review** — the same `frame_message` bytes, the same `set_nonblocking(true)` + `write_all` call shape, the same `Buffer.concat` reader loop, driven over a real `AF_UNIX` `SOCK_STREAM` pair and a real `node` child process.

That substitution is honest for this subsystem specifically, because the protocol layer has no Chromium in it: it is a socket, a length prefix, and two framers. What it cannot cover is Electron-specific runtime behaviour (`net.Socket({fd})` inside Electron's main process, `webContents.sendInputEvent` throughput). Those are called out in §11.

The current protocol is a good starting point. The framing choice is right (length-prefixed, not delimited — the existing test `binary_payload_with_newlines_survives` is exactly the test that matters), the coalescing backpressure design in `flushFrame` is the correct trade for interactivity, and the doc comments are unusually honest about why. The problems below are all of the "this is v0 and has not met a hostile or merely slow peer yet" kind.

---

## 1. Findings

| # | Severity | Finding | Evidence |
|---|---|---|---|
| **F1** | **Critical** | Core→engine writes are fire-and-forget on a non-blocking socket. Any message larger than `SO_SNDBUF` (**8192 B**, measured) is written partially and the error discarded → **permanent stream desync**. A >8 KB paste kills the session and eats the shutdown command. | §2.1, `desync2.rs` |
| **F2** | **High** | No version negotiation of any kind. No magic, no version byte, no handshake. The `ready` event is the only version-ish signal and the core never parses it. `FRAME_HEADER_LEN = 32` is a magic number duplicated in two languages with no agreement test. | §2.2 |
| **F3** | **High** | No message-size cap and no buffered-bytes cap, in either direction. A 4 GiB length prefix is accepted by both framers. Core reaches 4 GiB resident in **14–24 s** at measured socket throughput. | §2.3, `thru.rs` |
| **F4** | **High** | The listener accepts the **first** connection with **no authentication**, and the socket path is in `argv`. A same-uid local process that wins the race becomes "the engine": it can drive terminal-escape injection (A09 TB3) and it receives every keystroke typed into the browser, including passwords. | §2.4 |
| **F5** | **Medium** | `FrameHeader::expected_payload()` overflows `usize` and **silently wraps in release builds** — `w=h=2^31` yields `0`. No geometry bound, no dirty-rect bound, and the `format` field is transmitted but never checked. | §2.5, `overflow.rs` |
| **F6** | **Medium** | Backpressure exists engine→core only, and is measured at the **socket**, not at the terminal — which A10 §0.1 identifies as the actual bottleneck. No ACK, no credit, no staleness bound on the coalesced frame. | §2.6 |
| **F7** | **Medium** | No reconnect semantics at all. The listener is dropped after `accept()`, so reconnection is impossible by construction. A renderer crash is recorded in `Status::crashed` and then **never displayed and never recovered**. | §2.7 |
| **F8** | **Medium** | No liveness check. A wedged engine main process is indistinguishable from an idle page: socket open, no frames, no error, poll loop spins forever. | §2.8 |
| **F9** | **Medium** | Paste is sent as one unbounded `text` field and expanded to one synchronous `sendInputEvent` **per character** in the engine. 10 MB paste = 10 M synchronous IPC calls. No attacker required. | §2.9 |
| **F10** | **Medium (perf)** | The protocol shape forces ~**2.0–2.3 ms/frame** of pure memcpy (1.07–1.14 ms JS `Buffer.concat` ×2, 0.87 ms Rust `to_vec`+`drain`) — **12–14 % of the measured 16.65 ms p50 frame budget**, burned on nothing. | §2.10, `concat.js`, `readerbench.rs` |
| **F11** | **Low** | `json_get_str` returns the wrong value for nested objects (confirmed), and mangles surrogate-pair escapes into `U+FFFD U+FFFD` (confirmed). **Not** injectable via page titles (confirmed safe — see below). | §2.11 |
| **F12** | **Low** | Unknown message types are silently ignored on both sides, so a future version bump degrades to "half working" instead of failing cleanly. JS reader retains whole ArrayBuffers via `subarray` (**512× measured**) and is O(n²) on fragmented input. | §2.12 |
| **F13** | **Low** | `navigate` / `resize` / `input` command arguments are unvalidated in the engine: no URL scheme allowlist, no dimension bound, no integer checks. | §2.13 |

---

## 2. Findings in detail

### 2.1 F1 (Critical) — partial write desync on the core→engine path

`apps/cli/src/main.rs:431` sets the stream non-blocking; `main.rs:436-439` then writes commands like this:

```rust
fn send(&mut self, json: &str) {
    let msg = proto::frame_message(proto::T_COMMAND, json.as_bytes());
    let _ = self.stream.write_all(&msg);      // <-- error discarded
}
```

`Write::write_all` does not retry `WouldBlock`, and std explicitly documents that on error *"it is unspecified how many bytes it has written"*. Two distinct failure modes follow, and I measured both.

**Small messages — silent input loss.** With 64-byte mouse-move messages, macOS returns a clean `EWOULDBLOCK` once the 8192-byte send buffer fills, after 127 whole messages. The message is simply dropped and nobody is told. A dropped `mouseUp` mid-drag leaves the page in a stuck-drag state; a dropped `keyUp` leaves a modifier latched.

**Large messages — permanent desync.** The paste path (`main.rs:645-652`) puts the entire clipboard into one JSON message. Measured:

```
SO_SNDBUF = 8192 bytes (macOS unix socketpair default)
one paste message = 200071 bytes
write_all result = Some(WouldBlock)   <-- discarded by `let _ =`
engine received 8192 of 200071 bytes -> 191879 bytes of a single message are missing
engine header says len=200066, has 8187 payload bytes -> stalls, needs 191879 more
core then sent {"t":"quit"} (17 bytes); engine now has 8204 payload bytes of a 200066-byte
  message -> quit is EATEN, not executed
```

So: **paste anything over ~8 KB and the browser stops responding to all input forever**, and `Session::shutdown`'s `{"t":"quit"}` is consumed as payload of the truncated message. Shutdown survives only because of the `child.kill()` fallback after the 1500 ms deadline.

This needs no attacker and no unusual terminal. It is one ⌘V away.

The fix is a byte-oriented outbound queue drained on `POLLOUT`, so that partial writes resume at the exact byte and only *whole messages* are ever dropped — see §5.1. Verified against the identical 200 KB paste:

```
queued 200088 bytes across 2 messages
fully drained after 102 POLLOUT passes, dropped=0
engine reframed 2 whole messages: [(10, 200066), (10, 12)]
last message body = {"t":"quit"}
PASS: 200 KB paste no longer desynchronises the stream; quit still executes
```

### 2.2 F2 (High) — no version negotiation

There is nothing on the wire that identifies the protocol. The first byte the core ever sees is a message type, and the first byte the engine ever sees is a message type. Concretely:

- **No magic.** Any process that connects and writes anything is treated as the engine (see F4). A wrong-version engine, a stale binary from a half-finished `cargo install`, or an unrelated program that happened to open the socket all produce *mis-framing*, not an error.
- **No version integer.** `T_FRAME=1`, `T_EVENT=2`, `T_COMMAND=10` are hardcoded in `bg-proto/src/lib.rs:11-13` and again in `apps/engine/src/main.js:30-32`. Nothing checks they agree.
- **`FRAME_HEADER_LEN = 32`** (`lib.rs:16`) is duplicated as a bare `Buffer.allocUnsafe(32)` at `main.js:87` with eight hand-written `writeUInt32BE` offsets. Add one field to that header on the JS side and the Rust side silently reads `width` out of `seq`'s old slot. The failure is not a parse error; it is a plausible-looking geometry that drives `expected_payload()` (see F5).
- **The one signal that exists is thrown away.** The engine sends `{"t":"ready","electron":...,"chrome":...}` at `main.js:292-298`. `Status::apply_event` has arms for `title`/`url`/`loading`/`crash` and nothing else — `ready` falls into `_ => {}`. It is written to the log file and never inspected. `Session::start` returns as soon as `accept()` succeeds; it never waits for `ready`, so there is also no point at which a version *could* currently be checked.
- **20 of the 32 header bytes are write-only.** `grep` for `dirty_x`, `dirty_w`, `.format`, `h.seq` across the core returns **zero** usages. `seq`, the full dirty rect, and `format` are serialised every frame and read by nobody. That is 20 bytes × 60 fps of dead weight, and — worse — it means `format` provides no protection: if the engine ever emits format 1, the core still calls `bgra_to_rgb` unconditionally (`main.rs:837`).

### 2.3 F3 (High) — unbounded allocation

**Rust side.** `MessageReader::next_message` (`lib.rs:81-93`) reads a `u32` length with no ceiling and `feed` (`lib.rs:72-74`) appends with no ceiling:

```rust
let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
if self.buf.len() < 5 + len { return None; }          // just keep buffering, forever
let payload = self.buf[5..5 + len].to_vec();          // up to 4 GiB copy
```

A declared length of `0xFFFFFFFF` makes the reader accumulate until the process dies. Measured `AF_UNIX` ingest with the core's own 1 MiB read size (`main.rs:455`) is **176–304 MB/s**, so a hostile or wedged peer drives the core to 4 GiB resident in **14–24 seconds**. There is no cap distinguishing a frame (megabytes, legitimate) from an event (should never exceed a few KB).

**JS side.** `attachReader` (`main.js:266-286`) has the same shape, and Node offers no backstop: on this host `require('buffer').constants.MAX_LENGTH` is **9007199254740991** (2⁵³−1), so `Buffer.concat` will not throw at 4 GiB — it will simply exhaust the machine.

The right ceiling is *derived*, not guessed: the core knows the viewport it asked for, so `max_frame = w × h × 4 + 32`. Anything larger is definitionally not a frame it requested. JSON messages get their own, much smaller cap. Both are enforced **on the length prefix**, before a single payload byte is buffered.

### 2.4 F4 (High) — first connection wins, with no authentication

`main.rs:399` binds the listener, `main.rs:404` passes the path in `argv` (`--bg-socket=…`), and `main.rs:415-430` takes the first connection that arrives:

```rust
let stream = loop {
    match listener.accept() {
        Ok((s, _)) => break s,        // whoever gets here first IS the engine
        ...
```

The 0700 directory (`main.rs:393-396`) correctly stops other *users*. It does not stop the same user — which is precisely the threat A09 §4.3 identifies: a malicious npm `postinstall`, a compromised editor extension, or an infostealer all run as the same uid, and can enumerate `$TMPDIR` or read `--bg-socket=` out of `ps`. Whoever connects first gets two capabilities:

1. **A terminal-escape injection primitive.** They now feed `T_EVENT` messages that become `status.title` / `status.url`. `sanitize_for_terminal` is the only thing standing between them and A09's OSC 52 chain — a single sanitizer, doing double duty as both the web-content boundary and the IPC boundary.
2. **A keystroke sink.** Far worse. The core forwards every decoded key to whoever is on the other end of that socket (`Session::handle_event` → `send`). That includes everything typed into a login form. The impersonator does not need to render anything convincing; the user is typing into what they believe is their browser.

`SecCodeCheckValidity` on the peer audit token (A09 §4.3) would fix the authentication, but there is a strictly better answer that deletes the whole category: **use a `socketpair` and pass the connected fd to the child**. No path, no listener, no `accept()`, no race, no permission dance, no cleanup. Verified working from Rust to Node v24.11.1 (§6): the child gets a socket whose `getsockname()` is empty — it has no filesystem presence for anything to connect to.

### 2.5 F5 (Medium) — integer overflow in `expected_payload()`

`lib.rs:51-53`:

```rust
pub fn expected_payload(&self) -> usize {
    self.width as usize * self.height as usize * 4
}
```

`width` and `height` are attacker-controlled `u32`s from the wire. On 64-bit this product can exceed `usize::MAX` and, in a release build, **wraps silently**. Measured:

```
usize::BITS = 64
w=0x80000000 h=0x80000000  wrapping=                     0  checked=None  OVERFLOW=true
w=0x80000000 h=0x80000002  wrapping=           17179869184  checked=None  OVERFLOW=true
w=0xffffffff h=0xffffffff  wrapping=  18446744039349813252  checked=None  OVERFLOW=true
w=0xc0000000 h=0x55555556  wrapping=            8589934592  checked=None  OVERFLOW=true
```

The `w=h=2^31 → 0` case is the reachable one. It sails through the truncation guard at `main.rs:834` (`pixels.len() < 0` is false), producing an empty slice and poisoning `self.page_w`/`self.page_h` with 2³¹.

**Today this is contained, not exploited**: `bgra_to_rgb` calls `out.clear()` first (`kitty.rs:41`), so `self.rgb` ends up empty and `present()` bails on `self.rgb.is_empty()`. But the containment is accidental. `kitty.rs:108` is

```rust
assert_eq!(rgb.len(), (w as usize) * (h as usize) * 3, "rgb buffer size must match w*h*3");
```

— the *same* wrapping multiply, in an `assert_eq!` that is live in release, guarding an encoder that would otherwise index with those dimensions. Any future change that lets `rgb` be non-empty while the geometry is poisoned turns this into a panic at best. The workspace sets `panic = "unwind"` specifically so the tty-restore hook runs, so a panic is a clean session kill rather than a corrupted terminal — but it is still a remote-ish DoS reachable from a malformed 32-byte header.

Three fixes, all one-liners: `checked_mul`; reject `width`/`height` outside the negotiated viewport; reject `format != 0`. Add a fourth for the damage work A07 §3 is heading toward: reject a dirty rect that is not contained in the frame, or `bgra_rect_to_rgb` (`kitty.rs:54-67`) indexes out of bounds.

### 2.6 F6 (Medium) — backpressure is one-directional and measured in the wrong place

The engine-side design (`main.js:44-98`) is right in spirit — one frame in flight, newest wins — and the comment explaining why is correct. Three gaps:

**It measures the socket, not the terminal.** `sock.write()` returning `true` means the bytes reached the kernel's socket buffer, not that the terminal consumed them. A10 §0.1 already establishes that the PTY write is the bottleneck. Under SSH (A07 §4.4) the divergence is enormous: the socket drains at 176+ MB/s while the link does 1 MB/s. The engine will happily report "not backpressured" while the terminal is 40 frames behind. What is needed is an **ACK from the core after the terminal write completes**, and a credit window of 1 — which is exactly today's `writeInFlight` variable, just anchored to the right event. A07 §5.3 already specifies frame-ACK pacing; this is the protocol hook it needs.

**`sendMessage` conflates "would block" with "socket is dead."** `main.js:52-57` returns `false` for both, and `flushFrame` (`main.js:70-79`) treats every `false` as backpressure:

```js
} else {
  sock.once('drain', () => { writeInFlight = false; flushFrame(); });
}
```

On a destroyed socket that `drain` never fires and `writeInFlight` stays `true` forever. It is currently unreachable (`onPaint` returns early when `sock.destroyed`, and `'close'` calls `app.exit(0)`), but it is one refactor away from being a silent stall, and if `sock` were ever `null` this line throws a `TypeError` out of a paint handler. Return a three-state value.

**No staleness bound.** If the terminal stalls for 10 s the core eventually renders a 10-second-old frame. There should be a rule: if the pending frame is older than N ms, drop it and request a fresh full repaint rather than presenting stale pixels.

Minor: `stats.sent++` (`main.js:69`) increments before the write is known to have succeeded, and there is no `dropped` counter.

### 2.7 F7 (Medium) — no reconnect semantics

- The `UnixListener` is a local in `Session::start` and is dropped when `start` returns. `Session` holds `child`, `stream`, `socket_path`, `socket_dir` — **no listener**. Reconnection is impossible by construction.
- The engine treats socket loss as fatal: `sock.on('close', () => app.exit(0))` (`main.js:307`), `sock.on('error', …) → app.exit(3)` (`main.js:301-304`).
- The core treats engine loss as fatal: `Ok(0) => { eprint_restore("engine exited"); return 1; }` (`main.rs:521-524`). The user is dumped back to a shell mid-browse.
- **A renderer crash is captured and then discarded.** `render-process-gone` → `{"t":"crash",…}` → `Status::crashed` (`main.rs:797`). `grep` shows `crashed` is read in exactly one place: a unit test. It never reaches `present()`, so a crashed tab renders as a frozen last frame with a normal-looking status bar.

For v1 the right semantics are simple and worth writing down explicitly: **the engine does not reconnect; the core restarts the engine.** With a socketpair there is no path to reconnect *to*, which makes this the only coherent model. The core needs a session `epoch` so it can invalidate cached geometry and force a full repaint, a bounded retry policy, and a visible banner.

### 2.8 F8 (Medium) — no liveness

Nothing in the protocol distinguishes "the page is idle" from "the Electron main process is wedged". Both look like: socket open, no data, `poll` timing out every 16 ms. `Renderer::frame_times` decays so the status bar eventually reads 0 fps, which is the only hint the user gets, and it is indistinguishable from a static page. A PING/PONG with a deadline costs 5 bytes.

### 2.9 F9 (Medium) — unbounded paste amplification

`main.rs:645-652` puts the whole bracketed-paste payload into one `text` field. `main.js:212-216` then does:

```js
for (const ch of cmd.text) {
  wc.sendInputEvent({ type: 'char', keyCode: ch, modifiers: mods });
}
```

One synchronous IPC round trip per character, on the main process's thread, with no yield. A 10 MB paste is 10 million of them. The engine stops servicing the socket entirely for the duration, which then triggers F1 on every subsequent command. This compounds: the oversized paste both causes the desync *and* wedges the reader that would have detected it.

Cap paste at the negotiated `max_paste` (64 KiB is generous for a terminal), chunk it into separate messages, and prefer `webContents.insertText()` over per-character `char` events where the target accepts it.

### 2.10 F10 (Medium, perf) — the protocol shape costs ~13 % of the frame budget in memcpy

Four full-frame copies happen per frame that do not need to happen. At the measured Ghostty geometry (2482×814 = 8,081,424 B):

**JS side** — `main.js:97` (`Buffer.concat([head, bitmap])`) and `main.js:57` (`Buffer.concat([header, payload])`), two full copies:

```
current: concat(head,bitmap)+concat(hdr,payload) 1.068 ms/frame   (repeat: 1.142)
proposed: cork + 3 writes, zero concat          0.001 ms/frame
saved: 1.067 ms/frame  (6.4% of the measured 16.65 ms p50 frame budget)
```

**Rust side** — `lib.rs:90` (`to_vec`) and `lib.rs:91` (`drain`), two more:

```
frame=8081424 bytes, 300 frames        (5 consecutive runs, median reported)
current (to_vec + drain):  1.556 ms/frame
cursor  (zero-copy)     :  0.689 ms/frame
speedup: 2.26x, saving 0.867 ms/frame
```

Together **~2.0–2.3 ms of the 16.65 ms p50 budget**, or 12–14 %, spent moving bytes from one address to another. Both fixes are mechanical: `cork()` + separate `write()`s so `writev` sees the header and bitmap as separate iovecs, and a read cursor with amortized compaction instead of `to_vec` + `drain`.

This does not contradict A10 §0.1 (the PTY write is still the bottleneck). It is 2 ms of headroom that is currently free to reclaim, and it grows linearly with viewport.

### 2.11 F11 (Low) — `json_get_str` edge cases

Three results, all measured by compiling `bg-proto` verbatim and feeding it real serialiser output.

**Not injectable via page titles — confirmed safe.** I tested hostile titles (`x","v":"HIJACKED`, `x"},{"t":"crash","reason":"pwned`) through `JSON.stringify` exactly as `sendEvent` does. The needle `"t"` requires an *unescaped* quote-t-quote, and escaped values render as `\"t\"`, so the substring search cannot be walked onto attacker text:

```
wire   : {"t":"title","v":"x\",\"v\":\"HIJACKED"}
  t      = Some("title")          <- correct
  v      = Some("x\",\"v\":\"HIJACKED")
  reason = None
```

The comment at `lib.rs:120-124` justifying the minimal parser is, for the current message set, accurate. I am reporting the two ways it is nonetheless fragile, not claiming a vulnerability that does not exist.

**Nested objects return the wrong value — confirmed.** First occurrence wins, regardless of depth:

```
wire   : {"meta":{"t":"inner","v":"inner-v"},"t":"title","v":"outer-v"}
  t      = Some("inner")          <- wrong; should be "title"
  v      = Some("inner-v")        <- wrong; should be "outer-v"
```

No current event nests (`render-process-gone` is flattened at `main.js:145-147`, `stats` is flat), so this is latent. It fires silently the first time anyone adds a nested field — which is exactly the kind of additive change a version scheme is supposed to make safe. Minimum fix: a debug assertion that payloads contain no `{` after index 0, or a documented "flat objects only" invariant enforced by a test.

**Surrogate-pair escapes become two `U+FFFD` — confirmed.** `lib.rs:144-148` decodes `\uXXXX` one unit at a time:

```
{"t": "title", "v": "emoji 😀 tail"}   ->  v = Some("emoji �� tail")
```

Not reachable from `JSON.stringify` (which emits astral characters as literal UTF-8 and only escapes *lone* surrogates, where `U+FFFD` is the correct answer). It **is** reachable from any peer using a serialiser with `ensure_ascii`-style defaults — Python's `json.dumps`, which is what the existing `benchmarks/a07/*.py` harnesses would reach for. Fix: buffer a high surrogate and combine it with the following low surrogate.

### 2.12 F12 (Low) — extension and buffer-management hygiene

**Unknown types are silently ignored on both sides.** `main.rs:551` has `_ => {}`; the JS reader only acts on `T_COMMAND`. After a version bump this produces a half-working session instead of a clean failure. The standard remedy is a must-understand/optional split in the type-ID space (§3).

**`subarray` retains the whole backing store.** `main.js:276` (`buf = buf.subarray(5 + len)`) leaves a small remainder pinning the entire allocation. Measured:

```
subarray tail: length 3 pins backing store of 4194304 bytes
copied  tail:  length 3 pins backing store of 8192 bytes
```

512× retention for a 3-byte tail. Low impact today because the core→engine direction carries only small JSON, but the same pattern would be fatal if frames ever flowed that way.

**`Buffer.concat` per chunk is O(n²)** on fragmented input (`main.js:269`). Accumulate a chunk list and concat once per `push` instead.

### 2.13 F13 (Low) — unvalidated command arguments in the engine

`handleCommand` (`main.js:226-262`) trusts its input completely:

- `win.loadURL(cmd.url)` — no scheme allowlist. The core normalises URLs (`normalize_url`), but the *engine* should not depend on that; with F4 unfixed, an impersonating peer can send `file:///…/.ssh/id_rsa` or `javascript:`. `webSecurity: true` does not block a top-level `file:` navigation. A09 §3.5 already establishes the scheme-allowlist pattern for `openExternal`; the same list belongs here.
- `win.setSize(Math.max(1, cmd.w), Math.max(1, cmd.h))` — lower bound only. `{"t":"resize","w":1e9,"h":1e9}` asks Chromium for a 4-exabyte backing store. `NaN` propagates through `Math.max` and throws, which the `try` at `main.js:278-282` swallows into a log line, so the failure mode is "silently ignored" rather than "reported".
- `handleInput` — `Math.round(NaN)` likewise, and `cmd.text` is unbounded (F9).

None of these are exploitable *from web content*; they matter because they are the second line of defence for F4, and because "swallowed by a `try`" is not the same as "validated".

---

## 3. Proposed protocol v1: a versioned handshake

Design constraints I held to: the existing framing is good and should not change; the change must be a strict addition so the diff is reviewable; and a mismatched peer must fail in the first five bytes rather than mis-framing eight megabytes.

### 3.1 Type-ID space

Framing is unchanged: `[u8 type][u32 BE len][payload]`.

| ID | Name | Direction | Notes |
|---|---|---|---|
| `0x00` | `HELLO` | engine → core | MUST be the first message on the connection |
| `0x01` | `FRAME` | engine → core | unchanged |
| `0x02` | `EVENT` | engine → core | unchanged |
| `0x0A` | `COMMAND` | core → engine | unchanged |
| `0x0B` | `WELCOME` | core → engine | MUST be the first message on the connection |
| `0x0C` | `PING` | either | liveness (F8) |
| `0x0D` | `PONG` | either | liveness (F8) |
| `0x0E` | `GOODBYE` | either | last message; carries a reason code |
| `0x0F` | `ACK` | core → engine | frame pacing (F6, A07 §5.3) |
| `0x03`–`0x09`, `0x10`–`0x7F` | reserved | — | **must understand**: unknown ⇒ fatal |
| `0x80`–`0xFF` | reserved | — | **optional**: unknown ⇒ skip payload, continue |

The existing three IDs keep their values, so this is additive. The must-understand/optional split is the TLS/SSH extension trick and it is what makes future versions safe: v2 can add a *mandatory* message that a v1 peer provably cannot ignore, while still allowing purely additive telemetry that old peers skip.

### 3.2 `HELLO` (engine → core, JSON, ≤ 4 KiB)

```json
{
  "t": "hello",
  "magic": "blackglass",
  "proto": 1,
  "proto_min": 1,
  "impl": "blackglass-engine/0.1.0",
  "electron": "43.2.0",
  "chrome": "150.0.0.0",
  "pid": 41231,
  "frame_header_len": 32,
  "features": ["frame.bgra8888", "frame.dirty", "event.json", "cmd.input", "cmd.nav", "ping", "ack"],
  "limits": { "max_msg": 33554464, "max_json": 65536 }
}
```

`frame_header_len` is the cheap drift detector for F2: the core asserts it equals its own `FRAME_HEADER_LEN` and refuses otherwise. That single field turns a silent field-offset catastrophe into a startup error.

### 3.3 `WELCOME` (core → engine, JSON, ≤ 4 KiB)

```json
{
  "t": "welcome",
  "magic": "blackglass",
  "proto": 1,
  "impl": "blackglass/0.1.0",
  "features": ["frame.bgra8888", "frame.dirty", "ping", "ack"],
  "limits": {
    "max_msg": 33554464, "max_json": 65536, "max_paste": 65536,
    "max_w": 4096, "max_h": 4096
  },
  "viewport": { "w": 2482, "h": 814 },
  "session": "9f2c1e7a4b0d3856",
  "epoch": 1
}
```

### 3.4 `GOODBYE` (either direction, JSON, ≤ 4 KiB)

```json
{ "t": "goodbye", "code": "proto_mismatch", "detail": "engine proto=2, core supports 1..1" }
```

Codes: `ok`, `bad_magic`, `proto_mismatch`, `header_mismatch`, `oversize`, `backlog`, `unknown_type`, `bad_geometry`, `bad_format`, `malformed`, `timeout`, `internal`.

### 3.5 Normative rules

1. The connecting peer sends `HELLO` first; the accepting peer sends `WELCOME` first. Neither may send anything else before its opener. Both openers may be in flight simultaneously — this is not a request/response.
2. `magic` MUST be exactly `"blackglass"`. Absent or wrong ⇒ `GOODBYE{bad_magic}` and close.
3. Compatibility: the core accepts the engine iff `hello.proto_min ≤ CORE_PROTO ≤ hello.proto`. `proto` increments **only** for a change no feature flag can express — framing, header layout, or the meaning of an existing type ID. Everything additive rides on `features`.
4. `hello.frame_header_len` MUST equal the receiver's `FRAME_HEADER_LEN`, else `GOODBYE{header_mismatch}`.
5. Effective limits per direction are `min(sender_advertised, receiver_advertised)`. A receiver always enforces its own, on the length prefix, before buffering payload.
6. Handshake deadline: 5 s from `accept`/`connect`. Miss ⇒ `GOODBYE{timeout}`, kill the child. (The existing 30 s budget stays for *process* start; 5 s applies once the socket is up.)
7. **A protocol error is fatal and non-recoverable.** There is no resynchronisation on a length-prefixed stream: once the framer is misaligned, every subsequent "length" is garbage or attacker-chosen. Send `GOODBYE`, close, restart.
8. `epoch` increments on every engine start within a session. `seq` is per-epoch and monotonic; a `seq` that goes backwards means a restart the core missed.
9. `PING` payload is an 8-byte BE nonce; `PONG` echoes it. Either side may ping. Two missed pongs at 2 s intervals ⇒ `GOODBYE{timeout}`.

### 3.6 Connection state machine

```
                 ┌──────────┐  socketpair / accept
                 │  INIT    │──────────────────────────┐
                 └──────────┘                          │
                       │ send opener                   │
                       ▼                               │
                 ┌──────────┐  peer opener valid       │
                 │HANDSHAKE │─────────────────────┐    │  any ProtoError,
                 └──────────┘                     │    │  timeout, or EOF
                   │      │ invalid / timeout     ▼    │        │
                   │      └──────────────►┌──────────┐ │        │
                   │                      │ CLOSING  │◄┴────────┘
                   ▼                      └──────────┘
              ┌──────────┐  raise limits        │ send GOODBYE, close
              │  READY   │                      ▼
              └──────────┘                ┌──────────┐  core: epoch+1, respawn
                   │ FRAME/EVENT/COMMAND  │  DEAD    │  (≤3 tries, 250ms/1s/3s)
                   └──────────────────────►└─────────┘
```

The important property: **limits start tight and are only raised after a successful handshake.** Before `WELCOME` the only legal messages are `HELLO` and `GOODBYE`, both JSON, so the pre-handshake caps can be 4 KiB per message and 16 KiB buffered. An unauthenticated peer that connects and immediately claims a 4 GiB frame is rejected against the *handshake* limits, having consumed 16 KiB.

---

## 4. Reference implementation — Rust

**VERIFIED.** Compiled with `rustc -O --edition 2021` and executed; all assertions pass. Full source: `scratchpad/b06/proposed.rs`.

### 4.1 Limits, errors, type space

```rust
pub const PROTO_VERSION: u32 = 1;
pub const PROTO_MIN_VERSION: u32 = 1;
pub const PROTO_MAGIC: &str = "blackglass";

pub const T_HELLO: u8 = 0x00;
pub const T_FRAME: u8 = 0x01;
pub const T_EVENT: u8 = 0x02;
pub const T_COMMAND: u8 = 0x0A;
pub const T_WELCOME: u8 = 0x0B;
pub const T_PING: u8 = 0x0C;
pub const T_PONG: u8 = 0x0D;
pub const T_GOODBYE: u8 = 0x0E;
pub const T_ACK: u8 = 0x0F;

/// Types at or above this are optional extensions: an unknown one is skipped, not fatal.
/// Below it, an unknown type is a fatal protocol error.
pub const T_OPTIONAL_BASE: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_frame: usize,
    pub max_json: usize,
    pub max_buffered: usize,
    pub max_w: u32,
    pub max_h: u32,
}

impl Limits {
    /// Pre-handshake ceiling. Deliberately small: before WELCOME the only legal messages
    /// are HELLO and GOODBYE, both JSON.
    pub const HANDSHAKE: Self = Self {
        max_frame: 0, max_json: 4 * 1024, max_buffered: 16 * 1024, max_w: 0, max_h: 0,
    };

    /// Derive session limits from negotiated geometry. One frame in flight plus one
    /// partially-received frame is all the core ever needs to buffer.
    pub fn for_viewport(max_w: u32, max_h: u32) -> Option<Self> {
        let max_frame = (max_w as usize)
            .checked_mul(max_h as usize)?
            .checked_mul(4)?
            .checked_add(FRAME_HEADER_LEN)?;
        Some(Self {
            max_frame,
            max_json: 64 * 1024,
            max_buffered: max_frame.checked_mul(2)?.checked_add(64 * 1024)?,
            max_w, max_h,
        })
    }

    fn cap_for(&self, type_id: u8) -> usize {
        match type_id { T_FRAME => self.max_frame, _ => self.max_json }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtoError {
    Oversize { type_id: u8, len: usize, cap: usize },
    Backlog { buffered: usize, cap: usize },
    UnknownType(u8),
    BadGeometry { w: u32, h: u32 },
    BadFormat(u32),
}

impl ProtoError {
    /// Wire code for the GOODBYE message.
    pub fn code(&self) -> &'static str {
        match self {
            ProtoError::Oversize { .. }    => "oversize",
            ProtoError::Backlog { .. }     => "backlog",
            ProtoError::UnknownType(_)     => "unknown_type",
            ProtoError::BadGeometry { .. } => "bad_geometry",
            ProtoError::BadFormat(_)       => "bad_format",
        }
    }
}
```

For `Limits::for_viewport(4096, 4096)` this yields `max_frame = 67_108_896` (64 MiB + 32 B) and `max_buffered = 134_283_328`. Compare against the measured Ghostty frame of 8,081,424 B — roughly 8× headroom, which covers a fullscreen 4K terminal. Terminals wider than 4096 px (a 5K/6K display) need `max_w` raised; derive it from the actual detected viewport rather than hardcoding, and treat 4096 as the fallback.

### 4.2 Bounded, zero-copy reader

This replaces `MessageReader` and fixes F3 and F10 together.

```rust
/// Incremental reader. Bounded, zero-copy, no per-message allocation.
#[derive(Default)]
pub struct MessageReader {
    buf: Vec<u8>,
    pos: usize,
    limits: Limits,
}

impl MessageReader {
    pub fn new(limits: Limits) -> Self { Self { buf: Vec::new(), pos: 0, limits } }

    /// Raise the caps once HELLO/WELCOME have been exchanged.
    pub fn set_limits(&mut self, limits: Limits) { self.limits = limits; }

    pub fn buffered(&self) -> usize { self.buf.len() - self.pos }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ProtoError> {
        // Amortized O(1): compact only when the consumed prefix is at least half the buffer.
        if self.pos > 0 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let after = self.buffered() + bytes.len();
        if after > self.limits.max_buffered {
            return Err(ProtoError::Backlog { buffered: after, cap: self.limits.max_buffered });
        }
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    /// Hand the next complete message to `f`, borrowed from the read buffer.
    /// Ok(false) means "need more bytes". Any Err is FATAL: close the connection.
    /// A length-prefixed stream cannot be resynchronised once misaligned.
    pub fn with_next<F>(&mut self, f: &mut F) -> Result<bool, ProtoError>
    where F: FnMut(u8, &[u8])
    {
        if self.buffered() < 5 { return Ok(false); }
        let h = &self.buf[self.pos..self.pos + 5];
        let type_id = h[0];
        let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;

        if type_id < T_OPTIONAL_BASE && !is_known(type_id) {
            return Err(ProtoError::UnknownType(type_id));
        }
        let cap = self.limits.cap_for(type_id);
        if len > cap {
            // Reject on the LENGTH PREFIX, before a single payload byte is buffered.
            return Err(ProtoError::Oversize { type_id, len, cap });
        }
        if self.buffered() < 5 + len { return Ok(false); }

        let start = self.pos + 5;
        if type_id >= T_OPTIONAL_BASE {
            self.pos = start + len;          // skip unknown optional extension
            return Ok(true);
        }
        f(type_id, &self.buf[start..start + len]);
        self.pos = start + len;
        Ok(true)
    }
}
```

The closure form is what makes it zero-copy: the payload is borrowed from the read buffer for the duration of the call and never copied out. The call site changes from `while let Some(msg) = reader.next_message()` to:

```rust
let mut on_msg = |type_id: u8, payload: &[u8]| { /* ... same match ... */ };
loop {
    match reader.with_next(&mut on_msg) {
        Ok(true) => continue,
        Ok(false) => break,
        Err(e) => { self.protocol_fault(e); return 1; }   // GOODBYE + close, no resync
    }
}
```

### 4.3 Validated frame header

```rust
pub const FORMAT_BGRA8888: u32 = 0;

impl FrameHeader {
    pub fn parse(b: &[u8], limits: &Limits) -> Result<Option<Self>, ProtoError> {
        if b.len() < FRAME_HEADER_LEN { return Ok(None); }
        let g = |i: usize| u32::from_be_bytes([b[i], b[i+1], b[i+2], b[i+3]]);
        let h = Self {
            seq: g(0), width: g(4), height: g(8),
            dirty_x: g(12), dirty_y: g(16), dirty_w: g(20), dirty_h: g(24),
            format: g(28),
        };
        if h.format != FORMAT_BGRA8888 {
            return Err(ProtoError::BadFormat(h.format));
        }
        if h.width == 0 || h.height == 0 || h.width > limits.max_w || h.height > limits.max_h {
            return Err(ProtoError::BadGeometry { w: h.width, h: h.height });
        }
        // Dirty rect must lie inside the frame, or damage-based rendering reads OOB.
        if h.dirty_x.saturating_add(h.dirty_w) > h.width
            || h.dirty_y.saturating_add(h.dirty_h) > h.height {
            return Err(ProtoError::BadGeometry { w: h.dirty_w, h: h.dirty_h });
        }
        Ok(Some(h))
    }

    /// Exact pixel-payload length. Checked: `width * height * 4` overflows usize for
    /// large u32 inputs and silently wraps in release builds.
    pub fn expected_payload(&self) -> Option<usize> {
        (self.width as usize).checked_mul(self.height as usize)?.checked_mul(4)
    }
}
```

Verified behaviour of the whole set:

```
session limits: Limits { max_frame: 67108896, max_json: 65536, max_buffered: 134283328,
                         max_w: 4096, max_h: 4096 }
hostile 4GiB length      -> type 1 declared 4294967295 bytes, cap is 67108896 (oversize)
pre-handshake 32KiB drip -> buffered 32768 bytes, cap is 16384 (backlog)
unknown type 0x42        -> unknown must-understand type 66 (unknown_type)
unknown type 0x90        -> skipped; delivered 1
2^31 x 2^31 geometry     -> impossible frame geometry 2147483648x2147483648 (bad_geometry)
checked expected_payload -> None (current code yields Some(0))
split reassembly         -> ok
measured Ghostty frame   -> 2482x814 payload=Some(8081392)

ALL PROPOSED-BEHAVIOUR ASSERTIONS PASSED
```

---

## 5. Reference implementation — the core's send path

### 5.1 Bounded outbound queue (fixes F1)

**VERIFIED** against the exact 200 KB paste that breaks the current code. Full source: `scratchpad/b06/outq.rs`.

```rust
const MAX_OUTBOUND: usize = 4 << 20;   // 4 MiB ~= 50 queued full-size pastes

/// A byte-oriented outbound queue. Messages are appended whole, so the stream can never
/// be left mid-message; only whole messages are ever dropped, and only when the queue is
/// full, which is reported rather than swallowed.
#[derive(Default)]
struct Outbox { buf: Vec<u8>, pos: usize, dropped: u64 }

impl Outbox {
    fn pending(&self) -> usize { self.buf.len() - self.pos }
    fn wants_write(&self) -> bool { self.pending() > 0 }

    /// Returns false if the message was dropped because the queue is full.
    fn push(&mut self, msg: &[u8]) -> bool {
        if self.pending() + msg.len() > MAX_OUTBOUND { self.dropped += 1; return false; }
        if self.pos > 0 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(msg);
        true
    }

    /// Drain as much as the socket will take. Call on POLLOUT. Partial writes are tracked
    /// by `pos`, so the next call resumes at the exact byte -- never a re-send, never a
    /// truncated message.
    fn drain(&mut self, s: &mut UnixStream) -> std::io::Result<()> {
        while self.pending() > 0 {
            match s.write(&self.buf[self.pos..]) {
                Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                Ok(n) => self.pos += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        self.buf.clear();
        self.pos = 0;
        Ok(())
    }
}
```

Wire it into the existing `poll` loop by making `POLLOUT` conditional, so the loop does not spin when there is nothing to send:

```rust
let mut sock_events = libc::POLLIN;
if outbox.wants_write() { sock_events |= libc::POLLOUT; }
let mut fds = [
    libc::pollfd { fd: stdin_fd, events: libc::POLLIN,  revents: 0 },
    libc::pollfd { fd: sock_fd,  events: sock_events,   revents: 0 },
];
// ...
if fds[1].revents & libc::POLLOUT != 0 {
    if let Err(e) = outbox.drain(&mut self.stream) {
        eprint_restore(&format!("engine write error: {e}"));
        return 1;
    }
}
```

`Session::send` then becomes fallible in a way the caller can see:

```rust
fn send(&mut self, json: &str) {
    let msg = proto::frame_message(proto::T_COMMAND, json.as_bytes());
    if !self.outbox.push(&msg) {
        log_line(&format!("outbox full, dropped command ({} bytes)", msg.len()));
    }
}
```

Two behaviours to add on top, both cheap:

- **Coalesce mouse moves.** Before pushing a `mouse/move`, if the tail of the outbox is also a `mouse/move`, replace it. Never coalesce `down`/`up`/`wheel`/key events — dropping a `mouseUp` is what causes stuck drags.
- **Chunk paste.** Split `Event::Paste` text into ≤ `max_paste` pieces and push them as separate messages (§2.9).

Verified end-to-end:

```
queued 200088 bytes across 2 messages
fully drained after 102 POLLOUT passes, dropped=0
engine reframed 2 whole messages: [(10, 200066), (10, 12)]
last message body = {"t":"quit"}
PASS: 200 KB paste no longer desynchronises the stream; quit still executes
```

---

## 6. Reference implementation — JavaScript

**VERIFIED.** `node --test` → **9 tests, 9 pass, 0 fail** on Node v24.11.1. Full source: `scratchpad/b06/proposed-framer.js` + `framer.test.js`. Written as a standalone module deliberately: it needs no Electron, so it is CI-able on any runner, which is the point (§9).

### 6.1 Bounded reader

```js
const HANDSHAKE_LIMITS = { maxJson: 4 * 1024, maxFrame: 0, maxBuffered: 16 * 1024 };

class ProtocolError extends Error {
  constructor(code, detail) { super(`${code}: ${detail}`); this.code = code; }
}

// Fixes vs the current attachReader():
//  * caps the declared length BEFORE buffering the payload
//  * caps total buffered bytes
//  * unknown must-understand types are fatal, 0x80+ are skipped
//  * copies the tail instead of subarray()-ing it, so an 8 MB ArrayBuffer is not
//    retained by a 3-byte remainder
class Framer {
  constructor(limits = HANDSHAKE_LIMITS) {
    this.limits = limits;
    this.chunks = [];
    this.buffered = 0;
  }

  setLimits(limits) { this.limits = limits; }
  capFor(type) { return type === T_FRAME ? this.limits.maxFrame : this.limits.maxJson; }

  // Returns [{type, payload}]. Throws ProtocolError -- which is FATAL: a length-prefixed
  // stream cannot be resynchronised once misaligned.
  push(chunk) {
    this.buffered += chunk.length;
    if (this.buffered > this.limits.maxBuffered) {
      throw new ProtocolError('backlog',
        `${this.buffered} buffered, cap ${this.limits.maxBuffered}`);
    }
    this.chunks.push(chunk);
    // Single concat per push, not per message; O(n) not O(n^2) on fragmented input.
    let buf = this.chunks.length === 1 ? this.chunks[0]
                                       : Buffer.concat(this.chunks, this.buffered);
    this.chunks = [buf];

    const out = [];
    let off = 0;
    for (;;) {
      if (buf.length - off < 5) break;
      const type = buf.readUInt8(off);
      const len = buf.readUInt32BE(off + 1);
      if (type < T_OPTIONAL_BASE && !KNOWN.has(type)) {
        throw new ProtocolError('unknown_type', `type ${type}`);
      }
      const cap = this.capFor(type);
      if (len > cap) {
        throw new ProtocolError('oversize', `type ${type} declared ${len}, cap ${cap}`);
      }
      if (buf.length - off < 5 + len) break;
      if (type < T_OPTIONAL_BASE) {
        out.push({ type, payload: buf.subarray(off + 5, off + 5 + len) });
      }
      off += 5 + len;
    }
    if (off > 0) {
      // Buffer.from() COPIES; buf.subarray() would pin the whole backing ArrayBuffer.
      const rest = Buffer.from(buf.subarray(off));
      this.chunks = rest.length ? [rest] : [];
      this.buffered = rest.length;
    }
    return out;
  }
}
```

### 6.2 Writer: `cork` + `writev`, three-state result

```js
// Fixes vs the current sendMessage()/flushFrame():
//  * cork+writev instead of Buffer.concat -- removes 2 full-frame memcpys per frame
//  * distinguishes "would block" (backpressure) from "socket is dead" (stop)
//  * clears writeInFlight if the socket dies while a drain is outstanding
class Writer {
  constructor(sock) {
    this.sock = sock;
    this.writeInFlight = false;
    this.pendingFrame = null;
    this.stats = { produced: 0, sent: 0, coalesced: 0, dropped: 0 };
    sock.on('close', () => { this.writeInFlight = false; this.pendingFrame = null; });
  }

  get dead() { return !this.sock || this.sock.destroyed || this.sock.writableEnded; }

  // Returns 'ok' | 'backpressure' | 'dead' -- never a bare boolean.
  send(type, ...parts) {
    if (this.dead) return 'dead';
    let len = 0;
    for (const p of parts) len += p.length;
    const head = Buffer.allocUnsafe(5);
    head.writeUInt8(type, 0);
    head.writeUInt32BE(len, 1);
    this.sock.cork();
    this.sock.write(head);
    let ok = true;
    for (const p of parts) ok = this.sock.write(p);
    this.sock.uncork();
    return ok ? 'ok' : 'backpressure';
  }

  // head and bitmap stay separate all the way to writev: no concat anywhere.
  queueFrame(head, bitmap) {
    this.stats.produced++;
    if (this.pendingFrame) this.stats.coalesced++;
    this.pendingFrame = [head, bitmap];
    this.flush();
  }

  flush() {
    if (this.writeInFlight || !this.pendingFrame) return;
    if (this.dead) { this.pendingFrame = null; return; }
    const parts = this.pendingFrame;
    this.pendingFrame = null;
    this.writeInFlight = true;
    const r = this.send(T_FRAME, ...parts);
    if (r === 'ok') {
      this.stats.sent++;
      this.writeInFlight = false;
      if (this.pendingFrame) setImmediate(() => this.flush());
    } else if (r === 'backpressure') {
      this.stats.sent++;
      this.sock.once('drain', () => { this.writeInFlight = false; this.flush(); });
    } else {
      this.stats.dropped++;
      this.writeInFlight = false;   // socket died: do NOT wait for a drain that never fires
    }
  }
}
```

`onPaint` then loses its `Buffer.concat` entirely:

```js
function onPaint(_event, dirty, image) {
  if (writer.dead) return;
  const size = image.getSize();
  const head = Buffer.allocUnsafe(32);
  head.writeUInt32BE(seq++, 0);
  head.writeUInt32BE(size.width, 4);
  head.writeUInt32BE(size.height, 8);
  head.writeUInt32BE(dirty.x, 12);
  head.writeUInt32BE(dirty.y, 16);
  head.writeUInt32BE(dirty.width, 20);
  head.writeUInt32BE(dirty.height, 24);
  head.writeUInt32BE(0, 28);              // format 0 = BGRA8888
  writer.queueFrame(head, image.toBitmap());
}
```

### 6.3 Handshake and fatal-error handling

```js
function onProtocolError(err) {
  // Fatal by design: a misaligned length-prefixed stream cannot be resynchronised.
  try { writer.sendJson(T_GOODBYE, { t: 'goodbye', code: err.code, detail: err.message }); }
  catch (_) { /* socket already gone */ }
  console.error('[engine] protocol fault:', err.message);
  app.exit(4);
}

let handshakeDone = false;
const framer = new Framer(HANDSHAKE_LIMITS);

socket.on('data', (chunk) => {
  let msgs;
  try { msgs = framer.push(chunk); }
  catch (e) { return onProtocolError(e); }

  for (const { type, payload } of msgs) {
    if (!handshakeDone) {
      if (type !== T_WELCOME) {
        return onProtocolError(new ProtocolError('malformed', `expected WELCOME, got ${type}`));
      }
      let w;
      try { w = JSON.parse(payload.toString('utf8')); }
      catch (e) { return onProtocolError(new ProtocolError('malformed', e.message)); }
      if (w.magic !== PROTO_MAGIC) {
        return onProtocolError(new ProtocolError('bad_magic', String(w.magic)));
      }
      if (w.proto !== PROTO_VERSION) {
        return onProtocolError(new ProtocolError('proto_mismatch',
          `core proto=${w.proto}, engine=${PROTO_VERSION}`));
      }
      framer.setLimits({
        maxJson: Math.min(w.limits.max_json, 65536),
        maxFrame: 0,                                    // core never sends frames
        maxBuffered: Math.min(w.limits.max_json, 65536) * 4,
      });
      negotiated = w;
      handshakeDone = true;
      clearTimeout(handshakeTimer);
      continue;
    }
    switch (type) {
      case T_COMMAND: /* ... existing handleCommand, now with validated args ... */ break;
      case T_PING:    writer.send(T_PONG, payload); break;
      case T_ACK:     onAck(payload.readUInt32BE(0)); break;
      case T_GOODBYE: app.exit(0); break;
      default:
        return onProtocolError(new ProtocolError('unknown_type', `type ${type}`));
    }
  }
});

// Send our opener immediately; both openers may be in flight at once.
writer.sendJson(T_HELLO, {
  t: 'hello', magic: PROTO_MAGIC, proto: PROTO_VERSION, proto_min: PROTO_VERSION,
  impl: 'blackglass-engine/0.1.0',
  electron: process.versions.electron, chrome: process.versions.chrome,
  pid: process.pid, frame_header_len: 32,
  features: ['frame.bgra8888', 'frame.dirty', 'event.json', 'cmd.input', 'cmd.nav', 'ping', 'ack'],
  limits: { max_msg: 33554464, max_json: 65536 },
});
const handshakeTimer = setTimeout(() => {
  onProtocolError(new ProtocolError('timeout', 'no WELCOME within 5s'));
}, 5000);
```

### 6.4 Argument validation (fixes F13)

```js
const ALLOWED_SCHEMES = new Set(['http:', 'https:', 'about:', 'data:']);
const MAX_DIM = 8192;

function safeUrl(u) {
  if (typeof u !== 'string' || u.length > 8192) return null;
  let parsed;
  try { parsed = new URL(u); } catch (_) { return null; }
  return ALLOWED_SCHEMES.has(parsed.protocol) ? parsed.toString() : null;
}

function safeDim(v) {
  return Number.isInteger(v) && v >= 1 && v <= MAX_DIM ? v : null;
}
```

`file:` is deliberately absent. `blackglass open /tmp/x.html` currently normalises to `file://…` in the core (`normalize_url`), so if local files are a supported product feature the core must pass an explicit `"allow_file": true` in `WELCOME` and the engine must additionally confirm the path lies under a directory the user named on the command line. Until that exists, local-file browsing should be treated as unsupported rather than accidentally supported.

---

## 7. Transport: replace the filesystem socket with a socketpair

This is the single highest leverage change relative to its size, because it deletes F4 outright along with the listener, the accept race, the 0700/0600 permission dance, the `--bg-socket=` argv disclosure, and the socket-file cleanup in `shutdown`.

**VERIFIED** Rust → Node v24.11.1 (`scratchpad/b06/fdpass.rs` + `child.js`):

```
[child] got type=11 len=55 body={"t":"welcome","proto":1,"limits":{"max_msg":33554464}}
core got type=0 len=41 body={"t":"hello","proto":1,"node":"v24.11.1"}
socket fd=3 has no filesystem path (socketpair)
```

Core side:

```rust
use std::os::fd::IntoRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;

const ENGINE_FD: i32 = 3;

let (core_end, engine_end) = UnixStream::pair()?;
let engine_fd = engine_end.into_raw_fd();

let mut child = unsafe {
    Command::new(&electron)
        .arg(&engine_main)
        .arg(format!("--bg-width={w}"))
        .arg(format!("--bg-height={h}"))
        .arg(format!("--bg-url={url}"))
        .env("BG_SOCK_FD", ENGINE_FD.to_string())
        .pre_exec(move || {
            // dup2 clears FD_CLOEXEC on the new descriptor, so fd 3 survives exec.
            if libc::dup2(engine_fd, ENGINE_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
        .spawn()?
};
unsafe { libc::close(engine_fd) };
core_end.set_nonblocking(true)?;
```

Engine side, replacing `net.createConnection(SOCKET_PATH, …)`:

```js
const FD = parseInt(process.env.BG_SOCK_FD, 10);
if (!Number.isInteger(FD)) {
  console.error('[engine] fatal: BG_SOCK_FD is required');
  process.exit(2);
}
const sock = new net.Socket({ fd: FD, readable: true, writable: true });
```

Notes on the details, since both bit me while verifying:

- The fd **number** must be communicated, and env is the right channel. It is not a secret (an fd number is worthless without being the child that holds it), so this does not reintroduce the A09 §4.4 "never in argv" concern — but it is worth stating explicitly in a comment so nobody later "hardens" it back into a token.
- Do **not** feed `net.Socket({fd})` an fd that is not actually a socket: Node's `guessHandleType` throws `ENOTTY` from `open`, which is what happens if the `dup2` lands on the wrong descriptor. My first attempt failed exactly this way.
- `pre_exec` is `unsafe` because it runs between `fork` and `exec`; `dup2` is async-signal-safe, so this specific use is sound.
- The child no longer needs `Stdio::null()` gymnastics for the socket, but keep `stdin(Stdio::null())` — the engine must never inherit the tty (A09 T-SBX-2).

**Caveat:** verified with plain `node`, not with Electron's main process. Electron's main process is Node, so `net.Socket({fd})` should behave identically, but this is UNVERIFIED here because Electron will not start under the agent sandbox. It is a five-minute check outside the sandbox and should gate the change.

---

## 8. Reconnect, liveness, and flow control

### 8.1 Restart policy

With a socketpair there is nothing to reconnect *to*, which makes the model unambiguous and worth writing into the protocol doc:

> The engine never reconnects. The core owns engine lifetime and restarts it.

On EOF, `GOODBYE`, ping timeout, or a `crash` event with reason `crashed`/`oom`/`launch-failed`, the core:

1. reaps the child (existing `shutdown` logic, minus the socket-file cleanup);
2. paints a visible banner — `Status::crashed` already carries the reason and is currently dropped on the floor (F7);
3. respawns with `epoch + 1`, the last committed URL, and the current viewport;
4. resets `MessageReader` — **any partially buffered message must be discarded**, or the first bytes of the new epoch are appended to a truncated message from the old one;
5. retries at most 3 times with 250 ms / 1 s / 3 s backoff, then exits with a diagnostic.

`epoch` in `WELCOME` lets the core reject a stray late frame from the previous engine, and a `seq` that moves backwards without an epoch bump is itself a protocol fault.

### 8.2 Liveness

`PING` carries an 8-byte BE nonce; `PONG` echoes it. Core pings every 2 s when no frame has arrived for 2 s; two consecutive misses ⇒ `GOODBYE{timeout}` + restart. Five bytes of header plus eight of payload, at most every two seconds — the cost is nil and it converts "browser mysteriously frozen" into a recoverable, loggable event.

### 8.3 Frame ACK

`ACK` payload is `[u32 BE seq]`, sent by the core **after `stdout.flush()` returns** in `Renderer::present`. The engine allows at most `credit` unacked frames (default 1) and coalesces the rest exactly as it does today.

This is the one change that makes backpressure measure the right thing. Today `writeInFlight` clears when the *socket* accepts the bytes; with ACK it clears when the *terminal* has taken them. Under SSH (A07 §4.4) those differ by orders of magnitude, and the ACK round-trip time is also precisely the signal A07 §5.3 needs for its adaptive quality ladder — the same 9 bytes serve both purposes.

---

## 9. Conformance test plan

All of these run without Electron, without a tty, and without a display, so they belong in CI rather than in the manual e2e path.

| ID | Test | Asserts |
|---|---|---|
| **P-CAP-1** | Feed `[0x01][FF FF FF FF]` to the Rust reader | `Err(Oversize)`, and **zero** payload bytes buffered |
| **P-CAP-2** | Same to the JS framer | throws `ProtocolError{oversize}` |
| **P-CAP-3** | Drip 32 KiB of junk pre-handshake | `Err(Backlog)` on both sides |
| **P-CAP-4** | `T_EVENT` declaring `max_json + 1` | rejected even though it is under `max_frame` |
| **P-VER-1** | `HELLO` with `proto: 2` | core sends `GOODBYE{proto_mismatch}`, exit non-zero |
| **P-VER-2** | `HELLO` with `magic: "nope"` | `GOODBYE{bad_magic}` |
| **P-VER-3** | `HELLO` with `frame_header_len: 36` | `GOODBYE{header_mismatch}` — this is the drift detector |
| **P-VER-4** | No opener within 5 s | `GOODBYE{timeout}`, child killed |
| **P-EXT-1** | Message of type `0x90` followed by a valid `T_EVENT` | `0x90` skipped, event delivered |
| **P-EXT-2** | Message of type `0x42` | fatal `UnknownType` |
| **P-GEO-1** | Frame header `w=h=0x80000000` | `Err(BadGeometry)`; `expected_payload()` is `None`, not `Some(0)` |
| **P-GEO-2** | Frame header `format = 1` | `Err(BadFormat)` |
| **P-GEO-3** | Dirty rect extending past the frame | `Err(BadGeometry)` |
| **P-FLOW-1** | Queue a 200 KB command on a non-blocking socket with a slow reader | both messages arrive whole; peer reframes exactly 2; `dropped == 0` |
| **P-FLOW-2** | Fill the outbox past `MAX_OUTBOUND` | whole messages dropped, `dropped` counter increments, stream still aligned |
| **P-FLOW-3** | Destroy the socket mid-flush | `writeInFlight === false`; no pending `drain` listener |
| **P-INTEROP-1** | Rust framer ⇄ JS framer over a real socketpair, 1000 random messages | byte-identical round trip both directions |
| **P-CONST-1** | Parse the type-ID and `FRAME_HEADER_LEN` constants out of `main.js` and compare with `bg-proto` | constants agree (catches drift until `frame_header_len` in `HELLO` is live) |
| **P-FUZZ-1** | 10⁶ random byte streams into both framers | never panics, never throws a non-`ProtocolError`, never allocates past the cap |

P-CAP-1/2/3/4, P-EXT-1/2, P-GEO-1/2, P-FLOW-1/3 and split-reassembly are **already implemented and passing** in the scratchpad harnesses — `proposed.rs` (9 assertions) and `framer.test.js` (9 `node --test` cases). Lifting them into `crates/bg-proto/src/lib.rs` tests and a new `apps/engine/test/framer.test.js` is mostly a copy.

P-CONST-1 is worth calling out: until `frame_header_len` negotiation lands, a ten-line test that greps the JS constants and asserts they match the Rust ones is the only thing standing between you and a silent header-offset bug.

---

## 10. Suggested commit order

Ordered by risk-reduction per line changed. Each step is independently shippable.

1. **F1 outbound queue** (`apps/cli`). Fixes a user-triggerable session kill. No protocol change, no engine change. ~60 lines.
2. **F3 caps + F5 checked geometry** (`bg-proto`, `apps/engine`). Fatal-error plumbing on both sides, still no handshake. Tests in §9 come with it.
3. **F10 zero-copy** (`bg-proto` reader, `apps/engine` `cork`/`writev`). ~2 ms/frame back. Naturally follows step 2 since the reader is already being touched.
4. **F2 handshake** (`HELLO`/`WELCOME`/`GOODBYE`, `frame_header_len`, limits negotiation). Now the caps from step 2 become *negotiated* rather than hardcoded.
5. **F4 socketpair transport.** Deletes the listener, the path, the argv disclosure, and the race. Gate on the Electron smoke test in §11.
6. **F8 ping/pong + F7 restart policy + surfacing `Status::crashed`.**
7. **F6 frame ACK.** Unblocks A07 §5.3 adaptive pacing.
8. **F9 paste chunking + F13 argument validation + F11 parser fixes.**

Steps 1–3 are pure hardening with no wire change and no cross-repo coordination; they could go in today.

---

## 11. Explicitly UNVERIFIED

- **`net.Socket({ fd })` inside Electron 43.2.0's main process.** Verified with plain `node` v24.11.1 only. Electron's main process is Node, so this should hold, but Electron cannot start under the agent sandbox. **Smoke-test this before committing §7.**
- **`webContents.sendInputEvent` throughput.** The F9 amplification claim (one synchronous IPC per character) is read off the code, not measured — I could not run Electron. The *shape* of the bug is certain; the exact wedge duration for a given paste size is not.
- **`Buffer.poolSize` interaction under Electron.** The retention measurement (512×) is plain Node. Electron uses the same V8/Node buffer machinery, so I expect parity, untested.
- **Whether Chromium's OSR can ever emit a `format` other than 0**, or a strided bitmap. The engine hardcodes format 0 and the ADR records "verified non-strided", but I did not re-verify, and the proposed `BadFormat` rejection assumes the invariant holds. If a HiDPI or colour-managed path ever produces something else, that rejection becomes a hard failure rather than a fallback — worth an explicit decision.
- **macOS `SO_SNDBUF` on a socket created by `UnixListener`/`accept` rather than `UnixStream::pair`.** I measured 8192 on a socketpair. The listener path may differ; if it is larger, F1's threshold moves but the bug does not go away.
- **Anything about iTerm2.** Out of scope here and blocked by TCC per the project brief.

## 12. Licence

Nothing in this report reuses third-party code. All snippets are original, written against the standard libraries already in the workspace (`std`, `libc` — already a workspace dependency — and Node built-ins `net`, `node:test`, `node:assert`). No new dependency is proposed: the handshake is JSON and can be produced with the existing `json_escape` helper and consumed with a hardened `json_get_str`, so no `serde` is required. If the commander prefers a real JSON parser for the handshake specifically — a defensible call, since `HELLO`/`WELCOME` are the one place nested objects appear — that is a new dependency decision and should be weighed against A09 §8.2 supply-chain policy, not made incidentally here.
