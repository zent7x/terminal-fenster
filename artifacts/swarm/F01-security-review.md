# F01 — Security review of Terminal-Fenster as written

Reviewer: swarm agent F01. Date: 2026-07-31. Scope: `crates/tf-term/src/*.rs`,
`crates/tf-proto/src/lib.rs`, `apps/cli/src/main.rs`, `apps/engine/src/main.js`.
No code was edited. Every claim below is either a `file:line` citation, a primary-source
quote, or a measured result from a command shown inline. Anything I could not verify in this
environment is marked **UNVERIFIED**.

> **2026-08-01 implementation update:** this is the historical review. Findings 1–3 are now
> closed. Engine sessions install deny-by-default permission check/request/device handlers;
> external-application schemes and redirects are blocked and reported. Terminal sanitization now
> rejects bidi/invisible formatting controls, final chrome is conservatively display-column
> clipped before the autowrap cell, and the Unicode renderer uses the detected cell width. Unit
> and real-Chromium tests pin these policies. Current release truth is in `README.md` and
> `RELEASE.md`.

Baseline: `cargo test --workspace` — 87 passed, 0 failed (run 2026-07-31).

---

## Summary of findings

| # | Severity | Area | Location |
|---|---|---|---|
| 1 | **High** | Electron grants every web permission by default; includes silent external-protocol app launch | `apps/engine/src/main.js:101-134` |
| 2 | Medium | `sanitize_for_terminal` char set is incomplete — bidi/invisible formatting chars survive into the address bar | `crates/tf-term/src/unicode.rs:60-75` |
| 3 | Medium | Status bar is budgeted in `char`s, not display columns, and never against terminal width — page-controllable line overflow | `apps/cli/src/main.rs:885-896` |
| 4 | Medium | Unbounded allocation from the length prefix, in both directions, plus unbounded event backlog and unbounded title retention | `crates/tf-proto/src/lib.rs:81-93`, `apps/engine/src/main.js:266-286`, `:60-62`, `apps/cli/src/main.rs:786-790` |
| 5 | Medium | No session partition / userData path — browsing state persists to a default Electron profile forever, with no way to clear it | `apps/engine/src/main.js:101-114` |
| 6 | Low | Page-derived text reaches `$TERMINAL_FENSTER_LOG` unsanitized; C1 and bidi survive `JSON.stringify` | `apps/cli/src/main.rs:549` |
| 7 | Low | `expected_payload()` multiplies unchecked; the only downstream guard is an `assert_eq!` panic | `crates/tf-proto/src/lib.rs:51-53`, `crates/tf-term/src/kitty.rs:108` |
| 8 | Low | Socket directory/file permission race; `create_dir_all` accepts a pre-existing or symlinked path | `apps/cli/src/main.rs:393-400` |
| 9 | Low | The first process to connect is trusted as "the engine" — no peer-credential check | `apps/cli/src/main.rs:416-430` |
| 10 | Info | `navigate` command accepts any scheme with no allowlist | `apps/engine/src/main.js:228-230` |
| 11 | Info | `setWindowOpenHandler` is correct, but the `popup` event it emits is never consumed | `apps/engine/src/main.js:129-132`, `apps/cli/src/main.rs:783-802` |

---

## 1. HIGH — Electron's default permission model grants everything, including external-protocol launches

**Location:** `apps/engine/src/main.js:101-134` (`createWindow`) — the omission is that neither
`session.setPermissionRequestHandler` nor `session.setPermissionCheckHandler` is ever called.

Verified absence:

```
$ grep -n "userData\|setPath\|user-data-dir\|partition\|session\." apps/engine/src/main.js
(none)
```

The `webPreferences` block is genuinely well configured — `nodeIntegration:false`,
`contextIsolation:true`, `sandbox:true`, `webSecurity:true` (`main.js:106-113`), no preload,
no IPC bridge, and no `--no-sandbox` anywhere in the repo. That is the renderer-escape story,
and it is solid. Permissions are a **separate** subsystem in the browser process that this
posture does not cover.

Primary sources, Electron v43.2.0 (the exact version in `apps/engine/node_modules/electron/package.json`):

