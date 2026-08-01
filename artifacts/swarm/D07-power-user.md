# D07 — Power-User Interaction: Command Palette, Link Hints, Quickmarks, Keymaps

**Mission:** design the command palette, discoverable shortcuts, Vim-style link hint mode
(*specifically*: how link positions get computed), quickmarks, and configurable keymaps with a
config file format.

**Scope note.** I own only this file. Everything under `crates/`, `apps/cli/` and
`apps/engine/src/main.js` is the commander's. Engine and core changes below are written as
specifications and reviewable diffs, **not applied**. All probe code lives in the scratchpad
(§1.2) and touches nothing in the repository.

---

## 0. Headline findings

Six things were measured on this machine (Apple M4, macOS 26.1, Electron 43.2.0 / Chromium 150,
offscreen rendering). Each one changed the design.

| # | Finding | Evidence |
|---|---|---|
| **1** | **`DOM.getBoxModel` is the wrong tool.** Collecting 306 link boxes cost **21.01 ms** sequentially and **10.77 ms** even fully parallelised, versus **1.83 ms** for a single injected-script round trip that returns strictly more information. It is 5.9–11.5× slower and blind to visibility, occlusion and label text. | §5.2 |
| **2** | **A main-world injected script returns attacker-controlled garbage.** A page that overrides `document.querySelectorAll` made the main-world collector report **0 links**; the same script in an isolated world reported **291**. A page that poisons `Element.prototype.getBoundingClientRect` made a cross-origin frame report every coordinate as **-9**. Hints must be collected in an **isolated world**, always. | §5.3 |
| **3** | **Cross-origin iframes are invisible to both obvious approaches.** The main frame's isolated world gets `contentDocument === null`, and `Page.getFrameTree` on the page-level CDP session reported **`childFrames: []`** for an out-of-process frame (verified OOPIF: parent pid 77398, child pid 77399). Reaching them needs `Target.setAutoAttach {flatten:true}` and a per-frame session — **2.19 ms** once you do. | §5.5 |
| **4** | **Focus events do not fire under offscreen rendering.** `document.hasFocus()` is `false` and `focusin` fires **0 times** in *both* the main and isolated worlds, even though `document.activeElement` updates correctly. Automatic insert mode — the feature that makes a modal browser usable — is impossible until `Emulation.setFocusEmulationEnabled` is turned on, after which `focusin` fires normally and survives navigation. **This is a whole-product bug, not just a D07 one:** `:focus-visible`, focus rings and autofocus are all currently wrong. | §4.2 |
| **5** | **A synthetic click on an injected-script rectangle really navigates.** Rect collected at `{x:0,y:0,w:200,h:30}` → `sendInputEvent` mouseDown/mouseUp at (100,15) → `did-navigate` to `/dest`. The hint→activate path needs **no new engine capability**, only the input command that already exists. | §7.1 |
| **6** | **Hint labels can be provably optimal.** A k-ary-tree label generator is prefix-free by construction (so activation fires on the last keystroke, no timeout, no Enter) and matches a k-ary Huffman oracle exactly on total keystrokes. Measured: 291 targets → **2.35 keys average, 3 maximum**. 25 tests pass. | §6 |

**The single recommendation:** build hint collection as **one injected script running in an
isolated world**, and attach the Chromium debugger **once at engine start** — not for hints, but
because `Emulation.setFocusEmulationEnabled` is the only way to make an offscreen page behave like
a focused one. Everything else in this document is downstream of those two decisions.

---

## 1. Evidence base

### 1.1 What was measured, and what was not

| Claim class | Status |
|---|---|
| Electron/Chromium API behaviour, timings, coordinate spaces, OOPIF behaviour, focus | **Measured on this machine**, 7 probes, numbers inline below |
| Label generation, key-spec parsing, chord resolution, config parsing | **Implemented and unit-tested** — 25 tests, `power_proto.rs` |
| Terminal key encodings | **Inherited from A06**, which measured them directly; I re-derive consequences, I do not re-measure |
| Rendering of palette/hints into cells | **Inherited from C06**; I add rows to its layer table rather than redesigning |
| Anything about iTerm2 | **UNVERIFIED** — TCC blocks automation (A04) |

### 1.2 Probe inventory

All under `…/scratchpad/d07/`. Every probe is offscreen, connects to no network beyond loopback,
and exits by itself.

| File | Answers |
|---|---|
| `page.html`, `child.html` | Hostile fixture: 306 links, an occluder, a CSS-transformed link, `display:none` / `visibility:hidden` / zero-size links, a nested scroller, an iframe, and a script that poisons `Element.prototype.getBoundingClientRect`, `Array.prototype.map` and `document.querySelectorAll` |
| `probe.js` | Main-world vs isolated-world under a hostile page; `DOM.getBoxModel` vs injected script timing; coordinate space after scrolling; occlusion hit-testing |
| `probe2.js` | Genuine cross-origin OOPIF (two loopback origins); 2000-link scaling; **end-to-end synthetic click** |
| `probe3.js` | CDP `Target.setAutoAttach` recipe for OOPIFs; `nodeIntegrationInSubFrames` preload alternative |
| `probe4.js` | Insert-mode focus tracking; `Runtime.addBinding`; world identity |
| `probe5.js` | Isolates *why* focus events were missing; `Emulation.setFocusEmulationEnabled` |
| `probe6.js` | Every route to a focused OSR page, and whether emulation survives navigation |
| `probe7.js` | Whether `win.focus()` steals macOS focus from the user's terminal |
| `power_proto.rs` | Label generator, `KeySpec`, chord trie, config parser, legacy-alias model — std-only, 25 tests |

Re-run:

```bash
cd apps/engine
./node_modules/.bin/electron …/scratchpad/d07/probe.js     # and probe2 … probe7
cd …/scratchpad/d07
rustc --test -O -o power_proto power_proto.rs && ./power_proto
```

Chromium child processes are killed by the agent Bash sandbox (`bootstrap_look_up … Permission
denied`), so the probes were run with the sandbox disabled. The Chromium sandbox inside Electron
stayed **on** (`sandbox: true`) in every probe — the engine's security posture was never relaxed
to obtain these numbers.

---

## 2. The constraint that shapes every keymap

Before any binding is chosen, one question decides the whole default keymap: **which chords can a
terminal actually deliver?**

From A06 §2.5–2.6, on a terminal without the kitty keyboard protocol:

- `ctrl`+letter collapses to a single C0 byte (`ctrl+a` → `0x01`). **The byte carries no shift
  bit.** `ctrl+shift+p` and `ctrl+p` are the same two bytes. They are not distinguishable, ever.
- `Enter`, `Escape`, `Tab` and `Space` are single bytes regardless of ctrl/shift, except
  `shift+tab` (`CSI Z`). `ctrl+enter`, `shift+enter`, `ctrl+tab` do not exist.
- Key **release** does not exist at all.
- On macOS, Option is a compose key by default in both Ghostty and Apple Terminal, so the Alt
  modifier is usually unobservable (A06 §2.7).

Our own decoder confirms the collapse: `crates/tf-term/src/input.rs:223-228` maps `0x01..=0x1a`
straight to `KeyCode::Char` + `ctrl`, with no shift information available to recover.

Measured terminal support (A04): **Ghostty 1.3.1 — kitty keyboard YES. Apple Terminal 465 — NO.**

### 2.1 Three rules that follow

1. **No default binding may use `ctrl+shift+…`, `ctrl+enter`, `shift+enter`, `ctrl+tab`, or any
   Alt chord.** A VS-Code-style `ctrl+shift+P` palette would silently do nothing for every Apple
   Terminal user. The palette is therefore bound to **`:`** — a plain printable character that
   every terminal on earth delivers — with `ctrl+p` as a secondary.
2. **Bindings are primarily *sequences*, not chords.** `g g`, `y y`, `' a` cost nothing in
   encoding terms and work identically on every terminal. This is why vim-style browsers use
   them, and the reason is not aesthetic.
3. **The user must be told when their own binding will do the wrong thing.** Users will copy
   `ctrl+shift+t` out of a blog post. `terminal-fenster doctor --keys` reports which bindings this
   terminal cannot deliver *as written* (§15) — distinguishing a harmless alias from a chord that
   silently triggers a different action. Implemented and tested in `power_proto.rs`, and it caught
   two defects in this document's own default keymap before it shipped.

