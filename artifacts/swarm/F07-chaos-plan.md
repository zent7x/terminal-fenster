# F07 — Chaos and Reliability Test Plan

**Mission:** kill the engine mid-frame, sever the socket, corrupt frame payloads, resize rapidly,
exhaust queues, soak for 8h with leak detection. For each: exact induction command, expected
correct behaviour, observable pass criterion.

**Critical focus:** in every case, is the user's terminal left usable? This document enumerates
the cases where it is **not**, and what must be fixed.

**Status of this document:** design + defect register. No core files were modified. Every defect
below is cited to `file:line` in the tree at the time of writing. Nine defects (D1–D9) are
identified; three of them (**D1, D2, D4**) leave the user with a destroyed terminal.

---

## 0. Executive summary

Terminal-Fenster's terminal restoration is genuinely well engineered for the failure modes it
anticipates. `crates/tf-term/src/tty.rs` wires `restore_raw()` to normal drop, early return,
panic hook, and four signals, and `Cargo.toml` deliberately pins `panic = "unwind"` so the hook
actually runs. Engine death, socket EOF, truncated frames, and hostile page titles are all
handled correctly today, and I verified each by reading the code path end to end.

The gaps are all in the paths nobody wired up:

| # | Defect | Terminal survives? |
|---|--------|--------------------|
| **D1** | `SIGKILL` leaves raw mode + mouse tracking + alt screen; no recovery subcommand exists | **NO** |
| **D2** | `SIGSTOP`/`SIGTSTP` freeze the process holding the tty; `ISIG` is off so ctrl+c and ctrl+z do nothing — the user's only exit is `kill -9`, i.e. D1 | **NO** |
| **D3** | `terminal-fenster doctor` looks like it repairs a broken terminal but restores raw termios, half-fixing it | **NO** (makes D1 worse) |
| **D4** | `MessageReader` accepts an unbounded `u32` length; a corrupt length allocates without limit → Rust alloc abort → **no unwind, no restore** | **NO** |
| **D5** | Engine is orphaned when the CLI is `SIGKILL`ed — a headless Chromium tree keeps burning CPU | tty n/a |
| **D6** | No `SIGWINCH` handling anywhere; CLI never sends `{"t":"resize"}` | yes (functional break only) |
| **D7** | Renderer crash is recorded into `Status::crashed` and never rendered — silent freeze | yes |
| **D8** | `FrameHeader::expected_payload()` multiplies unchecked; wraps in release | yes (crash) |
| **D9** | Diagnostics are written into the alt screen immediately before it is discarded — the user never sees why the browser vanished | yes (UX) |

**The single most actionable item is D1 + D3 together.** There is currently no way for a user
whose terminal has been destroyed to repair it with this tool. `terminal-fenster reset` must be added,
and it must construct a sane termios from scratch rather than reusing the saved-termios path,
because after a `SIGKILL` there is no saved state to reuse.

---

## 1. Method: proving "the terminal is still usable" without a screenshot

The environment constraint (machine at a lock screen, screenshot verification unavailable) is
not a limitation here — it forces the better design. A human looking at a terminal is a bad
oracle anyway: it is slow, subjective, and not CI-able.

Instead every experiment terminates in a mechanical predicate over the **pty slave's termios**
and the **recorded output byte stream**. Both are observable from a parent process with no
display attached.

### 1.1 The `TERM-OK` predicate

A session leaves the terminal usable if and only if all four hold after the CLI process has
exited:

```
TERM-OK(session) :=
  (1) TERMIOS-RESTORED:  tcgetattr(pty_slave).c_lflag & (ECHO|ICANON|ISIG)
                           == (ECHO|ICANON|ISIG)
                         AND c_oflag & OPOST != 0
  (2) MODES-RESTORED:    every token of RESTORE_SEQ appears in the output tail
  (3) NO-ORPHANS:        pgrep -f 'bg-socket=' returns nothing
  (4) NO-LITTER:         the session's /tmp/terminal-fenster-<pid>-<nanos>/ directory is gone
```

Clause (1) is the one that actually matters to a human: it is exactly the difference between a
shell that echoes what you type and one where you must blind-type `reset`. A pty slave starts in
cooked mode, `TtyGuard::acquire` saves that (`tty.rs:92-99`), and `restore_raw` writes it back
via `tcsetattr` (`tty.rs:61-64`). So comparing against the pre-spawn snapshot is a sound test.

Clause (2)'s token list is not invented — it is `RESTORE_SEQ` at `crates/tf-term/src/tty.rs:27-40`:

```
\x1b[<u  \x1b[?1006l  \x1b[?1016l  \x1b[?1003l  \x1b[?1002l  \x1b[?1000l
\x1b[?1004l  \x1b[?2004l  \x1b_Ga=d,d=A\x1b\\  \x1b[?25h  \x1b[?1049l
```

There is already a unit test asserting this list is complete relative to
`enable_input_protocols` (`tty.rs:222-234`). The chaos harness asserts the list is *emitted at
runtime*, which is the complementary half.

Clause (2) has real teeth independent of clause (1). `\x1b[?1003h` (any-motion mouse tracking,
enabled at `tty.rs:156`) is the nastiest mode to leave on: with it set, simply moving the mouse
over the window sprays `\x1b[<35;…M` sequences into the shell's input. A user can survive a
hidden cursor; they cannot survive that.

### 1.2 Why a pty is mandatory

`cmd_open` refuses to run unless `isatty(0)` (`apps/cli/src/main.rs:227-231`), and
`TtyGuard::acquire` refuses a non-tty (`tty.rs:85-91`, with test `acquire_rejects_non_tty` at
`tty.rs:249-255`). Piping will not work. The harness must allocate a real pty.

Python 3's `pty` module is in the standard library, so this costs no disk — which matters given
the 98%-full disk constraint. The repo already establishes this pattern:
`benchmarks/a07/ptytest.py` and `benchmarks/a07/ptythru.py` exist.

### 1.3 Substituting the engine without touching core source

This is the key enabler for the corruption experiments, and it needs **zero changes to
`apps/cli` or `apps/engine`**.

`locate_engine()` (`apps/cli/src/main.rs:319-350`) honours `$TERMINAL_FENSTER_ENGINE` and only requires
that `<dir>/node_modules/.bin/electron` exists and is executable. `Session::start`
(`main.rs:370-385`) then derives `engine_root` by walking up three parents and requires
`<engine_root>/src/main.js` to exist — it never reads it, it only stats it. The child is spawned
as:

```
<electron> <engine_root>/src/main.js --bg-socket=<path> --bg-width=W --bg-height=H --bg-url=U
```

So a fake engine is just an executable script at that path. The real engine will never emit a
malformed frame; a fake one emits nothing else. This is how every corruption case below is
induced.

```bash
# One-time setup for the adversarial engine harness.
FAKE=/tmp/bg-fake-engine
mkdir -p "$FAKE/node_modules/.bin" "$FAKE/src"
touch "$FAKE/src/main.js"                 # only stat()'d by Session::start, never read
cp artifacts/swarm/chaos/fake_engine.py "$FAKE/node_modules/.bin/electron"
chmod +x "$FAKE/node_modules/.bin/electron"
export TERMINAL_FENSTER_ENGINE="$FAKE"
```

### 1.4 Bounded runs

