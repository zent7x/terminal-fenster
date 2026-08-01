# D04 — Focus and Clipboard Specification

**Mission:** specify OSC 52 write, adjudicate OSC 52 read, audit the shipped bracketed-paste
implementation, and define the exact user-consent model for a page touching the clipboard.

**Ownership note.** Per the swarm rules this document writes to no source file. Every defect below
lives in commander-owned code (`crates/tf-term/`, `apps/cli/`, `apps/engine/src/main.js`) and is
therefore *described*, with the exact change, exact test vector, and exact file:line — not applied.
The one artifact I created is a throwaway probe harness outside the repo, at
`/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/d04probe/`,
which path-depends on `crates/tf-term` read-only and is reproduced verbatim in §9 so it can be
re-run or promoted into `crates/tf-term/src/input.rs` tests by whoever owns that file.

---

## 0. Executive summary

Five findings, four of them measured on this machine against the shipped code.

| # | Severity | Finding | Evidence |
|---|---|---|---|
| **F1** | **CRITICAL** | The input decoder has no OSC/DCS/APC/PM state. Any string-terminated reply arriving on stdin is decoded into individual **keystrokes and delivered to the page**. An `OSC 52` clipboard-read reply becomes the base64 of the user's clipboard typed into whatever input the page has focused. | §3.1, measured |
| **F2** | **HIGH** | A pasted payload containing a literal `CSI 201~` truncates the paste in our decoder, and the bytes after it are parsed as input. I measured a paste synthesizing a **left mouse click at page coordinates the attacker chose**. A raw `0x11` in the same position exits Terminal-Fenster (`main.rs:572`). | §3.2, measured |
| **F3** | **HIGH** | Enabling `DECSET 2004` **disables Ghostty's copy/paste-attack protection** — verified from the shipped binary's own documentation for `clipboard-paste-bracketed-safe`, default `true`. The terminal has explicitly delegated paste safety to Terminal-Fenster, and Terminal-Fenster currently implements none. | §3.3, verified primary source |
| **F4** | **HIGH** | Paste is delivered as synthetic per-character `char` key events (`main.rs:645`, `main.js:213`), not as a DOM `paste` event. Consequences: no `onpaste` handler ever fires; no size cap anywhere in the chain; an unterminated paste wedges input **permanently** with unbounded memory growth; no `\r\n` normalisation; a trailing newline is retyped as Enter, which implicitly submits forms. | §3.4, measured (wedge) |
| **F5** | **MEDIUM** | `DECSET 1004` focus reporting is enabled (`tty.rs:152`) and decoded (`input.rs:270`) but then **discarded** (`main.rs:653`). The engine never learns about focus. This is the root cause of the async Clipboard API's `NotAllowedError: Document is not focused` risk in offscreen rendering, and it forfeits a free idle-power win. | §5 |

**Single most actionable recommendation:** fix F1 before anything else in this mission. It is nine
lines of state machine in `input.rs`, it is the only finding that leaks user secrets *out* of the
trust boundary rather than merely letting bad data *in*, and it also fixes a live non-security bug —
today a stray kitty-graphics `APC` reply or an `OSC 11` colour reply types visible garbage into the
page (§3.1, both measured).

---

## 1. Evidence base

Everything in this section was run on the target machine (macOS 26.1, Apple M4) during this mission.

### 1.1 Ghostty 1.3.1 clipboard defaults — from the shipped binary

```
$ /Applications/Ghostty.app/Contents/MacOS/ghostty +show-config --default | grep -iE 'clip|paste'
clipboard-read = ask
clipboard-write = allow
clipboard-trim-trailing-spaces = true
clipboard-paste-protection = true
clipboard-paste-bracketed-safe = true
```

With `--docs`, the binary's own documentation for the two that matter:

```
# Require confirmation before pasting text that appears unsafe. This helps
# prevent a "copy/paste attack" where a user may accidentally execute unsafe
# commands by pasting text with newlines.
clipboard-paste-protection = true

# If true, bracketed pastes will be considered safe. By default, bracketed
# pastes are considered safe. "Bracketed" pastes are pastes while the running
# program has bracketed paste mode enabled (a setting set by the running
# program, not the terminal emulator).
clipboard-paste-bracketed-safe = true
```

This is the F3 finding stated by the vendor. The protection exists; it is switched off for any
program that sets mode 2004; Terminal-Fenster sets mode 2004 unconditionally at `tty.rs:153`.

Corroborating strings in the same binary confirm the enforcement path is real, not vestigial:

```
$ strings -a /Applications/Ghostty.app/Contents/MacOS/ghostty | grep -n 'unsafe paste'
204097:info(surface): potentially unsafe paste detected, rejecting until confirmation
```

### 1.2 Ghostty terminfo — the clipboard and paste capabilities we may rely on

```
$ TERMINFO=/Applications/Ghostty.app/Contents/Resources/terminfo infocmp -x -1 xterm-ghostty \
    | grep -E 'Ms=|BE=|BD=|PS=|PE='
	BD=\E[?2004l,
	BE=\E[?2004h,
	Ms=\E]52;%p1%s;%p2%s\007,
	PE=\E[201~,
	PS=\E[200~,
```

`Ms` is the standard terminfo user capability for "set clipboard". Its presence is the portable
feature test for OSC 52 write, and Ghostty advertises the **BEL** terminator form. A06 §6.1 records
that Ghostty's *reply* construction uses `ESC \` (ST). Both terminators must be accepted on both
sides; only ST should be emitted by us (§4.2).

### 1.3 iTerm2 3.6.9 — clipboard policy, from the shipped binary

iTerm2 is installed at `$HOME/Applications/iTerm.app` (not `/Applications`, which is why
earlier probes missed it), `CFBundleShortVersionString = 3.6.9`.

```
$ strings ~/Applications/iTerm.app/Contents/MacOS/iTerm2 | grep -iE 'clipboard|osc52'
-[PTYSession supportsOSC52]
Claim to support osc52 but will actually prompt
Clipboard contents reported
Share clipboard contents with app in terminal?
AllowClipboardAccess
PhonyAllowSendingClipboardContents
_allowClipboardAccessFromTerminal
_allowsSendingClipboardContents
Clipboard access allowed
Clipboard access explicitly denied
Clipboard access implicitly denied and prompt disabled
Clipboard access denied for CopyToClipboard
```

Three things follow. **Write** is gated by a boolean preference `AllowClipboardAccess`, with a
tri-state outcome (allowed / explicitly denied / implicitly denied with the prompt suppressed).
**Read** is a separate, per-request consent — the literal prompt string is *Share clipboard contents
with app in terminal?* — backed by `PhonyAllowSendingClipboardContents`. And the symbol
`-[PTYSession supportsOSC52]` sitting next to the string *Claim to support osc52 but will actually
prompt* is iTerm2 telling us in its own words that **advertising OSC 52 in DA1 does not mean a
request will be honoured**. Any capability detection that treats DA1 parameter `52` as "clipboard
works" is wrong (see A04 §"primary", which reads `52` as clipboard in Ghostty's `?62;22;52c`).