```rust
// power_proto.rs — tested against A06's tables
assert!(!KeySpec::parse("ctrl+shift+p").unwrap().legacy_expressible());
assert!( KeySpec::parse("ctrl+p").unwrap().legacy_expressible());
assert!(!KeySpec::parse("ctrl+enter").unwrap().legacy_expressible());
assert!( KeySpec::parse("shift+tab").unwrap().legacy_expressible());   // CSI Z
assert!( KeySpec::parse("ctrl+shift+left").unwrap().legacy_expressible()); // CSI param carries a real bitmask
```

---

## 3. Mode model

Five modes. The mode is always visible in the status band (C06 §3, priority 10) — a modal
interface that hides its mode is a trap.

| Mode | Keys go to | Entered by | Left by |
|---|---|---|---|
| **normal** | Terminal-Fenster | default; `Escape` from anywhere | — |
| **insert** | the page | focusing an editable after a user action (§4.3); `i` | `Escape`, or focus leaving the editable |
| **hint** | hint label buffer | `f` / `F` / `y f` … | label completed, `Escape`, or invalidation (§8) |
| **palette** | palette query buffer | `:` / `ctrl+p` | `Enter` (run), `Escape` (cancel) |
| **passthrough** | the page, *everything* | `ctrl+v` (configurable) | **only** `ctrl+]` |

**Why passthrough exists and why its exit key is `ctrl+]`.** Web terminals, editors and games
need every key including `Escape`. Passthrough forwards literally everything except one reserved
chord. `ctrl+]` is C0 `0x1D`, expressible on every terminal (A06 §2.6), and essentially unused by
web applications — unlike `ctrl+[`, which *is* Escape and therefore unusable as an escape hatch.
The mode line must display the exit key while passthrough is active, because a user who forgets
it has no way out short of killing the process.

---

## 4. Automatic insert mode, and the focus bug underneath it

### 4.1 Why this section exists

The difference between a modal browser that feels excellent and one that is infuriating is a
single behaviour: when you click into a search box, typing must produce text, not scroll the
page. That requires knowing when an editable element takes focus.

### 4.2 Measured: focus events do not fire under offscreen rendering

`probe4.js`, then isolated in `probe5.js`:

```
without Emulation.setFocusEmulationEnabled:
  {"isolatedN":0, "mainN":0, "active":"txt", "hasFocus":false}
with Emulation.setFocusEmulationEnabled {enabled:true}:
  {"isolatedN":3, "mainN":3, "active":"txt", "hasFocus":true}
```

`document.activeElement` tracked focus correctly the whole time — it became `txt` after a
synthetic click and `lnk` after clicking the link — and synthetic typing worked (`typed_value:
"hi"`). But **`focusin` fired zero times in both worlds**, because the document was never
considered focused. Chromium suppresses focus event dispatch for an unfocused document; OSR with
`show: false` is permanently unfocused.

`probe6.js` compared every route:

| Route | `document.hasFocus()` | `focusin` fires |
|---|---|---|
| baseline | false | no |
| `webContents.focus()` | **false** | **no** |
| `BrowserWindow.focus()` | true | yes |
| `BrowserWindow.focusOnWebView()` | true | yes |
| `Emulation.setFocusEmulationEnabled` | true | yes, **and survives navigation** |

**Recommendation: `Emulation.setFocusEmulationEnabled`, not `win.focus()`**, for three reasons.

1. It is a renderer-side flag and cannot touch OS window state. `probe7.js` measured that
   `win.focus()` on a hidden offscreen window did *not* steal macOS focus (frontmost application
   stayed `Discord` before and after, window stayed `isVisible: false`) — but that is one
   observation of an OS behaviour we have no contract for. A terminal browser that steals
   keyboard focus from the user's terminal is broken beyond repair, and this is not a risk worth
   carrying for zero benefit.
2. It is **per-`webContents`**. B04 puts each tab in its own offscreen `BrowserWindow`; "which
   window is focused" is meaningless there, whereas "focus emulation is enabled on exactly the
   active tab" models the truth precisely. Enable on activate, disable on deactivate.
3. It survives navigation (measured), so it is set once per tab rather than re-armed per page.

**This finding is larger than D07.** Until it lands, every Terminal-Fenster page renders `:focus` and
`:focus-visible` incorrectly, autofocused search boxes never fire their focus handlers, and any
page whose JS waits on a focus event stalls. The commander should treat it as an engine bug with
a D07 dependency, not a D07 feature.

### 4.3 The entry rule

Auto-entering insert mode on *any* focus is wrong: a large fraction of pages autofocus a search
box on load, and a user who then presses `j` to scroll would type a `j`. qutebrowser's default
for the equivalent setting is off, for this reason.

**Rule: enter insert mode only on a user-initiated focus.** The core records a timestamp whenever
it activates a hint or forwards a click. A `focus` event naming an editable element within
`insert_grace_ms` (default 250) of that action enters insert mode. Any other focus — page load,
timer, script — does **not** change the mode; the status band shows `[input focused — i to type]`
instead. Explicit `i` always works.

Editability is decided in the isolated world, where the page cannot lie about it:

```js
const editable = (el) => !!el && (el.isContentEditable ||
  (el.tagName === 'INPUT' && !/^(button|submit|reset|checkbox|radio|file|image|hidden)$/i.test(el.type || 'text')) ||
  el.tagName === 'TEXTAREA' || el.tagName === 'SELECT');
```

### 4.4 The push channel

The isolated world needs to tell the main process that focus changed. Verified working
(`probe5.js`): `Runtime.addBinding {name:'__bgSend', executionContextId: <isolated ctx>}` installs
a real function (`typeof __bgSend === "function"`) whose calls arrive as `Runtime.bindingCalled`
in the main process — payload `"direct-call"` was delivered. No preload, no `nodeIntegration`, no
change to the engine's security posture.

The same channel carries hint invalidation (§8).

---

## 5. Link hints: how link positions are computed

This is the question the mission asks explicitly. The answer is **an injected script running in an
isolated world**, not `DOM.getBoxModel`, and the rest of this section is why.

### 5.1 What a hint actually needs

A hint is not a rectangle. To place and activate one you need, per candidate:

1. a rectangle in **top-level viewport CSS pixels**, so it can be divided by the measured cell
   size (Ghostty: 17×37 px) into a cell position;
2. whether the element is **visible** — `display:none`, `visibility:hidden`, `opacity:0`,
   zero-size and off-viewport elements must never get a label;
3. whether a **synthetic click at the chosen point would actually hit it**, or whether something
   is on top;
4. a **label string** (`aria-label`, text content, `value`) for text-matching hints and for the
   status line;
5. the **element kind**, because `f` on a link navigates but `f` on a text input should focus and
   enter insert mode.

`DOM.getBoxModel` returns exactly one of those five. Everything else needs script execution
anyway — so the question is not "CDP or script", it is "script, plus how much useless CDP".

### 5.2 Measured cost

Fixture: 306 `a[href]` elements plus form controls, 1200×800 viewport, 291 elements surviving
visibility filtering. Median of 5–7 runs (`probe.js`):

| Approach | Median | Min | Returns |
|---|---:|---:|---|
| **Electron `executeJavaScriptInIsolatedWorld`** | **1.83 ms** | 1.38 ms | all five fields |
| CDP `Runtime.evaluate` in a CDP isolated world | 4.05 ms | 3.68 ms | all five fields |
| CDP `DOM.getBoxModel`, parallel (`Promise.all`) | 10.77 ms | 10.66 ms | rectangles only |
| CDP `DOM.getBoxModel`, sequential | 21.01 ms | 20.15 ms | rectangles only |

Scaling (`probe2.js`, 2000 links in the DOM, 397 inside the viewport): **4.83 ms** median,
19,751 bytes of JSON. Viewport culling does the heavy lifting — 2000 candidates collapse to 397
before anything crosses a process boundary.

The frame budget is 16.65 ms (measured p50 frame gap). One injected-script collection fits inside
a single frame with room to spare; a sequential `getBoxModel` sweep does not fit at all.

`getBoxModel` is slow for a structural reason, not an incidental one: it needs a `nodeId` per
element, so the sequence is `DOM.getDocument` → `DOM.querySelectorAll` → **N** separate
`DOM.getBoxModel` round trips into the renderer. Parallelising hides latency but not the N
crossings. The injected script crosses once.

**One thing `getBoxModel` does better, and it does not matter.** For a CSS-transformed element it
returns the true rotated quad, where `getBoundingClientRect` returns the axis-aligned bounding
box:

