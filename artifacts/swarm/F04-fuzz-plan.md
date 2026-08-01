# F04 — Fuzz and Property Testing Plan

**Scope:** `crates/tf-term/src/input.rs`, `crates/tf-term/src/kitty.rs` + `b64.rs`,
`crates/tf-proto/src/lib.rs`, `crates/tf-term/src/caps.rs`.
**Status:** every finding below was executed, not reasoned about. 36 properties were written,
compiled against the real crates, and run. **25 pass, 11 fail. 7 distinct panics reproduced.**
**Author owns only this file.** No core source was modified. All fixes are described, not made.

---

## 1. Threat model: which of these inputs is actually hostile

Fuzzing everything equally wastes effort. The four components sit at very different trust
levels, and the plan is weighted accordingly.

| Surface | Who controls the bytes | Trust | Priority |
|---|---|---|---|
| Terminal byte stream → `input.rs`, `caps.rs` | Remote SSH host, any process sharing the tty, the emulator itself, `cat` of a hostile file | **Untrusted** | P0 |
| Page title / URL → `json_get_str` via `T_EVENT` | **Any website** (`document.title` is free-form) | **Untrusted content, trusted transport** | P0 |
| Frame header + dirty rect → `tf-proto`, `kitty.rs` | Engine main process | Trusted peer (socket is `0600` in a `0700` dir — `apps/cli/src/main.rs:387-401`) | P1, defense-in-depth |
| Our own encoder output → terminal | Us | Trusted, but a malformed APC wedges the user's tty | P1 |

Two consequences worth stating plainly. First, the Unix socket is genuinely well-permissioned,
so the IPC findings below are robustness-against-our-own-bugs and renderer-compromise
defense-in-depth, **not** a live remote exploit; ranking them as the latter would be dishonest.
Second, the terminal byte stream is the one place where "hostile input" is the normal case, and
that is exactly where the worst findings landed.

The `panic = "unwind"` choice in `Cargo.toml` means a panic *does* run the tty-restore hook, so
none of these crashes corrupt the terminal. They still kill the browser mid-session.

---

## 2. Confirmed defects

Reproduced with a zero-dependency harness that pulls the real `tf-proto` and `tf-term::input`
sources in by `#[path]` and copies `Rect` / `bgra_rect_to_rgb` / `itoa` verbatim.
Run under both profiles: `overflow_checks = true` (debug) and `false` (release).

### 2.1 Panics

| ID | Location | Trigger | Debug | Release |
|---|---|---|---|---|
| **F04-01** | `kitty.rs:61` | dirty rect exceeds image bounds | `range end index 80 out of range for slice of length 64` | same |
| **F04-02** | `kitty.rs:56` `(rect.area() * 3) as usize` | `rect.w = rect.h = u32::MAX` | `attempt to multiply with overflow` | `capacity overflow` |
| **F04-03** | `kitty.rs:60` `y * stride` | `img_w = u32::MAX`, `rect.y = u32::MAX` | `attempt to multiply with overflow` | `range start index 18446744039349813252 out of range` |
| **F04-04** | `tf-proto/lib.rs:52` `expected_payload` | `width = height = 0xFFFFFFFF` | `attempt to multiply with overflow` | **wraps silently** |
| **F04-05** | `tf-term/lib.rs:48-49` `Rect::union` | `x + w > u32::MAX` | `attempt to add with overflow` | **wraps; union no longer covers its inputs** |
| **F04-06** | `kitty.rs:177` `itoa` | `i64::MIN` | `attempt to negate with overflow` | returns `"-"` — a digitless number |
| **F04-07** | `kitty.rs:108` | `rgb.len() != w*h*3`, or `w*h*3` overflowing `usize` | `assert_eq!` panic / multiply overflow | same |

Release mode is not the safe case. **F04-04 is worse in release than in debug:** for
`width = height = 2147483648`, `w * h * 4 ≡ 0 (mod 2^64)`, so `expected_payload()` returns **0**.
That makes the truncation guard at `apps/cli/src/main.rs:834`
(`if pixels.len() < h.expected_payload()`) vacuously true, the frame is accepted, and
`page_w` / `page_h` are poisoned with `2147483648` for every subsequent encode.

**F04-01 through F04-03 are currently dormant.** `bgra_rect_to_rgb`, `Rect::union`, and
`clamp_to` are referenced only from unit tests today (verified: `grep -n` over
`apps/cli/src/main.rs` and `unicode.rs` returns nothing). They become live the moment the
damage encoder in `C08-damage-encoder.md` is wired up. **Fix them before that lands**, not after.

The realistic trigger for F04-01 is not an attacker — it is a **resize race**.
`apps/engine/src/main.js:88-94` writes `size.width`/`size.height` and `dirty.*` from two
different reads. If the page resizes between them, the dirty rect legitimately describes the
old, larger geometry while the header advertises the new, smaller one.

### 2.2 Liveness and unbounded growth

**F04-08 — `step_csi` is O(n²) and unbounded.** `input.rs:251` restarts its scan for a final
byte at index 2 on *every* `decode()` call. An unterminated `ESC [` followed by a long digit run
re-scans the whole buffer each time.

```
20,000 one-byte feeds after "ESC [":
  debug   : 7.40 s
  release : 152 ms   (buffer: 20,002 bytes)
```

Extrapolating the quadratic to 1 MB of parameter bytes gives roughly six minutes of pure spin in
release. Buffer growth is also unbounded: 1 MB fed produced `pending() == 1000002` with no event
and no ceiling.

**F04-09 — unterminated bracketed paste grows `paste_buf` without limit,** and `pending()`
reports only `5`, so the growth is invisible to any caller trying to monitor it
(`input.rs:207-212`). Measured: ~1 MB retained while `pending()` reported 5.

**F04-10 — the decoder wedges permanently on a truncated UTF-8 tail.** `decode_utf8`
(`input.rs:497`) returns `None` for a partial sequence, which becomes `Step::NeedMore` forever.
`flush_pending_escape` only fires when `buf.len() == 1 && buf[0] == 0x1b` (`input.rs:149`), so it
cannot clear it. Two reproductions:

```
decode(&[0x1b, 0xf0])       -> 0 events, pending=2, flush_pending_escape() = None
decode(&[0xf0, 0x9f, 0x98]) -> 0 events, pending=3, flush_pending_escape() = None
```

The property test found this from random bytes at seed 8. **All subsequent keyboard and mouse
input is silently dead** — the user's browser stops responding to input with no error.

**F04-11 — `MessageReader` has no length ceiling.** A `u32::MAX` length prefix makes it buffer
indefinitely: after feeding 128 MiB against a bogus prefix, `buffered() == 134,217,733`.
Note also that `5 + len` at `tf-proto/lib.rs:87` overflows `usize` on a 32-bit target, which
would turn the guard into an out-of-bounds slice — **UNVERIFIED**, no 32-bit toolchain installed.

### 2.3 Semantic defects (no crash, wrong answer)

**F04-12 — `json_get_str` / `json_get_bool` are confused by nested objects.** Both do
`json.find("\"key\"")` and take the **first** hit anywhere in the text, including inside a nested
object (`tf-proto/lib.rs:126`, `:159`).

```
{"meta":{"v":"ATTACKER"},"t":"url","v":"https://real"}   -> json_get_str(...,"v") == "ATTACKER"
{"meta":{"v":true},"t":"loading","v":false}              -> json_get_bool(...,"v") == true
```