- `docs/tutorial/security.md:283` — Electron approves all permission requests by default when
  the developer has configured no custom handler.
- `shell/browser/electron_permission_manager.cc:228-251` — with `request_handler_` null, the
  loop emits `PermissionStatus::GRANTED` for every requested permission.
- `shell/browser/electron_permission_manager.cc:358-363` — with `check_handler_` null,
  `CheckPermissionWithDetails` returns `true` for everything except
  `DEPRECATED_SYNC_CLIPBOARD_READ`.

So a page loaded in Terminal-Fenster is silently granted `media` (camera and microphone),
`geolocation`, `notifications`, `clipboard-read`, `midi`/`midiSysex`, `display-capture`,
`pointerLock`, `fullscreen`, `idle-detection`, `window-management` and `storage-access`.
Because the `BrowserWindow` is `show:false` and offscreen, there is no window, no prompt, and
no indicator — the user has no channel through which to learn any of this happened.

The sharpest edge is **external protocol launching**:

- `shell/browser/electron_browser_client.cc:1069-1090` → `HandleExternalProtocolInUI`
- `:1040-1059` — the only gate is for sandboxed sub-frames; a top-level navigation from an
  ordinary page passes straight through
- `:1061-1064` → `RequestOpenExternalPermission`
- `shell/browser/web_contents_permission_helper.cc:304-311` → `RequestPermission(..., OPEN_EXTERNAL, ...)`
  → the default-grant path above
- `electron_browser_client.cc:1006-1009` → `platform_util::OpenExternal(escaped_url, ...)`

Net effect: a hostile page can navigate to any registered URL scheme on the user's machine
(`x-apple-*`, `ms-*`, vendor updaters, IDE and VPN handlers, `zoommtg:`, etc.) and the host OS
will launch the handler application — with no window, no prompt, and no log entry the user
will see. For a tool whose whole premise is "browse arbitrary sites from a terminal", this is
the one finding I would block a release on.

**Concrete fix** (engine-owned, so described rather than applied). In `app.whenReady()`, before
`createWindow`:

```js
const { session } = require('electron');
const ALLOW = new Set([]);              // grant nothing by default
const s = session.defaultSession;
s.setPermissionRequestHandler((_wc, permission, cb, details) => {
  sendEvent({ t: 'permission', permission, url: details && details.requestingUrl, granted: false });
  cb(ALLOW.has(permission));
});
s.setPermissionCheckHandler((_wc, permission) => ALLOW.has(permission));
s.setDevicePermissionHandler(() => false);
```

Deny-by-default plus an event so the terminal core can eventually show "this page asked for the
camera". If interactive grants are wanted later, they belong behind an explicit keybinding, not
behind silence. `OPEN_EXTERNAL` in particular should stay denied until there is a UI that can
show the user which application is about to launch.

---

## 2. MEDIUM — `sanitize_for_terminal`'s character set is incomplete

**Location:** `crates/tf-term/src/unicode.rs:60-75`.

Placement first, because the mission asked: sanitization *is* applied at every point where
page-derived text reaches the terminal today.

```
$ grep -rn "sanitize_for_terminal" --include="*.rs" . | grep -v node_modules | grep -v unicode.rs
apps/cli/src/main.rs:887:        let title = unicode::sanitize_for_terminal(&status.title, 40);
apps/cli/src/main.rs:888:        let url = unicode::sanitize_for_terminal(&status.url, 60);
```

Those are the only two page-derived sinks that write to the tty. `doctor`'s terminal replies go
through `caps::escape_for_display` (`caps.rs:233-244`), which is independently safe. The
kitty payload is base64 (`kitty.rs:118-119`), so page *pixels* cannot smuggle escapes. Good.

The gap is the predicate itself. It covers C0, DEL, C1 and U+2028/2029 — which correctly kills
the ESC/OSC-52 class of attack the existing tests pin (`unicode.rs:126-142`). It does not cover
Unicode **bidi controls** or **invisible formatting characters**. Measured, using a verbatim
copy of lines 60-75 compiled standalone:

```
$ rustc -O probe.rs -o probe && ./probe
bidi RLO U+202E survives: true
U+200B ZWSP survives: true
U+200E LRM survives: true
U+2066 LRI survives: true
U+FEFF BOM survives: true
U+00AD SHY survives: true
U+061C ALM survives: true
```

Why it matters here specifically: `main.rs:890-895` renders title and URL adjacent on one line
— that line **is** the address bar. A page whose title ends in U+202E can visually reverse the
URL that follows it, or a title of `evil.test` + U+2066 isolates can make the rendered order
disagree with the logical order. This is the classic homograph/trojan-source presentation
attack, aimed at the one UI element the user is supposed to trust. Zero-width characters
additionally let a page consume the 40/60 char budget while rendering nothing, pushing the real
URL out of view.

**Concrete fix** — extend the `dangerous` predicate at `unicode.rs:68-71`:

```rust
|| c == 0x061C                       // Arabic letter mark
|| (0x200B..=0x200F).contains(&c)    // ZWSP, ZWNJ, ZWJ, LRM, RLM
|| (0x202A..=0x202E).contains(&c)    // LRE, RLE, PDF, LRO, RLO
|| (0x2066..=0x2069).contains(&c)    // LRI, RLI, FSI, PDI
|| c == 0x00AD || c == 0x2060 || c == 0xFEFF  // SHY, word joiner, BOM/ZWNBSP
|| (0xFFF9..=0xFFFB).contains(&c)    // interlinear annotation
```

and add tests mirroring `sanitize_strips_c1_controls`, one per class. Consider also collapsing
runs of combining marks: `./probe` shows 39 stacking U+0301 survive and render as a single
visible cell, which both defeats the length budget and can overdraw neighbouring cells.

---

## 3. MEDIUM — the status bar is budgeted in `char`s, never in columns, and never against terminal width

**Location:** `apps/cli/src/main.rs:885-896`.

Two independent errors compound:

1. `sanitize_for_terminal(..., 40)` counts `char`s (`unicode.rs:63`). East-Asian wide
   characters and emoji occupy two terminal columns each, so a 40-char title is up to 80
   columns. Measured: `title chars=41 but terminal columns=82 (wide CJK)`.
2. Nothing ever compares the assembled bar against `c.winsize.cols`. The value is available
   (`main.rs:286` passes `rows`, not `cols`) but unused.

Worst case for a pure-ASCII bar, computed from the format string at `main.rs:890-895`:

```
$ python3 -c "...(format string reproduced)..."
max status-bar columns (pure ASCII): 147
Ghostty cols measured on this machine: 146 -> overflow by 1
```

So on the project's own reference terminal a page with a long title and long URL already
overflows the last row by one column. Writing past the last column of the last row scrolls the
screen in most terminals, which drags the kitty image placed at `\x1b[H` (`main.rs:855`) up one
row — and it does it again on the next frame. A CJK title reaches ~230 columns and makes this
severe. This is display-integrity corruption that a page controls, which is why I am filing it
as a security finding rather than a cosmetic one.

**Concrete fix:** pass `c.winsize.cols` into `Renderer::present`, compute the fixed-chrome width
first, split the remainder between title and URL, and measure in display columns rather than
`char`s (a small in-tree `char_width` covering the East-Asian Wide/Fullwidth ranges plus
zero-width combining marks avoids adding the `unicode-width` dependency). Then truncate the
final assembled bar to `cols - 1` as a belt-and-braces guard, and add a test asserting the bar
never exceeds `cols` for a hostile title.

---

## 4. MEDIUM — unbounded allocation and unbounded queueing, in both directions

Four related spots, all reachable from a page that simply sets a very large `document.title` in
a loop.

- `crates/tf-proto/src/lib.rs:81-93` — `next_message` reads a `u32` length (up to 4 GiB) and
  waits for `self.buf.len() >= 5 + len`. `feed` (`:72-74`) appends with no cap. The reader will
  happily accumulate gigabytes before it ever yields a message.
- `apps/engine/src/main.js:266-286` — the JS side has the identical shape: `buf` grows via
  `Buffer.concat` with no ceiling and `len` is unvalidated.
