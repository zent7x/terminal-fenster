# B05 — `useSharedTexture` on macOS: anatomy, contract, and whether it is ever worth it

- **Author:** B05 (swarm)
- **Date:** 2026-07-31
- **Machine:** macOS 26.1, Apple M4, arm64, Electron 43.2.0 / Chromium 150.0.7871.129
- **Status:** Complete. All claims below are measured on this machine unless marked UNVERIFIED.
- **Scope note:** This report writes only to itself. Two proposed changes to `crates/tf-term/src/kitty.rs` are described in §7 rather than made, per file-ownership rules.

---

## 1. Verdict

Do not build the native addon. Three independent findings each defeat the proposal on their own, and the third one redirects the effort somewhere far more valuable.

The performance advantage recorded in ADR-0001 does not reproduce. That ADR compared a single paired run and found shared-texture p99 ahead by 1.29 ms. Over ten paired runs of the same spike on the same machine, shared texture came out **0.52 ms slower** on mean p99 (t = 0.41, not significant), and the run-to-run noise band inside a single mode is **7× larger than the effect the ADR measured**. There is no measurable tail-pacing benefit to buy.

Zero-copy to a terminal is structurally impossible, not merely hard. The kitty graphics protocol transports base64 of deflated pixel bytes over a pty. Every pixel must be read by the CPU to be compressed. The best case is not "no copy", it is "one copy instead of two", and the copy is nowhere near the dominant cost.

The real bottleneck is a scalar loop in our own code. `kitty::bgra_to_rgb` (`crates/tf-term/src/kitty.rs:40`) runs at **0.94 GiB/s** and costs 7.98 ms per frame at the real Ghostty resolution. A byte-identical replacement runs at 23.87 GiB/s and costs 0.32 ms. **Fixing that one loop saves 7.66 ms per frame — roughly five times more than the entire zero-copy project could ever save — and it is a contained change to a single function that already has a scalar reference to test against.**

---

## 2. What `event.texture` actually is

Probe: `texture-anatomy.js`, 3 s, 1200×800, `offscreen: { useSharedTexture: true }`. Full runtime dump, not inference from the type definitions.

The `paint` event carries exactly one own property, `texture`. That object has two own properties, `release` (a function) and `textureInfo`. The `image` argument is **not null** — it is a `NativeImage` with `isEmpty() === true`. That is a live footgun: any handler written as `if (image) { ...use bitmap... }` passes the truthiness check and then silently processes zero pixels. Checking `event.texture` first, as `spike/fps-matrix.js:78` already does, is the correct order.

`textureInfo` on this run:

| Field | Value |
|---|---|
| `pixelFormat` | `bgra` |
| `codedSize` | `{1200, 800}` (2482×814 confirmed at the real terminal size) |
| `visibleRect` / `contentRect` | full frame, `{0,0,1200,800}` |
| `colorSpace` | `{primaries: bt709, transfer: srgb, matrix: rgb, range: full}` |
| `widgetType` | `frame` |
| `metadata.captureUpdateRect` | populated, full-frame on this content |
| `metadata.frameCount` | `0` |
| `timestamp` | exact 16666 µs multiples: 0, 16666, 33332, 49998 … |
| `handle` | one own key: `ioSurface` |

### The handle is a raw pointer, not a mach port

`handle.ioSurface` is a **Node `Buffer` of exactly 8 bytes** containing a little-endian 64-bit value. First frame: hex `40619d0114010000` → `0x0000_0114_019d_6140`. Every sampled value falls in the narrow range `0x01140004b560`–`0x0114019dfe80`, which is an in-process CoreFoundation heap range on arm64 macOS.

This is an `IOSurfaceRef` — a `CFTypeRef` pointer valid **only inside the Electron main process address space**. It is not a mach port name (those are 32-bit), not an `IOSurfaceID` (also 32-bit, and that is the one that *is* global), and not shareable across a process boundary as-is. This matches the type definition note at `electron.d.ts:13525`: *IOSurfaceRef holds the shared texture. Note that this IOSurface is local to current process (not global).*