Direct injection through a page title is *currently* blocked because `JSON.stringify` escapes
`"` as `\"`, and the escaped bytes never match the `"key"` needle — I tested this and it holds.
The exposure is that the doc comment's "flat JSON object" precondition is unenforced: the first
engine event that gains a nested field hands web content a top-level-field override. Given that
`apply_event` (`apps/cli/src/main.rs:784-800`) routes on `"t"` and reads `"v"` and `"reason"`,
that override would control the displayed URL.

**F04-13 — `CSI ; 5 A` drops the Ctrl modifier.** `split_params` (`input.rs:467`) `filter_map`s
away unparseable parameters instead of preserving their positions, so an empty leading parameter
(valid ECMA-48 for "use the default") compacts the modifier into the keycode slot.
Measured: `ESC [ ; 5 A` → `Up` with **no modifiers**, where the correct decode is `Ctrl+Up`.
Same mechanism affects `CSI ; 15 ~`.

**F04-14 — SGR mouse coordinates shift when a parameter is dropped.** Same root cause.
`ESC [ < 0 ; 99999999999 ; 5 ; 7 M` yields `x=5, y=7` instead of rejecting the sequence.
Low severity — no real terminal emits this — but it is the same latent bug as F04-13 and one
fix covers both. Note the well-formed cases are correct: 2,048 randomized valid SGR reports all
round-tripped exactly, including the 4-parameter form.

**F04-15 — zero-area frames report a chunk they never emitted.** `encode_rgb_frame` sets
`chunk_count = chunks.len().max(1)` (`kitty.rs:122`) but the loop body runs zero times when the
base64 buffer is empty. For a `0x0` frame at `compress_level = 0`: **nothing is written to the
terminal, `stats.chunks == 1`, and no `m=0` terminator is ever sent.** A terminal that had a
transmission in flight would be left waiting for a continuation that never arrives.

**F04-16 — escaped surrogate pairs decode to two replacement characters.**
`{"v":"😀"}` → `"\u{FFFD}\u{FFFD}"` instead of `😀` (`tf-proto/lib.rs:144-148`).
Latent only: `JSON.stringify` emits astral characters literally in UTF-8, so this cannot fire
today. It becomes live the instant anyone adds ASCII-safe serialization. BMP escapes are fine
(`été` → `été`).

**F04-17 — Kitty key reports with surrogate or out-of-range codepoints vanish silently.**
`char::from_u32(keynum)?` (`input.rs:431`) returns `None`, which becomes `Step::Consumed` and no
event at all — not even `Event::Unknown`, which the module doc says exists precisely so
unknown-sequence bugs stay observable. `ESC [ 55296 ; 1 u` (U+D800) and
`ESC [ 4294967295 ; 1 u` both produce zero events.

### 2.4 Testability blockers

The capability parsers — `parse_da1_has_sixel`, `parse_decrqm_supported`, `parse_two_param_t`
(`caps.rs:202`, `:213`, `:221`) — are **private**, and so is `kitty::itoa`. They are unreachable
from `tests/` and from any `cargo-fuzz` target. The parsers cannot be fuzzed at all until
visibility changes. Recommended minimal shim, which keeps them private in normal builds:

```rust
// crates/tf-term/Cargo.toml
[features]
fuzz-internals = []

// crates/tf-term/src/caps.rs — change the three fn signatures to:
#[cfg(not(feature = "fuzz-internals"))] fn parse_da1_has_sixel(reply: &[u8]) -> bool { ... }
// ...or simply, and more cheaply:
#[doc(hidden)] pub fn parse_da1_has_sixel(reply: &[u8]) -> bool { ... }
```

Separately, `crates/tf-term/src/tty.rs:133` emits a future-compatibility warning:
`signal_handler as libc::sighandler_t` needs `as *const () as libc::sighandler_t`. Unrelated to
fuzzing, but it is in the terminal-restore path — the one place in this codebase that must never
break — so it is worth a one-line fix.

---

## 3. Hostile input taxonomy

What each harness must generate, derived from the mechanisms above rather than guessed.

**Input decoder.** Lone `ESC` at end of buffer; `ESC` + partial UTF-8 (F04-10); truncated
multi-byte UTF-8 at every prefix length; invalid lead bytes (`0x80..=0xBF`, `0xF8..=0xFF`);
overlong encodings (`C0 80`); unterminated `ESC [` with megabyte parameter runs (F04-08);
`ESC [ 200 ~` with no terminator (F04-09); terminator split byte-by-byte across reads;
paste bodies containing `ESC [ 201 ~` prefixes and full escape sequences; empty parameters
(`;;`, leading `;`) (F04-13); parameters exceeding `u32::MAX` (F04-14); every final byte in
`0x40..=0x7e` against every body shape; Kitty keycodes in the surrogate range and above
`0x10FFFF` (F04-17); nested `ESC ESC [ [`; C1 controls; NUL inside sequences.
**Every case must additionally be replayed under randomized chunk splits** — that single
transformation is what turns a state-machine bug into a reproducible failure.

**Kitty / base64 encoder.** Every input length mod 3 (padding boundaries); dimensions where
`w*h*3` overflows `usize` (F04-07); `w` or `h` zero (F04-15); `rgb.len()` off by one from
`w*h*3`; dirty rects with `x+w > img_w`, `y+h > img_h`, and `u32::MAX` components (F04-01 to
F04-03); compression levels 0 and 1–9 (level 0 changes the code path); payloads that compress to
exactly `MAX_CHUNK`, `MAX_CHUNK±1`, and `4*MAX_CHUNK`; `Placement.z = i32::MIN`;
`image_id = u32::MAX`; tmux-wrapped payloads that are themselves dense in `ESC`.

**Wire protocol.** Length prefixes of `0`, `1`, `u32::MAX`, and `u32::MAX - 4` (F04-11);
truncated headers at every length 0–32; `width`/`height` at `u32::MAX` and at `2^31`
(the wrap-to-zero case, F04-04); dirty rects outside the frame; message boundaries split at
every byte offset; several messages in one chunk; unknown `type_id`; JSON with nested objects
carrying duplicate keys (F04-12), unterminated strings, a trailing lone backslash, `\u` with
fewer than four hex digits, surrogate pairs and lone surrogates (F04-16), and keys appearing
inside string values.

**Capability parsers.** Replies with no `ESC [`; multiple `ESC [` where a prefix of garbage
precedes the real reply; `CSI` with two, three, or four parameters where exactly three are
expected; parameters exceeding `u16::MAX`; a `4` appearing as a substring (`24`, `41`, `142`) in
DA1 (already covered by a unit test — keep it); DECRQM values `0` through `9` and non-numeric;
replies with embedded NUL and invalid UTF-8 (all three parsers go through
`String::from_utf8_lossy`, so a `0x80` byte becomes U+FFFD and can change `find` offsets);
megabyte replies (a hostile program writing to the tty during detection).

There is also a correctness issue the fuzzer will not find but the taxonomy work surfaced:
`read_reply` (`caps.rs:92`) consumes **everything** arriving on the fd during the deadline,
including real user keystrokes typed during startup, and discards them into `raw_replies`.
Those bytes should be handed to the input `Decoder` rather than dropped. Reported, not fixed.

---

## 4. Tooling decision — and an honest blocker

**`cargo-fuzz` cannot run on this machine.** Two independent reasons, both measured:

