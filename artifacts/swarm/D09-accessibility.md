# D09 — Accessibility, text mode, and the low-bandwidth path

**Mission:** specify a text-mode rendering driven by the Chromium accessibility tree
(`Accessibility.getFullAXTree`), screen-reader-friendly structured output, keyboard-only
navigation of all chrome, contrast/focus requirements, and the exact text layout algorithm.
This doubles as the low-bandwidth mode.

**Status:** specification. No core source was modified. Every change to `crates/`,
`apps/cli/`, or `apps/engine/src/main.js` is *described* here for the commander, per the
file-ownership rule.

**Provenance:** all CDP numbers in this document were measured on this machine on
2026-07-31 against the engine's own Electron build —
`electron 43.2.0`, `chrome 150.0.7871.129`, `node 24.18.0`, `v8 15.0.1240245-electron.0`,
`icu 78.2`, `unicode 17.0` — with `webPreferences.offscreen = true` and the same hardening
flags used by `apps/engine/src/main.js:106-113`. Reproduction recipes are in §1.1.

---

## 0. Executive summary

Text mode is not a degraded fallback. It is the same browser rendered through a different
projection, and on the wire it is **two orders of magnitude cheaper than pixels**:

| Path | Bytes for one Ghostty viewport (146×23 cells, 2482×851 px) |
|---|---|
| Kitty graphics, full frame | **53,999** (measured, project brief) |
| Text mode, full viewport | **627 – 969** (measured, §1.3) |

That is a **56–86× reduction**, before any damage tracking. On a 1 Mbps SSH uplink the pixel
path costs ~432 ms per frame (2.3 fps); the text path costs ~8 ms. This is the argument for
building text mode, and accessibility is what makes it *correct* rather than a lossy
screen-scrape.

Four measured results shape the design, and each contradicts the obvious approach:

1. **`Accessibility.getFullAXTree` costs 1,273 ms and 6.03 MB on a 16,007-node page.** The AX
   tree is *larger than the pixel frame it replaces*. Shipping the tree to the terminal core
   would make "low-bandwidth mode" slower than pixels. **Layout must run in the engine and
   only laid-out lines may cross the socket.** (§3.1, F8)
2. **There is no push invalidation.** `Accessibility.nodesUpdated` never fired across two
   independent probes, even with the domain enabled and the DOM mutated. Re-fetching *does*
   return fresh data. The design needs an explicit, budgeted invalidation policy. (F2, §3.3)
3. **Block structure cannot be derived from role names.** `<p>` is exposed as `paragraph`
   normally but collapses to an *ignored* `none` when block-fragmented. The rule that
   actually holds, verified across nine isolation cases: **a node is a block box iff it has
   at least one direct inline child.** (F6, §4.3) This single rule is what makes the layout
   algorithm exact.
4. **A background-swap focus indicator is mathematically incompatible with a semantic colour
   palette on a dark terminal.** On a `#0b0b0c` background, a focus background meeting the
   3:1 non-text minimum forces every text colour above L≥0.6705; 5 of 8 palette entries fall
   below it. The focus indicator must be **marker-and-underline based, not background
   based**. (F9, §7.4)

**Single most actionable recommendation:** implement the engine-side extractor and the
§4 layout algorithm behind `terminal-fenster open --text`, and gate it on the §9 golden-fixture
tests. The reference implementation already runs; it produced every worked example in §4.12.

---

## 1. Evidence base

### 1.1 How these numbers were produced

Eight probes were run against the engine's own Electron binary. They live in the session
scratchpad (ephemeral):

```
/private/tmp/claude-501/-Users-builder/<session>/scratchpad/
    ax-probe.js    capability + first tree + big-tree cost
    ax-probe3.js   invalidation, lean APIs, focus, scroll  (watchdog-guarded)
    ax-probe4.js   iframes, depth param, geometry cost
    ax-probe5.js   div/span/pre/blockquote/dl role mapping
    ax-probe6.js   nine-case isolation of the paragraph-role instability
    ax-probe7.js   torture fixture capture
    ax-probe8.js   16k-node page capture
    layout.js      reference implementation of §4
    contrast.js    WCAG validator
    solve2.js      focus-indicator feasibility proof
```

Invocation (the Chromium sandbox must be disabled *at the agent-shell level*; the Chromium
sandbox itself stays on inside the probe, matching `main.js:111`):

```bash
cd $REPO
./apps/engine/node_modules/.bin/electron <probe>.js --out=<result>.json
```

The minimal probe core, sufficient to reproduce every CDP result below:

```js
const win = new BrowserWindow({ show: false, width: 1280, height: 800,
  webPreferences: { offscreen: true, nodeIntegration: false,
                    contextIsolation: true, sandbox: true, webSecurity: true } });
const wc = win.webContents;
await new Promise(r => { wc.once('did-finish-load', r); wc.loadFile(page); });
wc.debugger.attach('1.3');                       // AFTER first load -- see F1
const tree = await wc.debugger.sendCommand('Accessibility.getFullAXTree', {});
```

