# C01 — Kitty Graphics Protocol Conformance Audit

**Target:** `crates/bg-term/src/kitty.rs` (396 lines, read-only for this audit)
**Auditor:** Swarm agent C01
**Date:** 2026-07-31
**Verdict:** **Wire format is conformant. No frame-corrupting defect found.** Ten defects
found, none in the emitted grammar; the two most serious are a *silent-failure* channel
(`q=2`) and a *missing invariant* that would let a one-character future edit silently
corrupt every frame while all 70 tests still pass.

## Primary sources

| Source | URL | Used for |
|---|---|---|
| Kitty graphics protocol spec (rendered) | https://sw.kovidgoyal.net/kitty/graphics-protocol/ | Key table, chunking, deletion, query |
| Kitty spec source (RST, authoritative) | https://raw.githubusercontent.com/kovidgoyal/kitty/master/docs/graphics-protocol.rst | Exact normative wording |
| Kitty reference implementation | https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/graphics.c | `quiet` semantics, response guard, chunk state |
| Ghostty implementation (our verified terminal) | https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/terminal/kitty/graphics_command.zig | Key parsing, `C`, `o`, `q` |
| Ghostty command execution | https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/terminal/kitty/graphics_exec.zig | Chunk accumulation, quiet propagation |

Normative sentences relied on, in the spec's own words (short quotes):
chunking — payload is base64-encoded then chunked into chunks *"no larger than 4096 bytes"*
and *"All chunks, except the last, must have a size that is a multiple of 4."*
Continuations — *"Subsequent chunks must have only the m and optionally q keys."*
Compression — *"only RFC 1950 ZLIB based deflate compression is supported"* via `o=z`.
Cursor — `C=1` *"sets the cursor movement policy to no movement"*.
Query — a query action *"will not replace an existing image with the same id, nor will it store the image."*
Id reuse — *"the existing image and all its placements must be deleted."*

## Method

I did not trust the source comments. I built a throwaway harness outside the repo
(`$SCRATCH/kdump`, a detached cargo project with a path dependency on `bg-term`) that calls
the public encoder and prints every emitted byte with escapes rendered. No repo file was
modified. Baseline confirmed first: `cargo test -p bg-term` → **70 passed; 0 failed**.

### Byte-exact output actually produced

Single-chunk frame (2x2, level 6):

```
<ESC>_Ga=T,f=24,t=d,q=2,o=z,s=2,v=2,i=1000,C=1,m=0;eJxjYEAAAAAMAAE=<ESC>\
```

Multi-chunk frame (300x300 incompressible noise, level 1) — 88 chunks:

```
chunk[0]  ctrl="<ESC>_Ga=T,f=24,t=d,q=2,o=z,s=300,v=300,i=1000,C=1,m=1"  payload_len=4096
chunk[1]  ctrl="<ESC>_Gm=1"                                              payload_len=4096
chunk[87] ctrl="<ESC>_Gm=0"                                              payload_len=4012
payload sizes: all-but-last multiple-of-4 = true, max = 4096
```

Control sequences:

```
delete_image(31) = <ESC>_Ga=d,d=I,i=31<ESC>\
delete_all()     = <ESC>_Ga=d,d=A<ESC>\
support_query(31)= <ESC>_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA<ESC>\
```

Compression header: first base64 chars of the payload are `eJzt…` → first byte `0x78`,
i.e. genuine **RFC 1950 zlib**, not raw RFC 1951 deflate and not gzip. `flate2::write::ZlibEncoder`
is the correct constructor; `DeflateEncoder` or `GzEncoder` here would have been silently
wrong on every frame.

## Conformance findings — the items the mission named