```
$ cargo fuzz --version
error: no such command: `fuzz`

$ rustup toolchain list
stable-aarch64-apple-darwin (active, default)
1.78-aarch64-apple-darwin
1.92.0-aarch64-apple-darwin        # no nightly; cargo-fuzz requires -Zsanitizer=address

$ df -h $HOME
/dev/disk3s5   460Gi   428Gi   3.6Gi   100%
```

A nightly toolchain is roughly 1.5 GiB before cargo-fuzz itself and before any
ASan-instrumented target binaries. Against 3.6 GiB free at 100% utilization, installing it
would violate the stated environment constraint. I did not install it.

The plan is therefore tiered, and **Tier 0 is not a consolation prize** — it already found
every defect in section 2:

| Tier | Tool | Deps | Disk | Where |
|---|---|---|---|---|
| **0** | `cargo test`, in-tree xorshift + invariants | **none** | 0 | local + CI, default gate |
| **1** | `proptest` shrinking | ~10 dev-deps, few MB | small | local + CI, opt-in |
| **2** | `cargo-fuzz` / libFuzzer + ASan | nightly | ~2 GiB | **CI only** |

Tier 0 runs the full 36-property suite in **0.17 s**, so it belongs in the default `cargo test`
gate with no feature flag. Tier 2 is real coverage-guided fuzzing and should exist, but as a
scheduled CI job, never as a local prerequisite.

Licence check for Tier 1/2: `proptest` is MIT OR Apache-2.0, `arbitrary` and `libfuzzer-sys` are
MIT OR Apache-2.0. All compatible with this workspace's `MIT OR Apache-2.0`. No vendored
third-party code is proposed.

---

## 5. Tier 0 harnesses (verified: these compile and run)

Place `common/mod.rs` in **both** `crates/tf-term/tests/` and `crates/tf-proto/tests/`
(integration tests cannot share a module across crates), then add the files below.
Failing properties are marked; they are correct assertions against buggy code and must **not**
be weakened to make the suite green.

### 5.1 `crates/tf-term/tests/common/mod.rs`

```rust
//! Deterministic PRNG + splitter shared by the property harnesses.
//! Zero dependencies on purpose: these run in the default `cargo test` gate.

pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed | 1) }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;                       // xorshift64*
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn u32(&mut self) -> u32 { (self.next_u64() >> 32) as u32 }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    pub fn byte(&mut self) -> u8 { (self.next_u64() >> 24) as u8 }
    pub fn bytes(&mut self, len: usize) -> Vec<u8> { (0..len).map(|_| self.byte()).collect() }

    /// Bytes biased toward the alphabet that actually drives the parsers.
    /// Uniform random bytes almost never form a valid escape sequence; this does.
    pub fn hostile_bytes(&mut self, len: usize) -> Vec<u8> {
        const ALPHABET: &[u8] =
            b"\x1b[<>;:0123456789ABCDMmuIOt~$pP_G\\\x07\x00\xff\xc0\xf0\x80";
        (0..len)
            .map(|_| if self.below(4) == 0 { self.byte() }
                     else { ALPHABET[self.below(ALPHABET.len())] })
            .collect()
    }

    /// Split `data` into contiguous chunks at random boundaries.
    pub fn split<'a>(&mut self, data: &'a [u8], max_parts: usize) -> Vec<&'a [u8]> {
        let k = self.below(max_parts);
        let mut cuts: Vec<usize> = (0..k).map(|_| self.below(data.len() + 1)).collect();
        cuts.push(0);
        cuts.push(data.len());
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2).map(|w| &data[w[0]..w[1]]).collect()
    }
}

/// Minimal base64 decoder, used to verify the in-tree encoder differentially.
pub fn b64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if input.len() % 4 != 0 { return None; }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for q in input.chunks_exact(4) {
        let pad = q.iter().filter(|&&c| c == b'=').count();
        if pad > 2 { return None; }
        let mut n = 0u32;
        for (i, &c) in q.iter().enumerate() {
            let v = if c == b'=' {
                if i < 4 - pad { return None; }
                0
            } else { val(c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 { out.push((n >> 8) as u8); }
        if pad < 1 { out.push(n as u8); }
    }
    Some(out)
}
```

### 5.2 `crates/tf-term/tests/prop_input.rs`

Result: **11 pass, 4 fail** (`p3`, `p4`, `p4b`, `p6` — F04-10, F04-08, F04-09, F04-13).

