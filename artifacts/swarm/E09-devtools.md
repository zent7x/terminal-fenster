# E09 — Developer workflows: inspect, console, network, responsive, DOM/CSS editing

**Mission:** specify the developer workflows (inspect element, console, network, responsive viewport,
DOM/CSS editing, element bounds overlay); decide what runs through CDP versus a separate DevTools
window; design the terminal UI for the console and network panes.

**Owner of this file:** E09. **Nothing in `crates/`, `apps/cli/`, `apps/engine/src/main.js`, or
`packages/mcp/` was edited.** Every change those files need is written up in §13 for the commander.

**Machine:** macOS 26.1, Apple M4, Electron 43.2.0 / Chromium 150.0.7871.129, Node v24.11.1,
Ghostty 1.3.1.

**Status:** implementation spec whose load-bearing claims are **measured on this machine**. Five
Electron probes ran against a real offscreen `BrowserWindow` driven over `webContents.debugger`.
Every number below carries the probe that produced it. Anything not measured is marked **UNVERIFIED**.

---

## 0. Recommendation in one paragraph

Run **every** developer workflow through CDP on `webContents.debugger` — the in-process route E01 §3.2
reserved for first-party verbs — and treat a real DevTools window as a rarely-used escape hatch for
the three workflows whose output is a 2-D visualization (Performance flamechart, Memory heap snapshot,
Coverage), because those are the only ones a terminal genuinely cannot render. The single most
consequential measurement in this mission is that **`Overlay.highlightNode` does not composite into
the offscreen paint stream** (§1 P10: zero paint events, zero pixels changed) even though the same
highlight *is* present in `Page.captureScreenshot` (§1 D1: 75,206 vs 66,172 PNG bytes). Chromium
rasterises the inspector overlay on a path Electron's OSR `paint` event does not capture, so BlackGlass
must draw the element-bounds overlay itself — which is the better outcome anyway, since a terminal-text
overlay costs ~150 bytes instead of a ~348 KiB full-frame retransmit (A03 §0.1), stays legible in the
half-block tier where a pixel highlight is an unreadable smear, and needs no viewport reflow at all
(D06 §3.3). Build the overlay on `DOM.getNodeForLocation` + `DOM.getBoxModel`, whose measured p50
round trip is **0.57 ms** (§1 C1+C2) — fast enough to track the cursor — and **not** on
`Overlay.setInspectMode`, whose one genuine advantage (it swallows the click so inspecting a link does
not navigate, §1 B1) we can reproduce ourselves in the input dispatcher. The one hazard that will
otherwise ship as "the inspect box is offset on some sites" is §4 F1: `DOM.getBoxModel` returns **CSS**
pixels while the frame is in **surface** pixels, and those differ by `visualViewport.scale`, measured
at **0.398** on a `mobile:true` viewport whose page lacks a viewport meta tag.

---

## 1. Evidence base

Probe sources live in the scratchpad, outside the repo per the ownership rule:
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/e09/`
(`probe2.js`, `probe3.js`, `probe4.js`, `probe5.js`, `mock.py`, and their `results*.txt`).

Each probe launched a real offscreen `BrowserWindow` (800x600, `sandbox: true`,
`contextIsolation: true`, `offscreen: true`), attached `webContents.debugger` at protocol `1.3`, and
served fixtures from a loopback HTTP server on an ephemeral port so the Network domain saw real HTTP.

### 1.1 Transport and lifecycle

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P0** | `webContents.debugger` attaches to an **offscreen** `webContents`. | **CONFIRMED** | `attach('1.3')` succeeded; `isAttached() = true`. |
| **A1** | Domain enables issued **before the first real navigation** never resolve, although they do take effect. | **CONFIRMED** | `Runtime.enable` on a fresh window: `TIMEOUT(3000ms)`. Yet `Runtime.consoleAPICalled` events flowed afterwards, so the command was delivered. |
| **A2** | The same enables **after** navigation resolve immediately. | **CONFIRMED** | `Log.enable=0.9ms Network.enable=0.3ms Page.enable=0.3ms DOM.enable=0.7ms CSS.enable=4.5ms Overlay.enable=0.4ms Accessibility.enable=0.2ms`. |
| **P19** | A detached DevTools window opens on an **offscreen** `webContents` **and coexists** with our `webContents.debugger` attach. | **CONFIRMED** | `openDevTools({mode:'detach'})` threw nothing; `isDevToolsOpened()=true`; `debugger.isAttached()` still `true`; `Runtime.evaluate('2+2')` still returned `4`. Closes E01 U6 for this pairing. |

### 1.2 The overlay finding

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P10** | `Overlay.highlightNode` composites into the OSR paint stream. | **REFUTED** | Non-white pixels inside the target rect: **before 365, after 365**. **Zero** new paint events, zero damage rects. The call itself returned `ok`. |
| **D1** | The same highlight **is** present in `Page.captureScreenshot`. | **CONFIRMED-PRESENT** | PNG with highlight **75,206 bytes**, without **66,172 bytes**, not byte-identical. So the overlay rasterises; the OSR `paint` path simply does not carry it. |
| **P11** | `Overlay.hideHighlight` is accepted (moot given P10). | PARTIAL | Returned `ok`; ink unchanged at 365 because nothing was ever drawn into the stream. |

This pair is the reason §5.2 draws the overlay in the terminal instead of asking Chromium for it.
Reaching for `captureScreenshot` to recover the highlight would mean abandoning the 60 fps OSR path
that the whole product rests on, so it is not an option.

### 1.3 Inspect-element primitives

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P12** | `Overlay.setInspectMode` + forwarded mouse yields `Overlay.inspectNodeRequested`. | **CONFIRMED** | `{"backendNodeId":11}` after one synthetic move+click. |
| **B1** | In inspect mode the click is **consumed** and never reaches the page. | **CONFIRMED** | `inspectNodeRequested=1`, page-observed clicks **0**. |
| **B2** | Control: with inspect mode **off**, the identical synthetic click **does** reach the page. | **CONFIRMED** | Page reported `PAGECLICK 450 130 styled`. Proves B1 measured a real difference, not a dead input path. |
| **C1** | `DOM.getNodeForLocation` resolves the node under a point with no inspect mode at all. | **CONFIRMED** | n=12, **p50 0.32 ms**, p100 1.38 ms; returned `{backendNodeId:9, frameId:…, nodeId:9}`. |
| **C2** | `DOM.getBoxModel` latency. | **CONFIRMED** | n=12, **p50 0.25 ms**, p100 0.44 ms. |
| **C3** | `CSS.getMatchedStylesForNode` latency. | **CONFIRMED** | n=6, **p50 2.93 ms**. |
| **P9** | `DOM.getBoxModel` returns all four quads in CSS px. | **CONFIRMED** | `content=[100,100,300,100,300,220,100,220] w=200 h=120`, with `padding`/`border`/`margin` quads also present. |

### 1.4 Console plumbing

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P2** | Three independent sources are needed; each carries a different class of message. | **CONFIRMED** | `Runtime.consoleAPICalled` n=4 `[log,warning,error,warning]`; `Log.entryAdded` n=1 `[network/error]`; `Runtime.exceptionThrown` n=1. |
| **P3** | `Log.entryAdded` does **not** re-report `console.*`, so no dedup layer is required. | **CONFIRMED** | Log entries with `source=console-api`: **0 of 1**. |
| **P4** | Uncaught exceptions arrive with a structured stack trace. | **CONFIRMED** | `desc="TypeError: Cannot read properties of null (reading 'boom')"`, `hasStack=true`, `url=http://127.0.0.1:…/`, `line=8`. |
| **P5** | `consoleAPICalled` args are RemoteObjects: primitives inline, objects only as previews or handles. | **CONFIRMED** | `#0[log] string="probe log line" \| number=42 \| object~preview`. |
| **P5b** | **Electron injects its own warning into every page console.** | **CONFIRMED** | Event `#3[warning]` was `"%cElectron Security…This renderer proce…"` — a message the page did not emit. Must be filtered (§4 F4). |
| **G1** | A page can flood the console far faster than a terminal can draw it. | **CONFIRMED** | 5,000 `console.log` calls delivered **5,000** events in **3,069 ms** ≈ **1,629 events/s**, every one forwarded over the debugger. |