The practical consequence: our architecture sends frames over a Unix socket to a separate Rust process. **That pointer is meaningless there.** Any native addon must therefore live inside the Electron main process, map the surface, and copy the pixels out — which is precisely the copy the feature was supposed to eliminate.

### Handles are recycled from a shallow pool

Across 24 consecutive sampled frames there were only **13 distinct handle values**, with the same pointer recurring on runs of up to four consecutive frames (`0x0114019d2b20` four times, then `0x0114019dfe80` four times).

Two consequences follow. Pointer identity cannot be used to detect a duplicate or dropped frame. And the pool is shallow enough that lifetime discipline is not a hygiene concern but a hard operational limit — quantified next.

---

## 3. The `release()` contract, measured

Probe: `release-contract.js`, 6 s, 800×600, one process per sub-experiment, sampling delivered frame count every 250 ms.

| Sub-experiment | Frames in 6 s | Stalled? | Outcome |
|---|---|---|---|
| `proper` — release each frame | 352 | no | baseline, 58.7 fps |
| `never` — never release | **11** | **yes, permanently** | delivery dies at ~315 ms |
| `double` — release twice | 352 | no | does **not** throw, does not crash |
| `after` — release in `setTimeout(…, 0)` | 345 | no | deferred release is legal |
| `readafter` — read handle after release | 351 | no | handle still readable, **value unchanged** |
| `delayed` — hold, release 500 ms later | 130 | no | throughput collapses to 21.7 fps |

### Missing a release is a silent, permanent freeze after 11 frames

The `never` result reproduced exactly three times: **11 frames, every run** (last arrivals 315.09 ms, 305.35 ms, 342.97 ms). That is ~183 ms of content at 60 Hz. After that, `paint` never fires again for the remaining 5.7 s.

The failure is completely silent. Across all three runs: **zero bytes written to stderr**, no `render-process-gone`, no `unresponsive`, no thrown exception, no Chromium warning. The browser simply stops painting forever, and nothing anywhere in the system says why.

This is the single worst property of the API for our use case. Our paint handler will eventually do real work — BGRA conversion, deflate, socket write — and any one of those can throw. One uncaught exception on the path before `release()` and Terminal-Fenster freezes with no diagnostic, eleven frames later. Surviving this requires `try { … } finally { texture.release(); }` with absolutely no exceptions, forever, on every path. The good news from the `double` experiment is that over-releasing is safe, so a defensive `finally` cannot itself cause harm.

### Reading the handle after release is a use-after-free

In `readafter`, the hex before and after `release()` were identical (`00a99c013c010000`), no error was raised, and `textureInfo` remained fully readable. The JS object is not poisoned or invalidated in any way.

Combined with §2's finding that the pool recycles aggressively, this means a native addon that copies from the pointer after releasing will read an `IOSurface` the compositor may have already refilled with a different frame — producing torn or simply wrong pixels, with no error and no crash to signal it. There is no runtime guard here at all; correctness rests entirely on discipline.

### Hold-time budget

Probe: `copy-cost.js texhold <ms>`, 2482×814, busy-wait inside the paint handler to model synchronous native work.

| Hold per frame | 0 ms | 2 ms | 5 ms | 10 ms | 20 ms |
|---|---|---|---|---|---|
| Delivered fps | 57.6 | 57.4 | 57.9 | 57.7 | **47.5** |

Up to 10 ms of synchronous work per frame is free; 20 ms costs 18% of frame rate. So a native addon does have a workable window for lock-and-copy. But note what the `delayed` experiment showed: holding textures *across* frames rather than within one halves throughput (130 frames vs 352). The pool depth of 11 is the entire budget, and it is spent on latency, not on parallelism.

---

## 4. What a native addon would actually have to do