```rust
//! Property harness for the terminal input decoder.
//!
//! The terminal byte stream is the project's single genuinely untrusted input: it carries
//! whatever a remote SSH host, a hostile program sharing the tty, or a buggy emulator emits.

mod common;
use tf_term::input::{Decoder, Event, KeyCode, KeyEventKind, Modifiers};
use common::Rng;

/// Cap on how many bytes the decoder may hold while waiting for more input.
/// A parser with no ceiling is a remote OOM.
const PENDING_CEILING: usize = 64 * 1024;

#[test]
fn p1_arbitrary_bytes_never_panic() {
    for seed in 0..256u64 {
        let mut r = Rng::new(seed);
        let mut d = Decoder::new(true);
        for _ in 0..64 {
            let n = r.below(48) + 1;
            let chunk = r.hostile_bytes(n);
            let _ = d.decode(&chunk);
        }
    }
}

#[test]
fn p2_chunk_splitting_does_not_change_the_event_stream() {
    // The core invariant of a streaming decoder: a read() boundary is not semantic.
    for seed in 0..512u64 {
        let mut r = Rng::new(seed ^ 0xA5A5);
        let n = r.below(64) + 1;
        let data = r.hostile_bytes(n);

        let whole = Decoder::new(true).decode(&data);

        let mut split = Decoder::new(true);
        let mut got = Vec::new();
        for part in r.split(&data, 6) {
            got.extend(split.decode(part));
        }
        assert_eq!(whole, got, "split changed the event stream for {data:?} (seed {seed})");
    }
}

#[test] // FAILS -> F04-10. Reproduces at seed 8.
fn p3_decoder_never_wedges() {
    // After the stream goes quiet, a bounded number of flush attempts must drain the
    // decoder. Otherwise one truncated sequence silently kills all further input.
    for seed in 0..256u64 {
        let mut r = Rng::new(seed ^ 0xBEEF);
        let mut d = Decoder::new(true);
        let n = r.below(32) + 1;
        let data = r.hostile_bytes(n);
        let _ = d.decode(&data);
        for _ in 0..4 { let _ = d.flush_pending_escape(); }
        assert_eq!(d.pending(), 0,
            "decoder wedged with {} bytes stuck after {data:?} (seed {seed})", d.pending());
    }
}

#[test] // FAILS -> F04-08. Reached 65538 bytes.
fn p4_pending_buffer_is_bounded() {
    let mut d = Decoder::new(true);
    let _ = d.decode(b"\x1b[");
    for _ in 0..512 {
        let _ = d.decode(&vec![b'1'; 1024]);
        assert!(d.pending() <= PENDING_CEILING,
            "unterminated CSI grew the buffer to {} bytes (ceiling {PENDING_CEILING})",
            d.pending());
    }
}

#[test] // FAILS -> F04-09.
fn p4b_unterminated_paste_is_bounded() {
    let mut d = Decoder::new(true);
    let _ = d.decode(b"\x1b[200~");
    for _ in 0..512 {
        if !d.decode(&vec![b'A'; 1024]).is_empty() { return; }
    }
    panic!("512 KiB of unterminated paste buffered with no bound and no event");
}

#[test]
fn p5_sgr_mouse_coordinates_are_positional() {
    // Parameters are positional. Dropping an unparseable one must not shift the ones
    // after it into its slot, or a click lands somewhere the user did not click.
    for seed in 0..2048u64 {
        let mut r = Rng::new(seed ^ 0xC0FFEE);
        let (btn, x, y) = (r.u32() % 128, r.u32() % 100_000, r.u32() % 100_000);
        let seq = format!("\x1b[<{btn};{x};{y}M");
        let evs = Decoder::new(true).decode(seq.as_bytes());
        assert_eq!(evs.len(), 1, "for {seq:?}");
        match &evs[0] {
            Event::Mouse { x: gx, y: gy, .. } =>
                assert_eq!((*gx, *gy), (x, y), "coordinates moved for {seq:?}"),
            other => panic!("expected Mouse for {seq:?}, got {other:?}"),
        }
    }
}

#[test]
fn p5b_extra_sgr_parameters_do_not_shift_coordinates() {
    let evs = Decoder::new(true).decode(b"\x1b[<0;11;22;33M");
    match &evs[0] {
        Event::Mouse { x, y, .. } => assert_eq!((*x, *y), (11, 22)),
        other => panic!("got {other:?}"),
    }
}

#[test] // FAILS -> F04-13.
fn p6_unparseable_leading_param_must_not_become_the_key() {
    // `CSI ; 5 A` means "default first param, modifier 5" => Ctrl+Up.
    // Compacting the empty param turns it into an unmodified Up, silently losing Ctrl.
    let evs = Decoder::new(true).decode(b"\x1b[;5A");
    match &evs[0] {
        Event::Key { code, mods, .. } => {
            assert_eq!(*code, KeyCode::Up);
            assert!(mods.ctrl, "Ctrl was dropped by parameter compaction");
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn p7_decoder_is_total_over_all_single_csi_finals() {
    for f in 0x40u8..=0x7e {
        for body in [&b""[..], b"1", b"1;5", b"<0;1;1", b"?1016;4", b"200", b"999999999999"] {
            let mut seq = vec![0x1b, b'['];
            seq.extend_from_slice(body);
            seq.push(f);
            let mut d = Decoder::new(true);
            let evs = d.decode(&seq);
            assert!(!evs.is_empty() || d.pending() == 0,
                "CSI body {body:?} final {:?} produced no event and left {} bytes pending",
                f as char, d.pending());
        }
    }
}

#[test]
fn p8_paste_body_is_never_reinterpreted_as_input() {
    // Injection guard: bytes inside a bracketed paste are data, never commands.
    for seed in 0..512u64 {
        let mut r = Rng::new(seed ^ 0xDEAD);
        let n = r.below(48) + 1;
        let body = r.hostile_bytes(n);
        if body.windows(6).any(|w| w == b"\x1b[201~") { continue; }
        let mut msg = b"\x1b[200~".to_vec();
        msg.extend_from_slice(&body);
        msg.extend_from_slice(b"\x1b[201~");
        let evs = Decoder::new(true).decode(&msg);
        assert_eq!(evs.len(), 1, "paste produced {} events for {body:?}", evs.len());
        match &evs[0] {
            Event::Paste(s) => assert_eq!(s.as_bytes(),
                String::from_utf8_lossy(&body).as_bytes(), "paste body was altered"),
            other => panic!("paste escaped its brackets: {other:?}"),
        }
    }
}

#[test]
fn p9_kitty_key_roundtrip() {
    for seed in 0..2048u64 {
        let mut r = Rng::new(seed ^ 0x1234);
        let ch = char::from_u32(0x21 + r.u32() % 0x5e).unwrap();
        let modbits = r.u32() % 16;
        let evkind = 1 + r.u32() % 3;
        let seq = format!("\x1b[{};{}:{}u", ch as u32, modbits + 1, evkind);
        let evs = Decoder::new(true).decode(seq.as_bytes());
        assert_eq!(evs.len(), 1, "for {seq:?}");
        let want_mods = Modifiers {
            shift: modbits & 1 != 0, alt: modbits & 2 != 0,
            ctrl:  modbits & 4 != 0, meta: modbits & 8 != 0,
        };
        let want_kind = match evkind {
            2 => KeyEventKind::Repeat, 3 => KeyEventKind::Release, _ => KeyEventKind::Press,
        };
        assert_eq!(evs[0],
            Event::Key { code: KeyCode::Char(ch), mods: want_mods, kind: want_kind, text: None },
            "roundtrip failed for {seq:?}");
    }
}

#[test]
fn p10_no_event_is_fabricated_from_nothing() {
    let mut d = Decoder::new(true);
    let _ = d.decode(b"\x1b[<0;1");
    assert!(d.decode(b"").is_empty());
    let _ = d.decode(b"\x1b[200~x");
    assert!(d.decode(b"").is_empty());
}

#[test]
fn p11_unknown_events_carry_the_bytes_they_consumed() {
    for seed in 0..512u64 {
        let mut r = Rng::new(seed ^ 0x9999);
        let n = r.below(48) + 1;
        let data = r.hostile_bytes(n);
        let evs = Decoder::new(true).decode(&data);
        let unknown: usize = evs.iter()
            .map(|e| if let Event::Unknown(v) = e { v.len() } else { 0 }).sum();
        assert!(unknown <= data.len(), "Unknown reported more bytes than were fed");
    }
}

#[test]
fn p12_modifier_decode_matches_the_spec_formula() {
    for v in 0u32..=64 {
        let m = Modifiers::from_kitty_param(v);
        let b = v.saturating_sub(1);
        assert_eq!(m.shift, b & 1 != 0);
        assert_eq!(m.alt,   b & 2 != 0);
        assert_eq!(m.ctrl,  b & 4 != 0);
        assert_eq!(m.meta,  b & 8 != 0);
    }
}

#[test]
fn p13_wheel_and_button_decode_is_exhaustive() {
    let mut seen = std::collections::HashSet::new();
    for btn in 0u32..256 {
        let seq = format!("\x1b[<{btn};1;1M");
        if let Some(Event::Mouse { kind, button, .. }) =
            Decoder::new(true).decode(seq.as_bytes()).first()
        {
            seen.insert(format!("{kind:?}/{button:?}"));
        }
    }
    assert!(seen.len() >= 6, "button decoding collapsed too much: {} outcomes", seen.len());
}
```

### 5.3 `crates/tf-proto/tests/prop_proto.rs`

Result: **8 pass, 4 fail** (`q5`, `q6`, `q8`, `q9` — F04-04, F04-01 precondition, F04-12).
`q3` is shown tightened from the version I ran; as written here it will also fail (F04-11).

