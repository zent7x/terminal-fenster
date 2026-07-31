# E07 — CLI Surface, Output Conventions, Versioned Local API, Completions, Exit Codes

Mission E07. Scope: the command tree, JSON/JSONL conventions, the versioned local API,
shell completions, the exit-code taxonomy, TypeScript and Rust client examples, and an
audit of what `apps/cli/src/main.rs` has today.

**Ownership.** I wrote only this file. Every defect below is described, not patched.
`apps/cli/src/main.rs`, `apps/engine/src/main.js` and `crates/` are untouched.

**Evidence base.** Everything under §1 was measured on this machine against the committed
release binary `target/release/blackglass` (619,424 bytes, built from the current tree).
Commands and outputs are reproduced verbatim. Anything I could not measure is marked
UNVERIFIED and is never presented as fact.

---

## 0. Headline

1. **The CLI has four commands, three exit codes, and no machine-readable output at all.**
   `grep` for `--json` in `apps/cli/src/main.rs` returns zero hits outside the
   engine-wire helpers. BlackGlass currently cannot be driven, asserted on, or smoke-tested
   by anything that is not a human sitting in Ghostty.

2. **Unknown flags are silently swallowed, not rejected.** `cmd_doctor` takes `_args: &[String]`
   and never reads it (`apps/cli/src/main.rs:97`). `blackglass doctor --json` runs the plain
   human path and exits as if the flag were honoured. D07 §15 already specifies
   `blackglass doctor --keys`; today that flag is accepted and ignored. Silent acceptance is
   worse than rejection — it teaches users the flag works.

3. **`blackglass open --help` performs a DuckDuckGo search for the string `--help`.**
   `normalize_url` (`main.rs:293-304`) has no flag awareness, so any argument that is not
   obviously a URL becomes a search query. Verified: the call reached the tty check, meaning
   help was never printed.

4. **Usage-on-error goes to stdout.** `main.rs:65-69` calls `print_help()`, which is
   `println!`, on the unknown-command path. Measured: `blackglass bogus 2>/dev/null` prints
   the full usage block. GNU/POSIX convention puts diagnostics and usage-after-error on
   stderr. Same class of bug in `cmd_doctor`'s not-a-tty branch (`main.rs:100-108`), where a
   *failure* message is printed to stdout and the process exits 1 — so
   `blackglass doctor > report.txt` captures the error into the report and leaves stderr empty.

5. **The single highest-value missing command is a non-interactive one.** There is no way to
   run the engine, take one frame, and exit without owning a tty. That is why this project is
   currently only verifiable by hand. A `blackglass capture` command plus `--output json`
   turns the entire pipeline — engine spawn, handshake, first paint, BGRA decode — into
   something CI can assert on, which is exactly the log/protocol-based evidence this
   environment forces us toward anyway.

6. **The design that makes all of the above cheap is one static command table** consumed by
   four things: the argument parser, `help`, the completion generator, and
   `blackglass api schema`. Written any other way, those four drift, and the drift is
   invisible until a user hits it.

---

## 1. Audit of the CLI as it exists today

### 1.1 The whole surface

Derived from `apps/cli/src/main.rs:52-93`.

| Token | Handler | Line | Behaviour |
|---|---|---|---|
| `open <url>` | `cmd_open` | 218 | Interactive browse. Consumes argv[1] only. |
| `doctor` | `cmd_doctor` | 97 | Capability report, human text only. |
| `version` / `--version` / `-V` | inline | 57 | `blackglass 0.1.0` |
| `help` / `--help` / `-h` / (no args) | `print_help` | 61 | Usage block on stdout. |
| anything else | inline | 65 | Error on stderr, usage on **stdout**, exit 2. |

Interactive key bindings intercepted before the page (`Session::handle_event`,
`main.rs:562-655`): `ctrl+q` quit, `ctrl+r` reload, `alt+left` back, `alt+right` forward.
Everything else is forwarded.

Environment variables read by the CLI:

| Variable | Read at | Effect |
|---|---|---|
| `BLACKGLASS_LOG` | `main.rs:33` | Append diagnostics to a file. Never stdout — stdout is the graphics channel. |
| `BLACKGLASS_ENGINE` | `main.rs:320` | Engine directory containing `node_modules/.bin/electron`. |
| `BLACKGLASS_BACKEND` | `main.rs:208` | Force `kitty` \| `unicode` \| `sixel`. |
| `BLACKGLASS_EXIT_AFTER_MS` | `main.rs:49` | Test hook: bounded run, then clean shutdown. |

### 1.2 Measured behaviour

```
$ B=./target/release/blackglass
$ for a in version --version -V help --help -h bogus open doctor; do
    $B "$a" >/dev/null 2>&1; printf "argv=%-11s rc=%s\n" "$a" "$?"; done
argv=version     rc=0
argv=--version   rc=0
argv=-V          rc=0
argv=help        rc=0
argv=--help      rc=0
argv=-h          rc=0
argv=bogus       rc=2
argv=open        rc=2
argv=doctor      rc=1
$ $B >/dev/null 2>&1; echo $?
0
```

Only `{0, 1, 2}` are ever produced. `doctor` returns 1 in a pipe because stdin is not a tty
(`main.rs:99-109`) — the same code a genuine engine failure would return.

```
$ $B doctor 2>/dev/null            # note: the FAILURE text is on stdout
blackglass doctor 0.1.0
  status: NOT A TTY -- run this from an interactive terminal.
  ...
$ $B bogus 2>/dev/null             # note: usage-on-error is on stdout
blackglass 0.1.0 -- a real browser in your terminal
USAGE: ...
$ $B doctor --json 2>&1 | head -1  # note: flag silently ignored
blackglass doctor 0.1.0
$ $B open --help 2>&1              # note: "--help" became a search query
blackglass: stdin is not a tty. Interactive browsing needs a terminal.
```

Dependency surface (`apps/cli/Cargo.toml`): `bg-term`, `bg-proto`, `libc`. No `clap`, no
`serde`. Any parser, formatter, or completion generator has to be written or a dependency
has to be added — §7.1 and §11.3 take a position on that.

### 1.3 Gaps and defects

Severity is about user-visible consequence, not effort.

