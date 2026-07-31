# E02 — Machine-readable automation CLI specification

**Status:** specification + measured evidence. No repo source was modified.
**Owner of this file:** E02. I wrote only `artifacts/swarm/E02-cli-automation-spec.md`.
**Date:** 2026-07-31
**Host:** macOS 26.1, Apple M4 arm64. Electron 43.2.0 / Chromium 150.0.7871.129, CDP protocol `1.3`, Node v24.11.1.

**Pinned revisions.** Core source is under concurrent edit by other agents, so every `file:line` below is
anchored to these exact bytes:

| File | lines | md5 |
|---|---:|---|
| `apps/cli/src/main.rs` | 1039 | `bceb2a511097d93ea17ac90b94fcb077` |
| `crates/bg-proto/src/lib.rs` | 284 | `edbb4c0b2e74e960857f2fc687210fe4` |
| `apps/engine/src/main.js` | 309 | `1520d7ab86e4c69e76508bd6d6bab2ce` |

---

## 1. Recommendation in one paragraph

Build the automation plane on **CDP `Runtime` / `DOM` / `Accessibility` / `Page.lifecycleEvent`**, reached
through `webContents.debugger`, and **not** on Electron's convenience wrappers. Three measurements force
this: `executeJavaScript` destroys error text (every thrown error becomes one generic English sentence,
§5.2) while `Runtime.evaluate` returns `exceptionDetails` with the real message; `capturePage()` returns a
**1800×1200** image for a 900×600 window on this Retina host while the OSR frame the terminal actually
renders is **900×600** (§5.5), so a `capturePage`-based `screenshot` would silently disagree with what the
user sees; and `did-finish-load` fired **855 ms before** the page reached `networkIdle` on a trivial local
fixture (§5.3), so any readiness signal built on load events is wrong by default. Element identity should
be an **opaque, epoch-scoped `ref`** minted by `snapshot` and backed by a `WeakRef` registry injected into
the page — measured to distinguish *live* / *detached* / *collected* / *no-such* and to be destroyed
automatically by cross-document navigation (§5.4), which is exactly the invalidation A03 c-F2 demands.
Every verb returns one JSON envelope on stdout with an explicit `error.code` mapped to a stable exit code,
and every wait is evaluated against a **document generation (`loaderId`)** with an absolute wall-clock
deadline. The single most important design rule is §7.R5: element *stability* must be sampled across a
wall-clock window, never across N adjacent animation frames — measured, two consecutive `requestAnimationFrame`
samples of a still-moving element returned **identical** coordinates and would have declared it settled.

---

## 2. Method, and what "verified" means here

Electron cannot spawn Chromium children under the agent Bash sandbox
(`bootstrap_look_up … Permission denied`), and the machine is at a lock screen, so nothing below rests on a
screenshot. Every claim is either read directly off the pinned source, or produced by running the Electron
binary already vendored in this repo with the sandbox disabled:

```
cd /Users/adeebbashir/projects/blackglass/apps/engine
./node_modules/.bin/electron <probe>.js          # sandbox disabled for the Bash call
./node_modules/electron/dist/version  ->  43.2.0
```

Probes live outside the repo, in
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/e02/`:

| Probe | Question | Log |
|---|---|---|
| `probe1.js` | first attempt; died on a CDP hang | `probe1.log` |
| `probe2.js` | `executeJavaScript` value marshalling, error fidelity, wedged renderer | `probe2.raw` |
| `probe3.js` | CDP domain availability, AX tree, handle registry, capture, quiescence | `probe3.raw` |
| `probe4.js` | lifecycle events, `Runtime.evaluate` errors, actionability, click/scroll | `probe4.raw` |
| `probe5.js` | coordinate spaces, lifecycle scoping, typing | `probe5.raw` |
| `probe6.js` | corrected occlusion + moving-element tests, screenshot geometry | `probe6.raw` |
| `probe7.js` | does full-page capture perturb the live OSR stream? | `probe7.raw` |

Fixtures: `fixture.html` (late DOM mutation at 700 ms, element *replacement* at 1200 ms),
`fixture2.html` (fully static), `fixture3.html` (animation + overlay + deferred fetch),
`occl.html`, `mover.html`.

### 2.1 Two methodology corrections worth recording

I got two probes wrong in ways that would have produced confident, false claims, so both are recorded here
rather than quietly fixed.

**`probe4` mistimed its own fixtures.** It sampled "is the target occluded?" and "is the element moving?"
*after* `sleep(3500)`, by which point the overlay had been removed (t=1500 ms) and the animation had
finished (t≈900 ms). Both samples came back clean and I nearly wrote down "occlusion is detected" and
"stability check works" on the strength of tests that had exercised neither. `probe6` re-ran both with
correct timing and produced the *opposite* result for stability (§5.6).

**`probe5` used `data:text/html,…` URLs containing CSS id selectors.** The `#` in `#ov{…}` is a URL
fragment delimiter, so Chromium truncated every fixture at the first `#` and loaded an empty document.
`Runtime.evaluate` then returned an object with no `objectId`, my `if (oid)` guard skipped the whole
section, and the probe printed *nothing at all* for it — a silent skip that looks identical to "not run".
`probe6` re-ran the same tests from real `file://` fixtures. Guard clauses in probes should log their own
skips; mine did not.

---

## 3. Where the automation plane sits

### 3.1 The constraint that decides the process model

`apps/cli/src/main.rs:30-31` states the rule that shapes everything:

> Logging must never go to stdout while browsing: stdout is the graphics channel, and a stray log line
> would corrupt an image mid-transmission.

In interactive mode stdout carries kitty graphics escape sequences (`main.rs:899-901`). Automation output
is JSON on stdout. **These cannot be the same process.** Therefore:

```text
  [blackglass serve]            long-lived session host: owns the tty (optionally),
        |                       owns the engine child, owns the control socket
        |  AF_UNIX control socket (0600 in a 0700 dir, per main.rs:389-400)
        |
  [blackglass click …]          short-lived verb process: connects, sends one command,
  [blackglass wait  …]          reads one reply, prints one JSON envelope, exits
  [blackglass snapshot …]
```

`serve` is new infrastructure this spec depends on; it is not one of the fourteen verbs. It reuses the
socket-creation code that already exists at `main.rs:389-400` (private `0700` directory, `0600` socket)
and inherits B06's hardening requirements, including **F4** — the current listener accepts the first
connection with no authentication, which for an automation socket is strictly worse than for the
interactive case because the socket path is now long-lived and predictable.

### 3.2 Compatibility with the existing CLI

`main.rs:54-70` dispatches `doctor`, `open`, `version`, `help`. Only `open` collides with this spec.

**Rule:** `blackglass open <url>` with no `--json` and no session in scope keeps today's behaviour exactly
— interactive TUI, `main.rs:218`. `open` becomes the automation verb only when `--json` is passed or a
session is in scope (`--session PATH` or `$BLACKGLASS_SESSION`). Automation verbs other than `open`
require a session and fail with `E_NO_SESSION` (exit 7) without one; they never start an engine implicitly,
because an implicit engine start is a 200–400 ms cost with no owner and no teardown path.

### 3.3 Relationship to sibling designs

| Artifact | What this spec consumes | What this spec must not contradict |
|---|---|---|
| **B04** tab lifecycle | `tab.new` / `tab.close` / `tab.activate` / `tab.list` commands; core-allocated ids; `MAX_TABS = 16`; one tab paints at a time | `open` / `close` / `focus` / `list` are thin wrappers over B04's commands. This spec adds no tab-management protocol of its own. |
| **B06** IPC hardening | message-size caps, version negotiation, socket authentication (F4) | Automation adds `eval` and `snapshot`, whose payloads are page-controlled and unbounded — they need B06's caps *more* than frames do (§9.4). |
| **A03** journey (c) | control arbitration `AGENT`/`HUMAN`/`SHARED`, `E_HUMAN_CONTROL`, action log, fenced page text | This spec supplies the verb surface A03 sketched as `bg serve --rpc`; error codes are aligned (§8). |
| **B08** crash recovery | `render-process-gone`, unresponsive detection | The wedge in §5.2 is B08's L2 case; `eval` cannot recover from it and must not pretend to. |
| **C07** scaling/colour | device scale factor handling | §5.5's 2× discrepancy is a scaling issue; C07 owns the rendering side, this spec owns only what `screenshot` reports. |
| **A09** threat model | terminal-escape injection, page-controlled strings | `snapshot` and `eval` return page-controlled text to a machine consumer; §9 covers fencing. |