**Two probes are recommended for adoption into `tests/e2e/` as regression fixtures**
(commander's call — I did not write into `tests/`): `ax-probe6.js`, because it pins the
block-classification rule the whole layout algorithm rests on, and `ax-probe8.js`, because
it pins the performance budget.

### 1.2 The CDP accessibility surface, measured

| Question | Measured answer |
|---|---|
| Does `Accessibility.getFullAXTree` work under OSR? | **Yes.** 127 nodes on the fixture page. |
| Does it require `Accessibility.enable`? | **No.** 127 nodes without it (34 ms cold); 127 with it (5 ms warm). Still works after `Accessibility.disable` (35 nodes on probe-3 page). |
| Can `enable` be called before the first load? | **No — it never resolves.** 8 s timeout on both `Accessibility.enable` and `Page.enable`. See F1. |
| Does `Accessibility.nodesUpdated` fire on DOM mutation? | **No.** 0 events across two probes. See F2. |
| Is a re-fetch after mutation fresh? | **Yes.** `h1` name went `"Primary heading"` → `"Changed heading"`. |
| Does the tree cross iframe boundaries? | **No.** Outer tree = 15 nodes, 1 `RootWebArea`, `Iframe` node `"Frame A"` with `childIds: []`. See F3. |
| Can a sub-frame be fetched? | **Yes.** `getFullAXTree {frameId}` → 12 nodes containing `"Inner heading"`, `"inner link"`. Frame ids from `Page.getFrameTree`. |
| Does the `depth` parameter work? | **Yes.** depth 1 → 5 nodes, 2 → 8, 3 → 11. |
| Is focus readable from the tree? | **Yes.** Exactly one node carried `focused: true` — `role: textbox`, `name: "Field one"`, `backendDOMNodeId: 11`. |
| Does synthetic `Tab` move focus? | **Yes.** `sendInputEvent({type:'keyDown',keyCode:'Tab'})` moved `INPUT#i1` → `BUTTON#b1`. |
| Can an AX node be resolved to a pixel rect? | **Yes.** `DOM.getBoxModel {backendNodeId}` → quad `[163.98,149.88, 229.31,149.88, 229.31,167.88, 163.98,167.88]`, 65×18. **0.6 ms/node** (50 nodes in 30 ms). |
| Scroll position and document extent? | `Page.getLayoutMetrics` → `cssLayoutViewport {clientWidth:1280, clientHeight:800, pageY:2435}`, `cssContentSize {width:1280, height:3235}`. `DOM.scrollIntoViewIfNeeded` moved `scrollY` to 2435. |

### 1.3 Scaling measurements

Fixture: 2,000 `<p>` each containing a link and wrapping prose → **16,007 AX nodes**.

| Stage | Measurement |
|---|---|
| `Accessibility.getFullAXTree` | **1,273 ms**, **6,033,403 bytes** raw JSON |
| Lean projection (drop `name.sources`, `chromeRole`, flatten) | **1,745,607 bytes** (3.46× smaller) |
| §4 layout @ 80 cols | **326.1 ms** → 6,001 lines |
| §4 layout @ 146 cols | **178.6 ms** → 4,001 lines |
| Render to bytes | 15.6 ms (80 col) / 24.1 ms (146 col) |
| Whole text document | 183,790 bytes |
| **One 23-row viewport slice** | **627 bytes** (80 col) / **969 bytes** (146 col) |

Smaller fixture (127 nodes, the semantic-rich probe page): full tree 13,866 B → lean 3,327 B
(4.17×); whole rendered document **506 bytes at 80 columns**.

`DOMSnapshot.captureSnapshot` on the same 16k page: **147 ms, 746,191 bytes, 8,013 layout
nodes, 2 documents** — 8.6× faster and 6.1× smaller than `getFullAXTree`, *and* it crosses
iframes and carries geometry. It does **not** carry roles, computed names, or states, so it
is not a substitute for accessibility semantics. It is the right companion for geometry
(§3.3) and a plausible fast path for a future "reader mode".

Bulk geometry alternative: one `Runtime.evaluate` returning `getBoundingClientRect()` for all
in-viewport focusable elements returned **18 rects in 11 ms / 270 bytes**. Compare 12,000
nodes × 0.6 ms = 7.2 s for per-node `DOM.getBoxModel`. **Never walk the tree calling
`getBoxModel`.**

### 1.4 Role and property vocabulary, measured

These are Blink internal role strings, not pure ARIA. Capitalised names are Blink-specific.
Full histogram from the 127-node fixture:

```
RootWebArea 1   none 12   banner 1     main 1        contentinfo 1  navigation 1
heading 2       paragraph 3  form 1    table 1       list 2         image 1
StaticText 30   alert 1   Iframe 1     link 4        strong 1       emphasis 1
LabelText 2     searchbox 1  combobox 1  checkbox 1  button 2       caption 1
rowgroup 1      row 3     listitem 5   InlineTextBox 30   generic 1  MenuListPopup 1
cell 4          ListMarker 5  option 2  columnheader 2
```

Additional roles observed on the probe-5 page: `blockquote`, `DescriptionList`, `term`,
`definition`, `LineBreak`.

Every `properties[].name` observed, with frequency:

```
focusable 11   url 6      level 7     labelledby 4   live 1    atomic 1   relevant 1
invalid 5      editable 4 settable 1  hasPopup 1     expanded 1
checked 1      disabled 1 selected 2  readonly 2     required 2
```

Role → property map (what you may actually rely on being present):

| Role | Properties carried |
|---|---|
| `RootWebArea` | `focusable`, `url` |
| `heading` | `level` (integer 1–6) |
| `link` | `focusable`, `url` |
| `listitem` | `level` |
| `image` | `url` |
| `searchbox` / `textbox` | `invalid`, `focusable`, `editable`, `settable`, `labelledby`; **`value.value` carries the text** |
| `combobox` | `invalid`, `focusable`, `hasPopup`, `expanded`, `labelledby`; `value.value` |
| `checkbox` | `invalid`, `focusable`, `checked`, `labelledby` |
| `button` | `invalid`, `focusable`, `disabled` |
| `option` | `focusable`, `selected` |
| `columnheader` | `readonly`, `required` |
| `alert` | `live`, `atomic`, `relevant` |
| `table` | `labelledby` |

`ignoredReasons` values observed: `uninteresting`, `labelFor`, `presentationalRole`,
`notRendered`. 12 of 127 nodes were `ignored: true`, all with `role: none`.

Confirmed correct-by-default behaviour: `<img alt="">` produced **no** node (decorative images
vanish), and `display:none` / `aria-hidden="true"` text was absent from the tree. Text mode
inherits Chromium's correctness here for free.

---

## 2. Findings

### F1 — CRITICAL: `Accessibility.enable` never resolves before the first document load

`wc.debugger.sendCommand('Accessibility.enable')` and `'Page.enable'` both timed out at 8,000
ms when issued against a freshly created offscreen `BrowserWindow` that had not yet loaded a
document. Probe 2 hung indefinitely and had to be `pkill`ed; probe 3 reproduced it with a
watchdog and captured the timeouts explicitly.

Curiously, `Page.enable` **took effect anyway** — `Page.frameStartedNavigating`,
`domContentEventFired`, `loadEventFired` and `frameStoppedLoading` all arrived afterwards.
So the domain is enabled but the command response is never delivered. Any implementation that
`await`s these calls at startup will deadlock.

*Root cause is UNVERIFIED.* The behaviour is consistent with no renderer process existing yet
for a `show:false` offscreen window that has never navigated, leaving the command queued.

**Required:** attach the debugger and enable domains only after the first
`did-finish-load`. Never `await` a domain-enable during engine bootstrap. Give every
`sendCommand` a timeout; a hung CDP call must not be able to wedge the engine.

### F2 — CRITICAL: there is no push invalidation for the accessibility tree

`Accessibility.nodesUpdated` fired **zero** times in both probes, including with the domain
successfully enabled (probe 1), a listener attached before the mutation, and
`Accessibility.getRootAXNode` already called. A DOM mutation that demonstrably changed the
tree (`h1` name became `"Changed heading"` on the next fetch) produced no event.

*Why is UNVERIFIED.* In DevTools, `nodesUpdated` is tied to the incremental
`getRootAXNode`/`getChildAXNodes` streaming mode; it is plausible that a client which only
calls `getFullAXTree` is never subscribed. I could not confirm this from the running build,
and I am not going to assert a mechanism I did not observe.

**Consequence:** text mode cannot be event-driven off the accessibility domain. It needs the
explicit invalidation policy in §3.3. This is the single largest piece of design forced on us
by measurement rather than preference.

### F3 — HIGH: `getFullAXTree` does not cross iframe boundaries

Measured: a page with one `<iframe srcdoc>` yielded a 15-node tree with exactly **one**
`RootWebArea`, and the `Iframe` node had `childIds: []`. The inner heading and link were
absent. Fetching with `{frameId}` from `Page.getFrameTree` returned the inner 12 nodes.

**Required:** the extractor must walk `Page.getFrameTree`, fetch per frame, and splice each
sub-tree in at its `Iframe` node. A single-call implementation silently drops all iframe
content — which on real sites means embedded video, comment widgets, payment fields, and
consent dialogs. Silent omission of a consent dialog is a correctness *and* a trust problem.

Splice key: match the frame's owner. The `Iframe` AX node carries `backendDOMNodeId`;
`DOM.describeNode {backendNodeId}` returns `frameId` for a frame owner element. (The
describeNode → frameId link is **UNVERIFIED**; I verified only that per-frame fetch works and
that `Page.getFrameTree` enumerates the frames.)

### F4 — HIGH: the flat `nodes` array is not in reading order

Measured directly: DFS from `RootWebArea` visited all 127 nodes (no orphans), but the flat
array order is **not** monotonic in DFS order. Concatenating `StaticText` names in array
order produced:

```
" Primary headingSome body text with an  and  and .Second level…Footer contentinline linkboldemphasis…"
```

— "inline link", "bold" and "emphasis" belong in the *first* paragraph but land after the
footer. Correct DFS traversal produced:

```
"• Home• Docs• AboutPrimary headingSome body text with an inline link and bold and
 emphasis.Second levelParagraph two…"
```

**Required:** always traverse `childIds` depth-first from the `RootWebArea`. Never iterate
`nodes[]`.

### F5 — HIGH: `StaticText` and `InlineTextBox` double-count every character

Every `StaticText` node has `InlineTextBox` children carrying the *same* text; neither is
`ignored`. Measured on the fixture: naive concatenation of all non-ignored `name` values =
**909 characters**; the correct `StaticText`-only concatenation = **357**. A 2.5×
duplication that a casual implementation will not notice because the output still "looks like
the page".

`InlineTextBox` is not junk — it is Blink's post-layout line fragmentation (a `<pre>` produced
three boxes: `"  preformatted"`, `"\n"`, `"  second line"`). But that fragmentation reflects
the **1280 px pixel viewport**, not the terminal's column count, so it is useless for our
re-wrap and must be dropped.

**Required:** drop `InlineTextBox` unconditionally during pruning. They are also identifiable
by negative `nodeId` (observed: `-1000000008`, `-1000000035`), but drop by role — the negative
id is an implementation detail I would not rely on.

### F6 — HIGH: block structure cannot be derived from role names

This is the finding that makes the layout algorithm specifiable. Probe 6 isolated it across
nine cases on the same page, mutating only `document.body.innerHTML`:

| Input | Resulting role |
|---|---|
| `<p>plain text</p>` | `paragraph` |
| `<p>with <span>span</span> tail</p>` | `paragraph` + 3 `StaticText` (inline `<span>` produces **no node**) |
| `<p>with <span style="display:block">blocky</span> tail</p>` | **`none` (ignored)**, with `StaticText`, ignored-`none`, `StaticText` as children |
| `<p></p><p>after</p>` | **`none` (ignored)**, then `paragraph` |
| `<div>divtext</div>` | `generic` |
| `<div><div>inner</div></div>` | outer **`none` (ignored)**, inner `generic` |
| `<p>a</p><div><p>b</p></div>` | `paragraph`, ignored `none`, `paragraph` |
| `<main><p>in main</p></main>` | `main` > `paragraph` |
| `<ol><li>first<ul><li>nested…` | `list` > `listitem` > `ListMarker` + `StaticText` + nested `list` |

