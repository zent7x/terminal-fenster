# E01 — The Authenticated CDP Broker

**Mission:** specify how Terminal-Fenster exposes the Chrome DevTools Protocol to automation clients
(Playwright, agents) without creating the total-browser-takeover hole that an open CDP port is.

**Status:** design specified, and the load-bearing claims are **empirically verified end-to-end on this
machine** (macOS 26.1, Apple M4, Electron 43.2.0 / Chromium 150.0.7871.129, playwright-core 1.62.1,
Node v24.11.1). A working broker prototype exists and Playwright drives a real browser through it.

**Owner of this file:** E01. This document proposes changes to `apps/engine/src/main.js`,
`apps/cli/`, and `crates/` but **makes none of them** — per the file-ownership rule they are
described in §11 for the commander to apply.

---

## 1. The decision, in one paragraph

Terminal-Fenster exposes CDP by launching the Electron host with **`--remote-debugging-pipe`** (CDP over
inherited file descriptors 3 and 4 — *no listening socket of any kind*), and by placing a **broker**
in the Rust core that holds the only ends of that pipe. When — and only when — the user grants
automation, the broker opens a **Unix-domain WebSocket endpoint** guarded by a capability token in
the `Authorization` header, a peer-credential check, and a per-method capability filter. Playwright
connects to it with `chromium.connectOverCDP('ws+unix://…', { headers: { Authorization: … } })`.
There is never a TCP port, the raw CDP surface is never proxied verbatim, and the whole facility is
**off by default**.

The reason this is not merely "an open port with a password on it" is §7: the broker is a
*policy-enforcing* protocol translator, not a tunnel.

---

## 2. Verified findings

Every row below was produced by a command run on this machine during this mission. Prototype sources
live in `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/e01/`
(`broker.js`, `client.mjs`, `pipetest.js`, `takeover.mjs`, `peercred.py`).

| # | Claim | Result | Evidence |
|---|---|---|---|
| **V1** | **Electron 43.2.0 honours `--remote-debugging-port` on the DEFAULT userData dir.** It does **not** inherit Chrome 136's protection. | **CONFIRMED** | `Electron . --remote-debugging-port=9444` with no `--user-data-dir` → `DevTools listening on ws://127.0.0.1:9444/devtools/browser/740f0d1e-…`; `curl 127.0.0.1:9444/json/version` returned `Chrome/150.0.7871.129 … Electron/43.2.0`, userData was the default `~/Library/Application Support/e01`. |
| **V2** | Source reason for V1: the default-profile check lives in the `//chrome` layer Electron does not build, and is additionally gated on Google-Chrome branding. | **CONFIRMED** | `chrome/browser/devtools/remote_debugging_server.cc`, fn `IsRemoteDebuggingAllowed` (line 163): under `#if BUILDFLAG(GOOGLE_CHROME_BRANDING)` `default_user_data_dir_check_enabled = true`, `#else` it is a **testing-only global defaulting to false** (line 52). Electron is neither Chrome-branded nor a `//chrome` build. |
| **V3** | An open CDP port is total takeover from **any** local process with **zero** credentials. | **CONFIRMED** | `takeover.mjs` (an unrelated Node process) → `connectOverCDP('http://127.0.0.1:9444')` → `Storage.getCookies` returned `[{"n":"SESSIONID","v":"super-secret-session-abc123","httpOnly":true,"secure":true}]` **in plaintext**, plus arbitrary JS execution. |
| **V4** | `--remote-debugging-pipe` works in Electron 43.2.0: fd 3 = browser reads, fd 4 = browser writes, **NUL-delimited JSON** framing. | **CONFIRMED** | `pipetest.js`: `{"id":1,"method":"Browser.getVersion"}\0` on fd 3 → reply on fd 4. Framing matches `content/browser/devtools/devtools_pipe_handler.cc` `PipeWriterASCIIZ` (line 292). |
| **V5** | The pipe gives **full browser-level CDP** — the thing `webContents.debugger` cannot give. | **CONFIRMED** | Over the pipe, `Browser.getVersion` **and** `Target.getTargets` both succeeded (returned `targetInfos` incl. the page target). |
| **V6** | With `--remote-debugging-pipe`, the browser process opens **zero** listening sockets. | **CONFIRMED** | `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` → `LISTENING_SOCKETS_ON_BROWSER_PID=0`; repeated across the whole helper tree while Playwright was attached → `0`. |
| **V7** | **Playwright connects over a Unix socket** via the `ws+unix://` scheme. | **CONFIRMED** | `chromium.connectOverCDP('ws+unix:///…/cdp.sock:/devtools/browser/terminal-fenster')` → `CONNECTED version= 150.0.7871.129`. |
| **V8** | A `ws://`-scheme endpoint **skips** DevTools HTTP discovery entirely — we need no `/json/*` endpoints. | **CONFIRMED** | `chromium.ts:459-460`: `if (endpointURL.startsWith('ws')) return endpointURL;` — `urlToWSEndpoint` returns immediately, no `/json/version` fetch. Observed: broker served zero HTTP requests other than the Upgrade. |
| **V9** | A bearer token in the WS handshake gates the connection, and Playwright supports supplying it. | **CONFIRMED** | Wrong token → `CONNECT_FAILED: … 401 Unauthorized`. Correct token → connected. Option verified in `chromium.ts:90-98` (`headers`) and `transport.ts:136-139` (`new ws(url, [], { headers: options.headers, … })`). |
| **V10** | Real automation works through the broker. | **CONFIRMED** | Through the broker: `title= E01`, `textContent('h1') = e01 probe`, `setContent`, `fill('#q')`, `inputValue = typed via broker`. |
| **V11** | The capability filter blocks cookie theft **while Playwright keeps working**. | **CONFIRMED** | `COOKIE_DUMP_BLOCKED: Protocol error (Storage.getCookies): Terminal-Fenster: method Storage.getCookies is not in this grant`, and in the same session `Browser.getVersion OK product= Chrome/150.0.7871.129`. |
| **V12** | **Playwright's own connect handshake calls `Browser.setDownloadBehavior`.** Denying it breaks `connectOverCDP` outright. | **CONFIRMED** | First filtered run failed with `CONNECT_FAILED: Protocol error (Browser.setDownloadBehavior): … not in this grant`. Fixed by **virtualizing** (rewriting `downloadPath`) instead of denying. |
| **V13** | `Network.getAllCookies` **no longer exists** at browser level in Chromium 150; the live cookie-dump method is `Storage.getCookies`. | **CONFIRMED** | `Protocol error (Network.getAllCookies): 'Network.getAllCookies' wasn't found`. A deny-list naming only the old method is **useless**. |
| **V14** | macOS Unix sockets expose peer identity: `LOCAL_PEERCRED`, `LOCAL_PEERPID`, and `LOCAL_PEERTOKEN`. | **CONFIRMED** | `peercred.py`: `LOCAL_PEERCRED … uid=501`; `LOCAL_PEERPID … pid=57395`; `LOCAL_PEERTOKEN ok: 32 bytes audit_token_t = (501, 501, 20, 501, 20, 57395, 100026, 11504631)` — the last field is `pidversion`, which defeats PID-reuse races. |

