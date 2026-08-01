# B08 — Crash Recovery & the Terminal Restoration Guarantee

**Status:** design spec + measured audit, ready to build
**Date:** 2026-07-31
**Host:** macOS 26.1, Apple M4, zsh 5.9 / bash 3.2.57 (`/bin/zsh`, `/bin/bash` as shipped)
**Code audited at:** `crates/tf-term/src/tty.rs`, `apps/cli/src/main.rs`, `apps/engine/src/main.js`, `crates/tf-proto/src/lib.rs`
**Evidence convention (matching A09/A10):** *[measured]* = produced on this machine during this session, with the program shown. *[code]* = read directly out of the repo at the cited `file:line`. *[doc]* = primary-source documented. **UNVERIFIED** = believed but not demonstrated here.

**Disk at time of writing: 6.4 GiB free of 460 GiB (99% full)** *[measured]* `df -h`. Nothing in this document requires a build larger than a few hundred KiB.

---

## 0. Executive summary

The mission asked me to verify that the terminal is always restored across four failure modes. It is not. Three of the four hold; one fails, and there are two more failure modes not on the list that also fail.

| # | Failure mode | Restored today? | Mechanism | Evidence |
|---|---|---|---|---|
| 1 | Engine crashes (`render-process-gone`) | **YES** | `TtyGuard::drop` on the way out of `cmd_open` | *[code]* `main.rs:274` |
| 2 | Our own Rust panic | **YES** | panic hook installed in `acquire`; `panic="unwind"` is pinned in the release profile precisely for this | *[code]* `tty.rs:125-129`, `Cargo.toml` |
| 3 | SIGKILL of the **engine** | **YES** | socket EOF → `run()` returns → guard drops | *[code]* `main.rs:505-507` |
| 4 | SIGKILL of the **CLI** | **NO** | uncatchable; nothing restores anything | *[measured]* §2.4 |
| 5 | SIGSEGV / SIGBUS / SIGABRT / stack overflow | **NO** | only SIGINT/TERM/HUP/QUIT are handled | *[code]* `tty.rs:132` |
| 6 | `render-process-gone` *handling* | n/a — **the event is plumbed and then dropped on the floor** | `Status.crashed` is written and never read | *[code]* `main.rs:779, 797`, only other reference is a unit test at `:961` |

Row 4 is the mission's explicit question, and the honest answer to "say what the user must do then" is worse than expected: **after a SIGKILL of the CLI, `zsh` leaves the tty with `ISIG=0` and `OPOST=0` — Ctrl-C is dead and every line of output staircases — and neither `zsh` nor `bash` ever turns off the alternate screen, mouse reporting, or the hidden cursor** *[measured]* §2.4.

But row 4 does not have to stay unfixable. §3 demonstrates, with a working prototype, a **restore nanny** that repairs the terminal **0.4–1.1 ms** after an uncatchable `SIGKILL` *[measured]*, costs **0.538 ms mean** to fork *[measured]*, and stays completely silent on a clean exit *[measured]*. The same mechanism subsumes rows 5 and 4 and the "we forgot a signal" class of bug entirely, because it does not care *how* the process died.

The rest of the document specifies the watchdog (§4), crash handling and restart (§5–6), session restoration (§7), the diagnostic bundle (§8), and a CI-able test plan that needs no screenshots (§10).

---

## 1. What the code does today

Reading the three files end to end, the crash-relevant surface is small enough to state exactly.

**Terminal ownership.** `TtyGuard::acquire(fd)` saves `termios` into a process-global `static mut`, applies a hand-spelled `cfmakeraw`, sets `RAW_ACTIVE`, installs a panic hook, and installs one `signal()` handler for `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`, plus `SIG_IGN` for `SIGPIPE` *[code]* `tty.rs:84-140`. `restore_raw()` is idempotent via an `AtomicBool::swap` and uses only `write(2)` and `tcsetattr(2)`, both async-signal-safe *[code]* `tty.rs:50-65`. The `RESTORE_SEQ` constant undoes ten modes and deletes all kitty images, with the alternate-screen exit deliberately last *[code]* `tty.rs:27-40`. There is a unit test asserting every enable has a matching disable *[code]* `tty.rs:223-234`. **This is good work and I am not proposing to redesign it.** The gaps below are things it does not attempt, not things it gets wrong.

**Engine lifetime.** `Session::start` binds a `0600` socket inside a `0700` dir, spawns Electron, and waits up to 30 s for a connect *[code]* `main.rs:373-414`. `Session::shutdown` sends `{"t":"quit"}`, then polls `try_wait` for up to **1500 ms** before `kill()` *[code]* `main.rs:643-664`. On the engine side, `sock.on('close', () => app.exit(0))` means a dead CLI takes the engine with it *[code]* `main.js:288`.