### 1.5 Network plumbing

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P6** | The Network domain yields per-request status, mime and timing. | **CONFIRMED** | Event tally `{requestWillBeSent:3, requestWillBeSentExtraInfo:3, responseReceived:4, responseReceivedExtraInfo:3, policyUpdated:1, loadingFinished:4, dataReceived:3}`; `asset.json` → status 200, `application/json`, `timing` present; the 404 → status 404. |
| **P7** | `Network.getResponseBody` returns the real body on demand. | **CONFIRMED** | `len=526 base64=false head={"hello":"world","pad":"xxxx…`. |
| **P8** | `loadingFinished` carries `encodedDataLength` for a Size column. | **CONFIRMED** | n=4, `encodedDataLength=[855, 0, 726, 178]`. |

### 1.6 Responsive viewport — and the coordinate hazard

`probe4` measured, for each configuration, where a `#red` div actually lands **in the OSR surface**
(by scanning the BGRA buffer for pure red) against where `DOM.getBoxModel` says it is **in CSS px**.

| Configuration | OSR frame | Red box in **surface** px | `getBoxModel` in **CSS** px | surface/CSS scale | `[innerW,innerH,vv.scale]` |
|---|---|---|---|---|---|
| Baseline, no emulation | 800x600 | x100..299 y200..299 (200x100) | x100..300 y200..300 (200x100) | **1.000** | `[800,600,1]` |
| 390x844 `mobile:true`, **no** viewport meta | 390x844 | **x40..118 y80..118 (79x39)** | x100..300 y200..300 (200x100) | **0.395** | `[980,2121,0.3979591727]` |
| 390x844 `mobile:true`, **with** viewport meta | 390x844 | x100..299 y200..299 | x100..300 y200..300 | 1.000 | `[390,844,1]` |
| 390x844 `mobile:false`, no viewport meta | 390x844 | x100..299 y200..299 | x100..300 y200..300 | 1.000 | `[390,844,1]` |
| 390x844 `dsf=2`, with viewport meta | 390x844 | x100..299 y200..299 | x100..300 y200..300 | 1.000 | `[390,844,1]`, dpr 2 |

Corroboration (`probe5`): `Page.getLayoutMetrics` reports
`visualViewport.scale = 0.3979591727256775` for row 2 and `1` for the others, and
`cssLayoutViewport.clientWidth = 980` against a 390-wide surface. The measured pixel ratio
(79 surface px for a 200 CSS px box) agrees with the reported scale to within rounding
(`0.39796 x 200 = 79.6`).

**Input is unaffected.** A click sent at the *surface* centre of the red box under the 0.395 scale —
`sendInputEvent` at `(79,99)` — was reported by the page at `clientX/Y = (198,248)`, i.e. the CSS
centre `(200,250)`. Chromium translates surface coordinates through the emulation scale correctly, so
`PointerMap` keeps working. **Overlay drawing is affected**, because it consumes `getBoxModel`'s CSS
quads directly. That asymmetry is §4 F1.

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P16 / E2** | The OSR frame size **follows** `setDeviceMetricsOverride`, and `clearDeviceMetricsOverride` restores it. | **CONFIRMED** | 800x600 → **390x844** on override; back to 1000x700 after clear. |
| **E1** | `win.setSize` (what SIGWINCH triggers today) **silently clobbers** an active override. | **CONFIRMED** | Under override the frame was 390x844; after `win.setSize(1000,700)` it became **1000x700** while the page still reported `innerWidth/Height = [980,2121]` — a desynchronized state. |
| **R1** | Re-applying the override after the resize restores consistency. | **CONFIRMED** | `390x844 → (setSize) 1000x700 → (re-apply) 390x844`. |
| **P17** | UA and touch emulation complete a device profile. | **CONFIRMED** | `setUserAgentOverride` ok; `setTouchEmulationEnabled` ok. |

### 1.7 DOM / CSS editing, tree cost, and method inventory

| # | Claim | Result | Evidence |
|---|---|---|---|
| **P13** | `CSS.getMatchedStylesForNode` returns a usable cascade. | **CONFIRMED** | `matchedCSSRules=3 inheritedLevels=2 selectors=[#styled] origins=[regular]`, with source ranges. |
| **P14** | `CSS.setStyleTexts` live-edits a rule and the result repaints into the OSR stream. | **CONFIRMED** | After rewriting the rule to `background:#ff0000`, **4,301** red pixels appeared inside the 90x50 sample box (≈4,500 expected). |
| **P15** | DOM editing verbs exist. | **CONFIRMED** | `DOM.setAttributeValue` ok; `DOM.getOuterHTML` → `"<div id=\"target\" data-e09=\"1\">TARGET</div>"`; `DOM.setOuterHTML` ok. |
| **F1** | A whole-document tree is expensive; the AX tree is far worse. | **CONFIRMED** | `DOM.getDocument(depth:-1,pierce:true)` = **102,616 bytes in 12 ms**; `Accessibility.getFullAXTree` = **313,467 bytes in 48 ms**, on a 300-element page. |
| **F2** | A depth-limited fetch is ~110x cheaper. | **CONFIRMED** | `DOM.getDocument(depth:2)` = **935 bytes in 2.4 ms**. |
| **P18** | Method-existence sweep, Chromium 150. | PARTIAL | **Present (25):** `Network.{setCacheDisabled,setBlockedURLs,emulateNetworkConditions,searchInResponseBody}`, `Runtime.{globalLexicalScopeNames,getProperties}`, `DOM.{performSearch,getNodeForLocation}`, `CSS.{getComputedStyleForNode,getInlineStylesForNode,getStyleSheetText}`, `Overlay.{setShowFPSCounter,setShowDebugBorders,setShowGridOverlays,highlightRect,setPausedInDebuggerMessage}`, `Emulation.{setEmulatedMedia,setCPUThrottlingRate}`, `Debugger.enable`, `Profiler.enable`, `Performance.enable`, `Audits.enable`, `Page.{captureScreenshot,getResourceTree}`, `Accessibility.enable`. **ABSENT (2): `CSS.setPropertyText`, `Overlay.setShowRulers`.** |

`CSS.setPropertyText` being absent matters: it is the method most tutorials reach for. The live edit
path is `CSS.setStyleTexts` (P14), which takes a `styleSheetId` plus a source `range`.

---

## 2. Transport decision: `webContents.debugger`, not the broker

E01 §3.2 already drew the right line and this mission confirms it holds for the DevTools workloads.

Everything in this document runs over **`webContents.debugger`**, the in-process Electron API. It adds
**zero** OS-level attack surface: no file descriptor, no socket, no flag, nothing for another process
to connect to. P0 confirms it attaches to an offscreen `webContents`, and A2 confirms every domain the
DevTools panes need (`Runtime`, `Log`, `Network`, `Page`, `DOM`, `CSS`, `Overlay`, `Accessibility`)
enables in under 5 ms.

E01's authenticated broker is for a *different* consumer — third-party automation clients such as
Playwright, which need the **browser-level** target that `webContents.debugger` structurally cannot
provide (E01 §3.2, V5). The DevTools panes are first-party UI and need only page-level sessions.

**Consequence for the commander: the DevTools panes must not be built on the broker, and must not
require automation to be granted.** A user pressing `^G d` to read their own console is not an
automation event and must never raise the E01 §6.4 consent prompt. Keeping these two paths separate
also means the DevTools feature ships without waiting on the broker.