- `apps/engine/src/main.js:60-62` — `sendEvent` discards `sendMessage`'s return value. Frames
  are carefully coalesced under backpressure (`:64-80`, and the comment at `:42-47` is right),
  but events are not: if the terminal core is slow, Node buffers every event in user space.
- `apps/cli/src/main.rs:786-790` — `status.title` stores the entire decoded title, unbounded,
  even though only 40 chars are ever rendered.

This is not remote code execution; it is a page-triggered OOM of the user's terminal browser,
and `json_get_str` (`tf-proto/src/lib.rs:125-155`) does a fresh `String` allocation per event on
top. Severity is Medium rather than High only because the socket peer is our own child process
behind a 0600 socket.

**Concrete fix:** add `pub const MAX_MESSAGE: usize = 64 << 20;` to `tf-proto`, reject and
surface an error when `len > MAX_MESSAGE` in both `MessageReader::next_message` and
`attachReader`, and cap event payloads much lower (`MAX_EVENT: usize = 64 << 10`). Truncate the
title at the engine (`main.js:117`: `title.slice(0, 512)`) so the large string never crosses the
socket at all, and make `sendEvent` drop-oldest when `sock.write` returns false.

---

## 5. MEDIUM — no session partition or userData path: browsing state persists silently and forever

**Location:** `apps/engine/src/main.js:101-114`.

`webPreferences` sets no `partition`, the app never calls `app.setPath('userData', ...)`, and no
`--user-data-dir` is passed on the command line built at `apps/cli/src/main.rs:402-411`. The
default persistent session is therefore used: cookies, localStorage, IndexedDB, service workers
and the HTTP cache are written to Electron's default profile directory and survive every
subsequent run, with no incognito mode and no way for the user to clear them.

The project already knows the right answer elsewhere — `packages/mcp/lib/engine.js:115` passes
`--user-data-dir=${this.profileDir}` for a throwaway profile. The interactive path is the one
that omits it, which is backwards: the interactive path is where real logins happen.

The exact on-disk path depends on `app.getName()` resolution for the scoped package name
`@terminal-fenster/engine` and I could not run Electron here to confirm it (see Environment section),
so the **path is UNVERIFIED**; the absence of any configuration is verified by the grep above.

**Concrete fix:** have the CLI pass `--user-data-dir=<socket_dir>/profile` for an ephemeral
session by default, with an opt-in `--persist` flag that points at a documented, per-user 0700
directory. Add a `t:"clear"` command that calls `session.defaultSession.clearStorageData()`.

---

## 6. LOW — page-derived text reaches `$TERMINAL_FENSTER_LOG` unsanitized

**Location:** `apps/cli/src/main.rs:549` — `log_line(&format!("event {s}"))`, where `s` is the
raw event JSON containing the attacker-controlled title and URL. Also `:257`, which logs the
URL from `argv`.

`JSON.stringify` in the engine (`main.js:61`) does escape C0, so the ESC/OSC class is handled.
It does not escape C1 or bidi. Measured:

```
$ node j.js
JSON.stringify output (hex): ...4142435c75303031625d303b70776ec29b33316de280ae44...
ESC 0x1b escaped away  : true
C1 CSI U+009B survives : true
bidi RLO survives      : true
```

`c2 9b` is U+009B (C1 CSI) and `e2 80 ae` is U+202E, both written raw into the log file. The
code's own rationale at `unicode.rs:58-59` states that a terminal decoding UTF-8 treats C1 as a
control introducer — by the project's own threat model, `cat terminal-fenster.log` is then an
injection sink. Whether Ghostty 1.3.1 specifically acts on C1 from UTF-8 is **UNVERIFIED**
(that would need a screen I cannot reach; xterm's `allowC1Printable` exists precisely because
some terminals do).

**Concrete fix:** route event logging through `unicode::sanitize_for_terminal(&s, 512)` (or a
`sanitize_for_log` variant that also escapes rather than replaces, so diagnostics stay useful),
and create the log file with mode 0600 at `main.rs:38` — right now it inherits the umask.

---

## 7. LOW — unchecked geometry arithmetic, guarded only by a panic

**Locations:** `crates/tf-proto/src/lib.rs:51-53` and `crates/tf-term/src/kitty.rs:108`.