**Crash signal.** The engine emits exactly one crash event, from `render-process-gone` *[code]* `main.js:125-127`. The CLI parses it into `Status.crashed` *[code]* `main.rs:795-798`. Nothing reads that field. `grep -rn "crashed" apps/cli/src crates/` returns three hits: the declaration, the write, and a unit test *[measured]*.

There is no heartbeat, no watchdog, no restart, no `unresponsive` handler, and no `child-process-gone` handler: `grep -n "unresponsive\|isCrashed\|child-process-gone\|heartbeat\|ping\|watchdog" apps/engine/src/main.js` matches nothing *[measured]*.

---

## 2. Audit of the restoration guarantee

### 2.1 Engine crash — holds, but the user is told nothing

When the renderer dies, `main.js` sends `{"t":"crash",...}`; the engine host survives, so the socket stays open and frames simply stop. The CLI records the reason and keeps looping. The terminal is fine *when the user eventually quits*, but until then they are staring at a frozen last frame with no explanation.

Worse, the status bar cannot even report the freeze. `Renderer::present` begins `if !self.dirty || self.rgb.is_empty() { return; }`, and `dirty` is set only in `on_frame` *[code]* `main.rs:770-791`. **No frames means no repaint, which means the fps counter freezes at its last healthy value rather than decaying to zero.** Any UI that is supposed to tell the user "the page died" is, today, structurally unable to draw itself. Fixing this is a prerequisite for everything in §4–6, and it is a two-line change (§11, C4).

### 2.2 Panic — holds

`panic = "unwind"` is pinned in `[profile.release]` with the comment "we MUST run our terminal-restore panic hook; abort would corrupt the tty" *[code]* `Cargo.toml`. The hook restores *before* delegating to the previous hook, which is the right order: with `OPOST` off, a backtrace printed first renders as an unreadable staircase *[code]* `tty.rs:123-129`.

Two residual holes. A **double panic** aborts without running hooks, and a **stack overflow** is caught by Rust's own `SIGSEGV` handler which prints and calls `rtabort!` — user panic hooks do not run *[doc]*. Both land on §2.5.

### 2.3 SIGKILL of the engine — holds

`read()` returns `Ok(0)`, the CLI calls `eprint_restore("engine exited")` and returns 1; `cmd_open` then runs `shutdown()` and drops the guard *[code]* `main.rs:504-507, 272-275`. The terminal is restored.

Two defects on this path, both worth fixing:

**The error message is invisible.** `eprint_restore` writes to stderr while the process is still inside the alternate screen, and `RESTORE_SEQ` ends with `\x1b[?1049l` *[code]* `tty.rs:38`, which discards the alternate screen buffer and restores the primary one *[doc]* xterm `ctlseqs`, private mode 1049. The message is written and then thrown away. The user sees the browser vanish with no explanation at all. Any diagnostic text must be emitted **after** the guard drops.

**Teardown blocks for up to 1.5 s with the terminal still wrecked.** `cmd_open` calls `session.shutdown()` *before* `drop(guard)` *[code]* `main.rs:273-274`, and `shutdown` will spin for 1500 ms waiting on a `try_wait` that will never succeed if the engine is wedged. The user presses ctrl+q and stares at a frozen page for a second and a half. Restoring the terminal first costs nothing and is strictly safer — the CLI is the only writer to the tty.

### 2.4 SIGKILL of the CLI — **does not hold**, and this is what the user is left with

This is the mission's pointed question, so I measured it rather than reasoned about it. Harness: `scratchpad/fgtest.py` — a real interactive shell on a `pty`, running a helper (`wreck.c`) that takes `/dev/tty` into raw mode and enters the alternate screen exactly the way `enable_input_protocols` does *[code]* `tty.rs:148-172`, as a **foreground job**. The helper is then `SIGKILL`ed so it can restore nothing, and the shell regains control.

```
shell: /bin/zsh
  shell at prompt        : ECHO=0 ICANON=0 ISIG=1 OPOST=1 ICRNL=1
  while terminal-fenster runs  : ECHO=0 ICANON=0 ISIG=0 OPOST=0 ICRNL=0
  SIGKILLed job pid=21425
  after job killed       : ECHO=0 ICANON=0 ISIG=0 OPOST=0 ICRNL=1
  termios repaired by shell : NO
  blind command executed    : YES
  alt screen exited         : NO <-- still in alt screen
  mouse reporting off       : NO <-- still spraying on mouse move
  cursor shown again        : NO <-- cursor still hidden

shell: /bin/bash
  after job killed       : ECHO=0 ICANON=0 ISIG=1 OPOST=1 ICRNL=0
  termios repaired by shell : NO
  blind command executed    : YES
  alt screen exited         : NO <-- still in alt screen
  ...
```
*[measured]*

Read carefully, because the naive reading is wrong in both directions:

- `ECHO=0 ICANON=0` is **not** damage. Both shells sit at their prompt with echo and canonical mode off, because `zle`/`readline` do their own echoing and line editing. The baseline row proves it. A test that asserts `ECHO=1` after recovery is testing the wrong thing — I wrote that test first and it gave a false failure.
- The **real** termios damage under `zsh` is `ISIG=0` and `OPOST=0`: **Ctrl-C no longer interrupts anything, and every newline is a bare LF so all output marches diagonally down the screen.** `bash` restores both; `zsh` restores neither. So the severity of a Terminal-Fenster SIGKILL depends on the user's shell, and the more common one is the worse one.
- **Neither shell ever restores the escape-sequence state.** Alternate screen, mouse reporting, hidden cursor, and any lingering kitty image are not `termios` — no shell will ever touch them, on any platform, ever. This is the durable damage and it is 100% reproducible.
- The one piece of good news: **blind commands do still execute** in both shells *[measured]*. The user is not locked out; they are flying blind.

**So what must the user do?** The recovery incantation below was measured to restore both shells to a state **byte-identical to their own at-prompt baseline**, and to emit all three visual resets:

```sh
printf '\033[<u\033[?1006l\033[?1016l\033[?1003l\033[?1002l\033[?1000l\033[?1004l\033[?2004l\033_Ga=d,d=A\033\\\033[?25h\033[?1049l'; stty sane
```

```
  -- after blind recovery command --
  termios                   : ECHO=0 ICANON=0 ISIG=1 OPOST=1 ICRNL=1   (zsh)
  matches at-prompt baseline: YES
  alt screen exited         : YES
  mouse reporting off       : YES
  cursor shown again        : YES
  RECOVERED                 : YES
```
*[measured]*, both `/bin/zsh` and `/bin/bash`

Note that plain `reset` — the folklore answer — is *worse* than this: it does not pop the kitty keyboard stack and does not delete kitty images, so a stale page bitmap can remain painted over the user's prompt. `stty sane` alone fixes none of the visual state.

**Therefore we must ship this as a command.** `terminal-fenster reset` should write `tty::RESTORE_SEQ` and apply a sane termios, so the documented recovery is six characters the user can type blind instead of a 130-character escape soup they must copy from a README they cannot currently see. It reuses the existing constant, so it cannot drift from the real restore path.

But shipping `terminal-fenster reset` is the *fallback*. §3 removes the need for it in the common case.

### 2.5 The failure modes that were not on the list

**Fatal signals are unhandled.** `tty.rs:132` installs handlers for four signals. `SIGSEGV`, `SIGBUS`, `SIGILL`, `SIGFPE`, `SIGABRT` are not among them *[measured]* `grep`. This is not hypothetical for this codebase specifically: `tty.rs` maintains `static mut SAVED_TERMIOS` and `static mut GUARD_FD` and dereferences a raw pointer to them *[code]* `tty.rs:43-44, 61-63`; `main.rs` calls `libc::read` into a raw buffer pointer *[code]* `main.rs:472-478`. A bug in exactly the module responsible for restoration is the one that leaves the terminal wrecked.

Adding those signals to the handler list is the obvious fix, but it has a real cost worth stating: installing our own `SIGSEGV` handler with `signal()` **replaces Rust's stack-overflow guard page handler**, so genuine stack overflows stop reporting "thread has overflowed its stack" and start reporting a bare segfault *[doc]*. Doing it properly means `sigaction` with `SA_ONSTACK`, saving the previous `struct sigaction`, and chaining to it after restoring. That is fiddly, easy to get subtly wrong, and it still leaves `SIGKILL`.

**`std::process::exit` bypasses `Drop`.** It is called at `main.rs:71`, safely — every caller drops the guard first *[code]* `main.rs:119, 241, 268, 274`. This is currently correct and is a standing invariant worth a comment and a review rule, because one future `process::exit` inside `cmd_open` silently breaks the guarantee with no test failure.

**Restore/re-raise is not fully reentrant.** If `SIGINT` arrives while the main thread is partway through `restore_raw()` from `Drop`, the handler's `RAW_ACTIVE.swap` returns `false`, the handler returns immediately without restoring, and then re-raises `SIGDFL` — killing the process midway through the `RESTORE_SEQ` `write`. The window is a single `write(2)` of 86 bytes so this is very unlikely, but the outcome is a half-emitted escape sequence, which is exactly the failure the module exists to prevent. Blocking the four handled signals with `pthread_sigmask` around the `Drop` path closes it.

---

## 3. The restore nanny — measured

Every failure in §2.4 and §2.5 has the same shape: *the process is gone and cannot run code*. Chasing them signal by signal is a losing game, because `SIGKILL` is never catchable and the list of fatal signals is not the list of ways a process dies.

Put the restoration somewhere the death cannot reach: **a second process**.

### 3.1 Mechanism

At `TtyGuard::acquire` time, `pipe()` then `fork()`. The child inherits the tty fd and the read end.