The machine is at a lock screen and iTerm2 automation is blocked by macOS TCC, so the *runtime*
default of `AllowClipboardAccess` on this install is **UNVERIFIED**; the prefs domain
`com.googlecode.iterm2` is empty (never launched), and the default is registered in code rather than
in a shipped plist (`grep -rl AllowClipboardAccess ~/Applications/iTerm.app/Contents/Resources`
returns nothing). §10 gives the probe.

### 1.4 Apple Terminal 465

A06 measured no reply and no clipboard effect for both OSC 52 write and read, and no `Ms` capability
is reachable. Treat OSC 52 as absent. This is not a bug to work around; it is the reason the
read-substitute in §4.4 must not depend on OSC 52 at all.

### 1.5 The clipboard/paste capability matrix, consolidated

| | Ghostty 1.3.1 | iTerm2 3.6.9 | Apple Terminal 465 |
|---|---|---|---|
| Bracketed paste (2004) | **yes** (`BE`/`BD` in terminfo) | **yes** (A06: `CSI ?2004$p` → `CSI ?2004;2$y`) | UNVERIFIED (no DECRQM; behavioural test in §10) |
| Focus reporting (1004) | **yes** (A06) | **yes** (A06) | UNVERIFIED (no DECRQM) |
| OSC 52 **write** | **allowed, no prompt** (verified §1.1) | pref-gated `AllowClipboardAccess` (§1.3); default UNVERIFIED | **not implemented** (A06, measured) |
| OSC 52 **read** | `ask` → modal that wedges the app (§1.1 + A06 §6.2) | per-request consent prompt (§1.3); A06 measured **no reply** | **not implemented** (A06, measured) |
| Paste-attack protection | **disabled while we hold 2004** (§1.1) | "Warn before pasting multiline" pref, UNVERIFIED | none |

Read the last row across: on the reference terminal we get the *most* permissive clipboard write in
the matrix **and** the paste protection turned off, simultaneously, by default. That combination is
what makes §7's consent model load-bearing rather than ceremonial.

---

## 2. What is already correct, and should not be disturbed

Three properties of the shipped code are right and are the foundation the rest of this spec builds
on. Naming them matters so that a fix does not regress them.

The paste body is never re-interpreted as escape sequences. `Decoder::step` checks `self.in_paste`
*before* it checks for `0x1b` (`input.rs:166-168`), and `step_paste` scans only for the literal
terminator. `input.rs:735` tests exactly this. That is the single most important security property
of a bracketed-paste implementation and it is present and covered.

The partial-terminator carry is correct. `step_paste` retains `END.len() - 1 == 5` bytes when no
terminator is found (`input.rs:205-211`), which is precisely the maximum length of a prefix of
`ESC [ 2 0 1 ~` that is not itself a match. Split-read handling is tested at `input.rs:726`.

Base64 is already right for this job. `b64::encode` (`b64.rs:7,12`) uses the standard RFC 4648
alphabet with `=` padding, which is exactly what Ghostty constructs and expects (A06 §6.1). No new
encoder is needed. Note also that the crate contains **no base64 decoder** — and per §4.4 none
should ever be added, because the only reason to decode base64 from a terminal is to parse an
OSC 52 read reply, which is the thing we are forbidding.

---

## 3. Findings

### 3.1 F1 — CRITICAL: string-terminated replies on stdin are decoded as keystrokes

`Decoder::step` (`input.rs:176-193`) dispatches on `self.buf[1]` with exactly two arms — `[` for CSI
and `O` for SS3 — and a catch-all that treats everything else as legacy `ESC <char>` = Alt+char. The
introducer bytes for OSC (`]`), DCS (`P`), APC (`_`) and PM (`^`) all fall into that catch-all. The
decoder emits Alt+introducer, then decodes the entire body one printable character at a time, each
carrying `text: Some(...)`, and finally chews the string terminator.

Measured against the shipped crate:

```
label                          input   events   reconstructed as page text
APC kitty graphics reply         12 B      10   "_Gi=31;OK\"
DCS DECRQM-style reply           12 B      10   "P1$r2004h\"
OSC 52 read reply (ST)           21 B      19   "]52;c;cGFzc3dvcmQ=\"
OSC 52 read reply (BEL)          20 B      19   "]52;c;cGFzc3dvcmQ=g"
PM string                         8 B       6   "^junk\"
```

`cGFzc3dvcmQ=` is base64 for `password`. The clipboard survives the round trip intact and arrives at
the page as ordinary text input.

Two distinct consequences, and both matter.

**Security.** This is A09's threat **T4** ("Exfiltration via OSC 52 read reply on our stdin"),
promoted from theoretical to demonstrated against the current build. A09 §3 already establishes the
correct doctrine — *"stdin is untrusted input"* — but the doctrine is not implemented in the steady-state
decoder. The mitigating factor is that Terminal-Fenster never *sends* `OSC 52 ; c ; ?`, so an attacker must
find another way to make the terminal emit one. That is a real constraint but not a guarantee: a
multiplexer, a sibling process sharing the tty, a shell hook, a terminal that emits an unsolicited
reply, or a future Terminal-Fenster feature that queries the clipboard all produce the same bytes. Defence
belongs at the parser, which is one place, rather than at every possible source, which is unbounded.

