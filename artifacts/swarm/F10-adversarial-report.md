# F10 — Adversarial Acceptance Report

**Role:** falsification, not confirmation. Every claim below was re-derived from the repo by
running commands and reading source. Where the commander's claim survives, I say so. Where it
does not, I say that too.

**Verdict: NO-SHIP** — but note carefully *why*. The engineering is largely sound; the
**evidence chain and the repository state are not**. Three P0s are process defects that can be
cleared in under an hour. Nothing here requires an architectural retreat.

---

## 1. Do the 87 tests actually pass?

**They pass. The number is wrong — it is 96, not 87.**

```
$ cd $REPO && cargo test
running 12 tests   test result: ok. 12 passed; 0 failed; 0 ignored   (tf-proto)
running 70 tests   test result: ok. 70 passed; 0 failed; 0 ignored   (tf-term)
running 14 tests   test result: ok. 14 passed; 0 failed; 0 ignored   (terminal-fenster)
   Doc-tests tf_proto   running 0 tests   ok. 0 passed
   Doc-tests tf_term    running 0 tests   ok. 0 passed
```

12 + 70 + 14 = **96 passing, 0 failing, 0 ignored**. No test is skipped, `#[ignore]`d, or
gated behind a feature. I checked for weakened assertions and found none — the suite is
honest, and several tests (`cell_coordinates_treated_as_pixels_would_collapse_the_page`,
`decrqm_permanently_reset_is_not_support`, `truncated_frame_is_dropped_not_rendered`) are real
regression guards against real bugs, not padding.

The "87" is stale. It is a *citation-hygiene* failure rather than a substantive one, but a
brief that misstates a number an auditor can check in nine seconds spends credibility it will
need for the numbers that are harder to check.

One compiler warning is outstanding, in the terminal-restore path:

```
warning: direct cast of function item into an integer
   --> crates/tf-term/src/tty.rs:133:50
133 |    libc::signal(sig, signal_handler as libc::sighandler_t);
```

---

## 2. Release binary and `doctor` non-tty behaviour

**Both claims hold. `doctor` is correct.**

```
$ ls -la target/release/terminal-fenster
-rwxr-xr-x@ 1 builder staff 619424 31 Jul 21:49 target/release/terminal-fenster
```

The binary is **not stale**: latest source mtime is `apps/cli/src/main.rs` at 21:49:04, binary
at 21:49:16.

```
$ ./target/release/terminal-fenster doctor < /dev/null ; echo "EXIT=$?"
terminal-fenster doctor 0.1.0
  status: NOT A TTY -- run this from an interactive terminal.
  ...
  engine: $REPO/apps/engine/node_modules/.bin/electron
EXIT=1
```

Correct on every axis: it detects the non-tty (`main.rs:99`), explains *why* capability
detection needs a terminal, still reports the one fact it can determine without one (engine
path), and **exits 1** so CI can distinguish it from a successful probe. Exit codes across the
surface are coherent — `0` help/version, `1` cannot-run, `2` usage error (`open` with no URL,
unknown command). I tried to break it and could not.

*Method note:* my first attempt piped `doctor` into `head` and read `$?` from `head`, which
reported `EXIT=0` and would have produced a false accusation. Re-run without the pipe.

---

## 3. Is the end-to-end claim consistent? — the arithmetic survives, the evidence does not

### 3a. The arithmetic is exactly right

```
2482 × 814          = 2,020,348 px
× 4 (BGRA)          = 8,081,392 B
+ 32 (frame header) = 8,081,424 B   ← matches the claim exactly
```

The 32 is real, not a fudge. `FRAME_HEADER_LEN = 32` (`crates/tf-proto/src/lib.rs:16`) and the
producer writes exactly eight `u32` BE fields into a 32-byte buffer
(`apps/engine/src/main.js:87-95`). The logged `payload_bytes` is `msg.payload.len()`
(`apps/cli/src/main.rs:543`), which is header + pixels. Consistent.

**This settles an open contradiction in the corpus.** `artifacts/swarm/C09-ssh-adaptive.md:235`
asserts: "2482 × 814 × 4 = 8,081,392 — 32 bytes (8 pixels) less. It cannot have been 8,081,424."
**C09 is wrong.** It compared against the pixel payload while the log records the framed
payload. C09 should be corrected before anyone builds a bandwidth model on it.

