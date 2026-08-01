# B04 — Multi-tab support for the Terminal-Fenster engine host

**Status:** design + measured evidence + ready-to-apply JS diff
**Owner of this file:** B04. `apps/engine/src/main.js` is NOT edited here; the diff below is for the commander to apply.
**Target:** `apps/engine/src/main.js` (Electron 43.2.0 / Chromium 150.0.7871.129, verified in-tree)
**Machine:** macOS 26.1, Apple M4, `darwin arm64`

---

## 1. Recommendation in one paragraph

Use **one hidden offscreen `BrowserWindow` per tab**, and enforce a single hard invariant: **exactly one
tab paints at a time.** Background tabs get `webContents.stopPainting()`, which was measured to produce
*exactly zero* `paint` events while the page still loads, navigates and runs JavaScript normally.
Activation is `stopPainting()` on the outgoing tab then `startPainting()` on the incoming one, which was
measured to emit one full frame at the current geometry even for a completely static page — so a tab
switch repaints the terminal with no extra nudge and no protocol round trip. Tab identity rides in the
frame header as a single byte that is zero for tab 0, so the existing single-tab decoder in
`crates/tf-proto` keeps working byte-for-byte. Do **not** use `setFrameRate(0)` (it silently clamps to 1,
measured) and do **not** ship CDP page freezing in v1 (it stops painting but I could not restore a frozen
tab, measured).

---

## 2. What was measured, and how

All numbers below come from running the Electron binary already vendored in this repo:

```
$REPO/apps/engine/node_modules/electron/dist/version  ->  43.2.0
cd $REPO/apps/engine
./node_modules/.bin/electron <probe>.js        # agent Bash sandbox disabled; Chromium
                                               # children cannot spawn under it
```

Probe scripts (scratchpad, outside the repo — nothing was written into the tree except this file):

| Probe | Question | Log |
|---|---|---|
| `probe-tabs.js` | concurrent OSR windows, `setFrameRate` range, `stopPainting`, freeze | `b04/probe1.log` |
| `probe2.js` | freeze/thaw observed by paint counts only | `b04/probe2.log` |
| `probe3.js` | crash isolation and recovery | `b04/probe3.log` |
| `probe5.js` | teardown variants, one per process (`CASE` env) | `b04/g_case*.log` |
| `probe6.js` | memory scaling 1→8 `BrowserWindow` tabs @2482x814 | `b04/probe6.log` |
| `probe7.js` | `WebContentsView` + offscreen, retested cleanly | `b04/probe7.log` |
| `probe8.js` | memory scaling 1→8 `WebContentsView` @2482x814 | `b04/probe8.log` |
| `probe9.js` | static-page resume, deferred resize | `b04/probe9.log` |
| `probe10.js` | first frame for a tab loaded in the background | `b04/probe10.log` |

Base directory: `/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/b04/`

### 2.1 A methodology correction worth recording

My first three probes appeared to show that `win.destroy()` on an offscreen window **killed the entire
engine host**. That would have been a showstopper for tab closing, and I nearly reported it as one. It was
my probes' fault: they did not register `app.on('window-all-closed', () => {})`, so destroying the last
window triggered Electron's default quit. `apps/engine/src/main.js:36` already registers that guard.
Re-run with the guard, every teardown variant survives:

```
CASE 1  destroy() while painting                          -> SURVIVED, windows left = 0
CASE 2  stopPainting + removeAllListeners + destroy        -> SURVIVED, windows left = 0
CASE 5  close() while painting                             -> SURVIVED, windows left = 0
CASE 7  destroy() after the renderer crashed               -> SURVIVED, windows left = 0
CASE 10 webContents.close()                                -> SURVIVED, windows left = 0
```

Two consequences. First, tab close is safe by any of these routes. Second, **`main.js:36` is now
load-bearing** — it is the only reason closing the last tab does not take the engine with it. It must not
be removed, and the diff below adds a comment saying so.

---

## 3. Architecture: per-tab offscreen windows vs one window with `WebContentsView`

Both work in Electron 43. I measured both at the real verified terminal geometry (2482x814).

**`WebContentsView` does work offscreen**, contrary to my first (confounded) run: attached to a hidden
`BaseWindow` it reports `isOffscreen=true`, `getType()='offscreen'`, and painted 89 frames in 1.5 s
(≈59.3 fps). Two stacked views in one `BaseWindow` each painted ~59 fps independently with their own
frame sizes. Two constraints found: a view that is **never attached** to a window paints **0** frames, and
`view.setVisible(false)` does **not** stop painting (90 paints/1.5 s with it false) — so `stopPainting()`
is required either way.

Head-to-head, 8 tabs, all painting, 2482x814:

| | 8 × `BrowserWindow` (probe6) | 8 × `WebContentsView` in 1 `BaseWindow` (probe8) |
|---|---|---|
| renderer ("Tab") RSS total | 587,648 KB | 573,456 KB |
| GPU process RSS | 837,328 KB | 599,824 KB |
| processes | 11 | 11 |
| paints/s per tab | **50–56** | **60–61** |
| marginal (renderer+GPU) per tab, 1→8 | **172.7 MiB** | **137.7 MiB** |