One caveat worth stating plainly: `webContents.debugger` is a *page-level* attach. Cross-origin iframes
in separate renderer processes require `Target.setAutoAttach` with `flatten: true` to reach OOPIF
targets. **UNVERIFIED** — no cross-origin iframe was exercised in these probes. Sites with third-party
iframes will show gaps in Elements and Console until this is closed (§14 U3).

---

## 3. What runs through CDP versus a separate DevTools window

### 3.1 The dividing line

**Rule: a workflow lives in a terminal pane when its output is fundamentally text, and in a DevTools
window when its output is fundamentally a 2-D visualization.**

This is not a capability line — P19 proves a DevTools window works fine on an offscreen `webContents`
and even coexists with our debugger attach. It is a *fidelity* line. A console log, a network row, a
DOM subtree, and a CSS rule are text; rendering them as text in the terminal is strictly better than
shipping a screenshot of a GUI rendering of the same text. A flamechart and a heap-snapshot treemap
are spatial; a terminal reproduction would be a lossy caricature.

| Workflow | Where | Why |
|---|---|---|
| Console (read, filter, expand, REPL) | **Terminal pane** | Text. §6. |
| Network (list, headers, body, timing) | **Terminal pane** | Text plus a one-dimensional waterfall, which a terminal renders fine. §7. |
| Elements tree + Styles | **Terminal pane** | Text. Lazy `depth:2` fetch measured at 935 bytes (F2). §5.6. |
| Inspect element / bounds overlay | **Terminal pane** | Must be, since P10 refutes the pixel path. §5.1–5.2. |
| Responsive viewport | **Terminal pane** | It is a property of the engine, not a panel. §5.5. |
| DOM / CSS editing | **Terminal pane** | Text in, repaint out (P14, P15). §5.6. |
| **Sources / breakpoint debugging** | **DevTools window** | Needs `Debugger.pause` plus a synchronised source view, gutter, scopes, watch and call stack simultaneously. A terminal pane could do it, but it is a whole product; not v1. `Debugger.enable` is present (P18) so this stays open. |
| **Performance flamechart** | **DevTools window** | 2-D. `Performance.enable` present. |
| **Memory heap snapshot** | **DevTools window** | 2-D treemap over hundreds of thousands of nodes. |
| **Coverage / Lighthouse** | **DevTools window** | 2-D and report-shaped. |

### 3.2 How the escape hatch behaves

`^G D` (capital D) opens the real DevTools window, **gated on a GUI session being available**. It must
be refused with a plain sentence — not a silent no-op — when there is no display, which is the common
case for this product: over SSH, in CI, or at a locked screen (this machine, right now).

Detection: `process.env.SSH_CONNECTION` or `SSH_TTY` set, or `app.dock` absent, or on Linux `DISPLAY`
and `WAYLAND_DISPLAY` both unset. **UNVERIFIED** as a complete rule; it is a heuristic and the failure
mode must be a message, never a hang.

Two behaviours matter and follow from P19:

1. **Opening the window does not detach us.** `debugger.isAttached()` stayed `true` and
   `Runtime.evaluate` still worked. So the terminal panes keep functioning while the window is open,
   and no state needs tearing down.
2. **The window is a second CDP consumer on the same target.** Both may set overrides. If the user
   sets device emulation in the DevTools window while our responsive mode is active, the last writer
   wins and our pane's displayed state goes stale. Mitigation: while the DevTools window is open,
   BlackGlass should re-read `Page.getLayoutMetrics` before drawing any overlay rather than trusting
   its cached override, and should show a `DT` token in the chrome row so the divided ownership is
   visible.

---

## 4. Findings for the commander (core files I must not edit)

Ranked by risk removed per unit of effort.

### F1 — HIGH — CSS pixels are not surface pixels, and every overlay we draw will be wrong when they diverge

`DOM.getBoxModel` returns **CSS** pixels (P9). The frame BlackGlass renders, and the coordinate space
`PointerMap` operates in (`apps/cli/src/main.rs:701-727`), is **surface** pixels. §1.6 measured these
diverging by a factor of **0.398** on a `mobile:true` viewport whose page has no viewport meta tag: an
element whose box model reads `x100..300` was actually painted at surface `x40..118`.

An overlay drawn straight from the box model would therefore be drawn at more than twice the correct
size, offset, and partly off the 390-wide frame — while the *click* on that same element lands
correctly, because Chromium translates `sendInputEvent` coordinates itself (§1.6). The bug would present
as "the inspect box is offset on some sites, but clicking still works", which is a genuinely confusing
signature to debug after the fact.

*Fix:* every CSS-px quad must pass through one transform before it reaches the renderer:

```
surface_x = (css_x - visualViewport.pageX) * visualViewport.scale + visualViewport.offsetX
surface_y = (css_y - visualViewport.pageY) * visualViewport.scale + visualViewport.offsetY
```

`visualViewport` comes from `Page.getLayoutMetrics` (verified in probe5 to carry `scale`, `pageX`,
`pageY`, `offsetX`, `offsetY`). Re-read it on `Page.frameResized`, on scroll, and after any emulation
change; do not cache it across a navigation. In the common case (no emulation, or emulation of a page
that has a viewport meta) the scale is exactly 1 and the transform is free — which is precisely why
this bug would survive local testing on modern sites and only appear on legacy ones.

### F2 — HIGH — a terminal resize silently breaks responsive mode

E1 measured `win.setSize(1000,700)` resetting the OSR surface to 1000x700 while the page kept its
emulated `innerWidth/Height` of `[980,2121]`. `apps/engine/src/main.js:231-238` calls exactly this on
the `resize` command, which is what SIGWINCH drives. So resizing a tmux pane while responsive mode is
active leaves the engine rendering a desktop-sized surface for a page still laid out as a phone.

*Fix:* the engine's `resize` handler must, when a device-metrics override is active, re-apply
`Emulation.setDeviceMetricsOverride` after `setSize` (R1 confirms this restores the correct state), or
refuse to resize the surface at all and instead let the terminal letterbox the emulated frame. The
second is cleaner: in responsive mode the whole point is that the viewport is *not* the pane, so pane
resizes should change only the scaling, never the emulated viewport.

### F3 — MEDIUM — `PointerMap.page_w/page_h` must track the emulated frame, not the window

`PointerMap` clamps to `page_w`/`page_h` and rejects `py >= page_h` (`main.rs:712-727`). Under an
active override the surface becomes 390x844 (P16) while the terminal pane may be 2482x814, so the
renderer must be scaling the image to fit. Unless `page_w`/`page_h` are refreshed from the *frame*
header rather than from the last `resize` command, every pointer event in responsive mode is clamped
against stale bounds. The frame header already carries `width`/`height`
(`apps/engine/src/main.js:88-94`), so the data is present; it is a question of which value the map is
built from. This is also where C07's scaling factor has to be composed in.

### F4 — MEDIUM — Electron injects its own warnings into the page console

P5b caught `"%cElectron Security…This renderer proce…"` arriving as a `Runtime.consoleAPICalled`
warning the page never emitted. Shipping that into a user-facing console pane would be a bug report on
day one: it is our text, attributed to their site.

*Fix:* filter `consoleAPICalled` events whose first argument begins with the Electron security-warning
marker, or set `ELECTRON_DISABLE_SECURITY_WARNINGS` in the engine environment. Prefer the filter, so
the warning still reaches our own logs. Note the message uses the `%c` format specifier, so the console
formatter must handle specifiers regardless (§6.4).

### F5 — MEDIUM — never `await` a domain enable before the first navigation

A1 measured `Runtime.enable` hanging indefinitely on a fresh window while still taking effect. An
implementation that awaits its enables in sequence at startup will hang forever on the first one and
never reach the rest. A2 shows the same calls resolve in under 5 ms once a document exists.

*Fix:* issue enables **after** the first `did-navigate`/`Page.frameNavigated`, or fire them without
awaiting and rely on the event stream. Any implementation must carry a timeout on
`debugger.sendCommand` regardless; the probes needed one to avoid wedging.