| # | Severity | Gap | Evidence |
|---|---|---|---|
| G1 | High | No machine-readable output anywhere. No `--output`/`--json`. | no such token in `main.rs` |
| G2 | High | Unknown flags silently accepted on `doctor`; `_args` unused. | `main.rs:97` |
| G3 | High | No non-interactive command. Every code path that starts the engine also seizes the tty. | `cmd_open` `main.rs:227-231` |
| G4 | High | Exit codes collapse every failure into 1. Engine-missing, tty-missing, geometry-unknown and engine-crash are indistinguishable. | `main.rs:230, 237, 249, 265, 281` all `return 1` |
| G5 | Med | `open --help` becomes a search. No flag parsing, no `--` separator, so a URL beginning with `-` is unreachable. | `main.rs:219-225`, `293-304` |
| G6 | Med | Usage-on-error and failure diagnostics printed to stdout. | `main.rs:67`, `100-108` |
| G7 | Med | `BLACKGLASS_BACKEND=iterm2` is silently ignored — `Backend::Iterm2` exists but has no match arm; an unrecognised value silently falls through to auto-detect instead of erroring. | `main.rs:208-213` vs `bg-term/src/lib.rs:68-77` |
| G8 | Med | Help documents `kitty \| unicode`; code also accepts `sixel`. Docs and code already disagree. | `main.rs:90` vs `main.rs:211` |
| G9 | Med | No shell completions of any kind. | absent |
| G10 | Med | No `--version --output json`; version is an unparsed string. Downstream tooling has to regex it. | `main.rs:57-60` |
| G11 | Med | Extra positional args after the URL are silently discarded. | `args.first()`, `main.rs:219` |
| G12 | Low | No `--timeout`, `--width`, `--height`, `--profile`, `--devtools`, `--no-color`, `--log-level`. B09/E03/D07 all assume some of these exist. | absent |
| G13 | Low | The 30 s engine-connect budget is a hard-coded literal with no override. | `main.rs:415` |
| G14 | Low | Socket path headroom is 7 bytes. Worst case measured: `/var/folders/…/T/blackglass-99998-<19-digit-nanos>/engine.sock` = **97 bytes** against a `sun_path` of **104** (confirmed by compiling `sizeof(((struct sockaddr_un*)0)->sun_path)` on this machine). A longer `$TMPDIR` overflows. A09 §598 reaches the same conclusion independently. | `main.rs:394` |

G14 detail, since it is the kind of thing that fails only on someone else's machine:

```
$ printf '%s\n' "$TMPDIR" ; echo ${#TMPDIR}
/var/folders/qn/qt5tx7_x27v3l44yls7zgvm80000gn/T/
49
$ cat su.c
#include <sys/un.h>
#include <stdio.h>
int main(void){ struct sockaddr_un a; printf("sizeof(sun_path)=%zu\n", sizeof(a.sun_path)); return 0; }
$ cc su.c -o su && ./su
sizeof(sun_path)=104
```

The nanosecond suffix costs 19 bytes for uniqueness that 8 hex characters would provide.
Not my file to change; recorded for the commander.

---

## 2. The organising principle: three planes

Every command belongs to exactly one plane, and the plane determines tty ownership, default
output format, and which exit codes are reachable. This is not taxonomy for its own sake —
it is what stops a pipeline from accidentally putting the terminal into raw mode.

| Plane | Owns the tty? | Starts an engine? | Default output | Examples |
|---|---|---|---|---|
| **A — Interactive** | yes, exclusively | yes | human, on the graphics channel | `open`, `doctor` |
| **B — Control** | never | attaches to one, or spawns a detached one | `human` on a tty, `json` otherwise | `nav`, `page`, `input`, `tab`, `watch`, `capture` |
| **C — Management** | never | never | `human` on a tty, `json` otherwise | `setup`, `config`, `profile`, `history`, `completions`, `api`, `version` |

Three rules follow, and they are testable:

1. **A plane-B or plane-C command must never call `tty::TtyGuard::acquire`.** A unit test can
   assert this by construction if the command table carries a `plane` field.
2. **A plane-A command must never write structured output to stdout.** While `open` is
   running, stdout is carrying kitty graphics escape sequences; a single stray JSON line
   corrupts an image mid-transmission. This is the reasoning already recorded at
   `main.rs:28-31` for logging, and it generalises. Plane A honours `--output json` by
   writing to `--output-fd <n>` or `--output-file <path>`, never to fd 1.
3. **Only plane A may fail with `E_NOT_A_TTY` (4).** If a plane-B command can return 4, the
   plane assignment is wrong.

---

## 3. The command tree

### 3.1 Full tree

`T0` = exists today. `T1` = required before the CLI is usable by anything but a human.
`T2` = follows the sibling designs (B04 tabs, B09 profiles/data, D05 downloads, D07 keymaps).

```
blackglass [GLOBAL FLAGS] <command> [ARGS]

  open <url>                              A  T0  interactive browse
  doctor [--keys] [--engine] [--strict]   A  T0* human/JSON capability report   (--keys per D07 §15)
  capture <url>                           B  T1  render N frames to PNG, no tty
  version                                 C  T0* structured version info

  session                                 B      lifecycle of detached engines
    start [--url <u>] [--detach] [--name <n>] [--profile <p>]   T1
    list                                                        T1
    info <id>                                                   T1
    attach <id>                                                 T2  (hands the tty to plane A)
    stop <id | --all>                                           T1

  nav --session <id>                      B      navigation
    goto <url> [--wait load|domcontent|none] [--timeout <ms>]   T1
    back | forward | reload | stop                              T1

  page --session <id>                     B      read the page
    title | url                                                 T1
    text [--max-bytes <n>]                                      T1
    html [--max-bytes <n>]                                      T2
    links [--output jsonl]                                      T2  (feeds D07 hints)
    a11y [--max-nodes <n>]                                      T2  (CDP; E03 §7.2 applies)
    screenshot --out <path> [--format png]                      T1
    eval <js> [--world isolated|main]                           T2  (D07 §5.3: isolated by default)

  input --session <id>                    B      synthesise input on the human code path
    key <spec>...                                               T1
    type <text>                                                 T1
    click <x> <y> [--button left|middle|right] [--count <n>]    T1
    move <x> <y> | scroll <dx> <dy>                             T1

  tab --session <id>                      B  T2  per B04 §8
    list | new <url> | close <id> | select <id>

  watch --session <id> [--events <a,b>]   B  T1  JSONL event stream on stdout

  setup [--force] [--verify-only]         C  T1  engine acquisition, per B10 §3.5
  uninstall [--purge]                     C  T2  per B10 §7

  profile                                 C  T2  per B09 §3
    list | create <name> | delete <name> | show <name>
  history  search <q> | clear             C  T2  per B09 §9
  bookmark list | add <url> | remove <id> C  T2  per B09 §10
  download list | show <id>               C  T2  per B09 §11 / D05 §3
  cookie   list | clear [--domain <d>]    C  T2  per B09 §7

  config                                  C  T1  per D07 §13
    path | show | check | get <key> | set <key> <value> | edit

  keys [--mode normal|insert] [--broken]  C  T2  per D07 §15

  api                                     C  T1  self-description
    version | schema [<method>] | methods | exit-codes | capabilities

  completions <bash|zsh|fish|nushell|powershell|elvish>   C  T1
  __complete <args>...                                    C  T1  hidden; dynamic candidates
  help [<command>]                                        C  T0*
```

`T0*` means the command exists but does not yet meet the conventions in §4.

### 3.2 Naming rules

* Two levels maximum: `<noun> <verb>`. No `blackglass page dom query selector`.
* The noun is always singular (`tab list`, never `tabs list`) and matches the API namespace
  exactly, so `blackglass page text` is `page.text` on the wire with no translation layer.
* Verbs are drawn from a closed set: `list, show, get, set, add, remove, create, delete,
  start, stop, new, close, select, check, clear, search`. A new verb requires a decision, not
  a commit.