`expected_payload()` computes `width as usize * height as usize * 4` with no overflow check. In
release mode that wraps. Measured:

```
expected_payload(2147483648x2147483648) wraps to 0 (checked = None)
```

A header claiming 2^31 x 2^31 yields an expected payload of `0`, so the truncation guard at
`apps/cli/src/main.rs:834` passes, and `Renderer` then sets `page_w`/`page_h` to 2^31 while
holding an empty RGB buffer. The `self.rgb.is_empty()` early return at `main.rs:849` catches
exactly this case today — the bug is latent, not live — but the safety net is one line away from
being removed, and the next line of defence is `assert_eq!` in `encode_rgb_frame`
(`kitty.rs:108`), i.e. a panic. A panic is memory-safe and the `TtyGuard` panic hook
(`tty.rs:126-129`) restores the terminal, so the blast radius is a crash, not corruption.

**Concrete fix:** make `expected_payload` return `Option<usize>` using `checked_mul`, return
`None` from `FrameHeader::parse` for implausible geometry (e.g. `width`/`height` > 16384), and
convert `kitty.rs:108`'s `assert_eq!` into an `Err(io::ErrorKind::InvalidInput)` return —
`encode_rgb_frame` already returns `io::Result`.

---

## 8. LOW — socket directory creation is not atomic and accepts a pre-existing path

**Location:** `apps/cli/src/main.rs:393-400`.

```rust
std::fs::create_dir_all(&socket_dir)?;
set_mode(&socket_dir, 0o700)?;
let socket_path = socket_dir.join("engine.sock");
let listener = UnixListener::bind(&socket_path)?;
set_mode(&socket_path, 0o600)?;
```

The intent stated at `:387-388` is right and the end state is right. Three gaps:

1. `create_dir_all` succeeds if the path already exists, including as a symlink to an
   attacker-controlled directory; `set_mode` then follows the symlink.
2. Between `create_dir_all` and `set_mode` the directory carries umask-default permissions
   (typically 0755).
3. Between `bind` and `set_mode` the socket carries umask-default permissions. On Linux,
   `connect(2)` to a Unix socket requires write permission on the socket file, so that window
   is genuinely world-connectable.

Exploiting any of these requires predicting `terminal-fenster-{pid}-{nanos}` and winning a
microsecond race, and on macOS `TMPDIR` is already a per-user 0700 directory, so this is Low.
On Linux with a shared `/tmp` it is real.

**Concrete fix:** `std::os::unix::fs::DirBuilderExt` — `DirBuilder::new().mode(0o700).create(&dir)?`
creates atomically with the right mode and fails if the path exists (closing gaps 1 and 2), and
set the process umask to `0o077` around the `bind` call to close gap 3.

---

## 9. LOW — the first connection is trusted as "the engine"

**Location:** `apps/cli/src/main.rs:416-430`.

`listener.accept()` takes whoever connects first and treats that stream as the engine for the
rest of the session. Any same-uid process that wins the race supplies frames rendered straight
to the user's terminal and events shown in the address bar. Given the 0700 directory this needs
same-uid access, which is already a strong position — hence Low — but the check is cheap.

**Concrete fix:** after `accept`, read the peer PID (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on
Linux) and compare against `child.id()`, aborting on mismatch. The structurally better fix,
which removes the whole class, is to hand the child a pre-connected `UnixStream::pair()` file
descriptor via `CommandExt::pre_exec` + an inherited fd number instead of using a filesystem
socket at all. That also deletes findings 8 entirely.

---

## 10. INFO — `navigate` accepts any scheme

`apps/engine/src/main.js:228-230` calls `win.loadURL(cmd.url)` with no scheme validation. The
interactive CLI never sends `navigate` (only `reload`/`back`/`forward`/`input`/`quit` —
`apps/cli/src/main.rs:574-658`), so today the only entry point is `--bg-url`, which is the
user's own argv. `normalize_url` (`:293-304`) passes `about:`, `data:` and anything containing
`://` through unchanged, including `file://`. That is defensible for a browser CLI. It becomes
material the moment a second client (the MCP package) can drive `navigate`, and it interacts
badly with finding 1. Worth an allowlist (`http`, `https`, `file`, `about`, `data`) at the
engine boundary so the policy lives next to the sink.