### F6 — LOW — `CSS.setPropertyText` and `Overlay.setShowRulers` do not exist in Chromium 150

P18. Any design or code naming them is dead on arrival. Use `CSS.setStyleTexts` (verified working,
P14). Rulers, if wanted, must be drawn by us.

### F7 — OPERATIONAL — E01's unauthenticated CDP port is **still open**, three hours later

E01 §2.2 flagged an open `--remote-debugging-port=9333` on this machine. It is still live:

```
$ ps -p 37164 -o lstart,command
Fri Jul 31 20:53:18 2026   …/Electron --remote-debugging-port=9333 …
$ curl -s http://127.0.0.1:9333/json/version
{ "Browser": "Chrome/150.0.7871.129", … "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/browser/dbd23dad-…" }
```

Any process on this box can still take that browser over with no credentials (E01 V3). It is not my
process and I did not kill it. **This re-confirms E01's recommendation for a repo-wide rule that test
fixtures never use `--remote-debugging-port`, plus the T-CDP-1 CI check.** My own probe processes were
cleaned up and hold no listening sockets (verified by `lsof -nP -iTCP -sTCP:LISTEN`).

---

## 5. Workflow specifications

### 5.1 Inspect element

**Decision: build on `DOM.getNodeForLocation`, not `Overlay.setInspectMode`.**

`setInspectMode` has exactly one advantage — it consumes the click, so inspecting a link does not
navigate (B1, with B2 as the control). Everything else about it is a liability here: it reports only
`backendNodeId` and expects Chromium's own overlay to provide the visual feedback, which P10 shows we
never receive. We would be paying for a mode whose visible half is missing.

`DOM.getNodeForLocation` gives the same node identity with **p50 0.32 ms** (C1), and we suppress the
click ourselves in the input dispatcher — which we must do anyway, because in inspect mode we also want
to suppress hover, scroll and keyboard forwarding.

The measured round trip for a complete hover update is:

| Step | Method | p50 | p100 |
|---|---|---|---|
| Node under cursor | `DOM.getNodeForLocation` | 0.32 ms | 1.38 ms |
| Its geometry | `DOM.getBoxModel` | 0.25 ms | 0.44 ms |
| **Total** | | **0.57 ms** | **1.82 ms** |

At 0.57 ms the overlay can follow the cursor in real time; there is no need to debounce for latency.
It should still be **coalesced to one in-flight request** — if a reply has not arrived, drop
intermediate mouse positions rather than queueing them, exactly the coalescing discipline
`apps/engine/src/main.js:42-80` already applies to frames.

Selection is sticky: moving the mouse updates the *hovered* node; clicking promotes it to the
*selected* node, which is what the Elements tree and Styles pane follow. `Esc` leaves inspect mode.

Keyboard-only inspection matters more here than in a GUI browser, because a terminal user may have no
mouse (D06 §1.2 shows tmux forwards the wheel, but not every host does). Provide `[`/`]` to move to the
previous/next sibling, `{`/`}` for parent/first child, driven by `DOM.getDocument(depth:1)` walks from
the selected node. This is also the accessible path and the one that works over a serial console.

### 5.2 Element bounds overlay — drawn by us, in text

This is forced by P10 and vindicated by the economics.

| Approach | Wire cost per hover update | Half-block tier | Reflow |
|---|---|---|---|
| Chromium's overlay via OSR | **not available** (P10) | — | — |
| Chromium's overlay via `Page.captureScreenshot` | full-frame PNG, 66–75 KB measured (D1) | unreadable | none, but abandons the 60 fps path |
| **Terminal text overlay (chosen)** | **~150 bytes** | legible | **none** (D06 §3.3) |

The overlay is an **overlay** in D06's precise sense: it draws over page pixels, does not reduce the
viewport, and costs one damage-rect repaint on dismissal from the retained `Renderer::rgb` buffer
(`main.rs:809`). It never triggers a Chromium reflow.

Rendering, all four box-model quads from `DOM.getBoxModel` (P9), each transformed through F1 first:

| Region | Rendered as | Rationale |
|---|---|---|
| margin | dotted rule, dim | outermost; least important |
| border | solid rule | the box most users mean |
| padding | dashed rule | between the two |
| content | corner ticks only | filling it would obscure the very pixels being inspected |

A one-line label sits below the box (or above, if the box is within two rows of the bottom) carrying
`selector`, `WxH` in CSS px, and margin/padding values — the same information Chromium's tooltip
carries, in the space a terminal actually has. When the box is larger than the pane, clamp the drawn
rules to the pane edges and mark the clipped sides with `<` / `>` so the user knows the element
continues off-screen.

In the **half-block / no-graphics tier** this design is the only one that works at all, and it works
well: box-drawing characters are exactly what that tier renders best. Use ASCII `+ - |` when the
terminal lacks box-drawing support, on the same fallback ladder D06 §7.2 uses for the tab strip.

### 5.3 Console — sources and semantics

Three event streams, all required, none overlapping (P2, P3):

| Source | Carries | Terminal treatment |
|---|---|---|
| `Runtime.consoleAPICalled` | `console.log/warn/error/info/debug/table/group/…` | Severity from `type`. Args are RemoteObjects (P5). |
| `Runtime.exceptionThrown` | uncaught errors, unhandled rejections | Severity `E`, always. Stack from `exceptionDetails.stackTrace` (P4). |
| `Log.entryAdded` | network failures, CSP/violation reports, deprecations | Severity from `level`, origin from `source` (measured: `network/error`). |

P3 is what makes this cheap: `Log.entryAdded` never re-reports `console.*`, so the three streams merge
into one ring buffer with **no dedup logic**. Order by the event's own timestamp, not arrival.

**Object rendering.** P5 confirms only primitives arrive inline; objects arrive as a preview or a bare
handle. So the console renders the preview by default and fetches children with `Runtime.getProperties`
(present, P18) only when the user expands a row. This is the same lazy discipline F2 justifies for the
DOM tree, and it is why a 1,284-message console does not cost 1,284 round trips.

**Backpressure.** G1 measured 1,629 events/s sustained. A terminal cannot draw that, and attempting to
would starve the frame path. Required behaviour:

- A fixed-size ring buffer, default 5,000 entries, oldest evicted. Never unbounded.
- Identical consecutive messages collapse to one row with an `xN` counter, as Chrome's console does.
- Render at most 20 Hz regardless of arrival rate; between ticks, accumulate.
- If arrivals exceed the drain rate for more than two seconds, show `+N more` in the status row rather
  than pretending to keep up.

### 5.4 Network — sources and semantics

`Network.enable` then one row per `requestId`, assembled across the event sequence measured in P6:

| Event | Fills |
|---|---|
| `requestWillBeSent` | method, URL, initiator, start time |
| `responseReceived` | status, mimeType, **timing** (verified present), remote address, protocol |
| `dataReceived` | running decoded size |
| `loadingFinished` | **`encodedDataLength`** = wire bytes (P8: `[855, 0, 726, 178]`), end time |
| `loadingFailed` | error text, `canceled` |
| `*ExtraInfo` | raw headers including `Set-Cookie` — **see §11** |

**Bodies are fetched lazily.** `Network.getResponseBody` (P7) is called only for the row the user
opens. This is not merely an optimisation: response bodies are unbounded, and eagerly buffering every
one in a process that also holds 8 MB frame buffers is a memory-exhaustion path.

Note `encodedDataLength` was **0** for one of the four requests — a cached or memory-served response.
The Size column must render that as `(cached)` rather than `0 B`, or users will read it as a broken
measurement.

### 5.5 Responsive viewport

`Emulation.setDeviceMetricsOverride` is the whole mechanism, and P16's key result is that **the OSR
frame size follows the override** — 800x600 became 390x844. This is a genuinely good fit for a terminal
browser: the emulated device becomes the image we ship, and the terminal scales it into the pane.