This section is design-level. **UNVERIFIED — no addon was built**, per the disk and scope constraints. The API sequence is standard `IOSurface` usage; the alignment hazard in §5 *is* measured.

The addon would be a Node native module loaded into the Electron **main** process (the only process where the pointer is valid). Per frame it would receive the 8-byte Buffer, reinterpret it as `IOSurfaceRef`, and then:

`CFRetain` the surface immediately (Chromium may release its own reference the moment we call `texture.release()`), then `IOSurfaceLock(surface, kIOSurfaceLockReadOnly, &seed)`, read `IOSurfaceGetBaseAddress`, `IOSurfaceGetBytesPerRow`, `IOSurfaceGetWidth`, `IOSurfaceGetHeight`, copy row-by-row honouring the stride, then `IOSurfaceUnlock`, `CFRelease`, and finally `texture.release()`.

For a Metal path instead of CPU, the surface maps via `MTLDevice.newTextureWithDescriptor:iosurface:plane:`, which is genuinely zero-copy into a `MTLTexture`. That is the real use case this Electron feature was designed for — feeding an external GPU rendering pipeline, exactly as `electron.d.ts:17200` describes. It is not our use case, for the reason in §6.

The build cost is a `node-gyp` toolchain, a per-Electron-ABI native binary, code signing and notarisation for a `.node` binary inside a distributed app, and a second unsafe language on the frame hot path. Against that, §7 shows the alternative is a change to one function.

---

## 5. Measured landmine: the IOSurface is strided, the bitmap is not

Probe: `iosurface-probe.c`, linked against `IOSurface` and `CoreFoundation`, creating BGRA surfaces at our real resolutions and reading `IOSurfaceGetBytesPerRow`.

| Resolution | `w*4` | `bytesPerRow` | Padding | Verdict |
|---|---|---|---|---|
| 1440×900 (ADR spike) | 5760 | 5760 | 0 | non-strided |
| 800×600 | 3200 | 3200 | 0 | non-strided |
| 1920×1080 | 7680 | 7680 | 0 | non-strided |
| **2482×814 (real Ghostty window)** | **9928** | **9984** | **56** | **STRIDED** |
| 2482×851 (full Ghostty height) | 9928 | 9984 | 56 | STRIDED |
| 1200×800 | 4800 | 4864 | 64 | STRIDED |
| 2481×813 | 9924 | 9984 | 60 | STRIDED |
| 1023×767 | 4092 | 4096 | 4 | STRIDED |

IOSurface aligns rows to 64 bytes. Whether that produces padding depends entirely on the width.

This is a nasty trap because of *which* resolutions are clean. The bitmap path is verifiably non-strided — Probe 3 measured `image.getBitmap().length === 8081392 === 2482*814*4` exactly, and `kitty::bgra_rect_to_rgb` at `crates/tf-term/src/kitty.rs:57` hard-codes `let stride = img_w as usize * 4` on that basis. Every round-number test resolution (1440×900, 800×600, 1920×1080) is also non-strided under IOSurface, so that assumption would pass the entire existing test suite. It breaks at **2482×814 — the actual measured Ghostty 1.3.1 window on this machine**, producing a progressively sheared image.

Caveat, stated plainly: this probe measures `IOSurfaceCreate`'s alignment policy on this machine, which is strong evidence for how the allocator behaves, but it is **not** a direct read of `IOSurfaceGetBytesPerRow` on Chromium's own surface — that read requires the native addon this report recommends against building. Treat the specific padding values as UNVERIFIED for Chromium's surfaces; treat "you must read the stride, never assume `w*4`" as established.

---

## 6. Is zero-copy to a terminal possible at all?

No, and the reason is structural rather than an implementation gap.