**Correctness, today, with no attacker.** The `APC` line above is a kitty graphics response. The
project brief records Ghostty replying `ESC_Gi=31;OK ESC\` to our own capability probe. C05 F3
correctly hardens the *handshake* reader in `caps::detect` and even classifies
`\e_Gi=99;OK\e\\` as `UnsolicitedSeq` — but that sieve runs only during detection, and its output
(`probe.pending_user_input`) is handed to `Decoder::decode`, which has no such classifier. Any reply
that arrives *after* the input loop starts — a late reply that missed C05's deadline, a re-probe on
resize (C05 F11), a multiplexer answering on its own schedule (C05 F8) — types visible junk into the
page. This is a live bug independent of the security story, and D04's finding is complementary to
C05 F3 rather than a duplicate of it: different function, different code path, different lifetime.

There is also a sharp edge in the BEL variant worth calling out. `0x07` is not consumed as a
terminator; it falls into the `0x01..=0x1a` control range at `input.rs:189` and becomes **Ctrl+G**.
The same arm maps `0x11` to **Ctrl+Q**, which `main.rs:572` treats as *quit the browser*, and `0x12`
to Ctrl+R, *reload*. Any raw C0 byte that reaches `step_plain` is a browser command.

**Fix (core-owned; `crates/tf-term/src/input.rs`).** Add a string-sequence arm to `step`:

```rust
match self.buf[1] {
    b'[' => self.step_csi(),
    b'O' => self.step_ss3(),
    // OSC / DCS / APC / PM / SOS: consume through BEL or ST, never emit key events.
    b']' | b'P' | b'_' | b'^' | b'X' => self.step_string_seq(),
    _   => { /* existing Alt+char */ }
}
```

`step_string_seq` scans for `0x07` or `ESC \`, returns `Step::NeedMore` if neither is present yet
(with a length cap — see below), drains the whole sequence, and returns
`Step::Event(Event::TerminalReply(Vec<u8>))`. The CLI must then **drop** every `TerminalReply` and
count it. Three counters, all cheap and all worth having in `doctor` output:
`stdin_reply_dropped_total`, and specifically `stdin_osc52_reply_dropped_total`, plus
`stdin_reply_truncated_total`. A non-zero `osc52` counter is the observable signature of an
exfiltration attempt and belongs in the diagnostic bundle (sanitised per B08 §"the bundle is itself
an attack surface").

Two details that a naive implementation gets wrong. First, an unterminated string sequence must not
buffer forever — cap it at 8 KiB, and on overflow discard the buffer, emit
`Event::Unknown(truncated)`, and resume normal decoding; otherwise F1's fix reintroduces F4's wedge
in a new place. Second, `ESC X` (SOS) is included above because xterm treats it as a string
introducer; if the commander prefers a minimal change, `]`, `P`, `_`, `^` cover every sequence any
of the three target terminals actually emits, and `X` can be left in the Alt+char arm.

**Test vectors:** §9, T1–T5.

### 3.2 F2 — HIGH: an embedded `CSI 201~` turns a paste into synthetic input

`step_paste` (`input.rs:196-215`) terminates on the first literal `ESC [ 2 0 1 ~` in the stream and
hands the remainder straight back to the general decoder. Measured:

```rust
decode(b"\x1b[200~x\x1b[201~\x1b[<0;5;5M")
  -> [ Paste("x"),
       Mouse { kind: Down, button: Left, x: 5, y: 5, mods: {} } ]

decode(b"\x1b[200~safe\x1b[201~\x1b[<0;100;100M\x1b[<0;100;100m")
  -> [ Paste("safe"), Mouse Down @ (100,100), Mouse Up @ (100,100) ]
```

A paste produced a complete synthetic click at attacker-chosen page coordinates. In a browser that
is clickjacking with no iframe and no overlay: the "Confirm payment", "Allow", "Download", or
"Delete account" button gets pressed by text the user pasted. Swap the mouse report for a raw `0x11`
and the session exits; `0x12` reloads.

This is the CVE-2021-31701 / CVE-2021-37326 / CVE-2021-40147 family that A09 §1.2 already catalogues
from the CyberArk *Don't Trust This Title* research, reproduced against Terminal-Fenster's own decoder.

The usual defence is that the terminal filters `CSI 201~` out of pasted data before bracketing it,
and A06 §5 asserts kitty and Ghostty do. **I could not verify that claim in this mission** — driving
a real paste needs UI automation and the machine is at a lock screen (§10). Even if it holds for
these two terminals today, it is a property of every terminal, multiplexer, and version in the
deployment matrix, and it is not something a browser should stake page integrity on.

**Fix (core-owned; `input.rs` + `apps/cli/src/main.rs`), defence in depth, two independent rules.**

The precise rule first, because it has no false positives: **a terminal cannot legitimately interleave
a mouse or focus report inside or immediately adjacent to a paste.** Track the read-chunk boundary in
the decoder and drop any `Mouse`, `FocusGained`, or `FocusLost` event whose bytes were consumed from
the same `decode()` call that produced a `Paste` event, after that `Paste`. This kills the measured
attack exactly, and a real user cannot trip it because their click and their paste arrive in
different `read(2)` calls separated by human reaction time.

The coarser rule second, because it catches what the first misses: for **100 ms** after a paste
terminator, the CLI must not honour a global chord (`Ctrl+Q` at `main.rs:572`, `Ctrl+R` at
`main.rs:573`, `Alt+Left/Right` at `main.rs:582,586`) and must not forward `Mouse` events. Printable
text still flows. 100 ms is comfortably below human paste-then-click latency and comfortably above
the tail of a single split paste.

Neither rule requires the terminal to be trustworthy, which is the point.

### 3.3 F3 — HIGH: setting mode 2004 switches off Ghostty's paste protection

Verified in §1.1 from Ghostty's shipped documentation. `clipboard-paste-protection = true` prompts
before pasting text that "appears unsafe … text with newlines", and
`clipboard-paste-bracketed-safe = true` exempts pastes made while the program has bracketed paste
mode on. Terminal-Fenster sets `DECSET 2004` unconditionally (`tty.rs:153`) for the whole session.

The exemption is correct terminal design. A shell with bracketed paste on does not execute a pasted
newline, so the warning would be pure friction. But that reasoning **does not transfer to
Terminal-Fenster**, because of F4: we take the bracketed payload and retype it as per-character key events,
so a pasted `\n` becomes a synthetic Enter after all. We have taken the exemption without honouring
the premise it rests on.

The exposure is concrete and chains with A09's headline attack. A09 §1.2A establishes that on
Ghostty a page title is an unprompted clipboard-write primitive (`clipboard-write = allow`, verified
§1.1) and notes that `clipboard-paste-protection` only warns on newlines. F3 removes even that
warning for the entire duration of a Terminal-Fenster session — including for text the user pastes into
*other* windows later, if they copied it while Terminal-Fenster had the terminal.

**Fix.** Two parts, one of which is free.

The free part: fixing F4 (deliver a real `paste` ClipboardEvent instead of retyped keystrokes)
restores the premise the exemption assumes, because a DOM paste event does not submit a form on a
newline. That is the structural fix and it should be the primary one.

The explicit part: Terminal-Fenster must reimplement the protection it disabled, on the *outbound* side,
in §7's content gate — because the value we are protecting is not what the page receives, it is what
the user's system clipboard holds when they later paste into a shell.

Consider also emitting `DECRST 2004` when Terminal-Fenster is backgrounded or suspended (`SIGTSTP`), so a
suspended session does not leave a shell running with paste protection silently disabled. The
existing `RESTORE_SEQ` (`tty.rs:35`) already handles clean exit correctly; the gap is suspend/resume.

### 3.4 F4 — HIGH: paste is retyped as keystrokes, uncapped, and can wedge input permanently

Four defects in one path. Today `Event::Paste(text)` becomes, at `main.rs:645-651`:

```json
{"t":"input","kind":"key","action":"press","keyCode":"","text":"<entire payload>"}
```

and the engine (`main.js:211-221`) does:

```js
wc.sendInputEvent({ type: 'keyDown', keyCode: cmd.keyCode, modifiers: mods });   // keyCode: ""
if (cmd.text) { for (const ch of cmd.text) wc.sendInputEvent({ type: 'char', keyCode: ch, modifiers: mods }); }
wc.sendInputEvent({ type: 'keyUp', keyCode: cmd.keyCode, modifiers: mods });     // keyCode: ""
```

**(a) It is not a paste.** No `paste` ClipboardEvent fires, so `clipboardData` is absent and every
editor that owns its own paste handling — CodeMirror, Monaco, ProseMirror, Quill, Slate, Google Docs,
paste-to-upload dropzones, "paste your API key" flows, and every password manager integration — sees
nothing. This is not an edge case; it is most of the text editing on the modern web.

**(b) There is no size cap anywhere in the chain.** `paste_buf` (`input.rs:118`) grows without bound,
the JSON command carries the whole payload in one message, and the engine issues **one IPC
`sendInputEvent` per character**. A 1 MB paste is one million synchronous main-process calls; frames
stop while it drains. A06 §5 called for exactly this cap ("Cap payload size and stream it… a 50 MB
paste arriving as one blocking read will stall your event loop") and it was not implemented. The
per-character IPC cost is **UNVERIFIED** — measuring it needs Electron, which needs the sandbox
disabled (§10) — but the structure is inspectable and the cap is warranted regardless.

**(c) An unterminated paste wedges input permanently.** Measured:

```rust
let mut d = Decoder::default();
d.decode(b"\x1b[200~payload that never ends...");  // -> 0 events
d.decode(b"and the user keeps typing qqqqq");       // -> 0 events
```

`in_paste` is set at `input.rs:268` and cleared only at `input.rs:201`, on terminator. There is no
timeout, no cap, and no recovery. A terminal that drops the terminator, a `SIGWINCH` mid-paste, a
truncated SSH read (A07), or a hostile `CSI 200~` with no partner leaves the browser accepting no
keyboard input at all, forever, while buffering every subsequent byte the user types into a Vec that
only grows. There is no user-visible symptom other than "the browser stopped responding to the
keyboard", which is close to the worst possible failure mode to diagnose.

Note the shape of the fix that the codebase already prefers: `Decoder::flush_pending_escape` exists
precisely so the caller owns timeout policy rather than burying a `sleep` in the parser
(`input.rs:9-14`). The paste fix should match — `Decoder::flush_stale_paste(idle: Duration)` — not
introduce a timer inside the decoder.

**(d) No `\r\n` normalisation, and no control filtering.** A06 §5 requires `\r\n → \n` and lone
`\r → \n`; neither happens. `String::from_utf8_lossy` (`input.rs:202`) is a reasonable choice for
invalid UTF-8 but should be documented as lossy, and the U+FFFD count is worth a counter.

Combined with (a), the `\r` case is a security issue and not merely a fidelity one: a `char` event
carrying `\r` into a focused single-line `<input>` inside a `<form>` triggers Blink's implicit form
submission. A pasted payload with a trailing newline therefore **submits the form without the user
pressing Return** — the browser-side equivalent of the shell paste attack that F3 just disabled the
warning for. The mechanism is well-established Blink behaviour; the specific end-to-end result in
Electron OSR is **UNVERIFIED** (probe in §10, P3).

**Fix.** §6 specifies the replacement path in full: a new `kind: "paste"` command, a real
ClipboardEvent in the engine, caps and normalisation in the decoder, `flush_stale_paste` for
recovery.

### 3.5 F5 — MEDIUM: focus is enabled, decoded, and thrown away

`tty.rs:152` enables `DECSET 1004`. `input.rs:270-273` decodes `CSI I` and `CSI O` into
`Event::FocusGained` / `Event::FocusLost`. `main.rs:653` then matches them alongside
`Event::Unknown` and returns `false`, doing nothing. The engine has no focus command at all — its
`handleCommand` switch (`main.js:228-258`) accepts `navigate`, `resize`, `input`, `reload`, `back`,
`forward`, `stats`, `quit`.

We are paying the wire cost of focus reporting and getting nothing. What is lost:

The page's `focus`/`blur` events never fire and `document.hasFocus()` never changes, so
focus-dependent UI (autofocus, caret, `:focus-within`, "pause when hidden" video players, editors
that save on blur) behaves as if the user never left. Sticky modifiers are unrecoverable: a `Cmd`
held down while the user switches away is never released, because the release event goes to the
other window — the standard TUI bug, and focus-out is the standard place to clear it. The idle-power
win is forfeited: `webContents.setFrameRate(60)` is set once at `main.js:115` and never lowered, so
an unfocused Terminal-Fenster keeps Chromium compositing at 60 fps behind another window. And most
importantly for this mission, focus is a **precondition of the async Clipboard API** — see §5.2.

---

## 4. OSC 52 specification

### 4.1 Wire syntax

```
write:   ESC ] 52 ; <Pc> ; <base64> ESC \        1B 5D 35 32 3B 63 3B … 1B 5C
         ESC ] 52 ; <Pc> ; <base64> BEL          (accepted from terminals; not emitted by us)