Three consequences the implementation must handle, all measured:

1. **F2:** a pane resize clobbers the override; re-apply it (R1).
2. **F1/F3:** with `mobile: true` on a page lacking a viewport meta tag, the layout viewport is 980 CSS
   px scaled to the device width — measured `[innerW,innerH] = [980,2121]` behind a 390x844 surface,
   scale 0.398. Overlays need the transform; the pane should *show* the scale so the user understands
   why the page looks shrunken, rather than assuming BlackGlass is broken. The mockup in §8.3 does this.
3. **`mobile: false` behaves completely differently** — measured scale 1.000, `innerWidth` 390. So
   "responsive" and "mobile device" are two distinct toggles and must be presented as such. A width
   change alone (`mobile:false`) is the common case for CSS breakpoint work; full device emulation
   (`mobile:true` + UA + touch, P17) is for testing mobile-specific code paths.

Device presets should ship as data, not code, so they can be corrected without a release. Rotation is
a width/height swap plus a re-apply.

### 5.6 DOM and CSS editing

All verbs verified present and working (P13, P14, P15):

| Action | Method | Evidence |
|---|---|---|
| Read the cascade | `CSS.getMatchedStylesForNode` | P13: 3 rules, 2 inherited levels, selectors and source ranges. p50 2.93 ms (C3). |
| Read computed | `CSS.getComputedStyleForNode` | P18 present. |
| **Edit a rule** | **`CSS.setStyleTexts`** | P14: rewrote a rule, 4,301 red px appeared in the OSR stream. **Not** `CSS.setPropertyText` — absent (F6). |
| Edit an attribute | `DOM.setAttributeValue` | P15. |
| Edit markup | `DOM.getOuterHTML` / `DOM.setOuterHTML` | P15. |

The Elements tree is fetched **lazily**: `DOM.getDocument(depth: 2)` at 935 bytes / 2.4 ms (F2), then
`DOM.requestChildNodes` per expansion. The eager alternative measured 102,616 bytes / 12 ms (F1) on a
300-element page and would scale linearly into the megabytes on a real application — over SSH that is
seconds of stall for a tree the user will look at three nodes of.

Editing UX follows the terminal, not the GUI: `e` on a selected node opens the value in a single-line
editor in the pane's own row (the D06 §7.1 focus model, with the sigil making ownership visible), and
`E` opens the whole `outerHTML` in `$EDITOR` and applies it on save. The second is a capability a GUI
browser cannot match and is the kind of thing that justifies this product existing.

**Edits are ephemeral.** They vanish on reload, exactly as in Chrome DevTools. Say so in the pane —
`edited (not saved)` — because a terminal user is more likely than a GUI user to assume a text edit
persisted somewhere.

---

## 6. Terminal UI — console pane

Every mockup below is generated and validated by `mock.py` in the scratchpad, which asserts that **no
line exceeds its target width** (D06 F2: a row that wraps scrolls the pane and drags the kitty image
out of position) and that every character is printable ASCII. Output: *"OK: 8 blocks, 111 lines, none
exceeds its width, all printable ASCII"*. The implementation should use box-drawing characters where
the terminal supports them; ASCII is the guaranteed-renderable floor, and it is what makes these width
assertions trustworthy.

### 6.1 Split mode, 80 columns

```
-- CONSOLE ------------- 1284  E3  W12 ------- /  --- ^J eval -- ^G d close --
 12:04:01.221 E TypeError: Cannot read properties of null       app.js:8:12
                 at boot (app.js:8:12)
 12:04:01.223 W Deprecated: webkitStorageInfo is obsolete    vendor.js:41:3
 12:04:01.298 L probe log line 42 {a: 1}                        app.js:6:11
 12:04:02.115 L > Object {id: 7, name: 'widget', tags: Array(3)} app.js:9:11
 12:04:02.980 N GET /missing-404 404 (Not Found)
 1281/1284  FOLLOW                                    j/k move  Enter expand
```

### 6.2 Full mode, 80 columns — one object expanded, REPL open

```
-- CONSOLE ------------- 1284  E3  W12 ------- /  --- ^J eval -- ^G d close --
 12:04:01.298 L probe log line 42 {a: 1}                        app.js:6:11
 12:04:02.115 L v Object                                        app.js:9:11
                   id:   7
                   name: 'widget'
                   tags: v Array(3)
                           0: 'alpha'
                           1: 'beta'
                           2: 'gamma'
                   owner: > Object {...}          (Enter to fetch)
 12:04:03.401 L rendered in 12.4ms                              app.js:22:5

 >_ document.querySelector('#app').dataset
 1284/1284  FOLLOW           y yank  o open $EDITOR  Tab complete  Esc cancel
```

The `>` / `v` glyphs are the collapsed/expanded markers; `owner` is a handle not yet fetched, which is
the visible consequence of P5's lazy-RemoteObject reality rather than a UI affectation.

### 6.3 Wide, 146 columns (the measured Ghostty full-screen width, `tty.rs:239`)

```
-- CONSOLE --------------------------------- 1284 msgs   E 3   W 12   L 1269 ------------ / filter ------------- ^J eval --- ^G d close -----
 12:04:01.221  E  TypeError: Cannot read properties of null (reading 'boom')                                              app.js:8:12
                    at boot (app.js:8:12)  at onReady (app.js:31:7)  at HTMLDocument.<anonymous> (app.js:44:3)
 12:04:01.223  W  Deprecated: 'webkitStorageInfo' is obsolete. Please use 'navigator.storage' instead.                  vendor.js:41:3
 12:04:01.298  L  probe log line 42 {a: 1}                                                                                app.js:6:11
 12:04:02.115  L  > Object {id: 7, name: 'widget', tags: Array(3), owner: Object}                                         app.js:9:11
 12:04:02.980  N  GET http://127.0.0.1:8080/missing-404  404 (Not Found)                                                
 1281/1284  FOLLOW                                              j/k move   Enter expand   / filter   E/W/L cycle severity   g/G top/bottom
```

### 6.4 Design rules

**Severity gutter is one column, and it is a letter, not a colour.** `E` error, `W` warn, `L` log,
`I` info, `D` debug, `N` network (from `Log.entryAdded`), `>` REPL input, `<` REPL result. Colour is an
enhancement; the letter is the guarantee, for monochrome terminals and for colour-blind users — the
same reasoning D06 §7.2 uses for bracketing the active tab.

**Field priority, truncated to `cols` as the final step**, following D06 §6.2 exactly, because a
console row is subject to the identical overflow bug (D06 F2):

| Pri | Field | Dropped when |
|---|---|---|
| 1 | Severity gutter | never |
| 2 | Message text | never; middle-elided |
| 3 | Source `file:line:col` | `cols < 60` |
| 4 | Timestamp | `cols < 70` |
| 5 | Repeat counter `xN` | count == 1 |

**Every string from the page is untrusted.** Console messages are fully attacker-controlled, more
directly than titles or URLs, so they must pass through the same sanitizer the chrome row uses —
including D06 F4's bidi and zero-width extensions, and F3's display-width truncation. A page that logs
`\u{202E}` would otherwise reorder the pane. This is a real attack, not a theoretical one: it is the
cheapest way for a hostile page to forge a plausible-looking log line.

**Format specifiers must be handled.** P5b caught Electron's own `%c` message; pages use `%s`, `%d`,
`%o` and `%c` routinely. Apply substitution, and **discard `%c` styling entirely** rather than mapping
CSS colours to terminal colours — a page that can set console colours can forge our severity colours.

**Navigation is `less`/vim, not a GUI list.** `j`/`k` move, `g`/`G` jump to top/bottom, `/` filters
with a regex, `n`/`N` step matches, `F` toggles follow mode (the `tail -f` behaviour, on by default,
auto-disabled the moment the user scrolls up and re-enabled by `G`). `E`/`W`/`L` cycle the severity
floor. This is muscle memory the audience already has.

