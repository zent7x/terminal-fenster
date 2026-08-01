# C06 — Compositor Design: merging page pixels with terminal-native chrome

**Mission:** C06. **Owner file:** `artifacts/swarm/C06-compositor-design.md` (this document only).
**Host:** macOS 26.1, Apple M4, arm64. **Date:** 2026-07-31.
**Status of evidence:** every claim about *our* code is cited to `file:line` and was read directly.
Every claim about *terminal* behaviour is cited to A04 (`[SPEC]`/`[SOURCE]`) or marked
**`[UNVERIFIED]`** with a probe recipe in §13. **No live terminal was available to this agent**
(`tty` returns `not a tty`), so nothing here claims visual confirmation it does not have.

---

## 0. The decision, in one paragraph

**There is exactly one pixel layer and one text layer. The page is the pixel layer; *all* chrome
is terminal text; the terminal emulator performs the composite.** We never blend pixels in our
process. This falls out of two facts in A04 that happen to compose perfectly: a kitty placement
with negative `z` is drawn *under* text (A04 §1.2), and `EL`/`ECH`/`ED 0`/`ED 1` are required not
to touch graphics (A04 §1.7) — so text chrome can be drawn, moved and erased over a resident page
image without ever retransmitting a pixel. The consequence is that "z-ordering" is really two
unrelated mechanisms: a **single, constant kitty z-index** separating page from chrome, and a
**cell-ownership arbiter** resolving chrome-vs-chrome entirely in our process before a byte is
emitted. The arbiter is a pure function, which matters a great deal here because this machine
cannot do visual verification — a pure function is testable in CI without a tty.

**Single most actionable recommendation** (expanded in §12): the page image is currently placed
with `z=0` and no `r=`, and the status bar is built with an unclamped `format!`. On the *already
verified* Ghostty geometry (146 columns) a page with a long title and URL produces a status line
of up to ~147–151 display columns, which wraps, scrolls the screen by one row, and desynchronises the
image placement from the cell grid. That is a live bug on the measured configuration, not a
hypothetical.

---

## 1. What is already true (grounding)

Read directly from the tree; not restated from the brief.

| Fact | Evidence |
|---|---|
| Page frame arrives as BGRA, 32-byte header + `w*h*4` pixels | `crates/tf-proto/src/lib.rs:16-54`; arithmetic check: `2482*814*4 + 32 = 8,081,424` — exactly the byte count in the brief |
| Wire reduction 8,081,424 → 53,999 B = **149.7×** | computed; matches the brief's "150×" |
| Ghostty geometry is cell-exact: 2482/17 = **146 cols**, 851/37 = **23 rows**, page 814/37 = **22 rows** | computed from A04's measured cell size 17×37 |
| Exactly **one** bottom row is reserved for chrome today | `apps/cli/src/main.rs:254` — `page_h = vp_h - cell_h` |
| The compositor already builds one buffer and does **one** `write`+`flush` | `apps/cli/src/main.rs:899-901` |
| `Placement` already carries `cols`, `rows`, `z`, `no_cursor_move` and encodes them | `crates/tf-term/src/kitty.rs:80-95`, encoded at `:137-148` |
| …but every caller uses `Placement::default()` → `cols=None, rows=None, z=0` | `apps/cli/src/main.rs:860`; default at `kitty.rs:93` |
| Page-derived text is sanitized (strips C0, DEL, C1, U+2028/9) before the tty | `crates/tf-term/src/unicode.rs:60-75`, tested `:126-142` |
| Three independent writers to the tty exist; A09 demands one | `artifacts/swarm/B01-architecture-rfc.md` §2.1 |
| Consumer currently ignores the dirty rect and re-encodes the full frame | `apps/cli/src/main.rs:848-880`; corroborated by B07:29 |
| The engine **discards** the dirty rect when coalescing (latent stale-region bug) | B07:12-13, 26, pointing at `apps/engine/src/main.js:96-97` |
| Teardown already deletes all images | `crates/tf-term/src/tty.rs:36` (`a=d,d=A`), tested `:233` |

Two of these are load-bearing for everything below: the placement mechanism is **built but
unused**, and damage-only transmit is **not yet safe** because the engine drops damage on coalesce.
This design therefore specifies the damage contract but recommends shipping full-frame mode until
B07's union fix lands.

---

## 2. Layer model

```
  ┌──────────────────────────────────────────────────────────────┐
  │ TEXT LAYER   (everything the user reads as glyphs)           │
  │   • reserved bands   — tab strip, omnibox, status, mode      │
  │   • floating overlays — find bar, hints, toasts, modals      │
  │   composited by: OUR cell-ownership arbiter (pure function)  │
  └──────────────────────────────────────────────────────────────┘
                              ▲  terminal draws text over image
                              │  (kitty z < 0)
  ┌──────────────────────────────────────────────────────────────┐
  │ PIXEL LAYER  (exactly one image, id 1000, placement 1)       │
  │   the page surface. Never contains chrome. Never scaled.     │
  └──────────────────────────────────────────────────────────────┘
```