`TERMINAL_FENSTER_EXIT_AFTER_MS` (`main.rs:48-50`) exits through the identical shutdown path as
`ctrl+q`, and `TERMINAL_FENSTER_LOG` (`main.rs:32-41`) writes diagnostics to a file rather than stdout
— because stdout is the graphics channel and a stray log line would corrupt an image
mid-transmission. Both are used throughout. Every experiment below sets `TERMINAL_FENSTER_LOG` so that
failures are diagnosable from CI artifacts alone.

---

## 2. The restoration matrix

Every way this process can end, and whether `restore_raw()` (`tty.rs:50-65`) runs. This is the
analytical core of the mission's critical focus; the experiments in §4 are its empirical test.

| Exit path | Mechanism | `restore_raw` runs | `TERM-OK` |
|---|---|:--:|:--:|
| `ctrl+q` | `handle_event` → true → `run` returns 0 → `shutdown()` → `drop(guard)` (`main.rs:285-290`) | yes | ✅ |
| Engine exits / socket EOF | `read` → `Ok(0)` → return 1 → shutdown → drop (`main.rs:521-524`) | yes | ✅ (but see **D9**) |
| Engine read error | `main.rs:527-530` | yes | ✅ |
| `poll()` error, non-`EINTR` | `main.rs:479-485` | yes | ✅ |
| Rust panic | panic hook (`tty.rs:126-129`) + `panic = "unwind"` (`Cargo.toml`) | yes | ✅ |
| `SIGINT` / `SIGTERM` / `SIGHUP` / `SIGQUIT` | `signal_handler` (`tty.rs:67-75`), restores then re-raises with `SIG_DFL` so exit status is still 128+N | yes | ✅ |
| Viewport undeterminable | `drop(guard)` **before** `eprintln!` (`main.rs:246-250`) — correct ordering | yes | ✅ |
| `TERMINAL_FENSTER_EXIT_AFTER_MS` elapsed | `main.rs:465-473` → normal return | yes | ✅ |
| **`SIGKILL`** | uncatchable by definition | **no** | ❌ **D1** |
| **`SIGSTOP` / `SIGTSTP`** | uncatchable / not in the handled set | **no** (frozen) | ❌ **D2** |
| **Allocation failure (OOM)** | Rust's alloc error handler **aborts**; abort does not unwind, so the panic hook never runs | **no** | ❌ **D4** |
| Double panic (panic inside the hook) | abort | no | ❌ (remote) |
| Terminal emulator window closed | `SIGHUP` → handler | yes | ✅ (moot — pty is gone) |

Two observations from this table.

**The `panic = "unwind"` decision in `Cargo.toml` is load-bearing and correctly commented**
(`# we MUST run our terminal-restore panic hook; abort would corrupt the tty`). Any future
change to `panic = "abort"` — a tempting binary-size optimisation — silently converts every
panic into a terminal-destroying event. This deserves a CI guard, listed in §7.

**Abort is the hole in the panic strategy.** `panic = "unwind"` protects against `panic!`, but
not against `abort()`. Rust aborts on allocation failure. D4 is precisely a path that reaches
allocation failure, which is what elevates it from "a memory bug" to "a terminal-destroying bug".

---

## 3. Defect register

Ordered by severity, where severity is dominated by whether the user's terminal survives.

### D1 — `SIGKILL` destroys the terminal and there is no repair command

**Severity: critical.** `restore_raw()` is reachable from drop, panic, and the four handled
signals (`tty.rs:132`: `SIGINT, SIGTERM, SIGHUP, SIGQUIT`). `SIGKILL` is uncatchable, so none of
these fire. The terminal is left with:

- raw termios: no `ECHO`, no `ICANON`, no `ISIG`, no `OPOST` (`tty.rs:111-117`)
- alt screen active (`\x1b[?1049h`, `tty.rs:150`)
- cursor hidden (`\x1b[?25l`, `tty.rs:151`)
- **any-motion mouse tracking on** (`\x1b[?1003h`, `tty.rs:156`) — mouse movement sprays escapes
- SGR + SGR-pixels mouse coordinates on (`tty.rs:157-162`)
- bracketed paste and focus reporting on (`tty.rs:152-153`)
- kitty keyboard flags pushed (`\x1b[>27u`, `tty.rs:167`) — key presses arrive in an encoding the shell does not understand
- a stale page image over the shell prompt

The user must blind-type `stty sane` or `reset` with no echo and no working ctrl+c.

This is not exotic. It is the *expected* end state of every route in D2, of a hung SSH session,
and of any user who gets impatient with a frozen render.

**Fix:** add a `terminal-fenster reset` subcommand. Critically, it must **not** go through
`TtyGuard`. See D3 for why.

### D2 — `SIGSTOP`/`SIGTSTP` is a trap that funnels users into D1

**Severity: critical (as an amplifier of D1).** Verified by grep: neither `SIGTSTP` nor `SIGCONT`
appears anywhere in `crates/` or `apps/cli/`.

Raw mode clears `ISIG` (`tty.rs:112`), so from inside the session **ctrl+c does nothing and
ctrl+z does nothing**. That is correct — those keystrokes belong to the page. But it means that
when the process is stopped from another terminal (`kill -STOP`), or when it becomes
unresponsive for any other reason, the user is holding a frozen terminal with no in-band escape.

The reachable escalation is: frozen → user tries ctrl+c → nothing → user tries ctrl+z → nothing
→ user opens another terminal → `kill -9` → **D1**.

There is also a genuine unresponsiveness path that reaches this state without any external
signal, described in D2b below.

**D2b — blocking stdout write with no interrupt.** `Renderer::present` ends with
`stdout.write_all(&self.out)` then `flush()` (`main.rs:899-901`). Stdout is never set
non-blocking. On a slow link, `write_all` blocks inside the poll loop, so stdin is not read and
`ctrl+q` is not processed. Measured frame sizes make this concrete: a 2482×814 frame encodes to
53,999 wire bytes; at a 9600-baud-equivalent constrained link that is ~45 s of blocking for a
single frame. During that window the session is unkillable from the keyboard.

**Fix:** handle `SIGTSTP` (restore, then `SIG_DFL` + re-raise) and `SIGCONT` (re-enter raw mode
and re-enable input protocols, then force a full repaint). Separately, bound the stdout write so
the poll loop stays live — write in chunks and re-poll stdin between chunks, or move rendering to
a thread with a bounded handoff.

### D3 — `terminal-fenster doctor` half-repairs a destroyed terminal

**Severity: high.** This is the trap that makes D1 worse, because `doctor` is exactly what a
user will reach for.

`cmd_doctor` calls `TtyGuard::acquire` (`main.rs:111`), which does `tcgetattr` into
`SAVED_TERMIOS` (`tty.rs:92-99`). If the terminal is *already* raw from a `SIGKILL`ed session,
the "saved" termios **is the broken raw termios**. On `drop(guard)` (`main.rs:119`),
`restore_raw` writes `RESTORE_SEQ` — which does correctly fix the DEC private modes, the images,
and the alt screen — and then calls `tcsetattr(fd, TCSADRAIN, saved)`, restoring raw mode.

Net effect: the mouse stops spraying and the image disappears, so it *looks* like it worked, but
there is still no echo and no line editing. The user concludes the tool cannot fix it.

**Fix:** `terminal-fenster reset` must construct a cooked termios from first principles, not reuse
`SAVED_TERMIOS`:

```
write(fd, RESTORE_SEQ)
tcgetattr(fd, &t)
t.c_iflag |= BRKINT | ICRNL | IXON
t.c_oflag |= OPOST | ONLCR
t.c_lflag |= ECHO | ECHOE | ECHOK | ECHONL | ICANON | ISIG | IEXTEN
t.c_cflag |= CS8
t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0
tcsetattr(fd, TCSANOW, &t)      // TCSANOW, not TCSADRAIN: the output queue may be wedged
```