**Terminal-native affordances that a GUI console cannot offer:** `y` yanks the message via OSC 52
(D06 §8.4 already reserves `^G y` for URL yank, so this is consistent), and `o` opens
`file:line:col` in `$EDITOR`. The second closes the loop from "I saw an error" to "I am editing the
line" without leaving the terminal, and is the strongest argument for reading a console here rather
than in a GUI browser.

**The REPL is the pane's highest-value feature.** `^J` opens it. Evaluate with `Runtime.evaluate`
(`awaitPromise: true`, `replMode: true`), render results with the same RemoteObject renderer as log
rows. `Tab` completes from `Runtime.globalLexicalScopeNames` (present, P18) plus
`Runtime.getProperties` on the receiver. History persists per-origin and must **never** be written to
disk for `file://` or for origins the user has not visited again — console history routinely contains
tokens pasted for debugging.

---

## 7. Terminal UI — network pane

### 7.1 Split mode, 80 columns

```
-- NETWORK ---- 47 req  1.2 MB  3.41 s ---- /  -- [ ] cache -- ^G d close ----
 #  M    S   Type    Size    Time  Waterfall     Name
 1  GET 200  doc    12.4 kB  142ms |==_          /
 2  GET 200  js     86.1 kB  221ms |  ===__      /assets/app.f3a2.js
 3  GET 200  css     4.9 kB   38ms |  =_         /assets/app.9c1d.css
 4  GET 404  fetch    178 B    12ms |     _       /missing-404
 5  GET 200  xhr      526 B    31ms |      =_     /api/session
 3/47   sort:time                        Enter detail   b body   h headers
```

### 7.2 Full mode, 80 columns — one request opened

```
-- NETWORK -- request 5 of 47 -------------------------- Esc back  ^G d close
 GET /api/session                                            200 OK  31.4 ms

 TIMING          queue 1.2  stall 0.4  dns 0.0  tcp 0.0  ttfb 28.1  dl 1.7
                 |_ ___________________________________________ ==

 [h] REQUEST HEADERS  (9)
      accept            application/json
      cookie            <hidden: press ^G s to reveal>

 [H] RESPONSE HEADERS (7)
      content-type      application/json; charset=utf-8
      cache-control     no-store
      content-length    526

 [b] BODY  526 B  application/json                     (fetched on demand)
      {
        "hello": "world",
        "pad": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx..."
      }

 y yank as curl   Y yank body   r replay   Tab next section   Esc back
```

The body block reproduces the real fixture response measured in P7 (`len=526`,
`{"hello":"world","pad":"xxx…`). `(fetched on demand)` is literal: nothing was requested until the
user opened this row.

### 7.3 Wide, 146 columns

```
-- NETWORK ------------------- 47 req   1.2 MB transferred   3.41 s   DOMContentLoaded 0.9 s ------- / filter ---- [ ] disable cache -- ^G d ---
 #   Method  Status  Type    Size      Time    Waterfall                          Name
 1   GET     200     doc      12.4 kB   142ms  |==__                              /
 2   GET     200     js       86.1 kB   221ms  |   ====___                        /assets/app.f3a2.js
 3   GET     200     css       4.9 kB    38ms  |   =_                             /assets/app.9c1d.css
 4   GET     404     fetch      178 B     12ms  |        _                         /missing-404
 5   GET     200     xhr        526 B     31ms  |         =_                       /api/session
 6   POST    500     xhr        1.1 kB   918ms  |          ==========___           /api/checkout
 3/47   sort: time                                     Enter detail   b body   h headers   y yank as curl   r replay   x block URL   g/G top/bot
```

### 7.4 Design rules

**Column priority under narrowing**, same discipline as §6.4: `#`, status and name are never dropped;
the waterfall goes first (below ~70 cols), then Type, then Size, then Method. Name is elided
**middle-out with the leaf segment preserved** — `/assets/…/app.f3a2.js` — because the filename is the
identifying part, exactly D06 §5.6's reasoning for URLs.

**The waterfall is one dimensional and therefore terminal-friendly.** Two glyph classes: `_` for the
waiting phases (queue, stall, DNS, TCP, TTFB) and `=` for bytes actually transferring. That is the
distinction that answers the only question a waterfall is really asked — "is this slow because the
server is thinking, or because the response is big?" Half-block characters give a finer ramp where
available; `_` and `=` are the ASCII floor. Timing data is confirmed present on `responseReceived`
(P6).

**Size renders `encodedDataLength`, with `(cached)` for zero** (§5.4) — P8 measured a genuine `0` among
four requests, and rendering that as `0 B` would misreport a cache hit as an empty response.

**Status colouring is advisory; the number is the truth.** `2xx` default, `3xx` dim, `4xx`/`5xx` loud.
Never render a status as a symbol alone.

**`y` yanks the request as a `curl` command line.** This is the single most useful thing a network pane
can do for a terminal user, and it composes with the shell they are already in. It must yank the
sanitized URL and must **redact `Cookie` and `Authorization` by default**, with an explicit
`^G s` to include them — the same "loud when it matters" posture D06 §7.3 takes for the security badge.
`r` replays via `Network.replayXHR`, or by re-issuing through `Fetch` when that is unavailable
(**UNVERIFIED** — `Network.replayXHR` was not in the sweep).

**Filtering** is a regex over method, status, type and URL simultaneously, entered with `/`. Type
shortcuts (`x` XHR, `j` JS, `c` CSS, `i` images, `d` doc) are convenience aliases over the same filter,
following the same one-key-plus-leader pattern as D06 §8.4.

---

## 8. Pane layout, and how it meets D06

### 8.1 Two modes, chosen by measured cost

D06 §3.3 establishes the rule that governs this entire section: a **reserved row** reduces the page
viewport and therefore costs a full Chromium reflow plus a full-frame retransmit (~348 KiB wire,
A03 §0.1), while an **overlay** costs only its own bytes. A DevTools pane is large, so it cannot be an
overlay. But unlike find-in-page it is a *mode the user stays in*, so paying one reflow on entry and
one on exit is acceptable — the cost is amortised over the whole session rather than per keystroke.

That yields two modes:

| Mode | Layout | Frame cost | Default for |
|---|---|---|---|
| **Split** | page shrinks, pane occupies the bottom `N` rows | one reflow + full frame on toggle, then normal | Elements, Styles, Inspect, Responsive |
| **Full** | pane replaces the page entirely; engine `stopPainting()` | **zero frame bytes while open** | Console, Network |

Full mode is the terminal-native win and it should not be an afterthought. A developer reading a
console or a network log is not looking at the page; suspending painting drops the frame stream to
nothing, which on A03's 2 Mb hotel link is the difference between a usable pane and a slideshow. B04
already establishes that `stopPainting()`/`startPainting()` is the tab-switch mechanism and that
resuming emits a full frame at current geometry, so the machinery exists.