There is no third layer and no compositing buffer. Adding one would mean reading back the page
RGB, alpha-blending chrome into it, and re-encoding — i.e. paying a full deflate + base64 of the
whole frame for a one-row status change. At the measured 53,999 wire bytes and 0.74 ms encode,
that is roughly **235× more expensive** than the ~230 bytes of text the change actually needs
(§8.4). The layer split is not stylistic; it is the difference between a chrome update costing
microseconds and costing a frame.

---

## 3. Per-element decision: pixels or text?

Priority is the arbiter's tie-break (§5.2); higher wins the cell. "Band" = carved out of the page
rectangle, unreachable by page pixels. "Float" = drawn over the page.

| Element | Layer | Placement | Prio | Why this choice |
|---|---|---|---|---|
| **Page surface** | **PIXEL** | band (the page band) | 0 | it *is* the content |
| **Omnibox / URL** | text | **band**, top | 10 | trust-critical: must be unreachable by page pixels (§9) |
| **Security / TLS badge** | text | **band**, fixed cells, right of omnibox | 10 | trust-critical; fixed position builds muscle memory |
| **Agent-control indicator** | text | **band** + whole-row recolour | 10 | trust-critical and must be *unmissable* (§7) |
| **Tab strip** | text | **band**, top | 10 | shows per-tab origin ⇒ trust-adjacent; crisp glyphs beat a 17×37 px bitmap |
| **Status / mode line** | text | **band**, bottom | 10 | always-visible, ~230 B, no reason to float |
| **Scrollbar** | text | float, right column | 50 | 1 column; cheap; conveys position the page can't |
| **Notification / toast** | text | float, corner, auto-dismiss | 60 | transient, low stakes, must not cost a page resize |
| **Link hints** | text | float, cell-quantised | 70 | must sit adjacent to targets at arbitrary page positions |
| **Grid-jump overlay** | text | float, whole page area | 75 | covering the page is its entire function |
| **Find bar** | text | float, bottom-anchored, 1 row | 80 | see below — a band would cost a resize round-trip |
| **Permission / modal prompt** | text | float, centred, input-blocking | 90 | must outrank everything, including hints |
| **Favicon** | **not drawn (v1)** | — | — | see below |

**Why the find bar floats rather than taking a band.** A band changes the page viewport, which
means a `resize` command to the engine, a Chromium reflow, and a forced full-damage frame
(B07:132-136 forces full damage on geometry change). That is the single most expensive event in
the system, incurred on every `^F` and every `Esc`. Floating costs one covered row of page content
and zero engine round-trips. The one real cost of floating — a search hit scrolled to underneath
the bar — is a genuine defect and is listed as an open question in §14.

**Why favicons are not pixels in v1.** Each favicon would be a separate kitty image plus a
placement that must be re-issued on every tab-strip repaint, and at a 17×37 cell a favicon is
visually unresolvable. That is N resident images, N placements, and z-tie-break complexity in
exchange for approximately no information. Use one high-contrast letter derived from the eTLD+1.
If favicons are ever revisited, they are the *only* legitimate candidate for pixels in chrome and
should be gated behind a measured win, not an aesthetic argument.

**Why hints are text despite quantisation error.** A text label can only be placed at cell
granularity, so at 17×37 a hint lands up to 16 px horizontally and 36 px vertically from its
target. This is acceptable because Vimium-style hinting is **label-driven, not position-driven**:
the user reads the label and types it, so the position only has to be close enough to *associate*
the label with its link. Drawing hints as pixels would require either re-encoding the page frame
(≈54 KB per hint toggle) or one transient image per hint (dozens of placements). Collision rule in
§5.3.

---

## 4. Geometry and the band budget

### 4.1 Adaptive band policy

Every reserved row costs `cell_h` pixels of page height — at 37 px that is 4.5% of an 814 px page
per row. Small terminals cannot afford what large ones should have.

| Terminal rows | Top band | Bottom band | Total chrome | Page rows |
|---|---|---|---|---|
| ≥ 40 | 2 (tab strip; omnibox+trust) | 2 (status; mode) | 4 | rows − 4 |
| 24 … 39 | 1 (tabs **merged into** omnibox) | 1 (status) | 2 | rows − 2 |
| 12 … 23 | 0 (omnibox becomes a `^L` modal float) | 1 (status, shows URL) | 1 | rows − 1 |
| < 12 | 0 | 1 | 1 | rows − 1, and `doctor` warns |

At 200×50 that is 46 page rows; at 80×24, 22 page rows. Note the current tree hardcodes exactly
one bottom row (`apps/cli/src/main.rs:254`), i.e. it implements the "12…23" case for all sizes.
Moving to this table is a change to a core file and is therefore **described here, not made**.

### 4.2 The page must never be scaled — a correctness requirement, not an aesthetic one

A04 §1.2 is explicit that `c`/`r` **scale** the image to fit; they do not clip. So requesting
`r=22` for an image whose natural height is 23 rows silently resamples the page.