* No verb-first top-level commands except the three that are genuinely modal and would be
  absurd otherwise: `open`, `capture`, `watch`.
* Every abbreviation is an explicit alias in the table, never a prefix-match. Prefix matching
  means adding a command can break someone's script.

### 3.3 Global flags

Accepted before or after the subcommand; `--` terminates flag parsing (fixes G5).

| Flag | Default | Notes |
|---|---|---|
| `--output human\|json\|jsonl` | `human` if stdout is a tty, else `json` | `-o` alias |
| `--output-fd <n>` | `1` | Mandatory escape hatch for plane A (§2 rule 2) |
| `--output-file <path>` | — | Mutually exclusive with `--output-fd` |
| `--session <id>` | `$BLACKGLASS_SESSION`, else the only running session | Ambiguity is an error (21), never a guess |
| `--timeout <ms>` | per-command | Replaces the literal at `main.rs:415` |
| `--color auto\|always\|never` | `auto` | Honours `NO_COLOR` and `CLICOLOR_FORCE` |
| `--quiet` / `-q` | off | Suppresses human chatter; no effect on json/jsonl |
| `--verbose` / `-v` | off | Repeatable; routes to `--log-file`, never stdout |
| `--log-file <path>` | `$BLACKGLASS_LOG` | |
| `--version` | | Position-independent, unlike today (G10) |
| `--help` / `-h` | | Position-independent and subcommand-aware, unlike today (G5) |

Auto-detection of `--output` from `isatty(1)` is the single most useful default here: it
makes `blackglass page text` readable in a terminal and parseable in a pipe with no flag,
which is the behaviour people already expect from `gh`, `docker` and `kubectl`.

---

## 4. Output conventions

### 4.1 The envelope

Every `--output json` invocation emits **exactly one** JSON object, `\n`-terminated, and
nothing else on that stream. No banner, no progress, no trailing newline-separated extras.

Success:

```json
{
  "bg": 1,
  "ok": true,
  "cmd": "doctor",
  "ts": "2026-07-31T21:14:03.412Z",
  "elapsed_ms": 312,
  "data": { "…": "…" },
  "warnings": [
    { "code": "no_pixel_backend", "msg": "terminal has no graphics protocol; using unicode fallback" }
  ]
}
```

Failure:

```json
{
  "bg": 1,
  "ok": false,
  "cmd": "open",
  "ts": "2026-07-31T21:14:03.412Z",
  "elapsed_ms": 8,
  "error": {
    "code": "engine_not_found",
    "exit": 10,
    "msg": "electron not found; set BLACKGLASS_ENGINE to the engine directory",
    "hint": "run `blackglass setup`",
    "detail": {
      "searched": [
        "$BLACKGLASS_ENGINE/node_modules/.bin/electron",
        "<workspace>/apps/engine/node_modules/.bin/electron"
      ]
    }
  }
}
```

* `bg` is the **envelope** version — an integer, and the only field whose absence is
  itself meaningful. A consumer that sees an unexpected `bg` should refuse rather than
  guess. It is bumped only when an existing field changes meaning or disappears. It is *not*
  the API version and *not* the wire-protocol version (§6.1).
* `ok` is redundant with `exit` and that is deliberate: a consumer reading a log file has no
  exit status, and a consumer reading `$?` has no JSON. Both must work alone.
* `error.exit` is the process exit code, so a script that captured only stdout can still
  reconstruct what the shell would have seen.
* `warnings` is present only when non-empty. `data` is present iff `ok`.

### 4.2 Field conventions

These exist so that two commands written six months apart still look like the same tool.

1. **Booleans are booleans.** The human renderer's `yesno()` (`main.rs:199-205`) must not leak
   into JSON. `"kitty_graphics": true`, never `"yes"`.
2. **Unknown is `null`, never a sentinel.** `main.rs:125` prints `-`, `main.rs:169` prints
   `UNKNOWN`, `main.rs:184` prints `(no reply)`. In JSON all three are `null`, and the
   distinction between "not supported" (`false`) and "not determined" (`null`) is preserved,
   because for capability detection those are genuinely different states —
   `crates/bg-term/src/caps.rs:9-10` notes that absence of a reply is the negative result but
   a slow terminal can look like an unsupporting one.
3. **Units are in the name.** `_ms`, `_bytes`, `_px`, `_hz`. Never a bare `size` or `time`.
   Byte counts are integers, never pre-divided into KB the way the status bar does at
   `main.rs:893`.
4. **Numbers are numbers.** Integers for counts, pixels and bytes; floats only where the
   measurement is genuinely fractional (`fps`, `encode_ms`).
5. **Timestamps** are RFC 3339 UTC with millisecond precision. Streams *additionally* carry
   `t_ms`, a monotonic offset from process start, because wall-clock can step and a frame
   timeline that goes backwards is unusable.
6. **Enums are lowercase snake_case strings** drawn from a closed set published by
   `blackglass api schema`. `"backend": "kitty"` matches `Backend::as_str`
   (`bg-term/src/lib.rs:80-87`) exactly, so there is one spelling in the codebase.
7. **Keys are `snake_case`.** The engine wire protocol uses terse keys (`t`, `v`) because it
   is a hot path; the CLI's public JSON does not, because it is read by humans debugging
   scripts.
8. **Additive-only within an envelope version.** Consumers MUST ignore unknown keys. This is
   stated in `api capabilities` output so it is a contract, not folklore.
9. **Page-controlled strings are escaped, not sanitised.** The status bar correctly strips
   control characters with `unicode::sanitize_for_terminal` (`main.rs:887-888`) because it
   writes to a terminal. JSON output must instead *escape* them as `\u001b`-style sequences
   (`bg_proto::json_escape`, `crates/bg-proto/src/lib.rs:106-118`) and preserve the
   true value — a consumer asserting on a page title needs the real title, and its own
   renderer is responsible for its own terminal safety. Getting this backwards in either
   direction is a bug: stripping in JSON loses data, escaping-only in the status bar is an
   escape-sequence injection.

### 4.3 JSONL streams

Used by `watch`, `page links`, `capture --frames N`, and any command with progress.

* One object per line, `\n`-terminated, no pretty-printing, flushed per record.
* The **first** line is always `stream.begin`, the **last** is always `stream.end`.
  Without those, a stream truncated by a kill is indistinguishable from a short-but-complete
  one — which matters here, because engine crashes are an expected event, not an exception.

```jsonl
{"bg":1,"type":"stream.begin","cmd":"watch","stream":"events","schema":"event/1","ts":"2026-07-31T21:14:03.412Z","session":"a1b2c3d4"}
{"bg":1,"type":"event","t_ms":212,"event":"ready","electron":"43.2.0","chrome":"150.0.0.0","width":2482,"height":814}
{"bg":1,"type":"event","t_ms":366,"event":"frame","seq":0,"width":2482,"height":814,"payload_bytes":8081424,"wire_bytes":53999,"encode_ms":0.74}
{"bg":1,"type":"event","t_ms":412,"event":"title","value":"Example Domain"}
{"bg":1,"type":"event","t_ms":415,"event":"url","value":"https://example.com/"}
{"bg":1,"type":"event","t_ms":998,"event":"loading","value":false}
{"bg":1,"type":"stream.end","ok":true,"exit":0,"count":5,"elapsed_ms":1004}
```