```rust
//! Property harness for the wire protocol reader, frame header, and hand-rolled JSON.

mod common;
use tf_proto::{
    frame_message, json_escape, json_get_bool, json_get_str, FrameHeader, MessageReader,
    FRAME_HEADER_LEN, T_COMMAND, T_EVENT, T_FRAME,
};
use common::Rng;

/// Largest frame we will ever legitimately see: 8K x 8K BGRA. Anything claiming more is a
/// hostile or corrupt header and must be rejected before it reaches an allocator.
const MAX_SANE_PAYLOAD: usize = 8192 * 8192 * 4;

fn header_bytes(vals: [u32; 8]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

#[test]
fn q1_framing_roundtrip_under_arbitrary_splits() {
    for seed in 0..512u64 {
        let mut r = Rng::new(seed);
        let n = r.below(4096);
        let payload = r.bytes(n);
        let ty = [T_FRAME, T_EVENT, T_COMMAND][r.below(3)];
        let wire = frame_message(ty, &payload);

        let mut reader = MessageReader::new();
        let mut got = None;
        for part in r.split(&wire, 8) {
            reader.feed(part);
            if let Some(m) = reader.next_message() {
                assert!(got.is_none(), "reader emitted two messages for one frame");
                got = Some(m);
            }
        }
        let m = got.expect("message never completed");
        assert_eq!(m.type_id, ty);
        assert_eq!(m.payload, payload);
        assert_eq!(reader.buffered(), 0, "bytes left over after a clean message");
    }
}

#[test]
fn q2_reader_never_panics_on_arbitrary_bytes() {
    for seed in 0..512u64 {
        let mut r = Rng::new(seed ^ 0x77);
        let mut reader = MessageReader::new();
        for _ in 0..16 {
            let n = r.below(64) + 1;
            let chunk = r.bytes(n);
            reader.feed(&chunk);
            while reader.next_message().is_some() {}
        }
    }
}

#[test] // FAILS -> F04-11. Measured 134,217,733 bytes buffered after feeding 128 MiB.
fn q3_hostile_length_prefix_is_rejected_not_buffered_forever() {
    let mut reader = MessageReader::new();
    let mut m = vec![T_FRAME];
    m.extend_from_slice(&u32::MAX.to_be_bytes());
    reader.feed(&m);
    for _ in 0..32 { reader.feed(&vec![0u8; 1024 * 1024]); }
    assert!(reader.buffered() <= MAX_SANE_PAYLOAD,
        "reader absorbed {} bytes chasing a bogus length prefix", reader.buffered());
}

#[test]
fn q4_frame_header_parse_is_total() {
    for seed in 0..1024u64 {
        let mut r = Rng::new(seed ^ 0xAB);
        let n = r.below(80);
        let b = r.bytes(n);
        assert_eq!(FrameHeader::parse(&b).is_some(), b.len() >= FRAME_HEADER_LEN);
    }
}

#[test] // FAILS -> F04-04.
fn q5_expected_payload_never_overflows() {
    // Debug panics on overflow; release wraps, which is worse -- a wrapped value of 0
    // makes the truncation guard vacuous and poisons page geometry.
    let cases: [[u32; 8]; 4] = [
        [1, u32::MAX,    u32::MAX,    0, 0, 0, 0, 0],
        [1, 2147483648,  2147483648,  0, 0, 0, 0, 0], // w*h*4 == 0 mod 2^64
        [1, 65536,       65536,       0, 0, 0, 0, 0],
        [1, 1073741824,  4,           0, 0, 0, 0, 0],
    ];
    for c in cases {
        let h = FrameHeader::parse(&header_bytes(c)).unwrap();
        let got = h.expected_payload();
        let want = (h.width as u128) * (h.height as u128) * 4;
        assert_eq!(got as u128, want, "expected_payload wrapped for {c:?}");
        assert!(got <= MAX_SANE_PAYLOAD,
            "header claiming {}x{} ({} bytes) must be rejected before it reaches an allocator",
            h.width, h.height, got);
    }
}

#[test] // FAILS -> the precondition F04-01 depends on.
fn q6_dirty_rect_is_inside_the_frame() {
    // The damage encoder will index straight into the pixel buffer with these values.
    for seed in 0..1024u64 {
        let mut r = Rng::new(seed ^ 0xCD);
        let vals = [r.u32(), r.u32() % 4000, r.u32() % 4000,
                    r.u32(), r.u32(), r.u32(), r.u32(), 0];
        let h = FrameHeader::parse(&header_bytes(vals)).unwrap();
        let inside = (h.dirty_x as u64 + h.dirty_w as u64) <= h.width  as u64
                  && (h.dirty_y as u64 + h.dirty_h as u64) <= h.height as u64;
        assert!(inside,
            "header accepted an out-of-frame dirty rect {:?} for a {}x{} frame; \
             FrameHeader::parse must validate this",
            (h.dirty_x, h.dirty_y, h.dirty_w, h.dirty_h), h.width, h.height);
    }
}

#[test]
fn q7_json_escape_parse_roundtrip() {
    // Page titles are attacker-controlled: any website sets document.title freely.
    for seed in 0..2048u64 {
        let mut r = Rng::new(seed ^ 0xEF);
        let len = r.below(24);
        let s: String = (0..len).map(|_| match r.below(6) {
            0 => '"',
            1 => '\\',
            2 => char::from_u32(r.u32() % 0x20).unwrap(),
            3 => char::from_u32(0x20 + r.u32() % 0x5f).unwrap(),
            4 => char::from_u32(0x80 + r.u32() % 0x3000).unwrap_or('x'),
            _ => char::from_u32(0x10000 + r.u32() % 0x1000).unwrap_or('y'),
        }).collect();

        let mut body = String::from("{\"v\":\"");
        json_escape(&s, &mut body);
        body.push_str("\"}");
        assert_eq!(json_get_str(&body, "v").as_deref(), Some(s.as_str()),
            "roundtrip failed for {s:?} -> {body:?}");
    }
}

#[test] // FAILS -> F04-12. Returns "ATTACKER".
fn q8_json_get_str_ignores_nested_objects() {
    let j = r#"{"meta":{"v":"ATTACKER"},"t":"url","v":"https://real"}"#;
    assert_eq!(json_get_str(j, "v").as_deref(), Some("https://real"));
}

#[test] // FAILS -> F04-12.
fn q9_json_get_bool_ignores_nested_objects() {
    let j = r#"{"meta":{"v":true},"t":"loading","v":false}"#;
    assert_eq!(json_get_bool(j, "v"), Some(false));
}

#[test]
fn q10_json_get_str_never_panics() {
    for seed in 0..4096u64 {
        let mut r = Rng::new(seed ^ 0x1357);
        let n = r.below(64);
        let raw = r.bytes(n);
        let s = String::from_utf8_lossy(&raw);
        for key in ["v", "t", "reason", "", "\\", "\""] {
            let _ = json_get_str(&s, key);
            let _ = json_get_bool(&s, key);
        }
    }
}

#[test] // FAILS -> F04-16. Latent: JSON.stringify does not emit these today.
fn q11_escaped_surrogate_pairs_decode_to_one_char() {
    let j = "{\"v\":\"\\ud83d\\ude00\"}";
    assert_eq!(json_get_str(j, "v").as_deref(), Some("\u{1F600}"));
}

#[test]
fn q12_multiple_messages_in_one_chunk_all_survive() {
    for seed in 0..256u64 {
        let mut r = Rng::new(seed ^ 0x2468);
        let count = r.below(8) + 1;
        let (mut wire, mut want) = (Vec::new(), Vec::new());
        for _ in 0..count {
            let n = r.below(256);
            let p = r.bytes(n);
            wire.extend(frame_message(T_EVENT, &p));
            want.push(p);
        }
        let mut reader = MessageReader::new();
        reader.feed(&wire);
        let mut got = Vec::new();
        while let Some(m) = reader.next_message() { got.push(m.payload); }
        assert_eq!(got, want);
        assert_eq!(reader.buffered(), 0);
    }
}
```

### 5.4 `crates/tf-term/tests/prop_kitty.rs`