`TCSADRAIN` is right for the normal path (it lets queued graphics finish) but wrong for repair,
where the queue may be exactly what is stuck.

`terminal-fenster reset` should also be safe to run when nothing is wrong, and should print one line
of confirmation *after* restoring, so the user gets echo-confirmation that it worked.

### D4 — unbounded frame length → allocation abort → no restore

**Severity: critical.** `MessageReader::next_message` (`crates/tf-proto/src/lib.rs:81-93`) reads
a `u32` length prefix with **no upper bound**:

```rust
let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
if self.buf.len() < 5 + len {
    return None;
}
```

A length of `0xFFFFFFFF` (4 GiB) means `next_message` returns `None` forever while `feed`
(`lib.rs:72-74`) keeps appending unconditionally. The CLI reads up to 1 MiB per poll iteration
(`main.rs:455`, `sock_buf`) at up to 62.5 Hz, so the buffer grows without limit until the
allocator fails.

Rust's allocation failure handler **aborts**. Abort does not unwind, so the panic hook at
`tty.rs:126` never runs. The terminal is destroyed exactly as in D1, and the user gets no message
explaining why.

This is reachable from a compromised or merely buggy engine. It is also reachable from a
truncated-then-resynced stream, where a byte offset error causes pixel data to be read as a
length prefix — 4 bytes of BGRA are an excellent source of large random `u32` values.

Note the asymmetry: the *engine's* reader (`apps/engine/src/main.js:266-286`) has the identical
unbounded-length shape. It is less exposed because commands are small and the CLI is the trusted
side, but it should be capped for symmetry.

**Fix:** cap the length in `tf-proto`. A frame is `w*h*4 + 32`; the largest plausible display
today is well under 64 MiB. Introduce `MAX_MESSAGE_LEN` (suggest 64 MiB), and on violation return
a distinct `Err`/poison state so the caller can tear down cleanly through the *normal* path — the
one that restores the terminal — rather than dying in the allocator.

A secondary, cheaper mitigation: `MessageReader::feed` should refuse to grow past
`MAX_MESSAGE_LEN + slack` regardless of parse state.

### D5 — orphaned Chromium tree when the CLI is killed

**Severity: medium.** `Session::shutdown` (`main.rs:657-678`) is thorough: it sends `quit`, waits
1500 ms, then `kill()` + `wait()`, then unlinks the socket and directory. The engine reciprocates
— `sock.on('close', () => app.exit(0))` (`apps/engine/src/main.js:307`) with the comment that it
must not linger as an orphan.

That reciprocal handling is what saves the `SIGKILL` case *partially*: when the CLI dies, the
socket closes, and the engine should exit. But this depends on the engine's event loop being
responsive. If the renderer is wedged in a long task, or the engine is itself stopped, the
close event is not processed and a headless Chromium tree survives with no parent and no
terminal.

The socket directory `/tmp/terminal-fenster-<pid>-<nanos>/` (`main.rs:393-394`) is also leaked on
`SIGKILL`, accumulating across crashes.

**Fix:** two independent belts. (a) In the engine, add a periodic liveness check on the parent
pid (`process.ppid` becoming 1, or a heartbeat timeout) and self-exit. (b) `terminal-fenster reset`
should also reap: scan `/tmp/terminal-fenster-*`, and `pgrep -f 'bg-socket=<that path>'`, kill and
clean.

### D6 — no resize handling at all

**Severity: high (functional), not terminal-lethal.** Verified by grep: `SIGWINCH` appears
nowhere in `crates/` or `apps/cli/`. And the CLI **never sends** `{"t":"resize"}` — the engine's
handler at `apps/engine/src/main.js:230-238` is unreachable dead code.

`Session::run` takes `page_w, page_h, cell_h, rows` **by value** at `main.rs:441-450` and never
re-reads them. `PointerMap` is constructed once at `main.rs:269-275`. Consequences of any resize:

1. **Status bar misplacement.** The bar is written to `\x1b[{rows};1H` (`main.rs:885`) with a
   stale `rows`. Grow the window → the bar lands mid-screen. Shrink → it clamps to the last row
   but the geometry beneath it is wrong.
2. **Page never resizes.** The engine keeps painting the old dimensions, so the kitty image keeps
   its old pixel size. With no `c=`/`r=` in the placement (`kitty.rs:137-142`, both `None` by
   `Placement::default`), kitty derives cell extent from pixel size. After a shrink the image is
   taller than the window, which scrolls the alt screen and permanently desynchronises the
   status-bar row from reality.
3. **Every click is wrong, silently and permanently.** `PointerMap.page_w/page_h` are frozen at
   startup. `Renderer` does update `page_w/page_h` from each frame header (`main.rs:838-839`),
   but `Session.pointer` does not — and the frame header still reports the stale engine size
   anyway. This is the same class of bug the pointer tests were written to prevent
   (`main.rs:984-998`), reintroduced through a different door.

Note that `SIGWINCH`'s default disposition is *ignore*, so `poll()` is not even interrupted. The
session is not merely wrong about the size; it is unaware anything happened.

**Fix:** install a `SIGWINCH` handler that sets an `AtomicBool`; check it each poll iteration;
re-run `window_size()` (`tty.rs:205-216`) and the cell-size derivation; update `rows`, `cell_h`,
`page_w/page_h`, **and `self.pointer`**; send `{"t":"resize","w":…,"h":…}`; force a full repaint.
Debounce ~100 ms so a window drag does not issue hundreds of engine resizes.

The signal handler must only set the flag — `window_size` calls `ioctl`, which is not on the
async-signal-safe list, and the existing handler is scrupulous about this (`tty.rs:18-19`).

### D7 — renderer crash is recorded and never shown

**Severity: medium.** The engine reports `render-process-gone` (`apps/engine/src/main.js:125-127`)
and `Status::apply_event` stores it into `Status::crashed` (`main.rs:796-799`). Grep for
`crashed` in `main.rs` returns exactly three hits: the field declaration (line 779), the
assignment (line 797), and a unit test (line 961). **It is never rendered.**

`Renderer::present` builds the status bar from title, url, fps, bytes, and encode time
(`main.rs:890-895`) — no crash indicator. When the page's renderer dies, the engine main process
stays alive, so there is no EOF and no exit. The user sees the last good frame, frozen forever,
with a status bar still cheerfully reporting the title. There is no auto-recovery either.

**Fix:** render the crash state prominently in the bar, and offer recovery (`ctrl+r` already maps
to reload at `main.rs:575-578`, so the mechanism exists — it just needs to be discoverable at the
moment it is needed).

### D8 — unchecked multiplication in `expected_payload()`

**Severity: medium.** `FrameHeader::expected_payload` (`crates/tf-proto/src/lib.rs:51-53`) is
`self.width as usize * self.height as usize * 4`. `[profile.release]` in `Cargo.toml` does not
set `overflow-checks`, so release builds wrap silently. I verified the arithmetic:

| `width` | `height` | `expected_payload()` in release |
|---|---|---|
| `0x80000000` | `0x80000000` | **0** |
| `0xFFFFFFFF` | `0xFFFFFFFF` | 18446744039349813252 |
| `0xFFFFFFFF` | `0x40000000` | 18446744069414584320 |

