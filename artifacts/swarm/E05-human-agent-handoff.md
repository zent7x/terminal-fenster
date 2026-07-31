# E05 — Human/Agent Shared Control: arbitration, takeover, gates, and audit

**Mission:** specify the shared-control model for a browser driven by both a human (tty) and an
agent (RPC) — visible ownership, instant human takeover, pause/resume, action preview and
confirmation gates for sensitive operations, and a per-session audit log format. The state machine
is specified explicitly in §3.

**Status of the underlying code:** there is **no agent RPC in the repository today**. `apps/cli`
speaks only to the tty and the engine socket. Everything in this document is design-forward, and
§12 lists precisely what that means for confidence. What *is* grounded is the loop it must live in
(`apps/cli/src/main.rs:441-558`), the input decoder it must classify (`crates/bg-term/src/input.rs`),
the terminal modes already enabled (`crates/bg-term/src/tty.rs:152-161`), and four measurements I
took on this host (§1.3, §7.4).

**File ownership:** I wrote only this file. Every change described for `apps/cli/src/main.rs`,
`crates/bg-term/`, `crates/bg-proto/`, and `apps/engine/src/main.js` is described for the commander,
not made by me.

---

## 0. Recommendation in one paragraph

Put the arbiter **inside the existing single-threaded poll loop in the core**, as a third file
descriptor in the `poll` set at `apps/cli/src/main.rs:474-477`, and never anywhere else. Every
alternative — arbitration in the engine, arbitration behind a mutex on a second thread, the agent
holding its own connection to the engine socket — buys nothing and costs the one property that
makes takeover trustworthy: that the decision to lock the agent out happens in the same thread, in
the same iteration, as the read that discovered the human's keystroke, with no lock, no IPC
round-trip, and no window in which both parties are live. Once arbitration is a branch rather than a
protocol, "instant takeover" stops being a latency target that can regress and becomes a structural
invariant that a CI test can prove by exhausting the transition table. The rest of this document is
the state machine (§3), the takeover trigger classification that mode 1003 forces on us (§4), the
ownership indicator that must never be able to grow a chrome row (§5), the gate taxonomy (§6), and
a JSONL audit format whose durability policy is chosen from measured fsync latency rather than
assumed (§7).

---

## 1. Evidence base

### 1.1 What the running code already establishes

| Fact | Where | Why it constrains this design |
|---|---|---|
| One thread, one `poll(2)` over `{stdin, engine}` with a 16 ms timeout | `main.rs:474-478` | The timeout is a *ceiling on idleness*, not a floor on response. `poll` returns the instant stdin is readable, so arbitration in this loop is inherently prompt. Adding the agent fd here keeps that property. |
| stdin is read, decoded, and dispatched before engine messages | `main.rs:488-505` then `:519` | Human input is already first in the iteration. Takeover ordering is free if the agent queue drains *after* this block. |
| Engine socket is non-blocking; stdout is **blocking** | `main.rs:431`; `main.rs:899-901` | A blocking `write_all` of a frame can in principle stall the loop and therefore takeover. Measured non-issue locally (§1.3-D); real over SSH — see §4.4. |
| Mode 1003 (any-motion mouse tracking) is enabled | `tty.rs:156` | **Idle mouse movement generates stdin bytes.** A naive "any byte = takeover" rule would fire when the user brushes the mouse across the pane while reading. §4 exists because of this line. |
| Focus reporting (1004) and bracketed paste (2004) are enabled | `tty.rs:152-153` | Focus events are available as arbiter inputs; paste is a distinct `Event::Paste`. |
| Decoder maps only `0x01..=0x1a` to Ctrl+letter | `input.rs:219-229` | `Ctrl+]` (0x1D) is **not** decoded as a chord. See F2. |
| `handle_event` returns `bool` meaning "exit" and otherwise forwards to the engine | `main.rs:562-655` | The arbiter must interpose here. This is a modification to an existing `match`, hence the commander's. |
| Status bar is the last row, redrawn every `present`, page text sanitized first | `main.rs:885-897` | The ownership indicator belongs here, and inherits the existing sanitization discipline. |
| Workspace dependencies are exactly `libc` and `flate2` | `Cargo.toml:12-14` | A design that needs a new crate carries a real cost in this project. §7.6 respects that. |

### 1.2 What sibling missions already decided, which this document extends rather than revisits

- **A03 §4 (journey c)** established the shape: one browser, two drivers, agent observes the AX tree
  not pixels, `E_HUMAN_CONTROL` on takeover, `control.returned` must carry a **fresh** snapshot
  (c-F5), and every action lands in a JSONL log with an actor field. E05 turns that sketch into a
  transition table and a schema, and **deviates on one point** — the takeover trigger — with reasons
  in §4.1.
- **A09 §7 (TB5)** established that prompt-injection defence is *architectural*: constrain actions,
  do not filter text. M2's action-classification table is the seed of §6; M4's `REDACT_SELECTOR` is
  reused verbatim in §6.4 so redaction and gating cannot drift apart; M7 (agent output is untrusted
  for tty purposes) is why §6.5 renders the agent's own rationale as untrusted text; M8 is the audit
  log this document specifies; M9 is the budget system in §3.6.
- **D05 §9** established that there must be **one** modal prompt layer with one state machine, that
  it owns the keyboard, that `ctrl+q` is checked before it, that mouse is discarded while it is up,
  that it is text over pixels, and that rendering is a pure function so CI can assert on it. The
  confirmation gate in §6 is **not a new prompt layer** — it is a sixth `Prompt` variant.
- **D05 §10** established the three protocol-hygiene rules this document inherits without
  restating them per-message: engine-generated ids echoed back, an engine-side timeout on every ask,
  and deny as the timeout answer.
- **D04 §7** established the clipboard consent model, including the two invariants that the
  clipboard is never an agent tool and that every consent string is sanitized first.
- **D06 §3.2/§3.3** established the hard cap `C <= 2` on chrome rows and the rule that `C` must
  never change while a page is loaded, because a reflow costs a full-frame retransmit. §5.2 is
  written around that constraint, and it is the reason the ownership indicator does not get its own
  row on demand.

### 1.3 Measurements taken for this mission, on this host

macOS 26.1, Apple M4, APFS, `/usr/bin/python3`. Commands are reproduced so they can be re-run.

**A. Audit-record append and durability.** 231-byte JSON record, `O_WRONLY|O_CREAT|O_APPEND`, 500
iterations per mode:

| Mode | p50 | p99 | max |
|---|---|---|---|
| `write(2)` only | **0.0052 ms** | 0.0850 ms | 0.7508 ms |
| `write` + `fsync` | **0.0638 ms** | 1.0687 ms | 2.2953 ms |
| `write` + `F_FULLFSYNC` | **4.6230 ms** | **15.7194 ms** | **20.2097 ms** |

This is decision-relevant, not decorative. The measured p50 frame gap on this machine is 16.65 ms
(mission brief). A single `F_FULLFSYNC` at p99 consumes 15.72 ms — **essentially an entire frame** —
and at max exceeds one. `fsync` at p99 costs 1.07 ms, about 6% of a frame, which is affordable if it
is not on every record. §7.4 turns this into a three-tier policy.

**B. `O_APPEND` atomicity with two concurrent writers.** 1500 records each from two processes,
grepping for any line containing both tags:

| Record size | Lines | Interleaved | Bytes vs expected |
|---|---|---|---|
| 231 B | 3000 | **0** | 693,000 / 693,000 |
| 512 B | 3000 | **0** | 1,536,000 / 1,536,000 |
| 1 KiB | 3000 | **0** | 3,072,000 / 3,072,000 |
| 4 KiB | 3000 | **0** | 12,288,000 / 12,288,000 |
| 16 KiB | 3000 | **0** | 49,152,000 / 49,152,000 |
| 64 KiB | 3000 | **0** | 196,608,000 / 196,608,000 |

APFS did not tear a single append up to 64 KiB. I use this result to justify the *opposite* of what
it invites: because §7.2 makes the core the **sole writer**, this atomicity is a safety margin we
never lean on, rather than a load-bearing assumption about a filesystem we do not control. Note the
limit of the evidence — 3000 trials is evidence, not proof, and POSIX's guarantee for regular files
is weaker than for sub-`PIPE_BUF` pipe writes.

**C. SHA-256 over one record** (for the optional hash chain, §7.6): **1.67 µs** per 222-byte record.
At 100 records/s that is 0.017% of one core — the chain is free; the *dependency* is the cost.

**D. Can a blocking `present()` stall takeover locally?** A pty master accepted **256 MiB** without
ever returning `EAGAIN`. A 52.7 KiB kitty frame (the measured E2E wire size) therefore cannot block
`write_all` on a local terminal. The stall risk is real only when the pty is remote and the SSH
channel window is the limiter — C09's domain, quantified there at 1/10/100 Mbit. §4.4 states the
consequence honestly instead of inventing a local problem.