read:    ESC ] 52 ; <Pc> ; ?     ST              -- NEVER EMITTED (§4.4)
clear:   ESC ] 52 ; <Pc> ; <neither base64 nor '?'>
```

`Pc` is `c` (system clipboard) on all supported platforms. Not `p`, not `s`, not an empty `Pc` —
empty defaults to `s0`, which is the X11 cut buffer and is meaningless on macOS.

### 4.2 Emission rules

Emit ST (`ESC \`), not BEL. Ghostty's terminfo advertises the BEL form on the request side
(`Ms=\E]52;%p1%s;%p2%s\007`, §1.2) and Ghostty's own replies use ST (A06 §6.1), so both are
understood; ST is the safer choice because a BEL that a multiplexer fails to consume rings the
terminal bell, and because ST is unambiguous inside `screen`/`tmux` passthrough wrappers.

Emit only from one function. A09 §1.3 already establishes a single-writer discipline for the tty
with a CI grep gate ("any write to the tty outside `src/tty/writer.rs` is a build failure"). The
clipboard writer must live behind that gate and take a newtype, not a `&str`:

```rust
/// The ONLY function permitted to emit ESC ] 52. Takes an already-adjudicated payload:
/// constructing `ClipboardGrant` requires passing the §7 consent gate.
pub fn write_clipboard(w: &mut TtyWriter, grant: &ClipboardGrant) -> io::Result<()>;
```

Assert the invariant on the way out. Because the payload is base64 by construction, the emitted
bytes cannot contain `ESC` — but that is a property worth asserting rather than assuming, since it
is the entire reason page-controlled text is safe to put on this wire. In the writer, and again in a
CI test:

```rust
debug_assert!(b64.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/' || *c == b'='));
```

Size cap: **1 MiB decoded**, hard. Rationale is empirical, not aesthetic — tmux historically chunked
OSC payloads around 74 KB, multiplexer and SSH buffering (A07, C09) make multi-megabyte single
writes a latency cliff, and no legitimate "copy" interaction produces more. Over a link whose rate
C09 has estimated below some threshold, lower the cap proportionally rather than blocking the frame
pump; C09 §2.2's marker-bracketed drain timing already provides the rate estimate.

Chunking: do **not** split a single OSC 52 across writes. A partial OSC left in the terminal's parser
when we are killed is a hung parser state. If the payload exceeds the cap, deny it and tell the user;
do not truncate silently, because a truncated command in a clipboard is more dangerous than no
command.

Fallback when the terminal has no OSC 52 (Apple Terminal, verified): the write must fail closed and
surface `NotAllowedError` to the page — the same error Chromium raises when a clipboard write is
denied, so pages already handle it. Feature detection is `Ms` in terminfo plus the capability probe;
it must **not** be DA1 parameter `52`, because iTerm2's own binary says that claim is unreliable
(§1.3). Absence of OSC 52 is also the case to surface in `terminal-fenster doctor`.

### 4.3 What triggers a write, and how the engine learns about it

The engine currently has **no preload script** (`main.js:106-113`: `offscreen`, `nodeIntegration:
false`, `contextIsolation: true`, `sandbox: true`, `webSecurity: true`). A minimal preload is
required, and A09 §3.7 already specifies its shape — "minimal, typed, no passthrough". The clipboard
surface it adds is two messages out, one message in.

The preload listens for the DOM `copy` and `cut` events on `document` (which fire for the user's own
copy chord, for `document.execCommand('copy')`, and for a page's synthetic copy alike) and shims
`navigator.clipboard.writeText` / `write`. Each interception posts `{text, origin, hasActivation}`
to main over the contextIsolated bridge. `navigator.userActivation.isActive` is read in the page
context at call time — it cannot be reconstructed in main, which is the reason the shim is needed at
all rather than a main-process-only hook.

Main applies §7's consent gate and, on grant, sends event type 2 to the CLI:

```json
{"t":"clipboard","op":"write","origin":"https://example.com","gesture":true,"b64":"<base64>"}
```

The CLI re-applies the size cap (never trust the engine's cap — B06's IPC hardening treats the
engine as a lower-trust peer), then calls `write_clipboard`. The payload crosses the socket already
base64-encoded, which conveniently means the control-plane JSON never carries raw page text and
`json_escape` (`tf-proto/src/lib.rs:106`) is never asked to escape a control character.

There is a second, more robust signal available when the engine runs on a machine with a real
desktop: Chromium has already written the text to the OS clipboard by the time the `copy` event
completes, so `electron.clipboard.readText()` is an independent confirmation of what was copied. Use
it as a cross-check when available and ignore the mismatch case silently — under SSH the engine may
be headless and `clipboard.readText()` will return empty or throw. Never make it the primary path.

### 4.4 OSC 52 READ — the verdict is never, in v1

**Terminal-Fenster must not emit `OSC 52 ; c ; ?` under any configuration in v1.** Not behind a flag, not
with a deadline, not once at startup. This is a stronger position than A06 §6.2's "offer it behind an
explicit opt-in flag with a 2-second deadline", and the evidence supports the stronger position:

On **Apple Terminal 465** it is not implemented — measured, no reply, no effect. On **iTerm2 3.6.9**
A06 measured no reply at all, and §1.3 shows the read path is gated behind a per-request *Share
clipboard contents with app in terminal?* consent that is off by default. So the feature is
unavailable on two of the three verified terminals.

On **Ghostty 1.3.1** it is available but hazardous. `clipboard-read = ask` (verified §1.1) raises a
modal, and A06 §6.2 records what that modal does, learned the hard way during that mission: it
**blocks the entire application**, including Ghostty's AppleScript interface and new-window creation.
A browser that can wedge the user's terminal emulator by rendering a page is not shippable. Worse,
the deadline mitigation does not help — the modal blocks the terminal, not us, so our timeout expires
and we move on while the user's terminal stays frozen.

And the upstream reference implementation says the same thing in its own words. kitty's default is
`write-clipboard write-primary read-clipboard-ask read-primary-ask`, with the note that disabling the
read confirmation *"is a security risk as it means that any program, even the ones running on a
remote server via SSH can read your clipboard"* (A06 §6.2, quoting `kitty/options/definition.py`).
Terminal-Fenster is exactly the "program running on a remote server" that warning is about (A07).

The consequence for the parser is worth stating explicitly, because it is a nice property: since we
never emit the request, **any** `OSC 52` on our stdin is unsolicited, and unsolicited means hostile
or broken. F1's fix should therefore not merely drop it but count it separately
(`stdin_osc52_reply_dropped_total`) as a tripwire. And there must remain **no base64 decoder in
`tf-term`** — the absence is itself a structural guarantee that no code path can turn a clipboard
reply into a string.

### 4.5 The substitute: `navigator.clipboard.readText()` → gesture-mediated paste

`readText()` and `read()` resolve from a real paste the user performs. The handshake:

1. Preload intercepts the call. If `navigator.userActivation.isActive` is false, reject immediately
   with `NotAllowedError` and do not prompt. Chromium requires transient activation here anyway;
   we are matching the platform, not inventing policy.
2. Main forwards `{t:"clipboard", op:"read-request", origin}` to the CLI.
3. The CLI arms a **10 second** window and shows one line in the status bar:
   `example.com wants to read your clipboard — press ⌘V to allow, Esc to deny`.
   The origin string is rendered through `unicode::sanitize_for_terminal` (A09 §1.3, `main.rs:829`);
   it is attacker-chosen text on the user's terminal and must not be trusted.
4. If the next `Paste` event arrives inside the window, the CLI resolves the request with the pasted
   text (after §6.2 normalisation). If Esc, focus loss, navigation, or the timeout comes first, the
   CLI resolves with a denial and the promise rejects `NotAllowedError`.
5. At most **one** request may be armed at a time. A second request while one is armed rejects
   immediately — otherwise a page can queue prompts and harvest a paste the user meant for a
   different request.

This is better than OSC 52 read on four counts, which is why it is the primary design rather than a
fallback. It works on all three verified terminals including Apple Terminal. The consent is a
physical keystroke, so it cannot be forged by a page. It never opens a terminal-side modal, so it
cannot wedge Ghostty. And the data path is one we already need for ordinary paste, so it is not new
attack surface.

Chromium's own `clipboard-read` permission stays **denied** in `setPermissionRequestHandler`, exactly
as A09 §3.4 specifies. The handshake lives above that layer, in the preload shim, so there is no
conflict: the page's `readText()` never reaches Chromium's clipboard at all.

---

## 5. Focus specification

### 5.1 Wire-up

`DECSET 1004` is already on. The missing pieces are three: forward the event, act on it in the
engine, and choose the right default when the terminal does not report.

CLI, replacing the discard at `main.rs:653`:

```rust
Event::FocusGained => { self.focused = true;  self.send(r#"{"t":"focus","v":true}"#);  false }
Event::FocusLost   => { self.focused = false; self.send(r#"{"t":"focus","v":false}"#); false }
```

Engine, a new `handleCommand` case:

```js
case 'focus':
  if (!win || win.isDestroyed()) break;
  if (cmd.v) { win.webContents.focus(); win.webContents.setFrameRate(60); }
  else       { win.webContents.blur();  win.webContents.setFrameRate(IDLE_FPS); }
  break;
```

`IDLE_FPS` wants to come from B07's frame scheduler rather than being invented here; something in the
1–5 range is the obvious starting point, and the paint pump at `main.js:66` already coalesces so a
low rate degrades gracefully. Flag this as a cross-mission dependency rather than settling it in D04.

### 5.2 Why focus is a clipboard problem, not just a polish problem

The window is created with `show: false` and `offscreen: true` (`main.js:102-113`). It is never
focused by the window server, because there is no window. Chromium's async Clipboard API rejects
`navigator.clipboard.writeText()` with `NotAllowedError: Document is not focused` when the focus
controller reports the page unfocused. If an offscreen `BrowserWindow` reports unfocused by default,
then **every** page that uses the modern clipboard API — which is most "click to copy" buttons —
fails in Terminal-Fenster regardless of how good §4 is.

Whether `webContents.focus()` is sufficient to satisfy that check for an offscreen view in Electron
43 / Chromium 150 is **UNVERIFIED** — it needs a running Electron, which needs the sandbox disabled
(§10, P1). It is the single highest-value probe in this document, because a negative result changes
the design: the fallback is for the preload's `writeText` shim to bypass Chromium's clipboard
entirely and resolve from our own OSC 52 path, which works but means we must replicate the platform's
activation checks ourselves rather than inheriting them.

The legacy `document.execCommand('copy')` path does not have the focus requirement and will keep
working either way, which is worth knowing when triaging a report.

### 5.3 Default when the terminal does not report focus

Apple Terminal 465's support for mode 1004 is UNVERIFIED and unprobeable (no DECRQM, A06 §"what I
could not verify"). The rule must therefore be **assume focused until told otherwise**: initialise
`focused = true` and let only an actual `CSI O` clear it. A terminal that never reports leaves the
page permanently focused, which is correct for a single-window session and fails safe. The inverse
default — assume blurred until a `CSI I` arrives — would leave the page permanently unfocused on
exactly the terminal that cannot tell us otherwise, breaking text input.

### 5.4 What focus loss must also do

Clear all modifier state, so a `Cmd` or `Ctrl` held while switching away does not stick. Cancel any
armed clipboard-read handshake (§4.5 step 4) — a page must not harvest a paste the user made after
switching to another window and back for an unrelated reason. Do **not** flush an in-progress paste;
a paste can legitimately straddle a focus event on a slow link.

---

## 6. Bracketed paste: the corrected path

### 6.1 Decoder changes (`crates/tf-term/src/input.rs`, core-owned)

Add three fields alongside `in_paste` / `paste_buf` (`input.rs:117-118`): a byte counter, a
`last_byte_at: Instant`, and a `paste_seq: u64` used to tag the events for the F2 quarantine.

Cap `paste_buf` at **4 MiB** hard. On overflow, abort: clear the buffer, keep `in_paste` true so the
remaining bytes are still discarded rather than executed, set an `aborted` flag, and emit
`Event::PasteAborted { bytes_dropped }` when the terminator finally arrives (or when the idle
timeout fires). Discarding the payload while *continuing* to swallow bytes is the important detail —
the alternative, dropping out of paste mode on overflow, converts an oversized paste directly into
F2.

Add `Decoder::flush_stale_paste(&mut self, idle: Duration) -> Option<Event>`, mirroring the existing
`flush_pending_escape` contract so timeout policy stays with the caller (`input.rs:9-14`). Suggested
caller policy: 250 ms of idle inside a paste, or 10 s total, whichever comes first. Both numbers are
generous next to a local paste and tight next to a human noticing that the keyboard is dead. Over
SSH, C09's rate estimate should scale the idle timeout — a 4 MiB paste over a slow link legitimately
has long gaps.

Normalise before emitting: `\r\n → \n`, then lone `\r → \n`. Strip C0 controls other than `\t` and
`\n`. Count and report the stripped bytes; a paste that contained `ESC` is worth a status-bar note
even though it is now harmless, because it is evidence.

Emit `Event::Paste { text, seq, truncated: bool }` rather than a bare `String`, so the CLI can
implement the F2 quarantine and report truncation.

### 6.2 Wire protocol addition

Replace the synthetic-keystroke command at `main.rs:645-651` with:

```json
{"t":"input","kind":"paste","text":"<normalised utf-8>"}
```

The framing is unchanged — type 10, JSON, per `tf-proto/src/lib.rs:13`. `json_escape`
(`tf-proto/src/lib.rs:106`) already handles the payload; after §6.1's control stripping there is
nothing exotic left in it.

### 6.3 Engine: deliver a real paste event

Two strategies, chosen at runtime, because the correct one depends on whether a system clipboard
exists — which under SSH (A07) it does not.

**Local, clipboard available (preferred).** Save the current clipboard text, write ours, invoke
Chromium's own paste command, restore:

```js
const saved = clipboard.readText();
clipboard.writeText(cmd.text);
wc.paste();                                   // real, trusted `paste` ClipboardEvent
setTimeout(() => clipboard.writeText(saved), 50);
```

This is the only way to get `isTrusted: true` and the browser's native default action, which is what
makes rich editors behave. Two honest caveats. The save/restore is a race, and it only preserves the
text flavour — a clipboard holding an image or RTF loses those flavours. And the restore is mostly
ceremonial in the common case, because the text we are writing came *from* the system clipboard when
the user pressed ⌘V, so it is usually already there. Keep the save/restore anyway for the case where
the paste arrived from a primary selection or from a remote terminal's clipboard.

**Headless or clipboard unavailable (SSH, CI).** Preload dispatches a synthetic
`ClipboardEvent('paste')` carrying a populated `DataTransfer` at `document.activeElement`, and if the
page does not call `preventDefault()`, falls back to `wc.insertText(cmd.text)` for the default
insertion that an untrusted event does not perform. Editors that check `isTrusted` will refuse; most
do not. This path is strictly worse and should be reported in `doctor` so a user knows why an editor
misbehaves.

Cap on this side too, independently of the CLI's cap (B06: the peer is not trusted): reject a paste
command above 4 MiB and log it.

---

## 7. The consent model

Consent is decided by **provenance** — how the clipboard operation came to exist — and then, for
writes only, filtered by **content**, because the risk of a clipboard write is not who wrote it but
what the user's shell does with it later.

### 7.1 Clipboard write — three provenance tiers

**Tier U — user-initiated.** The user selected content in the page and pressed Terminal-Fenster's copy
chord. The gesture is the consent. Write immediately, no prompt, no per-origin state; show a
one-line status-bar confirmation (`copied 42 chars`) so the action is never silent.

This tier is not optional, and it is worth explaining why. The terminal's own ⌘C copies the *rendered
cells*, and Terminal-Fenster's cells are pixels — the user would get nothing, or mojibake. Terminal-Fenster must
own copy, which means Terminal-Fenster must own OSC 52 write. There is no configuration in which we can
opt out of this feature.

**Tier G — page-initiated with transient user activation.** The page called
`navigator.clipboard.writeText()` or `execCommand('copy')` inside a user gesture we forwarded. This
is the ubiquitous "click to copy" button. Grant it, subject to §7.2, and remember the grant
**per-origin for the session only** — never persisted to disk, cleared on navigation to a different
origin and on exit. Notify in the status bar every time, including for already-granted origins:
`example.com copied 42 chars to your clipboard`. The notification is the mediation. A modal on every
copy button would train the user to dismiss modals, which is worse than no modal.

The first write from an origin in a session gets a one-time inline confirmation in the status bar
(`allow example.com to write to your clipboard? [y/N]`) rather than silent approval. One prompt per
origin per session is a cost users tolerate; one per click is not.

**Tier A — page-initiated with no user activation.** No gesture within the transient activation
window. **Deny**, reject with `NotAllowedError`, and count it. This is the drive-by clipboard-hijack
case and it is also what Chromium does on its own for the async API, so aligning here mostly means
not building a bypass. The counter matters: repeated Tier A attempts from one origin is a signal
worth surfacing in the status bar and the diagnostic bundle.

### 7.2 Clipboard write — the content gate

Applied to Tier G and Tier A alike, and deliberately *not* to Tier U, because in Tier U the user
selected the bytes themselves.

The gate exists because of F3. Ghostty's paste protection — the thing that would have caught this —
is disabled for the entire session precisely because we hold mode 2004 (verified §1.1). We disabled
it; we owe the user a replacement. And the replacement belongs on the write side, not the paste side,
because the dangerous paste happens *later*, in a shell, possibly after Terminal-Fenster has exited.

Escalate to a blocking confirmation when the decoded payload contains any of `\n`, `\r`, `\x1b`, or
any C0 control other than `\t`. The confirmation shows the first 200 characters with controls
rendered as visible glyphs (`␛`, `⏎`, `␉`) and requires an explicit keystroke. Ghostty's own
threshold is newlines; matching it keeps the user's mental model consistent across the two programs
sharing their terminal.

Never rewrite the payload to make it pass. Stripping newlines from a page's clipboard write would
silently corrupt legitimate multi-line copies (code snippets, addresses, SQL) and would teach users
that Terminal-Fenster mangles clipboards. Confirm or deny; do not edit.

Deny above **1 MiB** decoded with `NotAllowedError` and a status-bar note, per §4.2.

### 7.3 Clipboard read — one tier

There is no origin-based tier for reads. Every read is mediated by a physical paste (§4.5), armed for
10 s, one at a time, cancelled by focus loss or navigation. There is no per-origin memory and no
"always allow" — a remembered read grant is a standing exfiltration channel for every password and
2FA code the user copies for the rest of the session, and no product benefit justifies it.

The user's own ⌘V into the page is not a read request and is never prompted. The user chose to hand
the page that data; that *is* the browser's clipboard model.

### 7.4 Two invariants that sit above all of it

The clipboard is never an agent tool. A09 M5 already states this; D04 restates it because the
handshake in §4.5 makes it newly tempting to expose `read_clipboard` to the model. No such tool in
v1. The handshake resolves to the *page*, never to the agent, and the agent cannot arm it.

Every string we render on the user's terminal in service of consent — origins, previews, byte counts
— goes through `unicode::sanitize_for_terminal` (A09 §1.3, `main.rs:829`) first. A consent prompt
that renders an attacker-chosen origin unsanitised is a consent prompt an attacker can redraw.

### 7.5 The model as a table

| Operation | Provenance | Content | Outcome |
|---|---|---|---|
| write | Tier U (our copy chord) | any | allow, notify |
| write | Tier G (gesture), origin granted | plain text ≤1 MiB | allow, notify |
| write | Tier G, origin not yet granted | plain text ≤1 MiB | one-time inline confirm, then allow + notify |
| write | Tier G | contains `\n \r \x1b` or C0 | blocking confirm with rendered preview |
| write | Tier G | >1 MiB | deny, `NotAllowedError` |
| write | Tier A (no activation) | any | deny, `NotAllowedError`, count |
| write | any | terminal has no OSC 52 | deny, `NotAllowedError`, surface in `doctor` |
| read | page `readText()`, activation | — | arm 10 s paste handshake, resolve on ⌘V |
| read | page `readText()`, no activation | — | deny immediately, no prompt |
| read | user's own ⌘V | — | allow, no prompt (it is a paste, not a read) |
| read | OSC 52 `?` | — | **never emitted** |
| read | agent | — | **no tool exists** |

---

## 8. Threat register delta

D04 confirms two A09 threats against the shipped build and adds three.

| ID | Status after D04 |
|---|---|
| A09 **T4** (clipboard exfil via OSC 52 reply on stdin) | **CONFIRMED against current code**, §3.1. Mitigation "treat stdin as untrusted" is doctrine but not implemented in `input::Decoder`. |
| A09 **T21** (agent reads clipboard) | Still mitigated — no tool exists. §7.4 restates the constraint under the new handshake. |
| **D04-1** | Paste-terminator injection synthesises mouse clicks and browser commands. §3.2, measured. |
| **D04-2** | Enabling 2004 disables the terminal's paste-attack protection for the whole session. §3.3, verified from vendor docs. |
| **D04-3** | Unterminated paste is a permanent input wedge with unbounded memory growth. §3.4(c), measured. |

---

## 9. Test vectors

All of these are pure-function tests over `Decoder::decode` with no tty, no Electron, and no network,
so they run in CI on any machine. T1–T5 currently **fail** (they describe the fixed behaviour);
T6–T8 currently fail; T9–T10 pass today and are regression guards for properties §2 says are already
correct.

| ID | Input | Required behaviour |
|---|---|---|
| T1 | `\x1b]52;c;cGFzc3dvcmQ=\x1b\\` | zero `Key` events; one dropped reply; `stdin_osc52_reply_dropped_total == 1` |
| T2 | `\x1b]52;c;cGFzc3dvcmQ=\x07` | same as T1; BEL terminator consumed, **no `Ctrl+G`** |
| T3 | `\x1b_Gi=31;OK\x1b\\` | zero `Key` events (kitty graphics reply) |
| T4 | `\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\` | zero `Key` events (colour reply) |
| T5 | `\x1b]52;c;` + 16 KiB with no terminator | buffer capped at 8 KiB, `Event::Unknown`, decoder resumes on the next valid sequence |
| T6 | `\x1b[200~x\x1b[201~\x1b[<0;5;5M` | `Paste("x")` **only**; the mouse event is dropped (same-chunk rule, §3.2) |
| T7 | `\x1b[200~x\x1b[201~\x11` | `Paste("x")` only; **no quit** |
| T8 | `\x1b[200~` + 8 MiB, no terminator | aborts at 4 MiB, `PasteAborted`, memory bounded, decoder still accepts input after `flush_stale_paste` |
| T9 | `\x1b[200~evil\x1b[<0;1;1M\x1b[201~` | `Paste("evil\x1b[<0;1;1M")` — payload never reinterpreted (already at `input.rs:735`) |
| T10 | `\x1b[200~part` then `ial text\x1b[201~` | `Paste("partial text")` (already at `input.rs:726`) |
| T11 | `\x1b[200~a\r\nb\rc\x1b[201~` | `Paste("a\nb\nc")` — normalisation |
| T12 | `\x1b[200~a\x00b\x1b[201~` | `Paste("ab")`, one stripped control counted |
| T13 | round-trip: `b64::encode(payload)` into `write_clipboard` | emitted bytes match `^\x1b\]52;c;[A-Za-z0-9+/]*={0,2}\x1b\\\\$`; no `ESC` inside the body |
| T14 | write payload of 1 MiB + 1 byte | denied, no bytes written to the tty |

Reproducing the measurements in this document — the probe harness, verbatim:

```rust
// scratchpad/d04probe/src/main.rs ; Cargo.toml path-depends on crates/tf-term
use tf_term::input::{Decoder, Event, KeyCode};
fn line(label: &str, bytes: &[u8]) {
    let mut d = Decoder::default();
    let evs = d.decode(bytes);
    let mut text = String::new();
    for e in &evs { if let Event::Key { code: KeyCode::Char(c), .. } = e { text.push(*c); } }
    println!("{label:28} in={:>4}B  events={:>3}  leaks_as_text={:?}", bytes.len(), evs.len(), text);
}
fn main() {
    line("APC kitty graphics reply", b"\x1b_Gi=31;OK\x1b\\");
    line("DCS DECRQM-ish reply",     b"\x1bP1$r2004h\x1b\\");
    line("OSC 52 read reply",        b"\x1b]52;c;cGFzc3dvcmQ=\x1b\\");
    line("OSC 52 read reply (BEL)",  b"\x1b]52;c;cGFzc3dvcmQ=\x07");
    line("PM string",                b"\x1b^junk\x1b\\");
    let mut d = Decoder::default();
    println!("{:?}", d.decode(b"\x1b[200~x\x1b[201~\x1b[<0;5;5M"));
    let mut d2 = Decoder::default();
    let a = d2.decode(b"\x1b[200~payload that never ends...");
    let b = d2.decode(b"and the user keeps typing qqqqq");
    println!("unterminated paste: {} {} events", a.len(), b.len());
}
```

```
$ cargo run --quiet --offline
APC kitty graphics reply     in=  12B  events= 10  leaks_as_text="_Gi=31;OK\\"
DCS DECRQM-ish reply         in=  12B  events= 10  leaks_as_text="P1$r2004h\\"
OSC 52 read reply            in=  21B  events= 19  leaks_as_text="]52;c;cGFzc3dvcmQ=\\"
OSC 52 read reply (BEL)      in=  20B  events= 19  leaks_as_text="]52;c;cGFzc3dvcmQ=g"
PM string                    in=   8B  events=  6  leaks_as_text="^junk\\"

[Paste("x"), Mouse { kind: Down, button: Left, x: 5, y: 5, mods: ... }]
unterminated paste: 0 0 events
```

---

## 10. What I could not verify

Stated plainly, with the probe each one needs, because an honest blocker is worth more than a
confident guess.

**P1 — Does an offscreen `BrowserWindow` satisfy Chromium's document-focused check after
`webContents.focus()`?** This determines whether `navigator.clipboard.writeText()` works at all in
Terminal-Fenster (§5.2). Blocked: needs a running Electron, and Chromium children fail under the agent
sandbox with `bootstrap_look_up … Permission denied`. Probe: load a data URL that calls
`navigator.clipboard.writeText('x').then(()=>log('ok')).catch(e=>log(e.name+': '+e.message))`, once
before and once after `wc.focus()`, and read the console over the existing event channel. Log-based,
therefore CI-able, no screenshot needed.

**P2 — Do Ghostty and iTerm2 strip a literal `CSI 201~` from pasted data?** Determines whether F2 is
reachable from a hostile web page's copy-to-clipboard or only from a hostile clipboard. Blocked: a
real paste needs UI automation and the machine is at a lock screen. Probe:
`printf '\033[200~' ; pbcopy < payload_with_embedded_201.txt` then a scripted ⌘V under
`script(1)` capture, with the raw bytes recorded. The fix in §3.2 is required either way, so this is
triage information rather than a gate.

**P3 — Does a `char` event carrying `\r` trigger implicit form submission in Electron OSR?** Same
blocker as P1. Probe: a data URL with `<form onsubmit="log('SUBMITTED');return false"><input></form>`,
send the current paste command with `text: "hello\r"`, watch the console. This is the concrete
severity of F4(d).

**P4 — iTerm2 3.6.9's runtime default for `AllowClipboardAccess`.** The prefs domain is empty
(never launched) and the default is registered in code, not in a shipped plist. iTerm2 automation is
blocked by macOS TCC on this machine. Probe: launch iTerm2 once by hand, then
`defaults read com.googlecode.iterm2 AllowClipboardAccess`, and separately `printf
'\033]52;c;aGVsbG8=\a'` followed by `pbpaste`. §4.2's fail-closed behaviour makes this a
product-quality question rather than a correctness one.

**P5 — Apple Terminal 465 support for modes 2004 and 1004.** Not probeable (no DECRQM, A06). Probe
behaviourally: enable each, then paste / Cmd-Tab away and back, and record what arrives on stdin.
§5.3's assume-focused default makes the 1004 answer safe either way; the 2004 answer determines
whether paste works at all on that terminal.

**P6 — Per-character `sendInputEvent` cost.** Would quantify F4(b). Same Electron blocker as P1.
Probe: time `wc.sendInputEvent({type:'char',...})` in a 100 k loop and divide. The 4 MiB cap in §6.1
is justified by structure regardless of the number.

---

## 11. Recommended order for the commander

Ordered by risk removed per line of core code changed, not by severity alone.

1. **F1**, `input.rs` string-sequence arm plus a drop-and-count in the CLI. Roughly nine lines of
   state machine. Stops clipboard exfiltration and stops stray terminal replies typing into pages.
2. **F4(c)**, the paste cap and `flush_stale_paste`. Small, self-contained, removes a permanent-wedge
   failure mode that is nearly undiagnosable in the field.
3. **F2**, the same-chunk quarantine. Small, exact, no false positives.
4. **F5**, focus wire-up. Three lines in the CLI, one case in the engine, and it unblocks P1 — which
   is the probe that decides how much of §4 is even reachable.
5. **F4(a,b,d)**, the real paste path: new `kind: "paste"` command, `clipboard.writeText` + `wc.paste()`
   with the headless fallback, normalisation. Largest change; also the one that structurally restores
   the premise F3's exemption assumes.
6. **§4 + §7**, OSC 52 write behind the single-writer gate with the consent model. Depends on the
   preload from A09 §3.7, so it sequences after that mission's work lands.

Nothing in this list requires OSC 52 read, a base64 decoder, or a persisted clipboard permission —
and none of those three should ever be added to v1.