### 2.1 Measured CDP method inventory

A minimal Playwright session (connect, read title, read text, set content, fill an input, one raw
browser CDP session) sent **32 calls across 20 distinct methods**, captured by the broker:

```
   9  Runtime.callFunctionOn          1  Page.setLifecycleEventsEnabled
   2  Browser.getVersion              1  Runtime.enable
   2  Target.setAutoAttach            1  Page.addScriptToEvaluateOnNewDocument
   2  Runtime.evaluate                1  Network.enable
   2  Runtime.releaseObject           1  Emulation.setFocusEmulationEnabled
   1  Browser.setDownloadBehavior     1  Emulation.setEmulatedMedia
   1  Page.enable                     1  Runtime.runIfWaitingForDebugger
   1  Page.getFrameTree               1  Target.getTargetInfo
   1  Log.enable                      1  Page.createIsolatedWorld
                                      1  Input.insertText
                                      1  Target.attachToBrowserTarget
```

This is the empirical floor for the allowlist (§8). It is a floor, not the whole set — screenshots,
navigation, file upload, and network interception were not exercised.

### 2.2 An operational finding the commander should act on

While testing I found **an unauthenticated CDP port already open on this development machine**:

```
Electron 37164 builder 31u IPv4 … TCP 127.0.0.1:9333 (LISTEN)
```

It belongs to a sibling agent's `cdptest` fixture (its `/json/version` reports
`cdptest/1.0.0 … Electron/43.2.0`). I did **not** kill it — it is not my process and may be in use.
It is a live demonstration of exactly the failure this document exists to prevent: any process on
this machine can currently take that browser over. **Recommend a repo-wide rule that test fixtures
never use `--remote-debugging-port`, plus the CI check in §10 (T-CDP-1).** My own test processes were
cleaned up (port 9444 confirmed closed); four orphaned Electron *helper* processes from killed
parents remain but hold **no** listening sockets.

---

## 3. Why the obvious options are wrong

### 3.1 `--remote-debugging-port` — rejected

V1 and V3 together are decisive. In Electron the switch works on the default profile, and the
resulting endpoint has **no authentication of any kind**. The DevTools HTTP handler binds
`127.0.0.1`, and loopback is not a security boundary on a multi-process desktop: every process
running as the user, every `npx` postinstall script, every VS Code extension, every browser
extension with native messaging, can reach it. V3 shows the payoff for an attacker is `HttpOnly`
cookies in plaintext plus arbitrary JS in every logged-in origin.