The numbers in that example are the ones already measured end-to-end in Ghostty 1.3.1 and
recorded in the mission brief (engine ready 212 ms, first frame 366 ms, 8,081,424 BGRA bytes
→ 53,999 wire bytes, 0.74 ms encode). The stream format is designed to carry exactly the
evidence this project already collects by hand, which is the point: `blackglass watch` should
make the performance claims reproducible by a script rather than by a person reading a log.

Backpressure: the engine already coalesces frames and keeps at most one in flight
(`apps/engine/src/main.js:42-50`). A JSONL consumer that reads slowly must not be allowed to
inflate memory. `watch` therefore coalesces the same way for high-rate event classes and
emits `{"type":"drop","event":"frame","dropped":N}` records so loss is *reported*, never
silent. A stream that lies about being complete is worse than one that admits gaps.

### 4.4 Human output

Unchanged in spirit from the current `doctor`, which is genuinely good — the inline NOTEs at
`main.rs:141-158` explaining *why* a missing capability matters are the best part of the
existing CLI and should survive. Rules: no ANSI when `--color never`, when `NO_COLOR` is set,
or when stdout is not a tty; diagnostics and usage-after-error on stderr (fixes G6); and the
human renderer is a pure function of the same struct that feeds the JSON encoder, so the two
cannot disagree about facts.

---

## 5. Exit-code taxonomy

### 5.1 The table

Codes stay below 100. 126, 127, and 128+N are owned by the shell, and colliding with them
makes "command not found" and "killed by SIGSEGV" ambiguous with application failures. I
deliberately did **not** use BSD `sysexits.h` (64–78): `EX_USAGE = 64` conflicts with the far
more widely-observed convention that argument errors are 2, which this CLI already follows
(`main.rs:68`).

| Code | Symbol | Meaning |
|---|---|---|
| 0 | `OK` | Success |
| 1 | `FAIL` | Unclassified failure. **A 1 in production is a bug in this table.** |
| 2 | `USAGE` | Bad arguments, unknown command, unknown flag *(already used at `main.rs:68`, `224`)* |
| 3 | `CONFIG` | Config file unparseable or a value out of range (D07 §13.3 policy: startup still proceeds; only `config check` exits 3) |
| 4 | `NOT_A_TTY` | Interactive command in a non-interactive context *(today this is 1: `main.rs:109`, `230`)* |
| 5 | `TERM_UNSUPPORTED` | No pixel backend and `--require-pixels` was given |
| 6 | `NO_GEOMETRY` | Capability detection could not determine pixel geometry *(today 1: `main.rs:249`)* |
| 10 | `ENGINE_NOT_FOUND` | Electron not located *(today 1: `main.rs:281`)* |
| 11 | `ENGINE_START_FAILED` | Spawn failed |
| 12 | `ENGINE_HANDSHAKE` | Protocol mismatch — maps 1:1 onto B06 §3.4 `bad_magic`, `proto_mismatch`, `header_mismatch` |
| 13 | `ENGINE_CRASHED` | `render-process-gone` (`apps/engine/src/main.js:125-127`) or engine exit *(today 1: `main.rs:523`)* |
| 14 | `ENGINE_TIMEOUT` | Engine did not connect or stopped answering PING *(today 1, via `main.rs:420-425`)* |
| 20 | `SESSION_NOT_FOUND` | No such session id |
| 21 | `SESSION_AMBIGUOUS` | `--session` omitted and 0 or >1 candidates exist |
| 22 | `SESSION_DENIED` | Control socket exists but is not ours (uid/permission mismatch) |
| 30 | `NAV_FAILED` | Chromium net error; `error.detail.net_code` carries `did-fail-load`'s code (`main.js:122-124`) |
| 31 | `NAV_TIMEOUT` | `--wait` deadline exceeded |
| 32 | `PAGE_CRASHED` | Renderer died mid-command |
| 40 | `TARGET_NOT_FOUND` | Selector, tab id, link hint, or download id not found |
| 41 | `ASSERTION_FAILED` | An explicit assertion command failed — a *result*, not an error |
| 50 | `IO` | Disk full, permission denied, path too long (§1.3 G14) |
| 51 | `INTEGRITY` | Checksum/signature mismatch during `setup`. B10 §3.5 requires fail-closed. |
| 70 | `INTERNAL` | Bug. Panic handler exits 70 after the tty is restored. |
| 75 | `TEMPORARY` | Explicitly retryable; a wrapper may back off and retry |
| 77 | `DENIED_BY_POLICY` | Feature disabled by config or env (e.g. CDP off, `BLACKGLASS_MCP_CDP=0`) |

Signals are not remapped. If the process dies of `SIGINT` the shell reports 130; if the CLI
catches `SIGINT` and shuts down cleanly it exits 0 for `watch` (a clean stop of a stream is a
success) and 75 for a command that was interrupted mid-work.

### 5.2 Rules

1. Exit code and `error.code` are **bijective**. One string, one number, forever. Renaming a
   code is an envelope bump.
2. `error.code` is stable API; `error.msg` is not, and a script that greps `msg` is
   explicitly unsupported. Say so in the docs so nobody is surprised later.
3. `doctor` exits 0 whenever detection *worked*, even on a terminal with no graphics —
   "your terminal is weak" is information, not failure. `--strict` turns any degraded
   capability into 5. This unbundles the two meanings currently jammed into 1.
4. `capture`, `nav`, `page` return 30/31/32 for page-level problems and never 1, so a CI job
   can distinguish "the site was down" from "our browser is broken". That distinction is the
   entire value of the taxonomy.

### 5.3 Machine-readable

```
$ blackglass api exit-codes --output json
{"bg":1,"ok":true,"cmd":"api.exit-codes","data":{"codes":[
  {"code":0,"symbol":"OK","retryable":false,"msg":"success"},
  {"code":10,"symbol":"ENGINE_NOT_FOUND","retryable":false,"msg":"electron not located"},
  {"code":75,"symbol":"TEMPORARY","retryable":true,"msg":"transient failure; retry may succeed"}
]}}
```

The table is generated from the same Rust enum the code returns, so it cannot drift. A
wrapper generates its own constants from this rather than hard-coding integers.

---

## 6. The versioned local API

### 6.1 Three version numbers, deliberately separate

Conflating these is the classic mistake, and this project already has all three in play.

| Name | Governs | Where it lives | Today |
|---|---|---|---|
| `proto` | core ↔ engine framing and frame header | B06 §3.1-3.2 `HELLO`/`WELCOME` | 1 (proposed; unversioned today) |
| `api` | client ↔ core control methods | §6.4 handshake below | 1 (does not exist yet) |
| `bg` | the JSON envelope shape | every JSON document | 1 (does not exist yet) |