That is not merely blurry — it breaks input. `PointerMap` (`apps/cli/src/main.rs:701-728`) assumes
terminal pixels map 1:1 onto page pixels. Under scaling, a click at terminal pixel *x* corresponds
to page pixel `x · page_w / placed_w`, and every pointer event lands in the wrong place. This is
precisely the failure class that `main.rs:686-699` and its regression test at `:984-998` exist to
prevent.

**Therefore the engine viewport height must be chosen as an exact multiple of the cell:**

```
page_rows = terminal_rows − chrome_rows
page_h    = page_rows × cell_h        // NOT  vp_h − cell_h
page_w    = terminal_cols × cell_w    // NOT  vp_w
```

On the measured Ghostty this changes nothing (851 = 23×37, so both formulas yield 814) — which is
exactly why the latent bug is invisible today. It bites when `CSI 14t` reports window padding that
is not a whole number of cells, which is the normal case on terminals with configurable padding.
With this rule, `r=` and `c=` become *assertions* rather than instructions: they should equal the
natural size, and a mismatch is a bug to log.

---

## 5. Z-ordering

### 5.1 Page vs chrome: one constant, set once

```
PAGE_Z = -1500000000
```

Rationale, from A04 §1.2's `z` semantics: negative `z` draws under text; `z < INT32_MIN/2`
(`< -1073741824`) draws under *non-default cell backgrounds* as well. `-1500000000` satisfies both
and stays well inside `i32`.

Choosing the stronger threshold — rather than the `z=-1` that Carbonyl uses (A02:162) — is what
makes **opaque overlays** possible. At `z=-1` the image covers a cell's background and only the
glyph strokes survive, so a find bar or a modal would render as bare letters floating over page
pixels: unreadable, and for a permission prompt, dangerous. Below the `INT32_MIN/2` threshold, any
cell we give an explicit background becomes fully opaque, while cells left at the default
background still show the page through. That single constant is what buys the entire floating-
overlay capability.

Today the page is placed at `z=0` — *above* text (`kitty.rs:93`, used at `main.rs:860`). It is
harmless only because nothing currently overlaps. Any overlay added before this constant changes
will be invisible.

### 5.2 Chrome vs chrome: an arbiter, not a z-index

All chrome is text, and text has no z-index — the last write to a cell wins. Relying on emission
order alone is how compositors acquire "the toast ate the modal" bugs. So overlaps are resolved
*before* emission:

```rust
struct Element { id: ElementId, rect: CellRect, prio: u8, content: Vec<TerminalText> }

/// Pure. No I/O, no tty, no allocation of escape bytes. Testable in CI.
fn resolve(elements: &[Element], geom: Geometry) -> CellOwnership
```

`CellOwnership` maps each cell to at most one `ElementId`: the highest-priority element whose rect
contains it. Emission then walks owners, never raw rects, so **no element can paint a cell it does
not own**. Ties (equal priority, overlapping rects) are a programming error and should panic in
debug and log-and-pick-lowest-id in release, rather than silently interleaving.

This is the design's main concession to the environment constraint: the machine is at a lock
screen and cannot verify anything visually, so the compositor's hard part is deliberately placed
in a pure function that CI can check byte-for-byte without a terminal (§15).

### 5.3 Hint collision rule

Hints quantise to `(px / cell_w, py / cell_h)`. When two hints claim the same cell: shift the
second right by the first's label width; if the row has no room, spill to the row below at the same
column; if that is also full, drop the hint and mark the target unreachable-by-hint (grid-jump
still reaches it). Deterministic, order-independent given a stable sort by `(py, px)`, and
unit-testable.

### 5.4 Layer emission order (belt and braces)

Even though `PAGE_Z` should make the terminal composite correctly, the page layer is always emitted
**first** in the byte stream and text after it. If a terminal mishandles the z threshold, text
still lands last and wins. This costs nothing and removes a dependency on `[UNVERIFIED]` behaviour.

---

## 6. Layout diagrams

Both diagrams below were generated programmatically and asserted to be exactly 80 and 200 content
columns by 24 and 50 content rows. **The outer `+--+` frame represents the terminal window edge and
is not drawn content.**

### 6.1 80 × 24 — resting state (1360 × 888 px at a 17×37 cell)

Two chrome rows (policy row "24…39"): merged tab-strip/omnibox on row 1, status on row 24.

```text
+--------------------------------------------------------------------------------+
| 1*Hacker News  2 MDN  +   news.ycombinator.com            TLS  --              |
|                                                                                |
|      +--------------------------------------------------------+                |
|      |                                                        |                |
|      |   P A G E   S U R F A C E   (kitty image i=1000)       |                |
|      |                                                        |                |
|      |   a=T f=24 o=z  i=1000 p=1  c=80 r=22                  |                |
|      |   z=-1500000000   C=1                                  |                |
|      |                                                        |                |
|      |   80 cols x 22 rows  =  1360 x 814 px  @ 17x37 cell    |                |
|      |                                                        |                |
|      |   The image placement is 22 rows tall, so it CANNOT    |                |
|      |   paint into row 1 or row 24. Trust chrome is          |                |
|      |   unspoofable by construction, not by policy.          |                |
|      |                                                        |                |
|      +--------------------------------------------------------+                |
|                                                                                |
|        [f] hint labels are TEXT, cell-quantised, drawn over                    |
|            the image via non-default cell background                           |
|                                                                                |
|           [aa] Sign up      [as] Comments     [ad] Past                        |
|                                                                                |
|                                                                                |
| * 58fps  54KB  0.8ms          ^L url   ^F find   ^T tab   ^Q quit              |
+--------------------------------------------------------------------------------+
```