---

## 2. Findings for the commander

These are in core files I must not edit. Severity is about the shared-control model specifically.

### F1 — HIGH — "any input = takeover" is unimplementable as literally stated, because mode 1003 is on

`tty.rs:156` enables any-motion mouse tracking, deliberately, because CSS `:hover` does not work
without it. The consequence is that moving the mouse anywhere over the BlackGlass pane emits SGR
motion reports on stdin **continuously, with no button held**. A takeover rule of "any byte on
stdin" would seize control from the agent every time the user's palm nudged the trackpad while they
watched the agent work in an adjacent pane — which is the exact posture journey (c) is designed for.

The fix is not to disable 1003 (that breaks hover). It is to classify events into intent-bearing and
passive before they reach the arbiter. §4.2 gives the classification. It requires no protocol change
and no new decoder capability — every distinction needed is already present in `input::Event`.

### F2 — HIGH — `Ctrl+]`, A03's proposed takeover chord, is currently typed into the page

`Ctrl+]` encodes to `0x1D` (A06 §2.6, matching the kitty/VT100 table). The decoder's C0 arm at
`input.rs:219-229` matches `0x01..=0x1a` only, so `0x1D` falls through to `decode_utf8`, which
happily decodes it as `KeyCode::Char('\u{001d}')` **with `text: Some("\u{001d}")`**. `handle_event`
(`main.rs:593-605`) then forwards it to the engine as page text. So today the chord A03 §4 step 6
chose is (a) not a chord and (b) an invisible control character injected into whatever field has
focus.

Two independent reasons this matters: any design binding `Ctrl+]` inherits the bug, and any design
binding `Ctrl+\`, `Ctrl+^`, or `Ctrl+_` (`0x1C`, `0x1E`, `0x1F`) inherits it identically. The
decoder arm needs a `0x1c..=0x1f` case mapping to `Char('\\' | ']' | '^' | '_')` with `ctrl: true`.

§4.1 removes the dependency on this fix for the *primary* takeover path, so F2 is not a blocker —
but it is a live input-injection bug regardless of what E05 does.

### F3 — MEDIUM — `handle_event` has no interposition point, and its signature cannot express "swallowed"

`handle_event` returns `bool` = "should exit" (`main.rs:561-562`). Under shared control there are
four outcomes, not two: forward to page, swallow (consumed by chrome/gate), swallow **and** change
arbiter state, and exit. The commander will need a richer return — the natural shape is
`enum Disposition { Forward, Consumed, Exit }` with the state change as a side effect on `&mut self`
— and the agent-queue drain must happen *after* the stdin block at `:505` and *before* the engine
read at `:519`, so that a takeover discovered in this iteration cancels commands that would
otherwise have been sent in this iteration. Ordering here is the whole mechanism; it deserves a
comment in the source saying so.

### F4 — MEDIUM — an ownership indicator cannot be given its own row on demand

D06 §3.3 is unambiguous: `C` must never change while a page is loaded, because changing it re-runs
`setSize`, forces a Chromium reflow, and invalidates the frame — ~348 KiB retransmit, 3.5 fps on a
10 Mbit link. An agent attaching mid-session therefore **must not** grow the chrome from one row to
two. §5.2 resolves this by making the indicator the highest-priority *field* rather than a new row,
which also gives the right answer at 80 columns for free.

### F5 — MEDIUM — there is no way to distinguish a human at the keyboard from a process writing to the pty

The takeover model rests on "stdin carries only human intent". That is true of *our* process tree —
the agent connects over a socket and never touches the tty. It is **not** true of the pty itself:
`tmux send-keys`, `script`, `expect`, or any process with the master fd can synthesize bytes that
are indistinguishable from a keystroke at the tty layer. There is no robust mitigation at that layer,
and I will not pretend otherwise. A09 §4 already treats local uid compromise as out of scope for
TB4, and this inherits that boundary; §12 restates it as a limitation rather than burying it.

The practical consequence is narrow but worth writing down: an agent framework that drives
BlackGlass by `tmux send-keys` instead of by RPC would be *invisible to the arbiter*, would appear
in the audit log as `actor: "human"`, and would bypass every gate in §6. The mitigation is
documentation plus a refusal: §7.5 makes the audit log mandatory whenever `--agent` is present, so
at minimum the two integration styles are distinguishable after the fact.

### F6 — LOW — `Status` has no field for control state, and `present` has no branch for it

`Status` (`main.rs:770-780`) carries title/url/loading/frames/crash. The arbiter state, task id,
step counter, and pending-gate summary all need to reach `present`. This is additive and mechanical;
noted only so it is not discovered late.

---

## 3. The control state machine

### 3.1 Where it lives, and why that is the whole design

One arbiter, in the core, in `Session`, stepped only from the loop in `run`. The agent's RPC socket
becomes `fds[2]` alongside stdin and the engine socket at `main.rs:474-477`.

This placement is doing real work and is worth defending explicitly, because three plausible
alternatives all fail:

**Arbitration in the engine** would mean the human's keystroke travels core → socket → engine before
it can lock the agent out, while agent commands are arriving on a *different* connection to the same
engine. There is then a genuine race with no natural ordering, and it is resolved by whichever
message the Node event loop happens to pick up. Worse, a wedged renderer (a page looping `alert()`,
D05 §7) is precisely when the human most needs to take over and precisely when the engine is least
able to arbitrate.

**Arbitration behind a mutex on a second thread** makes takeover latency a function of lock
contention and of whatever the other thread is doing, which is exactly the property we are trying to
eliminate. It also makes the CI test in §10.1 impossible to write as a pure function.

**The agent connecting directly to the engine socket** removes the core from the path entirely, so
there is no takeover at all. It is worth naming because it is the cheapest thing to build and it
will be proposed.

With the arbiter in the loop, the ordering that makes takeover correct is a consequence of statement
order in one function:

```
poll({stdin, engine, agent}, 16ms)
  ├─ 1. stdin readable?  → decode → classify (§4.2) → arbiter.step()   ← takeover happens HERE
  ├─ 2. drain ≤K queued agent commands, but ONLY if state == AGENT     ← cancelled by step 1
  ├─ 3. agent socket readable? → parse request → classify (§6) → arbiter.step()
  ├─ 4. engine readable? → frames + events
  └─ 5. present()