1. Child calls `setsid()` so a process-group-wide kill (`kill -9 -PGID`) cannot take it along.
2. Child sets `SIGTTOU`/`SIGTTIN`/`SIGHUP` to `SIG_IGN` — after `setsid()` it is in a different session, so a `tcsetattr` on a terminal it does not control must not be allowed to stop it.
3. Child blocks in `read()` on the pipe. A blocked `read` is the cheapest possible wait.
4. On clean shutdown the parent writes one byte `'K'` and closes. Child sees `'K'` at EOF and exits silently — the parent already restored.
5. On **any** death — `SIGKILL`, `SIGSEGV`, abort, panic-abort, OOM killer — the kernel closes the write end, `read()` returns 0 with no `'K'`, and the child writes `RESTORE_SEQ` and `tcsetattr`s the saved termios.

The child should also hold the engine pid and `kill()` its process group, which closes a gap the current design has: `main.js:288` only reaps the engine if the engine's event loop is *running*. A wedged engine host outlives a SIGKILLed CLI as an orphan Chromium tree.

### 3.2 Proof

`scratchpad/nanny2.c` runs three scenarios against a `pty`, with a second slave fd held open for the whole test to model the user's shell — without that the pty resets its line discipline when the last fd closes, which flatters the control case and is not how a real terminal behaves. (I made exactly that mistake on the first pass; the corrected control is below.)

```
mode=control  killed=SIGKILL  bytes_after=  0  termios_restored=0  alt_screen_off=0
         ^ user is stranded: raw mode + alt screen + mouse still on
mode=control  killed=SIGKILL  bytes_after=  0  termios_restored=0  alt_screen_off=0
mode=control  killed=SIGKILL  bytes_after=  0  termios_restored=0  alt_screen_off=0
mode=nanny    killed=SIGKILL  bytes_after= 86  termios_restored=1  alt_screen_off=1  restore_latency=0.6ms
mode=nanny    killed=SIGKILL  bytes_after= 86  termios_restored=1  alt_screen_off=1  restore_latency=0.4ms
mode=nanny    killed=SIGKILL  bytes_after= 86  termios_restored=1  alt_screen_off=1  restore_latency=0.8ms
mode=clean    killed=no  bytes_after=  0  termios_restored=1  alt_screen_off=0
mode=clean    killed=no  bytes_after=  0  termios_restored=1  alt_screen_off=0
mode=clean    killed=no  bytes_after=  0  termios_restored=1  alt_screen_off=0
```
*[measured]*

And the full byte trace from the first prototype (`scratchpad/nanny.c`), showing the exact `RESTORE_SEQ` reaching the terminal *after* an uncatchable kill:

```
--> SIGKILL 18182 (uncatchable, no cleanup possible in B)
B reaped: signaled=1 sig=9
master saw (124 bytes): ESC[?1049hESC[?25lESC[?1003hESC[?1006hESC[?1016h
  ESC[<uESC[?1006lESC[?1016lESC[?1003lESC[?1002lESC[?1000lESC[?1004lESC[?2004l
  ESC_Ga=d,d=AESC\ESC[?25hESC[?1049l
RESULT termios_restored=1 alt_screen_off=1 cursor_on=1 images_deleted=1 mouse_off=1
VERDICT: NANNY SURVIVES SIGKILL -- terminal fully restored
```
*[measured]*

| Property | Value | Source |
|---|---|---|
| Restore latency after SIGKILL | **0.4 – 1.1 ms** (n=8) | *[measured]* |
| `fork()` + `pipe()` cost | **0.538 ms mean, 0.219 ms min** (n=200) | *[measured]* `forkcost.c` |
| Cost relative to startup | engine ready at 212 ms, first frame at 366 ms → **< 0.3% of startup** | prior measurement + *[measured]* |
| False fires on clean exit | **0 of 3** (`bytes_after=0`) | *[measured]* |
| Extra RSS | a `fork` blocked in `read()`, copy-on-write, no allocations after fork | *[code]* by construction |

### 3.3 What it does and does not cover

Covers: `SIGKILL` of the CLI, `SIGSEGV`/`SIGBUS`/`SIGABRT`/`SIGILL`, double panic, panic-abort, OOM killer, and any fatal signal we forget to add to the list. It makes §2.5's `sigaction` chaining work optional rather than load-bearing.

Does not cover: `kill -9` aimed at the whole process group *if* `setsid()` failed; the terminal emulator itself being killed (nothing can help, and nothing needs to); a wedged nanny (it does nothing but block in `read`, so there is nothing to wedge). If the tty has gone away the nanny's `write` returns `EIO` and it must ignore the error and exit.

One caveat to state honestly: the prototypes are **C**, not Rust, and they model `TtyGuard`'s exact syscall sequence rather than calling it. What is proven is the **OS mechanism** — fd inheritance across `fork`, pipe-EOF delivery after `SIGKILL`, and `write`/`tcsetattr` on an inherited tty fd from a different session. Porting that to `tf-term` is mechanical, but it is not yet done and must not be reported as done. `fork()` in a Rust process is only async-signal-safe in the child until `exec`; the child here must therefore touch nothing but `read`, `write`, `tcsetattr`, `kill`, `setsid`, `signal`, `_exit` — no allocation, no `String`, no `println!`. Pre-format everything before the fork.