The `w = h = 0x80000000` case yields **zero**, which passes the guard at `main.rs:834`
(`pixels.len() < h.expected_payload()` is `false`), so a corrupt frame is *accepted*.
`bgra_to_rgb` then produces an empty buffer, and `page_w`/`page_h` are poisoned to `0x80000000`
(`main.rs:837-839`).

The system survives this only because `Renderer::present` early-returns on `self.rgb.is_empty()`
(`main.rs:849`). That guard is load-bearing and its load-bearing role is undocumented.

A wrap to a small *non-zero* multiple of 4 is worse: `rgb` is non-empty and short, `present`
proceeds, and `kitty::encode_rgb_frame` hits its `assert_eq!` at `crates/tf-term/src/kitty.rs:108`
→ panic. The panic hook restores the terminal, so `TERM-OK` holds, but the browser dies. In debug
builds the multiply itself panics first.

An `assert_eq!` on values derived from the wire, in the render hot path, is the wrong instrument.
It converts a data-validation problem into a crash.

**Fix:** `expected_payload()` returns `Option<usize>` via `checked_mul`. Validate header geometry
against a sane bound on receipt (e.g. `width, height ∈ 1..=32768`) and drop the frame otherwise.
Replace the `assert_eq!` in `encode_rgb_frame` with a returned `Err`.

**Related, pre-emptive:** `kitty::bgra_rect_to_rgb` (`kitty.rs:54-68`) slices
`&bgra[start..start + rect.w as usize * 4]` with **no bounds validation**. The dirty rectangle
comes straight off the wire (`main.js:91-94` writes it; `lib.rs:42-45` parses it) and is never
checked against the frame dimensions. This function is currently unused by the render path, but
it is exactly what the C08 damage encoder will call. It will panic on the first out-of-range
dirty rect. Worth fixing before that lands, not after.

### D9 — failure diagnostics are written into a surface about to be destroyed

**Severity: low (UX), but it degrades every other diagnosis.** `eprint_restore`
(`main.rs:763-766`) writes to stderr with explicit `\r\n` — correct, since `OPOST` is off. But it
is called at `main.rs:522` and `main.rs:528` **while the alt screen is still active**. The
message lands on the alt screen, and `drop(guard)` then emits `\x1b[?1049l` (`tty.rs:38`),
switching back to the main screen and discarding it.

So when the engine dies, the user sees the browser vanish with **no explanation**. Note that
`cmd_open` gets this right in the viewport-failure path — `drop(guard)` comes *before* `eprintln!`
at `main.rs:246-250`. The two paths are inconsistent.

**Fix:** buffer the reason and print it after the guard is dropped. Structurally, make `run()`
return a `Result<i32, String>` and let `cmd_open` print after `drop(guard)`.

---

## 4. Chaos experiments

Common preamble for every experiment:

```bash
cd $REPO
export TERMINAL_FENSTER_LOG=/tmp/bg-chaos/$EXPERIMENT.log
export RUST_BACKTRACE=1
cargo build --release            # binary: target/release/terminal-fenster
```

Every experiment ends with the `TERM-OK` predicate from §1.1. Where an experiment is expected to
fail today, that is stated explicitly and the test is marked `xfail` so CI records the defect
rather than going green on a broken thing.

---

### C01 — Kill the engine mid-frame

**Induce.** Run against a page repainting continuously, then kill the engine while a large frame
is in flight. Targeting the engine by its socket argument is precise and avoids killing an
unrelated Electron:

```bash
target/release/terminal-fenster open https://example.com &
CLI=$!
sleep 2
# The engine is the only process carrying our socket path on its argv.
pkill -9 -f "bg-socket=.*terminal-fenster-${CLI}-"
```

To hit *mid-frame* rather than between frames, kill in a loop over a 60fps page so the kill lands
uniformly at random within the write of an 8,081,424-byte frame:

```bash
for i in $(seq 1 50); do
  # random offset within a frame interval
  python3 -c "import time,random; time.sleep(2 + random.random()*0.016)"
  pkill -9 -f 'bg-socket=' ; sleep 1
done
```

**Expected.** `read` returns `Ok(0)` → `eprint_restore("engine exited")` → `return 1`
(`main.rs:521-524`). Any partial message still sitting in `MessageReader.buf` is discarded with
the reader. `Session::shutdown` sends `quit` to a dead socket — `write_all` fails and the error is
discarded by `let _` (`main.rs:438`), which is correct here — then `try_wait` reaps the already-dead
child. `SIGPIPE` is ignored (`tty.rs:137`) so the write cannot kill us. Then `drop(guard)`
restores.

**Pass criterion.**
- `TERM-OK` holds. ✅ expected to pass today.
- Exit code is 1.
- No partial image is left on the main screen (covered by the `\x1b_Ga=d,d=A` token in clause 2).
- Over 50 randomised trials, zero variance in outcome — a flaky pass here would indicate a race
  between the reader and shutdown.

**Known gap.** The user is told nothing, because of **D9**. The `xfail` assertion is: the string
`engine exited` appears in the output stream *after* the last `\x1b[?1049l`. It currently appears
before it.

---

### C02 — Renderer crash without engine death

**Induce.** Distinct from C01: the engine survives, so there is no EOF.

```bash
target/release/terminal-fenster open https://example.com &
sleep 2
# Kill only the renderer child, leaving the Electron main process alive.
pkill -9 -f 'type=renderer.*terminal-fenster'
```

**Expected.** Engine emits `{"t":"crash",...}` (`main.js:125-127`); `Status::crashed` is set.

**Pass criterion.**
- `TERM-OK` holds after `ctrl+q`. ✅
- **`xfail`:** the status bar contains a crash indicator. Fails today — **D7**. The session
  presents a frozen frame indistinguishable from a slow page.
- **`xfail`:** `ctrl+r` recovers the page.

---

### C03 — Sever the socket, established connection

**Induce.**

```bash
target/release/terminal-fenster open https://example.com &
sleep 2
rm -f /tmp/terminal-fenster-*/engine.sock
sleep 3
```

**Expected.** **Nothing happens.** Unlinking a Unix socket path does not affect an established
connection. This experiment exists to assert that benign event is benign, and to prevent a future
"cleanup" change from introducing a spurious teardown.

**Pass criterion.** Frames keep arriving (`status.frames` still climbing in
`$TERMINAL_FENSTER_LOG`); session survives to a clean `ctrl+q`; `TERM-OK` holds. Note that
`Session::shutdown`'s `remove_file` will then fail harmlessly (`let _`, `main.rs:676`). ✅

---

### C04 — Sever the socket, half-close and abort

**Induce.** Requires the fake engine (§1.3), which can close one direction or issue `SO_LINGER 0`
to force an RST rather than an orderly FIN.

```bash
TERMINAL_FENSTER_ENGINE=/tmp/bg-fake-engine \
BG_FAKE_MODE=abort_after_frames=10 \
  target/release/terminal-fenster open about:blank
```

**Expected.** `read` returns either `Ok(0)` or `ECONNRESET`. Both are handled — the latter by the
`Err(e)` arm at `main.rs:527-530`. `EWOULDBLOCK` is correctly *not* treated as fatal
(`main.rs:526`), which matters because the stream is non-blocking (`main.rs:431`).

**Pass criterion.** `TERM-OK` holds for both variants; exit code 1; no hang. ✅ expected to pass.

---

### C05 — Truncated frame payload

**Induce.**

```bash
BG_FAKE_MODE=truncate_pixels=0.5 ...   # header claims WxH, sends half the pixels
```

**Expected.** `Renderer::on_frame` drops it: `pixels.len() < h.expected_payload()` →
`return` (`main.rs:834-836`). Frame counter must not increment.