A new page-reading method bumps none of them (it is a `feature` string). Changing the frame
header bumps `proto`. Removing a method bumps `api`. Renaming `data` to `result` bumps `bg`.
`blackglass api version` reports all three plus the implementation version:

```json
{"bg":1,"ok":true,"cmd":"api.version","data":{
  "impl":"blackglass/0.1.0","bg":1,"api":1,"api_min":1,"proto":1,"proto_min":1,
  "engine":{"electron":"43.2.0","chrome":"150.0.0.0"},
  "features":["nav","page.text","page.screenshot","input","watch","capture"]
}}
```

The `engine` block is populated from the `ready` event the engine already sends
(`apps/engine/src/main.js:292-298`) and is `null` when no engine is running — that is a real
distinction, not an omission.

### 6.2 Transport

A **second** Unix socket, distinct from the engine socket, owned by the core:

```
$TMPDIR/blackglass-<uid>/<session>/control.sock      dir 0700, socket 0600
```

Rationale, and each point is load-bearing:

* Separate from `engine.sock` because that one carries 8 MB BGRA frames and a control client
  must not be able to stall the frame path (or read frames it was not granted).
* `$TMPDIR` rather than `~/Library/Application Support` because on macOS `$TMPDIR` is already
  per-user `0700` by the OS (A09 §598 verified this), and because of the path budget below.
* `<uid>` in the directory name so two users on one machine cannot collide.
* No TCP listener, ever. The engine's own posture is "opens no listening port of its own"
  (`apps/engine/src/main.js:13-14`); the control API must not quietly break that the way CDP
  does (E03 §7.2, `packages/mcp/lib/engine.js:15-23`).

**Path budget, measured** — `sun_path` is 104 bytes on this machine (§1.3):

```
/var/folders/qn/qt5tx7_x27v3l44yls7zgvm80000gn/T/   49
blackglass-501/                                     15
a1b2c3d4/                                            9
control.sock                                        12
                                                  ----
                                                    85    (19 bytes headroom)
```

An 8-hex-character session id is the right size: 4.3 × 10⁹ values, collision-checked at
create time because the directory creation is `O_EXCL` anyway, and it costs 8 bytes instead
of the 19 the current nanosecond scheme spends (G14).

### 6.3 Framing and type IDs

Reuse `bg_proto`'s `[u8 type][u32 BE len][payload]` framer verbatim — one framer, one set of
bugs, and `MessageReader` (`crates/bg-proto/src/lib.rs:57-94`) is already tested against
split reads and binary payloads containing newlines. Control types are drawn from B06 §3.1's
**must-understand** reserved range `0x10–0x7F`, so an old peer fails loudly rather than
skipping a message it needed:

| ID | Name | Direction | Payload |
|---|---|---|---|
| `0x20` | `C_HELLO` | client → core | JSON, ≤4 KiB, first message |
| `0x21` | `C_WELCOME` | core → client | JSON, ≤4 KiB, first message |
| `0x22` | `C_REQ` | client → core | JSON, ≤64 KiB |
| `0x23` | `C_RES` | core → client | JSON |
| `0x24` | `C_ITEM` | core → client | JSON, one stream item |
| `0x25` | `C_END` | core → client | JSON, stream terminator |
| `0x26` | `C_BLOB` | core → client | `[u32 BE id][bytes]` — screenshots, page HTML |
| `0x27` | `C_CANCEL` | client → core | JSON `{"id":N}` |

`C_BLOB` exists so a 3 MB PNG never has to be base64'd into JSON. It is the same asymmetry
`bg-proto` already documents at lines 7-9: JSON where readability pays, binary where volume
does.

### 6.4 Handshake

The rule is copied from B06 §3.5 rule 3 verbatim, because two different compatibility rules
in one codebase is how you get a bug nobody can reason about:

> the server accepts the client iff `hello.api_min ≤ SERVER_API ≤ hello.api`

```json
→ 0x20 {"t":"hello","magic":"blackglass-ctl","api":1,"api_min":1,
        "impl":"bg-client-ts/0.1.0","pid":8891,"features":["stream","blob"]}
← 0x21 {"t":"welcome","magic":"blackglass-ctl","api":1,"impl":"blackglass/0.1.0",
        "session":"a1b2c3d4","epoch":1,
        "features":["nav","page.text","page.screenshot","input","watch"],
        "limits":{"max_req_bytes":65536,"max_blob_bytes":33554432,"max_inflight":64}}
```

Mismatch closes with a `goodbye` carrying a B06 §3.4 code. A handshake failure is fatal and
non-recoverable — B06 §3.5 rule 7's reasoning applies unchanged: on a length-prefixed stream
there is no resynchronisation, and a misaligned framer reads attacker-chosen lengths.

**Authentication.** Filesystem permissions are the whole story, and that is a decision worth
stating rather than assuming: dir `0700`, socket `0600`, plus `SO_PEERCRED`-equivalent
(`LOCAL_PEERCRED` / `getsockopt(SOL_LOCAL, LOCAL_PEEREPID)` on Darwin) checked at accept to
reject any peer whose euid differs. That is strictly stronger than the CDP port, which is
unauthenticated to every process running as the user (E03 §7.2). If a token is ever needed
it goes in `C_HELLO`, but it should not be needed and adding one without need is theatre.
**UNVERIFIED:** I did not compile a `LOCAL_PEERCRED` probe on this machine; the API exists in
Darwin's `sys/un.h` but I am not asserting behaviour I did not run.

### 6.5 Method catalogue

Each maps 1:1 to a CLI path. `blackglass page text --session X` ⇒ `page.text`.

| Method | Params | Result | Errors |
|---|---|---|---|
| `api.version` | — | §6.1 object | — |
| `api.schema` | `{method?}` | JSON-Schema-ish descriptor | 40 |
| `session.list` | — | `{sessions:[…]}` | — |
| `session.info` | `{id}` | session object | 20 |
| `session.stop` | `{id}` | `{stopped:true}` | 20 |
| `nav.goto` | `{url, wait?, timeout_ms?}` | `{url, status?}` | 30, 31, 32 |
| `nav.back` / `nav.forward` / `nav.reload` / `nav.stop` | — | `{ok:true}` | 32 |
| `page.title` / `page.url` | — | `{value}` | 32 |
| `page.text` | `{max_bytes?}` | `{text, truncated}` | 32 |
| `page.screenshot` | `{format?}` | `C_BLOB` + `{width,height,bytes}` | 32, 50 |
| `page.eval` | `{js, world?}` | `{value}` | 32, 77 |
| `input.key` / `input.type` / `input.click` / `input.move` / `input.scroll` | see §3.1 | `{ok:true}` | 32 |
| `watch.subscribe` | `{events?}` | stream of `C_ITEM` | — |

`input.*` are routed through the **same** engine `input` command a keystroke takes
(`main.rs:594-642` → `apps/engine/src/main.js:152-224`), never through CDP. That is the
principle already established in `packages/mcp/lib/engine.js:8-12`, and it is worth
restating: one input pipeline means one set of bugs, and a page cannot tell an agent from a
person.

