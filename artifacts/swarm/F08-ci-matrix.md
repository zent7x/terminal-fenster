# F08: CI Design, Test Tiers, and a Mock-PTY Terminal Conformance Harness

Status: design complete, harness prototyped and **validated on this machine**.
Author: F08. Date: 2026-07-31.
Scope: this document plus a prototype in a scratchpad. No repo source files were modified.

---

## 0. Bottom line first

BlackGlass looks like a project that cannot be tested in CI (a browser, in a terminal, drawing
pixels). It mostly can. I measured the split:

| Tier | What it covers | Needs a graphics terminal? | Needs Electron? | Runs on GH-hosted runners |
|---|---|---|---|---|
| 0 build/lint | compile, fmt, clippy | no | no | yes |
| 1 unit | **96** Rust tests | no | no | yes |
| 2 MCP protocol | 24 JSON-RPC checks | no | no | yes |
| 3 **PTY conformance** (new) | capability detection, backend choice, geometry, terminal restore, signal restore | **no** | no | **yes** |
| 4 engine e2e | input injection against real Chromium pixels | no | yes | macOS yes, Linux needs xvfb |
| 5 real terminal | Ghostty/kitty actually painting | **yes** | yes | **no**, manual gate |

Only tier 5 is genuinely un-CI-able. Tier 3 is the new capability this document delivers: it
tests the terminal protocol layer with **no terminal emulator at all**, by putting the real
binary on a kernel PTY and replaying recorded escape-sequence transcripts from the master side.

**There is a hard blocker before any of this can run.** See section 1.

---

## 1. BLOCKER: the repository has no commits

```
$ git rev-list --all --count
0
$ git ls-files | wc -l
0
$ ls -la .gitignore
ls: .gitignore: No such file or directory
```

Nothing is tracked. GitHub Actions cannot run on a repo with no commits, so every workflow in
this document is inert until an initial commit exists. Two things must happen together, because
committing without the first would be worse than not committing:

1. Add a `.gitignore`. There is currently **414 MB** of build output sitting untracked in the
   worktree (`target/` 105 MB, `apps/engine/node_modules/` 309 MB), plus `crates/.DS_Store`.
   With the disk at 98%, an accidental `git add -A` would be genuinely painful.
2. Make the initial commit and push.

Minimum `.gitignore` (this file is unowned by me; I did not create it):

```gitignore
/target/
**/node_modules/
.DS_Store
apps/engine/spike/out/
benchmarks/probes/*.o
tests/e2e/*-results.json
```

Two smaller repo-hygiene items found while surveying:

- `/package.json` at the repo root is unrelated leftover content:
  `{"name":"qtest","version":"1.0.0","main":"safestore.js"}`. There is no `safestore.js`. Any
  root-level `npm` invocation in CI will pick this up. It should be deleted or replaced with a
  real workspace manifest.
- `packages/mcp/package.json` declares `"dependencies": {}` and has no lockfile, so `npm ci`
  will fail there. That is fine, because it needs no install; CI should invoke
  `node packages/mcp/test/handshake.js` directly.

---

## 2. Corrections to the brief, from measurement

The mission brief states 87 unit tests. The current count is **96**:

```
$ cargo test --workspace 2>&1 | grep -E "test result" | awk '{s+=$4} END {print s}'
96
```

Broken down: `bg-term` 70, `bg-proto` 12, `blackglass` (bin) 14, doc-tests 0. Whole suite runs
in **0.08 s** wall clock. The number presumably grew as other agents landed work. CI should
never hard-code a test count, but the growth is worth knowing.

Two gates in the brief's implied "just turn CI on" plan would go red on day one:

**`cargo fmt --all --check` currently fails.** Example diff, `apps/cli/src/main.rs:30`, where
rustfmt wants to expand the deliberate one-line `let ... else`:

```
-    let Ok(path) = std::env::var("BLACKGLASS_LOG") else { return };
+    let Ok(path) = std::env::var("BLACKGLASS_LOG") else {
+        return;
+    };
```

The existing code is hand-formatted in a consistent, wider style. Do not let CI reformat it by
fiat. Either commit one mechanical `cargo fmt --all` pass and accept rustfmt's taste from then
on, or add a `rustfmt.toml` encoding the house style (`max_width`, `single_line_let_else_max_width`)
and only then make the check blocking. Until that decision is made, the workflow below runs fmt
as **non-blocking**.

**`cargo clippy` emits 6 distinct warnings, 0 errors.** So `-D warnings` would also fail today.
The warnings are all minor (`manually reimplementing div_ceil` x2, `too many arguments (8/7)`,
`very complex type`, `std::io::Error::other`, `direct cast of function item into an integer`).
The workflow runs clippy non-blocking with an explicit TODO to promote it once the count is zero.