Cell map: row 1 = trust band. Rows 2–23 = page band, `c=80 r=22` → 1360 × 814 px.
Row 24 = status band. The image placement spans rows 2–23 only, so rows 1 and 24 are
**physically unreachable** by page pixels (§9).

### 6.2 200 × 50 — active state (3400 × 1850 px at a 17×37 cell)

Four chrome rows (policy row "≥40"), with three floats live simultaneously to show the arbiter
working: a toast (prio 60), hints (prio 70), and the find bar (prio 80).

```text
+--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------+
|  1*Hacker News          2 MDN Web Docs        3 crates.io          4 +                                                                                          [ 4 tabs ]                             |
|  >  https://news.ycombinator.com/newest                                                                        TLS EV  |  AGENT: claude-opus  step 12/40  ^C stop                                      |
|                                                                                                                                                                                                        |
|      +----------------------------------------------------------------------------------------------------------------------------------+   +------------------------+                                 |
|      |                                                                                                                                  |   |  NOTIFICATION (float)  |                                 |
|      |   P A G E   S U R F A C E                                                                                                        |   |  Download complete     |                                 |
|      |                                                                                                                                  |   |  report.pdf            |                                 |
|      |   kitty image  i=1000  p=1   a=T,f=24,o=z                                                                                        |   |  auto-dismiss 4s       |                                 |
|      |   c=200  r=46   z=-1500000000  C=1                                                                                               |   +------------------------+                                 |
|      |                                                                                                                                  |     prio 60, floats over the page,                           |
|      |   200 cols x 46 rows  =  3400 x 1702 px  @ 17x37 cell                                                                            |     owns a 6 x 26 cell rect                                  |
|      |                                                                                                                                  |                                                              |
|      |   Damage-mode patch: transmit the dirty sub-rect as a                                                                            |                                                              |
|      |   transient image (i in 1001..1008, N=1), placed at                                                                              |                                                              |
|      |   cell (dx/17, dy/37) with X=dx%17, Y=dy%37 and                                                                                  |                                                              |
|      |   z=-1499999999 -- strictly above the page surface,                                                                              |                                                              |
|      |   still below non-default cell backgrounds.                                                                                      |                                                              |
|      +----------------------------------------------------------------------------------------------------------------------------------+                                                              |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|        [aa] Show HN: ...            [as] 142 comments            [ad] hide            [af] past                                                                                                        |
|                                                                                                                                                                                                        |
|        Link hints: TEXT at prio 70. Cell-quantised -- up to 16 px horizontal and 36 px vertical                                                                                                        |
|        placement error at a 17x37 cell. Collision rule: same-cell hints shift right by label width,                                                                                                    |
|        then spill to the row below. Deterministic and unit-testable without a terminal.                                                                                                                |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|                                                                                                                                                                                                        |
|   FIND  [ terminal emulator____________ ]   3 of 17   n next   N prev   Esc close        prio 80, floats, owns 1 row of page                                                                           |
|                                                                                                                                                                                                        |
|  * 60fps  53,999 B  0.74ms  dmg 4%      |  q0 coalesced 0      |      ^L url   ^F find   ^T tab   ^[ hints   ^Q quit                                                                                   |
|  MODE: STANDARD          scroll 34%          3 requests in flight          engine pid 48211  rss 412 MB                                                                                                |
+--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------+
```

Cell map: rows 1–2 = trust band. Rows 3–48 = page band, `c=200 r=46` → 3400 × 1702 px.
Rows 49–50 = status band. The toast owns a 6×26 cell rect at the top right *of the page area*; the
find bar owns one full row near the bottom *of the page area*. Both are floats: the page image
still spans rows 3–48, and the terminal composites the opaque text over it.

---

## 7. The agent-control indicator

Called out separately because it is the one chrome element whose failure mode is a security
incident rather than an annoyance. A09 §TB5 names "web content → agent reasoning context" as a
trust boundary; this indicator is its user-facing half.

**Rules.**

1. **Band, never float.** It lives in the trust band so a page cannot draw over it or near it.
2. **Whole-row recolour, not just a badge.** While an agent holds input control, the entire
   omnibox row changes background colour. A 3-cell badge can be overlooked; a full-row colour
   change cannot, and — critically — a page cannot reproduce it, because the effect occurs on a
   row the page image does not span.