### 6.6 Compatibility policy

* Additive changes (new method, new optional param, new result field) require **no** version
  bump; they are announced through `features`.
* A field is never removed within an `api` major. It is marked
  `"deprecated": {"since": "0.4.0", "use": "page.text"}` in `api schema` output and kept for
  at least two minor releases.
* Behaviour changes to an existing method are forbidden. Add `page.text_v2` or a param;
  never silently change what `page.text` returns.
* `blackglass api capabilities --output json` is the machine-readable statement of all of the
  above, so a client can decide at runtime rather than parsing a version string.

---

## 7. Shell completions

### 7.1 One table, four consumers

Since there is no `clap` (§1.2), there is no completion generator for free. That is an
opportunity rather than a cost: define the command table once, in Rust, as data.

```rust
// apps/cli/src/spec.rs — the single source of truth. Illustrative shape only.
pub struct Cmd {
    pub path:     &'static [&'static str],   // ["page", "text"]
    pub plane:    Plane,                     // A | B | C  (§2)
    pub summary:  &'static str,
    pub args:     &'static [Arg],
    pub flags:    &'static [Flag],
    pub method:   Option<&'static str>,      // "page.text"
    pub exits:    &'static [Exit],           // which codes this command can return
    pub hidden:   bool,
    pub stability:Stability,                 // Stable | Preview | Hidden
}
```

Four consumers, none of which can drift from the others:

1. the argument parser,
2. `help` and `help <command>`,
3. `completions <shell>`,
4. `api schema` / `api methods` / `api exit-codes`.

A test asserts every `Cmd` with `method: Some(_)` is plane B or C, and that every method in
the API catalogue has exactly one `Cmd`. That is how G7/G8 (code and help disagreeing about
which backends exist) stop being possible.

### 7.2 Static script plus dynamic candidates

`blackglass completions zsh` emits a small static script covering the fixed tree. Anything
whose values are only known at runtime — session ids, tab ids, profile names, config keys,
history URLs — is resolved by a hidden call, the pattern Cobra popularised and which keeps
the shipped script tiny:

```
$ blackglass __complete page --session ''
{"bg":1,"type":"candidate","value":"a1b2c3d4","desc":"example.com (2 tabs)"}
{"bg":1,"type":"candidate","value":"9f2c1e7a","desc":"news.ycombinator.com"}
{"bg":1,"type":"directive","space":true,"files":false}
```

JSONL, so the same parser handles it as every other stream.

### 7.3 The latency budget is a real constraint, not a nicety

`caps::detect(fd, 300)` is called with a 300 ms deadline (`main.rs:118`), and detection works
by writing query sequences to the terminal and waiting for replies
(`crates/bg-term/src/caps.rs:113-119`). A completion handler that touched that path would
add up to 300 ms to every `<TAB>` **and would write escape sequences to the user's terminal
mid-typing.** Therefore, normatively:

* `__complete` and `completions` MUST NOT acquire the tty, MUST NOT call `caps::detect`, and
  MUST NOT start an engine.
* Budget: p99 < 50 ms. Enforce it with a test that runs `__complete` under
  `BLACKGLASS_NO_TTY=1` and asserts wall time.
* `__complete` reads session ids from the control-socket **directory listing**, not by
  connecting to each socket. Listing a directory is microseconds; a handshake is not.

### 7.4 Per-shell notes

| Shell | Mechanism | Note |
|---|---|---|
| bash | `complete -F _blackglass` | Needs `bash-completion` ≥ 2 for `_init_completion`; degrade gracefully without it |
| zsh | `#compdef blackglass`, `_arguments` | Richest: descriptions on candidates. Install to `$fpath` |
| fish | `complete -c blackglass -f -a '(blackglass __complete …)'` | `-f` matters: without it fish offers files everywhere |
| nushell | `extern` with a custom completer | Typed; the spec table maps cleanly |
| powershell | `Register-ArgumentCompleter` | For WSL/cross-platform users |
| elvish | `edit:completion:arg-completer` | Cheap once the JSONL contract exists |

URL completion (`open`, `nav goto`) draws from B09's history and bookmark stores and must be
opt-in: `[completion] history = false` by default in `config.toml` (D07 §13). Leaking browsing
history into a shell completion buffer — which lands in `~/.zsh_history` — is a privacy bug,
not a feature.

---

## 8. TypeScript client

Zero dependencies, Node ≥ 22 — the same floor `packages/mcp/lib/cdp.js:33-38` already
requires for global `WebSocket`. The framing reader is the same shape as
`apps/engine/src/main.js:266-286`, deliberately.

```ts
// packages/client-ts/src/index.ts
import { connect, Socket } from 'node:net';
import { once } from 'node:events';

const C_HELLO = 0x20, C_WELCOME = 0x21, C_REQ = 0x22,
      C_RES = 0x23, C_ITEM = 0x24, C_END = 0x25, C_BLOB = 0x26, C_CANCEL = 0x27;

export class BlackGlassError extends Error {
  constructor(readonly code: string, readonly exit: number, msg: string,
              readonly detail?: unknown) { super(msg); this.name = 'BlackGlassError'; }
}

type Pending = {
  resolve(v: any): void;
  reject(e: Error): void;
  items?: any[];
  onItem?: (item: any) => void;
  blob?: Buffer;
};

export class BlackGlass {
  private sock!: Socket;
  private buf = Buffer.alloc(0);
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private welcome: any = null;

  static async connect(socketPath: string, timeoutMs = 5000): Promise<BlackGlass> {
    const c = new BlackGlass();
    c.sock = connect(socketPath);
    c.sock.on('data', (chunk) => c.onData(chunk));
    c.sock.on('close', () => c.failAll(new BlackGlassError(
      'engine_crashed', 13, 'control socket closed')));
    await once(c.sock, 'connect');

    // Both openers may be in flight at once; this is not request/response.
    // (B06 §3.5 rule 1 — the same rule the engine handshake follows.)
    c.send(C_HELLO, Buffer.from(JSON.stringify({
      t: 'hello', magic: 'blackglass-ctl', api: 1, api_min: 1,
      impl: 'bg-client-ts/0.1.0', pid: process.pid, features: ['stream', 'blob'],
    })));

    const deadline = Date.now() + timeoutMs;
    while (!c.welcome) {
      if (Date.now() > deadline) {
        c.sock.destroy();
        throw new BlackGlassError('engine_handshake', 12, 'no welcome within timeout');
      }
      await new Promise((r) => setTimeout(r, 5));
    }
    return c;
  }

  get session(): string { return this.welcome.session; }
  has(feature: string): boolean { return this.welcome.features.includes(feature); }

  private send(type: number, payload: Buffer) {
    const head = Buffer.allocUnsafe(5);
    head.writeUInt8(type, 0);
    head.writeUInt32BE(payload.length, 1);
    this.sock.write(Buffer.concat([head, payload]));
  }

  private onData(chunk: Buffer) {
    this.buf = this.buf.length ? Buffer.concat([this.buf, chunk]) : chunk;
    for (;;) {
      if (this.buf.length < 5) return;
      const type = this.buf.readUInt8(0);
      const len = this.buf.readUInt32BE(1);
      // Enforce our own advertised limit on the LENGTH PREFIX, before buffering the
      // payload. B06 F3: a peer that claims 4 GiB must not be allowed to allocate it.
      if (len > 33_554_432) {
        this.failAll(new BlackGlassError('engine_handshake', 12, `oversize message ${len}`));
        this.sock.destroy();
        return;
      }
      if (this.buf.length < 5 + len) return;
      const payload = this.buf.subarray(5, 5 + len);
      this.buf = this.buf.subarray(5 + len);
      this.dispatch(type, payload);
    }
  }

  private dispatch(type: number, payload: Buffer) {
    if (type === C_WELCOME) { this.welcome = JSON.parse(payload.toString('utf8')); return; }
    if (type === C_BLOB) {
      const id = payload.readUInt32BE(0);
      const p = this.pending.get(id);
      if (p) p.blob = Buffer.from(payload.subarray(4));
      return;
    }
    const msg = JSON.parse(payload.toString('utf8'));
    const p = this.pending.get(msg.id);
    if (!p) return;
    switch (type) {
      case C_ITEM:
        if (p.onItem) p.onItem(msg.item); else p.items!.push(msg.item);
        break;
      case C_END:
        this.pending.delete(msg.id);
        p.resolve({ items: p.items, count: msg.count });
        break;
      case C_RES:
        this.pending.delete(msg.id);
        if (msg.ok) p.resolve(p.blob ? { ...msg.data, blob: p.blob } : msg.data);
        else p.reject(new BlackGlassError(
          msg.error.code, msg.error.exit, msg.error.msg, msg.error.detail));
        break;
    }
  }

  private failAll(e: Error) {
    for (const [, p] of this.pending) p.reject(e);
    this.pending.clear();
  }

  call<T = any>(method: string, params: object = {}): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.send(C_REQ, Buffer.from(JSON.stringify({ id, m: method, p: params })));
    });
  }

  /** Subscribe to the event stream. Returns a cancel function. */
  watch(onEvent: (e: any) => void, events?: string[]): () => void {
    const id = this.nextId++;
    this.pending.set(id, { resolve: () => {}, reject: () => {}, onItem: onEvent });
    this.send(C_REQ, Buffer.from(JSON.stringify({ id, m: 'watch.subscribe', p: { events } })));
    return () => this.send(C_CANCEL, Buffer.from(JSON.stringify({ id })));
  }

  close() { this.sock.end(); }
}
```