Two further properties make it worse than it first looks:

- **It defeats at-rest encryption by construction.** A09 §5 has us flipping the `EnableCookieEncryption`
  fuse. That protects the SQLite file from a `tar` of `~/Library/Application Support`. It does
  nothing here, because CDP asks the *running browser* — which already holds the key — to decrypt.
  This is precisely why cookie-theft malware moved to CDP after App-Bound Encryption shipped.
- **A web page can sometimes reach it.** A loopback HTTP endpoint is reachable by `fetch()` from a
  malicious page under DNS-rebinding conditions (A09 T14). The DevTools handler does validate the
  `Host` header, which blunts the classic rebind, but relying on that is a single-check defence for
  a total-compromise capability.

**Verdict: never. `--remote-debugging-port` stays on the argv guard's fatal list, unconditionally.**

### 3.2 `webContents.debugger` — necessary but insufficient

Electron's per-`webContents` debugger (`attach`, `sendCommand(method, params, sessionId)`,
`on('message')`) is attractive because it adds **zero** OS-level attack surface: it is an in-process
API, no fd, no socket, no flag. For Terminal-Fenster's *own* internal needs (E-class agent verbs, DOM
snapshots for the terminal UI, accessibility extraction) it is the correct tool and should be used.

But it cannot serve Playwright. `connectOverCDP` requires the **browser** target: the measured
inventory (§2.1) includes `Browser.getVersion`, `Target.setAutoAttach`, `Target.getTargetInfo` and
`Target.attachToBrowserTarget`. `webContents.debugger` attaches to *one page target*; there is no
browser-level session to route those to. V5 confirms the pipe does provide them.

**Verdict: use `webContents.debugger` for first-party verbs; it is not a Playwright transport.**

### 3.3 `--remote-debugging-pipe` — chosen

V4/V5/V6: full browser-level CDP, zero listening sockets. The authority it confers is bounded by
**who holds fd 3 and fd 4**, and only the process that spawned the engine can hold them. There is no
path in the filesystem, no port in the netstat table, and nothing for a same-uid process to connect
to. This is the single most important property in the design.

---

## 4. Resolving the A09 contradiction

A09 §2.3's argv guard lists `--remote-debugging-pipe` as fatal, while A09 §4.5 rule 2 recommends
that exact flag as the safe alternative. As written, the guard forbids the design A09 itself
prefers. This needs a decision, and the resolution follows from *what authority each flag grants an
attacker who controls our argv*:

- `--remote-debugging-port=N` grants authority **to everyone**. Anyone who can add it to our command
  line opens the browser to every process on the box. It must stay fatal.
- `--remote-debugging-pipe` grants authority **only to our parent process**, because CDP then flows
  over fds 3/4 that the parent must have created. An attacker who can both set our argv *and* be our
  parent already owns us completely — they could simply run their own Chromium. The flag adds
  nothing to their position.

**Recommendation: split the guard.** `--remote-debugging-port` (and `--inspect*`) stay
unconditionally fatal. `--remote-debugging-pipe` becomes *permitted*, because it is inert without
the parent-supplied descriptors. The guard should additionally **verify** the pipe is ours: if the
flag is present, assert that fds 3 and 4 are pipes (`fstat` → `S_ISFIFO` or a socketpair) and that
automation was requested through the authenticated control channel; otherwise exit 70.

This keeps A09's intent (no listener, no ambient authority) while making the design implementable.

---

## 5. Architecture

```
  ┌─────────────────┐                       ┌──────────────────────────────────┐
  │  Playwright /   │                       │  Rust core  (apps/cli)           │
  │  agent process  │                       │                                  │
  │                 │  ws+unix, 0600 socket │   ┌──────────────────────────┐   │
  │  connectOverCDP ├──────────────────────►│   │      CDP BROKER          │   │
  │  + Bearer token │  Authorization: …     │   │  · token verify (CT-eq)  │   │
  └─────────────────┘                       │   │  · LOCAL_PEERTOKEN check │   │
                                            │   │  · method policy engine  │   │
   ── automation plane (opt-in) ────────────│   │  · target scoping        │   │
                                            │   │  · audit log             │   │
   ── control plane (always) ───────────────│   └───────────┬──────────────┘   │
                                            │               │ fd 3 / fd 4      │
  ┌─────────────────┐   socketpair (B06)    │               │ NUL-delim JSON   │
  │ Electron engine │◄──────────────────────┤               │                  │
  │ apps/engine     │   no path, no listener└───────────────┼──────────────────┘
  │                 │                                       │
  │  Chromium 150   │◄──────────────────────────────────────┘
  │  BrowserWindow  │   --remote-debugging-pipe
  └─────────────────┘
```

Two planes, deliberately separate:

- **Control plane** (existing, B06): Rust core ⇄ engine over a `socketpair`. No filesystem path, no
  `accept()`, nothing to connect to. Unchanged by this design.
- **Automation plane** (new, this document): the broker's Unix listener. It exists only while a
  grant is live.

### 5.1 Why the automation plane needs a listener at all, when B06 abolished one

B06 §7 correctly removed the control-plane listener by passing a pre-connected fd to a child we
spawn. That trick is unavailable here: the Playwright process is **not our child**. The user runs it
independently, possibly minutes later, possibly repeatedly. Something must be connectable.

So the automation plane accepts a listener and compensates with four controls the control plane
never needed: filesystem-scoped rendezvous (§6.1), a capability token (§6.2), peer verification
(§6.3), and — the part that actually matters — method-level policy (§7).

An alternative that preserves the no-listener property is **fd-passing via a one-shot handoff**: the
user runs `terminal-fenster automation exec -- npx playwright test`, and Terminal-Fenster spawns the client as
its own child with the socket fd pre-connected on fd 3. This is strictly stronger and should be
offered, but it cannot be the only mode because it forbids attaching to an already-running agent.
**Recommend shipping both; make `exec` the documented default in docs and examples.**

### 5.2 Message flow

1. Client opens the Unix socket and sends an HTTP `GET … Upgrade: websocket` with
   `Authorization: Bearer <token>`.
2. Broker verifies token (constant-time), peer credentials, and grant liveness. On failure:
   `401`, socket closed, audit event. **No detail is returned** — a probing process learns only
   "no".
3. On success: RFC 6455 `101`. The broker deliberately **omits** `Sec-WebSocket-Extensions`, so
   `permessage-deflate` is declined and every frame is inspectable without a decompression step.
   (Verified: Playwright offers it and proceeds happily without it.)
4. Each client text frame is parsed as JSON, run through the policy engine (§7), then written to fd 3
   as `<json>\0`.
5. Each `\0`-delimited message from fd 4 is run through the **egress** filter (§7.4) and framed back
   to the client.

### 5.3 Framing notes for the implementer

- Message sizes are unbounded in practice (`Page.captureScreenshot` returns base64 megabytes).
  Chromium's own receive buffer is `100 * 1024 * 1024` (`devtools_pipe_handler.cc:44`). The broker
  must handle 64-bit WebSocket lengths and continuation frames; the prototype does.
- Client→server frames are always masked; server→client must be unmasked. Standard, but the #1
  source of hand-rolled-WS bugs.
- The RFC 6455 accept GUID is **`258EAFA5-E914-47DA-95CA-C5AB0DC85B11`**. I initially wrote a
  transposed variant from memory and Playwright rejected the handshake with
  `Invalid Sec-WebSocket-Accept header`; the value above is the one in Playwright's own bundled `ws`
  (`node_modules/playwright-core/lib/utilsBundle.js`). Copy it, do not retype it.
- Prefer an audited WS server implementation over the hand-rolled one in the prototype. In Rust,
  `tokio-tungstenite` (MIT/Apache-2.0 — compatible) handles masking, fragmentation and control
  frames correctly. **Check `LICENSE` before vendoring anything.**

---

## 6. The capability-token model

### 6.1 Rendezvous

- Socket at `$TMPDIR/terminal-fenster-<uid>/automation-<session>.sock`, directory `0700`, socket `0600`,
  both owned by the invoking uid. Verified in the prototype: `stat -f "%Sp %u"` →
  `drwx------ 501` and `srw------- 501`.
- The path is **not** a secret and may appear in `ps`; all authority lives in the token. This matches
  A09 §4.4's "never in argv" rule — the *path* is fine there, the *token* never is.
- Created on grant, unlinked on revoke and on process exit. Stale-socket cleanup must `connect()`
  first and only unlink on `ECONNREFUSED`, never blind-unlink (that is a hijack primitive).

### 6.2 The token

Following A09 §4.4, with which this design is fully aligned:

| Property | Specification |
|---|---|
| Entropy | 256 bits from the OS CSPRNG (`SecRandomCopyBytes` / `getrandom(2)`). Never a userspace PRNG. |
| Encoding | base64url, no padding. |
| Comparison | Constant-time (`subtle::ConstantTimeEq`; the prototype uses `crypto.timingSafeEqual`). Length-check first, and note that a length mismatch is itself a (harmless) oracle — compare against a fixed-length canonical form. |
| Lifetime | ≤ 15 min sliding TTL; hard cap 8 h. Revoked on engine restart, profile switch, and explicit `terminal-fenster automation revoke`. |
| Scope | Carries an explicit capability set (§7.2) and a target scope (§7.3). There is **no** "all" token. |
| Delivery | Printed once to the user's TTY by `terminal-fenster automation grant`, or written to a `0600` file the user names. **Never** in argv, never in a child's environment, never logged, never in shell history. |
| At rest | Not persisted by default. If the user opts into persistence, `safeStorage.encryptString` (Keychain-backed), file mode `0600` — with A09 §5.1's caveat that this does not stop malware that can drive the Keychain prompt. |