---

## 4. Watchdog

### 4.1 The mistake to avoid

The intuitive watchdog is "no frame for N seconds ⇒ hung". **It is wrong, and it would false-fire on almost every page.** Chromium's offscreen `paint` event is damage-driven *[code]* `main.js:116`; a static article emits zero frames for as long as the user reads it. The CLI already encodes this assumption — `present()` skips work when `!dirty` *[code]* `main.rs:791`. Frame arrival measures *page activity*, not *engine liveness*, and the two are unrelated.

Liveness must be **actively probed**.

### 4.2 Three levels

| Level | Detects | Probe | Threshold | Engine change needed? |
|---|---|---|---|---|
| **L0** process | engine exited / SIGKILLed / orphaned | socket EOF (`read` → `Ok(0)`) + `child.try_wait()` each loop pass | immediate | none — EOF already handled at `main.rs:505` |
| **L1** host | wedged browser-process event loop (sync native call, GPU deadlock) | request/reply on the control channel | 3 missed @ 1 Hz = **3 s** | **none today** — see below |
| **L2** renderer | infinite JS loop, wedged compositor | `unresponsive`/`responsive`, optionally an `executeJavaScript` round-trip | Chromium's own hang timer | yes, small |

**L1 is implementable today with zero engine changes.** `handleCommand` already has `case 'stats': sendEvent({ t: 'stats', ...stats })` *[code]* `main.js:236-238`. That is a complete request/reply pair sitting unused. Send `{"t":"stats"}` on a 1 s timer, treat the returning `stats` event as a pong, and you have host liveness plus free frame-accounting telemetry (`produced`/`sent`/`coalesced`, already tracked at `main.js:50`) with which to distinguish "engine is fine, page is idle" from "engine is producing frames we are failing to draw". A dedicated `{"t":"ping","id":N}` → `{"t":"pong","id":N}` with sequence matching is cleaner and should replace it, but the capability exists now and the watchdog need not wait on an engine PR.

**L2 needs four lines in `main.js`** (described, not written, per the ownership rule): `win.on('unresponsive')` and `win.on('responsive')` forwarding to `sendEvent({t:'unresponsive'})` / `{t:'responsive'}`, plus `app.on('child-process-gone', ...)` so a dead GPU or utility process is reported rather than manifesting as mysterious frame loss. Chromium's hang monitor drives these, so it is strictly better information than anything we can synthesise. **UNVERIFIED on this machine** that `unresponsive` fires for an `offscreen: true` window — it is documented on `BrowserWindow` generally *[doc]*, and the hang monitor lives in the browser process, but offscreen rendering is a lightly-trodden path and this needs a live check before it is relied on. The L1 probe does not depend on it.

### 4.3 State machine

```
                 pong ok
      ┌──────────────────────────────┐
      v                              │
  [Healthy] ──miss 1──> [Suspect] ──miss 3──> [Hung] ──> [Restarting]
      ^                     │                                  │
      │                  pong ok                          ready/timeout
      └──────────────────────────────────────────────────────┘
                                                               │
                                  3 restarts in 60 s, or same URL 3x
                                                               v
                                                          [Failed] -> banner, about:blank
```

`Suspect` must be visible: the status bar shows a `?` after one missed pong and `HUNG` after three. That is only possible once the status bar repaints on a timer (§2.1), which is why C4 in §11 is a prerequisite and not a nicety.

Two thresholds worth arguing about, so I will state the reasoning rather than assert numbers. **1 Hz probe / 3 misses** puts detection at ~3 s. A 60 fps compositor means a wedge is perceptible within ~200 ms, so 3 s feels slow; but a 3 s threshold is comfortably clear of GC pauses, a heavy layout, and SSH round-trips on a bad link (A07 territory), and the cost of a false restart is losing the user's page. Prefer late detection over spurious restarts. Over SSH the threshold should scale with measured RTT.

---

## 5. Crash handling

`{"t":"crash","reason":...,"exitCode":...}` already arrives *[code]* `main.js:125-127`. `reason` is Chromium's `render-process-gone` reason string; the values that matter are `crashed`, `oom`, `killed`, `launch-failed`, `integrity-failure`, and `abnormal-exit` *[doc]*. They demand different responses:

| `reason` | Meaning | Response |
|---|---|---|
| `oom` | renderer exceeded memory | Restart, but **do not reload the same URL a second time** — it will OOM again. Land on `about:blank` with the URL shown. |
| `crashed` / `abnormal-exit` | renderer bug | Restart with full session restore. Usually transient. |
| `killed` | external `SIGKILL` | Restart once; if repeated, something on the machine is killing us (memory pressure, security tooling) — say so and stop. |
| `launch-failed` | renderer never started | Do **not** loop. This is environmental (sandbox denial, missing binary) and retrying cannot fix it. Surface immediately, with the engine path, and offer the diagnostic bundle. |
| `integrity-failure` | code integrity check failed | Do not restart. Report. |