**Pass criterion.** `status.frames` unchanged across the corrupt frame; no panic; the previously
rendered frame remains on screen; `TERM-OK`. ✅ **Already covered by unit test**
`truncated_frame_is_dropped_not_rendered` (`main.rs:1014-1024`) — this experiment confirms the
unit test's guarantee holds through the real socket path.

---

### C06 — Corrupt length prefix (the memory-exhaustion path)

**This is the highest-value corruption experiment. It is expected to fail today.**

**Induce.**

```bash
BG_FAKE_MODE=bogus_length=0xFFFFFFFF ...
# fake engine writes: [0x01][FF FF FF FF] then streams filler forever
```

**Expected (correct behaviour).** The reader rejects a length above `MAX_MESSAGE_LEN`, tears the
session down through the normal path, restores the terminal, and reports a protocol error.

**Actual behaviour today.** `next_message` returns `None` forever (`lib.rs:86-89`) while `feed`
grows without bound (`lib.rs:72-74`). RSS climbs at up to 1 MiB per poll iteration. Eventually the
allocator fails, Rust **aborts**, the panic hook does not run, and the terminal is destroyed —
**D4**.

**Pass criterion.**
- CLI RSS never exceeds 256 MiB at any sample (1 s interval via `ps -o rss= -p $CLI`).
- Exit is orderly with a non-zero code; `TERM-OK` holds.
- **`xfail` today on both clauses.**

Variants worth running once the cap lands: `len = MAX+1` (must reject), `len = MAX` (must accept
or reject deterministically, never hang), and `len` valid but stream truncated (must block
harmlessly, not spin).

---

### C07 — Corrupt frame geometry

**Induce.** Three sub-cases, from the arithmetic verified in D8:

```bash
BG_FAKE_MODE=geometry=0x80000000,0x80000000   # expected_payload() wraps to 0
BG_FAKE_MODE=geometry=0xFFFFFFFF,0xFFFFFFFF   # wraps huge
BG_FAKE_MODE=geometry=0,0                     # degenerate
BG_FAKE_MODE=geometry_mismatch                # header WxH inconsistent with byte count
```

**Expected (correct).** Header geometry validated against a sane bound on receipt; frame dropped;
session continues.

**Actual today.** Case 1 is *accepted* with a zero-length payload and poisons `page_w/page_h`;
survival depends on the undocumented `rgb.is_empty()` guard at `main.rs:849`. A wrap to a small
non-zero value reaches the `assert_eq!` at `kitty.rs:108` and panics — terminal restored by the
hook, but the browser dies.

**Pass criterion.**
- No panic in any sub-case.
- `status.frames` does not increment for any rejected frame.
- `Renderer.page_w/page_h` never take a value outside `1..=32768`.
- `TERM-OK` holds. ✅ (holds today via the panic hook, but through a crash — record as
  `pass-with-crash`, which is not the same as pass.)

---

### C08 — Hostile page content aimed at the terminal

**Induce.** The status bar is the only place page-controlled text reaches the terminal
(`main.rs:887-888`). A page whose title contains escape sequences is the attack.

```bash
cat > /tmp/hostile.html <<'EOF'
<html><head><title>PWN&#x1b;[2J&#x1b;[?1049l&#x1b;]0;owned&#x07;&#x9b;31m</title></head>
<body>x</body></html>
EOF
target/release/terminal-fenster open /tmp/hostile.html
```

Extend with: OSC 52 clipboard write, `\x1b]0;` title-set, DCS introducer, `\x9b` (single-byte C1
CSI), U+2028/2029, and a 100 KB title.

**Expected.** `unicode::sanitize_for_terminal` replaces every dangerous codepoint with U+FFFD and
truncates to 40 (title) / 60 (url) chars. I read the implementation: it covers C0 (`< 0x20`,
which includes ESC `0x1b`), DEL `0x7f`, **C1 `0x80..=0x9f` (which includes the single-byte CSI
`0x9b`)**, and U+2028/2029. That C1 coverage is the subtle one most implementations miss, and it
is correct here.

**Pass criterion.** The recorded output stream contains no `\x1b`, `\x9b`, or `\x07` byte between
the status-bar cursor-position sequence (`\x1b[{rows};1H`) and the closing `\x1b[0m`, other than
the framing sequences the renderer itself emits. `TERM-OK` holds. ✅ expected to pass — this
should be locked in as a regression test because it is a security property, not just a
reliability one.

---

### C09 — Rapid resize

**Induce.** Resize the pty and deliver `SIGWINCH`, which is what a real terminal emulator does:

```python
import fcntl, termios, struct, signal, os, random
for _ in range(200):
    rows = random.randint(10, 60); cols = random.randint(40, 200)
    xpix, ypix = cols * 17, rows * 37          # Ghostty 1.3.1 cell: 17x37 px (measured)
    fcntl.ioctl(master_fd, termios.TIOCSWINSZ,
                struct.pack('HHHH', rows, cols, xpix, ypix))
    os.kill(cli_pid, signal.SIGWINCH)
    time.sleep(random.uniform(0.005, 0.05))    # faster than a human drag
```

Then settle at a known size and hold for 3 s.

**Expected (correct).** Debounced resize; `{"t":"resize","w":…,"h":…}` sent to the engine; new
frames at the new geometry; status bar on the true last row; `PointerMap` updated so a click at
the new bottom-right maps near the new page's bottom-right.

**Actual today.** Nothing happens at all — **D6**. `SIGWINCH`'s default disposition is ignore, so
`poll()` is not even interrupted. All of: engine geometry, status-bar row, and pointer mapping
remain frozen at startup values.

**Pass criterion.**
- After settling, a `resize` command with the final `w`/`h` appears in the engine's received
  commands (fake-engine mode records them).
- Final frame header geometry matches the final pty size minus one status row.
- A synthetic click at `(cols, rows-1)` maps to within one cell of the page's bottom-right —
  reusing the assertion style of `cell_coordinates_treated_as_pixels_would_collapse_the_page`
  (`main.rs:984-998`).
- Engine resize commands ≤ 25 for 200 `SIGWINCH`s (debounce working).
- `TERM-OK` holds.
- **`xfail` on the first four clauses today.** `TERM-OK` itself does hold — resize is a functional
  break, not a terminal-lethal one.

**Resize-during-teardown sub-case.** Deliver `SIGWINCH` during the 1500 ms shutdown window
(`main.rs:662`). Must not deadlock or double-restore. `restore_raw`'s `RAW_ACTIVE.swap` guard
(`tty.rs:51-53`) makes double-restore safe; assert that explicitly.

---

### C10 — Queue exhaustion, engine side (slow consumer)

**Induce.** The CLI is the slow consumer. Use a real engine with a fake core that stops reading:

```bash
python3 artifacts/swarm/chaos/stall_core.py \
    --engine apps/engine \
    --url 'https://example.com/repaint-60fps' \
    --stall-after 2 --stall-for 30
```

`stall_core.py` binds the socket, spawns the real Electron, reads normally for 2 s, then stops
reading for 30 s while the page repaints at 60 fps.

**Expected.** The engine's coalescing backpressure (`apps/engine/src/main.js:42-99`) holds at
**exactly one** pending frame: `sock.write` returns `false`, `writeInFlight` latches, and
subsequent paints overwrite `pendingFrame` while incrementing `stats.coalesced`
(`main.js:96-97`). Memory must stay flat; the newest frame wins, which is the correct trade for
interactivity.