---

## 4. Design principles

**P1 — One envelope per invocation.** A verb prints exactly one JSON object to stdout and exits. No
progress chatter, no partial lines. `--stream` is the only exception and switches the whole stdout to JSONL.

**P2 — Observation is cheap and semantic; pixels are opt-in.** `snapshot` returns an accessibility-derived
node list (measured 675 bytes scoped vs 9 327 bytes full for a trivial page, §5.4) — A03 c-F4 exists
precisely because agents that screenshot everything burn their context on nothing.

**P3 — Every mutating verb names what it acted on.** `click` returns the resolved `ref`, the computed
point, and the actionability record that permitted it. A log of results is a replayable trace.

**P4 — Failure is typed, never silent.** A stale element ref returns `E_STALE_REF` and exit 4. It never
degrades to a no-op that an agent will loop on (A03 c-F2).

**P5 — Determinism over convenience.** No verb auto-waits for something it was not asked to wait for,
except the actionability gate on pointer verbs (§7.6), which is explicit and reported.

**P6 — Absolute deadlines.** Timeouts are wall-clock from command receipt at the session host, not
per-poll and not per-retry. A command with `--timeout 5000` cannot take 9 s.

---

## 5. Measured foundation

Everything in §6–§8 rests on these numbers. This section is the evidence, not the design.

### 5.1 CDP is available, but page-scoped domains hang before the first document commits

`probe1` attached the debugger at window creation, before any `loadURL`, then awaited `Network.enable`.
It never resolved and the probe hit its 60 s watchdog. `probe4` isolated the cause:

| Call | Before any document | After first `did-finish-load` |
|---|---|---|
| `Browser.getVersion` (browser-scoped) | **ok**, returns `Chrome/150.0.7871.129`, `protocolVersion 1.3` | ok |
| `Network.enable` (page-scoped) | **`TIMEOUT_3000ms`** | **ok, 1 ms** |

`probe3`, attaching after a real document existed, resolved every domain immediately:

```
cdp.getVersion      ok   0 ms
cdp.runtimeEnable   ok   4 ms
cdp.domGetDocument  ok   3 ms
cdp.pageEnable      ok   1 ms
cdp.networkEnable   ok   1 ms
cdp.axEnable        ok   3 ms
```

**Consequence for the engine:** attach the debugger and enable page-scoped domains only after the first
document commits, and treat a domain-enable that does not resolve within a bounded time as a session
start-up failure rather than waiting forever. This is an engine-side change; per the file-ownership rule I
describe it rather than making it.

### 5.2 `executeJavaScript` marshalling — measured, and mostly a trap

`probe2`, against a loaded fixture. Round-trip p50 was 1–2 ms.

| Expression | Result | Verdict for an `eval` verb |
|---|---|---|
| `1+1`, `true`, `null`, `[1,"a",true]`, `({a:1,b:[2,3]})` | correct | fine |
| `undefined` | `typeof "undefined"` | must serialise distinctly from `null` |
| `document.title` | `"E02 fixture"` | fine |
| `document.getElementById("h")` | **`{}`** | **silent data loss** — a DOM node becomes an empty object |
| `new Map([["a",1]])` | **`{}`** | silent data loss |
| `(function(){return 1})` | throws `An object could not be cloned.` | acceptable, but the message is opaque |
| `new Date(0)` | cloned, JSON-encodes as ISO string | fine |
| `BigInt(9)` | clones as `bigint`, then **`JSON.stringify` throws** | must be caught before encoding |
| cyclic object | clones fine, then **`JSON.stringify` throws** | must be caught before encoding |
| `Promise.resolve(42)` | auto-awaited → `42` | fine, and worth documenting |
| `(()=>{throw new TypeError("precise-message")})()` | rejects with `Script failed to execute, this normally means an error was thrown. Check the renderer console for the error.` | **the actual error text is destroyed** |
| `(()=>{while(true){}})()` | never resolves | see below |

The marshalling is **structured clone**, not JSON — which is why cyclic objects survive the boundary and
then explode at the JSON encoder instead. Two independent failure surfaces.

`Runtime.evaluate` fixes the error problem outright (`probe4`):

```
C.executeJavaScriptError : "Script failed to execute, this normally means an error was thrown…"
C.runtimeEvaluateError   : {hasExceptionDetails:true, text:"TypeError: precise-message", line:0}
C.runtimeEvaluateValue   : {type:"object", value:{a:1,b:"x"}}
C.runtimeEvaluateNodeObjectId : {type:"object", subtype:"node", hasObjectId:true, className:"HTMLButtonElement"}
C.runtimeEvaluateCyclic  : error "Object reference chain is too long"   ← contained, not a hang
```

**The wedge.** After the infinite loop, `wc.isCrashed()` was **`false`** and every subsequent
`executeJavaScript` also timed out — the renderer is alive, unresponsive, and indistinguishable from a busy
page over this API. This is B08's L2 case. `eval` must therefore have a hard timeout, and a timed-out
`eval` must be reported as `E_RENDERER_UNRESPONSIVE` (a session-level condition), not as a per-command
hiccup, because the *next* command will fail too.

### 5.3 Readiness signals — the load event is not one

`probe4`, `fixture3.html` over `file://`, all offsets relative to the navigation start:

| Signal | Offset |
|---|---:|
| `did-finish-load` (Electron) | **31 ms** |
| lifecycle `DOMContentLoaded` | 18 ms |
| lifecycle `load` | 19 ms |
| `firstPaint` / `firstContentfulPaint` | 186 ms |
| `networkAlmostIdle` | 559 ms |
| `networkIdle` | **886 ms** |
| last paint of the settling page | **1 499 ms** |

`did-finish-load` fired **855 ms before** `networkIdle` and **1 468 ms before** the page stopped changing.

The raw in-flight request count is also a trap. Measured `Network` events:

```
req rel=1   inflight=1
fin rel=15  inflight=0      ← "network is idle!"  (wrong: a fetch starts at 321 ms)
req rel=321 inflight=1
fin rel=374 inflight=0
```

A naive `inflight == 0` predicate fires at 15 ms. Chromium's own `networkIdle` arrived at 886 ms, i.e.
**512 ms after** the last `loadingFinished` at 374 ms — consistent with the documented 500 ms zero-request
quiet window. `probe5` reproduced the same shape on a different fixture (last events → `networkAlmostIdle`
1157 ms, `networkIdle` 1159 ms).

**Use Chromium's `Page.lifecycleEvent` `networkIdle`. Do not reimplement it from request counters.**

### 5.4 Element identity

**AX tree** (`probe3`, `fixture.html`, 900×600):

| Measurement | Value |
|---|---|
| Full tree | 23 nodes, 8 ms, **9 327 bytes** |
| `Accessibility.queryAXTree{role:"button",accessibleName:"Sign in"}` | 1 match, **675 bytes** |
| AXNodeIds stable across two consecutive `getFullAXTree` calls | **true** |
| AX `backendDOMNodeId` for the button | `9` |
| DOM `describeNode.backendNodeId` for `#b1` | `9` — **same join key** |
| `DOM.getBoxModel` for `#b1` | content quad `[36, 90.875, 76.77, 90.875, 76.77, 105.875, 36, 105.875]`, w 73, h 35 |

Scoped queries are **13.8× smaller** than the full tree on a page with 23 nodes; the ratio only grows.
This is A03 c-F7's argument, now with a number attached.

**Injected `WeakRef` registry** (`probe3`). A registry of `Map<id, WeakRef<Element>>` injected into the
page's main world, with a resolver returning one of four states:

| Scenario | Resolver result |
|---|---|
| handle to `#b1`, still in the document | `"BUTTON#b1"` |
| handle to `#b2` **after the fixture replaced the node** | **`"DETACHED"`** |
| handle to `#b1` after that same mutation | `"BUTTON#b1"` (unaffected) |
| handle `"e999"` never minted | `"NO_SUCH"` |
| registry read after `location.hash = "#frag"` | survives (counter preserved) |
| registry read after cross-document navigation | **`"GONE"`** |

This is precisely the behaviour a stable ref needs: node replacement is detected rather than papered over,
same-document navigation preserves refs, and cross-document navigation destroys them automatically. No
bookkeeping required for the last case — the JS world is torn down for us.