```
transformed_el.grbc            = {x:36.72, y:347.93, w:110.89, h:58.30}
transformed_el.cdp_border_quad = [44.25,347.93, 147.61,385.55, 140.09,406.23, 36.72,368.60]
```

Hints are cell-quantised text anyway (C06 §3: at 17×37 a label lands up to 16 px horizontally and
36 px vertically from its target, and this is fine because hinting is label-driven rather than
position-driven). A rotated quad's extra precision is destroyed by quantisation. Where it *would*
matter is the click point, and there the hit-test in §5.4 catches the error directly. If a
rotated-element miss is ever observed in practice, the fix is `getClientRects()[0]` plus an
`elementFromPoint` walk, not a per-node CDP call.

### 5.3 Isolated world, never the main world

The fixture page runs this before anything else:

```js
Element.prototype.getBoundingClientRect = function () { return { x:-1, y:-1, width:-1, height:-1, … }; };
Array.prototype.map = function () { return ['PWNED']; };
document.querySelectorAll = function () { return []; };
```

Measured (`probe.js`):

```
mainWorld_n : 0      ← the collector found no links at all
isoWorld_n  : 291    ← correct, and isoWorld_poisoned_flag: false
```

The isolated world has its own JS globals and its own DOM wrappers, so neither the prototype
patches nor the `window.__POISONED__` marker were visible to it, while sharing the same live DOM.

This is not a hypothetical. A page that can make hint mode report zero links can suppress a
"Cancel" button; a page that can shift reported coordinates can make the hint for "Cancel" sit on
top of "Confirm". **Hint collection is a trust boundary and must be treated as one.**

`probe4.js` also confirms three distinct worlds: a global set in the CDP isolated world was
invisible both in Electron's isolated world 9 and in the page's main world. Pick one world id and
use it consistently, or state will silently vanish.

### 5.4 The collection script

One round trip, viewport-culled, hit-tested. This is the specification; the engine-side wrapper
is in §17.

```js
(() => {
  const SEL = 'a[href], button, input:not([type=hidden]), select, textarea, summary,' +
              '[role=button], [role=link], [role=checkbox], [role=tab], [onclick],' +
              '[tabindex]:not([tabindex="-1"]), [contenteditable=""], [contenteditable=true]';
  const vw = innerWidth, vh = innerHeight, out = [];
  const els = document.querySelectorAll(SEL);
  for (let i = 0; i < els.length; i++) {
    const el = els[i];
    const rs = el.getClientRects();          // getClientRects, not getBoundingClientRect:
    if (!rs.length) continue;                // a wrapped inline link has several boxes and the
    let r = null;                            // union box can land in whitespace between them
    for (let j = 0; j < rs.length; j++) {
      const q = rs[j];
      if (q.width < 1 || q.height < 1) continue;
      if (q.bottom < 0 || q.top > vh || q.right < 0 || q.left > vw) continue;
      r = q; break;
    }
    if (!r) continue;
    const st = getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none' || st.opacity === '0') continue;
    const cx = Math.min(Math.max(r.left + Math.min(r.width / 2, 20), 0), vw - 1);
    const cy = Math.min(Math.max(r.top + r.height / 2, 0), vh - 1);
    const hit = document.elementFromPoint(cx, cy);
    out.push({
      x: Math.round(r.left), y: Math.round(r.top),
      w: Math.round(r.width), h: Math.round(r.height),
      cx: Math.round(cx), cy: Math.round(cy),
      clickable: !!hit && (hit === el || el.contains(hit) || hit.contains(el)),
      kind: el.tagName === 'A' ? 'link' : (editable(el) ? 'edit' : 'click'),
      label: (el.getAttribute('aria-label') || el.textContent || el.value || '').trim().slice(0, 40)
    });
  }
  return { n: out.length, vw, vh, sx: scrollX, sy: scrollY, items: out };
})()
```

Two details that are load-bearing:

- **`Math.min(r.width / 2, 20)`** — the click point is 20 px in from the left edge rather than at
  the centre. A full-width nav link's centre is often over a child element or a gap; the leading
  edge is over the text.
- **`clickable`** comes from `document.elementFromPoint`. Measured on the fixture: the link
  underneath a fixed overlay returned `elementFromPoint → "occluder"` and was correctly marked
  `clickable: false`. `DOM.getBoxModel` would have happily reported that link's rectangle and the
  synthetic click would have hit the overlay. Occluded elements are still labelled — cookie
  banners cover real content — but activation routes through §7.2 instead of a raw click.

### 5.5 Cross-origin iframes

`probe2.js` built a genuine out-of-process iframe (parent `http://127.0.0.1:50369`, child
`http://localhost:50368`) and confirmed the split: **parent pid 77398, child pid 77399**.

Measured behaviour of every route into that frame:

| Route | Result |
|---|---|
| Main frame's isolated world → `iframe.contentDocument` | **`contentDocument null`** — same-origin only |
| `webContents.executeJavaScriptInIsolatedWorld` | Main frame only (`electron.d.ts:17954` is on `WebContents`) |
| `WebFrameMain.executeJavaScript` (`electron.d.ts:18921`) | **Reaches the frame, but runs in the MAIN world.** Measured against the poisoned child: `{x:-9, y:-9, w:-9, h:-9, poisonVisible:true}`. `WebFrameMain` has **no** `executeJavaScriptInIsolatedWorld` — I checked the whole class body. Unusable for a trust boundary. |
| Page-level CDP `Page.getFrameTree` | **`childFrames: []`** — the OOPIF is not in the page session's tree at all |
| CDP `Target.setAutoAttach {flatten:true}` → per-frame session | **Works.** Session `type:"iframe"`, then `Page.createIsolatedWorld` + `Runtime.evaluate` returned correct coordinates with `poisonVisible:false` in **2.19 ms** |

The verified recipe (`probe3.js`):

```js
dbg.attach('1.3');
dbg.on('message', (_e, method, params) => {
  if (method === 'Target.attachedToTarget') sessions.set(params.sessionId, params.targetInfo);
});
await dbg.sendCommand('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
// then, per cross-origin frame session:
await dbg.sendCommand('Page.enable', {}, sessionId);
const ft = await dbg.sendCommand('Page.getFrameTree', {}, sessionId);
const w  = await dbg.sendCommand('Page.createIsolatedWorld', { frameId: ft.frameTree.frame.id, worldName: 'bg-hints' }, sessionId);
const r  = await dbg.sendCommand('Runtime.evaluate', { expression: COLLECT, contextId: w.executionContextId, returnByValue: true }, sessionId);
```

`sendCommand`'s third parameter is the session id (`electron.d.ts:7606`).

**The tiered plan.** Most pages have no cross-origin frames, so nobody should pay for them:

- **Tier 1 — always.** One `executeJavaScriptInIsolatedWorld` on the main frame. The script
  recurses into **same-origin** iframes through `contentDocument` (measured working: the
  same-origin fixture returned `reached:1`) and composes their offsets itself. Cost 1.83–4.83 ms.
- **Tier 2 — only when a cross-origin frame exists.** Detected with
  `webContents.mainFrame.framesInSubtree` (`electron.d.ts:18975`) by comparing `frame.origin`
  against the main frame's. One `Runtime.evaluate` per such frame, ~2.19 ms each, composed as
  below. A page with no cross-origin frames never touches a CDP session.

**Coordinate composition.** A child frame reports coordinates in *its own* viewport — measured
`vw:320, vh:70` for a 320×70 iframe, with its link at `(0,0)` local. Composition is:

```
top_viewport_point = child_local_point + iframe_rect_in_parent   (recursively, parent by parent)
```

Measured: child `(0,0)` + iframe offset `(203.59, 0)` = `(203.59, 0)`. Verified again in the
nested case in `probe.js`: child `y:8` + iframe `y:1009.375` = `1017.375`.

Two consequences that are easy to get wrong:

1. **A child frame cannot cull against the top-level viewport.** It culls against its own, so a
   frame scrolled off the main page still reports "visible" links. The parent must clip each
   frame's results to the intersection of the frame's rect with the top viewport. In `probe.js`
   the iframe sat at `y = 1009` in an 800 px viewport — every one of its links was off-screen and
   the child had no way to know.
2. **Frame ids and sessions are invalidated by navigation.** `Target.detachedFromTarget` must
   evict the cached session or the next hint cycle evaluates against a dead frame.

### 5.6 Rejected: the preload alternative