### 3b. The tuple is a composite of two different frames

The claim "first frame at 366 ms, 8,081,424-byte frame encoded to 53,999 wire bytes in 0.74 ms"
**cannot come from one measurement.** The code emits two records, and neither contains all
four numbers:

- `main.rs:539-544` — `first-frame after {}ms geometry={:?} payload_bytes={}`.
  Carries the 366 ms and the 8,081,424. **No `wire_bytes`, no `encode_ms`.**
- `main.rs:467-470` — `bounded-run complete frames={} fps={:.0} last_wire_bytes={} encode_ms={:.2}`.
  Carries the 53,999 and the 0.74 — and the field is named **`last_`**, i.e. the *final* frame
  of the run, emitted at exit.

So 53,999 B / 0.74 ms describe some frame at the end of a bounded run, not the first frame at
366 ms. Splicing them into one sentence produces a claim no instrument in this repo measured.
The individual numbers are plausible; the *conjunction* is an artifact of prose.

### 3c. There is no primary log — the citation is circular

There is **no `.log` or `.jsonl` capture anywhere in the repo.** The numbers appear in exactly
two places:

1. `artifacts/swarm/E07-cli-sdk-design.md:402-403`, as an illustrative JSONL block — and E07
   states its own provenance plainly: *"The numbers in that example are the ones already
   measured end-to-end in Ghostty 1.3.1 and **recorded in the mission brief**."* E07 copied the
   brief. It is not independent confirmation of the brief.
2. `benchmarks/bench.mjs:948`, a synthetic parser fixture. To its credit, `bench.mjs:943-945`
   labels itself honestly: *"This is NOT a benchmark result: no browser runs, no timing is
   measured."*

Every downstream artifact that cites these figures (C07, C09, C10, F07, D09, E02, E09) traces
back to the brief. That is one unreproduced observation wearing seven citations. The fix is
cheap and the harness already exists — see the recommendation.

### 3d. "Measured 60 fps" is an engine-side number presented as a pipeline number

`apps/engine/spike/fps-matrix.js` is a **standalone Electron process** that measures gaps
between `paint` callbacks on a canvas page at **1440×900** (`fps-matrix.js:29, 56-57`). It
never touches `MessageReader`, `bgra_to_rgb`, `deflate`, base64, or the tty write. It is a
good experiment for the question it asks — "does OSR paint continuously or once?" — and the
spike header says so.

But 60 fps at 1440×900 engine-side and an 8 MB frame at 2482×814 are **two different
experiments at two different resolutions**, and the brief presents them as one system
property. The repo's own profiler artifact contradicts the composite:
`artifacts/swarm/C10-rendering-profiler.md:33` records deflate alone at **16.067 ms** for
incompressible content at 2482×814 — one stage exceeding the entire 16.67 ms frame budget.

**No end-to-end frame-rate measurement exists.** "60 fps" should read "engine paints at 60 fps
at 1440×900; end-to-end throughput is unmeasured."

---

## 4. Defects

Everything below was **executed**, not inferred. Harness:
`/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/adv`
(a separate crate path-depending on the repo; **no repo file was modified**).

### P0-1 — The project has no version control history at all

```
$ git log --oneline
fatal: your current branch 'main' does not have any commits yet
$ git status --short
?? Cargo.lock  ?? Cargo.toml  ?? apps/  ?? artifacts/  ?? benchmarks/
?? crates/  ?? docs/  ?? package.json  ?? packages/  ?? target/  ?? tests/
```

**Zero commits. 100% of the work is untracked.** There is no `.gitignore`, so `target/`
(105 MB, measured) is staged for inclusion in the first commit. On a volume with **2.4 GiB
free** (`df -h /` — tighter than the 9 GiB in the brief), this is one `git add -A` from a
problem. Nothing is revertable, bisectable, or attributable. No amount of code quality
survives a `rm -rf`. This is the single highest-risk item in the repository.

### P0-2 — `license` is declared but no LICENSE file exists

`Cargo.toml:7` declares `license = "MIT OR Apache-2.0"` and `Cargo.toml:8` a public
`repository = "https://github.com/zent7x/terminal-fenster"`.

```
$ find . -iname "LICENSE*" -not -path "*/node_modules/*"    # → no results
```