The kitty graphics protocol carries an APC escape containing **base64 of deflated pixel bytes** written to a pty file descriptor. Working backwards: the pty needs bytes, base64 needs bytes, deflate needs to scan every byte, and deflate is a serial CPU algorithm operating on CPU-addressable memory. There is no path from a GPU texture to a terminal that does not pass every pixel through the CPU. The half-block Unicode fallback is worse still — it needs per-pixel access to compute cell colours.

So the honest framing of the choice is not "copy versus no copy". It is: **the bitmap path costs one `getBitmap()` copy; the shared-texture path costs one `IOSurfaceLock` + manual `memcpy`.** Both land the same 7.71 MiB in CPU memory.

Measured costs at 2482×814, 8,081,392 bytes per frame:

| Operation | p50 | p95 | p99 | Throughput |
|---|---|---|---|---|
| `image.getBitmap()` (bitmap path) | 2.206 ms | 7.767 ms | 17.576 ms | 3.41 GiB/s |
| `image.getSize()` (metadata only, control) | 0.016 ms | 0.028 ms | — | — |
| pure `memcpy` of the frame (floor for any copy) | 0.726 ms | — | — | 10.36 GiB/s |

The theoretical ceiling on what zero-copy can save is therefore `2.206 − 0.726 ≈ 1.48 ms` at p50, and that assumes `IOSurfaceLock` overhead is zero, which it is not (UNVERIFIED, not measured). Call it **~1.5 ms per frame, best case.**

Now weigh that against what it is a share of. Probe 4b, running the actual `tf-term` functions across realistic page content:

| Content | zlib | `bgra_to_rgb` | deflate + base64 + chunk | Total | Wire bytes | Ceiling |
|---|---|---|---|---|---|---|
| blank white | L1 | 15.14 ms | 3.22 ms | 18.36 ms | 37,151 | 54.5 fps |
| example.com-like | L1 | 10.34 ms | 2.47 ms | 12.80 ms | 43,136 | 78.1 fps |
| text page | L1 | 8.67 ms | 2.86 ms | 11.53 ms | 108,272 | 86.7 fps |
| photo / video | L1 | 7.26 ms | 65.24 ms | 72.50 ms | 7,380,719 | 13.8 fps |
| text page | L3 | 6.29 ms | 11.65 ms | 17.94 ms | 42,068 | 55.7 fps |
| example.com-like | L3 | 7.38 ms | 12.32 ms | 19.70 ms | 11,480 | 50.8 fps |
| text page | L6 | 7.24 ms | 15.39 ms | 22.62 ms | 25,652 | 44.2 fps |
| photo / video | L6 | 8.01 ms | 295.61 ms | 303.62 ms | 6,673,395 | 3.3 fps |

The CPU encode path already fails to hit 60 fps on ordinary text content at L3. Frame acquisition is not the constraint and is not close to being the constraint. Spending a native addon to shave ~1.5 ms off a 17.94 ms pipeline is optimising the wrong term.

Two secondary findings worth recording. Deflate cost is violently content-dependent — 2.5 ms for near-blank pages, 296 ms for photographic content at L6, a 100× spread — which means **compression level must be adaptive, not a constant**, and that is a far more urgent scheduling problem than frame acquisition. And L1 is dramatically better than its compression ratio suggests: on text-page content L1 costs 2.86 ms for 108 KB of wire, L3 costs 11.65 ms for 42 KB. Over a local pty, L1 is plainly correct; over SSH the trade flips. That belongs to A07/A10, flagged here only because it dwarfs the effect this report was asked to evaluate.

---

## 7. The actual bottleneck, and the change worth making

Probe: `convopt.rs`, 2482×814 text-page content, 40 iterations, `--release` with the workspace profile.

| Implementation | p50 | Throughput | Speedup |
|---|---|---|---|
| `scalar_current` — what `kitty.rs:40` does today | 7.98 ms | 0.94 GiB/s | 1.0× |
| `unrolled` — `get_unchecked` + raw pointer writes, no SIMD | 0.33 ms | 22.78 GiB/s | **24.2×** |
| `neon` — `vld4q_u8` / `vst3q_u8` | 0.32 ms | 23.87 GiB/s | **25.3×** |
| `swap_in_place` — BGRA→RGBA, no repack | 0.40 ms | 18.79 GiB/s | 19.9× |