## 11. INFO — `setWindowOpenHandler` is correct, but its event goes nowhere

`apps/engine/src/main.js:129-132` returns `{ action: 'deny' }` for every `window.open` /
`target=_blank`, which is the right default and correctly prevents an invisible popup window
from being created with inherited `webPreferences`. Two observations:

- The `t:'popup'` event it emits has no arm in `Status::apply_event`
  (`apps/cli/src/main.rs:783-802`), so it only ever reaches the log file. A user clicking a
  `target=_blank` link sees nothing happen at all. That is a functional gap that will read as a
  bug, and it deserves at least a status-bar notice.
- There is no `app.on('web-contents-created', ...)` applying the same policy globally. Today
  `webviewTag` defaults to `false` and no other `webContents` is created, so nothing is exposed;
  it is worth adding before any second surface exists.

---

## What I checked and found sound

Stated explicitly so the commander does not re-spend effort here.

- **Electron hardening.** `nodeIntegration:false`, `contextIsolation:true`, `sandbox:true`,
  `webSecurity:true` (`main.js:106-113`), no preload script, no `ipcMain` handlers, no
  `--no-sandbox` / `--ignore-certificate-errors` / `--disable-web-security` anywhere in the
  repo (grep across `*.js`, `*.json`, `*.rs` excluding `node_modules`). The spikes
  (`apps/engine/spike/*.js`) carry the same flags. Renderer→main escape surface is genuinely
  minimal: there is no bridge to attack.
- **Escape-injection sinks.** Both tty sinks for page-derived text are sanitized
  (`main.rs:887-888`); `doctor` escapes terminal replies via `escape_for_display`
  (`caps.rs:233-244`); kitty payloads are base64 so pixel data cannot carry escapes; the
  restore sequence is complete and test-pinned (`tty.rs:27-40`, `tty.rs:223-234`) and runs on
  drop, panic, SIGINT/TERM/HUP/QUIT.
- **Input decoding.** `parse_u32` uses `checked_mul`/`checked_add` (`input.rs:471-483`),
  `decode_utf8` handles partial and invalid sequences without panicking (`:486-504`), bracketed
  paste treats escape-looking bytes as literal text and has a test that says so
  (`input.rs:735-740`). The fuzz-lite test at `:754-767` is the right instinct.
- **Socket exposure.** No listening TCP port is opened by `apps/engine/src/main.js`; the engine
  only connects out to the path it is handed. (Note for the commander: `packages/mcp/lib/engine.js:115`
  *does* pass `--remote-debugging-port=0`, an unauthenticated loopback CDP listener. Its header
  comment at `:14-22` documents this honestly and it is opt-out via `TERMINAL_FENSTER_MCP_CDP=0`.
  Out of scope for F01, but it is the largest security delta in the tree and should get its own review.)
- **Licensing.** No third-party code reuse is proposed by this review. `apps/engine/node_modules/electron/package.json`
  declares MIT; `tf-term` depends only on `libc` and `flate2` (both MIT/Apache-2.0); `b64.rs` is
  in-tree and hand-written. Minor: there is no `LICENSE` file at the repo root, though
  `Cargo.toml` references `license.workspace`.

---

## Environment limitations

- Electron cannot be launched under the agent Bash sandbox (`bootstrap_look_up ... Permission denied`
  for Chromium child processes), so findings 1 and 5 are established from Electron v43.2.0
  source and documentation rather than from a live run. The source citations are exact file and
  line references in the pinned version; a 15-minute manual confirmation (load a page calling
  `navigator.geolocation.getCurrentPosition` and a page navigating to a custom scheme) would
  convert them from source-derived to observed, and I recommend it before the fix lands so
  there is a before/after.
- The machine is at a lock screen, so no rendered-terminal screenshot was taken. Findings 2, 3,
  6 and 7 were instead reproduced by compiling the exact predicate and arithmetic standalone
  and by exercising `JSON.stringify` under Node; those commands and their outputs are shown
  inline above and are CI-able as-is.