**Pass criterion.**
- Engine RSS growth over the 30 s stall < 50 MiB, and it returns to within 10% of the pre-stall
  baseline within 10 s of resuming.
- `stats.coalesced` climbs to roughly `60 × 30 = 1800`; `stats.sent` stays nearly flat during the
  stall.
- On resume, the very first frame delivered has a `seq` close to `stats.produced` — proving the
  newest frame won rather than a stale queued one.
- No engine crash; socket stays open.

✅ Expected to pass — this is the best-engineered part of the system and the test exists to keep
it that way.

**Harness note.** `stats` is retrievable via `{"t":"stats"}` (`main.js:255-257`), but the CLI
**never sends it** — grep confirms only `reload`, `back`, `forward`, `quit`, and `input` are ever
emitted. The fake core sends it directly. Worth noting for the commander: `stats` is currently
unreachable in production, so this useful diagnostic is dead code from the user's perspective.

---

### C11 — Queue exhaustion, terminal side (slow tty)

**Induce.** Constrain the pty drain rate to simulate a slow SSH link:

```bash
python3 artifacts/swarm/chaos/pty_runner.py \
    --drain-bytes-per-sec 50000 \
    --cmd 'target/release/terminal-fenster open https://example.com/repaint-60fps' \
    --run-for 60 --then-send-ctrl-q
```

At 50 KB/s against measured 53,999-byte frames, the terminal absorbs roughly one frame per
second while the engine produces 60.

**Expected (correct).** Backpressure propagates cleanly: the CLI blocks on stdout, stops reading
the socket, the engine's socket buffer fills, the engine coalesces. Frame *rate* collapses;
nothing grows without bound; **the session stays responsive to `ctrl+q`**.

**Actual today.** The first three hold — the design is sound. The last does not: `write_all`
(`main.rs:900`) blocks with no interrupt, so `ctrl+q` is not read until the write completes —
**D2b**.

**Pass criterion.**
- CLI RSS flat (< 20 MiB growth over 60 s).
- Engine RSS flat.
- **`ctrl+q` takes effect within 250 ms.** `xfail` today; measured latency will be roughly one
  frame-drain time (~1 s at 50 KB/s, far worse on a genuinely slow link).
- `TERM-OK` holds after exit.

This experiment is the empirical link between D2b and D1: it demonstrates the unresponsiveness
that drives a user to `kill -9`.

---

### C12 — Signal matrix

**Induce.** For each signal, one clean run interrupted at t=3 s:

```bash
for SIG in INT TERM HUP QUIT USR1 PIPE TSTP STOP KILL; do
  python3 artifacts/swarm/chaos/pty_runner.py \
      --cmd 'target/release/terminal-fenster open https://example.com' \
      --signal-at 3 --signal "$SIG" --expect-term-ok auto
done
```

**Expected and pass criteria.**

| Signal | Expected | `TERM-OK` |
|---|---|:--:|
| `INT`, `TERM`, `HUP`, `QUIT` | handler restores, re-raises with `SIG_DFL`; exit status 128+N | ✅ must pass |
| `PIPE` | ignored (`tty.rs:137`); session continues | ✅ must pass |
| `USR1` | default disposition terminates; **not** handled | ❌ documents the gap — either handle or accept |
| `TSTP` | *should* restore, stop, and re-init on `CONT` | ❌ **`xfail` — D2** |
| `STOP` → `CONT` | *should* re-init on `CONT` | ❌ **`xfail` — D2** |
| `KILL` | unrecoverable in-process | ❌ **`xfail` — D1**; asserts `terminal-fenster reset` repairs it once it exists |

The `KILL` row is the most important single assertion in this document. Today it fails, and there
is no command that makes it pass. That is the gap D1 describes.

Additional assertion for `INT`/`TERM`/`HUP`/`QUIT`: exit status must be `128+N`, not 0 or 1. The
handler is deliberately written to preserve this (`tty.rs:70-73`) because shells and supervisors
depend on it, and a future refactor to a plain `exit(0)` would silently break process supervision.

---

### C13 — Startup and handshake chaos

Failure before `TtyGuard` exists, or between acquire and the first frame, is a distinct window.

| Case | Induce | Expected | Criterion |
|---|---|---|---|
| Engine binary missing | `TERMINAL_FENSTER_ENGINE=/nonexistent` | clean error, exit 1 | terminal never entered raw mode; message visible on the main screen ✅ (`main.rs:364-369`, `278-282`) |
| `src/main.js` missing | fake dir without `src/main.js` | clean error (`main.rs:380-385`) | ✅ |
| Engine never connects | fake engine that sleeps forever | 30 s timeout then clean error (`main.rs:415-425`) | `TERM-OK`; **but 30 s of a raw-mode terminal with no feedback** — recommend a progress indicator and a shorter default |
| Engine connects then exits immediately | fake engine: connect, `exit(0)` | EOF handled | `TERM-OK` ✅ |
| Terminal never answers capability queries | fake pty that echoes nothing | `caps::detect` deadline expires (300 ms, `main.rs:118`/`241`); `Unicode` backend chosen | `TERM-OK`; correct fallback ✅ |
| Terminal reports no pixel size | pty with `xpixel=ypixel=0`, no `CSI 14t` reply | `viewport_px()` → `None` → clean error with `drop(guard)` **before** print (`main.rs:246-250`) | ✅ — correct ordering here, unlike D9 |
| Two sessions, same terminal | launch two concurrently | both use `PAGE_IMAGE_ID = 1000` (`kitty.rs:30`) and will fight over the image slot; on exit the first to leave issues `d=A`, **deleting the other's image** | ❌ documents a real conflict; recommend a per-session image id |

The last row is worth the commander's attention: `RESTORE_SEQ` uses `\x1b_Ga=d,d=A` — delete
*all* images (`tty.rs:36`). That is the right call for a single session (it guarantees nothing
lingers over the shell) but it is destructive to any concurrent user of the terminal's image
store, including a second Terminal-Fenster or an unrelated image viewer in another tmux pane.

---

### C14 — Multiplexer and remote paths

Not strictly chaos, but they change the restoration story and belong in the same suite.

| Case | Induce | Criterion |
|---|---|---|
| tmux without passthrough | `tmux new -d; tmux send-keys 'terminal-fenster open …'` | detected (`mux_label`, `main.rs:189-197`) and either wrapped via `kitty::wrap_tmux` (`kitty.rs:241-250`) or degraded to Unicode — **never** raw graphics into a tmux that will truncate the DCS |
| tmux pane killed mid-session | `tmux kill-pane` | `SIGHUP` → handler → restore; no orphan engine |
| SSH connection dropped | `ss`/`pkill` the ssh client | `SIGHUP` → restore; engine exits on socket close |
| SSH with high latency | `dnctl`/`pfctl` 300 ms delay | same class as C11; assert responsiveness |

**Short-write caveat for the remote path.** `restore_raw` calls `libc::write` once and **discards
the return value** (`tty.rs:56-60`). `RESTORE_SEQ` is ~80 bytes, so a short write is unlikely on a
local pty, but on a congested link with a full tty output queue a partial write is possible, and a
partially applied restore sequence is a partially broken terminal. `write` is async-signal-safe;
a bounded retry loop on `EINTR`/short-count is also async-signal-safe. Low probability, cheap fix,
and the consequence is exactly the failure this module exists to prevent.

---

## 5. The 8-hour soak

### 5.1 Why 8 hours