```

No lock is taken because no lock is needed. The invariant "the agent cannot act after the human has
been seen" is not enforced by a check; it is enforced by step 2 following step 1.

### 3.2 States

Six states. `agent may act` means the arbiter will dispatch agent-originated commands to the engine.

| id | name | agent may act | human input → page | prompt owns keyboard | agent sees |
|---|---|---|---|---|---|
| `S0` | `SOLO` | — (none attached) | yes | no | — |
| `S1` | `AGENT` | **yes** | no (any intent → `S2`) | no | normal results |
| `S2` | `HUMAN` | no | **yes** | no | `E_HUMAN_CONTROL` |
| `S3` | `GATE` | no (blocked on this action) | no (captured by prompt) | **yes** | request pending |
| `S4` | `PAUSED` | no | **yes** | no | `E_PAUSED` |
| `S5` | `ENDED` | no | **yes** | no | `E_NO_TASK` |

`S2` and `S4` route input identically, and collapsing them is the obvious simplification. Keep them
separate for two reasons that are not cosmetic. First, the agent must be able to distinguish "a
human grabbed the wheel mid-turn, your action may have partially applied, re-ground before you do
anything" from "you were suspended cleanly at a turn boundary, nothing of yours is half-done" — an
agent framework makes different decisions (retry vs. wait vs. abort) on those two, and conflating
them produces the c-F5 class of bug where the agent resumes against a stale model. Second, the audit
trail loses its most interesting signal if implicit seizure and explicit pause look the same:
repeated `S1 → S2` transitions mean the agent is doing things the human keeps having to interrupt,
which is a quality signal worth being able to count.

### 3.3 Events

| Class | Event | Source |
|---|---|---|
| Human, intent | `H_INTENT` | key press (non-modifier), mouse button down, wheel, paste — §4.2 |
| Human, passive | `H_PASSIVE` | mouse motion without button, focus in/out, unknown escape — §4.2 |
| Human, chrome | `H_HANDBACK` `H_PAUSE` `H_ABORT` `H_QUIT` | `^G h`, `^G p`, `^G x`, `ctrl+q` |
| Human, gate | `H_ALLOW` `H_DENY` `H_SEIZE` | keys in the gate prompt only |
| Agent | `A_ATTACH` `A_DETACH` `A_ACTION(c)` `A_DONE` | RPC; `c` ∈ {`AUTO`,`NOTIFY`,`CONFIRM`,`STRONG`,`NEVER`} per §6 |
| System | `S_GATE_TIMEOUT` `S_BUDGET` `S_CRASH` `S_RECOVERED` | timers, §3.6 budgets, engine `crash` event |

### 3.4 Transition table (normative)

Effects are listed in execution order. Every row writes exactly one audit record (§7) — the `audit`
column names its `act`.

**From `S0` (SOLO)**

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `A_ATTACH` | human grant present (§3.5) | `S1` | ack; snapshot AX; start budgets | `agent.attach` |
| `A_ATTACH` | no grant | `S0` | reject `E_NO_GRANT` | `agent.attach.refused` |
| `H_*` | — | `S0` | normal browsing | (not audited; §7.3) |

**From `S1` (AGENT)** — the interesting state

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `H_INTENT` | — | **`S2`** | **1.** clear agent command queue **2.** `gen += 1` **3.** mark in-flight action `interrupted` **4.** push `control.seized` to agent **5.** forward the triggering input to the page | `control.takeover` |
| `H_PASSIVE` | — | `S1` | forward to page (hover still works while the agent drives) | — |
| `H_PAUSE` | — | `S4` | drain-stop; `control.paused` | `control.pause` |
| `H_ABORT` | — | `S5` | cancel task; `control.aborted` | `agent.abort` |
| `H_QUIT` | — | exit | §3.7 shutdown | `session.end` |
| `A_ACTION(AUTO)` | budget ok; target resolved | `S1` | enqueue expansion (§3.8) | `agent.act` |
| `A_ACTION(NOTIFY)` | budget ok | `S1` | enqueue; flash indicator line | `agent.act` |
| `A_ACTION(CONFIRM\|STRONG)` | budget ok | **`S3`** | build preview (§6.5); raise prompt; start 60 s timer | `gate.ask` |
| `A_ACTION(NEVER)` | — | `S1` | reject `E_FORBIDDEN`; `denials += 1` | `gate.refused` |
| `A_ACTION(*)` | target unresolved | **`S3`** | fail-closed: escalate to `CONFIRM` (§6.3) | `gate.ask` |
| `A_DONE` | — | `S5` | `control.done` | `agent.done` |
| `A_DETACH` | — | `S0` | drop grant? no — grant persists for the session | `agent.detach` |
| `S_BUDGET` | — | `S5` | abort, reason in record | `agent.budget_exhausted` |
| `S_CRASH` | — | `S4` | pause; surface in indicator; await B08 recovery | `engine.crash` |

**From `S2` (HUMAN)**

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `H_INTENT` / `H_PASSIVE` | — | `S2` | forward to page | — |
| `H_HANDBACK` | no pending gate | `S1` | **fresh AX snapshot**; `control.returned{ax_hash}` (A03 c-F5) | `control.handback` |
| `H_PAUSE` | — | `S4` | — | `control.pause` |
| `H_ABORT` | — | `S5` | — | `agent.abort` |
| `A_ACTION(*)` | — | `S2` | reject `E_HUMAN_CONTROL`; count | `agent.act.rejected` |
| *(timeout)* | — | `S2` | **nothing — there is no auto-handback, ever** (I3) | — |

**From `S3` (GATE)**

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `H_ALLOW` | armed (§6.6); for `STRONG`, word matched | `S1` | dispatch **this one action only**; clear gate | `gate.answer{allow}` |
| `H_DENY` | — | `S1` | reject `E_DENIED`; `denials += 1` | `gate.answer{deny}` |
| `H_SEIZE` | — | `S2` | deny the action, then seize | `gate.answer{deny}` + `control.takeover` |
| `H_QUIT` | — | exit | checked **before** gate routing (D05 §9 rule 3) | `session.end` |
| `S_GATE_TIMEOUT` | — | `S1` | **deny** (D05 §10); `denials += 1` | `gate.answer{timeout_deny}` |
| `H_PASSIVE`, mouse | — | `S3` | **discarded** (D05 §9 rule 4 — forwarding clicks under a modal is a clickjacking primitive) | — |
| `S_CRASH` | — | `S4` | gate torn down as denied | `engine.crash` |

**From `S4` (PAUSED)**

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `H_HANDBACK` | engine healthy | `S1` | fresh AX snapshot; `control.resumed{ax_hash}` | `control.resume` |
| `H_ABORT` | — | `S5` | — | `agent.abort` |
| `H_INTENT` / `H_PASSIVE` | — | `S4` | forward to page | — |
| `A_ACTION(*)` | — | `S4` | reject `E_PAUSED` | `agent.act.rejected` |
| `S_RECOVERED` | — | `S4` | stay paused; human resumes explicitly | `engine.recovered` |

**From `S5` (ENDED)**

| Event | Guard | → | Effects | audit |
|---|---|---|---|---|
| `A_ATTACH` (new task) | grant still valid | `S1` | reset budgets, new `tid` | `agent.attach` |
| `A_DETACH` | — | `S0` | — | `agent.detach` |
| `H_*` | — | `S5` | normal browsing | — |

### 3.4a Exhaustive next-state matrix

For the CI test in §10.1. `—` means self-transition with no state change.

| | `H_INTENT` | `H_PASSIVE` | `H_HANDBACK` | `H_PAUSE` | `H_ABORT` | `H_ALLOW` | `H_DENY` | `H_SEIZE` | `A_ATTACH` | `A_ACT(auto/notify)` | `A_ACT(confirm/strong)` | `A_ACT(never)` | `A_DONE` | `A_DETACH` | `S_GATE_TIMEOUT` | `S_BUDGET` | `S_CRASH` |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **S0** | — | — | — | — | — | — | — | — | S1 | — | — | — | — | — | — | — | — |
| **S1** | **S2** | — | — | S4 | S5 | — | — | — | — | — | **S3** | — | S5 | S0 | — | S5 | S4 |
| **S2** | — | — | **S1** | S4 | S5 | — | — | — | — | — | — | — | S5 | S0 | — | S5 | S4 |
| **S3** | — | — | — | — | S5 | **S1** | **S1** | **S2** | — | — | — | — | — | S0 | **S1** | S5 | S4 |
| **S4** | — | — | **S1** | — | S5 | — | — | — | — | — | — | — | S5 | S0 | — | S5 | — |
| **S5** | — | — | — | — | — | — | — | — | S1 | — | — | — | — | S0 | — | — | — |

Blank-looking cells are not "undefined" — every one is a defined self-transition whose effect is
either "forward to page", "reject with the state's error code", or "ignore". §10.1 asserts that
`arbiter_step` is total over `State × Event`.

`H_QUIT` is deliberately absent from the matrix: it exits from **every** state without exception
(I9), so giving it a column of six identical cells would imply it is negotiable. It is checked before
any other routing, including the gate's (D05 §9 rule 3), and §10.1 tests it as a separate assertion
over all six states rather than as a matrix row.

### 3.5 Invariants

These are the properties CI proves, and the reason to prefer a table over prose.

- **I1 — Exactly one input source reaches the engine at any instant.** There is no state in which
  both `human input → page` and `agent may act` are true.
- **I2 — Human intent always wins, in the same loop iteration, with no syscall other than a
  buffered 5.2 µs audit append.** Guaranteed by statement order (§3.1), not by a check.
- **I3 — Control is never returned to the agent implicitly.** No timeout, no idle timer, no
  heuristic. `S2 → S1` and `S4 → S1` require `H_HANDBACK`. The asymmetry is the point: seizing is
  automatic and free, yielding is deliberate and explicit.
- **I4 — Every handback carries a fresh AX snapshot and its hash.** (A03 c-F5.) An agent resuming
  against a pre-takeover model is the failure this prevents.
- **I5 — Deny is the answer on timeout, crash, disconnect, or ambiguity.** (D05 §10.)
- **I6 — The agent cannot cause a transition into `S1`.** Only `H_HANDBACK` or a human-granted
  `A_ATTACH`. An agent that could grant itself control has no gates.
- **I7 — Every transition emits exactly one audit record, written before its effects are
  observable.** Not after: a crash between effect and record must not lose the record.
- **I8 — Ownership is visible in the chrome in every state**, and it is the last field truncated
  (§5.2).
- **I9 — `ctrl+q` is honoured in every state**, checked before gate routing (D05 §9 rule 3).

### 3.6 Budgets (A09 M9, made concrete)

Budgets are counters on the arbiter; exhausting any one fires `S_BUDGET` → `S5`.

| Budget | Default | Rationale |
|---|---|---|
| navigations per task | 20 | A09 M9. Contains an injected loop. |
| cross-site navigations per task | 3 | The dominant exfil channel; each already costs a `CONFIRM`. |
| wall clock per task | 10 min | |
| engine commands per task | 5,000 | Catches a runaway expansion (§3.8). |
| **consecutive gate denials** | **3** | **Not in M9. Three refusals in a row is the signature of an injected instruction the human keeps rejecting; continuing to ask is how habituation is manufactured.** Abort instead. |
| bytes of page text into agent context | 2 MiB | |

### 3.7 Shutdown

`H_QUIT` in any state: deny any open gate, cancel the task, write `session.end`, `F_FULLFSYNC` the
audit fd once (§7.4), then the existing `shutdown()` path at `main.rs:657-678`. The `SIGINT`/`SIGTERM`
handlers installed at `tty.rs:132-134` must additionally flush and full-sync the audit fd — a
signal-safe concern the commander should note, since the existing handler only restores the tty.

### 3.8 Action expansion, and why takeover granularity depends on it

One agent action is many engine commands. `type("hunter2 is not my password")` becomes ~29 key
commands through the path at `main.rs:593-605`. If the arbiter expands eagerly and writes all 29 to
the socket, takeover cannot interrupt them — they are already committed, and the stream's ordering
guarantee works against us.

So: an accepted action produces an `ActionPlan` — a queue of engine commands — and step 2 of the
loop drains **at most `K` per iteration**, only while `state == S1`. `H_INTENT` clears the queue.

Choosing `K`: `K = 1` gives 16 ms granularity but makes a 29-character string take 464 ms, which
looks broken. `K = 8` bounds the post-takeover overshoot to 8 commands (~8 keystrokes worth, all of
which land before the human's own input by construction) and types at ~500 chars/s. Take `K = 8`,
and cap the overshoot honestly in the docs rather than claiming zero.

In-flight *asynchronous* effects are a different matter and should not be cancelled: a navigation
the agent started before the takeover completes normally. Yanking it would leave the page in a state
neither party asked for. The rule is **takeover stops new actions; it does not roll back committed
ones**, and the audit record for the interrupted action carries `result: "interrupted"` so the
transcript is honest about the boundary.

---

## 4. Takeover

### 4.1 The trigger: any intent-bearing input, not a chord

A03 §4 step 6 proposed `Ctrl-]` to toggle takeover. E05 deviates: **takeover is implicit on
intent-bearing input; only handback is a chord.** Four reasons.

The mission states the requirement as "instant human takeover on *any input*", and a chord is not
any input — it is one specific input the user must remember under exactly the conditions (the agent
is doing something alarming) where recall is worst. A user who lunges for the keyboard and types
`no no no` should already have control; with a toggle they have typed `no no no` into the page.

Second, the asymmetry is correct on its own terms. Seizing control is the safety action and should
have the lowest possible activation energy. Yielding control is the dangerous one and should be
deliberate. A single toggle gives both the same cost, which is precisely backwards.

Third, F2: the proposed chord does not currently decode, and worse, injects `0x1D` into the page. An
implicit trigger has no such dependency and works identically on Apple Terminal, where the kitty
keyboard protocol is unavailable (mission brief matrix).

Fourth, it composes with the gate: in `S3` the prompt owns the keyboard (D05 §9 rule 2), so takeover
there is an explicit key in the prompt's own map (`t`), which is discoverable because the prompt
shows it.

Handback stays behind D06's leader — `^G h` — with `^G p` pause and `^G x` abort, satisfying D06 §9
rule 5 (everything reachable through the leader, nothing Apple-Terminal-unreachable). If the
commander fixes F2, `Ctrl+]` may be added as a Tier-A fast path for handback, never as the only path.

### 4.2 Classification (this is the F1 fix)

Every distinction needed already exists in `input::Event`. No decoder capability is added; only a
classifier, which is pure and therefore testable.

| `input::Event` | Class | Note |
|---|---|---|
| `Key{kind: Press}`, printable or named | **INTENT** | |
| `Key{kind: Release}` | passive | already dropped at `main.rs:566-568` |
| `Key` that is modifier-only | passive | a bare Shift is not intent |
| `Paste(_)` | **INTENT** | unambiguous deliberate act |
| `Mouse{kind: Down}` | **INTENT** | |
| `Mouse{kind: Up}` | passive | avoids a second trigger from the same click |
| `Mouse{kind: Wheel*}` | **INTENT** | scrolling is reading; the human is engaged |
| `Mouse{kind: Move}` | **passive** | **the F1 case: mode 1003 emits these constantly** |
| `Mouse{kind: Move}` with a button held (drag) | **INTENT** | a drag is a deliberate act |
| `FocusGained` / `FocusLost` | passive | §4.3 |
| `Unknown(_)` | passive | never let an undecoded escape seize control |

`Mouse{Move}` with a button held is not directly representable today — `MouseKind::Move` carries a
`button` field but drag state is not tracked across events. The classifier needs a one-bool
`buttons_down` latch, set on `Down` and cleared on `Up`. Small, and it belongs with the classifier,
not the decoder.

### 4.3 Focus is deliberately *not* a takeover or pause trigger

Tempting, and wrong for the primary journey. In A03 journey (c) the human watches BlackGlass in an
adjacent tmux pane while working elsewhere; the BlackGlass pane is unfocused for most of the task.
Pausing the agent on `FocusLost` would make the headline use case unusable.

Focus does matter in two narrower ways. A gate raised while the pane is unfocused must not have its
timer count against a human who cannot see it: the 60 s timer **starts on `FocusGained`**, or
immediately if focus reporting is unavailable (Apple Terminal, per the matrix) — and in that case
the timeout is raised to 120 s, since we cannot know whether the user saw it. And `FocusLost` must
still cancel D04's clipboard paste-handshake arm, which is D04 §5.4's rule and unaffected by any of
this.

### 4.4 Latency: what is bounded, and what is not

Bounded, and provable by construction:

| Step | Cost |
|---|---|
| `poll` returns on stdin readable | immediate; the 16 ms is a ceiling on *idleness*, not a delay |
| `read` + decode + classify | pure computation, no allocation on the common path |
| `arbiter.step` → clear queue, bump `gen` | O(queue), no syscall |
| audit append | **5.2 µs p50, 85 µs p99** (§1.3-A) |
| agent-visible lockout | same iteration; no IPC needed — the core simply stops dispatching |
| **worst-case agent overshoot** | **≤ `K` = 8 commands already queued but undrained** |

Not bounded, and I will not claim otherwise: the wall-clock time from the user's finger to the
kernel marking stdin readable, and — over SSH — the time `present()` may spend in a blocking
`write_all` before the loop reaches `poll` again. Locally the latter is a non-issue (§1.3-D: a pty
absorbed 256 MiB without blocking). Remotely it is bounded by the SSH channel window and is exactly
what C09's byte-credit pacer exists to control; C09 §5 caps in-flight bytes so the loop cannot be
parked behind a large committed frame. E05 adds one requirement to that: **the pacer's credit limit
is also a takeover-latency budget**, and should be documented as such in C09's terms, because
raising it for throughput silently raises worst-case takeover latency.

An end-to-end takeover latency number is **UNVERIFIED** — it cannot be measured without a running
engine (blocked, §12) and without an agent RPC (does not exist).

---

## 5. Visible ownership

### 5.1 What it must survive

The indicator answers one question — *whose keystrokes are these?* — and it has to answer it when
the terminal is 80 columns, when the page is trying to lie about it, when the user glances for 200 ms
from another pane, and in the half-block tier where the page is an impressionist smear. That rules
out anything cute: no colour-only signal (breaks in monochrome and for colour-blind users), no
Unicode-only sigil (C04's font-coverage problem), no overlay (an overlay can be dismissed, and
ownership must not be dismissible).

### 5.2 Placement: highest-priority field, never a new row

F4 is the constraint. The resolution has two cases.

**Launched with `--agent`:** `C = 2` from the start, so the chrome budget is settled before the first
`setSize` and never changes. The second row is the agent row. This is the recommended way to run
agent sessions and the flag doubles as the human grant (§6.7).

**Agent attaches mid-session:** `C` does **not** change. The indicator is inserted into the existing
chrome row as the **highest-priority field**, ahead of everything in D06 §6.2's list — ahead of the
URL, ahead of the title, ahead of the fps/bytes telemetry, ahead of the `^G menu` hint. Telemetry is
diagnostic; ownership is safety. When the row is too narrow for both, telemetry is what goes.

This also gives the 80-column answer for free, with no separate narrow-layout rule.

### 5.3 Rendering

ASCII first, bracketed, reverse-video, fixed width so it never reflows:

```
[AGENT ]   agent is driving        reverse video, bold
[ YOU  ]   human control           normal video
[PAUSED]   agent suspended         reverse video
[CONFIRM]  gate open               reverse video + the gate prompt is up
```

Fixed 8 columns (`[AGENT ]`) so the fields after it never shift as state changes — a shifting status
bar is how users learn to stop reading it.

**Wide, `--agent`, `C = 2`:**

```
 [AGENT ]  step 7/20  click  "Sign in"                                    nav 4/20  ^G p pause  ^G x stop
  developer.mozilla.org/en-US/docs/Web/API/fetch                                        60fps 53KB 0.7ms   ^G menu