Declaring a license in metadata without shipping its text grants nothing. Blocks publication
and blocks any third party from lawfully vendoring this. Both texts are required for the
`MIT OR Apache-2.0` dual grant.

### P0-3 — Root `package.json` is another project's file

```json
{"name":"qtest","version":"1.0.0","main":"safestore.js"}
```

`safestore.js` does not exist in this repo. This is foreign debris at the root of a project
about to be published. (`apps/engine/package.json` is correct and unaffected.)

### P1-4 — `MessageReader` trusts a 32-bit length; unbounded buffering — **proven**

`crates/tf-proto/src/lib.rs:86` reads a `u32` length and buffers until satisfied; `feed`
(`:72-74`) appends to an unbounded `Vec` with no ceiling.

```
$ adv unbounded
declared len = 4294967295
buffered after feeding 64 MiB = 67108869 bytes
next_message() = None (still waiting) -> reader will buffer to 4 GiB
RESULT: UNBOUNDED ALLOCATION CONFIRMED -- no MAX_MESSAGE_LEN check
```

A confused or compromised engine host declares 4 GiB and the terminal core grows without
bound until the OOM killer arrives. **This was already reported** at
`artifacts/swarm/B08-crash-recovery.md:336`, which even specifies the fix (a `MAX_MESSAGE_LEN`
of 32 MiB, "a five-line change"). It remains unfixed. A known, documented, trivially-fixable
defect that survived into the acceptance candidate is a process signal as much as a code one.

### P1-5 — Capability probe cannot tell a CSI 14t reply from a CSI 16t reply

`parse_two_param_t` (`crates/tf-term/src/caps.rs:221-230`) validates `parts.len() != 3` but
**never inspects `parts[0]`** — the report-type discriminant that distinguishes `CSI 4;h;w t`
(window pixels) from `CSI 6;h;w t` (cell pixels). Both call sites use it interchangeably:
window at `caps.rs:167-169`, cell at `caps.rs:173-175`. The crate's own tests bless this:
`caps.rs:285` and `caps.rs:290` feed the two different reply types to the same parser and
assert only on the trailing pair.

Consequence when a stale or slow `CSI 14t` reply lands in the `CSI 16t` read window:
`c.cell` becomes `(2482, 851)`, then `main.rs:252-254` computes

```
page_h = vp_h.saturating_sub(cell_h) = 851 - 851 = 1
```

— a **2482×1 pixel browser viewport**, and a `PointerMap` with `cell_w = 2482`. This exact
failure was predicted at `artifacts/swarm/C05-detector-hardening.md:204-206` and is unfixed.

Reachability is not hypothetical: the probe deadline is hard-coded to **300 ms** at both call
sites (`main.rs:118`, `main.rs:241`) and is **never raised for SSH**, even though `caps.rs:136`
already detects `c.remote`. On a link with >300 ms RTT — squarely inside the stated SSH use
case — probes time out and replies desync into the following read.

### P1-6 — `expected_payload()` integer overflow accepts a bogus frame — **proven**

`crates/tf-proto/src/lib.rs:51-53` computes `width * height * 4` in `usize` with no checked
arithmetic. Release builds have overflow checks off (`Cargo.toml` `[profile.release]` does not
set `overflow-checks`).

```
$ adv overflow
width=2147483648 height=2147483648
expected_payload() = 0
true value would be = 18446744073709551616
RESULT: OVERFLOW CONFIRMED -- an 8-exabyte frame reports a 0-byte payload
```

`2^31 × 2^31 × 4 = 2^64`, which wraps to exactly **0**. The truncation guard at
`main.rs:834` (`pixels.len() < h.expected_payload()`) then passes trivially, and the frame is
**accepted**: `status.frames += 1` (`main.rs:840`), fps is polluted (`main.rs:841-844`), and
`page_w`/`page_h` are poisoned to 2147483648 (`main.rs:838-839`).

**Honest scope limit:** I tried to escalate this to the `assert_eq!` panic in
`encode_rgb_frame` and **could not**. `present()` bails on `self.rgb.is_empty()`
(`main.rs:849`), and I searched the `u32 × u32` space for a geometry whose product wraps to a
small *non-zero* value — near 2^31 the step size is 2^31, so the only reachable wrap is the
degenerate 0. This is a data-integrity and metric-corruption bug, **not** memory unsafety. It
should still be a `checked_mul`.