At the measured 60 fps (p50 frame gap 16.65 ms, p99 19.94 ms), 8 hours is **1,728,000 frames**.
That is the amplification factor: a leak of one byte per frame is 1.7 MB; one kilobyte per frame
is 1.7 GB. Frame-proportional leaks become unmissable, which is the entire point of the duration.

Throughput context, from the verified 8,081,424-byte BGRA frame at 2482×814: the engine moves
roughly **485 MB/s** through `image.toBitmap()` and `Buffer.concat` (`main.js:86-97`), or ~14 TB
over the full soak. Allocation churn at that rate is the dominant soak risk.

### 5.2 Tiering

An 8-hour test cannot gate a PR. Three tiers:

| Tier | Duration | When | Gates |
|---|---|---|---|
| Smoke | 10 min | every PR | C01–C08, C12; RSS slope |
| Nightly | 60 min | scheduled | all of C01–C14 + 60 min soak |
| Release | 8 h | pre-release | full soak with all metrics |

The 10-minute smoke is 36,000 frames — already enough to catch a per-frame leak above ~30 KB, and
cheap enough to run on every change.

### 5.3 Workload

Rotate every 15 minutes so no single behaviour dominates:

1. Continuous repaint (drives the 60 fps path) — `apps/engine/spike/fps-matrix.js` already exists
   as a repaint driver.
2. Idle static page (must produce ~0 frames; catches spurious repaint loops).
3. Navigation churn — a new URL every 10 s (exercises `did-navigate`, title/url events, and
   `loadURL` teardown).
4. Scroll and input storm (exercises `sendInputEvent`, `enteredOnce` latching at `main.js:140`).
5. Resize cycling (once D6 is fixed; until then this is C09 and expected to do nothing).
6. Deliberate renderer kills every 30 min (interaction between C02 and long-run state).

### 5.4 Metrics

Sampled every 30 s to a CSV, plus fd/port counts every 5 min:

```bash
ps -o rss=,vsz= -p "$CLI_PID"
ps -o rss=,vsz= -p "$ENGINE_PID"
ps -o rss= -p $(pgrep -P "$ENGINE_PID")        # renderer + GPU children
lsof -p "$CLI_PID" | wc -l                     # fd leak
lsof -p "$ENGINE_PID" | wc -l
ps -o rss= -p "$TERMINAL_PID"                  # the terminal emulator itself -- see 5.5
```

macOS-specific and relevant here: Mach port exhaustion. The environment already exhibits
`bootstrap_look_up ... Permission denied` for Chromium children under the agent sandbox, which is
a Mach-namespace symptom. Sample `lsmp -p "$ENGINE_PID" | wc -l` every 5 min; a monotonically
climbing port count is a leak even when RSS looks flat.

Also record from `$TERMINAL_FENSTER_LOG`: cumulative frames, fps, `last_wire_bytes`, `last_encode_ms`,
and (once reachable) engine `stats.produced/sent/coalesced`.

### 5.5 The metric most likely to be forgotten: the terminal emulator's own memory

**This is the top soak risk and it is currently UNVERIFIED.**

`encode_rgb_frame` always emits `a=T` — transmit *and* display — with a fixed image id
`i=1000` (`kitty.rs:130-136`, `PAGE_IMAGE_ID` at `kitty.rs:30`). Reusing one id is deliberate and
correct in intent: the comment at `kitty.rs:28-29` explains ids are namespaced to avoid colliding
with other programs' images.

The open question is what a terminal does with **placements** when an image id is retransmitted.
The encoder never specifies a placement id (`p=`), and `Placement` has no field for one
(`kitty.rs:77-89`). Under the kitty graphics protocol, an `a=T` without `p=` creates a placement.
If retransmitting id 1000 frees the previous image *and its placements*, memory is flat. If
placements accumulate, the terminal emulator leaks — 1.7 million placements over the soak.

I have **not** verified which behaviour Ghostty 1.3.1 exhibits. I am not going to guess: this is
a leak in the *user's terminal emulator*, not in our process, so it would be invisible to every
other metric in §5.4 and it is exactly the kind of thing an 8-hour soak exists to find.

**Experiment (run this first, it is 10 minutes and cheap):**

```bash
GHOSTTY_PID=$(pgrep -x ghostty | head -1)
ps -o rss= -p "$GHOSTTY_PID"                    # baseline
TERMINAL_FENSTER_EXIT_AFTER_MS=600000 \
  target/release/terminal-fenster open https://example.com/repaint-60fps
ps -o rss= -p "$GHOSTTY_PID"                    # after ~36,000 frames
```

**Pass criterion.** Terminal emulator RSS growth < 100 MB over 36,000 frames, and it returns to
near baseline after the session exits and `d=A` has run.

**If it fails,** the encoder must explicitly delete or reuse a placement — add `p=` to
`Placement` and either reuse a fixed placement id or emit `delete_image` (already implemented at
`kitty.rs:211-216`) before each transmit. Both are small changes, but only if we know which is
needed, hence measuring first.

### 5.6 Pass criteria

| Metric | Criterion |
|---|---|
| CLI RSS | linear-regression slope over hours 2–8 < **2 MB/h**; final < 1.10 × the hour-1 value |
| Engine main RSS | slope < **10 MB/h**; final < 1.25 × hour-1 |
| Renderer RSS | slope < 20 MB/h (page-dependent; a leaky *page* is not our bug — hold it constant across runs so the delta is attributable) |
| **Terminal emulator RSS** | slope < **12 MB/h**; returns to within 50 MB of baseline after exit |
| CLI fd count | strictly bounded; no monotonic growth |
| Mach ports (engine) | no monotonic growth |
| fps | p50 ≥ 55 in hour 8; no degradation > 10% vs hour 1 |
| `last_encode_ms` | p99 in hour 8 within 20% of hour 1 (catches allocator fragmentation) |
| Crashes | zero unplanned; the 16 deliberate renderer kills all recovered |
| **`TERM-OK`** | **holds at the end.** Non-negotiable. |

Hour 1 rather than hour 0 is the baseline because JIT warmup, page load, and allocator growth all
settle in the first few minutes; measuring from t=0 produces a false positive slope every time.

### 5.7 Known-bounded allocations (verified by reading, not assumed)

Auditing the obvious candidates so the soak has a prior:

- `Renderer.frame_times` (`main.rs:812`) — pushed per frame, then `retain`ed to a 1 s window
  (`main.rs:841-844`). Bounded at ~60 entries. Capacity never shrinks, which is fine. **Bounded.**
- `Renderer.rgb` / `Renderer.out` — reused across frames; `clear()` keeps capacity. **Bounded** at
  one frame's worth.
- `MessageReader.buf` (`lib.rs:59`) — `drain` keeps capacity, so it stays at the high-water mark.
  Bounded in practice *only because* frames are bounded — which is exactly what D4 breaks.
- Engine `pendingFrame` (`main.js:49`) — at most one frame retained. **Bounded**, and the best
  part of the design.
- Engine `Buffer.concat([head, bitmap])` per paint (`main.js:97`) — allocates a fresh ~8 MB buffer
  every frame. Not a leak, but 485 MB/s of GC pressure. This is the most likely source of RSS
  drift and of `last_encode_ms` p99 degradation. Watch it; if it drifts, a preallocated
  double-buffer with `bitmap.copy(target, 32)` removes the churn entirely.
- Engine `seq` (`main.js:40`, `main.js:88`) — a JS Number written via `writeUInt32BE`, which
  **throws `ERR_OUT_OF_RANGE`** above 2^32-1. At 60 fps that is 828 days, so it is not a soak
  concern, but it is an uncaught throw inside the paint handler on a long-lived session. A
  one-character fix (`seq = (seq + 1) >>> 0`) closes it.