3. **Driven by core state, not by an engine event.** The core is the process synthesising the
   agent's input, so it already knows. Deriving the indicator from an engine-sent event would mean
   a compromised engine could suppress it.
4. **Fail loud, not silent.** If agent state cannot be determined, display `AGENT: UNKNOWN`. The
   error must never resolve to "no agent".
5. **Clear only on acknowledgement.** `^C` revokes control, but the indicator clears only after the
   engine confirms release — never optimistically. A hung agent must leave the indicator *on*.
6. **Fixed cell range** so its absence is as noticeable as its presence.

Content: agent identity, step counter, last action verb, revoke key —
`AGENT: claude-opus  step 12/40  ^C stop` as drawn in §6.2.

---

## 8. Damage propagation

### 8.1 Two independent damage domains

```rust
struct Damage {
    page:   Option<Rect>,      // page-PIXEL coords, from the frame header
    chrome: bool,              // any band content changed
    floats: Vec<(ElementId, CellRect)>,  // per-float, old ∪ new
    full:   bool,              // resize / alt-screen / ED2 / backend switch
}
```

`page` and `chrome` are **never unioned**, because they address disjoint cells by construction: the
image placement does not span band rows (§4.1), and floats live in `floats`, not in `page`. This is
the single simplification that keeps the compositor tractable — most of the complexity in a real
browser compositor comes from one damage space containing everything.

### 8.2 Rules

1. **Frame arrives** → `page = union(page, header.dirty)`. The union across coalesced frames is the
   engine's job (B07 invariant I2). **It is currently broken** — `apps/engine/src/main.js:96-97`
   discards the old header on coalesce (B07:12-13, 26). Until that is fixed, damage-only transmit
   is unsound and the compositor must stay in full-frame mode.
2. **Element changes** → mark `old_rect ∪ new_rect` dirty. Damaging only the new rect is the
   classic compositor bug: vacated cells keep stale glyphs.
3. **Float dismissed** → its old rect is dirty and resolves to "page shows through" (§8.3).
4. **Resize (SIGWINCH)** → `full = true`; re-query geometry; recompute bands; send engine `resize`;
   **discard** `page` — its coordinates refer to a viewport that no longer exists. Same reasoning as
   B07:132-136.
5. **`ED 2`, `RIS`, or alt-screen toggle** → `full = true` **and the image must be re-transmitted,
   not merely re-placed**: A04 §1.7 states these clear all images.

### 8.3 Restoring page pixels under a dismissed float — without trusting the spec

A04 §1.7 says `EL` must not touch graphics, so in principle clearing the float's text is enough and
the page reappears for free. That is `[SPEC]`, and **`[UNVERIFIED]` on Ghostty from this machine**.
Rather than depend on it, dismissal always does:

```
ESC [ <row> ; <col> H          position
ESC [ K                        clear the float's text
ESC _ G a=d,d=i,i=1000 ESC \   drop the PLACEMENT, keep pixels cached (lowercase = no free)
ESC _ G a=p,i=1000,p=1,c=<C>,r=<R>,z=-1500000000,C=1 ESC \   re-place
```

Cost ≈ 90 bytes and **zero pixel retransmit**, because lowercase `d=i` keeps the image data (A04
§1.2 deletion table). This is correct whether or not the terminal honours the `EL` rule, which is
the right trade for ~90 bytes.

### 8.4 Byte budget — why band damage tracking is premature optimisation

| Item | Bytes |
|---|---|
| One 80-column band row: CUP(8) + EL(3) + SGR + 80 glyphs + reset(4) | ≈ 115–135 |
| One 200-column band row | ≈ 235–255 |
| All 4 bands at 200 columns | ≈ 940–1,020 |
| All 2 bands at 80 columns | ≈ 230–270 |
| **One measured page frame (2482×814)** | **53,999** |

The SGR range is the difference between reverse video (`ESC[7m`, 3 B — what `main.rs:886` uses
today) and a truecolor fg+bg pair (~40 B). Taking the pessimistic end: redrawing *every* band on
*every* flush costs **1.9%** of a frame at 200 columns and **0.5%** at 80. Per-element damage
tracking for bands would add state, invalidation bugs, and tests to save under two percent.
**Do not build it.** Track damage only for floats, which can be large (a hint overlay with 200
labels is ~4 KB, comparable to a damage patch).

Over A07's remote transports the conclusion holds: ~1 KB at 2 Mbit is ≈ 4 ms.

### 8.5 Flush trigger and emission order

Trigger: `page.is_some() || chrome || !floats.is_empty() || full`, evaluated on the existing 16 ms
poll (`apps/cli/src/main.rs:478`).

Order, into **one buffer, one `write`, one `flush`** (as `main.rs:899-901` already does — partial
writes tear):

1. Page layer — only if `page.is_some()` or `full`.
2. Reserved bands, top → bottom — if `chrome` or `full`.
3. Floats, ascending priority — if changed **or** if `page` damage intersects the float's rect
   (so a page repaint underneath cannot leave the float half-erased).
4. Park the cursor at a fixed harmless cell.