Usage — and note that the error handling is the point, not an afterthought:

```ts
import { BlackGlass, BlackGlassError } from '@blackglass/client';
import { writeFile } from 'node:fs/promises';

const bg = await BlackGlass.connect(process.env.BLACKGLASS_SOCKET!);

try {
  await bg.call('nav.goto', { url: 'https://example.com', wait: 'load', timeout_ms: 15_000 });
  console.log(await bg.call<{ value: string }>('page.title'));

  if (bg.has('page.screenshot')) {
    const shot = await bg.call<{ width: number; height: number; blob: Buffer }>(
      'page.screenshot', { format: 'png' });
    await writeFile('shot.png', shot.blob);
  }

  const stop = bg.watch((e) => console.log(JSON.stringify(e)), ['title', 'url', 'loading']);
  setTimeout(stop, 5_000);
} catch (e) {
  if (e instanceof BlackGlassError) {
    // The exit code travels with the error, so a wrapper script can propagate it.
    console.error(`${e.code}: ${e.message}`);
    process.exit(e.exit);          // e.g. 30 NAV_FAILED, 13 ENGINE_CRASHED
  }
  throw e;
} finally {
  bg.close();
}
```

---

## 9. Rust client

`crates/bg-client`. Blocking, `std` + `libc` only — the workspace has no async runtime today
(`Cargo.toml` workspace deps are `libc` and `flate2`) and a control client is not a reason to
acquire one. It reuses `bg_proto`'s framer, so there is exactly one framing implementation in
Rust.

```rust
//! crates/bg-client/src/lib.rs — blocking client for the BlackGlass control API.

use bg_proto::{frame_message, MessageReader};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub const C_HELLO: u8 = 0x20;
pub const C_WELCOME: u8 = 0x21;
pub const C_REQ: u8 = 0x22;
pub const C_RES: u8 = 0x23;
pub const C_ITEM: u8 = 0x24;
pub const C_END: u8 = 0x25;
pub const C_BLOB: u8 = 0x26;

pub const API: u32 = 1;
const MAX_MSG: usize = 33_554_432;

#[derive(Debug)]
pub struct ApiError {
    pub code: String,
    /// The process exit code a CLI wrapper should return (§5.1).
    pub exit: i32,
    pub msg: String,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Handshake(String),
    /// The peer announced a message larger than we will buffer (B06 F3).
    Oversize(usize),
    Api(ApiError),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

pub struct Client {
    stream: UnixStream,
    reader: MessageReader,
    next_id: u64,
    pub session: String,
    pub features: Vec<String>,
}

impl Client {
    pub fn connect(path: &Path, timeout: Duration) -> Result<Self, Error> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(timeout))?;
        let mut c = Client {
            stream,
            reader: MessageReader::new(),
            next_id: 1,
            session: String::new(),
            features: Vec::new(),
        };
        let hello = format!(
            r#"{{"t":"hello","magic":"blackglass-ctl","api":{API},"api_min":{API},"impl":"bg-client-rs/{}","pid":{}}}"#,
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
        c.stream.write_all(&frame_message(C_HELLO, hello.as_bytes()))?;

        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() > deadline {
                return Err(Error::Handshake("no welcome within timeout".into()));
            }
            let (t, payload) = c.read_message()?;
            if t == C_WELCOME {
                let w = String::from_utf8_lossy(&payload);
                if bg_proto::json_get_str(&w, "magic").as_deref() != Some("blackglass-ctl") {
                    return Err(Error::Handshake("bad magic".into()));
                }
                c.session = bg_proto::json_get_str(&w, "session").unwrap_or_default();
                return Ok(c);
            }
        }
    }

    /// Read exactly one framed message, enforcing our own size limit on the length
    /// prefix before any payload is buffered.
    fn read_message(&mut self) -> Result<(u8, Vec<u8>), Error> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            if let Some(m) = self.reader.next_message() {
                return Ok((m.type_id, m.payload));
            }
            if self.reader.buffered() > MAX_MSG {
                return Err(Error::Oversize(self.reader.buffered()));
            }
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(Error::Handshake("control socket closed".into()));
            }
            self.reader.feed(&chunk[..n]);
        }
    }

    /// Issue a request and return the raw JSON of `data`, or the mapped error.
    pub fn call(&mut self, method: &str, params_json: &str) -> Result<String, Error> {
        let id = self.next_id;
        self.next_id += 1;
        let req = format!(r#"{{"id":{id},"m":"{method}","p":{params_json}}}"#);
        self.stream.write_all(&frame_message(C_REQ, req.as_bytes()))?;

        loop {
            let (t, payload) = self.read_message()?;
            if t != C_RES {
                continue; // items/blobs for other ids
            }
            let s = String::from_utf8_lossy(&payload).into_owned();
            if bg_proto::json_get_bool(&s, "ok") == Some(true) {
                return Ok(s);
            }
            return Err(Error::Api(ApiError {
                code: bg_proto::json_get_str(&s, "code").unwrap_or_else(|| "fail".into()),
                exit: 1, // real impl parses "exit"; json_get_int is the missing helper (§11.3)
                msg: bg_proto::json_get_str(&s, "msg").unwrap_or_default(),
            }));
        }
    }

    /// Stream events until `f` returns false or the stream ends.
    pub fn watch(&mut self, mut f: impl FnMut(&str) -> bool) -> Result<(), Error> {
        let id = self.next_id;
        self.next_id += 1;
        let req = format!(r#"{{"id":{id},"m":"watch.subscribe","p":{{}}}}"#);
        self.stream.write_all(&frame_message(C_REQ, req.as_bytes()))?;
        loop {
            let (t, payload) = self.read_message()?;
            match t {
                C_ITEM => {
                    if !f(&String::from_utf8_lossy(&payload)) { return Ok(()); }
                }
                C_END => return Ok(()),
                _ => {}
            }
        }
    }
}
```