The NEON output was verified **byte-identical** to the crate's existing scalar function on a full 2482×814 frame.

The current implementation pushes three bytes at a time into a `Vec` through `Vec::push`, which cannot be vectorised because each push carries a capacity check the optimiser will not hoist across the loop. At 0.94 GiB/s it is running roughly an order of magnitude below what the memory subsystem allows — the pure `memcpy` on the same machine hits 10.36 GiB/s, and the deinterleaving NEON version beats even that at 23.87 GiB/s because it reads and writes in wide aligned bursts.

Note that the plain `unrolled` variant captures 24.2× of the available 25.3× **with no architecture-specific intrinsics at all**. The portable version gets essentially all of the win, so this does not require committing to an aarch64 code path.

Putting the whole pipeline together at 2482×814, text-page content, zlib L3, against a 16.67 ms budget:

| Configuration | Acquire | Convert | Encode | Total | Ceiling | 60 fps? |
|---|---|---|---|---|---|---|
| today | 2.21 ms | 7.98 ms | 11.65 ms | 21.84 ms | 45.8 fps | **no**, 5.17 ms over |
| fix the conversion only | 2.21 ms | 0.32 ms | 11.65 ms | 14.18 ms | 70.5 fps | **yes**, 2.49 ms spare |
| conversion fix + native zero-copy addon | 0.73 ms | 0.32 ms | 11.65 ms | 12.70 ms | 78.7 fps | yes, 3.97 ms spare |

The conversion fix alone moves the pipeline from failing the frame budget to clearing it with headroom. The native addon then adds 1.48 ms of headroom we do not need, at the cost of a native toolchain, a second unsafe language on the hot path, the silent-freeze hazard from §3, and the stride hazard from §5.

Two changes to `crates/tf-term/src/kitty.rs` are therefore proposed for the commander, **described rather than made** per file ownership:

The first is to rewrite the body of `bgra_to_rgb` (`kitty.rs:40`) to reserve once and write through an unchecked pointer, keeping the existing signature. The existing scalar implementation should be retained as a `#[cfg(test)]` reference so the byte-identical property is asserted by a test rather than by this report. `bgra_rect_to_rgb` (`kitty.rs:54`) has the same per-byte push pattern and the same fix applies.

The second is a documentation correction. The comment at `kitty.rs:36-38` justifies dropping alpha as saving "33% more bytes". Measured, that is true before deflate and false after it: RGB deflates to 31,443 bytes and RGBA to 31,429 bytes on identical content, a difference of −0.0%. Zlib eliminates the constant alpha plane entirely. Dropping alpha is still the right call, but the real reason is that it hands deflate 6.06 MB to scan instead of 8.08 MB, which is a *time* saving, not a wire saving. Worth correcting so nobody later "optimises" wire size by revisiting a premise that does not hold.

---

## 8. Discrepancy flagged for reconciliation

The mission brief records, as verified, that an 8,081,424-byte BGRA frame at 2482×814 was "encoded to 53,999 wire bytes in 0.74 ms".

The wire size reconciles well — my L3 run on synthetic example.com-like content produced 55,923 bytes at 145.0× reduction against the brief's 53,999 at ~150×. **The 0.74 ms does not reconcile.** Running `kitty::bgra_to_rgb` followed by `kitty::encode_rgb_frame` at that exact resolution, I could not get the full path below **11.53 ms** for any content, including a completely blank white frame (18.36 ms at L1). Deflating 6.06 MB in 0.74 ms would require ~8 GiB/s of zlib throughput, which is not achievable.