**Never emit `ESC [ 2 J`.** It is the reflexive way to clear a screen and per A04 §1.7 it destroys
every image, forcing a full retransmit. This deserves a lint or a debug assertion on the output
buffer, because it is the kind of line a future contributor adds without knowing.

### 8.6 Invariants

- **C1 — Disjointness.** Page-band cells and chrome-band cells never intersect.
- **C2 — Ownership.** Every emitted cell write is attributable to the element that owns that cell
  in `CellOwnership`.
- **C3 — Vacancy.** After any element shrinks or is dismissed, no cell it previously owned retains
  its glyphs.
- **C4 — No scaling.** `placed_cols × cell_w == page_w` and `placed_rows × cell_h == page_h`. If
  this fails, pointer mapping is wrong (§4.2).
- **C5 — Atomicity.** One flush produces one `write`.
- **C6 — Erasure safety.** The output buffer never contains `ESC[2J`, `RIS`, or a 1049 toggle.

---

## 9. How chrome avoids covering page content

Two mechanisms, chosen per element in §3.

**Bands — the page is *shrunk*, so nothing is covered, ever.** The page viewport is reduced by
`chrome_rows × cell_h` and the engine reflows. Cost: a `resize` round-trip and a forced full-damage
frame — paid once at startup and on SIGWINCH, never during interaction.

**Floats — content *is* covered, and the mitigations are explicit.** Floats are user-invoked and
transient; they anchor to the edge furthest from the focused element where possible; and they are
dismissible with a single key. The find bar's occlusion of a search hit is a real unsolved defect
(§14, Q1).

### 9.1 The anti-spoofing property, stated precisely

**Claim.** With bands, a hostile page cannot render anything into a trust-band cell.

**Argument.** The page's only route to the tty is the placement of image `i=1000` with `r=page_rows`
positioned by an explicit `CUP` at the page band's first row; a placement of `r` rows starting at
row `R` occupies exactly rows `R … R+r−1`, and trust bands lie outside that interval. The page has
no second channel: page-*derived text* (title, URL) is stripped of ESC, C0, C1 and DEL before it
reaches the tty (`crates/tf-term/src/unicode.rs:60-75`, tested `:126-142`), so it cannot smuggle
`CUP`/`SGR` to escape its band.

**This argument depends on three things, and each is a real obligation:**

1. `C=1` on every placement, so the image cannot move the cursor and shift subsequent writes
   (already set — `kitty.rs:93,146-148`).
2. The single-writer discipline B01 §2.1 recommends but which **is not yet enforced** — three
   writers exist today. The compositor is the natural home for that fix: it should be the sole
   writer for the duration of a session, with chrome content accepted only as a `TerminalText`
   newtype that only the sanitizer can mint.
3. `r=` actually matching the natural size (§4.2), or the image scales and spills.

### 9.2 The residual risk bands do *not* solve

A page can draw a convincing fake tab strip and omnibox **inside its own area** — the terminal
equivalent of a browser-in-browser phishing kit. Bands guarantee the *real* chrome exists at a
*fixed* location; they cannot stop a forgery elsewhere on screen. Mitigations, in increasing cost:
the fixed band position itself; the agent indicator's whole-row recolour (§7), which a page cannot
reproduce outside its band; and optionally a **per-session random accent colour** for the trust
band, which defeats a page that hard-codes a fake band's palette. The last is a known anti-phishing
pattern but needs UX validation before adoption — flagged, not recommended.

---

## 10. Backend degradation

The ownership model (§5.2) is **backend-independent**; only the page-layer emit changes. That is a
strong argument for it, since three of our four backends cannot do z-ordering at all.

| Backend | Page layer | Float support | Consequence |
|---|---|---|---|
| **Kitty** | `a=T` transmit+display; `a=p` to re-place | Full — `PAGE_Z` makes overlays opaque | Design as specified |
| **Sixel** | pixels written into cells at the cursor; no z-index, no placement reuse (A04 §3, §402) | Text drawn *after* the sixel wins, but **dismissal requires redrawing the page** = full retransmit | **Prefer bands.** Make the find bar a band; replace hint overlays with a band-based numbered-link list |
| **Unicode half-block** | the page *is* text (`unicode.rs:18-50`) | Trivial — everything is text | Cheapest dismissal of all: repaint from the retained RGB buffer (`main.rs:809`), no protocol involved |
| **iTerm2 OSC 1337** | no positioning primitive, no z (A04 §402) | as sixel | **Effectively dead code** — iTerm2 3.6.9 supports kitty graphics (A04 §2) and `best_backend` prefers it (`caps.rs:56-66`). Consider removing `Backend::Iterm2` rather than maintaining a path nothing selects |

---

## 11. Byte-exact sequence catalogue

Derived from A04 §1.2/§1.6 and from our encoder (`crates/tf-term/src/kitty.rs:125-156`).