Caller, showing the exit-code propagation that makes the taxonomy worth having:

```rust
fn main() {
    let path = std::env::var("BLACKGLASS_SOCKET").expect("BLACKGLASS_SOCKET");
    let mut c = match bg_client::Client::connect(
        std::path::Path::new(&path), std::time::Duration::from_secs(5)) {
        Ok(c) => c,
        Err(e) => { eprintln!("connect: {e:?}"); std::process::exit(12); }
    };
    match c.call("nav.goto", r#"{"url":"https://example.com","wait":"load"}"#) {
        Ok(_) => {}
        Err(bg_client::Error::Api(e)) => { eprintln!("{}: {}", e.code, e.msg); std::process::exit(e.exit); }
        Err(e) => { eprintln!("{e:?}"); std::process::exit(70); }
    }
    let _ = c.watch(|line| { println!("{line}"); true });
}
```

Note what the Rust example exposes: `bg_proto` has `json_get_str` and `json_get_bool` but no
integer accessor, so `error.exit` cannot be read without adding one. That is a concrete,
one-function gap I am reporting rather than patching (§11.3).

---

## 10. Environment-variable registry

Every variable in the tree today, plus what the design needs. Registering them in one place
is the only way `doctor` can report on all of them.

| Variable | Status | Owner | Meaning |
|---|---|---|---|
| `BLACKGLASS_LOG` | exists | CLI `main.rs:33` | Diagnostic log file. Never stdout. |
| `BLACKGLASS_ENGINE` | exists | CLI `main.rs:320` | Engine directory |
| `BLACKGLASS_BACKEND` | exists | CLI `main.rs:208` | Force backend; **must** also accept `iterm2` and **must** error on unknown values (G7) |
| `BLACKGLASS_EXIT_AFTER_MS` | exists | CLI `main.rs:49` | Test hook, bounded run |
| `BLACKGLASS_HOME` | proposed | B09 §3.4 | Data root |
| `BLACKGLASS_CONFIG` | proposed | D07 §13.2 | Explicit config file |
| `BLACKGLASS_CONFIG_DIR` | proposed | D07 | Config directory |
| `BLACKGLASS_CDP` | proposed | E03 §9.2 | Opt-in DevTools port |
| `BLACKGLASS_SESSION` | **new here** | E07 §3.3 | Default `--session` |
| `BLACKGLASS_SOCKET` | **new here** | E07 §6.2 | Explicit control-socket path |
| `BLACKGLASS_OUTPUT` | **new here** | E07 §4 | Default `--output` |
| `NO_COLOR` / `CLICOLOR_FORCE` | **new here** | E07 §3.3 | Standard, third-party |

Rule: a flag always beats an environment variable, which always beats config, which beats the
built-in default. Stated once, applied everywhere, and reported by
`blackglass config show --output json` with a `source` field per key so "why is this value
set?" is answerable without guessing.

---

## 11. Verified, unverified, and open decisions

### 11.1 Verified on this machine

* The entire §1 audit: every exit code, every stream, `doctor --json` being ignored,
  `open --help` becoming a search, usage-on-error landing on stdout. Reproduced above with
  the exact commands.
* `sizeof(sun_path) == 104` on Darwin, by compiling and running a probe.
* `$TMPDIR` is 49 characters; worst-case current socket path is 97 bytes (7 bytes headroom).
* `apps/cli/Cargo.toml` depends only on `bg-term`, `bg-proto`, `libc` — no `clap`, no `serde`.
* Release binary size 619,424 bytes.

### 11.2 UNVERIFIED

* No part of the proposed control API exists; nothing in §6 has been executed. It is a design.
* `LOCAL_PEERCRED` behaviour on Darwin (§6.4) — API documented, not probed here.
* The TypeScript and Rust clients compile-check by inspection only. There is no control server
  to run them against, and I will not claim a mock is working software.
* Completion latency budget (§7.3) is derived from the 300 ms detection deadline in the source,
  not from a measured `<TAB>` round trip.

### 11.3 Decisions for the commander

1. **JSON encoding dependency.** Emitting JSON can be hand-rolled — `bg_proto::json_escape`
   already exists and the frame path must stay dependency-free. *Parsing* client requests is
   different: the control server accepts arbitrary local input, and `json_get_str` is a
   deliberately minimal scanner whose edge cases B06 F11 already flags. My recommendation is
   `serde_json` for the control server only, accepted as roughly +300 KB of binary and a few
   seconds of build time against 619 KB today. Hand-rolling a parser to save that is a poor
   trade for something on a trust boundary.
2. **`bg_proto` needs `json_get_i64`** (§9). One function. Without it no client can read
   `error.exit`.
3. **`clap` or the hand-written spec table?** The table in §7.1 gives completions, help, and
   `api schema` from one source and adds no dependency, at the cost of writing a parser. `clap`
   with `clap_complete` is faster to reach but does not generate `api schema` and pulls a
   sizeable tree. Given that the schema output is what makes the API self-describing, I lean to
   the table — but this is the commander's call and it is cheap to reverse early, expensive later.
4. **Socket-path scheme** (G14): replacing `<pid>-<nanos>` with 8 hex characters reclaims
   19 bytes of a 104-byte budget. `main.rs:394` is core source; not mine to change.
5. **`error.msg` is explicitly not API.** Worth writing into the README before anyone greps it.

---

## 12. Recommendation

Land the spec table (§7.1) first, then use it to ship `--output json|jsonl` (§4) and
`blackglass capture` (§3.1) before any other command. Those three together convert BlackGlass
from a program only a human in Ghostty can verify into one a CI job can assert on — which is
precisely the protocol-and-log-based evidence this environment already forces us toward, and
it makes every subsequent E-series and D-series feature testable the day it lands instead of
the day someone sits down at a terminal.