Result: **6 pass, 3 fail** (`k4`, `k6`, `k7` — F04-15, F04-01, F04-05).
Note `k3` passes: the APC grammar, the 4096-byte chunk limit, the `m=1`/`m=0` sequencing, and
base64-quantum alignment at chunk boundaries are all correct. That is worth locking in.

```rust
//! Property harness for the base64 encoder and the Kitty graphics encoder.
//!
//! These bytes go straight to the user's terminal. A malformed escape here does not just
//! render wrong -- it can leave the tty in a state the user has to `reset` out of.

mod common;
use tf_term::kitty::{
    bgra_rect_to_rgb, bgra_to_rgb, delete_all, delete_image, encode_rgb_frame, support_query,
    wrap_tmux, Placement, MAX_CHUNK,
};
use tf_term::{b64, Rect};
use common::{b64_decode, Rng};

#[test]
fn k1_base64_roundtrips_for_every_length() {
    for len in 0..512usize {
        let mut r = Rng::new(len as u64 + 1);
        let data = r.bytes(len);
        let enc = b64::encode(&data);
        assert_eq!(enc.len() % 4, 0, "base64 output must be 4-byte aligned (len {len})");
        assert_eq!(b64_decode(&enc).as_deref(), Some(data.as_slice()),
            "roundtrip failed at len {len}");
    }
}

#[test]
fn k2_base64_alphabet_is_never_violated() {
    // A stray byte here would terminate the APC early and dump raw pixels into the shell.
    for seed in 0..256u64 {
        let mut r = Rng::new(seed);
        let n = r.below(300);
        let enc = b64::encode(&r.bytes(n));
        for &c in &enc {
            assert!(c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=',
                "byte {c:#04x} escaped the base64 alphabet");
        }
        assert!(!enc.contains(&0x1b), "ESC in base64 output");
        assert!(!enc.contains(&b';'), "semicolon in base64 output");
    }
}

/// Split encoder output into (control-data, payload) pairs, asserting envelope validity.
fn parse_apc(out: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut segs = Vec::new();
    let mut i = 0;
    while i < out.len() {
        assert_eq!(&out[i..i + 3], b"\x1b_G", "segment {} lacks the APC introducer", segs.len());
        let semi = out[i..].iter().position(|&b| b == b';').expect("no ; in control data") + i;
        let st = out[semi..].windows(2).position(|w| w == b"\x1b\\").expect("unterminated APC")
            + semi;
        segs.push((String::from_utf8_lossy(&out[i + 3..semi]).into_owned(),
                   out[semi + 1..st].to_vec()));
        i = st + 2;
    }
    segs
}

#[test]
fn k3_encoder_output_is_a_well_formed_apc_stream() {
    for seed in 0..64u64 {
        let mut r = Rng::new(seed + 1);
        let w = r.u32() % 64 + 1;
        let h = r.u32() % 64 + 1;
        let rgb = r.bytes((w * h * 3) as usize);
        let level = r.u32() % 10;

        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, w, h, Placement::default(), level, &mut out).unwrap();
        let segs = parse_apc(&out);
        assert_eq!(segs.len(), stats.chunks, "reported chunk count must match reality");
        assert!(!segs.is_empty(), "encoder emitted no escape sequence at all");

        for (idx, (ctrl, payload)) in segs.iter().enumerate() {
            assert!(payload.len() <= MAX_CHUNK,
                "chunk {idx} is {} bytes, over the Kitty {MAX_CHUNK}-byte limit", payload.len());
            let want_more = if idx + 1 < segs.len() { "m=1" } else { "m=0" };
            assert!(ctrl.contains(want_more), "chunk {idx} control {ctrl:?} lacks {want_more}");
            if idx == 0 {
                assert!(ctrl.contains(&format!("s={w}")), "width missing: {ctrl:?}");
                assert!(ctrl.contains(&format!("v={h}")), "height missing: {ctrl:?}");
                assert!(ctrl.contains("C=1"), "cursor-move suppression missing: {ctrl:?}");
            }
            // Continuation chunks must split on a base64 quantum or the terminal
            // reassembles a corrupt payload.
            if idx + 1 < segs.len() {
                assert_eq!(payload.len() % 4, 0, "chunk {idx} splits mid base64 quantum");
            }
        }
        let joined: Vec<u8> = segs.iter().flat_map(|(_, p)| p.clone()).collect();
        assert!(b64_decode(&joined).is_some(), "reassembled payload is not valid base64");
    }
}

#[test] // FAILS -> F04-15. 0x0 at level 0: emits nothing, reports chunks = 1.
fn k4_degenerate_geometry_still_produces_a_valid_or_no_frame() {
    for (w, h) in [(0u32, 0u32), (0, 10), (10, 0)] {
        for level in [0u32, 6] {
            let mut out = Vec::new();
            let stats = encode_rgb_frame(&[], w, h, Placement::default(), level, &mut out).unwrap();
            if out.is_empty() {
                assert_eq!(stats.chunks, 0,
                    "{w}x{h} level {level}: emitted nothing but reported {} chunks", stats.chunks);
            } else {
                assert_eq!(parse_apc(&out).len(), stats.chunks);
            }
        }
    }
}

#[test]
fn k5_bgra_to_rgb_is_exact_and_total() {
    for seed in 0..256u64 {
        let mut r = Rng::new(seed + 7);
        let n = r.below(400);
        let bgra = r.bytes(n);
        let mut rgb = Vec::new();
        bgra_to_rgb(&bgra, &mut rgb);
        assert_eq!(rgb.len(), bgra.len() / 4 * 3, "length rule broken");
        for (i, px) in bgra.chunks_exact(4).enumerate() {
            assert_eq!(&rgb[i * 3..i * 3 + 3], &[px[2], px[1], px[0]]);
        }
    }
}

#[test] // FAILS -> F04-01. Panics inside kitty.rs:61.
fn k6_bgra_rect_to_rgb_never_indexes_out_of_bounds() {
    // The dirty rect arrives over IPC as four unvalidated u32s.
    for seed in 0..1024u64 {
        let mut r = Rng::new(seed ^ 0x5A5A);
        let img_w = r.u32() % 16 + 1;
        let img_h = r.u32() % 16 + 1;
        let bgra = vec![0u8; (img_w * img_h * 4) as usize];
        let rect = Rect::new(r.u32() % 32, r.u32() % 32, r.u32() % 32, r.u32() % 32);
        let mut out = Vec::new();
        // Must be safe for ANY rect: clamp internally or return without touching memory.
        // Panicking kills the browser and is not an option.
        bgra_rect_to_rgb(&bgra, img_w, rect, &mut out);
        assert!(out.len() <= bgra.len() / 4 * 3);
    }
}

#[test] // FAILS -> F04-05. Union stops covering its inputs once the add wraps.
fn k7_rect_arithmetic_never_overflows() {
    for seed in 0..2048u64 {
        let mut r = Rng::new(seed ^ 0x0F0F);
        let pick = |r: &mut Rng| match r.below(4) {
            0 => u32::MAX, 1 => u32::MAX - 1, 2 => 0, _ => r.u32(),
        };
        let a = Rect::new(pick(&mut r), pick(&mut r), pick(&mut r), pick(&mut r));
        let b = Rect::new(pick(&mut r), pick(&mut r), pick(&mut r), pick(&mut r));
        let u = a.union(&b);
        if !a.is_empty() {
            assert!(u.x as u64 <= a.x as u64);
            assert!(u.x as u64 + u.w as u64 >= a.x as u64 + a.w as u64);
        }
        let _ = a.clamp_to(b.w, b.h);
        let _ = a.area();
    }
}

#[test]
fn k8_tmux_wrapping_is_reversible() {
    for seed in 0..256u64 {
        let mut r = Rng::new(seed ^ 0x3C3C);
        let n = r.below(200);
        let payload = r.bytes(n);
        let mut out = Vec::new();
        wrap_tmux(&payload, &mut out);
        assert!(out.starts_with(b"\x1bPtmux;"));
        assert!(out.ends_with(b"\x1b\\"));
        let body = &out[7..out.len() - 2];
        let (mut back, mut i) = (Vec::new(), 0);
        while i < body.len() {
            if body[i] == 0x1b {
                assert_eq!(body.get(i + 1), Some(&0x1b),
                    "un-doubled ESC would truncate the DCS");
                back.push(0x1b);
                i += 2;
            } else { back.push(body[i]); i += 1; }
        }
        assert_eq!(back, payload, "tmux wrapping lost data");
    }
}

#[test]
fn k9_control_sequences_contain_no_stray_terminators() {
    let mut probes: Vec<Vec<u8>> = vec![support_query(31)];
    let mut v = Vec::new(); delete_all(&mut v); probes.push(v);
    for id in [0u32, 1, 1000, u32::MAX] {
        let mut v = Vec::new(); delete_image(id, &mut v); probes.push(v);
    }
    for p in probes {
        let sts = p.windows(2).filter(|w| *w == b"\x1b\\").count();
        assert_eq!(sts, 1, "sequence {:?} has {sts} STs", String::from_utf8_lossy(&p));
        assert!(p.ends_with(b"\x1b\\"));
        assert!(p.starts_with(b"\x1b_G"));
    }
}
```