`probe3.js` measured a genuinely simpler design: `webPreferences.preload` +
`nodeIntegrationInSubFrames: true`. It works — the preload ran in **both** the top frame and the
OOPIF, both reported `poisonVisible: false` (preloads run in the isolated world when
`contextIsolation: true`), and the child correctly reported its link. It needs no CDP, no
auto-attach and no session bookkeeping, and it worked with `sandbox: true`.

**It is still the wrong choice here**, because `nodeIntegrationInSubFrames: true` runs our preload
and opens an IPC endpoint inside *every* subframe on the page — every ad, every tracker, every
embedded widget — and `apps/engine/src/main.js:12-14` states the opposite posture in as many
words. That is a change to the engine's threat model (A09's territory) traded for avoiding ~40
lines of session bookkeeping, on a code path that only executes for pages that actually have
cross-origin frames. Recorded here as measured-and-viable in case the commander later wants
per-frame instrumentation for other reasons; not recommended for hints.

### 5.7 Coordinate space, settled

`DOM.getBoxModel` and `getBoundingClientRect` return **the same space**: viewport-relative CSS
pixels, scroll already subtracted. After `scrollTo(0, 500)` (`probe.js`):

```
transformed_el.grbc.y            = 347.92999267578125
transformed_el.cdp_border_quad y = 347.92999267578125   ← identical
Page.getLayoutMetrics.cssVisualViewport.pageY = 500     ← the scroll did happen
```

So: **no scroll compensation is needed anywhere.** Rects go straight to cells:

```
col = floor(x / cell_w)      row = floor(y / cell_h)      // Ghostty measured 17 × 37
```

The page image is placed 1:1 and unscaled (C06 §2), so there is no additional scale factor. If
C07's zoom/scaling work ever introduces one, hint rects must be multiplied by exactly the same
factor and this is the line that has to change.

---

## 6. Hint labels

### 6.1 The property that matters

Labels must be **prefix-free**: no label may be a prefix of another. With that property, hint mode
fires on the final keystroke — no confirm key, no ambiguity timer. Without it, every hint costs a
timeout, and the interaction feels broken.

### 6.2 The generator

Labels are leaves of a k-ary tree, which makes them prefix-free by construction. Let `d` be the
smallest depth with `k^d ≥ n`. Of the `k^(d-1)` prefixes at depth `d-1`, the first
`s = floor((k^d − n) / (k − 1))` stay as short labels and the rest expand into their `k` children.
Short labels go to the earliest targets, and collection is in document order, so the top of the
page is cheapest to reach.

```rust
pub fn hint_labels(n: usize, alphabet: &[char]) -> Vec<String>   // power_proto.rs
```

Default alphabet, 14 characters: `a s d f j k l e w i o c m p` — home row first, no `g` (reserved
as a sequence prefix), no characters that collide with hint-mode control keys.

### 6.3 Verification

25 tests pass. The two that matter:

- **`labels_are_prefix_free_for_every_count`** — checks the property exhaustively for every
  `n` in 1..=600, not for a handful of sampled values.
- **`labels_are_length_optimal`** — compares total keystrokes against an **independent k-ary
  Huffman oracle** with unit weights, implemented separately in the test. Huffman is optimal for
  prefix-free codes, so an exact match means the generator cannot be beaten. It matches at
  `n ∈ {1,2,13,14,15,27,100,196,197,397,1000,2000}`, and label lengths never differ by more
  than 1.

My first implementation failed both length tests — expanding the last leaf repeatedly produced
labels that grew without bound. The oracle caught it. This is why the test compares against a
computed optimum instead of a hand-picked threshold; my first hand-picked threshold (`avg < 2.5`
at n=397) was itself wrong — the true optimum is 2.55.

### 6.4 Measured cost

```
n=8    first=[a, s, d, f, j, k, l, e]              avg 1.00 keys   max 1
n=20   first=[a, s, d, f, j, k, l, e, w, i]        avg 1.35 keys   max 2
n=60   first=[a, s, d, f, j, k, l, e, w, i]        avg 1.83 keys   max 2
n=291  first=[aa, as, ad, af, aj, ak, al, ae, …]   avg 2.35 keys   max 3
n=397  first=[aa, as, ad, af, aj, ak, al, ae, …]   avg 2.55 keys   max 3
```

291 and 397 are the measured element counts from probes 1 and 2. **A dense page costs 2–3
keystrokes per link.** Raising the alphabet to 20 characters would buy roughly 0.2 keystrokes at
n=397 at the cost of weaker keys; not worth it, but it is one config line if a user disagrees.

### 6.5 Text filtering