**Rotation:** one token, one connection. On successful upgrade the token is marked used and a
connection-scoped session key takes over; a replayed token is refused. This limits the damage of a
token that leaks through a screenshot, a log, or shoulder-surfing.

### 6.3 Peer verification

V14 makes this cheap on macOS, and it is genuinely useful:

- `LOCAL_PEERCRED` → `uid`. Reject any uid ≠ ours. Cheap, and it stops cross-account access on
  shared machines.
- `LOCAL_PEERTOKEN` → full `audit_token_t` **including `pidversion`**. Feed it to
  `SecCodeCreateWithAuditToken` + `SecCodeCheckValidity` to bind the grant to a *code identity*
  (e.g. "the `node` at this path, signed by this Team ID"). `pidversion` closes the PID-reuse race
  that makes `LOCAL_PEERPID`-based checks unsound.
- **Honest limit:** same-uid is not a security boundary against malware already running as the user,
  and code-identity pinning to `node` is weak because `node` will run any script. Peer verification
  is a *speed bump and an audit aid*, not the load-bearing control. §7 is the load-bearing control.
  Do not let it be described in docs as more than it is.

Linux equivalents (`SO_PEERCRED`, `SO_PEERSEC`) are **UNVERIFIED** — no Linux host was available.

### 6.4 Consent

CDP is **off by default**. Enabling it requires an explicit human act, and the first connection under
a grant raises an in-TUI prompt naming the peer:

```
  ⚠  Automation client wants to control this browser
     process : node  (pid 57395, /usr/local/bin/node)
     profile : default          targets: 1 tab (example.com)
     grants  : dom:read dom:write input:inject screenshot   [cookies:read NOT granted]
     [g] grant 15 min    [o] grant once    [d] deny    [r] deny + revoke token
```

Rationale: the token proves *possession*, not *intent*. A token exfiltrated from a CI log is a valid
token. The prompt is what converts possession into authorisation, and it is the only control that
survives token compromise. It must be suppressible only by an explicit, persisted, per-profile
setting — never by a flag an attacker can pass.

---

## 7. Method policy — why this is not just a port with a password

This is the section that answers the mission's critical question. Authentication alone would be
insufficient: it would mean any process that obtains the token gets *everything* CDP can do. The
broker therefore parses every message and enforces policy. **The broker is not a tunnel.**

### 7.1 Default-deny allowlist

The prototype used a deny-list to make the demo legible. **Ship an allowlist.** CDP has well over a
thousand methods across ~50 domains and grows every Chromium release; a deny-list silently fails open
on every new release. V13 is the proof: a deny-list naming `Network.getAllCookies` protects nothing
today, because the method was renamed to `Storage.getCookies` and the old name no longer exists.

Unknown method → deny, log, and return a CDP-shaped error so the client fails cleanly.

### 7.2 Capabilities

Grants are composed from A09 §4.4's scopes:

| Capability | Grants (illustrative) | Default | Notes |
|---|---|---|---|
| `dom:read` | `DOM.*` getters, `Runtime.evaluate`, `Runtime.callFunctionOn`, `Page.getFrameTree`, `Page.createIsolatedWorld`, `Accessibility.*` | on | Cannot be withheld from Playwright — its selector engine is built on `Runtime.callFunctionOn` (9 of 32 measured calls). |
| `dom:write` | `Input.*`, `Page.navigate`, `Page.reload`, `Runtime.evaluate` with side effects | on | |
| `screenshot` | `Page.captureScreenshot`, `Page.captureSnapshot` | on | Reads any pixel of any authenticated page. Rate-limit and audit. |
| `network:observe` | `Network.enable`, response metadata | on | |
| `network:intercept` | `Fetch.*`, `Network.setExtraHTTPHeaders`, `Network.setRequestInterception` | **off** | Can inject/steal `Authorization` headers on live sessions. |
| `cookies:read` | `Storage.getCookies`, `Network.getCookies` | **off** | Crown jewel. Per-invocation confirm. |
| `cookies:write` | `Storage.setCookies`, `Network.setCookie` | **off** | Session fixation. |
| `download` | `Browser.setDownloadBehavior`, `Page.setDownloadBehavior`, `IO.read` | **virtualized** | See §7.5 — cannot be denied outright. |
| `targets:create` | `Target.createTarget`, `Target.createBrowserContext` | on, filtered | URL scheme filter (§7.6). |
| `profile:admin` | `Browser.setPermission`, `Browser.grantPermissions`, `SystemInfo.*` | **off** | |

### 7.3 Target discovery and scoping