The most likely explanation is that the 0.74 ms timed only part of the path — plausibly the escape-sequence assembly and chunking after compression, or `encode_rgb_frame` on a pre-converted buffer with an unrepresentative frame. It is also within noise of the 0.726 ms pure `memcpy` measured here, which is suggestive.

This matters beyond bookkeeping. If 0.74 ms stands, the pipeline has enormous headroom and none of §7 is urgent. If ~12–22 ms stands, **Terminal-Fenster is already encode-bound below 60 fps** and the conversion fix is the top-priority performance work in the project. I recommend the commander re-time the end-to-end path with the stage boundaries made explicit before any further performance planning depends on that number.

---

## 9. Reproduction

All probe sources are in the session scratchpad at
`/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/b05/`.
Nothing was written into the repository except this file.

Electron probes require the harness sandbox disabled, for the Mach-port rendezvous reason already documented in ADR-0001. Run from `apps/engine`:

```
./node_modules/.bin/electron <scratchpad>/b05/texture-anatomy.js
./node_modules/.bin/electron <scratchpad>/b05/release-contract.js {never|double|after|readafter|delayed|proper}
./node_modules/.bin/electron <scratchpad>/b05/copy-cost.js bitmap
./node_modules/.bin/electron <scratchpad>/b05/copy-cost.js texhold 10
```

Rust benches (`cargo build --release --offline`, path-dependency on `crates/tf-term`, no repo files modified):

```
<scratchpad>/b05/encbench/target/release/encbench 80   # pipeline cost by zlib level
<scratchpad>/b05/encbench/target/release/content       # cost by page content
<scratchpad>/b05/encbench/target/release/convopt       # conversion implementations
```

IOSurface alignment probe:

```
cc -O2 -o iosurface-probe iosurface-probe.c -framework IOSurface -framework CoreFoundation && ./iosurface-probe
```

Reproducibility statistics in §1 come from ten paired runs of the existing `apps/engine/spike/fps-matrix.js` in `gpu` and `shared` modes, unmodified.

Licence check: Electron is MIT (`apps/engine/node_modules/electron/LICENSE`), so the API is free to use. No third-party code is proposed for reuse in this report.

---

## 10. Answers to the mission questions, condensed

**What does `event.texture` expose?** An 8-byte Buffer holding a raw `IOSurfaceRef` pointer valid only in the Electron main process, plus `textureInfo` metadata (BGRA, coded size, colour space, `captureUpdateRect`, exact 16666 µs timestamps). Not a mach port, not a global ID, not transportable to our Rust process.

**The `release()` contract, and what happens if you miss it?** Pool depth is exactly 11 frames, reproduced three times. Miss one release and painting stops permanently after ~183 ms with zero diagnostic output on any channel. Double-release is safe. Deferred release is legal. Reading the handle after release is silently permitted and is a genuine use-after-free against a recycled pool.

**What would a native addon need?** To live in the main process, `CFRetain`, `IOSurfaceLock(kIOSurfaceLockReadOnly)`, honour `IOSurfaceGetBytesPerRow` (**56 bytes of row padding at our real 2482×814 window**, where the bitmap path is exactly non-strided), copy out, unlock, release — all inside a `finally`, forever. Metal mapping via `newTextureWithDescriptor:iosurface:plane:` is genuinely zero-copy but useless to a terminal.

**Is zero-copy to a terminal possible?** No. Deflate and base64 are CPU algorithms over CPU memory; a pty takes bytes. The choice is one copy versus a different copy.

**Is the complexity ever justified?** Not on this evidence. The p99 advantage does not survive repetition (n=10: −0.52 ms, t = 0.41, noise band 7× the claimed effect). The maximum saving is ~1.5 ms per frame against a 17.94 ms pipeline. And a 24× win worth 7.66 ms per frame is sitting unclaimed in `kitty.rs:40`. Revisit only if a future profile shows the `getBitmap()` copy is the binding constraint — which would require the conversion fix and adaptive compression to land first, and on these numbers it still would not be.