Typing a character that is not in the alphabet switches to **text-filter mode**: the buffer
matches against `label`, hints that do not match are hidden, and remaining hints are
**relabelled**. This is how `f` + `che` + `a` reaches "Checkout" on a page with 300 links, and it
is why `label` is collected in §5.4. Filter matching is case-insensitive substring first,
subsequence second (§10.3's scorer, reused).

---

## 7. Activating a hint

### 7.1 Synthetic mouse, verified end to end

`probe2.js` collected a rect via the injected script, then drove the existing engine input path:

```
hintRect      {x:0, y:0, w:200, h:30}
clickedAt     {cx:100, cy:15}
sendInputEvent mouseEnter → mouseMove → mouseDown → mouseUp
navigatedTo   "http://127.0.0.1:50369/dest"      ✅
```

So the whole chain works with `apps/engine/src/main.js:157-203` exactly as written — `mouse`
action `move`/`down`/`up` with the coordinates the collector produced. **Hint activation needs no
new engine input capability.**

Synthetic mouse is preferred over `el.click()` because it produces the real event sequence
(`mouseover`, `mousedown`, `focus`, `mouseup`, `click`) with correct `:hover`/`:active` states and
correct coordinates, which is what analytics-heavy and framework-heavy pages expect. The engine's
existing `mouseEnter` latch (`main.js:160-168`) already handles the OSR `:hover` quirk.

### 7.2 When the point is occluded

`clickable: false` means a synthetic click would hit something else. Escalation ladder:

1. **Scroll it into the clear** — `el.scrollIntoView({block:'center'})` in the isolated world,
   re-collect, retry. Fixes the common case of a sticky header covering the target.
2. **Try another rect** — a wrapped inline link has several client rects; test each.
3. **Fall back to `el.click()`** in the isolated world. This bypasses hit-testing entirely.
   `executeJavaScriptInIsolatedWorld(worldId, scripts, userGesture)` takes a user-gesture flag
   (`electron.d.ts:17954`); it must be `true` or activation-gated APIs (popups, clipboard,
   fullscreen) will refuse.
4. **Report honestly** — `hint blocked by overlay` in the status band. Silently doing something
   other than what the user asked is worse than saying it could not be done.

### 7.3 Hint verbs

One collection, many actions — the mode carries the verb:

| Binding | Action | Behaviour |
|---|---|---|
| `f` | `hint.follow` | click; if the target is editable, focus it and enter insert mode |
| `F` | `hint.follow_new_tab` | `tab.new` with the resolved `href`, background |
| `y f` | `hint.yank` | copy `href` to the clipboard via OSC 52 (A06 §6) |
| `; d` | `hint.download` | download the `href` |
| `; i` | `hint.image` | restrict candidates to `img` |
| `; h` | `hint.hover` | move the pointer only — the one way to open a CSS-only dropdown |

`hint.follow_new_tab` and `hint.yank` need the `href` string, which the collector already returns
for links. For non-link targets these verbs are disabled rather than approximated.

---

## 8. Hint lifecycle

Collected coordinates are a snapshot. They go stale, and a stale hint clicks the wrong thing.

| Event | Response | Why |
|---|---|---|
| `did-navigate`, `did-navigate-in-page` | **cancel** | every rect is meaningless |
| `resize` | **cancel** | reflow invalidates everything, and geometry change forces full damage (B07) |
| scroll (any) | **recollect** | cheap at 1.83 ms; cancelling on scroll would be infuriating |
| DOM mutation inside a hinted subtree | **recollect**, debounced 100 ms | SPAs mutate constantly |
| tab switch | **cancel** | hints belong to a tab |
| `Escape` | **cancel** | — |

Scroll and mutation are detected in the isolated world and pushed through the verified
`Runtime.addBinding` channel (§4.4), debounced on the page side so a `requestAnimationFrame`-driven
animation cannot flood the socket:

```js
let t = 0;
const invalidate = () => { clearTimeout(t); t = setTimeout(() => __bgSend('{"t":"hints.stale"}'), 100); };
addEventListener('scroll', invalidate, { capture: true, passive: true });
new MutationObserver(invalidate).observe(document, { subtree: true, childList: true, attributes: true });
```

The observer is disconnected when hint mode exits. An always-on `MutationObserver` on a busy SPA
is a real CPU cost and there is no reason to pay it outside hint mode.

**Bandwidth note (A03/A07).** Hints are text (C06 §3, priority 70). 400 labels of 2–3 characters
plus positioning is a few KB of terminal output against ~54,000 wire bytes for one page frame. In
the SSH tiers of A03, link hints get *cheaper* relative to everything else, and at tier T3
(text-only) they are the only viable pointing device. The 19,751-byte collection JSON never
crosses the SSH link — it is engine→core, and in A07's recommended topology (b′) both live on the
remote host.

---

## 9. Grid-jump

A02:260 recommends shipping grid-jump alongside hints, because hints cannot position a *cursor*
on a canvas, map or drag target. It shares this document's machinery — recursive 3×3 subdivision
with `qweasdzxc` labels, each keystroke narrowing to one ninth, then a synthetic mouse event at
the resulting point. It needs **no** DOM access at all: it is pure arithmetic on the viewport
rectangle, so it works on `<canvas>`, WebGL, PDFs and cross-origin frames where hinting cannot.
C06 already reserves it a layer (priority 75, "covering the page is its entire function"). Bound
to `; g`. Specified here only for completeness; the interaction detail belongs to whoever owns
pointer emulation.

---

## 10. Command palette

### 10.1 What it is

One fuzzy-searchable list over every action, tab, bookmark, quickmark and history entry, opened
with `:`. It is the discoverability engine (§11) and the escape hatch for everything not worth a
keybinding.

### 10.2 Rendering

Text, not pixels — C06 §2's layer split. It is a **float**, input-blocking, drawn over the page
image at `PAGE_Z = -1500000000`.

**C06's layer table has no row for the palette.** Proposed, for the commander to fold into C06 §3:

| Element | Layer | Placement | Prio | Why |
|---|---|---|---|---|
| **Command palette** | text | float, centred, input-blocking | **85** | outranks the find bar (80) because it is modal; below permission prompts (90), which must outrank everything |

Layout: one query row, up to 10 result rows, one help row. At 2482×851 px / 17×37 (measured
Ghostty window geometry) the grid is 146 × 23 cells, so a 12-row palette covers just over half
the viewport. **Cap results at `min(palette.max_results, rows/2 − 2)`** so the palette never
swallows a short terminal. It is a float, not a band, so opening it costs no `resize` and
therefore no Chromium reflow and no forced full-damage frame (B07:132-136, via C06 §3).

### 10.3 Scoring

fzf-style subsequence matching, deterministic and dependency-free:

```
score = Σ over matched characters:
          +16  match at a word boundary (start, or after ' ', '.', '-', '_', '/')
          +8   match immediately after the previous match (contiguous run)
          +4   case-sensitive exact match
          +1   otherwise
        −1 per skipped character between matches
        + source weight (see below)
```

Rank descending, break ties by recency then alphabetically. Bounded by construction: candidates
are capped at 2000 per source and the query at 64 characters, so worst case is a few hundred
thousand character comparisons — microseconds, and far below the 16.65 ms frame budget. If it ever
is not, the fix is capping candidates, never dropping the frame.

### 10.4 Sources and sigils

Empty query shows actions ranked by recency of use, which makes the palette a personal shortcut
list within a day of use.

| Sigil | Source | Weight | Notes |
|---|---|---|---|
| *(none)* | everything | — | actions first |
| `>` | actions only | +20 | the registry (§10.5) |
| `@` | open tabs | +15 | from B04's `tabs` event |
| `#` | history | +0 | `bg.sqlite` (B09 §9) |
| `*` | bookmarks + quickmarks | +10 | B09 §10 and §12 |
| `/` | text on the current page | — | hands off to find |

Anything that parses as a URL or contains a dot with no spaces offers "Open …" as the first row,
so the palette is also the omnibox and there is one thing to learn instead of two.

### 10.5 The action registry

Actions are dotted `noun.verb` strings. **One registry serves three consumers**: the keymap, the
palette, and the agent-control API from A03 journey (c). Adding an action makes it bindable,
searchable and scriptable at once; there is no second list to keep in sync.

```
tab.new  tab.close  tab.next  tab.prev  tab.first  tab.last  tab.reopen  tab.select
nav.back  nav.forward  nav.reload  nav.reload_hard  nav.stop  nav.home  nav.up  nav.root
scroll.down  scroll.up  scroll.left  scroll.right  scroll.top  scroll.bottom
scroll.page_down  scroll.page_up  scroll.half_down  scroll.half_up
hint.follow  hint.follow_new_tab  hint.yank  hint.download  hint.image  hint.hover
palette.open  omnibox.open  omnibox.open_new_tab  find.open  find.next  find.prev
yank.url  yank.title  yank.markdown  paste.open
mode.normal  mode.insert  mode.passthrough
quickmark.set  quickmark.open  quickmark.open_new_tab  quickmark.delete
zoom.in  zoom.out  zoom.reset
view.source  view.devtools_off  help.keys  doctor.run  config.reload  quit
```

Each entry carries a human title, the source, and **its current binding**, which §11 depends on.

### 10.6 Page-derived text is hostile

Tab titles, history titles, bookmark titles and hint labels are attacker-controlled. Every string
from page content must pass through A09's sanitizer before reaching a cell — A09 §D is explicit
that stripping `0x1B` alone is defeated by the 8-bit C1 introducers, and §F covers Trojan-Source
bidi overrides (CVE-2021-42574, CVE-2021-42694). D07 adds no sanitizer of its own; it declares
that **the palette, hint labels and the status band are all inside A09's untrusted-text boundary**
and must call the same function. Titles are additionally clamped to the column width by display
width, not by byte or `char` count, or a wide CJK title will overflow the float and corrupt cells
C06's arbiter believes it owns.

---

## 11. Discoverable shortcuts

A modal interface that requires reading a manual has failed. Five mechanisms, in ascending
deliberateness:

1. **Mode + hint line, always visible.** The status band (C06 §3, priority 10) shows the mode and
   the three or four keys most relevant to it: `NORMAL  f hints  : palette  o open  ? keys`. It
   is ~230 bytes of text and costs nothing.

2. **Which-key, free from the chord trie.** When the resolver returns `Resolve::Prefix` and
   `mode_timeout_ms` elapses, a float lists every continuation of the pending sequence with its
   action title. Pressing `g` and waiting teaches you `g g`, `g t`, `g m`. This falls directly
   out of the same trie that does the dispatching — no second data structure:

   ```rust
   pub enum Resolve<'a> {
       Action(&'a str),
       Prefix,                          // ← which-key renders from this
       Ambiguous { action: &'a str },   // bound AND a prefix: fire on timeout
       NoMatch,
   }
   ```

3. **`?` opens the full keymap**, which is just the palette filtered to bound actions, grouped by
   mode, searchable. One implementation, two entry points.

4. **The palette shows the binding next to every action.** This is the main path by which people
   learn shortcuts: they reach for the palette, see `f` beside "Follow link", and next time they
   press `f`. Actions with no binding show a dimmed `unbound` that opens the config at the right
   line.

5. **`terminal-fenster doctor --keys`** reports what is silently broken (§15).

**First-run.** On first launch, one line: `Press ? for keys, : for the command palette.` If the
terminal reports no kitty keyboard support, add A06 §2.7's advice about
`macos-option-as-alt = true`. No tour, no overlay.

---

## 12. Quickmarks

### 12.1 The distinction from bookmarks

B09 §10 owns bookmarks: a `bg.sqlite` table, per-profile, precious, imported and exported as
Netscape HTML. Quickmarks are a different thing wearing a similar name — **a name bound to a URL,
optimised for typing**. They belong with the keymap, not the bookmark table:

| | Bookmarks (B09 §10) | Quickmarks (D07) |
|---|---|---|
| Storage | `bg.sqlite`, per-profile | plain text in the config dir |
| Count | hundreds to thousands | 5–40 |
| Accessed by | browsing, searching | **typing a name** |
| Hand-editable | no (SQLite) | yes, and dotfile-managed |
| Cardinality | many per site | one per name |

A user who keeps their config in git expects their quickmarks to come with it. A SQLite blob does
not travel that way, and putting them in `bg.sqlite` would make the natural workflow — editing
them in an editor — impossible.

### 12.2 Format

`$TERMINAL_FENSTER_CONFIG_DIR/quickmarks`, tab-separated, one per line, `#` comments:

```
# name        url
gh            https://github.com
h             https://news.ycombinator.com
docs          https://doc.rust-lang.org/std/
i             https://internal.corp.example/dashboard
```

Names are `[A-Za-z0-9._-]+`. A duplicate name is an error reported by `doctor`, and the first
occurrence wins so a broken file can never make the browser unusable.

### 12.3 The two access paths

Quickmark names are arbitrary strings, so they cannot all live in a chord trie. Two paths, and the
file feeds both:

1. **Single-character names become real bindings.** Any quickmark whose name is one character is
   synthesised into the normal-mode trie as `' <c>` at load time. `' g` is two keystrokes to
   GitHub, dispatched by the same resolver as everything else. This is what "quick" should mean.
2. **Every quickmark is in the palette** under the `*` sigil, so long names stay reachable by
   fuzzy search: `:*docs`.

`M` (`quickmark.set`) opens a one-row prompt pre-filled with a suggested name derived from the
eTLD+1, and appends to the file. **The file is rewritten atomically** — write to a temporary file
in the same directory, then `rename(2)` — because a half-written quickmarks file after a crash is
a corrupted config the user has to debug, and B09 already establishes staging-and-rename as the
house pattern for exactly this reason.

Synthesised bindings are **weaker than explicit config**: if the user binds `' g` in
`[keys.normal]`, that wins and `doctor` reports the shadowed quickmark. Silent shadowing is the
kind of thing that costs someone an afternoon.

### 12.4 Profiles

Quickmarks are global by default, because they are keybindings and muscle memory should not change
when the profile does. A profile may add `$TERMINAL_FENSTER_CONFIG_DIR/quickmarks.<profile>`, merged
over the global set. Private sessions read but never write, mirroring B09 §14's rule for
bookmarks.

---

## 13. Config file format

### 13.1 The decision: a strict TOML subset, hand-parsed

The workspace declares exactly two dependencies — `libc` and `flate2` (`Cargo.toml:10-12`) — and
`tf-proto` hand-rolls its JSON reader with the comment that "a full JSON parser is a dependency and
an attack surface we do not need here" (`crates/tf-proto/src/lib.rs:122-124`). Adding `serde` +
`toml` for a config file would pull in roughly a dozen transitive crates and contradict a posture
the codebase has already argued for in writing.

So: **a strict subset of TOML, parsed in ~190 lines of `std`-only Rust.** Subset, not a new
language, because it means editors already highlight it, users already know it, and if the config
ever outgrows the subset the escape hatch is dropping in a real TOML crate with **zero migration
for users** — every file already written stays valid.

Supported: `#` comments (including trailing), `[table]` and `[table.sub]` headers, bare and quoted
keys, string / integer / boolean values, and flat string arrays. Not supported: multi-line
strings, inline tables, arrays of tables, dates, floats. A file using them gets a precise error,
not a wrong parse.

### 13.2 Location

```
$TERMINAL_FENSTER_CONFIG          # explicit file, wins over everything
$XDG_CONFIG_HOME/terminal-fenster/config.toml
~/.config/terminal-fenster/config.toml           # default
```

Plus `quickmarks` in the same directory.

This deliberately **differs from B09's data root** (`~/Library/Application Support/Terminal-Fenster`).
Config is user-authored, hand-edited and dotfile-managed; data is machine state. Terminal users
keep the former in `~/.config` and expect to symlink it — Ghostty and kitty both read
`~/.config/<app>/`, and `~/.config/ghostty` exists on this machine. Making a terminal-native tool
hide its config in `Application Support` would be a small, daily irritation.

### 13.3 Failure policy

**A broken config never blocks startup.** Every unparseable line is skipped, recorded, and the
rest of the file applies:

```rust
let c = Config::parse("[keys.normal]\nthis is not valid\nf = \"hint.follow\"\n");
assert_eq!(c.errors.len(), 1);
assert_eq!(c.str("keys.normal.f"), Some("hint.follow"));   // the good line still applied
```

Errors surface in the status band on startup (`config: 1 problem — run :doctor`) and in full from
`terminal-fenster doctor`. A browser that refuses to start because of a typo in a keybinding is a
browser the user cannot use to look up what they typed wrong.

### 13.4 The default config, in full

```toml
# ~/.config/terminal-fenster/config.toml
# Every value here is the default. Delete anything you do not change.

[general]
start_page       = "about:blank"
mode_timeout_ms  = 500     # ambiguous-prefix wait, e.g. `g` when `g g` also exists
esc_timeout_ms   = 50      # legacy-terminal lone-ESC wait (A06 2.8); ignored under kitty keyboard
insert_grace_ms  = 250     # focus within this long after a user action enters insert mode
smooth_scroll    = true

[hints]
alphabet        = "asdfjklewiocmp"   # 14 chars: 2.35 keys average at 291 targets
match_text      = true               # typing a non-alphabet char filters by label text
uppercase       = false              # render labels uppercase (easier to spot, same keys)
selector_extra  = []                 # extra CSS selectors, e.g. ["[data-testid]"]
scroll_into_view = true              # retry occluded targets by scrolling them clear

[palette]
max_results   = 10
sources       = ["actions", "tabs", "history", "bookmarks", "quickmarks"]
history_limit = 2000

[keys.normal]
# --- navigation
f = "hint.follow"
F = "hint.follow_new_tab"
o = "omnibox.open"
O = "omnibox.open_new_tab"
H = "nav.back"          # not alt+left: Option is a compose key on macOS (A06 2.7)
L = "nav.forward"
r = "nav.reload"
R = "nav.reload_hard"
"g u" = "nav.up"
"g U" = "nav.root"

# --- scrolling
j = "scroll.down"
k = "scroll.up"
d = "scroll.half_down"
u = "scroll.half_up"
space = "scroll.page_down"
b = "scroll.page_up"            # NOT shift+space: shift+space IS space in legacy (A06 2.5)
"g g" = "scroll.top"
G = "scroll.bottom"

# --- tabs
t = "tab.new"
x = "tab.close"
X = "tab.reopen"
"g t" = "tab.next"
"g T" = "tab.prev"
"g 0" = "tab.first"
"g $" = "tab.last"

# --- palette, find, help
":" = "palette.open"
"ctrl+p" = "palette.open"       # NOT ctrl+shift+p: shift is unrecoverable in legacy (A06 2.6)
"/" = "find.open"
n = "find.next"
N = "find.prev"
"?" = "help.keys"

# --- clipboard (OSC 52, A06 6)
"y y" = "yank.url"
"y t" = "yank.title"
"y m" = "yank.markdown"
p = "paste.open"

# --- quickmarks
M = "quickmark.set"
# `' <c>` for every one-character quickmark is synthesised from the quickmarks file

# --- modes
i = "mode.insert"
"ctrl+v" = "mode.passthrough"

[keys.insert]
escape   = "mode.normal"
"ctrl+[" = "mode.normal"        # ctrl+[ IS Escape in legacy encoding; harmless to bind both

[keys.hint]
escape    = "hint.cancel"
backspace = "hint.backspace"
tab       = "hint.cycle"

[keys.palette]
escape = "palette.cancel"
enter  = "palette.accept"
"ctrl+n" = "palette.next"
"ctrl+p" = "palette.prev"
# NOT ctrl+j: it is LF (0x0A), which input.rs:220 folds into Enter -- it would accept, not
# advance. This is the exact class of bug `doctor` exists to catch.

[keys.passthrough]
"ctrl+]" = "mode.normal"        # the only key passthrough intercepts
```

**Every binding above is verified deliverable on a terminal with no kitty keyboard protocol.**
Not asserted — tested: `default_config.toml` is `include_str!`'d into `power_proto.rs` and
`every_default_binding_survives_a_legacy_terminal` runs the whole shipped keymap through the same
predicate `doctor` uses. The first draft of this table failed it twice (§15.1).

### 13.5 Grammar of the subset

```
file    := line*
line    := ws* (comment | table | pair)? ws* NL
comment := '#' <to end of line, unless inside a string>
table   := '[' ws* key ('.' key)* ws* ']'
pair    := key ws* '=' ws* value
key     := bare | quoted            ; bare = [A-Za-z0-9_-]+ , quoted = "…" with \" \\ \n \t
value   := quoted | integer | 'true' | 'false' | '[' (quoted (',' quoted)*)? ']'
```

Keybinding keys are quoted whenever they contain a space, which sequences always do. A bare key
with a space is rejected with a message that says what to do:

```rust
let c = Config::parse("[keys.normal]\ng g = \"scroll.top\"\n");
assert!(c.errors[0].contains("quote it"));
```

`#` inside a string is not a comment (tested), which matters the moment somebody stores a URL with
a fragment.

### 13.6 Reload

`config.reload` (and `:reload-config`) re-reads config and quickmarks, rebuilds the keymap and
swaps it atomically. No restart, no engine involvement — the keymap lives entirely in the core.
Errors from a reload replace the previous error set rather than accumulating.

---

## 14. Keymap resolution semantics

Implemented and tested in `power_proto.rs`.

**Sequences.** Keys accumulate into a pending buffer. Each key re-runs `resolve`:

```rust
match keymap.resolve(mode, &pending) {
    Resolve::Action(a)          => { run(a); pending.clear(); }
    Resolve::Prefix             => { /* wait; arm which-key at mode_timeout_ms */ }
    Resolve::Ambiguous { action } => { /* arm timer: fire `action` at mode_timeout_ms */ }
    Resolve::NoMatch            => { pending.clear(); if mode.forwards_to_page() { forward_all(); } }
}
```

**Ambiguity.** When a sequence is both a complete binding and a prefix of a longer one (`g` bound,
`g g` also bound), the timer decides — vim's `timeoutlen`, default 500 ms. Any key arriving first
cancels the timer and extends the sequence.

**No-match forwarding.** In normal mode an unmatched sequence is dropped. In insert and
passthrough it is forwarded to the page **in full and in order** — a partially-consumed prefix
must not be swallowed, or typing `gg` into a text field would lose a `g`.

**Unbinding.** An empty action string removes a binding, which is how a user disables a default:

```toml
[keys.normal]
x = ""       # stop closing the tab by accident
```

**Precedence.** Defaults, then config file, then quickmark-synthesised bindings — with the
important exception that a quickmark never overrides an explicit config binding (§12.3).

**Modes are independent tables.** A binding in `[keys.normal]` has no effect in hint mode. Tested:
`resolve(Mode::Insert, "f")` is `NoMatch` even though `f` is bound in normal mode.

---

## 15. `terminal-fenster doctor --keys`

### 15.1 An alias is not automatically a defect

My first version of this check was too blunt, and writing the test for §13.4 proved it. It
reported three dead defaults; only two were real.

`ctrl+[` arrives as `Escape` on a legacy terminal (A06 §2.6). In `[keys.insert]` both are bound to
`mode.normal`, so a user pressing `ctrl+[` gets exactly what they wanted. That is an **alias**, and
reporting it as broken would train people to ignore the tool. Meanwhile `shift+space` arrives as
`space`, which is bound to the *opposite* action, and `ctrl+j` arrives as `Enter` — which in the
palette means it would **accept the highlighted entry instead of moving down**.

So the model is not "can this chord be encoded" but "what does this chord actually arrive as, and
is that bound to something else":

```rust
pub fn legacy_alias(&self) -> Option<KeySpec>   // None = arrives as itself
pub fn legacy_expressible(&self) -> bool { self.legacy_alias().is_none() }
```

A binding is reported only when its alias is bound to a **different** action in the same mode.
`ctrl+j`'s alias comes from our own code, not the terminal: `input.rs:220` maps both `\r` and `\n`
to `KeyCode::Enter`, so `ctrl+j` (LF, `0x0A`) is folded into Enter before the keymap ever sees it.

### 15.2 Output

Real diagnostics, produced by the tested implementation:

```
$ terminal-fenster doctor --keys
terminal: Apple Terminal 465 — kitty keyboard: NO

2 bindings will not do what you meant on this terminal:
  normal    shift+space   scroll.page_up
      `shift+space` is indistinguishable from `space` — but `space` is bound to
      `scroll.page_down`. Use a plain key such as `b`.
  palette   ctrl+j        palette.next
      `ctrl+j` is indistinguishable from `enter` — but `enter` is bound to
      `palette.accept`. Use `ctrl+n`.

1 alias, working as intended:
  insert    ctrl+[  ->  escape, also bound to mode.normal

1 quickmark is shadowed by an explicit binding:
  ' g  ->  keys.normal (quickmark `g` -> https://github.com is unreachable)
```

Under Ghostty (kitty keyboard YES) the same config reports nothing, because everything works
there — asserted by `doctor_is_silent_under_kitty`. This is the difference between "my keybinding
doesn't work and I don't know why" and a two-line explanation with a fix.

---

## 16. Protocol additions (specification for the commander)

Following B04 §8's conventions. **Additive; nothing existing changes.** Unknown fields are already
ignored by `Status::apply_event` (`apps/cli/src/main.rs:725`), so an older core keeps working
against a newer engine.

### 16.1 Commands (core → engine, type 10)

| Command | Payload | Behaviour |
|---|---|---|
| `hints.collect` | `{"t":"hints.collect","id":<tab>,"token":<u32>,"selectors":["…"]}` | Run the isolated-world collector on the main frame plus every cross-origin frame; reply `hints`. `token` correlates the reply and lets the core drop a stale one. |
| `hints.watch` | `{"t":"hints.watch","id":<tab>,"on":true}` | Install/remove the scroll + mutation observers (§8). |
| `hints.activate` | `{"t":"hints.activate","id":<tab>,"ref":<u32>,"how":"click"\|"dom"\|"hover"\|"focus"}` | `click` uses the existing synthetic-mouse path. `dom` is the occlusion fallback and must pass `userGesture: true`. |
| `page.focus_emulation` | `{"t":"page.focus_emulation","id":<tab>,"on":true}` | §4.2. On for the active tab, off for background tabs. |
| `page.eval_isolated` | `{"t":"page.eval_isolated","id":<tab>,"code":"…","token":<u32>}` | The generic primitive the above are built on. **Core-internal only — never reachable from page content or from a URL.** |

### 16.2 Events (engine → core, type 2)

| Event | Payload |
|---|---|
| `hints` | `{"t":"hints","tab":n,"token":u32,"vw":w,"vh":h,"items":[{"ref":u32,"x":..,"y":..,"w":..,"h":..,"cx":..,"cy":..,"clickable":bool,"kind":"link"\|"edit"\|"click","label":"…","href":"…"}]}` |
| `hints.stale` | `{"t":"hints.stale","tab":n,"reason":"scroll"\|"mutation"}` |
| `focus` | `{"t":"focus","tab":n,"editable":bool,"tag":"INPUT"}` |

`x/y/cx/cy` are **top-level viewport CSS pixels**, composition already applied (§5.5). The core
divides by cell size and never thinks about frames.

**Payload size.** 19,751 bytes measured for 397 items. That is one-quarter of a single 54 KB page
frame and it crosses a local socket, so it needs no compression — but the core must cap `items`
(suggest 500) so a pathological page cannot make one message enormous, consistent with B06's IPC
hardening.

---

## 17. Engine changes (described, not applied)

`apps/engine/src/main.js` is the commander's file. This is the shape of the change, for review.

```js
// --- one-time, after the window exists -------------------------------------------
// Focus emulation. Without this, focus events never fire under OSR (D07 §4.2), which
// breaks automatic insert mode, :focus-visible, and any page that waits on focus.
async function enableFocusEmulation(wc, on) {
  if (!wc.debugger.isAttached()) wc.debugger.attach('1.3');
  await wc.debugger.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: on });
}

// --- hint collection --------------------------------------------------------------
const HINT_WORLD = 1000;                     // fixed id; worlds do not share globals (D07 §5.3)

async function collectHints(wc, token, selectors) {
  const code = buildCollector(selectors);    // §5.4
  const main = await wc.executeJavaScriptInIsolatedWorld(HINT_WORLD, [{ code }]);
  const items = main.items;

  // Tier 2 only when a cross-origin frame actually exists (D07 §5.5).
  const origin = wc.mainFrame.origin;
  const foreign = wc.mainFrame.framesInSubtree.filter((f) => f.origin !== origin);
  for (const f of foreign) {
    const sess = await sessionForFrame(wc, f);          // Target.setAutoAttach, cached
    if (!sess) continue;
    const w = await wc.debugger.sendCommand('Page.createIsolatedWorld',
                { frameId: sess.frameId, worldName: 'bg-hints' }, sess.sessionId);
    const r = await wc.debugger.sendCommand('Runtime.evaluate',
                { expression: code, contextId: w.executionContextId, returnByValue: true },
                sess.sessionId);
    const off = await frameRectInTopViewport(wc, f);    // compose + clip (D07 §5.5)
    for (const it of r.result.value.items) {
      it.x += off.x; it.y += off.y; it.cx += off.x; it.cy += off.y;
      if (it.y > main.vh || it.y + it.h < 0 || it.x > main.vw || it.x + it.w < 0) continue;
      items.push(it);
    }
  }
  sendEvent({ t: 'hints', token, vw: main.vw, vh: main.vh, items: items.slice(0, 500) });
}
```

Four review notes:

1. `debugger.attach` throws if DevTools are open or if it is already attached — `isAttached()`
   first, and treat a throw as non-fatal (hints degrade to main-frame-only; focus emulation is the
   part that must be loud if it fails).
2. Sessions must be evicted on `Target.detachedFromTarget` and on navigation, or the next cycle
   evaluates against a dead frame.
3. `collectHints` is `async` while `handleCommand` (`main.js:226`) is synchronous. Either make the
   case `void collectHints(...)` and rely on the `token` for correlation, or make the dispatcher
   async — the former is smaller and the token already exists for exactly this.
4. Per B04, every one of these takes an optional `id` addressing a tab, defaulting to the active
   one.

---

## 18. Test plan

CI-able without a screenshot, which A03 already requires.

| Id | Test | Pass |
|---|---|---|
| **D07-1** | `power_proto.rs` suite | 25/25 (currently green) |
| **D07-2** | Labels prefix-free for every `n` in 1..=600 | property holds (in suite) |
| **D07-3** | Labels match the k-ary Huffman oracle | exact match at 12 values of `n` (in suite) |
| **D07-4** | Hostile-page fixture: collect in the isolated world | ≥ 290 items, none with a negative coordinate; main world is **not** used |
| **D07-5** | Collection latency on the 306-link fixture | median < 5 ms (measured 1.83) |
| **D07-6** | Occluded link is marked `clickable:false` | `elementFromPoint` returns the occluder |
| **D07-7** | Synthetic click on a collected rect navigates | `did-navigate` to the expected URL |
| **D07-8** | OOPIF hints appear with composed coordinates | child link found; `poisonVisible:false`; `x ≈ 203.59` |
| **D07-9** | Focus emulation on → `focusin` fires; off → does not | asserts both directions, so a regression is caught either way |
| **D07-10** | `default_config.toml` is `include_str!`'d and every shipped binding run through the alias model | zero bindings whose legacy alias is bound to a different action. **This test caught two real defects in §13.4** |
| **D07-11** | Broken config line does not prevent the rest applying | 1 error recorded, good binding live |
| **D07-12** | Quickmarks file with a duplicate name | first wins, `doctor` reports it, startup succeeds |
| **D07-13** | A page title of `"\x1b]52;c;<b64>\x07"` in the palette | zero escape bytes reach the tty (A09's sanitizer) |
| **D07-14** | Hint mode + navigation | hints cancelled, no synthetic click delivered afterwards |

D07-4/5/6/7/8/9 are the probe scripts with assertions instead of `console.log` — the fixtures
already exist.

---

## 19. Open items and UNVERIFIED claims

| # | Item | Status |
|---|---|---|
| U1 | Everything here was measured **only** in Ghostty-equivalent conditions and headless Electron. **No terminal rendering of a palette or hint overlay has been drawn on a real screen** — the machine is at a lock screen. C06's `PAGE_Z` float capability (its U1) gates the visual result for both. If that probe fails, hints and palette must fall back to C06's band-based list. | **UNVERIFIED — highest risk in this document** |
| U2 | `Emulation.setFocusEmulationEnabled` measured on Chromium 150 only. It is a stable CDP method but not a public Electron API, so a Chromium bump could change it. D07-9 asserts both directions, so a regression fails CI rather than silently disabling insert mode. | Measured, pinned by test |
| U3 | `win.focus()` did not steal macOS focus in one observation, while the machine was at a lock screen. Not relied upon — §4.2 recommends focus emulation instead. | UNVERIFIED, avoided |
| U4 | Deeply nested cross-origin frames (A inside B inside C) were not tested; only one level. Composition is recursive by construction but unproven past depth 1. | UNVERIFIED |
| U5 | CDP session cost was measured with one OOPIF. A page with 30 ad iframes pays ~2.19 ms each — 65 ms, which exceeds the frame budget. **Recommend capping tier-2 frames (suggest 8, largest-area first) and reporting the truncation** rather than stalling. | Extrapolated, not measured |
| U6 | Palette scoring performance is argued from bounds, not benchmarked. | UNVERIFIED |
| U7 | OSC 52 clipboard for `y y` inherits A06 §6's constraints, including terminals that refuse writes. Untested here. | Inherited |
| U8 | Whether `debugger.attach` measurably changes rendering performance was not benchmarked. It should be checked against the existing 60 fps baseline before shipping, since the debugger will now be attached for the whole session. | **UNVERIFIED — worth measuring early** |

---

## 20. Licenses

Rule 4 check. **No third-party code is copied into Terminal-Fenster by this design.**

| Project | License | Checked | Use here |
|---|---|---|---|
| Vimium | **MIT** | `api.github.com/repos/philc/vimium` → `spdx_id: MIT` | Interaction model only (`f` to hint, home-row labels). MIT would permit reuse with attribution; the label generator in `power_proto.rs` is an independent k-ary-tree construction verified against a Huffman oracle, not a port. |
| qutebrowser | **GPL-3.0** | fetched `LICENSE` → "GNU GENERAL PUBLIC LICENSE Version 3" | **Do not copy any code or config.** Referenced only for two *behavioural* precedents: passthrough mode, and not auto-entering insert mode on page load. |
| Tridactyl | `NOASSERTION` via the GitHub API | checked | Not used. Treat as unlicensed until someone reads the repository. |
| Electron | MIT (`node_modules/electron/package.json`) | already a dependency | API use only |

CDP method names and semantics are from the Chrome DevTools Protocol, which is an interface, not
code, and every claim about it here was verified by running it (§1.2) rather than by quoting docs.

---

## 21. Instructions for the commander

**Do first, before any D07 feature work:**

1. **Turn on `Emulation.setFocusEmulationEnabled` for the active tab.** This is a standalone
   engine bug fix worth more than everything else in this document: without it, `:focus`,
   `:focus-visible`, autofocus and every focus handler on the web are wrong in Terminal-Fenster today,
   and nobody would connect the symptom to the cause. ~10 lines. Enable on tab activate, disable
   on deactivate.
2. **Measure U8** — whether a permanently attached debugger moves the 60 fps baseline. Everything
   in §16 assumes it does not.
3. **Run C06's U1 float probe.** Both the palette and hint overlays are floats over `PAGE_Z`. If
   the z-threshold does not behave, both need C06's band-based fallback, and that changes the UI
   before it is built rather than after.

**Then, in order:** the action registry (§10.5) first, because the keymap, palette and agent API
all read from it; then the config parser and keymap (`power_proto.rs` lifts almost directly into a
`bg-config` crate with no new dependencies); then hints; then the palette; then quickmarks.

**Do not** implement hint positioning with `DOM.getBoxModel`, and do not collect hints in the main
world. Both were measured to be wrong here — one is 5.9–11.5× slower for strictly less
information, the other returns whatever the page decides to tell you.

**One row for C06:** the command palette needs a layer-table entry — text, float, centred,
input-blocking, priority 85, between the find bar (80) and permission prompts (90).

---

## 22. Summary

Link positions come from **one injected script in an isolated world**, not `DOM.getBoxModel`:
measured 1.83 ms versus 10.77–21.01 ms, returning visibility, occlusion and label text that the
CDP route cannot provide at all. The isolated world is not a preference but a trust boundary — a
hostile page reduced the main-world collector to zero links and forced a cross-origin frame to
report every coordinate as `-9`. Cross-origin frames need `Target.setAutoAttach {flatten:true}`
and cost 2.19 ms each, paid only by pages that actually have them.

The keymap is shaped by what a terminal can physically deliver: no default uses `ctrl+shift`,
because that chord does not survive Apple Terminal, and `doctor` tells users when their own
bindings do not either. Hint labels are provably optimal and prefix-free — 2.35 keystrokes average
on a 291-target page. The palette, the keymap and the agent API all read one action registry, which
is what makes shortcuts discoverable: you find `f` next to "Follow link" in the palette rather than
in a manual.

And underneath all of it sits the finding that was not in the mission brief: offscreen Chromium
never believes it is focused, so focus events never fire, so a modal browser cannot know when to
start typing. One CDP call fixes it.