The broker owns the target namespace. It maintains a map of real `targetId` ⇄ grant-scoped id and:

- Filters `Target.getTargets` responses to targets inside the grant.
- Filters `Target.targetCreated` / `targetInfoChanged` / `targetDestroyed` **events** on egress —
  otherwise the client learns about tabs it was never granted, including their URLs and titles.
- Rejects `Target.attachToTarget` / `attachToBrowserTarget` for out-of-scope targets.
- Tracks `sessionId` on every message so that a session established for target A cannot later be used
  to address target B. **`sessionId` is the real authorisation subject in flattened CDP; policy that
  ignores it is bypassable.**

Scope granularity: whole-profile, single-tab, or "tabs this grant created". Default for an agent
grant should be **"tabs this grant created"** — the agent gets a fresh tab and cannot see the user's
logged-in banking tab at all. This is the single highest-value scoping decision in the design and it
is what makes agent automation defensible on a browser that also holds the user's real sessions.

`Target.exposeDevToolsProtocol` must be **permanently denied for every grant**. It injects a raw CDP
binding into a page's JavaScript context, which bypasses the broker entirely — it is a complete
escape from every control in this document.

### 7.4 Egress filtering

Responses and events need filtering too, not just requests:

- Strip `Set-Cookie` / `Cookie` from `Network.responseReceivedExtraInfo` and
  `requestWillBeSentExtraInfo` unless `cookies:read` is granted. Without this, the cookie jar leaks
  through the network domain even though `Storage.getCookies` is blocked — a silent bypass of §7.2.
- Filter target lifecycle events by scope (§7.3).
- Redact `Runtime.consoleAPICalled` payloads from out-of-scope targets.
- Cap total egress bytes per grant and per minute; log overruns.

### 7.5 Virtualize, don't deny (V12)

V12 is the design's most useful surprise: **Playwright calls `Browser.setDownloadBehavior` during
`connectOverCDP` itself.** Denying it fails the connection outright, so a naive "block anything that
touches the filesystem" policy makes the whole feature unusable.

The fix is to rewrite rather than refuse — the broker replaces `downloadPath` with a broker-owned
jail directory (`0700`) and forwards the call. The client believes it succeeded; the browser can only
write inside the jail. Verified working: after this change the same session connected, automated, and
still had `Storage.getCookies` blocked.

Generalise this: for every method that Playwright *requires* but that carries authority, prefer a
virtualizing rewrite. Other candidates (**UNVERIFIED**, to be measured as the surface grows):
`Page.setDownloadBehavior`, `Browser.setPermission`, `Emulation.setGeolocationOverride`,
`Page.setInterceptFileChooserDialog`, `DOM.setFileInputFiles` (must be jailed to files the user
explicitly chose — this is a direct filesystem-read primitive).

### 7.6 Argument-level checks

Method names alone are not sufficient; several safe-looking methods are dangerous by argument:

- `Page.navigate`, `Target.createTarget`: deny `file://`, `chrome://`, `devtools://`, `blob:` with a
  foreign origin. `file://` navigation plus `dom:read` is arbitrary local file read.
- `DOM.setFileInputFiles`: jail to user-selected paths.
- `Runtime.addBinding`, `Page.addScriptToEvaluateOnNewDocument`: allowed (Playwright uses the latter,
  measured), but the injected source must be recorded in the audit log.
- `Runtime.evaluate` with `awaitPromise` + `userGesture`: permitted, but `userGesture: true` should be
  gated on `input:inject` since it unlocks gesture-restricted APIs.

### 7.7 Audit

Every decision — `auth_ok`, `auth_reject`, `method_allowed`, `method_denied`, `method_rewritten` —
appends to the session audit log with grant id, `sessionId`, method, and a truncated argument digest.
The prototype emits exactly these events. Audit output must never contain the token.

---

## 8. The security argument, stated honestly

An open CDP port fails on three independent axes at once: **reachability** (any local process),
**authority** (no authentication), **scope** (every method, every target). The broker breaks all
three, and the argument is only as strong as its weakest honest claim, so:

**What this design guarantees**

1. **No network surface.** Verified V6: zero listening sockets on the browser tree. The automation
   endpoint is a filesystem object with `0600`/`0700` permissions. A remote attacker has no path in,
   and a malicious *web page* cannot `fetch()` a Unix socket — DNS rebinding (A09 T14) is
   structurally impossible, not merely mitigated.
2. **No ambient authority.** Unauthenticated connects are refused (V9), and possession of the token
   still requires passing the consent prompt (§6.4).
3. **No unrestricted method set.** Even a fully authorised client cannot dump the cookie jar (V11),
   cannot escape to raw CDP via `Target.exposeDevToolsProtocol`, cannot write outside the download
   jail (§7.5), and cannot see targets outside its grant (§7.3).
4. **Default off.** The endpoint does not exist until a human grants it, and evaporates on revoke,
   engine restart, or TTL expiry.