### 5.5 Coordinate spaces — three of them, and they disagree

`probe5` and `probe6`, window content size 900×600, primary display `scaleFactor: 2`:

| Quantity | Value |
|---|---|
| `win.getContentSize()` | `[900, 600]` |
| **OSR `paint` image size** (what the terminal renders) | **900×600** |
| Page's own `[innerWidth, innerHeight, devicePixelRatio]` | `[900, 600, 1]` |
| **`webContents.capturePage()`** | **1800×1200** |
| `Page.captureScreenshot{format:"png"}` | **900×600** |
| `Page.captureScreenshot{captureBeyondViewport:true, clip:{…,height:1200}}` | 900×1200 |
| Byte sizes | `capturePage` PNG 11 208 B; CDP default base64 4 456 B; CDP beyond-viewport base64 8 272 B |

`capturePage()` honours the *display's* scale factor (2×) even though the offscreen page runs at `DPR 1`
and the OSR frame is 1:1. `Page.captureScreenshot` matches the OSR frame exactly.

**`screenshot` must use `Page.captureScreenshot`.** `sendInputEvent` coordinates are CSS pixels
(`probe4`: target centre `{x: 46.53, y: 91.375}` in a 900×600 window, click observed), which is the same
space as the OSR frame and the same space `PointerMap` already produces (`main.rs:712-727`). A
`capturePage`-based screenshot would be the only artefact in the system at 2×.

### 5.6 Actionability — verified in both directions, and one heuristic that fails

`probe6`, `occl.html`: a `position:fixed; inset:0; z-index:99` overlay covering the button, removed at
2 500 ms.

| Moment | `elementFromPoint` at the element's centre | Real click dispatched at that point |
|---|---|---|
| While the overlay is up | `hitSelf: false`, `topTag: "DIV#ov"` | handler fired **0** times |
| After the overlay is removed | `hitSelf: true`, `topTag: "BUTTON#t"` | handler fired **1** time |

The hit test predicts the click outcome correctly in both directions. A hidden element
(`display:none`) reported `w:0, h:0, visible:false, hitSelf:false, topTag:"HTML"`.

**The stability heuristic fails.** `mover.html` moves a button 10 px every 50 ms. Sampled every 50 ms, the
left edge walked `[20, 40, 50, 60, 80, 90, 100, 110]` — plainly moving. But two samples taken in
*consecutive `requestAnimationFrame` callbacks* returned:

```
mover.rafPairWhileMoving   : [130, 130]     ← element is still moving
mover.rafPairAfterSettle   : [400, 400]     ← element has genuinely stopped
```

The two readings are **indistinguishable**. A 16.7 ms frame pair cannot see a 50 ms animation tick, so the
widely-copied "same bounding box on two consecutive animation frames ⇒ stable" rule reports **false
stable** for any element animated on a timer slower than the frame interval. §7.R5 is the fix.

### 5.7 Quiescence

| Fixture | Paints after `did-finish-load` | Last paint offset | Largest gap *before* the last paint | Idle tail observed |
|---|---:|---:|---:|---:|
| `fixture.html` (mutations at 700 ms, 1 200 ms) | 3 | 1 199 ms | **692 ms** | 2 801 ms |
| `fixture2.html` (fully static) | 1 | 15 ms | — | 3 985 ms |
| `fixture.html` after full settle (`probe7`) | **0 paints in 1 800 ms** | — | — | — |

A static page reaches exactly zero paints, so paint inactivity is a usable quiescence signal. But a
**500 ms** quiet window would have declared `fixture.html` settled during its 692 ms gap, one mutation
early. Default `--quiet-ms` is therefore **800**, above the measured worst gap, and §7.R5 marks it as a
heuristic bound rather than a guarantee.

### 5.8 Full-page capture perturbs the live frame stream

`probe7` recorded OSR paint geometry across a capture:

```
beforeCapture              paints 0                     (page fully quiesced)
afterPlainCapture          paints 3, sizes ["900x600"]
afterBeyondViewportCapture paints 2, sizes ["900x600","900x3400"]   ← 187 ms
                           timeline: 900x600@7588, 900x3400@7695
afterRecovery              paints 1, sizes ["900x600"], innerSize [900,600]
```

`Page.captureScreenshot{captureBeyondViewport:true, clip.height:3400}` **emits a 900×3400 frame into the
live OSR stream**, which the terminal compositor would receive as a real frame at the wrong geometry. It
recovers afterwards. `screenshot --full-page` must therefore suspend the frame stream for the duration,
and this spec requires it to report a warning when it does (§6.12).

### 5.9 Input dispatch

| Measurement | Result |
|---|---|
| `mouseMove` + `mouseDown` + `mouseUp` at the element centre | handler observed after **6 ms**, first poll |
| Wheel `deltaY: -300` | `scrollY` became **300** (negative delta scrolls down, matching `main.rs:630`) |
| Scripted `scrollTo(0, 1000)` | `scrollY` 1000 |
| `keyDown`/`char`/`keyUp` per character for `"Hi Ab1!"` | input value `"Hi Ab1!"`, `input` event echoed correctly |
| CDP `Input.insertText` with `"héllo wörld"` | value `"héllo wörld"` — non-ASCII intact in one call |

`Input.insertText` is one IPC call for arbitrary text; the per-character path in `main.js:210-219` is one
synchronous `sendInputEvent` per character, which is B06's F9. For the `type` verb this matters: a
1 000-character fill is 3 000 synchronous IPC calls one way and 1 call the other.

---

## 6. The verb catalogue

Common flags on every verb: `--session PATH`, `--tab N`, `--timeout MS`, `--id STR`, `--json` (default on
for all verbs except interactive `open`), `--stream`, `--quiet-ms MS`.

### 6.0 Envelope

Every invocation writes exactly one object:

```json
{
  "v": 1,
  "cmd": "click",
  "id": "01JQ8F3A2C",
  "session": "bg-8f3a1c",
  "tab": 0,
  "ok": true,
  "started_ms": 1754003400123,
  "elapsed_ms": 42,
  "doc": { "epoch": 3, "loader": "8B1F…", "url": "https://example.com/", "title": "Example" },
  "result": { },
  "warnings": []
}
```

Failure replaces `result` with `error` and never emits both:

```json
{
  "v": 1, "cmd": "click", "id": "01JQ8F3A2C", "session": "bg-8f3a1c", "tab": 0,
  "ok": false, "started_ms": 1754003400123, "elapsed_ms": 5000,
  "doc": { "epoch": 3, "loader": "8B1F…", "url": "https://example.com/", "title": "Example" },
  "error": {
    "code": "E_NOT_ACTIONABLE",
    "message": "element is covered by another element at its click point",
    "retryable": true,
    "detail": { "ref": "bg1.0.3.17", "point": {"x": 100, "y": 70},
                "occluded_by": "DIV#ov", "checks": {"attached": true, "visible": true,
                "enabled": true, "hit_self": false, "stable": true} }
  },
  "warnings": []
}
```

| Field | Type | Notes |
|---|---|---|
| `v` | int | Envelope version. `1`. Bumped only on a breaking change; consumers must reject unknown majors. |
| `cmd` | string | The verb, echoed. |
| `id` | string | Caller-supplied `--id`, else a session-generated monotonic id. Correlates with `--stream` events and the action log. |
| `session` | string | Session identifier, stable for the life of `serve`. |
| `tab` | int | Tab the command addressed, `0..15` per B04's `MAX_TABS`. |
| `ok` | bool | Authoritative. Never infer success from the presence of `result`. |
| `started_ms` / `elapsed_ms` | int | Unix ms at command receipt by the session host; wall-clock duration. |
| `doc` | object | Document generation at **completion**. `epoch` is the ref generation (§6.4). |
| `result` \| `error` | object | Exactly one is present. |
| `warnings` | array | Non-fatal. Each `{code, message}`. Never affects `ok` or the exit code. |

`--stream` switches stdout to JSONL: zero or more `{"v":1,"type":"event",…}` lines, then exactly one final
envelope as above with `"type":"result"`. In non-stream mode the `type` field is absent.

### 6.1 `open`

```
blackglass open <url> [--json] [--tab N|--new-tab] [--wait load|dom|paint|network-idle|quiet|none]
                      [--timeout MS] [--referrer URL]
```

Without `--json` and without a session: today's interactive TUI, unchanged (`main.rs:218`).