`launch-failed` deserves emphasis in this environment specifically: the brief records that Chromium child processes fail under the agent Bash sandbox with `bootstrap_look_up ... Permission denied`. That is precisely a `launch-failed`, and a naive restart loop would spin forever producing nothing. **Restart policy must be reason-aware, not uniform.**

The user-visible treatment on crash: keep the last good frame on screen — `Renderer.rgb` still holds it *[code]* `main.rs:751` — and overlay a one-line banner rather than clearing. Clearing the screen destroys the user's context and makes the failure feel worse than it is. The banner text must be run through `unicode::sanitize_for_terminal`, already used for titles at `main.rs:829`, because `reason` and the URL in the banner are attacker-influenced (A09 §1).

---

## 6. Safe restart

Sequence, in this order:

1. **Freeze and annotate.** Repaint the last good frame with the banner. Do not clear.
2. **Kill hard, kill the group.** `kill(-pgid, SIGKILL)`. `Child::kill` sends `SIGKILL` to one pid; Chromium is a process *tree*. This requires `setsid()` in the spawned engine (a `pre_exec` on the `Command`) so the engine gets its own process group — otherwise `-pgid` would signal the CLI too. Then `waitpid` to reap; without it the zombie stays until CLI exit.
3. **Tear down the socket.** `remove_file` + `remove_dir`, then mint a fresh `0700` dir. Reusing the path risks binding onto a socket the dead engine still holds.
4. **Respawn** with the last-known-good URL and current geometry.
5. **Bounded wait for `ready`.** The current 30 s accept timeout *[code]* `main.rs:399` is right for cold start but far too long for a restart, where the binary is warm in page cache; 5 s is generous. Exceeding it means `Failed`.
6. **Replay session state** (§7) once `ready` arrives.
7. **Clear the banner** on the first new frame.

**Crash-loop protection is not optional.** A page that reliably OOMs plus an unconditional restart is an infinite loop that also sprays the terminal with a full-screen image on every iteration. Token bucket: **max 3 restarts per 60 s**, backoff **250 ms → 1 s → 4 s**. Track `(url, reason)`; if the same URL produces the same fatal reason twice, restart to `about:blank` instead of the URL. On bucket exhaustion enter `Failed`: banner, `about:blank`, diagnostic bundle offered, terminal still fully functional and still restorable by ctrl+q.

---

## 7. Session restoration

What is worth restoring, what it costs, and what must **not** be restored.

| State | Where it lives now | Cost to preserve | Restore |
|---|---|---|---|
| Current URL | already in `Status.url` from `did-navigate` *[code]* `main.rs:731-735` | **free** | `--bg-url=` on respawn |
| Page title | already in `Status.title` | free | cosmetic, redraw the bar |
| Viewport geometry | already computed in `cmd_open` *[code]* `main.rs:238-249` | free | respawn args |
| Nav history | not captured | needs an engine command | `navigationHistory.getAllEntries()` / `.restore({index, entries})`, Electron ≥ 35 *[doc]*; we run 43. **UNVERIFIED** here |
| Scroll offset | not captured | one `executeJavaScript` per sample | **must be sampled before the crash** — a dead renderer cannot be asked. 2 Hz is enough; Chromium also restores scroll on history navigation, so this may be redundant once history restore works |
| Cookies / localStorage | Electron session on disk | free | survives automatically **iff** `userData` is stable across restarts. Verify this — a changing `userData` path means every crash silently logs the user out of everything |
| Zoom | not captured | free to track in the CLI | set after `ready` |
| **Form field contents** | renderer only | — | **do not restore.** Requires DOM access we would have to poll continuously, and it is exactly the data a user least wants written anywhere (§8) |
| **POST bodies** | — | — | **never replay.** Silently re-POSTing after a crash can double-submit a payment or an order. If the top history entry is a POST, land on it without re-submitting and say so |

The last two rows are the ones to hold the line on. "Restore everything" sounds like better UX right up until a crash restart charges someone twice.

---

## 8. Diagnostic bundle

**The bundle is itself an attack surface, and that shapes its design.** Users `cat` diagnostic bundles, and the bundle is full of attacker-controlled strings — page titles, URLs, crash reasons, console text. A09 §1 establishes that a title can carry `OSC 52` and poison the clipboard. A bundle that faithfully records raw bytes is a stored-escape-injection vector that fires later, in a shell, outside Terminal-Fenster, after the browser is closed. **Every string must be sanitised at write time, not at display time** — reuse `unicode::sanitize_for_terminal` *[code]* `main.rs:829`, and additionally assert no `0x1B` survives (§10 T8).

**Contents** (`$TMPDIR/terminal-fenster-diag-<unix_ms>/`, then `tar.gz`):