```

**Wide, after takeover:**

```
 [ YOU  ]  agent locked out - 12 steps done, 1 interrupted                          ^G h hand back  ^G x stop
  developer.mozilla.org/en-US/docs/Web/API/fetch                                        60fps 53KB 0.7ms   ^G menu
```

**Narrow (80 cols, `C = 1`) — the indicator preempts telemetry:**

```
 [AGENT ] 7/20 click "Sign in"   developer.mozilla.org/...          ^G
```

```
 [ YOU  ] agent locked out       developer.mozilla.org/...   ^G h   ^G
```

### 5.4 Anti-spoofing, honestly

The page can paint pixels that look like our chrome. It cannot paint *into* our chrome: reserved
rows are outside the viewport (`PointerMap` already excludes them, `main.rs:712-729`), and C06
establishes that our text composites above a negative-`z` kitty placement. So a forgery is
necessarily a *second* band directly above the real one, and two stacked status bars look wrong.

Three cheap reinforcements: draw the indicator on **every** `present`, even when nothing changed, so
it can never go stale behind a page that repainted; keep the bottom-most row unconditionally ours
(never let chrome float); and make the transition to `[ YOU  ]` also emit a brief distinct
appearance (bold + a leading `!` for ~500 ms) so a takeover is noticed even peripherally.

What this does **not** achieve: a user who does not know the "bottom row is always real" invariant
can still be fooled by a convincing forgery one row up. I considered a per-session nonce glyph
rendered in the indicator (the page cannot know it) and rejected it — it only helps a user who
checks it, which is the same user who would notice the doubled band, and it adds a secret to the
rendering path for no marginal benefit. Documented as a limitation, not solved.

---

## 6. Confirmation gates

### 6.1 The classes

| Class | Behaviour | State effect |
|---|---|---|
| `AUTO` | execute; audit only | stays `S1` |
| `NOTIFY` | execute; audit; flash a line in the indicator row | stays `S1` |
| `CONFIRM` | modal; single keypress; 60 s timeout → **deny** | `S1 → S3 → S1` |
| `STRONG` | modal; **must type a shown word**; accept key armed only after 750 ms | `S1 → S3 → S1` |
| `NEVER` | refuse; audit; `denials += 1` | stays `S1` |

`STRONG` exists because `CONFIRM` degrades under repetition: a user who has pressed `y` forty times
will press it the forty-first time without reading. Requiring a *typed word that appears only in the
prompt* defeats muscle memory, and the 750 ms arming delay defeats the case where a keystroke already
in flight (or an autorepeat) answers a prompt the user has not yet seen. Both are standard
anti-habituation measures and both are cheap; the cost is that `STRONG` must be rare or it becomes
the thing users route around.

### 6.2 The taxonomy

Extends A09 M2. Where A09 already ruled, this table agrees with it.

| Agent action | Class | Basis |
|---|---|---|
| read AX tree / text, scroll, screenshot, hover | `AUTO` | A09 M2 read-only |
| click a non-submit element, same origin | `AUTO` | A09 M2 |
| back / forward / reload | `AUTO` | no new egress |
| type into an ordinary text input | `NOTIFY` | visible, reversible, high frequency |
| new tab | `NOTIFY` | budgeted |
| close tab | `NOTIFY` | recoverable via B04 |
| navigate within same eTLD+1 | `AUTO` | A09 M3 origin-scoped session |
| **navigate cross-site** | **`CONFIRM`** | A09 M2; exfil channel #1 |
| navigate to a non-`http(s)` scheme | **`NEVER`** | A09 §3.5 scheme allowlist |
| **form submit (any)** | **`CONFIRM`** | A09 M2; exfil channel #2 |
| form submit matching destructive heuristics | **`STRONG`** | §6.4 |
| form submit on a payment surface | **`STRONG`** | §6.4 |
| **type into a password / OTP / cc field** | **`NEVER`** | A09 M2 "never; user types it" + M4 |
| **clipboard read** | **`NEVER`** | D04 §7.4, A09 M5 — no such tool in v1 |
| clipboard write | **`NEVER`** | D04 §7.1 defines only Tier U/G; the agent is neither |
| **download** | **`CONFIRM`** | A09 M2; routed through D05 §3 `downloadAsk` |
| **answer a file chooser** | **`NEVER`** | D05 §4 — the human picks files, always |
| **answer a permission prompt** | **`NEVER`** | D05 §5 — grants outlive the task |
| **answer a JS dialog** | **`NEVER`** | D05 §7 |
| upload via `DOM.setFileInputFiles` | **`NEVER`** | same reasoning as the chooser |
| cookie / storage read | `CONFIRM` **+ capability, off by default** | A09 M2 "confirm + separate capability" |
| evaluate arbitrary JS in the page | `STRONG` **+ capability, off by default** | equivalent to all of the above at once |
| `shell.openExternal` | **`NEVER`** | A09 §3.5 |

"Capability, off by default" means the class only becomes reachable when the human passes an explicit
launch flag; without it the action is `NEVER`. This reconciles A09 M2 (which permits these under
confirm-plus-capability) with a v1 default that ships nothing dangerous.

### 6.3 Where classification happens, and the fail-closed rule

Classification needs the *resolved target node's* attributes, which only the engine has. So: the
agent's request arrives at the core; the core forwards a `classifyTarget` query to the engine; the
engine resolves the node and returns its class plus the attributes needed to render a preview; the
core decides. The core decides — always — because the core is the arbiter and the engine may be
serving a compromised renderer.

**Fail-closed rule (invariant):** if the engine cannot resolve the target — node vanished, page
navigated, cross-origin iframe opaque, query timed out — the action is **never** `AUTO`. It is
escalated to at least `CONFIRM`, with the preview stating plainly that the target could not be
verified. Unknown must cost more than known, or every attack is an attack on the resolver.

Corollary (A03 c-F3): every agent action carries the `ax_hash` it was grounded on. A mismatch against
the current tree escalates the class by one step and says so in the preview. An agent acting on a
model of a page that has since changed is the c-F3 bug, and the gate is the right place to catch it.

### 6.4 Detection heuristics — named as heuristics

**Credential, OTP, and payment fields:** reuse A09 M4's `REDACT_SELECTOR` **verbatim, from one shared
constant**. This is the single most valuable structural decision in §6: redaction (what never enters
model context) and gating (what the agent may never type into) must be the same list, because a
field that drifts out of one list and not the other creates exactly the hole both were built to
close. One `const`, two consumers, one test asserting they are the same object.

**Destructive submits:** the accessible name of the submit control, or the form's action path,
matching `\b(delete|remove|destroy|deactivate|terminate|revoke|wipe|erase|unsubscribe|cancel
(subscription|account)|close account|transfer)\b` case-insensitively.

**Payment surfaces:** any `autocomplete="cc-*"` field in the form; or `PaymentRequest` constructed
(observable from the preload shim D05 §6 already needs); or submit-control text matching
`\b(pay|purchase|buy|place order|checkout|donate|subscribe)\b`.

These are heuristics and they are stated as heuristics. They will produce false positives (a "Delete
draft" button gets a `STRONG` gate) and false negatives (a destructive action labelled "Continue"
does not). The design absorbs that asymmetrically: false positives cost the human one keystroke;
false negatives are caught by the `CONFIRM` that *every* form submit gets regardless. That is why the
baseline for form submission is `CONFIRM` and not `AUTO` — the heuristics only decide whether to
escalate, never whether to gate at all. Nothing depends on the regex being complete.

### 6.5 The preview

Six things, in this order, because it is the order a person actually asks them in:

1. **Who** — agent id and task id.
2. **What** — the verb and the resolved target: role, accessible name, and the visible text.
3. **Where** — origin, sanitized (D05 §9 rule 6); for navigation, destination eTLD+1 called out
   separately from the full URL, since eTLD+1 is what the decision turns on.
4. **Why** — the agent's own rationale, ≤120 chars, **sanitized and visibly marked as untrusted**
   (A09 M7). It is rendered in normal video, indented, prefixed `agent says:`. It must never be
   styled like our chrome, because a model can be induced to emit text that impersonates chrome.
5. **Effect** — for a submit: the field names being sent, each marked `<redacted>` where §6.4
   matched. This is the highest-value line in the box; it is what turns "submit this form?" into an
   answerable question.
6. **Warnings** — stale `ax_hash`, unresolved target, cross-site destination, prior denials this task.

Wide (`CONFIRM`, cross-site navigation):

```
  +-- agent wants to navigate off-site ------------------------------------------------+
  |                                                                                     |
  |   from  developer.mozilla.org                                                       |
  |     to  api-collect.example.net        <-- different site                           |
  |    url  https://api-collect.example.net/r?d=eyJ1c2VyIjoi...                          |
  |                                                                                     |
  |   agent says:  following the documentation link for fetch()                         |
  |                (this text came from the agent and is not verified)                  |
  |                                                                                     |
  |   cross-site navigation 1 of 3 allowed this task                                    |
  |                                                                                     |
  |   [y] allow once      [n] deny      [t] take over      [Esc] deny                   |
  +-------------------------------------------------------------------------------------+