URL normalisation reuses `normalize_url` (`main.rs:293-304`), including its search fallback. In automation
mode the search fallback is **disabled** — a bare word is `E_BAD_URL`, because silently turning a typo into
a DuckDuckGo query is a determinism hazard in a script.

Default `--wait` is `load`. Note §5.3: `load` is genuinely early; `network-idle` or `quiet` is usually what
a script wants, and this spec deliberately makes that an explicit choice rather than a hidden default.

```json
"result": {
  "tab": 0, "opened": true, "new_tab": false,
  "requested_url": "https://example.com",
  "final_url": "https://example.com/",
  "redirect_chain": ["https://example.com", "https://example.com/"],
  "http_status": 200,
  "waited_for": "load", "wait_satisfied_ms": 233,
  "epoch": 4
}
```

`http_status` is `null` for `file:`, `data:` and `about:` schemes. A navigation that reaches
`did-fail-load` (`main.js:122-124`) returns `E_NAVIGATION_FAILED` with `detail.code` and `detail.desc`
carried verbatim from Chromium.

### 6.2 `list`

```
blackglass list [--json]
```

Wraps B04 `tab.list`. Pure observation, never mutates, never waits.

```json
"result": {
  "active": 0,
  "tabs": [
    {"tab": 0, "url": "https://example.com/", "title": "Example Domain",
     "active": true, "loading": false, "crashed": false, "epoch": 4,
     "painting": true, "opened_ms": 1754003390011},
    {"tab": 1, "url": "about:blank", "title": "",
     "active": false, "loading": false, "crashed": false, "epoch": 1,
     "painting": false, "opened_ms": 1754003399820}
  ]
}
```

`painting` is B04's invariant made visible: exactly one tab has `painting: true` at any time. `title` is
page-controlled — see §9.2.

### 6.3 `focus`

```
blackglass focus <tab> [--json]
```

Wraps B04 `tab.activate`. B04 measured that activation emits a full frame at current geometry even for a
static page, so a focus is complete once that frame is observed.

```json
"result": { "tab": 1, "previous": 0, "already_active": false, "frame_observed": true, "epoch": 1 }
```

Unknown or closed tab → `E_NO_SUCH_TAB` (exit 4). `focus` on the already-active tab is a success with
`already_active: true` and dispatches nothing — B04 specifies `tab.activate` is a no-op in that case.

### 6.4 `snapshot` — and the element reference design

```
blackglass snapshot [--scope <ref|--selector CSS>] [--role ROLE] [--name TEXT]
                    [--interactive-only|--all] [--max-nodes N] [--depth N] [--json]
```

`snapshot` is the **only** verb that mints refs. This is deliberate: a ref must correspond to something the
caller has observed, so that "act on what you saw" is structurally enforced.

**Ref format.** `bg1.<tab>.<epoch>.<ordinal>` — e.g. `bg1.0.3.17`. Opaque to callers by contract, but
structured so the session host can reject a stale or cross-tab ref **without a round trip to the page**:

| Check | Failure |
|---|---|
| prefix `bg1` | `E_BAD_REF` |
| `<tab>` equals the addressed tab | `E_WRONG_TAB` |
| `<epoch>` equals the tab's current epoch | `E_STALE_REF` |
| `<ordinal>` resolves in the page registry | `E_STALE_REF` with `detail.state` = `NO_SUCH` / `DETACHED` / `COLLECTED` |

**Epoch.** A per-tab counter incremented on every cross-document commit. §5.4 measured that the injected
registry is destroyed by such a navigation anyway (`"GONE"`); the epoch turns that from an obscure
resolver error into a local, immediate, precisely-attributed rejection. Same-document navigation does
**not** bump the epoch — measured, the registry survives `location.hash` changes.

**Backing store.** For each snapshotted node the session host keeps `{ordinal, backend_node_id, ax_node_id,
object_id}` and the page keeps `Map<ordinal, WeakRef<Element>>`. `backend_node_id` is the join key between
the AX and DOM views — measured identical (`9` in both, §5.4). `WeakRef` means a snapshot of 5 000 nodes
does not pin 5 000 elements in memory; `COLLECTED` is a distinct, reportable state.

**Why not CSS selectors as the primary identity.** A selector is re-resolved at use time, so the element
acted upon may not be the element observed — the exact race A03 c-F3 describes. Selectors are supported
as a convenience (`--selector`), but every mutating verb reports the `ref` it resolved to, so a trace is
replayable even when the invocation used a selector.

**Why not AXNodeId alone.** Measured stable across calls, but only defined while `Accessibility` is
enabled, and it addresses an AX node rather than a DOM node. It is carried in the output as a join key,
not used as the handle.

```json
"result": {
  "epoch": 3,
  "scope": "document",
  "node_count": 4,
  "truncated": false,
  "nodes": [
    {"ref": "bg1.0.3.17", "role": "button", "name": "Sign in",
     "backend_node_id": 9, "ax_node_id": "9",
     "box": {"x": 36, "y": 90.875, "w": 73, "h": 35},
     "in_viewport": true, "focusable": true, "disabled": false,
     "tag": "BUTTON", "attrs": {"id": "b1"}},
    {"ref": "bg1.0.3.18", "role": "textbox", "name": "",
     "backend_node_id": 12, "ax_node_id": "12",
     "box": {"x": 36, "y": 140, "w": 180, "h": 24},
     "in_viewport": true, "focusable": true, "disabled": false,
     "tag": "INPUT", "attrs": {"id": "inp", "placeholder": "type here"},
     "value_masked": false}
  ],
  "untrusted": true,
  "source": "accessibility"
}
```

`--interactive-only` (the default) keeps nodes that are focusable, have a click handler, or carry an
interactive AX role. `--all` returns the full tree. `--max-nodes` defaults to **2 000**; exceeding it sets
`truncated: true` and adds `W_TRUNCATED` — it never errors, because a truncated snapshot is still useful
and an agent that gets an error learns nothing.

`--role` / `--name` map onto `Accessibility.queryAXTree`, measured at 675 bytes vs 9 327 for the full tree.
Prefer them.

`untrusted: true` is always present on `snapshot` output. See §9.2.

### 6.5 `click`

```
blackglass click <ref> | --selector CSS [--button left|middle|right] [--count N]
                 [--modifiers ctrl,shift,alt,meta] [--position center|topleft|X,Y]
                 [--force] [--timeout MS] [--json]
```

Runs the actionability gate (§7.6) until it passes or the deadline expires, then dispatches
`mouseMove` → `mouseDown` → `mouseUp` through the existing input path (`main.js:157-204`), which already
handles the `mouseEnter` latch that CSS `:hover` requires offscreen (`main.js:160-168`).

```json
"result": {
  "ref": "bg1.0.3.17",
  "resolved_from": "ref",
  "point": {"x": 72, "y": 108},
  "button": "left", "count": 1,
  "dispatched": ["mouseMove", "mouseDown", "mouseUp"],
  "actionability": {"attached": true, "visible": true, "enabled": true,
                    "hit_self": true, "stable": true,
                    "attempts": 3, "settled_ms": 812},
  "forced": false
}
```

`--force` skips the gate and dispatches at the geometric centre regardless. It sets `forced: true` and
emits `W_FORCED`. It exists because some legitimate UIs never satisfy the hit test (custom overlays that
intentionally proxy events), and a spec with no escape hatch gets worked around in worse ways.

`--position X,Y` is relative to the element's border box, in CSS pixels.

### 6.6 `type`

```
blackglass type <ref>|--selector CSS <text> [--clear] [--method insert|keys]
                [--delay MS] [--timeout MS] [--json]
```

Focuses the element (through the actionability gate), optionally clears it, then inserts text.

`--method insert` (default) uses CDP `Input.insertText` — measured to place `"héllo wörld"` correctly in a
single call. `--method keys` replays `keyDown`/`char`/`keyUp` per character via the existing path
(`main.js:210-219`), which is what a page with per-keystroke handlers may require, and what `--delay`
applies to. The default is `insert` because the per-character path is B06's F9 amplification.

```json
"result": {
  "ref": "bg1.0.3.18", "method": "insert", "chars": 11, "cleared": false,
  "value_after": null, "value_read_back": false,
  "actionability": {"attached": true, "visible": true, "enabled": true, "hit_self": true, "stable": true}
}
```

`value_after` is `null` unless `--read-back` is passed, and the flag exists so that reading a password
field back into a log is always an explicit act (§9.3).