Defaults: **Full** when `rows < 30`, or when the link is known-slow (C09's adaptive tier), or for
Console and Network regardless. **Split** otherwise. `^G d` toggles the pane; `^G D` cycles
split/full.

Because a mode change is a viewport change, it obeys D06 §3.3's rule: **the pane's row count must not
change while it is open.** Resizing the pane is a deliberate act (`+`/`-`), not a side effect of
content.

### 8.2 Where the pane sits

Below the page and **above** the D06 chrome rows, so the chrome row remains the last row of the
terminal and D06's whole layout model is untouched:

```
   rows 1 .. H-C-P    PAGE            (kitty image; suppressed entirely in full mode)
   rows H-C-P+1 .. H-C  DEVTOOLS PANE (P rows)
   row H-1              tab strip     (wide layout only)
   row H                omnibox / status / find / command
```

This matters for a reason D06 already found the hard way: `PointerMap::to_page` has **no y-origin**
(D06 F5, `main.rs:712-727`) and rejects `py >= page_h`. Keeping the pane strictly below the page keeps
that guard correct, and clicks landing in the pane are rejected by the page mapper and handled by the
pane's own hit test — the same ordering D06 §7.2 requires for the tab strip, where the chrome click
handler runs *before* the page mapping.

### 8.3 Inspect overlay and responsive mode, at 80 columns

```
     +--------------------------------------------+
     :                                            :   <- margin  (dotted)
     : +----------------------------------------+ :
     : |                                        | |   <- border  (solid)
     : |   div#hero.banner                      | |
     : |                                        | |
     : +----------------------------------------+ :
     :                                            :
     +--------------------------------------------+
   div#hero.banner   640 x 220   margin 16   padding 24

-- ELEMENTS ------------------------------------------------ ^G d close ------
 <body>
   <header class='site'>
 v <div id='hero' class='banner'>          <- selected
     <h1>Ship it</h1>
   </div>
 STYLES  #hero  {display:flex; padding:24px; background:#0b0b0b}  app.css:41
         .banner {margin:16px 0}                                  app.css:88
 i inspect  Enter expand  e edit  c copy selector  s styles  Esc leave inspect
```

```
-- RESPONSIVE -- iPhone 15 Pro  390x844  dsf 2  touch  UA:iOS ---- ^G d close -

          +----------------------------------------------+
          |                                              |
          |            page rendered at 390x844          |
          |            scaled to fit 46 cols             |
          |                                              |
          +----------------------------------------------+

 w/W width -/+   h/H height   d device   r rotate   t touch   u UA   0 reset
 scale 0.398 (page has no viewport meta: layout 980 CSS px -> 390 device px)
```

The last row is not decoration. It is the measured 0.398 from §1.6 surfaced to the user, so that a page
rendering unexpectedly small reads as *"this page has no viewport meta, here is the scale"* rather than
as a BlackGlass bug. This is the kind of thing a terminal tool can explain in one row and a GUI browser
usually does not explain at all.

---

## 9. Keybindings

Constrained by D06 §8.1's hard rules, all of which hold here: never `Ctrl+A`/`Ctrl+B` (screen and tmux
prefixes), never `Ctrl+Tab`, never `Alt+arrow` as a primary, never `Ctrl+I/M/J/H/[` (C0 aliases that
break outside the kitty protocol), and every Tier-A-only chord needs a leader fallback.

Because D06's Tier 1 is already dense, **E09 adds exactly one new global chord** and puts everything
else behind the existing leader or inside the pane's own focus state.

| Key | Context | Action |
|---|---|---|
| `^G d` | global (leader) | Toggle the DevTools pane |
| `^G D` | global (leader) | Cycle split / full |
| `^G i` | global (leader) | Enter inspect mode |
| `^G D` (shift, gated) | global (leader) | Open the real DevTools window — refused with a message when there is no GUI session (§3.2) |
| `^J` | pane | Open the console REPL |
| `1`–`5` | pane, non-editing | Console / Network / Elements / Styles / Responsive |
| `j` `k` `g` `G` | pane | Move, top, bottom |
| `/` `n` `N` | pane | Filter, next match, previous match |
| `Enter` | pane | Expand row / open detail |
| `Esc` | pane | Collapse → leave inspect → close pane → forward to page |

`^G d` follows D06 §8.4's leader table, which already reserves `d` for "toggle `--stats`". **That is a
collision the commander must resolve**; `--stats` is developer telemetry and the DevTools pane is the
more valuable binding, so the recommendation is to move `--stats` to `^G ^D` or fold it into the
DevTools pane as a sixth tab.