- `env.json` — Terminal-Fenster version, Electron/Chromium versions (already reported in the `ready` event *[code]* `main.js:273-279`), OS build, arch, `TERM`, `TERM_PROGRAM`+version, tmux/screen, ssh, shell.
- `capabilities.json` — the full `caps::Capabilities`, including `raw_replies`, which is already collected and already escaped for display *[code]* `caps.rs:233` `escape_for_display`. This is the single highest-value artefact for terminal bugs and it costs nothing; it is what makes A04's matrix reproducible on a user's machine.
- `timeline.jsonl` — last N protocol events with timestamps. `log_line` already writes exactly this shape to `$TERMINAL_FENSTER_LOG` *[code]* `main.rs:32-41`; make it a ring buffer in memory so a bundle is possible without the user having set the env var in advance.
- `frames.json` — frame count, fps history, wire bytes, encode ms, plus the engine's `produced`/`sent`/`coalesced` *[code]* `main.js:50`. The gap between `produced` and `sent` is the single number that separates "engine is slow" from "terminal is slow" — A10's headline finding is that the PTY write is the bottleneck, so this distinction is the first thing any performance bug report needs.
- `crash.json` — reason, exitCode, restart history with timestamps and backoff state, watchdog state transitions.
- `engine-sample.txt` — **for hangs only**: `sample <pid> 3 -f`. A10 verified `sample`/`spindump` work headless with no sudo on own-uid targets *[measured, A10]*. A stack of the wedged process is worth more than everything else combined.
- `lastframe.png` — **opt-in only** (`--include-page-data`). It is a picture of the user's screen, potentially their bank.

**Redaction is the default.** URLs reduce to `scheme://host` with the path and query replaced by a truncated SHA-256; titles are omitted entirely; cookies, storage, and form values are never collected at any flag level. `--include-page-data` widens this and must print exactly what it will include before writing.

**Environment guards.** Cap the bundle at 2 MiB and refuse to write below 100 MiB free — this machine is at 6.4 GiB of 460 GiB *[measured]*, and a diagnostic tool that fills the last of a user's disk during a crash is worse than no diagnostic tool.

---

## 9. Protocol additions

For the commander; `tf-proto` and both endpoints are core-owned, so this is a description, not a patch. No new message *types* are needed — everything fits inside the existing `T_EVENT` / `T_COMMAND` JSON on the existing framing *[code]* `tf-proto/src/lib.rs:11-13`.

| Direction | Message | Purpose |
|---|---|---|
| core → engine | `{"t":"ping","id":N}` | L1 liveness (`{"t":"stats"}` works today as a stand-in) |
| engine → core | `{"t":"pong","id":N}` | reply; echo `id` so a late pong cannot mask a fresh miss |
| engine → core | `{"t":"unresponsive"}` / `{"t":"responsive"}` | L2, from `win.on(...)` |
| engine → core | `{"t":"childGone","service":...,"reason":...}` | from `app.on('child-process-gone')` |
| core → engine | `{"t":"getHistory"}` → `{"t":"history","entries":[...],"index":N}` | session capture |
| core → engine | `{"t":"restoreHistory","entries":[...],"index":N}` | session restore |
| core → engine | `{"t":"getScroll"}` → `{"t":"scroll","x":X,"y":Y}` | 2 Hz sampling |

One protocol-level hardening note while this is open: `MessageReader::feed` appends to an unbounded `Vec` *[code]* `tf-proto/src/lib.rs:72-74`, and `next_message` trusts a 32-bit length. A confused or hostile engine can request a 4 GiB allocation. A `MAX_MESSAGE_LEN` (largest plausible frame: 2482×851×4 ≈ 8.45 MB, so 32 MiB is generous) turning an oversized length into a hard error is a five-line change that converts a potential OOM into a clean, diagnosable failure. Out of scope for B08 but it belongs to the same "fail loudly, never silently" family.

---

## 10. Test plan

All of it runs headless on a `pty` and asserts on bytes and `termios` flags — no screenshots, which suits both the locked screen here and CI. The harnesses in §3 are the prototypes.

| ID | Scenario | Assertion | Status |
|---|---|---|---|
| T1 | SIGKILL the CLI, nanny present | `RESTORE_SEQ` on the master; termios == pre-launch baseline | **passing in prototype** *[measured]* |
| T2 | Clean exit, nanny present | nanny emits **0** bytes | **passing in prototype** *[measured]* |
| T3 | SIGKILL the CLI, no nanny (control) | terminal stays wrecked — pins the regression | **passing in prototype** *[measured]* |
| T4 | SIGKILL the engine | banner appears; restart; terminal restored on quit; message visible **after** guard drop | to build |
| T5 | Engine stops answering pings (stub engine that ignores `ping`) | `Hung` within 3 s ± 1 probe | to build |
| T6 | Static page, zero frames for 30 s | watchdog stays `Healthy` — **the false-positive test, the most important one here** | to build |
| T7 | Crash loop (page that OOMs deterministically) | ≤ 3 restarts / 60 s, backoff observed, ends on `about:blank`, terminal healthy | to build |
| T8 | Title/URL of `\x1b]52;c;...\x07` through a crash into a bundle | no `0x1B` anywhere in the bundle | to build |
| T9 | Panic inside `Renderer::present` | terminal restored, backtrace legible (CR-LF intact) | to build |
| T10 | `raise(SIGSEGV)` mid-frame | terminal restored (nanny, or `sigaction` chaining) | to build |