### 6.7 `key`

```
blackglass key <combo> [--repeat N] [--json]
```

Sends a key to the focused element, not to an element by ref. Combos are `+`-separated:
`ctrl+r`, `alt+Left`, `Enter`, `shift+Tab`, `F5`. Names map through `electron_key`
(`main.rs:741-761`), which is the single existing source of truth for this mapping.

**Shortcut interception is bypassed.** `main.rs:569-592` intercepts `ctrl+q`, `ctrl+r`, `alt+Left`,
`alt+Right` as *browser* shortcuts before the page sees them. The `key` verb targets the **page**, so it
must route around that interception; a script asking for `ctrl+r` wants the page's handler, and a script
wanting a reload has `open --tab N <same url>` or a dedicated reload. This asymmetry is a deliberate
divergence from the interactive path and is called out because it is exactly the kind of thing that
silently differs otherwise.

```json
"result": {"combo": "ctrl+shift+k", "key_code": "k",
           "modifiers": {"ctrl": true, "shift": true, "alt": false, "meta": false},
           "repeat": 1, "dispatched": ["keyDown", "keyUp"], "target": "focused"}
```

### 6.8 `hover`

```
blackglass hover <ref>|--selector CSS [--position …] [--timeout MS] [--json]
```

Same gate as `click`, dispatches `mouseMove` only. `main.js:160-168` already sends `mouseEnter` before the
first move, which is required for CSS `:hover` under OSR; the result reports whether that latch fired, since
a hover that does not trigger `:hover` is a confusing silent failure.

```json
"result": {"ref": "bg1.0.3.17", "point": {"x": 72, "y": 108},
           "entered": true, "dispatched": ["mouseEnter", "mouseMove"],
           "actionability": {"attached": true, "visible": true, "enabled": true,
                             "hit_self": true, "stable": true}}
```

### 6.9 `scroll`

```
blackglass scroll [--by DX,DY] [--to X,Y] [--into <ref>] [--page up|down] [--timeout MS] [--json]
```

Exactly one of `--by` / `--to` / `--into` / `--page`.

`--by` dispatches wheel events. Measured sign convention: `deltaY: -300` scrolls **down** to `scrollY 300`,
matching `main.rs:630` where `WheelDown` maps to `-120`. **The CLI surface inverts this**: `--by 0,300`
means "scroll down 300 CSS px", because a user-facing tool with an inverted Y axis is a bug generator. The
inversion happens at the CLI boundary and the result reports both.

`--to` and `--into` are scripted scrolls (`scrollTo` / `scrollIntoViewIfNeeded`), which are exact.
`--by` is a wheel dispatch and is **not** exact: it is subject to smooth-scrolling, scroll-snap and
`overscroll-behavior`. The result always reports observed `from` and `to`, so a caller can detect the
difference rather than assume it.

```json
"result": {"mode": "by", "requested": {"dx": 0, "dy": 300},
           "wheel_delta": {"deltaX": 0, "deltaY": -300},
           "from": {"x": 0, "y": 0}, "to": {"x": 0, "y": 300},
           "exact": false, "at_bottom": false, "scroll_height": 4000}
```

### 6.10 `wait`

Specified in full in §7.

### 6.11 `eval`

```
blackglass eval <expression> [--file PATH] [--await] [--arg NAME=JSON]...
                [--on <ref>] [--timeout MS] [--json]
```

**Disabled by default.** `serve` must be started with `--allow-eval`; otherwise `E_EVAL_DISABLED`
(exit 11). `eval` is arbitrary code execution in the page's origin with access to its cookies and storage,
and it is the one verb whose blast radius is not bounded by the actionability gate.

Implemented on `Runtime.evaluate` with `returnByValue: true` and `awaitPromise: true`, **not** on
`executeJavaScript`, for the error-fidelity reason measured in §5.2. With `--on <ref>` it becomes
`Runtime.callFunctionOn` against that element's `objectId`, and the expression is a function body whose
`this` is the element — the same mechanism the actionability gate uses.

```json
"result": {
  "value": {"a": 1, "b": "x"},
  "type": "object",
  "serialized": true,
  "truncated": false,
  "untrusted": true
}
```

Serialisation rules, each derived from a measured failure in §5.2:

| Case | Behaviour |
|---|---|
| Cyclic / too-deep object | `serialized: false`, `E_UNSERIALIZABLE` with the CDP text (`Object reference chain is too long`) — never a hang |
| `BigInt` | `value` is the decimal **string**, `type: "bigint"` |
| `undefined` | `value: null`, `type: "undefined"` — distinguishable from real `null`, which is `type: "null"` |
| DOM node | **not** returned as `{}`. `type: "node"`, and a `ref` is minted for it under the current epoch |
| Function | `E_UNSERIALIZABLE`, `type: "function"` |
| Value over `--max-bytes` (default 1 MiB) | truncated, `truncated: true`, `W_TRUNCATED` |
| Thrown error | `E_EVAL_THREW` with `detail.name`, `detail.message`, `detail.line`, `detail.column`, `detail.stack` from `exceptionDetails` |
| No resolution within `--timeout` | `E_RENDERER_UNRESPONSIVE`, **session-level** (§5.2 — the next command will also hang) |

`untrusted: true` is always set. See §9.2.

### 6.12 `screenshot`

```
blackglass screenshot [--out PATH|-] [--format png|jpeg|webp] [--quality N]
                      [--full-page] [--clip X,Y,W,H] [--of <ref>] [--scale N] [--json]
```

Uses `Page.captureScreenshot`, measured to produce **900×600 for a 900×600 window** — identical to the OSR
frame the terminal renders. `capturePage()` is not used: it returned **1800×1200** for the same window
(§5.5) and would be the only 2× artefact in the system.

`--out -` writes base64 into the envelope under `result.data`; anything else writes the file and reports
its path. Base64 in the envelope is bounded by `--max-bytes`.

```json
"result": {
  "path": "/tmp/shot.png", "format": "png",
  "width": 900, "height": 600, "bytes": 4456, "scale": 1,
  "full_page": false, "clip": null,
  "device_scale_factor": 1,
  "matches_frame_geometry": true
}
```

`matches_frame_geometry` states plainly whether this image has the same dimensions as the frame the
terminal is currently displaying. It is `false` for `--full-page`, `--clip` and `--scale ≠ 1`.

**`--full-page` suspends the frame stream.** §5.8 measured that `captureBeyondViewport` injects a
900×3400 paint into the live OSR stream. The sequence must be: suspend frame forwarding → capture →
resume → force one repaint at the true geometry. The result carries `W_FRAME_STREAM_SUSPENDED` with the
measured suspension duration, because a 187 ms freeze of the user's view is something a caller should be
able to see in a log.

### 6.13 `close`

```
blackglass close [<tab>] [--all] [--session] [--json]
```

Wraps B04 `tab.close`, which stops painting, detaches the paint listener, destroys the window, and
auto-activates the next remaining tab or emits `tabs.empty`.

```json
"result": {"closed": [1], "remaining": [0], "active": 0, "session_ended": false}
```

`--session` shuts down the whole session: sends `{"t":"quit"}` and follows the existing teardown
(`main.rs:657-678`) — a 1 500 ms grace period, then `kill`, then socket and directory removal. Closing the
last tab does **not** end the session; it leaves the session alive with zero tabs so a subsequent `open`
does not pay Electron cold start again.

Closing a tab invalidates every ref for that tab. Refs for other tabs are unaffected — this is why the tab
id is inside the ref.

### 6.14 `status`

```
blackglass status [--json] [--wait-ready MS]
```

The health verb, and the only verb that is meaningful when the session is unhealthy. It must never block on
the renderer, because §5.2 measured that a wedged renderer hangs every page-scoped call — `status` reads
session-host state and process liveness only.

```json
"result": {
  "session": "bg-8f3a1c",
  "socket": "/var/folders/…/blackglass-4711-…/engine.sock",
  "pid": 4711, "engine_pid": 4712,
  "uptime_ms": 91234,
  "protocol": {"envelope": 1, "wire": 1, "cdp": "1.3"},
  "engine": {"electron": "43.2.0", "chrome": "150.0.7871.129",
             "connected": true, "responsive": true, "last_frame_ms_ago": 12},
  "terminal": {"backend": "kitty", "viewport": {"w": 2482, "h": 814},
               "cell": {"w": 17, "h": 37}, "pixel_mouse": true},
  "tabs": {"count": 2, "active": 0, "crashed": []},
  "capabilities": {"eval": false, "snapshot": true, "screenshot": true},
  "control": "AGENT"
}
```