5. **Auditable.** Every method is logged; anomalies are detectable after the fact.

**What this design does *not* guarantee — and must not be marketed as guaranteeing**

- **An authorised agent can read everything its granted pages can read.** `Runtime.evaluate` and
  `Runtime.callFunctionOn` cannot be withheld from Playwright (§7.2), and they are Turing-complete
  within the page. If you grant a tab, you grant that tab's rendered content and its non-`HttpOnly`
  cookies. The defensible boundary is *which tabs*, which is why §7.3's "tabs this grant created"
  default matters more than any method filter.
- **Same-uid malware is not stopped by peer credentials** (§6.3). It is stopped, if at all, by the
  consent prompt and by the fact that there is nothing listening until a human says so.
- **A stolen live token plus a suppressed prompt is a real compromise**, bounded by the grant's
  capabilities and target scope. This is why grants are narrow, short, and one-shot.
- **The broker is new attack surface.** It parses attacker-influenced WebSocket frames and JSON in a
  privileged process. It must be fuzzed (§10, T-BRK-3) and should reuse an audited WS implementation
  rather than the hand-rolled prototype parser.

The honest one-line summary: **we cannot make automation safe in the abstract, but we can make the
blast radius equal to the tabs the user deliberately handed over, instead of the entire browser and
every cookie in it.**

---

## 9. Client compatibility

| Client | Status | Notes |
|---|---|---|
| **Playwright** (`connectOverCDP`) | **VERIFIED** | `ws+unix://<sock>:/devtools/browser/<id>` + `headers.Authorization`. |
| Playwright via `exec` handoff | Designed, **UNVERIFIED** | Pre-connected fd; strongest mode (§5.1). |
| **Puppeteer** (`connect({ browserWSEndpoint })`) | **UNVERIFIED** | Also uses the `ws` package, so `ws+unix://` plausibly works; `headers` support differs. Must be tested before it is documented. |
| `chrome-remote-interface`, raw DevTools frontend | Will not work | They require the `/json/*` HTTP endpoints, which we deliberately do not serve (V8). |
| Anything needing `http://` discovery | Opt-in shim only | See below. |

We serve **no `/json/version`, `/json/list`, or `/json/new`**. V8 shows Playwright skips discovery
entirely for a `ws://`-scheme URL, so these endpoints buy nothing and each is an unauthenticated
information leak in every other CDP implementation. `terminal-fenster automation url` prints the endpoint
instead.

For clients that genuinely cannot do Unix sockets, offer an explicitly-flagged
`--automation-tcp` mode: ephemeral port on `127.0.0.1`, same token, same policy engine, plus strict
`Host`/`Origin` validation to blunt DNS rebinding. It must print a visible warning and must never be
the default. Note also that Playwright **strips `Authorization` on redirect**
(`transport.ts:120-124`) — the broker must never redirect.

---

## 10. Test plan (all CI-able, no screenshots needed)

Consistent with the project's protocol-response-and-log evidence approach.

- **T-CDP-1 (extends A09).** No listening socket anywhere in the engine tree with the pipe in use.
  `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` = 0 rows for every pid in the tree; `nc -z 127.0.0.1
  {9222,9229,5858,8315}` all fail. **Verified passing in this mission.**
- **T-CDP-2.** Argv guard: `--remote-debugging-port=9222` → exit 70. `--remote-debugging-pipe`
  without parent-supplied fds 3/4 → exit 70. With them → starts.
- **T-CDP-3 (regression for V1).** Assert Electron still has no default-profile protection, so the
  guard is never quietly relied upon: launch with the port flag and a default userData dir, expect a
  live endpoint, fail the build if this ever silently changes.
- **T-BRK-1.** Unauthenticated connect → `401`, no CDP bytes exchanged, audit shows `auth_reject`.
  Wrong-length and wrong-value tokens both rejected. **Verified.**
- **T-BRK-2.** Full Playwright round trip through the broker: connect, read title, fill an input,
  assert value. **Verified** (`client.mjs`).
- **T-BRK-3.** Policy: `Storage.getCookies` denied without `cookies:read` (**verified**);
  `Network.getAllCookies` denied *or absent*; `Target.exposeDevToolsProtocol` denied; `Page.navigate`
  to `file:///etc/passwd` denied; `Browser.setDownloadBehavior` rewritten into the jail and the jail
  path asserted on disk. Plus a fuzz corpus of malformed WS frames and oversized/truncated CDP JSON
  against the broker parser.
- **T-BRK-4.** Egress: with `cookies:read` withheld, no `Set-Cookie` appears in any
  `*ExtraInfo` event; out-of-scope `Target.targetCreated` events never reach the client.
- **T-BRK-5.** Lifecycle: token expiry closes live connections; revoke unlinks the socket; engine
  restart invalidates all grants; TTL is enforced server-side, not client-side.