---

## 6. Harness implementation

Three small programs, all Python 3 standard library — no installs, respecting the 98%-full disk.
Suggested location `artifacts/swarm/chaos/` (F07-owned; **no core files touched**).

### `pty_runner.py` — the universal wrapper

```
--cmd STR                 command to run on the pty slave
--run-for SEC             wall-clock bound (pair with TERMINAL_FENSTER_EXIT_AFTER_MS)
--signal SIG --signal-at SEC
--drain-bytes-per-sec N   throttle the master read rate (C11)
--resize-storm N          C09
--send KEYS               inject keystrokes (e.g. ctrl+q)
--record PATH             raw output byte stream
--assert-term-ok          run the §1.1 predicate, exit non-zero on failure
```

Core: `pty.openpty()`; snapshot `tcgetattr(slave)`; spawn with the slave as stdin/stdout/stderr in
a new session; drive; on exit re-read `tcgetattr(slave)` and diff `c_lflag`/`c_oflag`; scan the
recorded stream for the `RESTORE_SEQ` tokens; `pgrep -f 'bg-socket='`; check `/tmp/terminal-fenster-*`.

Emits one JSON object per run so results aggregate mechanically —
matching `tests/e2e/input-injection-results.json`, which already establishes this convention.

### `fake_engine.py` — adversarial engine

Installed at `$TERMINAL_FENSTER_ENGINE/node_modules/.bin/electron` per §1.3. Parses `--bg-socket=`,
connects, sends a plausible `{"t":"ready"}`, then behaves per `$BG_FAKE_MODE`: `normal`,
`truncate_pixels=F`, `bogus_length=N`, `geometry=W,H`, `abort_after_frames=N`,
`never_connect`, `connect_then_exit`, `flood`, `hostile_title`. Records every command it
receives so C09 can assert on `resize`.

### `stall_core.py` — adversarial core

Binds the socket, spawns the **real** Electron (`apps/engine`), speaks the real protocol, and
stops reading on command. This is the only way to test engine-side backpressure (C10), since the
real CLI always drains.

**Sandbox note.** Chromium children fail under the agent Bash sandbox with
`bootstrap_look_up ... Permission denied`. Every experiment using the real engine (C01, C02, C03,
C10, C11, the soak) must run **outside** the sandbox. The fake-engine experiments (C04–C07, C13)
have no Chromium dependency and run anywhere — which makes them the natural PR-gate tier.

---

## 7. CI integration

```
.github/workflows/chaos.yml     # does not exist yet -- no .github/ in the tree
```

**PR gate (~3 min, no Chromium):** `cargo test --workspace` (currently **96 tests**: 12 in
`tf-proto`, 70 in `tf-term`, 14 in `apps/cli` — verified by running it), plus C04–C08, C12, C13
against the fake engine under a pty.

**Nightly (~90 min, self-hosted macOS with a real terminal):** all experiments + 60-minute soak.

**Pre-release:** full 8-hour soak.

Three static guards worth adding, each cheap and each protecting a property that is invisible
until it breaks:

1. **`panic = "unwind"` must not regress.** `grep -q 'panic = "unwind"' Cargo.toml` — flipping to
   `abort` silently converts every panic into a destroyed terminal (§2).
2. **`RESTORE_SEQ` completeness** is already unit-tested (`tty.rs:222-234`). Extend the same test
   to fail if `enable_input_protocols` gains a new `\x1b[?…h` without a matching `l`. This is
   currently enforced by a hand-maintained list; deriving it mechanically is better.
3. **No `unwrap`/`expect`/`assert` on wire-derived values** in the render path — the `assert_eq!`
   at `kitty.rs:108` is the existing violation (D8).

**Licensing.** There is **no LICENSE file in the repo root** (`ls LICENSE*` → no matches), though
`Cargo.toml` declares `license = "MIT OR Apache-2.0"`. Flagging per the third-party-reuse rule:
the harness above introduces no third-party code (Python standard library only), but the missing
`LICENSE` file is a real gap for a project declaring dual licensing.

---

## 8. Recommended fix order

Ordered by (terminal survives?) × (likelihood) ÷ (effort).

| # | Fix | Defect | Effort | Terminal-lethal |
|---|---|---|---|:--:|
| 1 | `terminal-fenster reset` — sane termios from scratch, not `SAVED_TERMIOS` | D1, D3 | S | **yes** |
| 2 | Cap message length in `tf-proto` (`MAX_MESSAGE_LEN`) | D4 | S | **yes** |
| 3 | `SIGTSTP`/`SIGCONT` handlers | D2 | S | **yes** |
| 4 | Non-blocking / chunked stdout so `ctrl+q` always lands | D2b | M | indirectly |
| 5 | `SIGWINCH` + resize plumbing (incl. `PointerMap`) | D6 | M | no |
| 6 | `checked_mul` in `expected_payload`; drop instead of `assert_eq!` | D8 | S | no |
| 7 | Bounds-check `bgra_rect_to_rgb` before C08 damage encoding lands | D8-rel | S | no |
| 8 | Print teardown diagnostics after leaving the alt screen | D9 | S | no |
| 9 | Render `Status::crashed`; offer recovery | D7 | S | no |
| 10 | Engine parent-liveness self-exit; `reset` reaps orphans | D5 | M | no |
| 11 | Per-session kitty image id; avoid global `d=A` when sharing a terminal | C13 | S | no |
| 12 | Retry short writes in `restore_raw` | C14 | S | marginal |

Items 1–3 are each a small, self-contained change, and together they close every
terminal-destroying path in this document except the genuinely unavoidable one (a `SIGKILL` with
no subsequent repair invocation — which item 1 makes recoverable).

---

## 9. What I could not verify

Stated plainly rather than guessed, per the evidence rule.

- **UNVERIFIED — kitty placement accumulation.** Whether Ghostty 1.3.1 frees prior placements when
  image id 1000 is retransmitted with `a=T` and no `p=`. This is the top soak risk (§5.5). The
  10-minute experiment in §5.5 resolves it; run it before the 8-hour soak.
- **UNVERIFIED — no experiment in this document has been executed.** This is a design deliverable.
  Every defect D1–D9 is derived from reading the code and is cited to `file:line`; every *runtime*
  claim is labelled expected, not measured. The one exception: the `expected_payload()` overflow
  arithmetic in D8, which I computed directly, and the 96-test count, which I obtained by running
  `cargo test --workspace`.
- **UNVERIFIED — iTerm2 3.6.9.** Automation blocked by macOS TCC, consistent with the project
  brief. All iTerm2-specific restoration behaviour (particularly its `1016` reporting quirk noted
  at `caps.rs:38-43`) remains untested.
- **UNVERIFIED — Apple Terminal restoration.** Apple Terminal 465 has no kitty graphics, so the
  Unicode half-block path applies. The `\x1b_Ga=d,d=A` token in `RESTORE_SEQ` will be emitted to a
  terminal that does not implement it. It *should* be ignored as an unknown APC, but "should be
  ignored" is a guess until tested. Worth one explicit run, since emitting an unrecognised APC to
  a terminal that mishandles it is precisely a terminal-corrupting event.
- **Not attempted — real SSH latency injection.** `pfctl`/`dnctl` require root; the C11 pty
  throttle is a reasonable proxy but is not the same thing as a real congested link with a full
  socket buffer.