`engine.responsive` is derived from a liveness probe, which B06's F8 correctly notes does not exist yet.
`control` reflects A03's arbitration state machine.

`--wait-ready MS` blocks until the session is up, for scripts that start `serve` and immediately issue
commands. Without it, `status` returns immediately with `ok: false` / `E_NO_SESSION` when nothing is
listening.

---

## 7. Wait semantics

This is the section the rest of the spec exists to support. §5.3, §5.6 and §5.7 established that no single
signal is sufficient and that the two most commonly used heuristics — load events and adjacent-frame
stability — are measurably wrong.

### 7.1 Surface

```
blackglass wait --for <predicate> [--timeout MS] [--poll-ms MS] [--quiet-ms MS]
                [--across-navigation] [--json]
```

`--for` takes exactly one predicate. Compose with `&&` for a conjunction evaluated against a single
deadline, e.g. `--for 'network-idle && visible=bg1.0.3.17'`.

### 7.2 Predicates

| Predicate | Satisfied when | Source |
|---|---|---|
| `dom` | lifecycle `DOMContentLoaded` for the current `loaderId` | `Page.lifecycleEvent` |
| `load` | lifecycle `load` for the current `loaderId` | `Page.lifecycleEvent` |
| `paint` | lifecycle `firstContentfulPaint` | `Page.lifecycleEvent` |
| `network-idle` | lifecycle `networkIdle` (Chromium: 0 in-flight for ~500 ms — measured 512 ms, §5.3) | `Page.lifecycleEvent` |
| `network-almost-idle` | lifecycle `networkAlmostIdle` (≤2 in-flight for the same window) | `Page.lifecycleEvent` |
| `quiet` | `network-idle` **and** no paint for `--quiet-ms` **and** no subtree DOM mutation for `--quiet-ms` | composite, §7.4 |
| `ref=<ref>` | ref resolves and the element is attached | page registry |
| `visible=<ref\|CSS>` | attached, non-zero border box, `visibility ≠ hidden`, `display ≠ none`, `opacity > 0` | `callFunctionOn` |
| `hidden=<ref\|CSS>` | not attached, or fails any visibility clause | `callFunctionOn` |
| `stable=<ref>` | §7.5 | `callFunctionOn` |
| `actionable=<ref>` | §7.6 | `callFunctionOn` |
| `text=<string>` | the string appears in the AX tree's accessible names or text content | `queryAXTree` |
| `selector=<CSS>` | `document.querySelector` matches | `Runtime.evaluate` |
| `count=<CSS>:<N>` | `querySelectorAll(CSS).length === N` | `Runtime.evaluate` |
| `js=<expr>` | expression evaluates truthy | `Runtime.evaluate`, requires `--allow-eval` |
| `url=<glob>` | the tab's committed URL matches | session host, no page round trip |
| `title=<glob>` | the tab's title matches | session host, no page round trip |
| `idle` | no frames delivered for `--quiet-ms` | session host, frame stream only |

`idle` deserves a note: it is the only predicate that needs neither CDP nor the page, so it is the one that
still works against a wedged renderer. It is the right predicate for "has the screen stopped changing".

### 7.3 The determinism rules

**R1 — Every wait is bound to a document generation.** The predicate is evaluated against the `loaderId`
current at command receipt. Measured (`probe5`): the main frame's `frameId` is **stable** across
navigation, while `loaderId` **changes** — so `frameId` is useless as a generation marker and `loaderId` is
correct. If the `loaderId` changes mid-wait, the wait fails with `E_NAVIGATED` (exit 6) unless
`--across-navigation` is passed, in which case the wait rebinds to the new generation and the result
reports `rebound: true`. Silently continuing across a navigation is how a script ends up asserting against
the wrong page.

**R2 — Discard lifecycle events replayed on enable.** Measured (`probe5`): enabling
`Page.setLifecycleEventsEnabled` immediately replayed **5** lifecycle events — `commit`,
`DOMContentLoaded`, `load`, `networkAlmostIdle`, `networkIdle` — all belonging to the *previous* document
and all timestamped at offset 0 relative to the subsequent navigation. A `wait --for network-idle` that
accepted these would return success in under a millisecond, every time, against the wrong document. Every
lifecycle event carries `loaderId`; accept only events whose `loaderId` matches the bound generation.
Measured: all nine events for the new document carried `sameLoader: true`.

**R3 — `did-finish-load` is not readiness.** Measured 855 ms before `networkIdle` and 1 468 ms before the
page stopped mutating (§5.3). It may be used to satisfy `load`, and for nothing else.

**R4 — Never reimplement network idle from request counters.** Measured, the raw in-flight count reached
zero at 15 ms and the page then issued another request at 321 ms (§5.3). Use Chromium's `networkIdle`.

**R5 — Stability is a wall-clock property, not a frame-pair property.** Measured (§5.6), two consecutive
`requestAnimationFrame` samples of an element moving 10 px every 50 ms returned **identical** coordinates
(`[130, 130]`) and were indistinguishable from the genuinely-settled case (`[400, 400]`). Therefore
`stable=` requires the border box to be unchanged across samples spanning **at least `--quiet-ms`
(default 800 ms)** of wall clock, with a minimum of 3 samples. The 800 ms default is above the largest
inter-paint gap measured during settling (692 ms, §5.7) — that is a measured lower bound, **not a proof**;
an animation with a period above 800 ms will still read as stable, and this spec does not claim otherwise.

**R6 — Absolute deadlines.** The deadline is computed once, at command receipt, as
`started_ms + timeout`. Polls, retries and the actionability gate all share it.

**R7 — Report the evidence.** Every `wait` result carries the sample count and the moment the predicate
flipped, so a flaky wait can be diagnosed from its own output rather than by re-running it.

**R8 — No hidden waits.** No verb other than the pointer verbs' actionability gate waits for anything, and
that gate is reported in the result. If a script needs the page settled, it says so.

### 7.4 `quiet`, precisely

`quiet` is satisfied at the first instant all three hold simultaneously:

1. lifecycle `networkIdle` has fired for the bound `loaderId`; **and**
2. no OSR frame has been delivered for `--quiet-ms`; **and**
3. no `DOM.childNodeInserted` / `childNodeRemoved` / `characterDataModified` / `attributeModified` for
   `--quiet-ms`.

Against `fixture.html` this resolves at roughly `1 499 + 800 = 2 299 ms` after load, versus 31 ms for
`did-finish-load` — a 74× difference in what "ready" means, on a fixture with two `setTimeout` calls in it.

Clause 2 is the one that catches CSS animations, which produce frames without DOM mutations or network
activity. A page with a permanent spinner never reaches `quiet`; that is correct behaviour, and the
timeout error reports which clause was unsatisfied so the caller can drop to `network-idle`.

### 7.5 `stable=<ref>`

Sample the border box every `--poll-ms` (default 100 ms). Satisfied when the last **N ≥ 3** samples are
identical *and* span ≥ `--quiet-ms`. `probe6`'s mover, sampled this way, is correctly reported as moving
throughout its animation and stable after.

### 7.6 `actionable=<ref>` — the gate on `click`, `hover` and `type`

All five clauses, evaluated in one `Runtime.callFunctionOn` round trip against the element's `objectId`:

| # | Clause | Measured behaviour |
|---|---|---|
| 1 | `attached` — `el.isConnected` | detects node replacement as `DETACHED` (§5.4) |
| 2 | `visible` — non-zero box, `visibility ≠ hidden`, `display ≠ none`, `opacity > 0` | hidden button reported `w:0, h:0, visible:false` |
| 3 | `enabled` — `!el.disabled` | — |
| 4 | `hit_self` — `document.elementFromPoint(cx, cy)` is the element or a descendant | **predicts click outcome in both directions**: occluded → 0 handler calls; un-occluded → 1 (§5.6) |
| 5 | `stable` — per R5 | — |

The gate re-evaluates every `--poll-ms` until all five pass or the deadline expires. On expiry:
`E_NOT_ACTIONABLE` (exit 5) with `detail.checks` naming every clause and its last value, plus
`occluded_by` when clause 4 failed. An agent that gets "covered by `DIV#ov`" can act; one that gets
"click failed" cannot.