```

Wide (`STRONG`, destructive submit):

```
  +-- agent wants to submit a destructive form -----------------------------------------+
  |                                                                                      |
  |   origin  admin.internal.corp                                                        |
  |   action  POST /accounts/8814/delete                                                 |
  |   button  "Delete account permanently"                                               |
  |                                                                                      |
  |   sending  account_id = 8814                                                         |
  |            confirm    = true                                                         |
  |            csrf       = <redacted>                                                   |
  |                                                                                      |
  |   agent says:  the task asked me to clean up the test account                        |
  |                (this text came from the agent and is not verified)                   |
  |                                                                                      |
  |   !  the page changed since the agent looked at it                                   |
  |                                                                                      |
  |   type  delete  to allow, or [n] to deny        > _                                  |
  +--------------------------------------------------------------------------------------+
```

Narrow (80 cols) — same content, same decision, ruthlessly cut:

```
 +-- agent: submit destructive form ------------------------+
 |  admin.internal.corp                                     |
 |  POST /accounts/8814/delete                              |
 |  "Delete account permanently"                            |
 |  sends: account_id=8814 confirm=true csrf=<redacted>     |
 |  agent says: clean up the test account   (unverified)    |
 |  ! page changed since agent looked                       |
 |  type  delete  to allow, [n] deny      > _               |
 +----------------------------------------------------------+