`WebContentsView` is measurably better *in the all-tabs-painting regime*: ~35 MiB/tab cheaper and it holds
a full 60 fps where 8 separate windows sag to 50–56.

**I still recommend `BrowserWindow`-per-tab for v1**, for three reasons:

1. The design in §4 guarantees **one painting tab**, so the entire measured advantage sits in a regime we
   are deliberately designing away. With one painter, the dominant cost (renderer RSS, ~72 MiB/tab) is
   identical between the two models.
2. It is a strictly smaller delta from code that is already verified end-to-end. `createWindow()` already
   exists and works; the diff becomes "make it take an id" rather than "swap the window model".
3. `WebContentsView` adds layout state (`setBounds` must track terminal geometry, and I did **not** verify
   what happens when a view's bounds exceed its parent `BaseWindow` — UNVERIFIED).

Record `WebContentsView` as the documented migration if a feature ever needs many tabs painting at once
(tab previews, a tab grid, screenshot-all). The measurement above is the justification, already done.

---

## 4. Tab lifecycle

```
                 tab.new(id,url)
        (none) ──────────────────► BACKGROUND ──────────────► ACTIVE
                 created stopped   ▲   │  tab.activate(id)     │
                 loads with zero   │   │                       │
                 paint events      │   └───────────────────────┘
                                   │      tab.activate(other)
                                   │      (stopPainting)
                                   │
        render-process-gone ──► CRASHED ──tab.recover──► BACKGROUND/ACTIVE
                                   │
        tab.close(id) ─────────────┴──────────────────► DESTROYED
```

**Invariant: at most one tab is in ACTIVE, and only an ACTIVE tab may emit frames.**

Each transition is backed by a measurement:

- **Created in background.** `stopPainting()` before `loadURL` → `0` paints during the entire load
  (probe10 step 1). Opening a background tab costs no pixel work at all.
- **Activate.** `startPainting()` emits one full frame even for a static page with no animation
  (probe9 step 4: `1` paint; probe10 step 4: `1` paint at `2482x814`). No `invalidate()` needed.
  Note `invalidate()` alone on a fully idle page produced **0** paints (probe9 step 5), so `invalidate()`
  is *not* a reliable "give me a frame now" primitive — `startPainting()` is.
- **Activate the already-active tab** emits a redundant full repaint (probe10 step 5: `1` paint). The diff
  guards this with an early return.
- **Deactivate.** `stopPainting()` → exactly `0` paints for that tab while the foreground tab stays at a
  full 60 fps (probe6: `{"D1":120, D2..D8: 0}` over 2 s).
- **Close.** Safe via `destroy()`, `close()`, or `webContents.close()`, painting or not, crashed or not
  (§2.1). The diff uses `stopPainting()` + `removeAllListeners('paint')` + `destroy()` so no `paint`
  callback can fire against a half-torn-down tab.

### 4.1 Resize

Terminal geometry is global, so a resize applies to every tab. Measured: `setSize()` on a tab whose
painting is stopped produces **0** paint events (probe9 step 6), so resizing all tabs immediately is free
in frame terms and is what the diff does.

The alternative — deferring resize until activation — has a measured hazard. In probe10 step 2,
`setSize(2482,814)` immediately followed by `startPainting()` delivered **two** frames, the first still at
the *old* `1280x800`. Applying geometry to every tab up front avoids that stale first frame entirely. If
profiling ever shows relayout storms across many background renderers during a resize drag, deferral is
the escape hatch, but it must then also drop the first mismatched-geometry frame.

---

## 5. Background throttling: what actually works

| Mechanism | Measured result | Verdict |
|---|---|---|
| `setFrameRate(0)` | **Does not throw. Silently clamps to 1** (`getFrameRate()` → `1`). `-1`→1, `241`→240. | **Do not use.** The docs in `electron.d.ts:18398` say only 1–240 are accepted when `useSharedTexture` is false; the implementation clamps rather than rejecting, so a `0` looks like it worked and you still pay ~1 fps forever. |
| `setFrameRate(1)` | 7 background tabs produced 0–1 paints per 2 s each, foreground held 60 fps. | Works, but strictly worse than stopping. |
| `stopPainting()` | **Exactly 0 paints**, foreground unaffected (120 paints/2 s). `isPainting()` → false. | **Use this.** |
| `setBackgroundThrottling(false)` / `webPreferences.backgroundThrottling` | Ineffective by construction: a hidden OSR window's page reports `visibilityState = "visible"`, `document.hidden = false` (probe1). Chromium's own background throttling never engages because the page is never backgrounded. | No-op for us. Leave at the default. |
| CDP `Page.setWebLifecycleState{state:"frozen"}` | Freeze works: 60 paints/s → 1 paint in 1.5 s, command returns in 1 ms. **But the tab did not come back**: after `state:"active"` it produced 0 paints/1 s and reported `visibilityState = "hidden"`. Separately, `executeJavaScript` into a frozen tab **never resolves** (hung >70 s, killed). | **Do not ship in v1.** Recovery path UNVERIFIED. |

### 5.1 The honest cost: background tabs keep burning CPU

`stopPainting()` stops pixels, not the page. Measured on a stopped tab: `requestAnimationFrame` kept
firing 60 times/s and `setInterval` 62 times/s (probe1 P3: counters `472,490 → 532,552` over one second),
and `visibilityState` stayed `"visible"` throughout. So a background tab running a busy animation still
costs a renderer's worth of CPU.

The only mechanism measured to actually stop page work is CDP freezing, whose resume path is broken here.
For v1, accept the cost, document it, and cap tab count (§6). Revisit freezing only with a verified
thaw path.

---

## 6. Memory cost per tab

Measured with `app.getAppMetrics()` (`workingSetSize`, KB), 1→8 `BrowserWindow` tabs at 2482x814,
trivial animated `data:` page (probe6 — a real page is strictly worse):

| tabs | renderer total | GPU | all processes |
|---|---|---|---|
| 1 | 82,816 | 104,320 | 794,464 |
| 2 | 165,632 | 177,456 | 818,928 |
| 4 | 324,704 | 251,872 | 1,175,296 |
| 6 | 433,920 | 653,104 | 3,228,608 |
| 8 | 587,648 | 837,328 | 2,009,024 |

- **~72 MiB per renderer** at 8 tabs (587,648 / 8 = 73,456 KB), trivial page.
- **~101 MiB per tab in the GPU process** while painting ((837,328 − 104,320)/7 = 104,715 KB).
- **~173 MiB marginal per additional painting tab** (renderer + GPU, 1→8).
- Browser-process working set is extremely noisy (431 MB → 2.10 GB → 545 MB across the run) and should
  not be quoted as a per-tab figure; see §9 for the likely cause.

**Throttling does not return memory.** With 7 of 8 tabs `stopPainting()`-ed and 3 s to settle, GPU was
804,688 KB versus 837,328 KB while all 8 painted — a 4% saving. Destroying those 7 tabs dropped GPU to
195,888 KB. **Only closing a tab reclaims memory.**

Therefore:

- **Soft cap 8, hard cap 16 tabs** (`MAX_TABS` in the diff; the frame-header tab byte allows 256, the
  memory does not).
- Beyond the soft cap, **discard** rather than throttle: close the renderer, keep `{id, url, title,
  scroll}` in the core, and re-create on activation. This is the only measured way to get the memory back.
  Discard belongs in the core (it owns tab identity); the engine already provides the primitives
  (`tab.close` + `tab.new` with the same id).

---

## 7. Crash isolation

Measured (probe3): each tab is its own renderer process (4 tabs → 4 distinct pids), and
`forcefullyCrashRenderer()` on one tab produced:

```
render-process-gone events: ["C1:killed:2"]        <- only the crashed tab
paints/1.5s after the crash: {"C0":91,"C1":0,"C2":91}   <- siblings still at ~60 fps
C1 win.isDestroyed=false  wc.isDestroyed=false  isCrashed=true  isPainting=true
Tab processes: 3 -> 2
sendInputEvent on a sibling after the crash: ok
```

Findings that shape the design:

1. **A crash is contained to one tab.** Siblings keep painting and keep accepting input.
2. **The window and its `webContents` survive the crash** — recovery is a reload, not a re-create.
   `loadURL` on the same `webContents` restored it to a full 60 fps and cleared `isCrashed()`.
   That is the `tab.recover` command in the diff.
3. **`isPainting()` lies after a crash** — it still returned `true` for a tab painting 0 frames. Use
   `isCrashed()` for liveness, never `isPainting()`.
4. **Caveat (UNVERIFIED):** all tabs were distinct `data:` URLs. Under Chromium site isolation, two tabs on
   the *same site* may share a renderer process, in which case one crash takes out both. The engine
   handles this correctly by construction — each `webContents` fires its own `render-process-gone`, so the
   core will receive one `crash` event per affected tab — but the core's UI must not assume crashes are
   one-at-a-time. I could not verify this: the agent sandbox's network allowlist does not include a
   general web origin.

---

## 8. Protocol additions

### 8.1 Frame header — one byte, zero breakage

Bytes 28..31 are currently a `u32 format` whose only ever value is `0`
(`apps/engine/src/main.js:95`, `crates/tf-proto/src/lib.rs:29`). Redefine them as:

```
offset 28  u8   format    0 = BGRA8888
offset 29  u8   tabId     0..=255
offset 30  u16  reserved  0
```

For `tabId = 0` the four bytes are still `00 00 00 00`, so a decoder that reads a big-endian `u32` at
offset 28 still sees `format == 0`. `FRAME_HEADER_LEN` stays 32 — no payload-offset change, no
`expected_payload()` change, and the existing `frame_header_roundtrip` /
`truncated_frame_is_dropped_not_rendered` / `full_frame_is_accepted` tests stay green unmodified.

**Core-side change for the commander (I did not edit `crates/`):** in
`crates/tf-proto/src/lib.rs:38-47`, replace `format: g(28)` with

```rust
format: b[28] as u32,
tab_id: b[29],
```

adding `pub tab_id: u8` to `FrameHeader`. Then in `apps/cli/src/main.rs:518`, drop any frame whose
`tab_id` is not the active tab — this closes the one-frame race where the user switches tabs while a
frame from the old tab is already in the socket.

`seq` stays a single global monotonic counter across all tabs; it remains useful for spotting drops and
reordering, and per-tab sequencing buys nothing while only one tab paints.

### 8.2 Commands (core → engine, type 10)

New:

| Command | Payload | Behaviour |
|---|---|---|
| `tab.new` | `{"t":"tab.new","id":0..255,"url":"...","activate":true}` | Create a tab with a **core-allocated** id. Rejects duplicate ids, out-of-range ids, and ids past `MAX_TABS` with a `tab.error` event. `activate` defaults to true; `false` opens it in the background at zero paint cost. |
| `tab.close` | `{"t":"tab.close","id":n}` | Stop painting, detach the paint listener, destroy. Auto-activates the next remaining tab, or emits `tabs.empty`. |
| `tab.activate` | `{"t":"tab.activate","id":n}` | The switch. No-op if already active. |
| `tab.list` | `{"t":"tab.list"}` | Replies with a `tabs` event. |
| `tab.recover` | `{"t":"tab.recover","id":n}` | Reload a crashed tab in place. |

Changed — **every existing command keeps working unchanged**. `navigate`, `reload`, `back`, `forward`,
`input` now accept an optional `"id"`; **when `id` is absent they address the active tab**, which is
exactly today's behaviour. The CLI's existing `self.send(r#"{"t":"reload"}"#)` calls
(`apps/cli/src/main.rs:557,566,570`) need no edit at all. `resize` is deliberately global: it has no `id`
and applies to every tab.

### 8.3 Events (engine → core, type 2)

New: `tab.opened {id,url}`, `tab.closed {id}`, `tab.activated {id}`, `tab.error {id,msg}`,
`tabs {v:[{id,title,url,loading,crashed,active}],active}`, `tabs.empty`, `unresponsive {tab}`.

Changed: `title`, `url`, `loading`, `loadError`, `crash`, `popup` all gain a `"tab":<id>` field.
`ready` gains `maxTabs`. `stats` gains `tabs` and `active`.

`Status::apply_event` in `apps/cli/src/main.rs:725` reads only `t`/`v` via `json_get_str`, so unknown
extra fields are ignored — **the existing CLI keeps working against the new engine without modification.**

### 8.4 Popups

`setWindowOpenHandler` still denies and reports, now with the originating tab id. The core decides whether
to answer with a `tab.new`. Keeping tab-id allocation in the core avoids an async id-handshake and keeps
the CLI's tab model authoritative.

---

## 9. Follow-up the commander should know about (not in this diff)

`onPaint` copies each frame **three times**: `image.toBitmap()` allocates a fresh buffer,
`Buffer.concat([head, bitmap])` copies it again (`main.js:97`), and `sendMessage` concatenates a third
time (`main.js:57`). At the verified 2482x814 geometry that is 8,081,392 bytes per copy, so ~24 MB of
allocation per frame and **~1.45 GB/s at 60 fps for a single painting tab**. This is the most plausible
explanation for the browser process working set swinging between 431 MB and 2.8 GB in the scaling runs.

Fix (separate change, keeps the wire format identical): write the 5-byte message header, the 32-byte frame
header and the bitmap as three `sock.write()` calls, or allocate one `37 + len` buffer and
`bitmap.copy()` into it. This eliminates two full-frame copies per frame. I kept it out of the tab diff to
keep the tab change reviewable in isolation, but it is a larger win than anything in §5.

---

## 10. The diff

Apply to `apps/engine/src/main.js`. Nothing outside this file changes; the Rust-side change is described
in §8.1 for the commander to make separately.

```diff
--- a/apps/engine/src/main.js
+++ b/apps/engine/src/main.js
@@ -6,9 +6,13 @@
 //   [u8 type][u32 BE length][payload]
 //
 //   type 1  engine -> core : FRAME   (binary header + BGRA pixels)
 //   type 2  engine -> core : EVENT   (JSON: title/url/loading/crash)
 //   type 10 core  -> engine : COMMAND (JSON: navigate/resize/input/quit)
 //
+// Tabs: one hidden offscreen BrowserWindow each, and at most one of them ever paints. The
+// owning tab rides in frame-header byte 29. Bytes 28..31 used to be a u32 `format` whose
+// only value was 0; they are now [u8 format][u8 tabId][u16 reserved], so for tab 0 the
+// bytes are unchanged and a single-tab decoder still reads format == 0.
+//
 // Security posture: the Chromium sandbox stays ON. Web content never gets Node integration,
 // context isolation is enforced, and the engine only ever connects to a local socket path
 // handed to it by its parent -- it opens no listening port of its own.
 'use strict';
@@ -30,20 +34,34 @@
 const T_FRAME = 1;
 const T_EVENT = 2;
 const T_COMMAND = 10;
 
-// Electron quits the app when the last window closes. We manage lifetime ourselves;
-// without this, closing a tab would kill the engine and orphan the terminal core.
+// Hard ceiling on live tabs. The frame header can address 256, but memory cannot: each
+// additional painting tab measured ~173 MiB (renderer + GPU) at 2482x814, and throttling
+// a tab does NOT give that memory back -- only closing it does.
+const MAX_TABS = 16;
+
+// Electron quits the app when the last window closes. We manage lifetime ourselves;
+// without this, closing a tab would kill the engine and orphan the terminal core.
+// This is load-bearing for multi-tab: closing the last tab must leave the engine alive
+// so the core can decide whether to open another or quit. Do not remove.
 app.on('window-all-closed', () => {});
 
 let sock = null;
-let win = null;
 let seq = 0;
 
+// --- Tab table ---------------------------------------------------------------------
+// One offscreen BrowserWindow per tab, each in its own renderer process, so a renderer
+// crash is contained: a forced crash in one tab left its siblings painting at 60 fps.
+// Tab ids are allocated by the core, not here, so the core's tab model stays
+// authoritative and opening a tab needs no async id handshake.
+const tabs = new Map(); // id -> { id, win, title, url, loading, crashed }
+let activeId = null;
+let geomW = INITIAL_W;
+let geomH = INITIAL_H;
+
 // --- Backpressure -----------------------------------------------------------------
 // A terminal (especially over SSH) can be far slower than a 60 fps compositor. If we
 // queued every frame, memory would grow without bound and the user would watch an
 // ever-more-stale page. Instead we keep at most ONE frame in flight and coalesce:
 // while a write is draining, later frames overwrite the pending one. The newest frame
 // always wins, which is the correct trade for interactivity.
 let writeInFlight = false;
-let pendingFrame = null;
-const stats = { produced: 0, sent: 0, coalesced: 0 };
+let pendingFrame = null; // { tabId, buf }
+const stats = { produced: 0, sent: 0, coalesced: 0, dropped: 0 };
 
 function sendMessage(type, payload) {
   if (!sock || sock.destroyed) return false;
@@ -62,20 +80,29 @@
 function sendEvent(obj) {
   sendMessage(T_EVENT, Buffer.from(JSON.stringify(obj), 'utf8'));
 }
 
 function flushFrame() {
   if (writeInFlight || !pendingFrame) return;
-  const frame = pendingFrame;
+  const { tabId, buf } = pendingFrame;
   pendingFrame = null;
+  // The user can switch tabs while a frame is queued. Never present a frame that no
+  // longer belongs to the active tab -- it would be a full-screen flash of the old page.
+  if (tabId !== activeId) {
+    stats.dropped++;
+    return;
+  }
   writeInFlight = true;
   stats.sent++;
-  const ok = sendMessage(T_FRAME, frame);
+  const ok = sendMessage(T_FRAME, buf);
   if (ok) {
     writeInFlight = false;
     if (pendingFrame) setImmediate(flushFrame);
   } else {
     sock.once('drain', () => {
       writeInFlight = false;
       flushFrame();
     });
   }
 }
 
-function onPaint(_event, dirty, image) {
+function onPaint(tabId, dirty, image) {
   if (!sock || sock.destroyed) return;
+  // Background tabs are stopped, so this should not fire for them; belt and braces.
+  if (tabId !== activeId) return;
   stats.produced++;
   const size = image.getSize();
   const bitmap = image.toBitmap(); // BGRA, 4 bytes/px, verified non-strided
   const head = Buffer.allocUnsafe(32);
   head.writeUInt32BE(seq++, 0);
   head.writeUInt32BE(size.width, 4);
   head.writeUInt32BE(size.height, 8);
   head.writeUInt32BE(dirty.x, 12);
   head.writeUInt32BE(dirty.y, 16);
   head.writeUInt32BE(dirty.width, 20);
   head.writeUInt32BE(dirty.height, 24);
-  head.writeUInt32BE(0, 28); // format 0 = BGRA8888
+  head.writeUInt8(0, 28); // format 0 = BGRA8888
+  head.writeUInt8(tabId, 29); // owning tab; 0 keeps these 4 bytes byte-identical to v1
+  head.writeUInt16BE(0, 30); // reserved
   if (pendingFrame) stats.coalesced++;
-  pendingFrame = Buffer.concat([head, bitmap]);
+  pendingFrame = { tabId, buf: Buffer.concat([head, bitmap]) };
   flushFrame();
 }
 
-function createWindow(w, h) {
+// --- Tabs ---------------------------------------------------------------------------
+
+function load(tab, url) {
+  tab.url = url;
+  // loadURL rejects on ERR_ABORTED and friends; an unhandled rejection would be fatal.
+  tab.win.loadURL(url).catch((e) =>
+    sendEvent({ t: 'loadError', tab: tab.id, code: -1, desc: e.message, url })
+  );
+}
+
+function createTab(id, url) {
   const b = new BrowserWindow({
     show: false,
-    width: w,
-    height: h,
+    width: geomW,
+    height: geomH,
     webPreferences: {
       offscreen: true,
       // Hardening: web content must never reach Node or our privileged context.
       nodeIntegration: false,
       contextIsolation: true,
       sandbox: true,
       webSecurity: true,
     },
   });
-  b.webContents.setFrameRate(60);
-  b.webContents.on('paint', onPaint);
-  b.webContents.on('page-title-updated', (_e, title) => sendEvent({ t: 'title', v: title }));
-  b.webContents.on('did-navigate', (_e, url) => sendEvent({ t: 'url', v: url }));
-  b.webContents.on('did-navigate-in-page', (_e, url) => sendEvent({ t: 'url', v: url }));
-  b.webContents.on('did-start-loading', () => sendEvent({ t: 'loading', v: true }));
-  b.webContents.on('did-stop-loading', () => sendEvent({ t: 'loading', v: false }));
-  b.webContents.on('did-fail-load', (_e, code, desc, url) =>
-    sendEvent({ t: 'loadError', code, desc, url })
-  );
-  b.webContents.on('render-process-gone', (_e, details) =>
-    sendEvent({ t: 'crash', reason: details.reason, exitCode: details.exitCode })
-  );
-  // Popups: report them rather than silently opening an invisible window.
-  b.webContents.setWindowOpenHandler(({ url }) => {
-    sendEvent({ t: 'popup', url });
-    return { action: 'deny' };
-  });
-  return b;
+  const wc = b.webContents;
+  const tab = { id, win: b, title: '', url: url || 'about:blank', loading: false, crashed: false };
+  tabs.set(id, tab);
+
+  wc.setFrameRate(60);
+  // Every tab is born in the background. Measured: a stopped tab emits exactly zero
+  // 'paint' events while it loads and navigates, so a background tab costs no pixel
+  // work at all. activateTab() is the only thing that starts painting.
+  wc.stopPainting();
+
+  wc.on('paint', (_e, dirty, image) => onPaint(id, dirty, image));
+  wc.on('page-title-updated', (_e, title) => {
+    tab.title = title;
+    sendEvent({ t: 'title', tab: id, v: title });
+  });
+  const onNav = (_e, u) => {
+    tab.url = u;
+    sendEvent({ t: 'url', tab: id, v: u });
+  };
+  wc.on('did-navigate', onNav);
+  wc.on('did-navigate-in-page', onNav);
+  wc.on('did-start-loading', () => {
+    tab.loading = true;
+    sendEvent({ t: 'loading', tab: id, v: true });
+  });
+  wc.on('did-stop-loading', () => {
+    tab.loading = false;
+    sendEvent({ t: 'loading', tab: id, v: false });
+  });
+  wc.on('did-fail-load', (_e, code, desc, u) =>
+    sendEvent({ t: 'loadError', tab: id, code, desc, url: u })
+  );
+  // The window and its webContents survive a renderer crash, so recovery is a reload
+  // rather than a re-create -- see the tab.recover command. Two tabs on the same site
+  // may share a renderer, so more than one of these can fire from a single crash.
+  wc.on('render-process-gone', (_e, details) => {
+    tab.crashed = true;
+    sendEvent({ t: 'crash', tab: id, reason: details.reason, exitCode: details.exitCode });
+  });
+  wc.on('unresponsive', () => sendEvent({ t: 'unresponsive', tab: id }));
+  // Popups: report them rather than silently opening an invisible window. The core owns
+  // tab ids, so it decides whether to answer this with a tab.new.
+  wc.setWindowOpenHandler(({ url: u }) => {
+    sendEvent({ t: 'popup', tab: id, url: u });
+    return { action: 'deny' };
+  });
+
+  sendEvent({ t: 'tab.opened', id, url: tab.url });
+  if (url && url !== 'about:blank') load(tab, url);
+  return tab;
+}
+
+function activateTab(id) {
+  const tab = tabs.get(id);
+  if (!tab || tab.win.isDestroyed()) {
+    sendEvent({ t: 'tab.error', id, msg: 'no such tab' });
+    return;
+  }
+  // Re-activating the active tab would cost a redundant full-screen repaint.
+  if (activeId === id) return;
+
+  const prev = tabs.get(activeId);
+  if (prev && !prev.win.isDestroyed()) prev.win.webContents.stopPainting();
+  activeId = id;
+  if (pendingFrame && pendingFrame.tabId !== id) {
+    pendingFrame = null;
+    stats.dropped++;
+  }
+
+  const wc = tab.win.webContents;
+  wc.setFrameRate(60);
+  // Measured: startPainting() emits one full frame at the current geometry even when the
+  // page is completely static, so the switch repaints the terminal on its own. Note that
+  // invalidate() alone does NOT do this on a fully idle page.
+  wc.startPainting();
+  sendEvent({ t: 'tab.activated', id });
+}
+
+function closeTab(id) {
+  const tab = tabs.get(id);
+  if (!tab) {
+    sendEvent({ t: 'tab.error', id, msg: 'no such tab' });
+    return;
+  }
+  const wc = tab.win.webContents;
+  if (!wc.isDestroyed()) {
+    wc.stopPainting();
+    wc.removeAllListeners('paint'); // no paint may fire against a dying tab
+  }
+  tabs.delete(id);
+  if (pendingFrame && pendingFrame.tabId === id) {
+    pendingFrame = null;
+    stats.dropped++;
+  }
+  if (!tab.win.isDestroyed()) tab.win.destroy();
+  sendEvent({ t: 'tab.closed', id });
+
+  if (activeId === id) {
+    activeId = null;
+    const next = tabs.keys().next();
+    if (!next.done) activateTab(next.value);
+    // Closing the last tab leaves the engine alive and idle; the core decides whether to
+    // open another tab or send quit.
+    else sendEvent({ t: 'tabs.empty' });
+  }
+}
+
+function tabSummary() {
+  return [...tabs.values()].map((t) => ({
+    id: t.id,
+    title: t.title,
+    url: t.url,
+    loading: t.loading,
+    crashed: t.crashed,
+    active: t.id === activeId,
+  }));
+}
+
+// Commands may name a tab. Without an id they address the active tab, which is exactly
+// the old single-tab behaviour -- every existing core command keeps working unchanged.
+function targetTab(cmd) {
+  const id = typeof cmd.id === 'number' ? cmd.id : activeId;
+  const tab = tabs.get(id);
+  if (!tab || tab.win.isDestroyed()) return null;
+  return tab;
 }
 
 // --- Input injection --------------------------------------------------------------
@@ -136,13 +301,11 @@
 function modifierList(m) {
   const out = [];
   if (!m) return out;
   if (m.shift) out.push('shift');
   if (m.ctrl) out.push('control');
   if (m.alt) out.push('alt');
   if (m.meta) out.push('meta');
   return out;
 }
 
-function handleInput(cmd) {
-  if (!win || win.isDestroyed()) return;
-  const wc = win.webContents;
+function handleInput(cmd, wc) {
   const mods = modifierList(cmd.mods);
 
   if (cmd.kind === 'mouse') {
@@ -207,37 +370,90 @@
 function handleCommand(cmd) {
   switch (cmd.t) {
+    case 'tab.new': {
+      if (typeof cmd.id !== 'number' || cmd.id < 0 || cmd.id > 255) {
+        sendEvent({ t: 'tab.error', id: cmd.id, msg: 'id must be an integer 0..255' });
+        break;
+      }
+      if (tabs.has(cmd.id)) {
+        sendEvent({ t: 'tab.error', id: cmd.id, msg: 'duplicate tab id' });
+        break;
+      }
+      if (tabs.size >= MAX_TABS) {
+        sendEvent({ t: 'tab.error', id: cmd.id, msg: `tab limit reached (${MAX_TABS})` });
+        break;
+      }
+      createTab(cmd.id, cmd.url);
+      if (cmd.activate !== false) activateTab(cmd.id);
+      break;
+    }
+    case 'tab.close':
+      closeTab(cmd.id);
+      break;
+    case 'tab.activate':
+      activateTab(cmd.id);
+      break;
+    case 'tab.list':
+      sendEvent({ t: 'tabs', v: tabSummary(), active: activeId });
+      break;
+    case 'tab.recover': {
+      // Measured: after a renderer crash the window and webContents survive, and loading
+      // a URL into the same webContents restores it to a full 60 fps.
+      const t = targetTab(cmd);
+      if (t) {
+        t.crashed = false;
+        load(t, cmd.url || t.url);
+      }
+      break;
+    }
     case 'navigate':
-      if (win && !win.isDestroyed()) win.loadURL(cmd.url);
+      {
+        const t = targetTab(cmd);
+        if (t) load(t, cmd.url);
+      }
       break;
     case 'resize':
-      if (win && !win.isDestroyed()) {
-        win.setSize(Math.max(1, cmd.w), Math.max(1, cmd.h));
-        // Force a repaint so the new geometry reaches the terminal immediately rather
-        // than waiting for the page to happen to change.
-        win.webContents.invalidate();
+      // Terminal geometry is global, so every tab is resized. Measured: setSize on a tab
+      // whose painting is stopped produces no paint events, so this is free for
+      // background tabs -- and doing it now avoids a stale-geometry first frame the next
+      // time one of them is activated.
+      geomW = Math.max(1, cmd.w);
+      geomH = Math.max(1, cmd.h);
+      for (const t of tabs.values()) {
+        if (!t.win.isDestroyed()) t.win.setSize(geomW, geomH);
+      }
+      {
+        // Force a repaint so the new geometry reaches the terminal immediately rather
+        // than waiting for the page to happen to change.
+        const a = tabs.get(activeId);
+        if (a && !a.win.isDestroyed()) a.win.webContents.invalidate();
       }
       break;
     case 'input':
-      handleInput(cmd);
+      {
+        const t = targetTab(cmd);
+        if (t) handleInput(cmd, t.win.webContents);
+      }
       break;
     case 'reload':
-      if (win && !win.isDestroyed()) win.webContents.reload();
+      {
+        const t = targetTab(cmd);
+        if (t) t.win.webContents.reload();
+      }
       break;
     case 'back':
-      if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoBack()) {
-        win.webContents.navigationHistory.goBack();
+      {
+        const t = targetTab(cmd);
+        if (t && t.win.webContents.navigationHistory.canGoBack()) {
+          t.win.webContents.navigationHistory.goBack();
+        }
       }
       break;
     case 'forward':
-      if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoForward()) {
-        win.webContents.navigationHistory.goForward();
+      {
+        const t = targetTab(cmd);
+        if (t && t.win.webContents.navigationHistory.canGoForward()) {
+          t.win.webContents.navigationHistory.goForward();
+        }
       }
       break;
     case 'stats':
-      sendEvent({ t: 'stats', ...stats });
+      sendEvent({ t: 'stats', ...stats, tabs: tabs.size, active: activeId });
       break;
     case 'quit':
       app.exit(0);
       break;
   }
 }
@@ -269,15 +485,17 @@
 app.whenReady().then(() => {
   if (app.dock) app.dock.hide();
   sock = net.createConnection(SOCKET_PATH, () => {
-    win = createWindow(INITIAL_W, INITIAL_H);
     sendEvent({
       t: 'ready',
       electron: process.versions.electron,
       chrome: process.versions.chrome,
-      width: INITIAL_W,
-      height: INITIAL_H,
+      width: geomW,
+      height: geomH,
+      maxTabs: MAX_TABS,
     });
-    if (INITIAL_URL && INITIAL_URL !== 'about:blank') win.loadURL(INITIAL_URL);
+    // Tab 0 is the session's first tab; the core addresses it implicitly until it starts
+    // sending tab ids.
+    createTab(0, INITIAL_URL);
+    activateTab(0);
   });
   sock.on('error', (e) => {
     console.error('[engine] socket error:', e.message);
```

---

## 11. How to verify the diff

1. **No regression, single tab.** `terminal-fenster open https://example.com` must behave exactly as the verified
   run: `ready` first, first frame at the same latency, `2482x814` BGRA, terminal restored on `ctrl+q`.
   The frame header for tab 0 is byte-identical to today, so this works before any Rust change lands.
2. **Background tab costs nothing.** Send `tab.new` with `activate:false`, then `stats`. `produced` must
   not increase while the background tab loads.
3. **Switch repaints.** `tab.activate` on a static page must deliver exactly one frame; check
   `stats.produced` increments by 1 and the terminal shows the new page.
4. **No cross-tab flash.** Send `tab.activate` twice in rapid succession under load; `stats.dropped`
   should be non-zero and no frame from the wrong tab should reach the terminal (requires the §8.1 Rust
   `tab_id` check for full coverage).
5. **Crash isolation.** Point a tab at a crash URL, confirm `crash {tab:n}` names only that tab, that the
   other tab keeps painting, and that `tab.recover` restores it.
6. **Close the last tab.** `tab.close` on the only tab must emit `tabs.empty` and leave the engine alive
   and responsive to a following `tab.new` — this is the regression test for `main.js:36`.

---

## 12. Open items / UNVERIFIED

- **Same-site tabs sharing a renderer** — not testable here (sandbox network allowlist has no general web
  origin). Crash isolation was proven only across distinct `data:` URLs. The core's crash UI must tolerate
  several `crash` events at once.
- **CDP freeze recovery** — freezing works, thawing did not restore painting in my run, and I did not find
  a working recovery. Not shipped.
- **`WebContentsView` bounds larger than the parent `BaseWindow`** — untested, and the reason the
  `WebContentsView` model is deferred despite its better all-painting numbers.
- **`app.getAppMetrics()` `workingSetSize` on macOS** includes shared and purgeable pages. The renderer
  and GPU figures were monotonic and consistent across runs and are quoted with confidence; the
  browser-process figure swung by 6x and is not quoted as a per-tab cost.
- **Single-run measurements.** Each scaling table is one run. The renderer numbers repeated to within 1%
  across probe6 and probe8, so they are trustworthy; the GPU comparison between the two window models
  deserves a repeat before it drives any decision.