| # | Item audited | Result | Evidence |
|---|---|---|---|
| 1 | Control key **names** (`a f t s v i c r z C m o q d`) | **PASS** | Every key emitted appears in the spec key table; no invented keys |
| 2 | Control key **values** (`a=T`, `f=24`, `t=d`, `o=z`, `d=I`, `d=A`, `C=1`, `q=2`, `a=q`) | **PASS** | All are legal values for their key |
| 3 | **4096 base64 chunk limit** | **PASS** | `kitty.rs:26` `MAX_CHUNK = 4096`; measured max payload 4096 |
| 4 | **Multiple-of-4 rule** on all but last chunk | **PASS (by luck, unguarded)** | 4096 % 4 == 0; measured `all-but-last multiple-of-4 = true`. See **D2** |
| 5 | **`m=` continuation semantics** | **PASS** | Continuations emit literally `<ESC>_Gm=1` — only `m`, exactly as the spec requires. `m=1` on all but last, `m=0` on last |
| 6 | **`o=z` valid for `f=24`** | **PASS** | Spec allows compression on any format; Ghostty's `Transmission.parse` reads `o` with no format check. Not PNG-only |
| 7 | **`C=1` cursor policy** | **PASS** | Ghostty's `Display` struct has `cursor_movement: CursorMovement = .after, // C` with `1 => .none`. Valid on `a=T` |
| 8 | **Deletion codes** | **PASS (code) / FAIL (doc)** | `d=I` = delete by id + free data ✓. `d=A` is *placements visible on screen*, not "every image". See **D6** |
| 9 | **First-chunk-carries-all-keys approach** | **PASS — confirmed correct** | See below. This was the riskiest question and it resolves in our favour |
| 10 | Image-id reuse every frame (`i=1000`, `a=T`) | **PASS** | Spec: re-transmitting an id deletes the existing image *and all its placements*. No placement accumulation, no leak |
| 11 | Query probe grammar | **PASS** | `support_query(31)` is byte-identical to the spec's own worked example, including the `AAAA` payload. See **D4** for the omitted DA1 |

### Item 9 in detail — is first-chunk-carries-all-keys correct?

Yes, and specifically the `q=2` set on chunk 0 **does** govern the response emitted after the
final chunk. I verified this in both implementations rather than assuming it, because if the
*last* chunk's (absent) `q` had won, every frame would have emitted an `OK` reply into our stdin.

Kitty keeps the first chunk's command in `self->currently_loading.start_command` and updates
only `more` and `payload_sz` per chunk; the response is generated from that preserved struct.
Ghostty does the same through its `LoadingImage`: on a continuation with no `q`, the `.no`
branch restores the accumulated value (`.no => quiet = loading.quiet`).

**Conclusion: the design is right.** Do not "fix" it by repeating keys on continuations —
that would violate the spec's *only `m` and optionally `q`* rule.

## Defects

Ordered by severity. None of these corrupt the current wire output; D1 and D2 are the ones
that matter for a renderer whose whole output depends on this file.

### D1 — `q=2` silently suppresses errors as well as OK responses (High)

`kitty.rs:130` emits `q=2` on every frame. The spec sentence reads as though `1` suppresses OK
and `2` suppresses failures, which invites reading `q=2` as "errors only". Both reference
implementations suppress **both** at `q=2`:

- Kitty `graphics.c`: `if (g->quiet) { if (is_ok_response || g->quiet > 1) return NULL; }` —
  `quiet=2` makes the second disjunct true for errors and the first true for OK.
- Ghostty `graphics_exec.zig`: `.failures => null` — returns no response at all.

**Impact.** Every failure mode of the renderer is invisible: bad dimensions, zlib stream error,
image-storage quota exceeded, unsupported format. We would observe "the screen is blank" with
zero diagnostic bytes. For the component the mission calls "our whole renderer depends on it",
having no error channel at all is the single biggest operational risk in the file.

**Recommendation.** Keep `q=2` for the steady-state hot path — it is the correct choice there,
and it is what protects D3. Add a diagnostic mode that emits `q=1` (errors still delivered) and
route replies to a log. That requires D3's decoder fix first.

Spec: https://sw.kovidgoyal.net/kitty/graphics-protocol/

### D2 — `MAX_CHUNK` has no multiple-of-4 invariant, and the test cannot catch a violation (High)

`kitty.rs:26` defines `MAX_CHUNK = 4096` and `kitty.rs:121` slices with `b64buf.chunks(MAX_CHUNK)`.
Correctness depends on `MAX_CHUNK % 4 == 0`, which is nowhere asserted or even mentioned. The
guarding test, `chunking_respects_the_4096_byte_limit` (`kitty.rs:305`), only asserts
`payload_len <= MAX_CHUNK` — it passes for any value.

Proof that the rule is load-bearing, not decorative (simulating a decoder that decodes each
chunk as it arrives, which is what kitty does):

```
chunk_size=8: all-but-last%4==0=True  -> incremental decode IDENTICAL to source
chunk_size=7: all-but-last%4==0=False -> incremental decode CORRUPT (354 bytes vs 372)
```

So changing `4096` to `4095` — a plausible "let's leave headroom" edit — would corrupt every
frame on every terminal, and **all 70 tests would still pass**.

**Recommendation.** Add `const _: () = assert!(MAX_CHUNK % 4 == 0, "kitty spec: all but the last base64 chunk must be a multiple of 4");`
next to the constant, and strengthen the test to assert `payload_len % 4 == 0` for every chunk
except the last. Zero runtime cost, converts a silent data-corruption regression into a build error.