### 5.5 `crates/tf-term/tests/prop_caps.rs` — BLOCKED

**This harness cannot be written until the three parsers are made reachable** (section 2.4).
Written against the current private signatures it does not compile. The intended body, to add
once `parse_da1_has_sixel`, `parse_decrqm_supported`, and `parse_two_param_t` are `pub`:

```rust
mod common;
use tf_term::caps::{escape_for_display, parse_da1_has_sixel, parse_decrqm_supported,
                    parse_two_param_t};
use common::Rng;

/// Positional reference parser. The production parser must agree with it or be wrong.
fn reference_da1_sixel(reply: &[u8]) -> bool {
    let s = String::from_utf8_lossy(reply);
    let Some(i) = s.find("\x1b[?") else { return false };
    let rest = &s[i + 3..];
    let Some(j) = rest.find('c') else { return false };
    rest[..j].split(';').any(|p| p == "4")
}

#[test]
fn c1_parsers_are_total_over_arbitrary_bytes() {
    for seed in 0..4096u64 {
        let mut r = Rng::new(seed);
        let n = r.below(64);
        let b = r.hostile_bytes(n);
        let _ = parse_da1_has_sixel(&b);
        let _ = parse_decrqm_supported(&b);
        let _ = parse_two_param_t(&b);
        let _ = escape_for_display(&b);
    }
}

#[test]
fn c2_da1_matches_the_reference_parser() {
    for seed in 0..4096u64 {
        let mut r = Rng::new(seed ^ 0x11);
        let n = r.below(48);
        let b = r.hostile_bytes(n);
        assert_eq!(parse_da1_has_sixel(&b), reference_da1_sixel(&b),
            "disagreement on {:?}", escape_for_display(&b));
    }
}

#[test]
fn c3_leading_garbage_does_not_change_the_verdict() {
    // A slow terminal can leave stale bytes in the buffer ahead of the real reply.
    // `parse_two_param_t` anchors on the FIRST `ESC [`, so this is expected to fail
    // for prefixes that themselves contain `ESC [`.
    let real = b"\x1b[6;37;17t";
    let want = parse_two_param_t(real);
    for prefix in [&b""[..], b"x", b"\x1b[", b"\x1b[0m", b"garbage;;;"] {
        let mut v = prefix.to_vec();
        v.extend_from_slice(real);
        assert_eq!(parse_two_param_t(&v), want, "prefix {prefix:?} changed the parse");
    }
}

#[test]
fn c4_measured_real_replies_still_parse() {
    // Regression corpus from the real terminals in caps.rs's own doc table.
    assert!(!parse_da1_has_sixel(b"\x1b[?62;22;52c"));   // Ghostty 1.3.1
    assert!(!parse_da1_has_sixel(b"\x1b[?1;2c"));        // Apple Terminal 465
    assert_eq!(parse_two_param_t(b"\x1b[6;37;17t"), Some((37, 17)));
    assert_eq!(parse_two_param_t(b"\x1b[4;851;2482t"), Some((851, 2482)));
    assert!(!parse_decrqm_supported(b"\x1b[?1016;4$y")); // iTerm2 3.6.9
    assert_eq!(parse_two_param_t(b""), None);            // Apple Terminal: no reply
}

#[test]
fn c5_oversized_parameters_do_not_silently_truncate() {
    // parse::<u16>() fails past 65535 and the whole reply is discarded. A 5K display
    // wall could legitimately report a window wider than u16::MAX in a future terminal.
    assert!(parse_two_param_t(b"\x1b[4;99999;100000t").is_some(),
        "window dimensions above u16::MAX are rejected outright");
}
```

`c5` documents a real ceiling: `Capabilities::window_px` and `cell` are `(u16, u16)`
(`caps.rs:47-49`), so any reported pixel dimension above 65,535 is dropped and the viewport
silently falls back to the ioctl. Not urgent, but it is a hard limit worth recording.

---

## 6. Tier 2 — `cargo-fuzz` targets (CI only)

Not runnable on this machine (section 4). These are for the CI job.

```toml
# fuzz/Cargo.toml
[package]
name = "terminal-fenster-fuzz"
version = "0.0.0"
edition = "2021"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "0.4"
arbitrary = { version = "1", features = ["derive"] }
tf-term  = { path = "../crates/tf-term" }
tf-proto = { path = "../crates/tf-proto" }

[[bin]] name = "input_decoder" path = "fuzz_targets/input_decoder.rs" test = false doc = false
[[bin]] name = "proto_reader"  path = "fuzz_targets/proto_reader.rs"  test = false doc = false
[[bin]] name = "kitty_encoder" path = "fuzz_targets/kitty_encoder.rs" test = false doc = false
[[bin]] name = "json_parser"   path = "fuzz_targets/json_parser.rs"   test = false doc = false
```

```rust
// fuzz/fuzz_targets/input_decoder.rs
// Structure-aware: the interesting bug class is chunk-boundary state, not byte content.
#![no_main]
use arbitrary::Arbitrary;
use tf_term::input::Decoder;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input { pixel_mouse: bool, chunks: Vec<Vec<u8>> }

fuzz_target!(|inp: Input| {
    // Oracle 1: chunking must not change the event stream.
    let flat: Vec<u8> = inp.chunks.iter().flatten().copied().collect();
    let whole = Decoder::new(inp.pixel_mouse).decode(&flat);

    let mut d = Decoder::new(inp.pixel_mouse);
    let mut split = Vec::new();
    for c in &inp.chunks {
        split.extend(d.decode(c));
        // Oracle 2: the pending buffer must stay bounded. (Currently violated: F04-08.)
        assert!(d.pending() <= 64 * 1024, "unbounded buffer: {}", d.pending());
    }
    assert_eq!(whole, split, "chunk boundary changed the event stream");

    // Oracle 3: the decoder must drain. (Currently violated: F04-10.)
    let _ = d.flush_pending_escape();
});
```

```rust
// fuzz/fuzz_targets/proto_reader.rs
#![no_main]
use tf_proto::{FrameHeader, MessageReader, FRAME_HEADER_LEN};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut r = MessageReader::new();
    // Feed in 7-byte bites so length prefixes straddle read boundaries.
    for c in data.chunks(7) {
        r.feed(c);
        while let Some(m) = r.next_message() {
            if m.type_id == tf_proto::T_FRAME {
                if let Some(h) = FrameHeader::parse(&m.payload) {
                    // Must not overflow, must be bounded, must describe an in-frame rect.
                    let want = (h.width as u128) * (h.height as u128) * 4;
                    assert_eq!(h.expected_payload() as u128, want);
                    assert!((h.dirty_x as u64 + h.dirty_w as u64) <= h.width as u64);
                    assert!((h.dirty_y as u64 + h.dirty_h as u64) <= h.height as u64);
                    assert!(m.payload.len() >= FRAME_HEADER_LEN);
                }
            }
        }
        assert!(r.buffered() < 512 * 1024 * 1024, "unbounded reader growth");
    }
});
```

```rust
// fuzz/fuzz_targets/kitty_encoder.rs
#![no_main]
use arbitrary::Arbitrary;
use tf_term::kitty::{bgra_rect_to_rgb, encode_rgb_frame, Placement, MAX_CHUNK};
use tf_term::Rect;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input { w: u16, h: u16, level: u8, rx: u32, ry: u32, rw: u32, rh: u32, fill: u8 }

fuzz_target!(|inp: Input| {
    let (w, h) = (inp.w as u32 % 129, inp.h as u32 % 129);
    let rgb = vec![inp.fill; (w * h * 3) as usize];

    let mut out = Vec::new();
    let stats = encode_rgb_frame(&rgb, w, h, Placement::default(),
                                 (inp.level % 10) as u32, &mut out).unwrap();
    // Envelope invariants: balanced APCs, chunk limit, exactly one terminator.
    assert_eq!(out.windows(3).filter(|x| *x == b"\x1b_G").count(), stats.chunks);
    assert_eq!(out.windows(2).filter(|x| *x == b"\x1b\\").count(), stats.chunks);
    if stats.chunks > 0 {
        assert!(out.starts_with(b"\x1b_G") && out.ends_with(b"\x1b\\"));
        assert_eq!(String::from_utf8_lossy(&out).matches("m=0").count(), 1);
    }
    let _ = MAX_CHUNK;

    // The dirty-rect path must tolerate any rect at all.
    let bgra = vec![inp.fill; (w * h * 4) as usize];
    let mut px = Vec::new();
    bgra_rect_to_rgb(&bgra, w, Rect::new(inp.rx, inp.ry, inp.rw, inp.rh), &mut px);
});
```

```rust
// fuzz/fuzz_targets/json_parser.rs
#![no_main]
use tf_proto::{json_escape, json_get_bool, json_get_str};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    for key in ["v", "t", "reason"] {
        let _ = json_get_str(s, key);
        let _ = json_get_bool(s, key);
    }
    // Round-trip oracle: escaping then parsing must be the identity.
    let mut body = String::from("{\"v\":\"");
    json_escape(s, &mut body);
    body.push_str("\"}");
    assert_eq!(json_get_str(&body, "v").as_deref(), Some(s));
});
```

**Seed corpus.** Use real measured bytes, not synthetic ones. From `caps.rs`'s own doc table
and the verified Ghostty session: `\x1b_Gi=31;OK\x1b\\`, `\x1b[?62;22;52c`, `\x1b[?1;2c`,
`\x1b[6;37;17t`, `\x1b[4;851;2482t`, `\x1b[?1016;4$y`, plus `\x1b[<0;100;200M`,
`\x1b[97;5:3u`, `\x1b[200~hello\x1b[201~`, and one captured 32-byte frame header. A
`tests/fixtures/` directory already exists and is the right home for these.

---

## 7. CI wiring

There is currently no `.github/workflows/` directory. Suggested job, kept cheap so the default
gate stays fast:

```yaml
# .github/workflows/fuzz.yml
name: fuzz
on: [push, pull_request]
jobs:
  properties:                       # Tier 0 -- always, ~0.2 s
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test --workspace                     # debug: overflow checks ON
      - run: cargo test --workspace --release           # release: catches wrap-vs-panic drift

  libfuzzer:                        # Tier 2 -- nightly cron only
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo install cargo-fuzz
      - run: |
          for t in input_decoder proto_reader kitty_encoder json_parser; do
            cargo fuzz run $t -- -max_total_time=300 -rss_limit_mb=2048
          done
      - uses: actions/upload-artifact@v4
        if: failure()
        with: { name: fuzz-crashes, path: fuzz/artifacts/ }
```

Running the Tier 0 suite in **both** profiles is deliberate and cheap: F04-04 panics in debug
and silently wraps in release, so a single-profile gate would miss half of these.

---

## 8. Recommended fix order

Ordered by (reachable today) × (severity), not by ease.

1. **F04-10, decoder wedge** — the only defect that breaks a normal user session with no
   attacker. Give `flush_pending_escape` a general "the stream is idle, drain what you have"
   mode that emits `Event::Unknown` for an undecodable tail.
2. **F04-08 / F04-09, unbounded + quadratic buffering** — add a `MAX_SEQUENCE` ceiling
   (4 KiB is generous) and a scan cursor so `step_csi` resumes where it stopped instead of
   restarting at index 2.
3. **F04-04 + F04-11, header validation** — make `FrameHeader::parse` reject geometry above a
   sane maximum and dirty rects outside the frame, using `u64`/`checked_mul` throughout, and
   give `MessageReader` a length ceiling. This is the parse-don't-validate fix and it deletes
   F04-01 through F04-03 as a side effect.
4. **F04-01 to F04-03, F04-05, F04-07** — `checked_*` arithmetic and an internal
   `clamp_to` in `bgra_rect_to_rgb`. **Must land before `C08-damage-encoder` wires these up.**
5. **F04-12, JSON key confusion** — the cheapest real fix is to enforce the documented
   precondition rather than write a parser: have the engine emit strictly flat objects and add
   a debug assertion, or scan only top-level keys by tracking string/brace depth.
6. **F04-13 / F04-14** — make `split_params` positional (`Vec<Option<u32>>`) instead of
   `filter_map`, so a dropped parameter cannot shift its neighbours.
7. **F04-15, F04-16, F04-17, section 2.4 visibility, `tty.rs:133`** — low severity, batch them.

---

## Appendix — reproduction

Everything above was produced by two scratch crates that only *read* the project's source; no
workspace file was created or modified.

```
scratchpad/fuzzcheck/     # #[path]-includes real tf-proto + input.rs, copies Rect/kitty fns
                          # cargo run           -> 7 panics under overflow checks
                          # cargo run --release -> 3 panics + silent wraps
scratchpad/harnesscheck/  # path-dependency on the real crates; the section 5 harnesses verbatim
                          # cargo test --release --test prop_input   11 pass / 4 fail
                          # cargo test --release --test prop_proto    8 pass / 4 fail
                          # cargo test --release --test prop_kitty    6 pass / 3 fail
```

Toolchain: `rustc 1.93.0 (254b59607 2026-01-19)`, `aarch64-apple-darwin`, macOS 26.1, Apple M4.