The pane gets its own focus owner in D06 §7.1's model, with sigil `#`, so keystrokes are unambiguous:
Tier-1 chords stay intercepted (so `^Q` still quits from inside the pane, D06's invariant), and
everything else routes to the pane rather than the page.

---

## 10. Performance budget

| Item | Measured | Consequence |
|---|---|---|
| Hover-inspect round trip | 0.57 ms p50, 1.82 ms p100 (C1+C2) | Can track the cursor; coalesce to one in-flight request, no debounce needed |
| Styles refresh on selection | 2.93 ms p50 (C3) | Fetch on selection change, not on hover |
| Lazy DOM expand | 935 B / 2.4 ms (F2) | Default |
| Eager DOM tree | 102,616 B / 12 ms (F1) | Never on the interactive path; seconds of stall over SSH |
| AX tree | 313,467 B / 48 ms (F1) | Only for the MCP snapshot path (`packages/mcp/lib/snapshot.js`), never for Elements |
| Console arrival rate | 1,629 events/s (G1) | Ring buffer + 20 Hz render cap + `xN` collapse |
| Overlay draw | ~150 B, no reflow | vs ~348 KiB for the pixel path |
| Full-mode pane | zero frame bytes (`stopPainting`) | The reason Console/Network default to full |

The pane must obey D06 §9's chrome discipline: **track pane dirtiness separately from frame
dirtiness.** A console that repaints on every frame would emit ~8 KB/s on a static page for no reason
— irrelevant locally, 3.5% of a 2 Mb link over SSH.

---

## 11. Security

DevTools panes handle the most sensitive data in the product, and two of the risks are specific to
rendering it in a terminal.

| Risk | Mechanism | Mitigation |
|---|---|---|
| **Terminal injection via console text** | Console messages are fully page-controlled | Route every message through the chrome sanitizer *plus* D06 F4's bidi/zero-width extensions and F3's display-width truncation. Higher priority than for titles, because the volume and attacker control are both greater. |
| **Forged log lines** | A page can `console.error` anything, including text imitating BlackGlass's own UI | Severity glyph comes from the CDP event `type`, never from message content. Never let `%c` styling reach the terminal. |
| **Cookie exposure in the network pane** | `*ExtraInfo` events carry raw `Set-Cookie` / `Cookie` (E01 §7.4) | Redact by default in the headers view and in `y`-as-curl; explicit `^G s` to reveal, and never include them in a yank without that step. |
| **Response bodies containing secrets** | `getResponseBody` returns tokens verbatim | Fetch only on explicit open (P7 is lazy by design); never log bodies to disk; never include them in crash reports (B08). |
| **REPL history leaking credentials** | Users paste tokens into consoles constantly | Per-origin, memory-only by default; opt-in persistence with `0600` and `safeStorage`, matching E01 §6.2's posture. |
| **`$EDITOR` invocation from page-controlled data** | `o` opens `file:line:col` from a stack frame | Treat the URL as untrusted: resolve only `http(s)`/`file` paths that map into a user-configured source root; never pass through a shell. |
| **The pane is a second CDP consumer** | A DevTools window can change overrides under us (§3.2) | Re-read `Page.getLayoutMetrics` before drawing; show a `DT` token in chrome. |

None of this requires the E01 broker or an automation grant (§2), so the DevTools panes add **no**
new OS-level attack surface: no socket, no port, no descriptor.

---

## 12. Test plan

All CI-able from protocol responses and log evidence, per the project's screenshot-free approach.

| ID | Test | Pass |
|---|---|---|
| E09-T1 | Regression for **P10**: highlight a node, capture frames for 500 ms | Zero paint events attributable to the overlay. If this ever changes, the terminal overlay becomes redundant and we should know. |
| E09-T2 | Regression for **F1**: emulate 390x844 `mobile:true` on a no-viewport-meta fixture; compare the drawn overlay rect against a red-box pixel scan | Overlay within ±2 px of the measured surface box. Fails today if the transform is skipped. |
| E09-T3 | Regression for **F2**: enter responsive mode, `SIGWINCH`, assert frame size | Frame returns to the emulated size, `innerWidth` unchanged |
| E09-T4 | Regression for **F5**: issue every domain enable pre-navigation with a 3 s timeout | Startup completes; no hang |
| E09-T5 | Regression for **F4**: load a blank page, read the console pane | Zero Electron security-warning rows |
| E09-T6 | Console merge: fixture emitting `console.*`, an uncaught throw, and a 404 | Exactly one row each; no duplicates (locks P3) |
| E09-T7 | Backpressure: 5,000 `console.log` in a tight loop | Pane stays responsive, ring buffer caps at its bound, render rate ≤ 20 Hz, no unbounded memory |
| E09-T8 | Network row assembly: fixture with a 200, a 404, and a cached asset | Statuses correct; `encodedDataLength: 0` renders `(cached)` |
| E09-T9 | Lazy body: open the pane, assert no `getResponseBody` sent; open one row, assert exactly one | Locks the laziness in |
| E09-T10 | Sanitizer: page logs `\u{202E}`, `\u{200B}`, `\x1b]0;pwn\x07`, 400 CJK chars | No pane line exceeds `cols`; no escape reaches the tty; no reordering |
| E09-T11 | Yank-as-curl on a request with cookies | `Cookie` redacted unless `^G s` was pressed |
| E09-T12 | Method inventory (regression for **P18/F6**) | `CSS.setStyleTexts` present; build fails loudly if `CSS.setPropertyText` ever reappears and the code silently prefers it |
| E09-T13 | Mockup golden file | `mock.py` exits 0: no line exceeds its width, all printable ASCII |
| E09-T14 | Full-mode frame suppression | Zero `T_FRAME` messages while a full-mode pane is open |

`mock.py` should be promoted into `tests/` as a golden-file check, as D06 §11.3 recommends for its own
mockups, so §6–§8 cannot silently drift from the implementation.

---

## 13. Changes required in files I do not own

Described, not made, per the ownership rule.

**`apps/engine/src/main.js`**

1. Attach `webContents.debugger` per tab and expose a `cdp` command type that forwards
   `{method, params, sessionId}` and returns the reply — the engine is a *pipe* here and must contain
   no DevTools policy.
2. Forward `Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, `Log.entryAdded`, and the
   `Network.*` events as `T_EVENT` messages. Consider a distinct type for high-rate CDP events so the
   Rust side can apply the ring-buffer discipline (§5.3) without parsing everything as chrome events.
3. **F5:** issue domain enables only after the first navigation, and never `await` them without a
   timeout.
4. **F2:** in the `resize` handler (`main.js:231-238`), re-apply
   `Emulation.setDeviceMetricsOverride` after `setSize` when an override is active — or skip the
   `setSize` entirely while in responsive mode.
5. **F4:** filter the Electron security warning, or set `ELECTRON_DISABLE_SECURITY_WARNINGS`.
6. A `devtools:open|close` command wrapping `openDevTools({mode:'detach'})`, gated per §3.2. P19
   confirms it is safe alongside our attach.
7. The existing hardening (`sandbox: true`, `contextIsolation: true`, `nodeIntegration: false`,
   `setWindowOpenHandler` denying popups, `main.js:101-134`) must not change.

**`apps/cli/` (Rust core)**

8. New module `bg-devtools`: the pane state machine, ring buffer, RemoteObject renderer, network row
   assembler, and the CSS-px → surface-px transform from **F1**.
9. **F3:** rebuild `PointerMap` from the frame header's `width`/`height` rather than the last `resize`
   command, so emulated geometry is respected.
10. Pane layout per §8: reserved rows below the page and above the D06 chrome; `stopPainting()` in full
    mode.
11. Resolve the `^G d` collision with D06 §8.4's `--stats` binding (§9).
12. Extend the sanitizer application to pane rows (depends on D06 F3/F4 landing).

**`crates/bg-proto`**

13. Message types for CDP request/reply and for the high-rate event stream, with a sequence number so
    the pane can detect drops rather than silently mis-ordering.

**Repo-wide**

14. **F7:** the CI check that no fixture ever uses `--remote-debugging-port` (E01 T-CDP-1). Still
    warranted — the offending process is still running.

---

## 14. UNVERIFIED and open questions

| # | Item | Why it matters | How to close |
|---|---|---|---|
| U1 | Cross-origin iframes (OOPIFs) over `webContents.debugger` | Elements and Console will have gaps on sites with third-party iframes | Fixture with a cross-origin iframe; `Target.setAutoAttach {flatten:true}`; assert child-frame nodes and console messages arrive |
| U2 | `Network.replayXHR` presence | §7.4 promises `r` replay | Add to the method sweep |
| U3 | Overlay behaviour under a page with a scrolled viewport | `visualViewport.pageX/pageY` were 0 in every measured case, so the offset half of F1's transform is **untested** | Scroll the fixture, re-run the probe4 red-box comparison |
| U4 | `deviceScaleFactor > 1` and OSR frame size | Measured `dsf=2` kept the frame at 390x844 with `dpr` reporting 2 — so the surface did **not** double. Whether that is correct or an OSR limitation is unresolved, and it affects retina-accuracy claims | Compare a `dsf=2` OSR frame against a `dsf=2` `captureScreenshot` |
| U5 | Whether a DevTools window and our attach can both set conflicting overrides safely | §3.2 assumes last-writer-wins | Open both, set different emulation from each, observe |
| U6 | Console rendering of `console.table`, `console.group`, `console.trace` | Only `log/warn/error` were exercised | Extend the fixture |
| U7 | Half-block-tier legibility of the bounds overlay | §5.2 asserts box-drawing renders well at ~17x18 px per cell; not measured | Render at the fallback tier and inspect |
| U8 | `$EDITOR` integration on a locked/remote session | §6.4's `o` binding | Not testable here (lock screen) |
| U9 | Performance of the pane under tmux with `allow-passthrough` | D06 §8.6's requirement applies to the pane too | Run under tmux |

Also unverified by construction: **iTerm2 3.6.9**, blocked by macOS TCC per the mission brief, and any
**Linux** behaviour — no Linux host was available.

---

## 15. Licensing

Nothing in this document derives from third-party source. CDP method semantics were established by
**running** the protocol against a vendored Electron and observing replies — black-box measurement, not
code reading. Electron is MIT (`apps/engine/node_modules/electron/package.json:"license":"MIT"`, with
`dist/LICENSE` present). The pane designs are original; they take inspiration from `less`, vim and tmux
conventions, which are behavioural idioms rather than copyrightable expression. `packages/mcp/lib/snapshot.js`
already documents the correct posture toward Playwright MCP (Apache-2.0, no code used); §5.6 reuses that
file's *lazy-geometry* idea, which is BlackGlass's own.

**The repo still has no top-level `LICENSE` file.** E01 §11 flagged this; it remains unresolved and
should be settled before any third-party code lands.

---

## 16. References

Measured this mission (scratchpad `e09/`): `probe2.js` → `results.txt` (P0–P19), `probe3.js` →
`results3.txt` (A1–A2, B1–B2, C1–C3, D1, E1–E2, F1–F2, G1), `probe4.js` → `results4.txt` (CSS-vs-surface
scale matrix, R1), `probe5.js` → `results5.txt` (`Page.getLayoutMetrics`), `mock.py` → `mock.out`
(width assertions).

Repo sources read: `apps/engine/src/main.js` (framing 52-99, window hardening 101-134, resize 231-238),
`apps/cli/src/main.rs:701-727` (`PointerMap`), `packages/mcp/lib/cdp.js` (flattened-session client),
`packages/mcp/lib/snapshot.js` (AX snapshot, lazy geometry, epoch guards).

Sibling artifacts: **E01** §3.2 (`webContents.debugger` for first-party verbs), §7.4 (egress cookie
filtering), §2.2 and §11 (the open port, the missing LICENSE); **D06** §3.3 (reserved rows vs
overlays), §6.2 (field priority), §7.1 (focus model), §8 (keybindings), F2/F3/F4/F5 (overflow, display
width, bidi, `PointerMap` origin); **A03** §0.1 (bandwidth tiers, 348 KiB full frame); **B04** (per-tab
painting, `stopPainting`); **B08** (crash recovery); **C09** (adaptive/slow-link tier);
**A09** §4.4 (token and scope posture).