### P1-7 — Terminal resize is dead code: the engine implements it, the CLI never calls it

```
$ grep -rn "SIGWINCH" crates/ apps/          → NONE (outside node_modules typings)
$ grep -n '"t":"resize"' apps/cli/src/main.rs → NONE
$ grep -n "case 'resize'" apps/engine/src/main.js → 231
```

`apps/engine/src/main.js:231-238` correctly handles a resize command including a forced
`invalidate()`. **Nothing ever sends one.** There is no `SIGWINCH` handler and the poll loop
(`main.rs:464-558`) never re-queries geometry. Resize your terminal mid-session and the page
stays at its original pixel size for the life of the process while the image no longer matches
the window. For a product whose entire premise is "a browser sized to your terminal", this is
a headline functional gap, not an edge case.

### P2-8 — Three unchecked-slice panics in public API — **all proven**

```
$ adv halfblock
panicked at crates/tf-term/src/unicode.rs:26:10:
index out of bounds: the len is 3 but the index is 1500

$ adv rect
panicked at crates/tf-term/src/kitty.rs:61:25:
range end index 400 out of range for slice of length 64

$ adv encode
assertion `left == right` failed: rgb buffer size must match w*h*3
  left: 12   right: 30000
```

- `unicode.rs:22-27` — the `px` closure indexes `rgb[i..i+2]` with no length check against
  `w*h*3`.
- `kitty.rs:61` — `bgra_rect_to_rgb` slices `bgra[start..start + rect.w*4]` with no bounds
  validation against the image.
- `kitty.rs:108` — `encode_rgb_frame` uses `assert_eq!` **although it already returns
  `io::Result`**. A size mismatch is exactly what an `Err` variant is for; panicking from the
  render hot path aborts a browser mid-frame.

Currently guarded by callers, so P2 — but all three are `pub`, and `bgra_rect_to_rgb` is the
designated entry point for the damage-encoder work in C08. It will be called with
caller-computed rects, which is precisely when this bites.

### P2-9 — `json_get_str` is a substring scanner documented as a parser — **proven**

`crates/tf-proto/src/lib.rs:120-155` is documented "Extract a **top-level** string field from a
flat JSON object". It does no nesting or depth tracking — `json.find()` matches the first
occurrence at any depth.

```
$ adv json
input : {"t":"outer","inner":{"v":"NESTED"},"v":"REAL"}
json_get_str(.., "v") = Some("NESTED")        ← wrong; a parser returns "REAL"
```

Not currently exploitable from a hostile page title: `JSON.stringify` on the engine side
(`main.js:61`) escapes embedded quotes, so a title cannot forge a key. The defect is that the
safety property depends entirely on an invariant asserted in a comment on the *other side of a
process boundary*, in a different language. If any future event gains a nested object — E07
already proposes richer envelopes — this silently returns the wrong field. Either enforce
depth or fix the doc comment to say "first match at any depth".

### P2-10 — Per-frame copy amplification undercuts the throughput story

For each 8 MB frame the core performs at minimum three full passes before a pixel is encoded:
`feed` → `extend_from_slice` (`lib.rs:73`), `next_message` → `self.buf[5..5+len].to_vec()`
(`lib.rs:90`), then `drain(..5+len)` (`lib.rs:91`). `on_frame` then runs the full
BGRA→RGB conversion (`main.rs:837`) for **every** decoded frame — including frames that
`present()` will never draw, since `dirty` is a single flag and only the last frame survives a
batch. At 60 fps that is ≈1.5 GB/s of memcpy performing work that is then discarded. A
zero-copy `next_message` returning a slice, plus deferring `bgra_to_rgb` into `present()`,
removes most of it.

### P2-11 — Engine backpressure can wedge on a destroyed socket

`apps/engine/src/main.js:70-79`: `sendMessage` returns `false` both for genuine backpressure
**and** for `!sock || sock.destroyed` (`main.js:53`). The `else` branch treats both identically
and registers `sock.once('drain', …)`. On a destroyed socket `drain` never fires,
`writeInFlight` stays `true` forever, and every subsequent frame silently coalesces into a
pending slot that is never flushed. Mitigated in practice by the `onPaint` guard
(`main.js:83`) and `sock.on('close', () => app.exit(0))` (`main.js:307`), but the
close-between-check-and-write race is real. Distinguish "destroyed" from "buffer full".