```text
# Page: transmit + display, full frame  (current hot path)
ESC _ G a=T,f=24,t=d,q=2,o=z,s=<W>,v=<H>,i=1000,p=1,c=<COLS>,r=<ROWS>,z=-1500000000,C=1,m=<0|1> ; <b64> ESC \

# Page: re-place only, no pixel retransmit          (~50 B)
ESC _ G a=p,i=1000,p=1,c=<COLS>,r=<ROWS>,z=-1500000000,C=1 ESC \

# Page: force a redraw of a region a float vacated   (~90 B, no retransmit)
ESC _ G a=d,d=i,i=1000 ESC \        # lowercase: drop placement, KEEP pixels cached
ESC _ G a=p,i=1000,p=1,... ESC \

# Damage patch (Mode B — only after B07's union fix)
ESC _ G a=T,f=24,o=z,s=<dw>,v=<dh>,i=<1001..1008>,N=1,X=<dx%cell_w>,Y=<dy%cell_h>,z=-1499999999,C=1,m=... ; <b64> ESC \
   preceded by CUP to cell ( dx/cell_w , dy/cell_h ), 1-based
   N=1 marks it transient so terminals evict our scratch first (A04 §1.10)
   z is one greater than PAGE_Z: strictly above the page, still under non-default backgrounds.
   Do NOT rely on equal-z "ties broken by lower image id" — semantics unconfirmed (§13 U3).

# Chrome band row
ESC [ <row> ; 1 H  ESC [ K  ESC [ 48;2;<r>;<g>;<b> m  ESC [ 38;2;<r>;<g>;<b> m  <text>  ESC [ 0 m

# Teardown — already correct in tty.rs:36
ESC _ G a=d,d=A ESC \
```

Chunking obligations when emitting the page layer (A04 §1.3, implemented at `kitty.rs:121-156`):
base64 first then split; ≤ 4096 B of base64 per escape; only the first chunk carries control keys;
`m=1` on all but the last; **no other graphics escape may be interleaved between chunks of one
image.** That last rule constrains the compositor directly: a float repaint must not be emitted in
the middle of a chunked page transmit. Emission order §8.5 satisfies this by construction, and it
is worth an assertion.

---

## 12. Findings in existing core code

Per the file-ownership rule these are **reported, not fixed**. All were read directly.

| # | Severity | Finding |
|---|---|---|
| **F1** | **High** | **Status bar is not width-clamped.** `apps/cli/src/main.rs:890-895` composes ` {flag} {title}  |  {url}  |  …ctrl+q quit `. Worst-case width, counted field by field: 1 + flag 3 + 1 + title **41** (`sanitize_for_terminal(…, 40)` appends an ellipsis on truncation, `unicode.rs:63`) + 5 + url **61** + 5 + fps 2–3 + `fps ` 4 + KB 2–3 + `KB ` 3 + ms 3–4 + `ms  ctrl+q quit ` 16 = **~147–151 display columns**. The measured Ghostty is **146 columns** (`tty.rs:239`), so this already overflows on the verified configuration. It is worse than the char count suggests: `sanitize_for_terminal` truncates by **chars**, not display width (`unicode.rs:61-63`), so a 41-character CJK title occupies 82 columns and the bar reaches ~190. On the last row this wraps, scrolls the screen up one row, and desynchronises the image placement from the cell grid. Needs a clamp to `cols` computed in **display width**, not `char` count. |
| **F2** | **High** | **Chrome never repaints without a frame.** `Renderer::present` early-returns unless a frame arrived (`main.rs:849-851`, `dirty` set only in `on_frame` at `:845`). On a static page the status bar freezes: `fps` holds its last value instead of decaying to 0 (`frame_times` is only pruned inside `on_frame`, `:842-844`), and title/URL/loading events repaint only when the page happens to paint. Needs a `chrome_dirty` flag OR'd into the flush trigger (§8.5). |
| **F3** | **Medium** | **Page placement omits `c`/`r`.** `main.rs:860` uses `Placement::default()` (`kitty.rs:93` → `cols: None, rows: None`), so the image is placed at natural size. It fits today only because 814 = 22 × 37 exactly. Any geometry where `cell_h ∤ page_h` spills the image into the status row and covers it. See §4.2 for the fix — set the viewport from the cell grid rather than clamping `r`. |
| **F4** | **Medium** | **Page is placed at `z=0`, i.e. above text.** `kitty.rs:93`. Latent only because nothing overlaps yet; the first float added will be invisible. Needs `PAGE_Z = -1500000000` (§5.1). |
| **F5** | **Low** | **Built-but-unused mechanism.** `Placement::{cols, rows, z}` are encoded (`kitty.rs:137-145`) but no caller sets them. The compositor is the intended caller. |
| **F6** | **Low** | `Backend::Iterm2` is unreachable: `best_backend` prefers kitty (`caps.rs:56-66`) and iTerm2 3.6.9 supports kitty graphics (A04 §2). Dead path to remove or document. |

F1 and F2 are the two that a user would notice within a minute of running the current build.

---

## 13. Unverified claims and probe recipes

**No live terminal was available** (`tty` → `not a tty`; the machine is at a lock screen, so
screenshot verification is also unavailable). The following are `[SPEC]`-sourced and must be
confirmed before the design is trusted in full.