So `paragraph` is **not** a reliable signal: block-fragmenting a paragraph (a `display:block`
child) or emptying it demotes it to an ignored `none`. Any classifier keyed on a
`BLOCK_ROLES` set mis-renders the third and fourth cases — merging a fragmented paragraph's
text into its neighbours.

**The rule that holds across all nine cases:**

> A node is a **block box** if and only if it has at least one *direct* child that is
> inline-classified. Otherwise it is **transparent** (structural passthrough), regardless of
> whether it is `ignored`, `none`, `generic`, or `paragraph`.

Verification against the awkward cases: the fragmented `<p>` becomes ignored-`none` with
`StaticText` children → block ✓, and its ignored-`none` inner span also has a `StaticText`
child → block ✓, yielding three lines (`with` / `blocky` / `tail`) which is exactly what the
browser renders. The pure `<div><div>` wrapper has no inline children → transparent ✓. The
empty `<p>` has no children → transparent, emits nothing ✓.

Inline-classification must be an explicit set that takes precedence, or a `<p>` containing a
`<a>` would classify the link as a block (the link has a `StaticText` child) and break it onto
its own line. Sets are in §4.3.

### F7 — MEDIUM: `name.sources` is roughly three-quarters of the payload

Every named node carries a `name.sources` array enumerating every *candidate* naming source
tried (`aria-labelledby`, `aria-label`, `title`, `contents`), including superseded ones. On
the 16k-node page the full payload is 6,033,403 B and the lean projection is 1,745,607 B —
**71% of the bytes are provenance we never use**.

`Accessibility.getChildAXNodes` returns nodes **without** `sources` (verified:
`hasSources: false`); `getPartialAXTree` **does** include them (`hasSources: true`).

**Required:** project to the lean shape in the engine before anything crosses the socket.
Since §3.1 keeps the tree in the engine entirely, this is a memory and GC concern rather than
a wire concern — but it is a 4.3 MB one on a large page and worth doing at parse time.

### F8 — MEDIUM: the accessibility tree is larger than the pixel frame it replaces

Measured, same page, same moment: kitty-encoded full frame **53,999 B** (project brief);
lean AX tree **1,745,607 B**. A "low-bandwidth mode" that ships the tree to the terminal is
**32× worse than sending pixels**.

This inverts the naive architecture and is the direct justification for §3.1: **layout runs
in the engine; only laid-out lines cross the socket** (627–969 B per viewport).

### F9 — HIGH: a background-swap focus indicator cannot coexist with a semantic palette

Proven numerically, not asserted. On page background `#0b0b0c` (L = 0.0034):

- SC 1.4.11 requires the focus indicator ≥ **3:1** against the page background
  → focus background needs **L ≥ 0.1101**.
- SC 1.4.3 requires text on that focus background ≥ **4.5:1**
  → text needs **L ≥ 0.6705**, where the maximum possible is 1.0 (pure white).

Against the candidate palette, only near-white survives:

| Entry | Colour | L | Survives a 3:1 background swap? |
|---|---|---|---|
| body | `#d6d6d6` | 0.6724 | survives (barely) |
| heading | `#ffffff` | 1.0000 | survives |
| widget | `#ffd479` | 0.6973 | survives |
| link | `#7fb8ff` | 0.4601 | **lost** |
| linkVisited | `#c6a0ff` | 0.4437 | **lost** |
| alert | `#ff9f9f` | 0.4856 | **lost** |
| marker | `#9aa0a6` | 0.3476 | **lost** |
| chrome | `#8a8a8a` | 0.2542 | **lost** |

An exhaustive search over the RGB cube for *any* focus background satisfying both constraints
simultaneously with the full palette returned **zero candidates** on dark and **zero** on
light. At the minimal grey that clears 3:1 (`#5e5e5e`, ratio 3.03), even body text fails at
4.46:1.

**Required:** the focus indicator is **marker + underline**, not a background swap. §7.4.

### F10 — MEDIUM: text mode must set both foreground *and* background on every cell it owns

We cannot know the user's terminal theme. `caps.rs` does not currently query OSC 10/11
(`crates/tf-term/src/caps.rs:128-193` probes kitty graphics, DA1, kitty keyboard, `CSI 14 t`,
`CSI 16 t`, and DECRQM 1016 — no colour query). Without knowing the background, no contrast
claim we make is true.

Two mitigations, and I recommend both: add OSC 10/11 to detection so `doctor` can *report*
the theme, and — independently — have text mode emit an explicit background with every
foreground, so the computed ratios in §7.3 hold regardless of theme or of whether the query
answered. The second is what actually guarantees the property; the first is diagnostics.

---

## 3. Architecture

### 3.1 Where layout runs — the decision

**Layout runs in the engine (Node). The terminal core receives laid-out lines, never the
tree.**

Forced by F8: the tree is 1.75 MB lean, a viewport of laid-out text is ~1 KB. The alternative
— ship the tree, lay out in Rust — is 32× worse than the pixel path it is meant to beat, and
over SSH at 1 Mbps costs 14 s before the first character appears.

The honest costs of this decision, stated plainly:

- The layout algorithm lands in JavaScript in `apps/engine/`, away from the Unicode machinery
  in `crates/tf-term/src/unicode.rs`. Width measurement gets implemented twice. **Mitigation:**
  §9 defines golden fixtures as plain text files, so both implementations can be tested
  against identical expected output, and a divergence is a test failure rather than a
  mystery.
- Scrolling and resizing need a round trip. **Mitigation:** over-scan. Ship the viewport plus
  ±2 viewports of lines (~5 KB); local scrolling inside the over-scan is instant, and the
  socket is touched only when it is exhausted. This is the standard virtualised-list pattern.
- The engine must learn the terminal's cell grid. It currently receives only pixel geometry
  (`resize` at `apps/engine/src/main.js:232-238`).

### 3.2 Wire protocol additions (described, not implemented — commander owns these files)

The existing framing is unchanged: `[u8 type][u32 BE len][payload]`, types 1/2/10 per
`crates/tf-proto/src/lib.rs:11-13`. Text mode needs no new type ids — it is one new command
and one new event kind, both JSON.

**Core → engine, `T_COMMAND` (10):**

```jsonc
{ "t": "textmode", "on": true,
  "cols": 146, "rows": 23,          // terminal grid, needed for line breaking
  "first": 0,                        // first line index wanted
  "count": 115,                      // viewport + overscan (rows * 5)
  "verbosity": "normal" }            // "compact" | "normal" | "verbose"  (§5)
```

```jsonc
{ "t": "textnav", "action": "focusNext" }   // §6; engine translates to real input
```

**Engine → core, `T_EVENT` (2):**

```jsonc
{ "t": "textdoc",
  "gen": 47,                         // generation; core discards stale replies
  "total": 4001,                     // total laid-out lines (for the scrollbar)
  "first": 0,
  "lines": [ { "i": 0, "s": [ [0,"# Big page"] ] }, … ],
  "focus": { "line": 12, "col": 4, "len": 9, "role": "link",
             "name": "link 3", "hint": "sd" } }
```

Each line's `s` is a run list `[styleId, text]`. `styleId` indexes a small enum
(`body|heading|link|visited|widget|label|chrome|marker|alert|code`), which keeps the payload
near the measured 627–969 B/viewport instead of inlining colour strings.

**Why not a new binary type:** the payload is small, low-rate, and benefits enormously from
being readable in logs — exactly the asymmetry `tf-proto` already documents at
`crates/tf-proto/src/lib.rs:7-9`. Consistency with the existing design beats novelty.

### 3.3 Acquisition and invalidation policy

F2 removed the option of being event-driven. The policy below is a budget, not a heuristic
pile:

**Hard refetch triggers** (always, full re-extraction):

| Trigger | Existing hook |
|---|---|
| `did-stop-loading` | `apps/engine/src/main.js:121` (already emits `loading:false`) |
| `did-navigate` / `did-navigate-in-page` | `main.js:118-119` |
| `Page.frameStoppedLoading` for any sub-frame | needs `Page.enable` (after first load — F1) |
| terminal resize with changed `cols` | new `textmode` command |

**Soft refetch** (cheap change-detect first, then refetch only if changed):

After any input event that could mutate the DOM, schedule a coalesced check at +150 ms and
+600 ms. The check is **not** a tree fetch — it is a ~10 ms fingerprint:

```js
// measured sibling: a comparable bulk Runtime.evaluate returned in 11 ms
const fp = await wc.executeJavaScript(`(()=>{
  const b=document.body;
  return b.innerText.length + ':' + b.getElementsByTagName('*').length
       + ':' + document.activeElement?.tagName + ':' + Math.round(scrollY);})()`);
```