- **T-BRK-6.** `ps auxww | grep -c terminal-fenster.*token` = 0 — the token never appears in argv.

---

## 11. Changes required in files I do not own

Per the ownership rule these are **described, not made**.

**`apps/engine/src/main.js`**
1. Add the A09 §2.3 argv guard, amended per §4: `--remote-debugging-port`, `--inspect*`,
   `--no-sandbox`, `--disable-web-security`, `--single-process` fatal (exit 70);
   `--remote-debugging-pipe` permitted **only** when fds 3/4 are parent-supplied pipes.
2. Add `app.enableSandbox()`.
3. No CDP logic belongs here. The engine's only role is to be launched with the flag; the pipe is
   consumed by the parent. This keeps the engine free of policy code.
4. The current file already gets the important things right — `sandbox: true`, `contextIsolation:
   true`, `nodeIntegration: false`, and `setWindowOpenHandler` denying popups
   (`main.js:96-134`) — none of that should change.

**`apps/cli/` (Rust core)**
5. Spawn the engine with fds 3/4 wired to pipes the core owns, and add `--remote-debugging-pipe`
   only when automation is enabled for the session.
6. New module `bg-cdp-broker`: NUL-delimited CDP framing on the pipe, WS server on a Unix socket,
   token mint/verify, `LOCAL_PEERCRED`/`LOCAL_PEERTOKEN` checks, policy engine, audit log.
7. New subcommands: `terminal-fenster automation grant [--caps …] [--scope …] [--ttl …]`,
   `automation url`, `automation revoke`, `automation status`,
   `automation exec -- <cmd>` (the fd-handoff mode of §5.1).
8. TUI consent prompt (§6.4).

**`crates/tf-proto`**
9. Control-plane messages for grant/revoke/status and the consent round trip.

**Dependency note:** if `tokio-tungstenite` is adopted for the WS layer, confirm its MIT/Apache-2.0
licensing against the repo's `LICENSE` policy before vendoring (rule 4). The repo currently has no
top-level `LICENSE` file — worth resolving before any third-party code lands.

---

## 12. Open questions

| # | Question | Why it matters | How to close |
|---|---|---|---|
| U1 | Does Puppeteer accept `ws+unix://` with auth headers? | Determines whether we can advertise Puppeteer support. | Install `puppeteer-core`, repeat T-BRK-2. |
| U2 | Do `SO_PEERCRED` / `SO_PEERSEC` give equivalent identity on Linux? | SSH/remote story (A07, C09). | Test on a Linux host; none available here. |
| U3 | Full method set for screenshots, navigation, file upload, network interception. | §2.1 measured only 20 methods from a minimal session; the allowlist is incomplete. | Run Playwright's own test suite through the broker with the policy engine in log-only mode and harvest the tally. |
| U4 | Does the broker add measurable latency at 60 fps under concurrent automation? | A10 performance budget. | Bench with `Page.captureScreenshot` in a loop while OSR runs. |
| U5 | Behaviour when the engine crashes with a live grant. | B08 crash recovery; grants must not survive into a new engine. | Kill the engine mid-session; assert socket unlinked and token invalidated. |
| U6 | Does `--remote-debugging-pipe` interact badly with `webContents.debugger` on the same target? | We plan to use both (§3.2). | Attach both, assert neither detaches the other. |

---

## 13. References

Primary sources, all fetched during this mission:

- Chromium — `chrome/browser/devtools/remote_debugging_server.cc` (fn `IsRemoteDebuggingAllowed`,
  line 163; branding gate, lines 52 & 171-175).
- Chromium — `content/browser/devtools/devtools_pipe_handler.cc` (`PipeWriterASCIIZ` line 292,
  `PipeWriterCBOR` line 302, 100 MB receive buffer line 44).
- Playwright — `packages/playwright-core/src/server/chromium/chromium.ts`
  (`connectOverCDP` lines 84-115; `urlToWSEndpoint` lines 458-474, `ws`-scheme short-circuit
  lines 459-460).
- Playwright — `packages/playwright-core/src/server/transport.ts`
  (`new ws(url, [], { headers … })` lines 136-139; redirect strips `Authorization` lines 120-124).
- Playwright docs — `browserType.connectOverCDP` (`headers`, `endpointURL` accepts http **or** ws).
- Chrome for Developers — "Changes to remote debugging switches" (Chrome 136).
- RFC 6455 (WebSocket handshake, framing, masking).
- Terminal-Fenster — A09 §2.3, §4.4, §4.5, §5; B06 §2.4, §7; B04 §3.

Prototype artifacts (scratchpad, not part of the repo): `e01/broker.js`, `e01/client.mjs`,
`e01/pipetest.js`, `e01/takeover.mjs`, `e01/peercred.py`, `e01/methods.json`.
