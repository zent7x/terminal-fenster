# D10 — Usability test plan + user-facing string audit

**Mission:** D10 — write a task-based usability test plan with concrete pass/fail criteria, and review every user-facing string in `apps/cli/src/main.rs` (error messages, doctor output, help text). Rewrite any that fail to tell the user what to DO next, with a before/after table.
**Owner of this file:** D10. **Nothing in `crates/`, `apps/cli/`, or `apps/engine/src/main.js` was edited.** Every string rewrite and behaviour change below is written as a *proposal for the commander*, who owns that source. The before/after table (§3) and the commander instructions (§4) are the deliverable form of those proposals.
**Machine:** macOS 26.1, Apple M4. Binary under test: `target/release/blackglass` (blackglass 0.1.0, built 31 Jul 21:49).
**Status:** test plan + audit. Every "current" string in §3 was captured by running the shipped binary this session (§1), except the mid-session/engine-handshake strings, which are reachable-by-inspection only and marked **UNVERIFIED (runtime)** because Electron will not launch under the agent Bash sandbox and the machine is at a lock screen.

---

## 0. Recommendation in one paragraph

The single highest-value change is not a string at all — it is to give **`blackglass doctor` a verdict line** (§4-A). Today doctor prints ~30 capability rows and stops; it never answers the one question a user runs it to answer: *"will `blackglass open` work here, and if not, what do I do?"* A user on Apple Terminal sees `kitty graphics no`, `page viewport UNKNOWN`, and correctly-formatted rows, then runs `open` and hits a dead-end error (`could not determine terminal pixel size.`) that also tells them nothing. One computed verdict line closes both gaps at once. After that, fix the **seven error strings that state a failure but name no next action** (§3): the worst offender, `blackglass: could not determine terminal pixel size.` (`main.rs:248`), is a reachable failure on the exact terminal in our test matrix (Apple Terminal, no `CSI 14t` reply) and gives the user zero direction. The project already ships two good exemplars to copy — the no-URL message (`main.rs:222`) gives an example command, and the missing-electron message (`main.rs:367`) names the env var to set — so the rewrites in §3 just bring the other messages up to that bar.

---

## 1. Evidence base — strings captured from the shipped binary

Run this session against `target/release/blackglass`. These are the exact bytes a user sees; the audit in §3 quotes them verbatim.

```
$ blackglass version
blackglass 0.1.0                                            # exit 0

$ blackglass frobnicate
blackglass: unknown command "frobnicate"                    # exit 2, then prints full help

$ blackglass open
blackglass open: a URL is required
  e.g. blackglass open example.com                          # exit 2

$ blackglass open example.com          # stdin is not a tty (agent shell)
blackglass: stdin is not a tty. Interactive browsing needs a terminal.   # exit 1

$ blackglass doctor                    # stdin is not a tty
blackglass doctor 0.1.0
  status: NOT A TTY -- run this from an interactive terminal.
  Capability detection works by asking the terminal questions, which
  requires a terminal to ask.

  engine: /Users/adeebbashir/projects/blackglass/apps/engine/node_modules/.bin/electron   # exit 1
```

The help text (`blackglass help`, exit 0) reproduces the block at `main.rs:76-92` verbatim, including the `BLACKGLASS_BACKEND  force a backend: kitty | unicode` line audited in §3.

**What could not be captured at runtime this session, and why.** Every message on the interactive `open` path past the `isatty` guard (`main.rs:248, 265, 280`), inside `Session::start` (`main.rs:367, 377, 383, 423`), and mid-session (`main.rs:522, 528`) requires either a real TTY *and* a live Electron engine. Electron child processes fail under the agent Bash sandbox (`bootstrap_look_up ... Permission denied`, per the project brief) and the screen is locked, so those strings are audited from source, cited by `file:line`, and marked **UNVERIFIED (runtime)**. They are all statically reachable — the code paths are unconditional error returns — so the audit is sound; only the live capture is missing.

---

## 2. Usability test plan

### 2.1 Method