### P2-12 — Test-count claim (see §1): 87 claimed, 96 actual.

---

## 5. What survived falsification

Stated plainly, because an adversarial report that finds only fault is as untrustworthy as one
that finds none:

- **All 96 tests genuinely pass.** Nothing weakened, skipped, or mocked-as-real.
- **`doctor` non-tty behaviour is correct**, including the exit code — better than most CLIs.
- **The 8,081,424-byte arithmetic is exactly right**, and C09's rebuttal of it is wrong.
- **The release binary exists and is current** with its sources.
- **The security posture in the engine is real**: sandbox on, `nodeIntegration: false`,
  `contextIsolation: true`, popup handler denies by default (`main.js:106-132`).
- **Terminal-safety discipline is real**: `sanitize_for_terminal` strips C0, DEL, C1, and
  U+2028/9 (`unicode.rs:60-75`), with a test using an actual OSC 52 clipboard-hijack payload.
- **The socket is correctly locked down**: 0700 dir, 0600 socket, no network listener
  (`main.rs:387-400`).
- **`bench.mjs` labels its own fixtures as non-results** (`bench.mjs:943-945`). That is exactly
  the honesty this report is asking the rest of the corpus to match.

---

## 6. Ship / no-ship

**NO-SHIP.** Blocking set, in order:

| ID | Sev | Defect | Fix cost |
|----|-----|--------|----------|
| P0-1 | P0 | Zero commits; no `.gitignore`; 105 MB `target/` untracked | minutes |
| P0-2 | P0 | `license` declared, no LICENSE file | minutes |
| P0-3 | P0 | Root `package.json` belongs to another project | seconds |
| P1-4 | P1 | `MessageReader` unbounded alloc (known since B08, unfixed) | ~5 lines |
| P1-5 | P1 | `parse_two_param_t` ignores report-type; 1px viewport on SSH | ~10 lines |
| P1-6 | P1 | `expected_payload()` overflow accepts bogus frame | 1 line (`checked_mul`) |
| P1-7 | P1 | Resize unwired: no SIGWINCH, CLI never sends `resize` | ~30 lines |
| P1-E | P1 | Evidence: e2e tuple is a composite; no primary log; 60 fps is engine-only | 1 run |

P2-8 through P2-12 are quality debt and should not block.

The code is in better shape than the evidence. Nothing above suggests the design is wrong —
it suggests the project has been documented faster than it has been recorded.

---

## 7. Single most actionable recommendation

**Run the bounded-run harness once in Ghostty and commit the raw log — then make that log the
only citable source for every performance number in the corpus.**

Everything needed already exists. `D10-ux-validation.md:78` specifies the command,
`main.rs:48-50` provides the `TERMINAL_FENSTER_EXIT_AFTER_MS` hook, `main.rs:32-41` writes the log,
and `bench.mjs:190` already parses this exact format:

```
TERMINAL_FENSTER_EXIT_AFTER_MS=10000 TERMINAL_FENSTER_LOG=artifacts/e2e/ghostty-$(date +%s).log \
  ./target/release/terminal-fenster open https://example.com
```

(The agent Bash sandbox blocks Chromium child processes with `bootstrap_look_up … Permission
denied`, so this must be operator-run with the sandbox disabled — which is also why I could
not produce it myself, and why I have marked the e2e claim unverified rather than false.)

Two changes make it decisive. First, add `wire_bytes` and `encode_ms` to the **first-frame**
log line (`main.rs:539-544`) so the headline tuple comes from one record instead of being
spliced from two. Second, log a p50/p99 frame-gap summary at exit so "60 fps" can be stated
end-to-end rather than borrowed from a 1440×900 engine spike.

That single artifact retires P1-E, converts seven circular citations into real ones, corrects
C09, and gives CI a regression baseline. It is the highest evidence-per-minute action
available, and it costs one command plus roughly twenty lines of logging.

---

*Harness (outside the repo, no repo file modified):*
`/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/adv`
— `cargo run --release -- <overflow|unbounded|halfblock|rect|encode|json>`