Refetch only when the fingerprint differs. This turns a 1,273 ms worst case into a 10 ms
no-op in the common case. The fingerprint is deliberately coarse — it can miss an in-place
text substitution of identical length. That is an accepted, documented miss, bounded by the
idle poll below.

**Idle poll:** while text mode is foreground and the page has a live region (`role=alert`,
`status`, or any node with a `live` property — measured present on `alert`), poll the
fingerprint every 2 s. Otherwise do not poll at all.

**Node budget.** If a fetch returns more than **8,000 nodes**, the page is classed *oversized*:
re-fetch is suppressed except on hard triggers, and the status line reports
`text: 16007 nodes (capped)`. Rationale: 8,000 nodes ≈ 640 ms extraction on the measured
1,273 ms / 16,007 node curve, which is the largest stall tolerable on a keystroke.

**Geometry** is acquired separately and only for what needs it: one bulk `Runtime.evaluate`
returning rects for in-viewport focusables (**measured: 18 rects, 11 ms, 270 B**), refreshed
on scroll and focus change. Per-node `DOM.getBoxModel` is **banned** outside single-node use
(0.6 ms × 12,000 = 7.2 s).

### 3.4 Budgets

| Stage | Budget | Measured basis |
|---|---|---|
| Change fingerprint | ≤ 20 ms | 11 ms for a comparable bulk evaluate |
| Full extraction, typical page (≤1,000 nodes) | ≤ 120 ms | 1,273 ms @ 16,007 nodes, linear |
| Full extraction, capped (8,000 nodes) | ≤ 700 ms | as above |
| Layout, typical | ≤ 30 ms | 178.6 ms @ 16,007 nodes / 4,001 lines |
| Viewport serialise | ≤ 5 ms | 15.6–24.1 ms for the whole 6,001-line document |
| **Keystroke → updated viewport, warm** | **≤ 60 ms** | fingerprint + slice, no refetch |
| **Navigation → first text, typical** | **≤ 250 ms** | engine ready 212 ms is the existing floor |

---

## 4. The text layout algorithm

This section is the deliverable's core. It is specified to be **exact and deterministic**: the
same tree and the same `cols` must produce byte-identical output on any implementation. A
reference implementation exists (`scratchpad/layout.js`) and produced every example in §4.12.

### 4.0 Contract

```
layout(frames: AXFrame[], cols: u16, opts: Options) -> TextDoc

TextDoc {
  lines:   Line[]                 // the whole document, not just the viewport
  anchors: (NodeId | null)[]      // anchors[i] is the AX node owning lines[i]
  hints:   Hint[]                 // { label, nodeId, role }, assignment order stable
}
Line { indent: u16, cells: Cell[] }
Cell { ch: Cluster, style: StyleSet, nodeId: NodeId, focusable: bool }
```

Invariants the implementation must uphold, each independently testable:

- **I1** For every `i`, `indent + Σ width(cells[j].ch) ≤ cols`. No line ever exceeds the grid.
- **I2** `lines.length == anchors.length`.
- **I3** Concatenating `cells[].ch` over all lines, with soft-wrap points replaced by a single
  space, reproduces the DFS `StaticText` concatenation modulo whitespace normalisation.
  (This is the "no text was lost or duplicated" property, and it is what catches F5.)
- **I4** `layout(t, c)` is pure: no dependence on wall clock, iteration order of a hash map,
  or locale.
- **I5** Every node with `focusable: true` reachable in the tree appears in exactly one
  `hints` entry.

### 4.1 Phase A — acquire and splice frames

```
frames := Page.getFrameTree()                       # F3
for each frame f (depth-first):
    t[f] := Accessibility.getFullAXTree({frameId: f.id})
root := t[main].RootWebArea
for each Iframe node n in every tree:
    child := the tree whose owner element is n.backendDOMNodeId
    if child exists: n.childIds := [ child.RootWebArea.nodeId ]
```

Splicing at the `Iframe` node preserves reading order: the sub-document lands exactly where
the frame sits in the parent's flow. A frame that fails to fetch (cross-origin restriction,
navigation in flight) leaves its `Iframe` node childless; §4.8 then renders it as
`[frame: <title>]` rather than silently emitting nothing.

### 4.2 Phase B — prune

Drop, unconditionally:

- `role ∈ {InlineTextBox, MenuListPopup}` — F5, and popups are not in flow.
- nodes unreachable from `RootWebArea` via `childIds` (measured: none on the fixture, but a
  malformed tree must not produce orphan text).

Do **not** drop `ignored: true` nodes. F6 proves they carry inline children that must be
rendered. `ignored` affects *classification* (they never become semantic decorations), not
*retention*.

### 4.3 Phase C — classify

```
INLINE_LEAF = { StaticText, ListMarker, LineBreak }

INLINE_BOX  = { link, strong, emphasis, image, button, checkbox, radio, switch,
                searchbox, textbox, combobox, spinbutton, slider, option, code,
                superscript, subscript, time, LabelText, menuitem, menuitemcheckbox,
                menuitemradio, tab, progressbar, meter, abbr, mark }

TABLE       = { table, grid, treegrid }

classify(n):
    if role(n) ∈ INLINE_LEAF: return INLINE_LEAF
    if role(n) ∈ INLINE_BOX:  return INLINE_BOX
    for c in directChildren(n):                       # F6 -- the measured rule
        if role(c) ∈ INLINE_LEAF ∪ INLINE_BOX: return BLOCK
    return TRANSPARENT
```

The explicit sets take precedence over the structural test; without that, a `link` (which has
a `StaticText` child) would classify as `BLOCK` and break onto its own line mid-sentence.

Unknown/future roles fall through to the structural test, which is the safe default: an
unrecognised container with text in it becomes a block, an unrecognised wrapper stays
transparent. The algorithm degrades gracefully as Chromium adds roles.

### 4.4 Phase D — box tree and anonymous blocks

```
walk(n, ctx):
    if role(n) ∈ TABLE:  layoutTable(n, ctx); return
    switch classify(n):
      TRANSPARENT:
          ctx' := ctx
          if role(n) == list:       ctx'.listDepth += 1     # increment EXACTLY here
          if role(n) == blockquote: ctx'.quote     += 1
          for c in children(n): walk(c, ctx')
      INLINE_LEAF | INLINE_BOX:
          renderBlock(n, [n], ctx)                          # stray inline -> own block
      BLOCK:
          pending := []
          for c in children(n):
              if classify(c) ∈ {INLINE_LEAF, INLINE_BOX}: pending.push(c)
              else: flush(pending); walk(c, ctx)            # ctx unchanged -- see note
          flush(pending)
```

`flush` emits `renderBlock(n, pending, ctx)` and clears. This is exactly CSS anonymous-block
behaviour: inline content interrupted by a block child is split into separate blocks around
it, which is what makes the fragmented-paragraph case (F6, row 3) render as three lines.

> **Implementation note, from a real bug.** `listDepth` must be incremented in the
> `TRANSPARENT` arm *only*. My first reference implementation also incremented it at the
> parent's dispatch, double-counting whenever a `list` was reached from a `BLOCK` parent — and
> `main` *is* a block (it had a direct `StaticText " "` child). Symptom: top-level list items
> indented by 2 columns while a visually identical nav list indented by 0. Fixed and
> re-measured; §4.12 output is post-fix. §9 has the regression vector.

### 4.5 Phase E — inline run collection

```
collectInline(n, ctx, out):
    if role(n) == LineBreak:  out.push({br: true}); return
    if role(n) ∈ INLINE_LEAF: out.push({text: name(n), style: ctx.style, nodeId: n.id}); return
    if role(n) ∈ INLINE_BOX:
        st := ctx.style
              + link      if role == link
              + bold      if role == strong
              + italic    if role == emphasis
              + code      if role == code
              + label     if role == LabelText
        w := widgetToken(n)
        if w != null: out.push({text: w, style: st+widget, nodeId: n.id,
                                focusable: prop(n,'focusable'), atomic: true}); return
        sub := []
        for c in children(n): collectInline(c, {style: st}, sub)
        if sub empty and name(n) != "": sub := [{text: name(n), style: st, nodeId: n.id}]
        for s in sub: s.owner := s.owner ?? n.id
                      s.focusable := s.focusable or prop(n,'focusable')
        out.append(sub); return
    for c in children(n): collectInline(c, ctx, out)      # transparent passthrough
```

`owner` is what makes a multi-run link a single focus target and a single hint: the runs carry
per-cell styling, the owner carries identity.

**Widget tokens.** Interactive controls render as self-describing atoms rather than as their
subtree, because a screen reader consuming a flat line needs the state inline:

| Role | Token | Source |
|---|---|---|
| `button` | `[ Go ]`, `[ Disabled (disabled) ]` | `name`, `properties.disabled` |
| `checkbox` | `[x] Enable thing` / `[ ] Enable thing` | `properties.checked` |
| `radio` | `(*) Label` / `( ) Label` | `properties.checked` |
| `searchbox`/`textbox` | `[hello   ]` (min 8 cells) | `value.value` |
| `combobox` | `[Beta v]` | `value.value` else `name` |
| `image` | `[img: An alt text image]` | `name`; **emits nothing when `name` is empty** |
| `Iframe` (unfetched) | `[frame: Frame A]` | `name` |

Widget tokens are **atomic**: never soft-wrapped internally (they may still be hard-broken if
they exceed the whole line — see §4.12's 20-column output, which is ugly but correct).

### 4.6 Phase F — whitespace normalisation

Applied per block, skipped entirely when `role == pre`:

1. Replace every run of `[\t\r\n ]+` with a single U+0020.
2. Strip leading spaces at the start of the block and immediately after a `br`.
3. Where run *k* ends with a space and run *k+1* begins with one, drop the second.
4. Strip trailing spaces at the end of the block; drop runs that become empty.
5. Widget tokens are opaque to all of the above.

This is CSS `white-space: normal` reduced to what a 1-D grid can express. Step 3 matters more
than it looks: the tree hands you `"Some body text with an "`, `"inline link"`, `" and "` as
separate nodes, and naive concatenation double-spaces at every boundary.

### 4.7 Phase G — line breaking

Greedy first-fit. Not Knuth–Plass: greedy is O(n), deterministic, and produces the same result
under incremental re-layout, which matters far more here than optical evenness.

**Width measurement.** `width(cluster)` in cells:

```
0  U+200B; combining marks (U+0300–U+036F, U+1AB0–U+1AFF, U+20D0–U+20F0);
   variation selectors (U+FE00–U+FE0F); C0 controls
2  U+1100–U+115F; U+2E80–U+A4CF; U+AC00–U+D7A3; U+F900–U+FAFF;
   U+FF00–U+FF60; U+FFE0–U+FFE6; U+1F300–U+1F64F; U+1F900–U+1F9FF
1  everything else
```

This is the East Asian Width table reduced to the ranges that matter. **It must agree
byte-for-byte with `crates/tf-term/src/unicode.rs`'s notion of width** — §9 has the shared
fixture. A zero-width cluster attaches to the preceding cell and never starts a line.

**Atoms.** Split each run at spaces, keeping spaces as separate atoms. Widget tokens are
single atoms flagged `atomic`.

**The loop:**

```
avail(lineIndex) := cols - (lineIndex == 0 ? indent : hangingIndent)

for atom a:
    if a.br:            flushLine(); continue
    if a.isSpace:
        if curWidth == 0:              continue        # no leading space on a wrapped line
        if curWidth + 1 > avail():     flushLine(); continue
        append(' '); curWidth += 1;    continue
    w := width(a)
    if curWidth + w <= avail():        append(a); curWidth += w; continue
    if w <= avail(nextLine):           flushLine(); append(a); curWidth = w; continue
    # overlong atom: no break opportunity fits -- hard-break at cluster boundaries
    flushLine()
    for cl in clusters(a):
        if curWidth + width(cl) > avail(): flushLine()
        append(cl); curWidth += width(cl)
```

Break opportunities are **spaces only** in v1. Deliberately not UAX #14: a full line-breaking
algorithm needs the break-property tables, and the measured benefit on the fixtures is
confined to hyphenated compounds and CJK. **CJK is the real gap** — CJK text has no spaces, so
it hard-breaks at cluster boundaries, which happens to be correct for Han but wrong for
kana-with-particles and wrong for the small set of characters forbidden at line start/end
(closing brackets, `。`, `、`). §10 lists this as the top v2 item.

### 4.8 Phase H — decoration and indentation

Computed per block before breaking; `prefix` occupies the first line, and continuation lines
are indented by `width(prefix)` so wrapped text aligns under the text, not under the marker.

| Block role | Prefix | Indent | Extra |
|---|---|---|---|
| `heading` level *n* | `#` × min(n,6) + space | 0 | blank line before **and** after |
| `paragraph` | — | 0 | blank line after |
| `listitem` | (the `ListMarker` node's own text — Chromium supplies `"• "`, `"1. "`, `"◦ "`) | `(listDepth-1) × 2` | hanging indent `+2` |
| `blockquote` descendants | `"> "` × quoteDepth | `quoteDepth × 2` | — |
| `alert` / `status` | `"! "` | 0 | — |
| `term` | — | 0 | — |
| `definition` | — | +2 | — |
| `pre` | — | 0 | whitespace preserved |

**Do not synthesise list markers.** Chromium already computes them, including ordinal numbers
and per-depth bullet variation — measured: `"1. "`, `"2. "` for `<ol>`, `"• "` for a top-level
`<ul>`, `"◦ "` for a nested one. Re-deriving them would fight CSS `list-style` and get
`<ol start>` wrong.

### 4.9 Phase I — tables

Tables are the one construct where a general block algorithm produces unusable output, so they
get a dedicated pass.

```
1. rows   := every `row` descendant, in DFS order (skip transparent rowgroups)
   cells  := children of each row with role ∈ {cell, columnheader, rowheader, gridcell}
   text   := plainText(cell)   -- Phases E+F, no breaking
2. ncols  := max |row|
   nat[j] := max over rows of width(text)                  -- natural width
   min[j] := max over rows of the widest single word       -- floor, never breached
3. gutter := 3   (" | ");  chrome := gutter × (ncols-1);  budget := cols - chrome
4. if Σnat ≤ budget:            w := nat                             -- fits, done
   elif Σmin ≤ budget:          distribute the deficit proportionally to slack
                                slack[j] := nat[j] - min[j]
                                w[j] := nat[j] - round(slack[j]/Σslack × deficit)
                                then repair rounding by trimming left-to-right,
                                never below min[j]
   else:                        STACKED MODE (below)
5. wrap each cell to w[j]; row height := max wrapped height; pad short cells
6. after the header row emit a rule: '-'×w[j] joined by '-+-'
```

**Stacked mode.** When even the minimum widths do not fit, a grid is a lie — the columns would
be 3 cells wide. Emit each row as an indented label/value list instead:

```
  Column one heading: alpha value here
  Column two heading: beta value here
```

This is also what a screen reader prefers, so the narrow case degrades toward the *more*
accessible rendering, not the less. The threshold is structural (`Σmin + chrome > cols`), not a
magic column count.

### 4.10 Phase J — assembly and the anchor map

Every emitted line pushes an entry to `anchors[]` carrying the AX `nodeId` of the block that
produced it (`null` for spacer lines). This single array carries all of:

- **focus → viewport**: the focused node (`properties.focused`, measured present and unique)
  maps to the first line whose anchor is that node, or the first line owned by a run whose
  `owner` is that node. Scroll so that line is visible.
- **viewport → page**: when the user switches back to pixel mode, the top visible anchor's
  `backendDOMNodeId` feeds `DOM.scrollIntoViewIfNeeded` so the pixel view lands where the text
  view was. (`scrollIntoViewIfNeeded` verified working: `scrollY` → 2435.)
- **hint activation**: a hint label resolves to a `nodeId`, which resolves to a
  `backendDOMNodeId`, whose rect comes from the bulk geometry call; a synthetic click at the
  rect centre goes through the existing `handleInput` path
  (`apps/engine/src/main.js:157-204`) with no new engine capability.

Finally, trim leading and trailing blank lines, and collapse any run of ≥2 blanks to one.

### 4.11 Determinism and complexity

- **Time:** O(N) classification + O(T) over total text. Measured 178.6 ms for 16,007 nodes /
  4,001 lines at 146 columns; 326.1 ms at 80 columns (more lines, more break decisions).
- **Space:** O(lines × cols) worst case for the cell grid. The 6,001-line document at 80
  columns rendered to 183,790 bytes; the in-memory cell representation is the thing to watch
  on oversized pages, and is another reason for the 8,000-node cap (§3.3).
- **Determinism:** the only ordering input is `childIds`, which CDP returns in document order.
  No hash-map iteration, no locale, no clock. I4 holds by construction.
- **Incrementality (v2):** because breaking is greedy and blocks are independent, a changed
  subtree only invalidates its own block's lines plus the `anchors` offsets after it. Not
  implemented in v1; the measured full-layout cost does not require it below the node cap.

### 4.12 Worked examples — measured reference output

All output below is verbatim from the reference implementation over a **real captured
`Accessibility.getFullAXTree` payload**, not hand-written.

**Fixture A** — the 127-node semantic page (headings, nav list, inline link/strong/em, form
with five controls, table with caption, list, image with alt, `role=alert`, footer).

At **80 columns** — 30 lines, **506 bytes**:

```
• Home
• Docs
• About

# Primary heading

Some body text with an inline link and bold and emphasis.

## Second level

Paragraph two. It has a fairly long run of prose so that wrapping behaviour can
be observed at a range of widths without contrivance.

Search query[hello   ]
Pick one[Beta v]
[x] Enable thing
[ Go ]
[ Disabled (disabled) ]

Small table
Name   | Qty
-------+----
Widget | 3
Gadget | 12

• list item one
• list item two
[img: An alt text image]
! Something happened
Footer content
```

Note the `<img alt="">` produced no output at all — Chromium never emitted a node for it.

At **20 columns** (stress), the overlong widget hard-breaks, which is correct per §4.7 and
deliberately not hidden:

```
[ Go ]
[ Disabled (disabled
) ]
```

**Fixture B** — the torture page (nested `<ol>`/`<ul>`, blockquote with two paragraphs,
definition list, CJK, emoji, an unbreakable URL, a 3-column table).

At **72 columns**:

```
# Torture fixture

1. first item
  ◦ nested one
  ◦ nested two
    1. deep
2. second item
  > A quoted paragraph that is long enough to need wrapping at narrow
    widths.

  > Second quoted para.

Term one
  Definition of term one which is reasonably long.
CJK: 今日は世界 你好世界 and emoji 😀🚀 then latin tail.

A very long unbreakable token:
https://example.com/a/very/long/path/that/will/not/fit/anywhere/at/all?q
=1&r=2

Wide table
Column one heading | Column two heading | Column three heading
-------------------+--------------------+---------------------
alpha value here   | beta value here    | gamma value here
delta              | epsilon            | zeta

## After table

Tail paragraph.
```

At **34 columns**, the column allocator narrows every column and wraps in place, and the CJK
line lands at exactly 34 cells (`"CJK: "`=5 + 10 + 1 + 8 + 1 + `"and emoji"`=9), confirming
the width table:

```
CJK: 今日は世界 你好世界 and emoji
😀🚀 then latin tail.

Wide table
Column    | Column    | Column
one       | two       | three
heading   | heading   | heading
----------+-----------+-----------
alpha     | beta      | gamma
value     | value     | value here
here      | here      |
delta     | epsilon   | zeta
```

Both fixtures at both widths satisfy I1 (no line exceeds `cols`).

---

## 5. Screen-reader-friendly structured output

A blind user is not running Terminal-Fenster to look at a picture of a browser. They are running a
screen reader over a terminal, and our output is what it speaks. Two consequences drive every
choice here: **the reader linearises, so the text must already be linear and unambiguous**,
and **the reader announces changes, so we must not churn the screen**.

### 5.1 Three sinks, one layout

| Sink | Use | Shape |
|---|---|---|
| **Live TTY** (`--text`) | interactive, screen reader attached | §4 output + §7 styling, damage-tracked |
| **Dump** (`terminal-fenster text <url>`) | pipe to a file, `grep`, an LLM, a braille display | §4 output, plain, no escape sequences, LF-terminated, UTF-8 |
| **Tree** (`terminal-fenster a11y <url>`) | debugging, conformance auditing, agents | §5.2 |

The dump sink is the one that must never regress: it is `cat`-able, diff-able, and is the
golden-fixture format in §9. When stdout is not a TTY, `--text` must produce exactly the dump
form — no colour, no cursor motion, no alternate screen.

### 5.2 The `a11y` tree dump

One node per line, two-space indentation per depth, deterministic field order:

```
RootWebArea "D09 Probe Page" url=file:///…/d09-probe.html focusable
  navigation "Main"
    list
      listitem level=1
        ListMarker "• "
        link "Home" url=/ focusable
  main
    heading "Primary heading" level=1
    paragraph
      text "Some body text with an "
      link "inline link" url=https://example.com/deep focusable
    form
      LabelText "Search query"
      searchbox "Search query" value="hello" focusable editable
      checkbox "Enable thing" checked focusable
      button "Go" focusable
      button "Disabled" focusable disabled
    table "Small table"
      columnheader "Name"
      cell "Widget"
    image "An alt text image"
    alert "Something happened" live=polite atomic
  contentinfo
```

Rules: `ignored` nodes are omitted (but their children are hoisted, per Phase B/C);
`StaticText` renders as `text "…"`; `InlineTextBox` never appears; properties appear in the
fixed order `url, level, value, checked, selected, expanded, disabled, readonly, required,
invalid, focusable, editable, live, atomic`. Fixed order is what makes the output diff-able
across runs and across Chromium versions.

This format is also the honest audit tool: if a page renders badly in text mode, `a11y` shows
whether the fault is ours or the page's. That distinction is worth having in bug reports.

### 5.3 Announcement model

Screen readers announce what changes. Uncontrolled repainting makes them unusable.

- **Never repaint a line whose content did not change.** Damage tracking is an accessibility
  requirement here, not only a bandwidth one.
- **Live regions.** Nodes carrying `live` (measured on `role=alert`: `live`, `atomic`,
  `relevant`) are rendered in place *and* mirrored into a one-line announcement band at the
  bottom of the screen, which is the only region allowed to change without user action.
  `live=assertive` replaces the band immediately; `live=polite` queues until the user is idle
  ≥500 ms.
- **Focus changes are announced by moving the terminal cursor** to the focused run's first
  cell. This is the mechanism screen readers already track, it costs ~8 bytes, and it works
  without the reader knowing anything about Terminal-Fenster.
- **Loading state** goes to the status band as text (`loading…` → `done`), never as a spinner.
  An animating spinner in a live region is a screen-reader denial-of-service.
- **The alternate screen buffer must not be used in dump mode**, so output survives in the
  scrollback the reader can review.

---

## 6. Keyboard-only navigation

### 6.1 Principles

1. **Every function reachable by mouse is reachable by keyboard.** No exceptions; this is
   WCAG SC 2.1.1 and it is also just what a terminal browser is *for*.
2. **No keyboard trap** (SC 2.1.2). From any state, `Esc` moves outward and eventually reaches
   page focus. This must be a tested invariant, not a promise.
3. **Focus order follows reading order** (SC 2.4.3), which §4.10's anchor map gives us for
   free because it is derived from the same DFS.
4. **Chrome focus is ours; page focus is Chromium's.** We never simulate page focus — we send
   real `Tab` (measured working: `INPUT#i1` → `BUTTON#b1`) and read the result back from the
   `focused` property. Simulating it would drift from the page's own focus handling.

### 6.2 The focus ring

Chrome inventory taken from C06 §3 so the two specs agree. `F6` cycles *regions*; `Tab` moves
*within* a region.

```
  ┌─ region ring (F6 / Shift-F6) ────────────────────────────────┐
  │  tab strip  →  omnibox  →  page  →  find bar (when open)     │
  │       ↑                                            │         │
  │       └────────────────────────────────────────────┘         │
  └──────────────────────────────────────────────────────────────┘
```

The **status/mode band and the security badge are not focusable** — they are output only.
Modals (permission prompts, C06 priority 90) **capture** the ring: while one is up, `F6` and
`Tab` cycle only within it, and `Esc` dismisses it as a deny. That is a deliberate,
bounded trap with exactly one documented exit, which SC 2.1.2 permits.

### 6.3 Keymap

Chrome-level, active whenever page focus is not inside a text field:

| Key | Action | Notes |
|---|---|---|
| `F6` / `Shift-F6` | next / previous region | the top-level ring |
| `Tab` / `Shift-Tab` | next / previous focusable **within** region | forwarded to page when region = page |
| `Ctrl-L` | focus omnibox, select all | |
| `Ctrl-F` | open find bar | floats, C06 §3 |
| `Esc` | close float → leave chrome region → clear selection | never a dead end |
| `Ctrl-T` / `Ctrl-W` | new / close tab | |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | next / previous tab | |
| `Alt-1`…`Alt-9` | tab by index | `Alt-9` = last tab |
| `Alt-←` / `Alt-→` | back / forward | existing `back`/`forward` commands, `main.js:245-254` |
| `Ctrl-R` | reload | existing `reload`, `main.js:242` |
| `f` | show link hints | labels from `hints[]`, alphabet `asdfghjkl` |
| `F` | show hints, open in new tab | |
| `Ctrl-Alt-T` | **toggle text mode** | the single entry point |
| `?` | keymap overlay | must itself be dismissible with `Esc` and `?` |

Text-mode navigation (active when text mode is on and page has region focus):

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | line down / up |
| `Ctrl-D` / `Ctrl-U` | half page |
| `Space` / `Shift-Space`, `PgDn` / `PgUp` | full page |
| `g` / `G` | document start / end |
| `n` / `p` | next / previous **focusable** |
| `]` / `[` | next / previous **heading** |
| `}` / `{` | next / previous **landmark** (`main`, `navigation`, `banner`, `contentinfo`, `region`) |
| `Enter` | activate focused node |
| `/` | find within the text document |
| `v` | verbosity cycle: compact → normal → verbose |

Heading and landmark jumps are the affordances screen-reader users actually navigate with, and
we get them almost free: the anchor map plus a role filter. Omitting them would make text mode
technically accessible and practically unusable.

### 6.4 Conflicts with existing specs

- **A06 §2.8 ESC ambiguity.** `Esc` is both our universal "back out" key and the lead byte of
  every CSI sequence. The keymap must consume `Esc` only via `input.rs`'s
  `flush_pending_escape()` path (`crates/tf-term/src/input.rs:148`), never on the raw byte.
- **D04 F1 (CRITICAL).** Terminal query replies on stdin are currently decoded as keystrokes.
  Text mode adds OSC 10/11 queries (F10), which makes that bug *more* likely to fire. D09
  depends on D04's fix; it does not work around it.
- **Kitty keyboard protocol.** `F6`, `Ctrl-Tab` and `Alt-<n>` are not reliably distinguishable
  in legacy encoding. Ghostty reports kitty keyboard support (measured, `caps.rs:163-165`), so
  the full keymap is available there. On Apple Terminal (no kitty keyboard, measured) the
  keymap must degrade to a documented subset, and `doctor` should say which keys are
  unavailable rather than letting them silently do nothing.

---

## 7. Contrast and focus requirements

### 7.1 Applicable success criteria

WCAG 2.2, with the terminal reading of each:

| SC | Level | Requirement | How it lands in a terminal |
|---|---|---|---|
| 1.4.1 Use of Color | A | colour is never the sole carrier of information | links need an underline, focus needs a glyph |
| 1.4.3 Contrast (Minimum) | AA | 4.5:1 for text | all terminal text is one size, so 4.5:1 applies everywhere — the large-text 3:1 exemption never applies to us |
| 1.4.6 Contrast (Enhanced) | AAA | 7:1 | target for the default dark palette |
| 1.4.11 Non-text Contrast | AA | 3:1 for UI components and graphics | focus indicator, table rules, scrollbar, list markers |
| 2.1.1 Keyboard | A | all functionality by keyboard | §6 |
| 2.1.2 No Keyboard Trap | A | focus can always leave | §6.2, tested |
| 2.4.3 Focus Order | A | order preserves meaning | anchor map is DFS-derived |
| 2.4.7 Focus Visible | AA | focus indicator is visible | §7.4 |
| 2.4.11 Focus Not Obscured | AA | focused element not entirely hidden | **see the C06 interaction below** |
| 2.4.13 Focus Appearance | AAA | ≥2px perimeter equivalent, ≥3:1 focused vs unfocused | §7.4 |

**SC 2.4.11 is a live conflict with C06.** C06 §3 floats the find bar over the page precisely
to avoid a resize round-trip, and C06 §14 already lists "a search hit scrolled underneath the
bar" as an open question. Under 2.4.11 that is not a nit, it is an AA failure. **Required
resolution:** any float that can cover page content must reserve a keep-clear rectangle around
the focused node's rect; if the focused rect intersects the float, the text view scrolls by the
overlap before the float is drawn. In text mode this is trivial — we own the line grid and
know the focused line index.

### 7.2 The terminal's background is unknown

Per F10, `caps.rs` does not query the terminal's colours. Two actions:

1. **Detection (describe-only, `caps.rs` is core-owned).** Add OSC 10 (`ESC ] 10 ; ? BEL`) and
   OSC 11 (`ESC ] 11 ; ? BEL`) to `detect()`, parsing the `rgb:RRRR/GGGG/BBBB` reply, storing
   `fg_rgb` / `bg_rgb` on `Capabilities`, and reporting both in `doctor` alongside the existing
   `raw_replies`. Absence of a reply is the negative result, exactly as the existing probes
   treat it (`caps.rs:9-10`).
2. **Rendering (the actual guarantee).** Text mode emits an explicit background with every
   foreground on every cell it owns. Then the ratios in §7.3 are properties of our output, not
   of the user's theme, and they hold whether or not the query answered.

Cost of always emitting background: with truecolor SGR, a style change is
`ESC[38;2;r;g;b;48;2;r;g;bm` ≈ 24 bytes. On the measured 969-byte viewport with ~10 style
changes per line over 23 lines, that is ~5.5 KB — still **10× cheaper than the 53,999-byte
pixel frame**. Run-length coalescing of identical styles (which the §3.2 run list already
gives us) keeps it near the low end.

Non-truecolor terminals (no `COLORTERM=truecolor`, `caps.rs:137`) get a 16-colour fallback
palette; §7.5's validator must be run against that palette too, and it will not reach AAA.

### 7.3 Verified palette

Computed with the WCAG relative-luminance formula and verified by `contrast.js`. All values
are AA-passing; the dark palette's body, heading and link are AAA.

**Dark** — page background `#0b0b0c`:

| Token | Colour | vs page bg | Required | |
|---|---|---|---|---|
| body | `#d6d6d6` | **13.54** | 4.5 | AAA |
| heading | `#ffffff` | **19.67** | 4.5 | AAA |
| link | `#7fb8ff` | **9.56** | 4.5 | AAA |
| link (visited) | `#c6a0ff` | **9.25** | 4.5 | AAA |
| widget | `#ffd479` | **14.00** | 4.5 | AAA |
| alert | `#ff9f9f` | **10.04** | 4.5 | AAA |
| chrome (rules, gutters) | `#8a8a8a` | **5.70** | 3.0 | non-text |
| marker (bullets, focus) | `#9aa0a6` | **7.45** | 3.0 | non-text |

**Light** — page background `#ffffff`:

| Token | Colour | vs page bg | Required |
|---|---|---|---|
| body | `#1c1c1e` | **17.01** | 4.5 |
| heading | `#000000` | **21.00** | 4.5 |
| link | `#0b4fbf` | **7.28** | 4.5 |
| link (visited) | `#6a2bb5` | **8.00** | 4.5 |
| widget | `#7a4b00` | **7.41** | 4.5 |
| alert | `#a01010` | **8.15** | 4.5 |
| chrome | `#5a5a5a` | **6.90** | 3.0 |
| marker | `#4a4a4a` | **8.86** | 3.0 |

Per SC 1.4.1, **links additionally carry SGR 4 (underline)** and visited links SGR 4:2 (double
underline) where supported, so link identity never depends on the blue.

### 7.4 The focus indicator — and why it is not a background swap

F9 proved the obvious design impossible. Restating the arithmetic because it is the load-bearing
part: on `#0b0b0c` (L = 0.0034), SC 1.4.11 forces the focus background to L ≥ 0.1101, which
then forces every text colour on it to L ≥ 0.6705. Of the eight palette entries, five sit
below that. An exhaustive RGB search for a focus background satisfying both constraints with
the full palette returned **zero** candidates on dark and **zero** on light.

**Therefore the focus indicator is composed of three channels, none of which is a background
swap:**

1. **Gutter marker.** `▸` in `marker` (`#9aa0a6`, **7.45:1** vs page bg — clears the 3:1
   non-text minimum with 2.5× headroom) in a reserved 2-cell left gutter, on the focused
   line. Where it was previously blank background, contrast between focused and unfocused
   states of the indicator area is that same 7.45:1, satisfying SC 2.4.13's 3:1.
2. **Underline.** SGR 4 across the focused run's cells. On a link, which is already underlined,
   focus escalates to SGR 21 (double underline) so the two states remain distinguishable.
3. **Terminal cursor.** Positioned at the focused run's first cell (§5.3). This is what screen
   readers track and what gives the indicator a real perimeter presence.

An optional decorative background tint may be applied *in addition*, and must be documented as
decorative — it is explicitly **not** the accessible indicator, and the palette above must
remain legible on it or the tint is dropped. This is the only honest way to have the pleasant
look without failing the criterion.

### 7.5 The validator

`contrast.js` is the reference; the shipped equivalent must run in CI, not by hand. It:

- computes WCAG relative luminance
  (`L = 0.2126·R + 0.7152·G + 0.0722·B` over linearised sRGB) and
  `ratio = (L_hi + 0.05) / (L_lo + 0.05)`;
- asserts every text token ≥4.5:1 and every non-text token ≥3:1 against **every** background
  it can legally appear on;
- asserts the focus indicator ≥3:1 against the page background;
- runs over the dark palette, the light palette, **and** the 16-colour fallback;
- fails the build on any violation.

It has already earned its place: it caught the F9 failure in the first palette I proposed, and
the exhaustive search then proved that palette unfixable rather than merely mis-tuned. A
hand-checked palette would have shipped the bug.

---

## 8. Low-bandwidth mode

Text mode *is* low-bandwidth mode; it needs no separate implementation, only a policy for when
it turns on.

| Path | Bytes / viewport | 1 Mbps SSH |
|---|---|---|
| Kitty full frame | 53,999 | 432 ms → 2.3 fps |
| Kitty damage patch (C08) | varies | — |
| **Text mode, styled** | ~5,500 | 44 ms |
| **Text mode, plain** | **627 – 969** | **8 ms** |
| Text mode, damage-tracked | ≪ above | — |

**Auto-engage policy.** `caps.rs` already detects `remote` from `SSH_CONNECTION`/`SSH_TTY`
(`caps.rs:136`). Text mode should be *offered*, never silently forced — a user who asked for a
browser and got a text dump will reasonably think it is broken. Concretely: when `remote` is
true and C09's measured throughput is below a threshold, the status band shows
`slow link — Ctrl-Alt-T for text mode`, and `--text` / `--no-text` override.

**The one case for forcing it:** `Backend::Unicode` with no truecolor. At that point the pixel
path is a low-fidelity half-block approximation with a palette constraint (C04), and text mode
is strictly better on every axis — fidelity, bytes, and legibility. Apple Terminal 465,
measured to support neither kitty graphics nor sixel nor truecolor querying, is exactly this
case.

---

## 9. Test vectors

All runnable without a terminal, which is the point — none of this needs the lock screen
opened or a TTY attached.

**T1 — block classification (F6).** For each of the nine probe-6 cases, assert the classifier
returns the expected class. This is the single most important test in the suite; if it
regresses, every page renders subtly wrong.

```
"<p>plain text</p>"                                  -> paragraph  = BLOCK
"<p>a <span>b</span> c</p>"                          -> paragraph  = BLOCK, span absent
"<p>a <span style=display:block>b</span> c</p>"      -> none(ign.) = BLOCK  (3 lines)
"<p></p><p>after</p>"                                -> none(ign.) = TRANSPARENT (0 lines)
"<div>t</div>"                                       -> generic    = BLOCK
"<div><div>t</div></div>"                            -> outer      = TRANSPARENT
```

**T2 — no text duplication (I3).** Over fixture A, assert the rendered document contains
exactly 357 characters of page text, not 909. Guards F5.

**T3 — reading order (F4).** Assert DFS text equals the golden string beginning
`"• Home• Docs• AboutPrimary headingSome body text with an inline link and…"`, and assert the
flat-array order does **not** equal it (so the test proves it is testing something).

**T4 — golden layout fixtures.** Fixture A at 80/40/20 columns and fixture B at 72/34 columns,
byte-compared against the §4.12 output. Because the dump sink is plain UTF-8, these are plain
`.txt` files and diff cleanly.

**T5 — line width invariant (I1).** Property test: for a corpus of trees and every
`cols ∈ [20, 200]`, no rendered line exceeds `cols` display cells. Catches the CJK and
emoji width errors that a Latin-only fixture never will.

**T6 — list depth regression.** Fixture A must render `• list item one` at indent **0**, not
2. This is the double-increment bug from §4.4, and it is exactly the kind of defect that
looks like a style choice until someone measures it.

**T7 — table narrowing.** Fixture B's 3-column table at 34 columns matches golden; at 12
columns it must switch to stacked mode.

**T8 — contrast.** §7.5's validator over all three palettes; zero violations.

**T9 — no keyboard trap.** Model the §6.2 region ring as a graph; assert every state reaches
page focus by some sequence of `Esc`, and that modal capture has exactly one exit.

**T10 — CDP contract guards.** Cheap regression tests that fail loudly when Chromium changes
under us: `getFullAXTree` returns >0 nodes without `enable`; a mutation followed by refetch
returns fresh data; an iframe page yields exactly one `RootWebArea`; `depth:1` returns fewer
nodes than `depth:3`.

**T11 — width table agreement.** The same UTF-8 corpus measured by the engine's JS width
function and by `crates/tf-term/src/unicode.rs`; any disagreement is a build failure. This is
the guard on the §3.1 duplication cost.

---

## 10. What I could not verify

Marked plainly rather than glossed.

- **Why `Accessibility.nodesUpdated` never fires (F2).** I observed the absence across two
  probes; I did not establish the mechanism. If it turns out to require the
  `getRootAXNode`/`getChildAXNodes` streaming mode, the §3.3 polling policy could be replaced
  with real events — a worthwhile follow-up probe, and it would remove the fingerprint hack.
- **Why `Accessibility.enable` hangs pre-load (F1).** The "no renderer yet" explanation is
  plausible and consistent with `Page.enable` taking effect without responding, but it is a
  hypothesis. The mitigation (attach after first load, always time out) is correct either way.
- **The `Iframe` node → `frameId` mapping (F3).** I verified per-frame fetch works and that
  `Page.getFrameTree` enumerates frames. I did **not** verify that
  `DOM.describeNode {backendNodeId}` returns the owning `frameId`. Probe before implementing
  the splice.
- **Real-world node counts.** I measured a 127-node hand-built page and a 16,007-node
  synthetic one. I could not fetch real sites — `example.com` is not in this environment's
  allowlist. The 8,000-node cap is therefore an interpolation on a synthetic curve, and should
  be re-tuned against ten real pages before it is treated as a constant.
- **iTerm2** remains unverified (TCC blocks automation, per the project brief). Nothing in
  this spec depends on iTerm2 specifics.
- **Screen-reader behaviour end-to-end.** I specified the announcement model from the WCAG
  criteria and from how readers consume terminal output; I did not run VoiceOver against
  Terminal-Fenster, and the machine is at a lock screen so I could not. §5.3 is design, not
  measurement, and should be validated with a real VoiceOver session before it is called done.
- **CJK line breaking (§4.7).** Space-only break opportunities are correct for Han but wrong
  for kana-with-particles and ignore the line-start/end prohibition classes. The measured
  output is legible, not correct. Top v2 item.
- **`caps.rs` OSC 10/11 support.** I did not probe the terminals for colour queries; §7.2's
  rendering guarantee is deliberately designed not to depend on the answer.
- **LICENSE.** The repository has no `LICENSE` file at root. Nothing in this spec reuses
  third-party code — the algorithm is original and WCAG is a specification, not an
  implementation — but the absence should be resolved before anything is vendored.

---

## 11. Recommendations to the commander

Ordered by value per unit of risk.

1. **Build the engine-side extractor + §4 layout behind `terminal-fenster open --text`.** This is
   the whole deliverable and the reference implementation already runs. Layout goes in the
   engine, not the core (F8); only laid-out lines cross the socket.
2. **Adopt T1 and T4 before writing the extractor.** The block-classification rule (F6) is the
   one thing that makes this algorithm exact rather than approximate, and it is derived from
   behaviour that could change with a Chromium bump. Pin it first.
3. **Fix the CDP lifecycle hazards now** — attach the debugger only after the first
   `did-finish-load`, and give every `sendCommand` a timeout (F1). This is a handful of lines
   in `apps/engine/src/main.js`, it prevents a hard hang, and it is worth doing even if text
   mode is deferred. My probe 2 hung on exactly this and had to be killed.
4. **Resolve the SC 2.4.11 conflict with C06** — floats must not obscure the focused element.
   C06 §14 already flags it as an open question; this spec upgrades it to an AA failure with
   a concrete fix (keep-clear rectangle, §7.1).
5. **Adopt the marker-based focus indicator (§7.4) everywhere, including pixel mode.** F9's
   infeasibility result is not text-mode-specific — it is a property of dark terminal
   backgrounds and applies to any focus highlight the project draws.
6. **Land `contrast.js` in CI (T8).** It caught a real failure in the first palette I wrote and
   then proved that palette unfixable. Hand-checked contrast will ship bugs.
7. **Ship `terminal-fenster a11y <url>` (§5.2) early.** It is roughly fifty lines on top of the
   extractor, it makes every subsequent text-mode bug diagnosable, and it is independently
   useful to anyone auditing a page.
8. **Re-probe the two open CDP questions** — `nodesUpdated` subscription semantics, and
   `describeNode → frameId` — before implementing invalidation and iframe splicing. Both could
   simplify the design; neither should be assumed.