Clause 4 is what makes `--force` necessary and what makes it dangerous, hence `W_FORCED`.

### 7.7 Result schema

```json
"result": {
  "predicate": "network-idle && visible=bg1.0.3.17",
  "satisfied": true,
  "satisfied_at_ms": 886,
  "clauses": [
    {"clause": "network-idle", "satisfied": true, "at_ms": 886, "source": "lifecycle"},
    {"clause": "visible=bg1.0.3.17", "satisfied": true, "at_ms": 190, "source": "callFunctionOn"}
  ],
  "samples": 9,
  "loader": "8B1F…",
  "rebound": false,
  "deadline_ms": 30000
}
```

On timeout, `ok: false`, `E_TIMEOUT` (exit 3), and the same structure with `satisfied: false` and each
clause's last observed state — a timeout must say *what it was still waiting for*.

### 7.8 Default timeouts

| Verb | Default `--timeout` | Rationale |
|---|---:|---|
| `open` | 30 000 ms | Matches the existing 30 s engine-connect bound (`main.rs:415`) |
| `wait` | 30 000 ms | — |
| `click`, `hover`, `type` | 5 000 ms | Gate convergence; measured click dispatch-to-handler was 6 ms, so 5 s is almost entirely settle budget |
| `snapshot` | 5 000 ms | Full AX tree measured at 8 ms for 23 nodes; 5 s covers pathological trees (A03 c-F7's 100k-node case) |
| `eval` | 5 000 ms | Bounded because a wedged renderer never returns (§5.2) |
| `screenshot` | 10 000 ms | Full-page capture measured at 187 ms; headroom for large pages |
| `key`, `scroll` | 5 000 ms | — |
| `list`, `focus`, `close`, `status` | 2 000 ms | Session-host state or a single engine round trip |

`--poll-ms` defaults to 100 ms; `--quiet-ms` to 800 ms.

### 7.9 Cancellation

| Trigger | Behaviour |
|---|---|
| `SIGINT` / `SIGTERM` on a verb process | Send `{"t":"cancel","id":…}` to the session host, wait up to 500 ms for acknowledgement, print the envelope with `E_CANCELLED`, exit **9**. |
| `--timeout` expiry | Same cancel message, then `E_TIMEOUT`, exit **3**. |
| Verb process dies without cancelling | The session host detects socket close and cancels the command bound to that connection. Commands are bound to their connection precisely so this is possible. |
| Session host shutdown mid-command | In-flight commands get `E_SESSION_CLOSED`, exit 7. |

**Cancellation is not a rollback, and the spec says so explicitly.** A cancelled `click` may already have
dispatched `mouseDown`. A cancelled `type` may have inserted part of its text. A cancelled `open` may have
committed a navigation. Any cancelled envelope therefore carries
`error.detail.side_effects_possible: true` and, where known, `error.detail.dispatched: [...]` listing what
actually went out. Pretending otherwise would be the single most dangerous lie in this spec: a script that
retries a cancelled `click` on a "Pay" button needs to know the first one may have landed.

Only `wait`, `list`, `status`, `snapshot` and `screenshot` are guaranteed side-effect-free; their cancelled
envelopes carry `side_effects_possible: false`.

### 7.10 Concurrency

One command in flight per tab, enforced at the session host. A second command for a busy tab either queues
(`--queue`, up to `--queue-depth`, default 8) or fails immediately with `E_BUSY` (exit 12) — the default.
Determinism requires serialisation: two concurrent clicks on one page have no defined interleaving.

Commands addressing *different* tabs run concurrently. Under B04's one-painter invariant a background tab
still loads and runs JavaScript normally, so a background `wait --for network-idle` is well-defined even
though the tab is not painting.

`control` arbitration (A03) sits above this: while `control` is `HUMAN`, every mutating verb returns
`E_HUMAN_CONTROL` (exit 13). Observation verbs continue to work, because locking an agent out of *looking*
serves nothing.

---

## 8. Exit codes and error taxonomy

`0`, `1` and `2` keep their existing meanings from `main.rs:54-71`: success, generic failure, usage error.

| Exit | Code family | Meaning | Retry sensible? |
|---:|---|---|---|
| 0 | — | Success | — |
| 1 | `E_INTERNAL` | Internal error, unexpected engine reply, protocol desync | no |
| 2 | `E_USAGE`, `E_BAD_URL`, `E_BAD_REF`, `E_BAD_ARGS` | Caller error, detected before anything was dispatched | no |
| 3 | `E_TIMEOUT` | Deadline expired with the predicate or gate unsatisfied | yes, with a longer deadline |
| 4 | `E_STALE_REF`, `E_NO_SUCH_TAB`, `E_WRONG_TAB`, `E_NOT_FOUND` | The thing addressed does not exist or no longer exists | only after a fresh `snapshot` |
| 5 | `E_NOT_ACTIONABLE` | Element exists but failed the gate | yes |
| 6 | `E_NAVIGATION_FAILED`, `E_NAVIGATED` | Navigation failed, or the document changed mid-command | yes |
| 7 | `E_NO_SESSION`, `E_SESSION_CLOSED` | No session host reachable | no, start `serve` |
| 8 | `E_RENDERER_GONE`, `E_RENDERER_UNRESPONSIVE`, `E_TAB_CRASHED` | Engine-side fault; the tab needs `tab.recover` (B04) | no, recover first |
| 9 | `E_CANCELLED` | Cancelled by signal or peer disconnect | see §7.9 — **side effects possible** |
| 10 | `E_PROTOCOL_VERSION` | Envelope or wire version mismatch | no |
| 11 | `E_EVAL_DISABLED`, `E_PERMISSION_DENIED` | Capability not enabled for this session | no |
| 12 | `E_BUSY` | Tab already has a command in flight | yes |
| 13 | `E_HUMAN_CONTROL` | A03 arbitration: a human holds input | yes, after handback |
| 14 | `E_UNSERIALIZABLE`, `E_EVAL_THREW` | `eval` produced a value that cannot cross the boundary, or threw | no |

Two invariants. First, **the exit code is always derivable from `error.code`**, so a consumer may use
either without them ever disagreeing. Second, `error.retryable` is advisory and must never be the only
signal a caller has — the code is the contract.

Warnings never affect the exit code:

| Warning | Raised by |
|---|---|
| `W_TRUNCATED` | `snapshot` past `--max-nodes`, `eval` past `--max-bytes` |
| `W_FORCED` | `click --force` |
| `W_FRAME_STREAM_SUSPENDED` | `screenshot --full-page` (§5.8) |
| `W_INEXACT_SCROLL` | wheel-based `scroll` whose observed delta differs from the request |
| `W_STALE_SNAPSHOT` | a mutating verb whose ref came from an epoch that is current but whose snapshot is older than `--quiet-ms` and the page has painted since |

---

## 9. Security

### 9.1 The automation socket is a browser-takeover primitive

B06's **F4** already establishes that the listener accepts the first connection with no authentication and
that the socket path is discoverable. For automation this is worse than for the interactive case: the
socket is long-lived, its path is predictable enough to race, and the verb surface now includes `eval`,
`screenshot` and `snapshot`. A same-uid process that wins the race can read every page the user has open
and type into them. F4's fix is a prerequisite for shipping `serve`, not a follow-up.

### 9.2 Page-controlled strings reach a machine consumer

`snapshot`, `list`, `eval` and the `doc.title` field all carry attacker-controlled text. Two distinct
hazards, and they need different mitigations.

**Terminal injection (A09 TB3).** The interactive renderer already sanitises before writing to the tty
(`main.rs:887-888`, `unicode::sanitize_for_terminal`). Automation output is JSON, and `json_escape`
(`bg-proto/src/lib.rs:106-118`) correctly escapes control bytes below `0x20` — including `ESC` as
`\u001b`. So JSON output is safe *as JSON*. It stops being safe the moment a consumer pipes it through
`jq -r` into a terminal. This spec therefore requires the `--raw-text` opt-in for any output mode that
emits page text unescaped, and recommends consumers keep the escaping.

**Prompt injection (A03 c-F6).** Every field carrying page text ships under `untrusted: true` at the top of
its result object. `snapshot` and `eval` always set it. This is a marker for the consumer, not a defence —
the actual fencing has to happen where the text meets the model, and this spec cannot enforce that. Saying
so plainly is better than implying the flag is protection.

### 9.3 Secrets

`type --read-back` is opt-in specifically so that a filled password never lands in an action log by
default. `snapshot` sets `value_masked: true` and omits `value` for `input[type=password]` and for any
element with `autocomplete="current-password"` or `"new-password"`. `--include-values` overrides this and
should be considered a debugging flag.

The existing paste path already sends unbounded text in one field (`main.rs:645-652`, B06 F9); `type
--method insert` inherits that shape and needs the same size cap.

### 9.4 Size caps

B06's F3 (no message-size cap in either direction) applies with more force here. Frame sizes are bounded
by geometry — `max_frame = w × h × 4 + 32` — but `snapshot` and `eval` replies are bounded by nothing but
the page. Caps required at the session host, enforced on the length prefix before buffering:

| Payload | Cap |
|---|---|
| `snapshot` reply | 4 MiB, then `W_TRUNCATED` |
| `eval` reply | 1 MiB (`--max-bytes`), then `W_TRUNCATED` |
| `screenshot` inline base64 | 8 MiB, then `E_USAGE` advising `--out PATH` |
| `type` text | 1 MiB |
| Any command | 256 KiB |

---

## 10. Worked example

```bash
#!/usr/bin/env bash
set -euo pipefail
export BLACKGLASS_SESSION=/tmp/bg.sock

blackglass serve --socket "$BLACKGLASS_SESSION" --action-log /tmp/actions.jsonl &
blackglass status --wait-ready 10000 >/dev/null

blackglass open https://example.com --json --wait quiet --timeout 20000 >/tmp/open.json
jq -e '.ok and .result.http_status == 200' /tmp/open.json >/dev/null

# Observe semantically. Scoped query: 675 bytes measured, vs 9327 for the full tree.
REF=$(blackglass snapshot --role button --name "Sign in" --json \
      | jq -r '.result.nodes[0].ref')

blackglass click "$REF" --json | jq -e '.ok and .result.actionability.hit_self' >/dev/null
blackglass wait --for 'quiet' --timeout 15000 --json | jq -e '.result.satisfied' >/dev/null
blackglass screenshot --out /tmp/after.png --json \
  | jq -e '.result.matches_frame_geometry' >/dev/null

blackglass close --session --json >/dev/null
```

Every failure mode in this script is a distinct exit code: a stale `REF` exits 4, an occluded button
exits 5, an unsettled page exits 3.

---

## 11. Test plan

| # | Test | Pass condition |
|---|---|---|
| T1 | `wait --for network-idle` immediately after enabling lifecycle events on an already-loaded page | Does **not** satisfy from the 5 replayed stale events (R2) |
| T2 | `wait --for load` on `fixture3.html`, compare with `--for network-idle` | `network-idle` satisfies ≥ 500 ms later; both report distinct `satisfied_at_ms` |
| T3 | `click` on `occl.html` while the overlay is up | `E_NOT_ACTIONABLE`, exit 5, `detail.occluded_by == "DIV#ov"` |
| T4 | Same after 2 500 ms | `ok`, `actionability.hit_self == true`, page handler fired exactly once |
| T5 | `wait --for stable=<ref>` on `mover.html` during animation | Not satisfied before the animation ends (guards R5's false-stable) |
| T6 | `snapshot`, navigate cross-document, then `click` the old ref | `E_STALE_REF`, exit 4, `detail.state` present |
| T7 | `snapshot`, `location.hash` change, then `click` the ref | Succeeds — same-document navigation preserves refs |
| T8 | `snapshot`, replace the target node, `click` | `E_STALE_REF` with `detail.state == "DETACHED"` |
| T9 | `eval 'document.body'` | `type == "node"` and a ref is minted — never `{}` |
| T10 | `eval` on a cyclic object | `E_UNSERIALIZABLE`, exit 14, no hang |
| T11 | `eval '(()=>{throw new TypeError("x")})()'` | `E_EVAL_THREW` with `detail.message == "x"` — not the generic string |
| T12 | `eval 'while(true){}'` with `--timeout 3000` | `E_RENDERER_UNRESPONSIVE`, exit 8, and `status` still returns |
| T13 | `screenshot` with no flags | `width`/`height` equal the current OSR frame geometry; `matches_frame_geometry == true` |
| T14 | `screenshot --full-page` | `W_FRAME_STREAM_SUSPENDED` present; no off-geometry frame reaches the compositor |
| T15 | `SIGINT` a `click` mid-gate | Exit 9, `side_effects_possible` present and correct |
| T16 | Two concurrent `click`s on one tab | Second returns `E_BUSY`, exit 12 |
| T17 | Verb process killed mid-command | Session host cancels; next command succeeds |
| T18 | `snapshot` on a page with a password field | `value_masked: true`, no `value` key |
| T19 | Page title containing `ESC ] 0 ;` | JSON output escapes it as `\u001b`; round-trips through `json_get_str` |
| T20 | Every verb with `--timeout 1` | Every one returns `E_TIMEOUT` exit 3 within ~1.5 s; none hangs |
| T21 | `wait --for idle` against a wedged renderer | Still returns — proves the frame-only predicate needs no page round trip |
| T22 | Envelope with `v: 2` sent to a `v: 1` consumer | `E_PROTOCOL_VERSION`, exit 10 |

T1, T5 and T14 are the regression guards for the three findings I nearly got wrong.

---

## 12. What is not verified

Marked plainly rather than smoothed over.

| # | Claim | Status |
|---|---|---|
| U1 | Everything measured here was against `file://` and `data:` fixtures. The agent sandbox's network allowlist has no general web origin, so **no measurement involves a real website** — no redirects, no third-party subresources, no service workers. `networkIdle` on a real page behaves differently in degree, and possibly in kind for pages with long-polling or streaming connections, which never reach it. | UNVERIFIED against real sites |
| U2 | The AX tree was measured at 23 nodes. A03 c-F7 warns of 100k-node trees. `--max-nodes 2000` and the `queryAXTree` preference are designed for that case but not tested at that scale. | UNVERIFIED at scale |
| U3 | `--force-renderer-accessibility` may be required for a complete AX tree in some configurations. It was not needed here — `Accessibility.enable` alone produced a populated tree — but the interaction with Chromium's lazy AX modes on large SPAs is untested. | UNVERIFIED |
| U4 | Multi-tab automation was not exercised. B04's tab commands are a design, not yet code; every `--tab N` claim rests on B04's measurements, not mine. | UNVERIFIED (depends on B04) |
| U5 | The session host (`serve`) does not exist. Everything in §3.1 and §7.10 is design. | NOT IMPLEMENTED |
| U6 | The 800 ms `--quiet-ms` default is above the largest gap I measured (692 ms) on one fixture. It is a heuristic with a measured lower bound, not a bound with a proof. | Heuristic, honestly labelled |
| U7 | Cross-origin iframes were not tested. `Runtime.evaluate` addresses one execution context; OOPIFs need per-frame contexts and `Target` domain handling. Refs are specified per-tab, not per-frame, which is a known gap. | UNVERIFIED, gap acknowledged |
| U8 | The measured `capturePage` 2× behaviour is specific to this Retina host (`scaleFactor: 2`). On a 1× display the two APIs would agree and the bug would be invisible — which is an argument for T13 running in CI on both. | Host-specific |

---

## 13. What the commander needs to decide

1. **B06 F4 (socket authentication) is a blocker for `serve`, not a nice-to-have.** A long-lived,
   predictably-pathed, unauthenticated socket that accepts `eval` and `screenshot` is a local browser
   takeover. Everything else in this spec can ship incrementally; this cannot.
2. **`crates/bg-proto` needs an automation message type.** Commands here are request/response with
   correlation ids and cancellation; the current protocol is fire-and-forget commands plus unsolicited
   events (`bg-proto/src/lib.rs:11-13`). I propose `T_RPC = 11` (request) and `T_RPC_REPLY = 12`, carrying
   `{id, cmd, args}` / `{id, ok, result|error}`, leaving types 1/2/10 untouched. I did not edit `crates/`.
3. **The engine must attach CDP after the first document commits** (§5.1) and enable `Runtime`, `DOM`,
   `Page`, `Accessibility` and `Network` there. `main.js:288-309` currently has no debugger attach at all.
4. **Confirm the `key` interception divergence** (§6.7) is wanted before it is built. It is defensible but
   it is a real behavioural difference between the interactive and automation paths.