```

### 6.6 Gate mechanics

Per D05 §9 this is a sixth `Prompt` variant, not a new layer, which means it inherits: one prompt at
a time with FIFO queueing and an `(n more)` badge, prompt owns the keyboard, `ctrl+q` checked first,
mouse discarded, text over pixels, everything sanitized. It adds four rules of its own:

- **Arming.** `CONFIRM` arms its accept key after 250 ms; `STRONG` after 750 ms. Before arming,
  accept keys are ignored (not queued — ignored, so a held key cannot answer on arming).
- **Scope of an allow.** `CONFIRM` may offer `[a] allow this kind from this origin for this task`,
  cleared on task end, on origin change, and on any takeover. `STRONG` offers **allow-once only**.
  Nothing is ever persisted to disk. A remembered `STRONG` grant is a standing authorisation to spend
  the user's money, and no product benefit justifies it.
- **Timeout.** 60 s, starting on `FocusGained` (§4.3), 120 s where focus reporting is unavailable.
  Deny on expiry.
- **Denial counter.** Three consecutive denials in a task abort it (§3.6).

### 6.7 The grant

Before any of this, the human must have authorised agent control at all. `--agent` at launch is the
grant; `^G a` grants it mid-session with a one-time `CONFIRM`. The grant is per-session, never
persisted, and revoked by `^G x` or exit. Without it, `A_ATTACH` is refused (§3.4, `S0`).

---

## 7. The audit log

### 7.1 What it is for

Two consumers with different needs, and the format serves both or it serves neither. The **security**
consumer wants tamper-evidence and completeness: A09 M8's point is that exfiltration leaves forensic
evidence *even when it succeeds*. The **debugging** consumer wants deterministic replay: A03's point
is that agent failures are unreproducible without a transcript that says what the agent believed.
The schema below carries `ax_hash` for the second and `seq`/`prev` for the first.

### 7.2 Physical format

- **JSONL.** One object per line, UTF-8, `\n`-terminated, no trailing commas, no pretty-printing.
- **Path:** `~/Library/Application Support/BlackGlass/audit/<sid>.jsonl` on macOS,
  `$XDG_STATE_HOME/blackglass/audit/<sid>.jsonl` on Linux. Directory `0700`, file `0600`, created
  with `O_CREAT|O_EXCL`.
- **Not inside the Chromium profile directory.** The renderer must have no path to it (A09 §5.2).
- **The core is the sole writer.** The engine never opens it; engine-side events reach the log by
  being sent to the core as events and logged there. This makes §1.3-B's atomicity result a margin
  rather than a dependency, and it means a compromised renderer cannot forge or truncate records.
- **Record cap 8 KiB.** Measured atomicity held to 64 KiB (§1.3-B), so 8 KiB is an 8x margin. Oversize
  fields are truncated with `"trunc": true` — never dropped silently.
- **One file per session.** No rotation within a session; retention default 30 days, swept at startup.

### 7.3 Schema

Every record. Field order is fixed for readability of raw files and for byte-stable hashing (§7.6).

| Field | Type | Req | Meaning |
|---|---|---|---|
| `v` | int | ✓ | schema version; `1` |
| `seq` | int | ✓ | per-session, starts at 0, **strictly +1** — a gap means loss or tampering |
| `ts` | string | ✓ | RFC 3339 UTC, ms precision |
| `mono_ms` | int | ✓ | monotonic ms since session start; survives wall-clock jumps |
| `sid` | string | ✓ | session id, 26-char ULID-style |
| `tid` | string\|null | ✓ | agent task id; `null` for human/system records |
| `actor` | enum | ✓ | `human` \| `agent` \| `system` \| `page` |
| `state` | enum | ✓ | arbiter state **after** the record's transition (`S0`..`S5` names) |
| `act` | string | ✓ | namespaced verb; the closed set in §7.7 |
| `result` | enum | ✓ | `ok` \| `denied` \| `forbidden` \| `error` \| `dropped` \| `interrupted` |
| `origin` | string\|null |  | page origin at the time, sanitized |
| `target` | object\|null |  | `{kind, role?, name?, url?, selector_hash?, path?}` |
| `gate` | object\|null |  | `{id, class, verdict, method, latency_ms, armed_ms}` |
| `ax_hash` | string\|null |  | first 16 hex of SHA-256 over the AX snapshot the action was grounded on |
| `budget` | object\|null |  | counters at the time, e.g. `{nav: 4, xnav: 1, cmds: 812}` |
| `err` | string\|null |  | short code, never a stack trace |
| `trunc` | bool |  | present and `true` only when a field was truncated |
| `prev` | string\|null |  | hash chain, §7.6; omitted in v1 |

**`act` is a closed set.** An open-ended string field is how audit logs become ungreppable. The v1
set: `session.start`, `session.end`, `agent.attach`, `agent.attach.refused`, `agent.detach`,
`agent.act`, `agent.act.rejected`, `agent.done`, `agent.abort`, `agent.budget_exhausted`,
`control.takeover`, `control.handback`, `control.pause`, `control.resume`, `gate.ask`, `gate.answer`,
`gate.refused`, `engine.crash`, `engine.recovered`, `page.navigate`, `page.download`.

### 7.4 Durability policy, chosen from §1.3-A

Three tiers, and the tiering is the point — a uniform policy is either too slow or too lossy.

| Tier | Records | Syscall | Measured cost |
|---|---|---|---|
| **Routine** | `agent.act` (AUTO/NOTIFY), `page.navigate` | buffered `write` only | p50 **5.2 µs**, p99 85 µs |
| **Decision** | every `gate.*`, every `control.*`, `agent.attach`/`abort`, `engine.crash` | `write` + `fsync` | p50 **64 µs**, p99 **1.07 ms** (≈6% of a 16.65 ms frame) |
| **Session** | `session.start`, `session.end`, `SIGINT`/`SIGTERM` | `write` + **`F_FULLFSYNC`** | p50 4.62 ms, p99 **15.72 ms**, max 20.21 ms |

**`F_FULLFSYNC` must never be called on a routine or decision record.** On this host its p99 is
15.72 ms against a measured p50 frame gap of 16.65 ms — one call can eat an entire frame, and its max
of 20.21 ms exceeds one outright. `fsync` alone does not flush the drive's write cache on macOS, so
`F_FULLFSYNC` is the only true durability barrier; the policy above accepts that a power-loss window
exists for decision records and closes it at session end. That trade is stated so it can be
overridden by anyone who disagrees, via a `--audit-durability=paranoid` flag that promotes decision
records to `F_FULLFSYNC` and accepts the frame cost.

### 7.5 What is never written

Values, not just fields. The audit log is a permanent artifact and the most likely place for a
secret to outlive the session that created it.

Never: any value from a field matching `REDACT_SELECTOR` (§6.4); clipboard contents in either
direction; cookie or `Authorization` values; page text bodies; full AX snapshots. Instead: presence,
field *name*, length, and a hash. A record says `password: <redacted len=11>`, never the password.

Query strings deserve a specific rule, because they are simultaneously the highest-value forensic
field (exfil channel #1 is a URL) and a common carrier of session tokens. The rule: log the full URL
for the origin the session started on and for any `CONFIRM`ed navigation (the human already saw it in
the preview); elsewhere log scheme + host + path and a hash of the query, with parameter *names*
preserved and values redacted. This keeps the forensic signal — "the agent tried to send an 800-byte
parameter to a host it had never contacted" — without warehousing the payload.

**Audit is mandatory for agent sessions.** `--no-audit` is refused when `--agent` is present, exits
non-zero, and says why. An unaudited agent session is not a supported configuration; A09 M8 is a
control, not a preference.

### 7.6 Tamper-evidence: `seq` in v1, hash chain in v1.1

v1 ships `seq` gap-detection only. It catches truncation and deletion, which is most of what a
local audit log realistically faces, and it costs nothing.

A `prev` hash chain (each record carrying SHA-256 of the previous record's canonical bytes) upgrades
that to detecting *edits*. It is cheap at runtime — **1.67 µs/record measured** (§1.3-C), 0.017% of a
core at 100 rec/s — but it is not cheap architecturally: this workspace depends on exactly `libc` and
`flate2` (`Cargo.toml:12-14`), and SHA-256 means either a new dependency or ~150 lines of
hand-rolled crypto. Hand-rolling is not out of character here (`b64.rs`, the kitty encoder, the JSON
reader in `bg-proto` are all deliberate hand-rolls), but hand-rolled crypto is a different risk class
from hand-rolled base64, and the honest v1 answer is to ship neither and revisit.

If adopted: `sha2` (RustCrypto) is `MIT OR Apache-2.0`, compatible with this workspace's
`MIT OR Apache-2.0` (`Cargo.toml:8`). Note also that a chain is only tamper-*evident*, not
tamper-*proof*, against an attacker who can rewrite the whole file — which anyone with the user's uid
can. The property it buys is that selective edits become detectable, not that the log is immutable.

### 7.7 Worked records

Session start:

```json
{"v":1,"seq":0,"ts":"2026-07-31T22:14:03.221Z","mono_ms":0,"sid":"01K1B9T0Q7XR4","tid":null,"actor":"system","state":"SOLO","act":"session.start","result":"ok","origin":null}
```

Agent navigates within the origin — routine tier, no fsync:

```json
{"v":1,"seq":41,"ts":"2026-07-31T22:15:11.882Z","mono_ms":68661,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"agent","state":"AGENT","act":"agent.act","result":"ok","origin":"https://developer.mozilla.org","target":{"kind":"node","role":"link","name":"fetch()"},"ax_hash":"9f2c1ab30de44571","budget":{"nav":3,"xnav":0,"cmds":804}}
```

Gate raised for a cross-site navigation — decision tier, fsync:

```json
{"v":1,"seq":42,"ts":"2026-07-31T22:15:12.004Z","mono_ms":68783,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"agent","state":"GATE","act":"gate.ask","result":"ok","origin":"https://developer.mozilla.org","target":{"kind":"url","url":"https://api-collect.example.net/r?d=<q:hash=3b81f0c2 len=812 names=[d]>"},"gate":{"id":"g-7","class":"CONFIRM"},"ax_hash":"9f2c1ab30de44571"}
```

Human denies it — note `latency_ms`, which is how habituation gets measured rather than assumed:

```json
{"v":1,"seq":43,"ts":"2026-07-31T22:15:19.510Z","mono_ms":76289,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"human","state":"AGENT","act":"gate.answer","result":"denied","gate":{"id":"g-7","class":"CONFIRM","verdict":"deny","method":"key","latency_ms":7506,"armed_ms":250}}
```

A forbidden action — the signature of injection, and the record a reviewer greps for first:

```json
{"v":1,"seq":44,"ts":"2026-07-31T22:15:20.117Z","mono_ms":76896,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"agent","state":"AGENT","act":"gate.refused","result":"forbidden","origin":"https://developer.mozilla.org","target":{"kind":"node","role":"textbox","name":"Password","selector_hash":"c41d0e77"},"err":"E_FORBIDDEN_CREDENTIAL_FIELD"}
```

Human takeover — decision tier, fsync:

```json
{"v":1,"seq":45,"ts":"2026-07-31T22:15:21.006Z","mono_ms":77785,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"human","state":"HUMAN","act":"control.takeover","result":"ok","origin":"https://developer.mozilla.org","target":{"kind":"none"},"budget":{"nav":3,"xnav":0,"cmds":829}}
```

The interrupted action, recorded honestly rather than as a success:

```json
{"v":1,"seq":46,"ts":"2026-07-31T22:15:21.007Z","mono_ms":77786,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"agent","state":"HUMAN","act":"agent.act","result":"interrupted","err":"E_HUMAN_CONTROL"}
```

Handback with re-grounding — the `ax_hash` differs from seq 41, which is exactly the point:

```json
{"v":1,"seq":52,"ts":"2026-07-31T22:16:40.330Z","mono_ms":157109,"sid":"01K1B9T0Q7XR4","tid":"t-02","actor":"human","state":"AGENT","act":"control.handback","result":"ok","origin":"https://developer.mozilla.org","ax_hash":"7e0aa4c9b1d2f803"}
```

### 7.8 Reading it back

`blackglass audit --session <sid>` renders the JSONL as a human timeline; `--gates` filters to
decisions; `--json` passes through for `jq`. Two things it must do that are easy to forget: verify
`seq` continuity and say so loudly on a gap, and pass **every** string through
`unicode::sanitize_for_terminal` before printing. The log contains attacker-chosen origins and
accessible names; a viewer that renders them raw re-introduces A09 §1's escape-injection at exactly
the moment a security reviewer is reading.

---

## 8. Protocol additions

Framing is unchanged (`crates/bg-proto/src/lib.rs:1-9`); every message is a flat JSON object with a
`t` field, parseable by the existing hand-rolled reader.

**Core ↔ engine** (additions to `apps/engine/src/main.js`):

| Dir | `t` | Payload | Trigger |
|---|---|---|---|
| C→E | `classifyTarget` | `id, kind, selector\|nodeId, url?` | before dispatching any agent action |
| E→C | `targetClass` | `id, class, role, name, formFields[], redacted[], resolved: bool` | reply |
| C→E | `axSnapshot` | `id, scope?` | handback, attach, staleness check |
| E→C | `axSnapshotResult` | `id, hash, nodes[]` | reply |
| C→E | `agentInput` | as existing `input`, **plus `gen`** | agent-originated input only |
| E→C | `paymentRequest` | `origin, total?` | preload shim observes `new PaymentRequest` |

`gen` is the takeover generation counter. Ordering over a stream socket already prevents most
reordering; `gen` closes the residual case where the engine has buffered commands it has not yet
dispatched, and it costs one integer compare.

**Agent ↔ core RPC** (new; JSON-RPC 2.0 over the Unix socket at `fds[2]`):

| Dir | Method / notification | Notes |
|---|---|---|
| A→C | `attach{agent_id, task}` | refused without a grant (§6.7) |
| A→C | `navigate` `click` `type` `read_ax` `find` `scroll` `screenshot` | classified per §6.2 |
| A→C | `detach` | |
| C→A | `control.seized` (notify) | takeover; all in-flight calls resolve `E_HUMAN_CONTROL` |
| C→A | `control.returned{ax_hash, ax}` (notify) | **must** carry the fresh snapshot (I4) |
| C→A | `control.paused` / `control.resumed` (notify) | |
| C→A | `gate.pending{id, class}` (notify) | so the agent can stop polling and wait |
| C→A | errors | `E_NO_GRANT` `E_HUMAN_CONTROL` `E_PAUSED` `E_NO_TASK` `E_DENIED` `E_FORBIDDEN` `E_BUDGET` `E_STALE_AX` |

The RPC socket inherits A09 §4.2's placement and permissions rules and §4.3's peer authentication
unchanged — it is a second local control-plane socket and must not be treated as a lesser one. It is
strictly more privileged than the engine socket, since it can request state transitions.

---

## 9. Failure modes

| Failure | Handling |
|---|---|
| Human takes over mid-action | Queue cleared; ≤`K`=8 commands may already be committed (§3.8); action recorded `interrupted`; async effects (a started navigation) complete rather than being rolled back |
| Engine crashes while a gate is open | Gate torn down as **denied**; `S3 → S4`; B08 recovery; human resumes explicitly |
| Core crashes while a gate is open | Engine-side timeout fires the deny callback (D05 §10) so the page does not hang; on restart the log ends without a `gate.answer`, which is itself the finding |
| Agent disconnects while in `S1` | `S1 → S0`; **the human is not left locked out** — this is why `S0` allows human input |
| Agent disconnects while in `S3` | Gate cancelled as denied; `S3 → S0` |
| Agent replays an answered gate id | Rejected; engine rejects unknown/answered ids (D05 §10) |
| Agent floods actions | Command budget (§3.6); RPC read is one message per iteration, so it cannot starve stdin |
| Wall clock jumps (NTP, sleep) | `mono_ms` is authoritative for ordering and for gate timers; `ts` is for humans |
| Terminal resized mid-gate | Gate re-renders at the new width (pure function, §10.2); `C` recomputed on `SIGWINCH` only (D06 §3.3) |
| Page loops `alert()` | `ctrl+q` is checked before gate routing (I9, D05 §9 rule 3) |
| Audit disk full | **Refuse to continue the agent session** — abort to `S5`, tell the user. An agent running unaudited is the configuration §7.5 forbids |

---

## 10. Test plan

Everything here runs in CI on this host with no engine, no agent, and no display — which is a design
requirement, not a convenience, given §12's blockers.

### 10.1 The arbiter is a pure function

```rust
fn arbiter_step(s: State, e: Event, ctx: &Ctx) -> (State, Vec<Effect>, AuditRecord)
```

- **Totality:** iterate `State × Event` (6 × 17 = 102 cells) and assert `arbiter_step` returns for
  every one. No panics, no `unreachable!()`.
- **Table conformance:** assert each of the 102 next-states equals §3.4a. This is the test that makes
  the table normative rather than decorative.
- **I1:** for every state, `!(human_input_to_page(s) && agent_may_act(s))`.
- **I3 (the important one):** property test over random event sequences up to length 200 — no
  sequence containing no `H_HANDBACK` and no `A_ATTACH` ever ends in `S1` having started in `S2`.
- **I6:** no `Event` whose source is `agent` maps to `S1` from `S2`/`S4`.
- **I7:** every call returns exactly one `AuditRecord`.
- **Takeover cancels:** with 40 commands queued, one `H_INTENT` leaves the queue empty and `gen`
  incremented.
- **Overshoot bound:** commands dispatched after an `H_INTENT` in the same iteration ≤ `K`.

### 10.2 Classification and rendering are pure functions

- `classify_input(Event, buttons_down) -> Intent|Passive` — table test over §4.2, with a specific
  case asserting a bare `Mouse{Move}` is `Passive` (**this is the F1 regression test**) and a
  `Mouse{Move}` with `buttons_down` is `Intent`.
- `classify_action(action, TargetAttrs) -> Class` — table test over §6.2, plus: unresolved target →
  never `AUTO`; stale `ax_hash` → escalates one step; the `REDACT_SELECTOR` identity test asserting
  redaction and gating read the same constant.
- `render_gate(&Gate, cols, rows) -> Vec<String>` — golden tests at 200×24 and 80×24 (D05 §9's
  required shape). Adversarial inputs: an origin containing RTL/bidi overrides (D06 F4); an
  accessible name of 4000 chars; a name containing `\x1b]52;c;AAAA\x07`; a name of CJK wide
  characters asserting **display width** not char count (D06 F3). Assert no line exceeds `cols`, the
  box closes, and no output byte is `<0x20` except the intended SGR sequences.

### 10.3 Audit

- `seq` strictly increments across a synthetic 10,000-record session, no gaps.
- No record exceeds 8 KiB; oversize input yields `trunc: true`.
- **Secret-leak fuzz:** drive the recorder with values matching A09 M4's patterns (`AKIA…`, `ghp_…`,
  `sk-…`, `xox[baprs]-…`, `eyJ…`, `-----BEGIN … PRIVATE KEY-----`) in every string-bearing field and
  assert none appears in the output bytes.
- Every emitted `act` is in §7.3's closed set.
- Durability tiering: assert `fsync` is called for decision records and **not** for routine ones, and
  `F_FULLFSYNC` **only** at session boundaries (a counting fake around the fd).
- Round-trip: `blackglass audit --json` reproduces the input records byte-identically.

### 10.4 What cannot be tested here

Real takeover latency against a real page, real gate behaviour against real Chromium, and any visual
verification of §5's mockups. §12.

---

## 11. Implementation order

1. `classify_input` + the `buttons_down` latch (§4.2) — pure, testable, fixes F1's premise, useful
   before any agent exists.
2. Decoder `0x1c..=0x1f` arm (F2) — an input-injection bug independent of E05.
3. `Arbiter` as a pure `arbiter_step` with the §3.4a table and §10.1's tests — **no RPC yet**. This is
   the whole safety argument and it can be fully proven before a single agent connects.
4. `Disposition` return from `handle_event` (F3) and arbiter interposition; `S0` only.
5. Audit writer with §7.4's tiering and §10.3's tests; wire `session.start`/`session.end`.
6. Ownership indicator (§5) as a field-priority change, with golden tests.
7. Gate as the sixth `Prompt` variant (D05 §9), `render_gate` golden tests, engine-side timeout.
8. `classifyTarget`/`targetClass` in the engine; the shared `REDACT_SELECTOR` constant.
9. RPC socket as `fds[2]`; `attach`/`detach`/`read_ax` only.
10. Action verbs, `ActionPlan` expansion with `K = 8`, budgets.
11. `blackglass audit` reader.

Steps 1–7 deliver a complete, tested arbiter and audit trail with **no agent in the system**. That
ordering is deliberate: the control model should be provably correct before there is anything to
control, because the alternative is debugging arbitration and agent integration simultaneously.

---

## 12. Explicitly UNVERIFIED

- **There is no agent RPC in this repository.** Every RPC message, error code, and latency claim
  about the agent path is design, not measurement. The only paths I exercised are the ones the human
  already uses.
- **No end-to-end takeover latency number exists.** I bounded the components I could (§4.4) and
  refuse to synthesise a total from them.
- **No gate was ever rendered against a real page.** Chromium child processes fail under the agent
  Bash sandbox (`bootstrap_look_up … Permission denied`, mission brief), so I ran no engine. This is
  why §10.2 insists on pure rendering functions.
- **No visual verification of §5 or §6.5.** The machine is at a lock screen. The mockups are
  hand-constructed and column-counted, not screenshotted.
- **iTerm2 3.6.9 remains unverified** (TCC blocks automation, mission brief). All terminal claims here
  derive from the mission's Ghostty/Apple Terminal matrix.
- **`O_APPEND` atomicity (§1.3-B) is 3000 trials per size on APFS on this host.** It is evidence, not
  a guarantee, and §7.2 is deliberately built so nothing depends on it.
- **F5 is a real hole in the takeover model, not a hypothetical.** Any process that can write to the
  pty master — `tmux send-keys`, `expect`, `script` — produces bytes the arbiter will classify as
  human intent. There is no fix at the tty layer. An agent integrated that way bypasses every gate in
  §6 and appears in the audit log as `actor: "human"`.
- **The heuristics in §6.4 are heuristics.** They will miss destructive actions labelled neutrally.
  The design compensates by gating *all* form submits, so nothing depends on the regex being
  complete — but a reader should not mistake §6.4 for a detector.
- **The 750 ms `STRONG` arming delay and the 60 s gate timeout are chosen, not measured.** No
  usability testing has been done (D10 specifies the method; it has not been run).

---

## 13. Licence

The workspace declares `MIT OR Apache-2.0` (`Cargo.toml:8`). **There is no `LICENSE` file at the repo
root** — worth fixing before any publication, independently of this mission.

This document proposes **no third-party code**. Dependencies remain `libc` and `flate2`
(`Cargo.toml:12-14`). The only crate this design could imply is `sha2` for the optional hash chain
(§7.6), which is RustCrypto and dual-licensed `MIT OR Apache-2.0` — compatible — and which §7.6
recommends **deferring past v1** rather than adopting. JSON-RPC 2.0 (§8) is a specification, not
code; the existing hand-rolled reader in `bg-proto` already handles the flat-object subset needed.

No code was copied from any source. The `REDACT_SELECTOR` list in §6.4 is cited from A09 §7.3 within
this repository and is reused by reference, not duplicated — which is the point of §6.4.