Spec: https://sw.kovidgoyal.net/kitty/graphics-protocol/

### D3 — No APC/DCS branch in the input decoder; only `q=2` prevents keystroke injection (High)

Not in `kitty.rs`, but it is the failure mode `kitty.rs` is implicitly relying on, so it belongs
in this audit. `crates/bg-term/src/input.rs` `step()` dispatches on `self.buf[1]` with arms for
`b'['` (CSI) and `b'O'` (SS3) only; everything else falls to the catch-all that decodes
`ESC <char>` as **Alt+char**. There is no `ESC _` (APC) or `ESC P` (DCS) arm — I grepped for one
and the only `b'P'` hit is an SS3 F1 mapping at `input.rs:318`.

A single unsolicited reply `ESC_Gi=1000;OK ESC\` therefore decodes as roughly twelve injected
keystrokes: `Alt+_`, then `G i = 1 0 0 0 ; O K` as literal characters, then `Alt+\`.

The exposure is real today because **`delete_image()` (`kitty.rs:211`) carries no `q=` key**, and
`caps.rs:151` calls it during startup detection. Kitty happens not to answer deletes — its
`finish_command_response` returns NULL when `data_loaded` is false — but that is an implementation
detail, not a spec guarantee, and Ghostty's response path is structured differently
(`.no => if (resp.empty()) null else resp`).

**Recommendation.** Two independent fixes, both cheap: add `q=2` to `delete_image` and
`delete_all` so no fire-and-forget command can ever elicit a reply; and add an APC/DCS
swallow branch to the decoder so a stray reply is discarded rather than typed.

### D4 — `support_query` omits the DA1 tail from the spec's own detection recipe (Medium)

`kitty.rs:225-234` produces exactly the spec's example probe but stops at the ST. The spec's
recommended detection sequence appends a primary device attributes request, i.e.
`…f=24;AAAA <ESC>\ <ESC>[c`, precisely so that a terminal *without* graphics support still
answers something (the DA1) instead of leaving the client waiting.

`caps.rs:143` sends our probe alone and only issues `ESC[c` as a separate later query with its
own deadline. On the terminals we have already measured as non-supporting — Apple Terminal 465,
DA1 `?1;2c`, no kitty graphics — this burns the entire `deadline_ms` before falling through.
That is pure startup latency on exactly the terminals we know will fail.

**Recommendation.** Have `support_query` return the probe with `\x1b[c` appended, and let
`caps.rs` treat "DA1 seen without `_G…OK`" as a definitive negative. This makes non-kitty
detection immediate instead of timeout-bound, and it is what the spec tells clients to do.

Spec: https://sw.kovidgoyal.net/kitty/graphics-protocol/

### D5 — No fallback when deflate expands the payload (Medium)

`kitty.rs:111-116` compresses unconditionally whenever `compress_level > 0` and always tags the
result `o=z`, even when compression made things worse. Measured on the noise frame:

```
raw 270,000 -> deflate 270,272   (+0.1%, at level 1 — the level the CLI actually uses)
```

`apps/cli/src/main.rs:865` passes level 1. Photographic images, video frames, gradients and
dithered content are common on real pages and are exactly where deflate loses. We currently pay
both the CPU of deflating and a larger payload.

**Recommendation.** After deflating, if `deflated_len >= raw_len`, send the raw RGB and omit
`o=z`. Fully spec-legal (`o` is optional), a few lines, strictly better in both bytes and latency.

### D6 — `delete_all` doc overstates `d=A`, and the sequence is duplicated in `tty.rs` (Medium)

Two related problems.

The doc comment at `kitty.rs:218` says "Delete every image." Per the spec's deletion table,
`d=a`/`d=A` addresses **all placements visible on screen**; uppercase additionally frees the
image data of those placements. Images held in terminal storage with no visible placement are
not touched. The comment should say what the code does.

More importantly, the sequence is written twice. `kitty.rs:220` emits `\x1b_Ga=d,d=A\x1b\\`, and
`crates/bg-term/src/tty.rs:33` hardcodes the identical literal inside `RESTORE_SEQ`, pinned by a
test at `tty.rs:233`. `kitty::delete_all` has **no callers anywhere in the repo** — I grepped.
Two sources of truth for the escape that enforces the crate's stated top invariant ("the terminal
must always be restored") is exactly where drift will happen.

The current *ordering* in `RESTORE_SEQ` is correct and worth preserving: `d=A` is sent while still
on the alternate screen, before `\x1b[?1049l`, so the visible placement is the one deleted.

**Recommendation.** Have `tty.rs` build `RESTORE_SEQ` from `kitty::delete_all()` (or add a
`pub const DELETE_ALL: &[u8]` in `kitty.rs` that `tty.rs` references), and correct the comment.

### D7 — Zero-size frame reports a chunk that was never emitted (Low)

`kitty.rs:122` computes `let chunk_count = chunks.len().max(1);` but the emit loop iterates
`chunks`, which is empty when the base64 buffer is empty. Reproduced:

```
encode_rgb_frame(&[], 0, 0, Placement::default(), 0, &mut out)
  -> reported chunks=1, wire_bytes=0, actual output=""
```

`EncodeStats` claims one chunk while nothing was written. The `.max(1)` is guarding against a
division that does not exist here. Any caller doing bookkeeping on `stats.chunks` is misled, and
the existing chunk-counting test would fail on this input if it were exercised.

**Recommendation.** Return early with `chunks: 0` on an empty payload, or reject `w == 0 || h == 0`
outright — `s=0,v=0` would be an invalid command anyway.

### D8 — `PAGE_IMAGE_ID = 1000` is not namespaced, contrary to its comment (Low)

`kitty.rs:29` claims ids "are namespaced by us so we never collide with an image some other
program left behind". There is no namespacing — 1000 is a plain constant in a global,
terminal-wide id space. Per the spec, transmitting an id that already exists deletes the
existing image *and all its placements*, so a collision destroys the other program's image
rather than merely conflicting.

The protocol's intended answer is `I=` (image number), where the terminal allocates a free id
and reports it back — but that reply is suppressed by our `q=2` (D1), so adopting it means
adopting a read-back path too.

**Recommendation.** For now, correct the comment to state the real risk. Revisit `I=` if and
when the D1 diagnostic path lands.

### D9 — Teardown can interleave with an in-flight chunked transfer (Low)

The spec requires a client to finish all chunks for one image before sending any other graphics
escape code. `present()` in `apps/cli/src/main.rs` writes the whole frame — measured at 361,200
bytes for the 300x300 noise case, and ~54 KB for the real Ghostty frame — through a single
`write_all`. A SIGINT arriving mid-write runs `restore_raw()`, which injects
`\x1b_Ga=d,d=A\x1b\\` into the middle of a partially transmitted chunked command.

Low likelihood, self-limiting (we are exiting anyway), but it is a genuine protocol violation and
the terminal's `currently_loading` state is left dangling.

**Recommendation.** Track an "in a chunked transfer" flag and have the restore path emit the
final `m=0` terminator before the delete. Note this only matters if BlackGlass ever leaves the
alternate screen with images intended to persist.

### D10 — Dead code and one unreachable panic (Low / informational)

`wrap_tmux` (`kitty.rs:241`) and `bgra_rect_to_rgb` (`kitty.rs:54`) have no callers anywhere in
the repo. `wrap_tmux`'s ESC-doubling is correct for tmux `allow-passthrough` — I traced it:
the inner terminator `ESC \` becomes `ESC ESC \`, which tmux unwraps back to `ESC \`. But since
nothing routes through it, **the tmux path is UNVERIFIED**; do not claim tmux support.

Separately, `itoa` (`kitty.rs:172`) negates with `v = -v`, which panics in debug on `i64::MIN`.
Unreachable from current call sites (all inputs are `u32` or `i32`), so this is a note, not a bug.

## What I could not verify

- **iTerm2 3.6.9** — TCC blocks automation on this machine, consistent with the brief. Its kitty
  graphics support is UNVERIFIED here; my Ghostty findings should not be assumed to transfer.
- **tmux passthrough end-to-end** — `wrap_tmux` is uncalled, so no live evidence exists.
- **Terminal-side behaviour under `q=1`** — not exercised, since nothing in the codebase emits it.
- All implementation claims about kitty and Ghostty come from reading their published sources at
  `master`/`main`, not from running those terminals against instrumented input.

## Recommended fix order

1. **D2** — one `const _: () = assert!(…)` plus one strengthened assertion. Highest
   risk-reduction per line of code in the whole file; prevents a silent corruption regression.
2. **D3** — add `q=2` to the two delete helpers and an APC/DCS swallow arm in the decoder.
   Closes the keystroke-injection path and unblocks D1.
3. **D1** — a diagnostic mode at `q=1`, so renderer failures stop being invisible.
4. **D5** — skip `o=z` when deflate expands. Direct win on the frame budget.
5. **D4** — append DA1 to the probe. Removes a full timeout from startup on non-kitty terminals.
6. **D6–D10** — correctness of comments, de-duplication, and edge cases.

Items 1, 2, 4 and 5 are each a handful of lines and none change the wire grammar that was
verified working end-to-end in Ghostty 1.3.1.