T6 deserves the emphasis. It is cheap, it will never fail by accident, and it is the one that catches the tempting-but-wrong frame-based watchdog if someone reintroduces it later.

Prototype sources, runnable as-is:
`…/scratchpad/nanny.c` (full byte trace), `nanny2.c` (three scenarios + latency), `wreck.c` + `fgtest.py` (real shells, foreground-job SIGKILL, recovery incantation), `forkcost.c`.

---

## 11. Changes required in core files

I own none of these files, so per the ownership rule these are described for the commander rather than made.

| # | File | Change | Why | Size |
|---|---|---|---|---|
| **C1** | `crates/tf-term/src/tty.rs` | Fork a restore nanny in `acquire`; `'K'` handshake on clean drop | Only thing that survives `SIGKILL`/`SIGSEGV`/abort. Mechanism proven, §3.2 | ~80 lines |
| **C2** | `apps/cli/src/main.rs:272-274` | `drop(guard)` **before** `session.shutdown()`; move `eprint_restore` text after the drop | Today teardown blocks up to 1.5 s with the tty wrecked, and the error message is written into the alt screen and discarded (§2.3) | 3 lines |
| **C3** | `apps/cli/src/main.rs` | Add `terminal-fenster reset` writing `tty::RESTORE_SEQ` + sane termios | The documented blind recovery; measured to fully restore both shells (§2.4). Reuses the constant so it cannot drift | ~15 lines |
| **C4** | `apps/cli/src/main.rs:790-791` | Repaint the status bar on a ~250 ms timer, independent of `dirty` | Without it the status bar cannot draw during a hang, so no watchdog or crash state is ever visible (§2.1). **Prerequisite for §4–6** | ~5 lines |
| **C5** | `apps/cli/src/main.rs` | Read `Status.crashed`; watchdog + restart state machine | The crash event is plumbed and then discarded (§0 row 6) | ~200 lines |
| **C6** | `apps/engine/src/main.js` | `win.on('unresponsive'/'responsive')`, `app.on('child-process-gone')`, `ping`→`pong`, history/scroll commands | L2 liveness + session capture (§4.2, §9) | ~30 lines |
| **C7** | `apps/cli/src/main.rs` | `pre_exec(setsid)` on the engine `Command`; kill by process group | `Child::kill` signals one pid; Chromium is a tree (§6 step 2) | ~10 lines |
| **C8** | `crates/tf-term/src/tty.rs` | `sigaction` + `SA_ONSTACK` chaining for SIGSEGV/BUS/ILL/FPE/ABRT | Defence in depth behind C1. **Lower priority if C1 lands** — and note it degrades Rust's stack-overflow message (§2.5) | ~40 lines |

Dependency order: **C4 → C2 → C1 → C3 → C5 → C6 → C7 → C8.** C4 first because nothing else is observable without it; C1 early because it makes C8 optional and de-risks every subsequent change by guaranteeing the terminal comes back while the crash paths are still being written.

---

## 12. Open questions and unverified claims

1. **Does `unresponsive` fire for `offscreen: true` windows?** Documented on `BrowserWindow` *[doc]*, but OSR is a lightly-trodden path. **UNVERIFIED.** L1 does not depend on it. Needs a live Electron run, which requires the sandbox disabled.
2. **`navigationHistory.restore()` on Electron 43.** Documented from Electron 35 *[doc]*; not exercised here. **UNVERIFIED.**
3. **Electron shutdown latency after `app.exit(0)`.** The 1500 ms in `shutdown` *[code]* `main.rs:648` is an unmeasured guess. Measure it and cut it hard — it is on the critical path of every quit.
4. **Is `userData` stable across restarts?** If not, every crash logs the user out of every site (§7). One-line check, high consequence.
5. **Nanny behaviour under tmux/screen.** The nanny writes to the tty fd, which is a tmux pty; graphics passthrough rules (A04) apply to images but plain mode-reset sequences should pass through. **UNVERIFIED.**
6. **Ported nanny under Rust.** §3.2 proves the OS mechanism in C. The Rust port must keep the child to async-signal-safe calls only and must be re-measured, not assumed.

---

*Prototypes and raw output for every *[measured]* claim: `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/` — `nanny.c`, `nanny2.c`, `wreck.c`, `fgtest.py`, `forkcost.c`.*