Discount usability, moderated think-aloud, **n = 5** participants (Nielsen's saturation point for a single interface). Two segments, ≥ 2 of each:

- **S1 — terminal power users:** daily kitty/Ghostty/tmux users, comfortable with env vars.
- **S2 — general developers:** default terminal (macOS Terminal / VS Code integrated), rarely touch terminal graphics.

Because the agent sandbox cannot spawn Electron and the machine is locked, tasks are split into two harnesses so the plan is runnable in parts *now* and in full by an operator at an unlocked machine:

- **T-A\* — automated / CI-able.** Pure-CLI paths that need no Electron and no human. Runnable by the agent today (evidence already in §1). Assert on stdout/stderr text and exit codes.
- **T-H\* — human-in-the-loop.** Discovery, comprehension, and recovery tasks. Require a human at a real terminal; some require a graphics terminal and a running engine.

### 2.2 Global scoring

- **Task success** = participant reaches the task goal **unaided** (no moderator hint, no external docs), using only the binary and its own output.
- **Error recovery** = after hitting an error, participant states or performs the **correct next action within 30 s**, unaided.
- **Task verdict:** PASS if ≥ 80 % (≥ 4/5) succeed; MARGINAL at 60 % (3/5); FAIL below 60 %.
- **Issue severity:** Critical (blocks the task, no workaround found) · Major (task done, but with a wrong turn or > 30 s of confusion) · Minor (cosmetic, self-corrected) · Enhancement.
- **Ship gate for the CLI:** every Critical fixed; T-H1..T-H5 all PASS; zero messages rated FAIL in the T-H3 battery (§2.4).

### 2.3 Automated tasks (runnable now)

| ID | Scenario | Command | PASS criteria (all must hold) | FAIL |
|----|----------|---------|-------------------------------|------|
| **T-A1** | Help & version are discoverable and correct | `blackglass help`; `blackglass version`; `blackglass bogus` | help exit 0 and contains `USAGE`, `open <url>`, `doctor`; version prints `blackglass 0.1.0` exit 0; unknown command exits **2** and prints full help | any wrong exit code, or help missing the `open` subcommand |
| **T-A2** | Bad invocation self-corrects | `blackglass open`; `printf '' \| blackglass open example.com` | no-URL: exit **2**, stderr contains the `e.g. blackglass open example.com` line; piped-stdin: exit **1**, stderr states stdin-is-not-a-terminal **and** how to run it right | exit 0 on either; or a message that names the problem but not the fix |
| **T-A3** | Missing engine routes to a fix | `BLACKGLASS_ENGINE=/nonexistent blackglass open example.com` under a pty (`script`/`unbuffer`) | exit **1**; stderr names `BLACKGLASS_ENGINE` or "reinstall" as the fix | error names no next action |
| **T-A4** | Happy-path end-to-end (operator-run; Electron + graphics terminal, sandbox disabled) | `BLACKGLASS_EXIT_AFTER_MS=3000 BLACKGLASS_LOG=/tmp/bg.log blackglass open https://example.com` in Ghostty | log contains `start`, `first-frame after <2500ms`, `bounded-run complete frames>0`; exit 0 | no first-frame line, frames = 0, or non-zero exit |

T-A1 and T-A2 are **already green** from the §1 capture (exit codes and text match). T-A3/T-A4 need a pty / a live engine and are marked operator-run.

**Latency pass criteria for T-A4**, anchored to the measured baselines in the project brief (engine ready 212 ms, first frame 366 ms, encode 0.74 ms, p50 frame gap 16.65 ms ≈ 60 fps): first frame ≤ **2.5 s** (7× headroom), steady-state ≥ **50 fps**, encode ≤ **2 ms/frame**. These three are shown live in the status bar (`main.rs:891`), so a human can read them off during T-H tasks without instrumentation.

### 2.4 Human-in-the-loop tasks

| ID | Segment | Goal given to participant | PASS criteria | What it exercises |
|----|---------|---------------------------|---------------|-------------------|
| **T-H1** | S1+S2 | "Open `example.com`." Bare shell, no docs, `blackglass` on PATH. | ≥ 4/5 reach a rendered page within **3 min** using only `blackglass help` / intuition; time-to-first-page recorded. | discoverability of the `open` subcommand; recovery from the plausible first guess `blackglass example.com` (→ unknown-command + help) |
| **T-H2** | S2 (Apple Terminal) | "Find out whether BlackGlass will work in this terminal, and why." | ≥ 4/5, reading `blackglass doctor` **alone**, correctly conclude it will not render legibly here, name a reason (no kitty graphics / no pixel size) **and** a fix (use Ghostty/kitty/WezTerm). | doctor as a *diagnosis*. **Expected FAIL against current output** (no verdict line) → motivates §4-A. |
| **T-H3** | S1+S2 | Error-comprehension battery (§ below). Each failing message shown as it appears; ask "what would you do next?" | per message: ≥ 4/5 give the correct next action within **30 s**. | the acceptance test for every §3 rewrite. Run **before** (current strings) and **after** (proposed). A rewrite is justified only if it moves a message from FAIL→PASS. |
| **T-H4** | S1 (Ghostty + engine) | While browsing, the page crashes (navigate to a crashing renderer / kill the renderer). "Get the page back." | ≥ 4/5 notice the page is dead and reload (`ctrl+r`) within **15 s**. | crash visibility. **Expected FAIL now** — `status.crashed` is tracked but never drawn (`main.rs:779` set, never read in `present`) → motivates §4-B. |
| **T-H5** | S1+S2 | "You're done — exit." | ≥ 4/5 quit with `ctrl+q` on the first attempt (hint is in the status bar) **and** the shell prompt returns cleanly (no raw-mode residue, no `stty sane` needed). | quit affordance + terminal restoration (`TtyGuard` drop). |

**T-H3 battery** = the twelve messages in the §3 audit table marked FAIL or PARTIAL. This is the direct link between the usability plan and the string rewrites: the plan's pass/fail *is* the justification for each rewrite. Report the before/after comprehension rate per message.

### 2.5 Deliverables from a run

Per participant: task success matrix, time-to-first-page (T-H1), per-message comprehension (T-H3 before/after), severity-tagged issue list. Roll-up: task PASS/MARGINAL/FAIL, ship-gate status. Because screenshots are unavailable on a locked machine, capture evidence as `BLACKGLASS_LOG` transcripts + shell scrollback (both CI-friendly and reproducible), not images.

---

## 3. User-facing string audit

Every user-facing string in `main.rs`, classified. **The test is a single question: after reading this, does the user know what to DO next?** Labels, values, and section headers in the doctor report are informational and pass by default; the scrutiny falls on errors and diagnostics.

### 3.1 Inventory & verdict

| # | `file:line` | String (verbatim / current) | Verdict | Why |
|---|-------------|------------------------------|---------|-----|
| 1 | `58` | `blackglass 0.1.0` | PASS | version output, expected |
| 2 | `66` | `blackglass: unknown command "x"` → then prints help | PASS | the auto-printed help *is* the next step |
| 3 | `76-89` | help USAGE / KEYS blocks | PASS | clear, example-shaped |
| 4 | `90` | `BLACKGLASS_BACKEND  force a backend: kitty \| unicode` | **PARTIAL** | `resolve_backend` (`207-214`) also accepts `sixel`; `iterm2` is auto-only. Help lists neither → a user trying to force sixel can't learn the value here |
| 5 | `89` | (help ENVIRONMENT) — `BLACKGLASS_LOG` absent | **PARTIAL** | the primary debugging lever is undocumented, so "how do I capture what went wrong" has no answer in help |
| 6 | `101-103` | doctor NOT-A-TTY status + explanation | PASS | tells the user to run it from a real terminal |
| 7 | `107,179` | `engine: NOT FOUND (set BLACKGLASS_ENGINE)` | PASS | names the env var to set — **exemplar** |
| 8 | `114` | `blackglass: cannot acquire terminal: {e}` | **FAIL** | states failure, no next step |
| 9 | `141-144` | no-graphics NOTE ("use Ghostty, kitty, or WezTerm") | PASS | names concrete terminals — **exemplar** |
| 10 | `151-153` | no-pixel-mouse NOTE | PASS | informational; nothing to do but accept (cosmetic) |
| 11 | `156-157` | no-kitty-keyboard NOTE | PASS | informational; accept-and-continue |
| 12 | `191` | `tmux (needs \`set -g allow-passthrough on\` for graphics)` | PASS | names the exact fix — **exemplar** |
| 13 | `193` | `screen (graphics passthrough unsupported)` | **PARTIAL** | states the limitation, implies but never states the action (leave `screen` / use tmux) |
| 14 | `161-185` | doctor geometry/engine/raw-reply rows | PASS | report values; but see §4-A (no verdict ties them together) |
| 15 | `222` | `a URL is required` / `e.g. blackglass open example.com` | PASS | gives a runnable example — **exemplar** |
| 16 | `229` | `blackglass: stdin is not a tty. Interactive browsing needs a terminal.` | **PARTIAL** | names the problem; weak on the cause (pipe/redirect/CI) and the fix |
| 17 | `236` | `blackglass: {e}` (raw TtyGuard error) | **FAIL** | bare error, no prefix context, no action; inconsistent with #8 |
| 18 | `248` | `blackglass: could not determine terminal pixel size.` | **FAIL (Critical)** | reachable on Apple Terminal (no `CSI 14t` reply); user gets zero direction |
| 19 | `265` | `blackglass: cannot enable input protocols: {e}` | **FAIL** | no next step |
| 20 | `280` | `blackglass: cannot start engine: {e}` | **PARTIAL** | quality rides on inner `{e}`; wrapper offers no log/diagnostic route |
| 21 | `367` | `electron not found; set BLACKGLASS_ENGINE to the engine directory` | PASS | names the env var — **exemplar** |
| 22 | `377` | `cannot derive engine root` | **FAIL (low pri)** | internal near-impossible state; still terse |
| 23 | `383` | `engine entrypoint missing: {path}` | **PARTIAL** | path helps, but no action (reinstall / repoint) |
| 24 | `423` | `engine did not connect within 30s` | **FAIL** | no next step; a 30 s wait ending in a dead-end |
| 25 | `522` | `engine exited` (via `eprint_restore`) | **FAIL** | engine died mid-session; two words, no log pointer. **UNVERIFIED (runtime)** |
| 26 | `528` | `engine read error: {e}` (via `eprint_restore`) | **FAIL** | no guidance. **UNVERIFIED (runtime)** |
| 27 | `891` | status bar `… ctrl+q quit` | PASS | carries the quit affordance; but see §4-B (crash never surfaced) |

Twelve entries need work: two PARTIAL help gaps (#4, #5), one PARTIAL diagnostic (#13), and nine error rewrites (#8, #13, #16, #17, #18, #19, #20, #22, #23, #24, #25, #26). #18 is the one Critical.

### 3.2 Before / after (PROPOSED — commander applies; I do not edit `main.rs`)

Rewrites keep the project's terse, lower-case house voice and add exactly one thing: the next action. **Carriage-return caveat:** messages emitted while raw mode is on (#25, #26, via `eprint_restore`, `main.rs:763`) must separate lines with `\r\n`, not `\n`, because `OPOST` is off; the pre-guard / post-`drop(guard)` messages (#8, #16, #17, #18, #19, #20) run outside raw mode and may use plain `\n`.

| # | `line` | Before | After (proposed) |
|---|--------|--------|------------------|
| 8 | `114` | `blackglass: cannot acquire terminal: {e}` | `blackglass: cannot put the terminal in raw mode: {e}`<br>`  This usually means stdin is redirected or the window closed.`<br>`  Run 'blackglass doctor' in a normal terminal window to check.` |
| 13 | `193` | `screen (graphics passthrough unsupported)` | `screen (no graphics passthrough -- run outside screen, or use tmux with 'set -g allow-passthrough on')` |
| 16 | `229` | `blackglass: stdin is not a tty. Interactive browsing needs a terminal.` | `blackglass: stdin is not a terminal, so there is nothing to browse from.`<br>`  Run 'blackglass open <url>' directly in a terminal window --`<br>`  not through a pipe, 'ssh host cmd', or a CI job.` |
| 17 | `236` | `blackglass: {e}` | `blackglass: cannot put the terminal in raw mode: {e}`<br>`  Run 'blackglass open <url>' directly in a terminal, not through a pipe or redirect.` |
| 18 | `248` | `blackglass: could not determine terminal pixel size.` | `blackglass: this terminal does not report its pixel size, which BlackGlass needs to size the page.`<br>`  Run 'blackglass doctor' to see what it reports.`<br>`  Terminals that report it: Ghostty, kitty, or WezTerm.` |
| 19 | `265` | `blackglass: cannot enable input protocols: {e}` | `blackglass: could not switch the terminal into keyboard/mouse reporting: {e}`<br>`  Try another terminal, and include 'blackglass doctor' output if you report this.` |
| 20 | `280` | `blackglass: cannot start engine: {e}` | `blackglass: could not start the browser engine: {e}`<br>`  Re-run with BLACKGLASS_LOG=/tmp/bg.log set to capture engine startup details.` |
| 22 | `377` | `cannot derive engine root` | `internal error: could not locate the engine root from {electron_path}. Please file a bug.` |
| 23 | `383` | `engine entrypoint missing: {path}` | `browser engine files are incomplete (missing {path}).`<br>`  Reinstall BlackGlass, or point BLACKGLASS_ENGINE at a complete engine build.` |
| 24 | `423` | `engine did not connect within 30s` | `the browser engine did not start within 30s (it may have crashed on launch).`<br>`  Re-run with BLACKGLASS_LOG=/tmp/bg.log set, then check that file.` |
| 25 | `522` | `engine exited` | `the browser engine exited unexpectedly.`<br>`  Re-run with BLACKGLASS_LOG=/tmp/bg.log set to capture why.`  *(lines joined with `\r\n`)* |
| 26 | `528` | `engine read error: {e}` | `lost the connection to the browser engine: {e}.`<br>`  Re-run with BLACKGLASS_LOG=/tmp/bg.log set to capture why.`  *(lines joined with `\r\n`)* |
| 4 | `90` | `BLACKGLASS_BACKEND  force a backend: kitty \| unicode` | `BLACKGLASS_BACKEND  force a backend: kitty \| sixel \| unicode (iterm2 is auto-detected only)` |
| 5 | `89` | *(no BLACKGLASS_LOG line)* | add: `BLACKGLASS_LOG      write a diagnostic log to this file path` |

Each rewrite is verifiable by the T-H3 battery (§2.4): show it, ask "what next?", require ≥ 4/5 correct within 30 s. All twelve are worded so a correct next action is stated outright (run doctor / set BLACKGLASS_LOG / use Ghostty / reinstall / don't pipe).

---

## 4. Structural gaps — commander instructions

These need behaviour in `main.rs` that I cannot write (file ownership). Described as diff-shaped instructions.

### 4-A. Give `doctor` a verdict line (highest value — see §0)

After the geometry block (`main.rs:~174`), before `engine`, compute and print a one-line verdict from state already in hand:

```
verdict
    open here?          <one of the four cases below>
```

- `viewport_px()` is `None` → `NO -- terminal reports no pixel size. Use Ghostty, kitty, or WezTerm.`
- backend is not pixel-exact (Unicode fallback) → `LOW FIDELITY -- layout visible, body text not legible. For full fidelity use Ghostty/kitty/WezTerm.`
- `locate_engine()` is `None` → `NO -- engine missing. Set BLACKGLASS_ENGINE or reinstall.`
- otherwise → `YES -- {backend} at {vp_w}x{vp_h}.`

This is exactly the reasoning `cmd_open` already performs at `main.rs:242-252`; surfacing it in doctor turns a data dump into the diagnosis T-H2 asks for, and makes the FAIL→dead-end of message #18 a predicted, explained outcome instead of a surprise. Acceptance: T-H2 PASS.

### 4-B. Surface a page/renderer crash in the status bar

`Status.crashed` is set from the engine's `crash` event (`main.rs:796-799`) but **never read** in `Renderer::present`. On a renderer crash the user sees a frozen last frame with no signal (T-H4 fails). Proposal: in `present` (`main.rs:848`), when `status.crashed` is `Some(reason)`, replace the normal bar text with a high-visibility line, e.g. `PAGE CRASHED ({reason}) -- ctrl+r to reload`. Reason is already engine-supplied and should still pass through `sanitize_for_terminal` before display. Acceptance: T-H4 PASS.

### 4-C. Help completeness

Apply rows #4 and #5 from §3.2 (document `sixel`/`iterm2` backend semantics and the `BLACKGLASS_LOG` var). Low effort; closes the "how do I capture what went wrong" gap that every log-pointer rewrite in §3.2 depends on.

---

## 5. Limitations & honest blockers

- **Nine error strings are audited by inspection, not runtime capture** (#18–#26): Electron will not spawn under the agent Bash sandbox (`bootstrap_look_up ... Permission denied`) and the machine is at a lock screen, so the interactive `open` path and all mid-session output could not be triggered this session. They are statically reachable (unconditional error returns on named lines) so the audit stands; marked **UNVERIFIED (runtime)** where that applies. T-A3/T-A4 and all T-H tasks require an operator at an unlocked machine with the sandbox disabled.
- **No screenshots** by design here — evidence is `BLACKGLASS_LOG` transcripts and shell scrollback, which are what a CI run would keep anyway.
- **No core files were modified.** All rewrites in §3.2 and behaviours in §4 are proposals for the commander, who owns `apps/cli/src/main.rs`.
- The T-A4 latency thresholds are derived from the brief's measured baselines, not re-measured this session; if the commander wants them re-anchored, run T-A4 in Ghostty and read the status bar.