| # | Claim | Source | Probe |
|---|---|---|---|
| **U1** | Ghostty honours `z < -1073741824` as "under non-default cell backgrounds" | A04 §1.2 `[SPEC]` | Place a solid red image at `z=-1500000000` over rows 1–5; write one cell at row 3 with an explicit blue background and a space glyph. **Pass** = a blue cell. **Fail** = red. Needs a human eye or a screenshot; not CI-able. **This is the highest-value probe — the entire float capability rests on it.** |
| **U2** | `ESC[K` (EL) leaves kitty images intact | A04 §1.7 `[SPEC]` | Place an image over rows 1–5, write text at row 3, then `ESC[3;1H ESC[K`. **Pass** = image still visible at row 3. Design already does not depend on this (§8.3), so a failure costs nothing. |
| **U3** | Equal-`z` tie-break "lower image id" — does lower id draw above or below? | A04 §1.2, wording ambiguous | Place two overlapping images at identical `z` with ids 1000 and 1001 and observe. Avoided by construction (§11 uses distinct `z`), so this is a nice-to-know. |
| **U4** | `c`/`r` scale rather than clip | A04 §1.2 `[SPEC]` | Transmit a 100×100 image and place with `r=1`. **Scale** = whole image squashed into one row. **Clip** = top slice only. Determines whether §4.2's exact-multiple rule is mandatory or merely tidy. |
| **U5** | tmux passthrough preserves `z` and placement semantics | A04, `kitty.rs:241-250` | Run the U1 probe inside tmux with `allow-passthrough on`. |

---

## 14. Open questions

- **Q1 — find-bar occlusion.** When a match scrolls to the bottom row, the find bar covers it. Real
  browsers avoid this because the bar is outside the viewport. Options: bias the engine's
  scroll-into-view by one cell height (an engine-side change); or promote the find bar to a band
  while it is open, paying the resize. Neither is free; needs a product call.
- **Q2 — hint density on small cells.** At a 17×37 cell, two links on adjacent CSS lines can share a
  cell row. §5.3's spill rule keeps labels distinct but positions become approximate. Acceptable for
  label-driven hinting; revisit if user testing disagrees.
- **Q3 — per-session accent colour** for the trust band (§9.2): real anti-phishing value, unproven
  UX cost.
- **Q4 — damage patches vs. band boundaries.** A damage patch whose rect touches the page band's
  last row must be clipped to the band before placement, or it spills into chrome. `Rect::clamp_to`
  (`crates/tf-term/src/lib.rs:53-63`) already does exactly this and should be used.

---

## 15. Test plan (no terminal required)

The environment cannot verify visually, so the compositor is split so that CI can verify
structurally:

```rust
fn compose(state: &UiState, geom: Geometry) -> Vec<Op>   // pure
fn emit(ops: &[Op]) -> Vec<u8>                            // pure
```

Unit tests assert on `Op`s; golden-byte tests assert on `emit`. Both run headless.

1. **Band arithmetic** across `rows ∈ {8, 12, 23, 24, 39, 40, 50}` — page rows match §4.1 exactly.
2. **C4 / no scaling** — for each geometry, `placed_rows × cell_h == page_h`.
3. **Ownership** — overlapping floats resolve by priority; equal priority + overlap panics in debug.
4. **Vacancy (C3)** — move an element; assert the vacated cells appear in the damage set.
5. **Disjointness (C1)** — no `Op` addresses a row outside its element's declared rect.
6. **Erasure safety (C6)** — the emitted buffer contains no `ESC[2J`, no `RIS`, no 1049 toggle.
7. **Chunk integrity** — no graphics escape is interleaved between chunks of one image (A04 §1.3).
8. **F1 regression** — build a status bar with a 40-character CJK title and a 60-character URL at
   `cols=146`; assert the rendered **display width** ≤ `cols`. This test fails against the current
   tree, which is the point.
9. **Hint collision** — deterministic labels and positions given a fixed input set.

---

## 16. Summary of recommendations to the commander

1. **Set `PAGE_Z = -1500000000` and place the page with explicit `c`/`r`** (F3, F4). One constant
   and two fields unlock every floating overlay; without them, floats are invisible or the image
   spills into chrome.
2. **Fix the status-bar width clamp using display width, not char count** (F1). This is a live bug
   on the already-verified 146-column Ghostty configuration.
3. **Add a `chrome_dirty` flush trigger** (F2) so chrome is not hostage to page repaints.
4. **Derive the engine viewport from the cell grid** (`page_h = page_rows × cell_h`), not by
   subtracting one cell from the window height — otherwise the page scales and every pointer
   coordinate is wrong (§4.2).
5. **Keep all chrome as text.** Bands for anything trust-critical, floats for anything transient.
   Do not build band damage tracking; at ≤ 2% of a frame it is not worth the bugs.
6. **Run probe U1 the moment a terminal is available.** The float capability is the one part of this
   design resting on an unverified `[SPEC]` claim.