I did not fix either, because `crates/`, `apps/cli/`, and root config are commander-owned.

---

## 3. What can run headless, and why

### 3.1 The 96 unit tests: fully portable, no tty

I checked what the tests actually touch. The tty-dependent surface is small and already isolated:

- `crates/bg-term/src/caps.rs` tests exercise **pure parsers** (`parse_da1_has_sixel`,
  `parse_two_param_t`, `parse_decrqm_supported`) against recorded byte strings. No fd involved.
- `crates/bg-term/src/tty.rs:250` `acquire_rejects_non_tty` opens `/dev/null` and asserts
  `TtyGuard::acquire` errors. That works anywhere.
- Everything in `kitty.rs`, `unicode.rs`, `b64.rs`, `input.rs`, `bg-proto` is pure byte-in/byte-out.

There are no `#[cfg(target_os)]` gates anywhere in the workspace, so the same tests compile and
run on Linux and macOS. `libc` usage is confined to `tty.rs` (36 sites), `caps.rs` (3), and
`apps/cli/src/main.rs` (10), all POSIX-portable calls (`tcgetattr`, `tcsetattr`, `poll`, `read`,
`ioctl`, `isatty`, `signal`, `raise`).

**Verdict: tier 1 runs unmodified on both platforms.** This is the cheap, high-signal core of CI.

### 3.2 The MCP server: headless, no Electron, no network

```
$ node packages/mcp/test/handshake.js
...
24/24 checks passed
(exit 0)
```

It spawns the server over stdio and drives JSON-RPC. It never starts a browser (one of its own
assertions is literally "calling a page tool with no session is a tool error, not a crash").
Free CI signal. `packages/mcp/package.json` requires `node >= 22`.

### 3.3 The engine e2e: headless, but needs Electron

`tests/e2e/input-injection.js:5-7` says it plainly, and I verified the claim holds: it speaks
the engine wire protocol over a unix socket directly, so it needs no terminal. Better, the page
under test is a `data:text/html;...` URL (`tests/e2e/input-injection.js:77`), so **it needs no
network** either. It is hermetic. Its prior run
(`tests/e2e/input-injection-results.json`) shows 9/9 passing including pixel-level assertions
and Chromium `150.0.7871.129`.

Cost: `apps/engine/node_modules` is 309 MB, essentially all Electron. That must be cached.

Linux caveat, and I want to be honest that this is the one part of the design I could **not**
verify: `apps/engine/src/main.js:107,111` uses `offscreen: true` with `sandbox: true`. On Linux
this needs a display (`xvfb-run`), and on `ubuntu-24.04` runners the Chromium sandbox trips over
AppArmor's restriction on unprivileged user namespaces. The usual mitigation is
`sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` before the run. I cannot test
this here (no Linux box, disk at 98%, and per the brief Chromium children already fail under the
agent sandbox on this machine). **The workflow therefore marks the Linux e2e job
`continue-on-error: true` with a note to promote it to required once it is observed green.**
Claiming it works would be fabrication; the first CI run is the experiment.

### 3.4 What cannot be tested in CI

Anything that requires a terminal emulator to actually rasterize: that a kitty-protocol image
lands at the right cell, that colors are right, that a human can read the text. That needs a GPU,
a display, and a real Ghostty/kitty/WezTerm. GitHub-hosted runners have none of it. This stays a
documented manual gate (section 8), ideally run against the release candidate before tagging.

---

## 4. The mock PTY: design and validation

This is the core deliverable. It closes the gap between "pure parser unit tests" and "a human in
Ghostty", and it is the only way to regression-test capability detection.

### 4.1 The decisive constraint

`caps.rs` is asymmetric about I/O, and this single fact determines the whole design:

```rust
// crates/bg-term/src/caps.rs:116-121
fn query(fd: RawFd, seq: &[u8], deadline: Duration, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let mut out = io::stdout();          // <-- writes to stdout, hard-coded
    let _ = out.write_all(seq);
    let _ = out.flush();
    read_reply(fd, deadline, done)       // <-- reads from the passed fd
}
```

Queries go out on `io::stdout()`; replies come in on an arbitrary `fd`. An **in-process** replay
(hand `detect()` a pipe) therefore cannot capture the outgoing queries, and could not verify that
the binary asked the right questions in the right order. It would also require editing
commander-owned core.

Additionally `TtyGuard::acquire` hard-refuses anything that is not a tty:

```rust
// crates/bg-term/src/tty.rs:85
if unsafe { libc::isatty(fd) } != 1 { return Err(...) }
```

So a plain pipe is rejected outright. A PTY is not a workaround here, it is the only door.

**Design decision: run the harness out-of-process.** Spawn the real binary with stdin, stdout and
stderr all attached to the *slave* side of a kernel PTY. Now stdout *is* the terminal, the
asymmetry disappears, `isatty` returns 1, and the parent holding the *master* side sees the
queries and injects the replies. This requires **zero changes to any core file** and, better, it
black-box tests the shipped binary rather than an internal function.

It also gives us `TIOCSWINSZ` for free: the parent sets the window size on the master, so
`window_size()` (`tty.rs:205`, a real `TIOCGWINSZ` ioctl) returns whatever the fixture declares.
The ioctl path becomes testable, not just the escape-sequence path.

### 4.2 Protocol flow

`detect()` (`caps.rs:128-194`) issues exactly six queries in a fixed order. The harness keys off
each query's bytes and writes back the recorded reply:

| # | Query sent | Completion predicate | Capability |
|---|---|---|---|
| 1 | `ESC _ G i=31,s=1,v=1,a=q,t=d,f=24; AAAA ESC \` | sees `ESC \` or `OK` | kitty graphics |
| 2 | `ESC [ c` | ends with `c` | sixel (DA1 param 4) |
| 3 | `ESC [ ? u` | ends with `u` | kitty keyboard |
| 4 | `ESC [ 14 t` | ends with `t` | window px |
| 5 | `ESC [ 16 t` | ends with `t` | cell px |
| 6 | `ESC [ ? 1016 $ p` | ends with `y` | SGR-pixels mouse |

An empty recorded reply is a first-class fixture value: it means "this terminal stays silent",
which is exactly how Apple Terminal behaves for 5 of the 6, and it forces the code down its
300 ms-deadline timeout path. That path is otherwise completely untested.

### 4.3 It works. Evidence.

Running the real `target/debug/blackglass doctor` under the harness with the recorded Ghostty
1.3.1 transcript reproduces **every measured value from the brief, exactly**:

```
=== EXIT CODE: 0
=== QUERIES MATCHED: 6 / 6

  graphics
    kitty graphics      yes
    sixel               no
    --> backend         kitty
  input
    kitty keyboard      yes
    sgr-pixels mouse    yes
  geometry
    cells               146x23
    window px (ioctl)   2488x858        <- injected via TIOCSWINSZ
    window px (CSI 14t) 2482x851        <- injected via escape reply
    cell px             17x37
    page viewport       2482x851
```

The Apple Terminal transcript (5 silent queries) yields `backend unicode`, `cell px 7x15`
derived from the ioctl rather than queried, and the full "no graphics protocol" advisory. Wall
clock **1.600 s**, which is 5 x 300 ms of deadline plus change: the timeout path is genuinely
being walked, not short-circuited.

Two further invariants turned out to be assertable from the master-side transcript, and both are
currently untested by anything:

**Terminal restoration.** All 11 bytes of `RESTORE_SEQ` (`tty.rs:27-40`) were observed on the
wire, plus the kitty probe-image cleanup:

```
restore-seq bytes present on the wire: 11 / 11
missing: []
probe image cleanup (a=d,d=I,i=31) emitted: True
```

**Signal restoration.** `tty.rs` has 4 unit tests, none of which touch the signal handler, which
is the single most paranoid and most user-visible code in the project. Sending `SIGTERM` mid-probe:

```
child killed by signal: True signal: 15
restore bytes emitted on SIGTERM path: 5 / 5
```

The process both restores the terminal and still dies by signal, preserving the 128+N exit status
the module docs promise. That is now a CI-enforceable guarantee.

### 4.4 It is not a rubber stamp

A harness that only ever passes is worthless. Mutation test, injecting one wrong expectation into
a MEASURED fixture and one into an ASSUMED fixture:

```
PASS  Apple Terminal 465  (6/6 queries)
FAIL  Ghostty 1.3.1
        stdout missing: '--> backend         sixel'
WARN  iTerm2 3.6.9  [advisory: provenance is ASSUMED]
        stdout missing: '--> backend         unicode'
PASS  SIGTERM mid-probe (terminal restoration invariant)  (2/6 queries)

3/4 fixtures passed
EXIT: 1
```

Correct regression detection, correct exit code, and correct provenance gating (below). The clean
suite is `4/4, exit 0` and was run three consecutive times with identical output, no flakes, in
**2.4 s** total.

### 4.5 Fixture provenance: the rule that keeps this honest

A replay harness can quietly launder a guess into a green check mark. To prevent that, **every
fixture carries a mandatory `provenance` field**, and the harness changes behavior based on it:

- `MEASURED ...` (with machine and date): failures are **hard failures**, they break the build.
- `ASSUMED ...` or `UNVERIFIED ...`: failures **warn only**, they never break the build.

This matters immediately for iTerm2. The brief says iTerm2 3.6.9 is UNVERIFIED because macOS TCC
blocks automation. But `caps.rs:189` asserts "iTerm2 3.6.9 was measured to support the Kitty
graphics protocol" and `caps.rs:201` asserts "4 is exactly what iTerm2 returns for 1016". Those
two claims contradict the brief's UNVERIFIED status. **Someone should reconcile that.** Until
then the iTerm2 fixture is marked ASSUMED and is advisory-only, so it documents the belief and
regression-tests the intent without ever letting an unmeasured assumption gate a merge.

The iTerm2 fixture is valuable even as advisory, because it pins the exact bug `caps.rs:196-211`
warns about: a DECRQM reply of `ESC [ ? 1016 ; 4 $ y` (permanently reset) must yield
`sgr-pixels mouse no`. Misreading that as support collapses every click into the page's top-left
corner. The fixture asserts the correct reading, and it does so without needing iTerm2 installed
or TCC unblocked.

### 4.6 Why Python

The harness is Python 3 stdlib only (`pty`, `termios`, `fcntl`, `struct`, `select`), which is
preinstalled on every GitHub-hosted runner. That means zero new Rust dependencies (the workspace
currently has exactly two: `libc` and `flate2`) and zero core-file edits.

The alternative, a Rust integration test at `apps/cli/tests/pty.rs` using `libc::forkpty`, is
also dependency-free and would fold these cases into `cargo test`. It is the better long-term
home. I chose Python for v1 only because it lands without touching commander-owned files. Worth
revisiting once the CI shape is settled.

---

## 5. Runner matrix

| Job | ubuntu-24.04 | macos-15 (arm64) | Blocking | Cold time |
|---|---|---|---|---|
| lint (fmt, clippy) | yes | - | **no**, see 2 | ~1 min |
| unit (96 tests) | yes | yes | yes | ~1 min |
| msrv (1.80 check) | yes | - | yes | ~1 min |
| pty conformance | yes | yes | yes | ~1 min |
| mcp handshake | yes | yes | yes | ~30 s |
| engine e2e | yes, `continue-on-error` | yes | macOS only | ~3 min |

`macos-15` is arm64, matching the M4 reference machine. Notes:

- The workspace declares `rust-version = "1.80"`. I did **not** verify a 1.80 build, because
  downloading a second toolchain onto a disk at 98% would be irresponsible. The MSRV job is
  included and will tell us on first run. If it fails, the honest fix is to raise
  `rust-version`, not to weaken the job.
- No Windows. `tty.rs` is termios/POSIX throughout and there is no `#[cfg(windows)]` anywhere.
  Windows support is a product decision, not a CI one.
- x86_64 macOS (`macos-13`) is omitted to keep the matrix cheap. Add it only if you intend to
  ship Intel binaries.

---

## 6. The workflow

Do not create this file yet; it needs the initial commit from section 1 first.
Target path: `.github/workflows/ci.yml`.

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  # --------------------------------------------------------------- tier 0
  lint:
    name: lint (advisory)
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2

      # NON-BLOCKING until the house style is settled. `cargo fmt --all --check`
      # fails on the current tree; the code is deliberately hand-formatted.
      # Promote to blocking after either one mechanical `cargo fmt --all` pass
      # or a committed rustfmt.toml. See F08 report section 2.
      - name: rustfmt
        continue-on-error: true
        run: cargo fmt --all -- --check

      # NON-BLOCKING: 6 clippy warnings on the current tree, 0 errors.
      # Promote to `-D warnings` once that reaches zero.
      - name: clippy
        continue-on-error: true
        run: cargo clippy --workspace --all-targets

  # --------------------------------------------------------------- tier 1
  unit:
    name: unit / ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    timeout-minutes: 20
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.os }}
      - name: cargo build
        run: cargo build --workspace --all-targets --locked
      - name: cargo test
        run: cargo test --workspace --locked -- --nocapture

  msrv:
    name: msrv 1.80
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      # Must match `rust-version` in the workspace Cargo.toml. UNVERIFIED as of
      # this workflow's authoring; if it fails, raise rust-version rather than
      # deleting this job.
      - uses: dtolnay/rust-toolchain@1.80
      - uses: Swatinem/rust-cache@v2
        with:
          key: msrv
      - run: cargo check --workspace --locked

  # --------------------------------------------------------------- tier 3
  pty:
    name: pty conformance / ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    timeout-minutes: 20
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.os }}
      - uses: actions/setup-python@v5
        with:
          python-version: '3.12'

      - name: build blackglass
        run: cargo build --locked -p blackglass

      # Replays recorded terminal transcripts on a kernel PTY. No graphics
      # terminal, no display, no GPU. Fixtures whose provenance is ASSUMED
      # warn instead of failing; see F08 report section 4.5.
      - name: replay recorded terminal transcripts
        env:
          BLACKGLASS_BIN: target/debug/blackglass
        run: python3 tests/pty/replay.py 'tests/pty/fixtures/*.json'

  # --------------------------------------------------------------- tier 2
  mcp:
    name: mcp handshake / ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    timeout-minutes: 10
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
      # packages/mcp has zero dependencies and no lockfile, so `npm ci` would
      # fail here. Run the test directly.
      - name: handshake
        run: node packages/mcp/test/handshake.js

  # --------------------------------------------------------------- tier 4
  engine-e2e:
    name: engine e2e / ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    timeout-minutes: 30
    # Linux Electron + offscreen + Chromium sandbox under AppArmor is UNVERIFIED.
    # Promote to required once observed green. See F08 report section 3.3.
    continue-on-error: ${{ matrix.os == 'ubuntu-24.04' }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'

      # Electron is ~309 MB installed. Caching it is the difference between a
      # 3-minute job and a 30-second one.
      - name: cache electron download
        uses: actions/cache@v4
        with:
          path: |
            ~/.cache/electron
            ~/Library/Caches/electron
          key: electron-${{ matrix.os }}-${{ hashFiles('apps/engine/package-lock.json') }}

      - name: install engine deps
        working-directory: apps/engine
        run: npm ci

      - name: linux prerequisites
        if: matrix.os == 'ubuntu-24.04'
        run: |
          sudo apt-get update
          sudo apt-get install -y xvfb libgbm1 libnss3 libasound2t64
          # ubuntu-24.04 restricts unprivileged user namespaces via AppArmor,
          # which the Chromium sandbox (engine main.js sets sandbox: true) needs.
          sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 || true

      # The page under test is a data: URL, so this needs no network.
      - name: input injection e2e (linux)
        if: matrix.os == 'ubuntu-24.04'
        run: xvfb-run -a node tests/e2e/input-injection.js

      - name: input injection e2e (macos)
        if: matrix.os == 'macos-15'
        run: node tests/e2e/input-injection.js

      - name: upload results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: e2e-results-${{ matrix.os }}
          path: tests/e2e/input-injection-results.json
          if-no-files-found: warn
```

---

## 7. Files the commander needs to create

I own only this report, so the harness and fixtures are reproduced here rather than written into
the tree. Validated working copies are in my scratchpad at
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`
(`replay.py`, `fx/*.json`) and can be copied verbatim.

Proposed layout:

```
tests/pty/
  replay.py
  fixtures/ghostty-1.3.1.json
  fixtures/apple-terminal-465.json
  fixtures/iterm2-3.6.9.json
  fixtures/sigterm-restore.json
```

### 7.1 `tests/pty/replay.py`

```python
#!/usr/bin/env python3
"""Mock-PTY terminal-conformance harness for BlackGlass.

Runs the real `blackglass` binary on a kernel PTY and replays a recorded
terminal's query/response transcript from the master side. No graphics
terminal, no GPU, no display, just termios and a pipe pair the kernel
happens to call a tty. Runs identically on macOS and Linux.

Usage:
    python3 tests/pty/replay.py 'tests/pty/fixtures/*.json'
    python3 tests/pty/replay.py --binary target/release/blackglass FIXTURE...
"""

import argparse
import fcntl
import glob
import json
import os
import pty
import select
import signal as sigmod
import struct
import sys
import termios
import time

DEFAULT_BINARY = "target/debug/blackglass"


def unesc(s):
    """Decode \\x1b-style escapes in fixture JSON into raw bytes."""
    return s.encode("utf-8").decode("unicode_escape").encode("latin-1")


def set_winsize(fd, ws):
    fcntl.ioctl(
        fd,
        termios.TIOCSWINSZ,
        struct.pack("HHHH", ws["rows"], ws["cols"], ws["xpixel"], ws["ypixel"]),
    )


def run_case(binary, fx, timeout=15.0):
    """Spawn the binary on a PTY, replay the transcript, return (wire, status)."""
    rules = [(unesc(e["q"]), unesc(e.get("reply", ""))) for e in fx["exchanges"]]
    argv = fx.get("argv", ["doctor"])
    sig = fx.get("signal")

    pid, master = pty.fork()
    if pid == 0:
        env = dict(os.environ)
        # Scrub the runner's own terminal identity so results depend only on
        # the fixture, never on where CI happens to be running.
        for k in ("TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM",
                  "TMUX", "STY", "SSH_CONNECTION", "SSH_TTY"):
            env.pop(k, None)
        env.update({k: v for k, v in fx.get("env", {}).items() if v != ""})
        env.setdefault("BLACKGLASS_ENGINE", "/nonexistent")
        env.pop("BLACKGLASS_BACKEND", None)
        try:
            os.execve(binary, [binary] + argv, env)
        finally:
            os._exit(127)

    set_winsize(master, fx["winsize"])

    wire = b""
    fired = set()
    eof = False
    sent_signal = False
    t0 = time.time()
    while time.time() - t0 < timeout and not eof:
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                eof = True          # slave hung up: child exited
                chunk = b""
            if not chunk:
                eof = True
            wire += chunk
        for i, (q, reply) in enumerate(rules):
            if i not in fired and q in wire:
                fired.add(i)
                if reply:
                    os.write(master, reply)
        if sig and not sent_signal and (time.time() - t0) * 1000 > sig["after_ms"]:
            os.kill(pid, getattr(sigmod, sig["send"]))
            sent_signal = True

    os.close(master)
    if not eof:
        os.kill(pid, sigmod.SIGKILL)
    _, status = os.waitpid(pid, 0)
    return wire, status, len(fired), len(rules)


def check(fx, wire, status, matched, total):
    """Return a list of failure strings; empty means the case passed."""
    fails = []
    text = wire.decode("utf-8", "replace")

    # A case that kills the process mid-probe deliberately truncates the query
    # sequence, so only require the full set when we let the binary finish.
    want_queries = fx.get("expect_queries", 0 if fx.get("signal") else total)
    if matched < want_queries:
        fails.append(f"only {matched}/{total} queries were issued (want >= {want_queries})")

    for want in fx.get("expect", []):
        if want not in text:
            fails.append(f"stdout missing: {want!r}")

    for want in fx.get("expect_wire", []):
        if unesc(want) not in wire:
            fails.append(f"wire missing bytes: {want!r}")

    for bad in fx.get("expect_absent", []):
        if bad in text:
            fails.append(f"stdout unexpectedly contains: {bad!r}")

    sig = fx.get("signal")
    if sig:
        if not os.WIFSIGNALED(status):
            fails.append(f"expected death by {sig['send']}, exited normally")
        elif os.WTERMSIG(status) != getattr(sigmod, sig["send"]):
            fails.append(f"died by signal {os.WTERMSIG(status)}, want {sig['send']}")
    else:
        want_code = fx.get("exit_code", 0)
        got = os.waitstatus_to_exitcode(status)
        if got != want_code:
            fails.append(f"exit code {got}, want {want_code}")

    return fails


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("fixtures", nargs="+")
    ap.add_argument("--binary", default=os.environ.get("BLACKGLASS_BIN", DEFAULT_BINARY))
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    binary = os.path.abspath(args.binary)
    if not os.access(binary, os.X_OK):
        print(f"FATAL: {binary} is not executable, build it first", file=sys.stderr)
        return 2

    paths = []
    for p in args.fixtures:
        paths.extend(sorted(glob.glob(p)) or [p])

    failed = 0
    for path in paths:
        with open(path) as f:
            fx = json.load(f)
        name = fx.get("terminal", os.path.basename(path))
        prov = fx.get("provenance", "UNSPECIFIED")
        # Fixtures that were never measured on real hardware must never gate a
        # merge. They document intent and catch regressions, advisory only.
        advisory = prov.strip().upper().startswith(("ASSUMED", "UNVERIFIED"))

        wire, status, matched, total = run_case(binary, fx)
        fails = check(fx, wire, status, matched, total)

        if args.verbose:
            print(wire.decode("utf-8", "replace"))

        if not fails:
            print(f"PASS  {name}  ({matched}/{total} queries)")
        elif advisory:
            print(f"WARN  {name}  [advisory: provenance is {prov.split(' ')[0]}]")
            for f_ in fails:
                print(f"        {f_}")
        else:
            failed += 1
            print(f"FAIL  {name}")
            for f_ in fails:
                print(f"        {f_}")

    print(f"\n{len(paths) - failed}/{len(paths)} fixtures passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
```

### 7.2 `tests/pty/fixtures/ghostty-1.3.1.json`

```json
{
  "terminal": "Ghostty 1.3.1",
  "provenance": "MEASURED 2026-07-31 on macOS 26.1 / Apple M4",
  "env": {"TERM":"xterm-ghostty","TERM_PROGRAM":"ghostty","TERM_PROGRAM_VERSION":"1.3.1","COLORTERM":"truecolor"},
  "winsize": {"rows":23,"cols":146,"xpixel":2488,"ypixel":858},
  "argv": ["doctor"],
  "exit_code": 0,
  "exchanges": [
    {"name":"kitty_graphics","q":"\\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;","reply":"\\x1b_Gi=31;OK\\x1b\\\\"},
    {"name":"da1","q":"\\x1b[c","reply":"\\x1b[?62;22;52c"},
    {"name":"kitty_keyboard","q":"\\x1b[?u","reply":"\\x1b[?1u"},
    {"name":"window_px","q":"\\x1b[14t","reply":"\\x1b[4;851;2482t"},
    {"name":"cell_px","q":"\\x1b[16t","reply":"\\x1b[6;37;17t"},
    {"name":"sgr_pixel_mouse","q":"\\x1b[?1016$p","reply":"\\x1b[?1016;2$y"}
  ],
  "expect": [
    "kitty graphics      yes","sixel               no","--> backend         kitty",
    "kitty keyboard      yes","sgr-pixels mouse    yes",
    "cells               146x23","window px (CSI 14t) 2482x851",
    "cell px             17x37","page viewport       2482x851"
  ],
  "expect_absent": ["NOTE: this terminal has no graphics protocol"],
  "expect_wire": [
    "\\x1b_Ga=d,d=I,i=31\\x1b\\\\",
    "\\x1b[<u","\\x1b[?1006l","\\x1b[?1016l","\\x1b[?1003l","\\x1b[?1002l",
    "\\x1b[?1000l","\\x1b[?1004l","\\x1b[?2004l","\\x1b_Ga=d,d=A\\x1b\\\\",
    "\\x1b[?25h","\\x1b[?1049l"
  ]
}
```

### 7.3 `tests/pty/fixtures/apple-terminal-465.json`

```json
{
  "terminal": "Apple Terminal 465",
  "provenance": "MEASURED 2026-07-31 on macOS 26.1 / Apple M4",
  "env": {"TERM":"xterm-256color","TERM_PROGRAM":"Apple_Terminal","TERM_PROGRAM_VERSION":"465"},
  "winsize": {"rows":30,"cols":120,"xpixel":840,"ypixel":450},
  "argv": ["doctor"],
  "exit_code": 0,
  "exchanges": [
    {"name":"kitty_graphics","q":"\\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;","reply":""},
    {"name":"da1","q":"\\x1b[c","reply":"\\x1b[?1;2c"},
    {"name":"kitty_keyboard","q":"\\x1b[?u","reply":""},
    {"name":"window_px","q":"\\x1b[14t","reply":""},
    {"name":"cell_px","q":"\\x1b[16t","reply":""},
    {"name":"sgr_pixel_mouse","q":"\\x1b[?1016$p","reply":""}
  ],
  "expect": [
    "kitty graphics      no","sixel               no","--> backend         unicode",
    "truecolor           no","kitty keyboard      no","sgr-pixels mouse    no",
    "window px (CSI 14t) no reply","cell px             7x15","page viewport       840x450",
    "NOTE: this terminal has no graphics protocol",
    "kitty_graphics      (no reply)"
  ],
  "expect_wire": ["\\x1b[?25h","\\x1b[?1049l","\\x1b_Ga=d,d=A\\x1b\\\\"]
}
```

### 7.4 `tests/pty/fixtures/iterm2-3.6.9.json`

Advisory only. Encodes the `caps.rs` beliefs so they are at least regression-tested, without
letting an unmeasured assumption gate a merge. Replace `provenance` with `MEASURED ...` the day
someone runs `blackglass doctor` in a real iTerm2 and confirms these bytes.

```json
{
  "terminal": "iTerm2 3.6.9",
  "provenance": "ASSUMED, derived from source comments crates/bg-term/src/caps.rs:189,201. NOT measured: iTerm2 automation is TCC-blocked on the reference machine.",
  "env": {"TERM":"xterm-256color","TERM_PROGRAM":"iTerm.app","TERM_PROGRAM_VERSION":"3.6.9","COLORTERM":"truecolor"},
  "winsize": {"rows":40,"cols":160,"xpixel":1280,"ypixel":800},
  "argv": ["doctor"],
  "exit_code": 0,
  "exchanges": [
    {"name":"kitty_graphics","q":"\\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;","reply":"\\x1b_Gi=31;OK\\x1b\\\\"},
    {"name":"da1","q":"\\x1b[c","reply":"\\x1b[?62;4c"},
    {"name":"kitty_keyboard","q":"\\x1b[?u","reply":"\\x1b[?0u"},
    {"name":"window_px","q":"\\x1b[14t","reply":"\\x1b[4;800;1280t"},
    {"name":"cell_px","q":"\\x1b[16t","reply":"\\x1b[6;20;8t"},
    {"name":"sgr_pixel_mouse","q":"\\x1b[?1016$p","reply":"\\x1b[?1016;4$y"}
  ],
  "expect": [
    "--> backend         kitty","sixel               yes",
    "sgr-pixels mouse    no","cell px             8x20",
    "NOTE: no pixel-accurate mouse"
  ]
}
```

### 7.5 `tests/pty/fixtures/sigterm-restore.json`

Not a terminal. A synthetic case guarding the invariant that matters most to users: if we die,
we still hand the shell back intact.

```json
{
  "terminal": "SIGTERM mid-probe (terminal restoration invariant)",
  "provenance": "MEASURED 2026-07-31, synthetic case, not a real terminal",
  "env": {"TERM":"xterm-256color","TERM_PROGRAM":"Apple_Terminal","TERM_PROGRAM_VERSION":"465"},
  "winsize": {"rows":30,"cols":120,"xpixel":840,"ypixel":450},
  "argv": ["doctor"],
  "signal": {"send":"SIGTERM","after_ms":400},
  "exchanges": [
    {"name":"kitty_graphics","q":"\\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;","reply":""},
    {"name":"da1","q":"\\x1b[c","reply":""},
    {"name":"kitty_keyboard","q":"\\x1b[?u","reply":""},
    {"name":"window_px","q":"\\x1b[14t","reply":""},
    {"name":"cell_px","q":"\\x1b[16t","reply":""},
    {"name":"sgr_pixel_mouse","q":"\\x1b[?1016$p","reply":""}
  ],
  "expect_wire": [
    "\\x1b[<u","\\x1b[?1006l","\\x1b[?1016l","\\x1b[?1003l","\\x1b[?1002l",
    "\\x1b[?1000l","\\x1b[?1004l","\\x1b[?2004l","\\x1b_Ga=d,d=A\\x1b\\\\",
    "\\x1b[?25h","\\x1b[?1049l"
  ]
}
```

---

## 8. The manual gate CI cannot replace

Before tagging a release, on a real machine with a real terminal, run `blackglass doctor` and
`blackglass open https://example.com` in Ghostty, kitty, WezTerm, and Apple Terminal. Confirm
text is legible, the image lands in the right place, colors are right, the mouse hits what you
aimed at, and `ctrl+q` leaves a clean shell. Then, for each terminal, capture the `raw replies`
block from `doctor` and commit it as a MEASURED fixture. **That is the loop that makes tier 3
worth having:** every manual session permanently converts into an automated regression test, and
the manual gate shrinks over time instead of repeating forever.

---

## 9. Core changes I recommend but did not make

Per the ownership rule, described rather than done.

1. **`apps/cli/src/main.rs:97`, implement or drop `doctor --replay`.** `caps.rs:253` documents
   `read_all` as being "used by tests and by `doctor --replay`", but the signature is
   `fn cmd_doctor(_args: &[String])`, and the underscore confirms the args are ignored.
   `--replay` does not exist. Given the PTY harness now covers replay end to end and more
   faithfully, my recommendation is to **delete the stale doc reference** rather than build the
   feature.
2. **`apps/cli/src/main.rs:97`, add `doctor --json`.** The harness currently asserts against
   human-readable output with hard-coded column alignment, which is brittle: any cosmetic change
   to the report breaks fixtures. A stable JSON output would let fixtures assert on fields
   instead of whitespace. This is the single highest-value change for CI durability.
3. **Root `package.json`**, delete or replace the `qtest`/`safestore.js` leftover.
4. Optional, later: move the harness to `apps/cli/tests/pty.rs` using `libc::forkpty` so it runs
   inside `cargo test` with no new dependency and no second language.

---

## 10. Honest limitations

- **Linux is entirely unverified.** Every Linux claim here is reasoned from POSIX portability
  and GitHub runner documentation, not measured. I have no Linux machine and cannot spare the
  disk for a container. The Python harness uses only POSIX APIs and should port cleanly, but
  the first CI run is the experiment, not this document.
- **MSRV 1.80 is unverified**, for the same disk reason. See section 5.
- **Electron on Linux under AppArmor is unverified** and is the likeliest job to fail first.
  It is deliberately non-blocking. See section 3.3.
- The harness asserts on formatted human output, which is brittle until recommendation 9.2 lands.
- The iTerm2 fixture is ASSUMED, not measured, and is advisory by design. It also surfaces a real
  contradiction between the brief and `caps.rs:189` that someone should resolve.
- Everything marked MEASURED was measured on this machine (macOS 26.1, Apple M4) on 2026-07-31
  with `target/debug/blackglass` built from the current worktree, and each result above is
  reproducible with the commands shown.
