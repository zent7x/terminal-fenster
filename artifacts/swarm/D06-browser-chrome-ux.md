# D06 — Browser chrome: layout, states, and keybindings

**Mission:** D06 — design the chrome (tab strip, omnibox, security indicator, loading, back/forward/reload, find-in-page, zoom, error pages) so it adapts from a 200-col window to an 80x24 tmux pane, with keybindings that do not collide with terminal/tmux defaults.
**Owner of this file:** D06. **Nothing in `crates/`, `apps/cli/`, or `apps/engine/src/main.js` was edited.** Every change those files need is written up in §2 as a diff-shaped instruction for the commander.
**Machine:** macOS 26.1, Apple M4, Ghostty 1.3.1, tmux 3.7b, Electron 43.2.0 (vendored in-tree).
**Status:** implementation spec. Measured facts carry the command that produced them. Anything not measured this session is marked **UNVERIFIED**.

---

## 0. Recommendation in one paragraph

Anchor **all** chrome at the bottom of the pane — two rows wide (`>=100` cols: tab strip + omnibox/status), one row narrow (`<100` cols: omnibox/status only, tabs folded into an `n/m` counter) — and never change the row count while a page is loaded, because a row-count change is a viewport resize, and a viewport resize is a full Chromium reflow plus a full-frame retransmit (348 KiB wire vs 0.1 KiB for a damage rect, A03 §0.1). Bottom-anchoring is not an aesthetic choice: `PointerMap::to_page` (`apps/cli/src/main.rs:712`) maps terminal pixels to page pixels with **no y-origin**, so chrome above the page would silently misroute every click by one row-height, while chrome below the page is already handled correctly by the existing `py >= page_h -> None` guard. Render chrome, error pages, and find results as **terminal text, never as page pixels** — text is legible in the no-graphics tier where a rendered Chromium error page is an unreadable grey smear, and it costs ~150 bytes instead of ~348 KiB. For keys, bind **neither `Ctrl+A` nor `Ctrl+B`** (screen and tmux prefixes), and drop the currently-shipped `Alt+Left`/`Alt+Right` back/forward binding, which **is dead in Ghostty with default configuration** (measured, §2 F1): use `Ctrl+Left`/`Ctrl+Right`, which the existing decoder already parses in both legacy and kitty encodings.

---

## 1. Evidence base

Everything in §2 and §8 rests on these. Commands are reproducible on this box.

### 1.1 Ghostty 1.3.1 default keybinds

```
/Applications/Ghostty.app/Contents/MacOS/ghostty +list-keybinds --default
```

93 default binds. 78 use `super` (macOS Cmd) and are irrelevant to us — Cmd never reaches a TUI. The **15 that do not use `super`** are the entire terminal-level collision surface:

| Ghostty default bind | Action | Consequence for BlackGlass |
|---|---|---|
| `ctrl+tab` | `next_tab` | **`Ctrl+Tab` never reaches the app.** Cannot be used for tab switching. |
| `ctrl+shift+tab` | `previous_tab` | Same. |
| `alt+arrow_left` | `esc:b` | **Ghostty transmits `ESC b`, not a modified Left.** Kills the shipped back binding (F1). |
| `alt+arrow_right` | `esc:f` | Same, for forward. |
| `shift+arrow_{left,right,up,down}` | `adjust_selection` | Consumed when a terminal selection exists. Avoid Shift+arrow binds. |
| `shift+page_{up,down}`, `shift+home`, `shift+end` | `adjust_selection` | Same. |
| `escape` | `end_search` | Only while Ghostty's own search overlay is open. Escape is otherwise ours. |
| `copy`, `paste` | clipboard | Named keys, not chords. No conflict. |

The `--default` flag is load-bearing here: this user's `~/.config/ghostty/config` exists but contains **only** font, palette, and window settings — zero `keybind` lines (verified by reading it). So the four hostile binds above are Ghostty's shipped defaults, not local customization.

### 1.2 tmux 3.7b default bindings

```
tmux -f /dev/null list-keys -T root
```

The root table (keys tmux intercepts **without** the prefix) contains **24 bindings, every one of them mouse, wheel, or scrollbar** — `list-keys -T root | grep -cE "Mouse|Wheel|Click"` returns 24 of 24, and the inverse grep returns 0. There are **zero non-mouse key bindings in the root table**. Therefore tmux steals exactly one key: the prefix, `Ctrl+B` by default. Everything else passes through untouched.

One root binding is worth quoting because we depend on it:

```
bind-key -T root WheelUpPane if-shell -F "#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}" \
    { send-keys -M } { copy-mode -e }
```

Because `TtyGuard::enable_input_protocols` enters the alternate screen (`\x1b[?1049h`, `crates/bg-term/src/tty.rs:150`), `alternate_on` is true, so tmux **forwards** the wheel to us via `send-keys -M` instead of hijacking it into copy-mode. **The alternate screen is load-bearing for mouse scroll under tmux.** Removing `?1049h` would silently break scrolling in every tmux pane.

### 1.3 Terminal state established by the existing tty guard

From `crates/bg-term/src/tty.rs:103-117`:

| Flag cleared | Line | Consequence for keybinding design |
|---|---|---|
| `IXON` | 110 | `Ctrl+S` (0x13) and `Ctrl+Q` (0x11) are **not** flow control. They arrive as bytes. The shipped `Ctrl+Q`=quit is safe *only* because of this line. |
| `ISIG` | 112 | `Ctrl+C`, `Ctrl+\`, `Ctrl+Z` generate **no signals**. They arrive as 0x03, 0x1C, 0x1A. The app owns them — and owes the user working substitutes (F6, F7). |
| `IEXTEN` | 112 | `Ctrl+V` literal-next and `Ctrl+O` discard are off. Both keys are free. |
| `ICANON`, `ECHO` | 112 | No line editing; we implement the omnibox editor ourselves. |

Kitty keyboard flags pushed are `\x1b[>27u` (tty.rs:167) = 1 (disambiguate) + 2 (event types) + 8 (report all keys as escapes) + 16 (associated text).

### 1.4 Geometry measured on this box

`crates/bg-term/src/tty.rs:239` encodes the measured Ghostty full-screen geometry: `rows: 23, cols: 146, xpixel: 2488, ypixel: 858` -> cell `17x37 px`. The end-to-end run in the mission brief produced a `2482x814` frame, and `814 = 22 x 37` — exactly 23 rows minus the one row the status bar already occupies. The row-cost arithmetic in §3.2 is derived from these numbers, not estimated.

### 1.5 Electron 43.2.0 API surface (vendored typings, `apps/engine/node_modules/electron/electron.d.ts`)

| Capability | Symbol | Line | Verdict |
|---|---|---|---|
| Find in page | `findInPage(text, FindInPageOptions)` | 17961 | Present. Options: `forward`, `findNext`, `matchCase` only. |
| Find results | `found-in-page` event, `activeMatchOrdinal`, `matches` | 17018, 23356 | Present. This is where `3 of 17` comes from. |
| Find teardown | `stopFindInPage('clearSelection'\|'keepSelection'\|'activateSelection')` | 18491 | Present. |
| Zoom | `setZoomLevel`, `setZoomFactor`, `getZoomLevel` | 18467, 18456, 18063 | Present. `scale := 1.2 ^ level`; **documented default limits 50%–300%**. |
| Navigation | `goBack`, `goForward`, `canGoBack` | 18071, 18079, 17820 | Present but **deprecated** in favour of `contents.navigationHistory.*` (lines 17816-18091). Use the new API. |
| Hard reload | `reloadIgnoringCache()` | 18283 | Present. |
| Stop | `stop()` | 18487 | Present. Backs `Esc`=stop-load. |
| Load failure | `did-fail-load(event, errorCode, errorDescription, validatedURL, isMainFrame, ...)` | 16565ff | Present. Gives us the real `net::` code for the error page. |
| Cert failure | `certificate-error` (app-level) | 251 | Present. |
| **Load progress %** | — | — | **ABSENT.** Only `did-start-loading`/`did-stop-loading` (16861), `isLoading()` (18167), `isLoadingMainFrame()` (18172), `isWaitingForResponse()` (18185). |

The last row changed the design. There is **no** load-progress percentage in Electron 43. A `62%` in the chrome would be a fabricated number. §7.4 specifies a three-phase indicator built from signals that actually exist instead.

A second consequence, from the `setZoomLevel` doc comment (line 18464): *"The zoom policy at the Chromium level is same-origin ... the zoom level for a specific domain propagates across all instances of windows with the same domain."* Combined with B04's one-`BrowserWindow`-per-tab design, **zooming one tab silently zooms every other tab on the same origin.** Handled in §7.8.

---

## 2. Findings for the commander (core files I must not edit)

Ranked by severity. Each is a concrete, located defect or gap that this design depends on.

### F1 — HIGH — `Alt+Left`/`Alt+Right` back/forward is dead in Ghostty with default config

`apps/cli/src/main.rs:580-592` intercepts `mods.alt` + `KeyCode::Left`/`Right` for back/forward. Ghostty 1.3.1's **default** binds are `alt+arrow_left=esc:b` and `alt+arrow_right=esc:f` (§1.1). Ghostty therefore transmits the two bytes `ESC 'b'`, which `Decoder::step` (`crates/bg-term/src/input.rs:179-192`) decodes via its `ESC <char> = Alt+char` branch into `KeyCode::Char('b')` with `alt: true`. The match arm tests `KeyCode::Left`, so **it never fires**; instead an `Alt+b` keystroke is forwarded to the page.

This is worse on macOS generally: `macos-option-as-alt` is unset by default (`ghostty +show-config --default`), so Option-plus-letter composes a Unicode character rather than producing an Alt-modified key. **Alt is not a dependable modifier for this product.**

*Fix:* bind `Ctrl+Left`/`Ctrl+Right`. They are free in Ghostty's defaults and in tmux's root table (§1.1, §1.2), and the decoder already handles them in **both** encodings — `decode_legacy_csi` parses `CSI 1;5D` (`input.rs:436-465`, modifier param 5 -> ctrl) and the kitty path parses the `u`-form. Keep `Alt+Left`/`Right` as an accepted alias for terminals that do deliver it; it costs one extra match arm and helps non-Ghostty users.

### F2 — HIGH — the status bar overflows and wraps on any pane narrower than ~147 columns

`apps/cli/src/main.rs:885-896` composes the bar from a 40-char title, a 60-char URL, and fixed decoration, then writes it at row `rows` with **no clamp to `cols`**. Worst-case composed width:

```
1 + flag 3 + 1 + title 41 + "  |  " 5 + url 61 + "  |  " 5 + "60fps 53KB 0.7ms" 16 + "  ctrl+q quit " 14  =  147 cols
```

The measured Ghostty full-screen width on this box is **146 cols** (§1.4). So the bar can overflow *on the machine it was verified on*, and always overflows in an 80-col tmux pane. Because it is written on the **last** row, wrapping scrolls the screen by one line — which drags the kitty image placement out of position. The failure is not cosmetic.

*Fix:* build the bar from the priority-ordered field list in §6.2 and truncate to `cols` as the final step. Move `fps/KB/ms` behind a `--stats` flag; they are developer telemetry, not chrome.

### F3 — HIGH — truncation counts characters, not display columns

`sanitize_for_terminal` (`crates/bg-term/src/unicode.rs:60-75`) truncates on `out.chars().count() >= max_len`, and the test at `unicode.rs:158-163` locks that behaviour in. A 40-character CJK title occupies **80 columns**. An emoji title occupies 2 columns per char; a title with combining marks occupies fewer. Any of these desynchronizes the chrome row's column arithmetic and, combined with F2, wraps the last row.

*Fix:* add a display-width-aware `truncate_to_columns(s, cols)` using a `wcwidth`/East-Asian-Width table, and have the chrome measure every untrusted string with it. Note this is a **new function**, not a change to `sanitize_for_terminal`'s contract — the existing tests should keep passing.

### F4 — HIGH (security) — the sanitizer does not strip bidi or invisible codepoints, so the origin can be spoofed

`unicode.rs:68-72` rejects `c < 0x20`, `0x7f`, `0x80..=0x9f`, `0x2028`, `0x2029`. It does **not** reject:

| Range | Name | Attack |
|---|---|---|
| `U+202A..U+202E` | Embedding / **RIGHT-TO-LEFT OVERRIDE** | `U+202E` reverses rendering. A URL crafted as `evil.com/\u{202E}moc.knab` displays as if the origin were `bank.com`. |
| `U+2066..U+2069` | Directional isolates | Same class of reordering. |
| `U+200B..U+200F` | ZWSP, ZWNJ, ZWJ, LRM, RLM | Zero-width padding splits a hostile origin so it reads as a trusted one. |
| `U+061C` | Arabic letter mark | Invisible bidi control. |
| `U+FE00..U+FE0F`, `U+E0100..` | Variation selectors | Invisible; inflates length without display width. |

The chrome row is the **only** place a user can check what origin they are on. Passing attacker-controlled codepoints into it defeats the security indicator entirely. This matters more here than in a GUI browser, because we have no favicon, no certificate popover, and no separate address-bar widget — one row of text is the whole trust surface.

*Fix:* extend the rejected set to the ranges above; render the URL through the origin-splitting scheme in §7.3 (registrable domain emphasized, everything else dimmed) so the path cannot visually impersonate a host; and punycode-encode non-ASCII hosts unless the label's script matches the user's locale (the standard IDN-display rule).

### F5 — MEDIUM — `PointerMap` has no y-origin, so top-anchored chrome would misroute every click

`apps/cli/src/main.rs:712-727` maps `py` straight through and rejects `py >= page_h`. That is correct **only** while all chrome is strictly below the page. This is the reason §3.1 anchors the tab strip at the bottom rather than the top. If the commander later wants a conventional top tab strip, `PointerMap` needs an `origin_y` subtracted before the bounds check, and the renderer needs to place the image at row 2 — otherwise clicks land one row-height (37 px here) above where the user aimed, which is roughly one line of body text and will read as "clicks are slightly off" rather than as a coordinate bug.

### F6 — MEDIUM — `Ctrl+Z` does nothing, and suspending would wedge the terminal

`ISIG` is cleared (`tty.rs:112`), so `Ctrl+Z` arrives as byte `0x1A` and is currently forwarded to the page — job control silently does not work. Worse, `TtyGuard` installs handlers for `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT` only (`tty.rs:132`); there is **no** `SIGTSTP`/`SIGCONT` path. If suspend is ever wired up naively, the user is dropped back to a shell with raw mode, mouse reporting, and the alternate screen still active.

*Fix:* handle `Ctrl+Z` explicitly — run `RESTORE_SEQ` + `tcsetattr`, then `raise(SIGTSTP)`; install a `SIGCONT` handler that re-acquires raw mode, re-enables input protocols, and forces a full repaint (the image was deleted by `a=d,d=A` on the way out).

### F7 — MEDIUM — no interrupt escape hatch

With `ISIG` off, `Ctrl+C` cannot kill a wedged session. If the input loop stalls, the user's only recourse is another terminal and `kill`. That is an uninstall event for a tool whose core promise is "it never wedges your terminal" (`tty.rs:1-19`).

*Fix:* check `Ctrl+Q` at the very top of the input dispatcher, before any mode or focus logic, so it cannot be swallowed by a modal state; and add a triple-`Escape`-within-500 ms wedge-breaker that force-quits. Both are cheap and both are testable.

### F8 — LOW — `Ctrl+Tab` is unavailable for tab switching

Ghostty consumes `ctrl+tab`/`ctrl+shift+tab` at the terminal layer (§1.1). Any design that reaches for the browser-conventional `Ctrl+Tab` will appear to work in Apple Terminal and silently fail in Ghostty. §8 uses `Ctrl+N`/`Ctrl+P` instead.

---

## 3. Layout model

### 3.1 Everything is bottom-anchored

```
                     +----------------------------------------+
                     |                                        |
   rows 1 .. H-C     |          PAGE  (kitty image or         |   page area:
                     |          half-block fallback)          |   H - C rows
                     |                                        |
                     +----------------------------------------+
   row H-1           | tab strip            (wide layout only)|   chrome:
   row H             | omnibox / status / find / command      |   C rows
                     +----------------------------------------+
```

Four independent reasons, in descending strength:

1. **It requires no core change.** `PointerMap::to_page` already returns `None` for `py >= page_h`, which is exactly "the click landed on chrome". A top strip needs an origin offset (F5).
2. **Terminal convention.** vim's `:` line, less's prompt, tmux's status line, and the shell prompt are all at the bottom. The audience is terminal users.
3. **Transient UI reuses the row it already owns.** Find, the command line, and the omnibox editor are all *the same row* in different modes — the way vim's bottom line is a status line until you press `:` or `/`. Zero additional rows, zero reflow.
4. **Resize stability.** The image origin stays at `(0,0)` across `SIGWINCH` regardless of how the chrome row count changes.

The cost is honest and should be stated: a bottom tab strip is unconventional for a browser. It is worth it here, and the `n/m` counter in the omnibox row means the narrow layout never shows a tab strip at all.

### 3.2 Row budget

Chrome rows are page pixels. On this box, one row = 37 device px of viewport permanently surrendered.

| Layout | Cols | Chrome rows `C` | Page rows | Page px (cell 17x37) | Chrome cost |
|---|---|---|---|---|---|
| Ghostty full screen, measured | 146 x 23 | 2 | 21 | 2482 x 777 | 8.7% of height |
| Ghostty full screen, shipped today | 146 x 23 | 1 | 22 | 2482 x 814 | 4.3% |
| tmux pane, narrow | 80 x 24 | 1 | 23 | 1360 x 851 | 4.2% |
| tmux pane, tiny | 80 x 10 | 1 | 9 | 1360 x 333 | 10.0% |

**Hard cap: `C <= 2`, and `C = 1` whenever `cols < 100` or `rows < 20`.** Below `rows < 8`, chrome is suppressed entirely and the pane is declared too small (§6.3).

### 3.3 The rule that matters most: reserved rows are fixed, overlays are free

Two distinct mechanisms, with very different costs:

| Mechanism | What it does | Cost | Use for |
|---|---|---|---|
| **Reserved row** | Reduces the page viewport. Changing `C` re-runs `setSize`, forces Chromium reflow, invalidates the whole frame. | **Full-frame retransmit: ~348 KiB wire, 3.5 fps on a 10 Mb SSH link** (A03 §0.1) | Tab strip, omnibox/status. Fixed at navigation time. |
| **Overlay** | Draws text *over* page pixels. Viewport unchanged, no reflow. Dismissal repaints from the retained `Renderer::rgb` buffer. | Bytes for the overlay text, plus one damage-rect repaint on dismiss | Omnibox suggestions, leader menu, security details, help. |

**Therefore: `C` must never change while a page is loaded.** It is recomputed on `SIGWINCH` (where a reflow is already unavoidable) and on nothing else. A find bar that pushes the page up by one row would cost a full reflow *per invocation* — hence find lives in the row it already owns.

`Renderer::present` already retains the decoded frame in `self.rgb` (`main.rs:809`), so overlay dismissal can repaint without an engine round trip. That is what makes overlays cheap.

---

## 4. Wide layout — 200 columns

Generated and width-asserted by `mock.py` (§11.3): every line below is **exactly 200 columns** and contains only printable ASCII `U+0020..U+007E`. Long lines will scroll horizontally in your viewer; that is the point.

### 4.1 Default

```
 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                                                               ^G menu  
```

Active tab in `[...]` **and** reverse video — the brackets exist so the active tab is still identifiable when colour is unavailable or the user is colour-blind. No padlock on a healthy HTTPS origin (§7.3).

### 4.2 Loading — three real phases, no invented percentage

```
 [1 MDN - fetch() API] | 2 torvalds/linux | 3 * grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                     1/5  
  / connecting  developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                                      Esc stop   ^G menu  
  - loading     developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                                      Esc stop   ^G menu  
  \ rendering   developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                                      Esc stop   ^G menu  
```

Rows 2–4 are the same row over time. `*` on tab 3 marks a background tab loading.

### 4.3 Omnibox focused

```
 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  > developer.mozilla.org/en-US/docs/Web/API/fetch_                                                                                                                            Tab accept   Esc cancel  
```

### 4.4 Omnibox with suggestions — an **overlay**, not extra rows

```
  +--------------------------------------------------------------------+
  | > moz|illa.org/en-US/docs/Web/API/fetch                            |
  +--------------------------------------------------------------------+
  | H  developer.mozilla.org/en-US/docs/Web/API/fetch     visited 2h   |
  | H  developer.mozilla.org/en-US/docs/Web/API/Request   visited 2d   |
  | S  mozilla fetch keepalive                            search       |
  +--------------------------------------------------------------------+

 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  > moz                                                                                                                                                                        Tab accept   Esc cancel  
```

The box floats over page pixels near the bottom-left; the viewport is untouched. `moz|illa.org/...` shows the inline ghost completion (typed text bright, completion dimmed) — `Tab` accepts it. Prefix letters: `H` history, `S` search, `B` bookmark, `O` open tab.

### 4.5 Find in page

```
 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  / abort signal_                                                                                                                                              3 of 17   ^N next   ^P prev   Esc close  
```

`3 of 17` is `activeMatchOrdinal` / `matches` from the `found-in-page` event (§1.5).

### 4.6 Insecure origin

```
 [1 admin - router] | 2 torvalds/linux | +                                                                                                                                                         1/3  
  !! NOT SECURE  http://192.168.1.1/admin/wan_settings                                                                                                                          ^G s details   ^G menu  
```

Scheme is shown in full **only** when it is a problem. `!!` renders in red-on-default; the badge is the loud element because the *absence* of chrome is the healthy state.

### 4.7 Certificate error

```
 [1 internal.corp] | 2 torvalds/linux | +                                                                                                                                                          1/3  
  !! CERT INVALID  internal.corp/deploy       NET::ERR_CERT_DATE_INVALID  expired 2026-03-01                                                                                    ^G s details   ^G menu  
```

### 4.8 Tab overflow — 25 tabs in 200 columns

```
 < 3 more | 4 torvalds/linux | [5 MDN - fetch() API] | 6 grafana.prod | 7 Inbox (3) | 8 Hacker News | 17 more > | +                                                                               5/25  
  developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                                                               ^G menu  
```

Scrolling window keeps the active tab centred where possible; `< n more` / `n more >` are click targets.

### 4.9 Leader menu — overlay, appears ~250 ms after `^G` if no second key

```
  +----------------- ^G ------------------+
  |  t  new tab          + zoom in        |
  |  w  close tab        - zoom out       |
  |  u  undo close       0 zoom reset     |
  |  n/p  next/prev      s security info  |
  |  1-9  goto tab       y copy URL       |
  |  g/G  top/bottom     Y copy page text |
  |  r  hard reload      ?  full keymap   |
  |  q  quit             Esc cancel       |
  +---------------------------------------+

 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  ^G-                                                                                                                                                                 waiting for command   Esc cancel  
```

The delay is deliberate — a fluent user pressing `^G t` never sees the menu; a hesitant one gets discoverability for free. This is the tmux/which-key pattern and it is the reason a leader beats a wall of chords.

### 4.10 `--stats` mode

```
 [1 MDN - fetch() API] | 2 torvalds/linux | 3 grafana.prod | 4 Inbox (3) | 5 Hacker News | +                                                                                                       1/5  
  developer.mozilla.org/en-US/docs/Web/API/fetch                                                                                                           kitty  60fps  53KB  0.7ms  dmg 4%   ^G menu  
```

Off by default. This is the telemetry that F2 recommends moving out of the default bar.

---

## 5. Narrow layout — 80 x 24 tmux pane

Every line is **exactly 80 columns**, ASCII only. One chrome row. No tab strip.

### 5.1 Default

```
12345678901234567890123456789012345678901234567890123456789012345678901234567890
 1/5  developer.mozilla.org/en-US/docs/Web/API/fetch                        ^G  
```

### 5.2 Loading

```
 1/5  / connecting  developer.mozilla.org/...                     Esc stop  ^G  
 1/5  - loading  developer.mozilla.org/en-US/docs/...                  Esc  ^G  
```

The phase word is a droppable field (§6.2); under ~60 columns only the spinner survives.

### 5.3 Omnibox focused — takes the whole row, tab counter yields

```
 > developer.mozilla.org/en-US/docs/Web/API/fetch_                         Esc  
```

### 5.4 Find in page

```
 / abort signal_                                             3/17  ^N  ^P  Esc  
```

### 5.5 Insecure origin — the badge is never dropped

```
 1/3  !! NOT SECURE  192.168.1.1/admin/wan_settings                         ^G  
```

### 5.6 Zoom active, long URL elided in the middle

```
 developer.mozilla.org/...Web/API/fetch                              z125%  ^G  
```

Single tab, so the counter is suppressed. Elision is **middle-out**: the origin (left) and the leaf path segment (right) are the two things a user needs; the middle is the disposable part. Never elide the origin.

### 5.7 Leader menu at 80 columns

```
  +----------- ^G -----------+
  | t new tab    + zoom in   |
  | w close tab  - zoom out  |
  | n/p tab      0 zoom 100% |
  | 1-9 goto     s security  |
  | u undo close y copy URL  |
  | r hard reload ? keymap   |
  | q quit       Esc cancel  |
  +--------------------------+

 ^G-  waiting for command                                                  Esc  
```

### 5.8 Error page — **text, not pixels**

```
                         !!  This site can't be reached                         

                             developer.mozilla.org                              

                       net::ERR_NAME_NOT_RESOLVED  (-105)                       

                 The DNS lookup for this host did not resolve.                  

                     ^R retry    ^L edit address    ^G menu                     

 1/5  !! developer.mozilla.org                                    ^R retry  ^G  
```

### 5.9 Crashed tab

```
                      !!  This tab has stopped responding                       

                         reason: oom  (renderer killed)                         

                    ^R reload tab    ^W close tab    ^G menu                    

 1/5  !! CRASHED  developer.mozilla.org                          ^R reload  ^G  
```

Fed by the `crash` event `Status::apply_event` already handles (`main.rs:796-799`).

### 5.10 No-graphics tier

```
 1/5  developer.mozilla.org/en-US/docs/Web/API/fetch                  TEXT  ^G  
```

A persistent `TEXT` token, not a dismissible warning — the user must be able to tell at a glance that they are looking at a half-block approximation rather than the real page. `doctor` explains why (`unicode.rs:8-10`).

---

## 6. Responsive rules

### 6.1 Breakpoints

| Condition | Chrome rows | Tab strip | Behaviour |
|---|---|---|---|
| `cols >= 100 && rows >= 20` | 2 | Yes, row `H-1` | Full layout (§4). |
| `cols < 100 \|\| rows < 20` | 1 | No — `n/m` counter in the omnibox row | Narrow layout (§5). |
| `rows < 8` | 0 | No | Suppress chrome; page uses all rows; print a one-line note on `^G`. |
| `cols < 30` | 0 | No | Refuse to render. `pane too small (need >=30 cols)`. |

Breakpoints are evaluated **only** on startup and `SIGWINCH` (§3.3).

### 6.2 Field priority — build, then truncate to `cols`

Compose the omnibox/status row by adding fields in priority order and stopping when the next field would not fit. This is the direct fix for F2.

| Pri | Field | Example | Dropped when |
|---|---|---|---|
| 1 | Security badge, if not healthy | `!! NOT SECURE` | **Never.** Reserved before anything else. |
| 2 | Registrable domain | `mozilla.org` | Never. Elide from the left with `...` only below 20 cols. |
| 3 | Mode sigil | `>`, `/`, `^G-` | Never while that mode is active. |
| 4 | Full host | `developer.mozilla.org` | `cols < 50` -> registrable domain only. |
| 5 | Tab counter | `1/5` | Single tab, or `cols < 40`. |
| 6 | Loading spinner | `/` | Not loading. |
| 7 | Path, middle-elided | `/en-US/.../fetch` | `cols < 60`. |
| 8 | Find counter | `3/17` | Find inactive. |
| 9 | Zoom | `z125%` | Zoom == 100%. |
| 10 | Contextual hints | `Esc stop` | `cols < 70`. |
| 11 | `^G` hint | `^G menu` -> `^G` | `cols < 45`. |
| 12 | Stats | `60fps 53KB` | Unless `--stats`. |

Every string entering this row passes through the display-width truncator from F3, then the extended sanitizer from F4. **The row is clamped to `cols` as the last step, unconditionally**, so a bug upstream degrades to a truncated row rather than a wrapped one.

### 6.3 Tab strip fitting

Budget `cols - 8` (reserving the right-hand `n/m`). Per-tab width: `2 (index) + min(title_cols, 18) + 3 (separator)`. If total exceeds budget, shrink the title cap to 12, then 8; if it still overflows, switch to the scrolling window with `< n more` / `n more >` (§4.8). Titles are display-width truncated (F3) and middle-elided.

---

## 7. Component specs

### 7.1 Focus model

Exactly one focus owner at a time. The owner is visible from the row's leading sigil, so the user never has to guess where their keystrokes are going.

```
                    ^L                        Esc / Enter
        PAGE  ------------------>  OMNIBOX  ------------------>  PAGE
          |                                                        ^
          |  ^F                                    Esc             |
          +----------------->  FIND  ------------------------------+
          |                                                        |
          |  ^G                          second key / Esc          |
          +----------------->  LEADER-PENDING  --------------------+
```

| Owner | Sigil | Routing |
|---|---|---|
| `PAGE` | none | Tier-1 chords intercepted; everything else forwarded to the engine. |
| `OMNIBOX` | `>` | Tier-1 chords intercepted; readline editing keys handled locally; nothing forwarded. |
| `FIND` | `/` | Tier-1 chords intercepted; incremental query editing local; `^N`/`^P` walk matches. |
| `LEADER-PENDING` | `^G-` | Next key consumed as a command. 3 s timeout -> `PAGE`. |

Tier-1 chords are intercepted in **all** states, including `OMNIBOX` — otherwise a user who has focused the omnibox cannot quit.

Context-sensitive keys resolve through the owner, which removes the classic conflicts for free: `^W` is *delete previous word* in `OMNIBOX` and *close tab* in `PAGE`, so a URL typo never closes a tab. `^U` is *clear line* in `OMNIBOX` and *half page up* in `PAGE`. `^N`/`^P` walk suggestions in `OMNIBOX`, matches in `FIND`, and tabs in `PAGE`.

### 7.2 Tab strip

| State | Rendering | Notes |
|---|---|---|
| Active | `[n Title]`, reverse video | Brackets survive monochrome. |
| Inactive | ` n Title `, default | |
| Loading | ` n * Title ` | Same spinner tick as §7.4. |
| Crashed | ` n ! Title `, red `!` | From the `crash` event. |
| Audible | ` n ~ Title ` | **UNVERIFIED** — needs `webContents.isCurrentlyAudible()`; not checked this session. |
| New-tab affordance | ` + ` | Click target. |

Mouse: a click in the tab-strip row selects the tab under the cursor; middle-click closes it. This needs the strip's column ranges retained after render so a click column maps back to a tab id. `PointerMap::to_page` already rejects these rows (`py >= page_h`), so the chrome click handler must run **before** the page mapping, not after.

Per B04, exactly one tab paints at a time; a tab switch is `stopPainting()` then `startPainting()`, which B04 measured to emit a full frame at current geometry. So tab switching needs no extra repaint nudge from the chrome layer.

### 7.3 Security indicator

The design principle is **quiet when healthy, loud when not** — the modern consensus (Chrome removed the padlock in 2023 because users read it as "this site is trustworthy" rather than "this connection is encrypted"). It also buys back 3–4 columns in the 80-col layout, where they are scarce.

| State | Badge | Origin rendering |
|---|---|---|
| HTTPS, valid cert | *(none)* | `developer.mozilla.org` bright, `/path` dim, scheme hidden |
| HTTP | `!! NOT SECURE` red | Full `http://` shown |
| Cert invalid/expired/self-signed | `!! CERT INVALID` red + `NET::ERR_*` | Full URL |
| Mixed active content | `!! MIXED` yellow | Origin bright |
| `file://` | `file` dim | Path only |
| `about:`/`chrome:` internal | `internal` dim | Scheme + path |

Anti-spoofing requirements, all of which follow from F4:

1. The **registrable domain** is the only bright span. Subdomains dim, path dim. An attacker registering `mozilla.org.evil.com` gets `evil.com` highlighted.
2. Strip bidi and zero-width codepoints (F4) before rendering.
3. Punycode-encode a host whose labels mix scripts or whose script does not match the user's locale; show `xn--` form rather than the rendered homograph.
4. `^G s` opens a security-detail overlay: full URL untruncated, scheme, cert issuer/subject/validity, TLS version. This is where the truth lives when one row is not enough.

### 7.4 Loading state

Electron 43 has no progress percentage (§1.5), so the indicator is a **phase plus an indeterminate spinner**, both built from signals that exist:

| Phase | Derived from | Shown |
|---|---|---|
| `connecting` | `did-start-loading` and `isWaitingForResponse() == true` | `/ connecting` |
| `loading` | `isWaitingForResponse() == false`, before `dom-ready` | `- loading` |
| `rendering` | after `dom-ready`, before `did-stop-loading` | `\ rendering` |
| done | `did-stop-loading` | badge cleared |

Spinner frames `- \ | /`, ticked at **8 Hz maximum**, and the tick is the *only* thing allowed to dirty the chrome row on its own. Rationale in §9.

`Esc` in `PAGE` focus with a load in flight calls `stop()` (verified, line 18487). If nothing is loading, `Esc` is forwarded to the page — modal dialogs and web apps need it.

### 7.5 Back / forward / reload

| Action | Binding | Engine call |
|---|---|---|
| Back | `Ctrl+Left` (`Alt+Left` alias) | `navigationHistory.goBack()` |
| Forward | `Ctrl+Right` (`Alt+Right` alias) | `navigationHistory.goForward()` |
| Reload | `Ctrl+R` | `reload()` |
| Hard reload | `^G r` | `reloadIgnoringCache()` (line 18283) |
| Stop | `Esc` while loading | `stop()` |

Use `contents.navigationHistory.*`, not the deprecated top-level methods (§1.5). The engine should emit `canBack`/`canForward` so the chrome can dim the affordances; a back press at the start of history should be a no-op, not an error toast.

### 7.6 Find in page

Incremental: every keystroke issues `findInPage(q, {findNext: false})`; `^N`/`^P` issue `{findNext: true, forward: true|false}`. Results come from `found-in-page` as `activeMatchOrdinal` / `matches` (§1.5). `Esc` calls `stopFindInPage('clearSelection')`.

Two terminal-specific problems worth calling out. First, Chromium scrolls the match into view, which produces a page repaint — so find is one of the few chrome interactions with a real frame cost, and on a slow link the damage-rect encoder is what keeps it usable. Second, **the match highlight is drawn by Chromium into the page pixels**, so it is invisible in the half-block tier at 17x18 effective resolution. In that tier, find must additionally report the match's surrounding text in the chrome row (available from the text plane, A03 P0-9) or it is useless. `matchCase` is exposed via `^G ?`; the `FindInPageOptions` interface has no regex or whole-word option, so do not promise them.

### 7.7 Zoom

Ladder, clamped to Electron's documented default limits of 50%–300% (§1.5):

```
50  67  75  80  90  100  110  125  150  175  200  250  300
```

`setZoomLevel(level)` with `scale := 1.2 ^ level`, so `level = ln(factor)/ln(1.2)`. Shown in chrome only when `!= 100%`.

Two terminal-specific notes. Zoom is more load-bearing here than in a GUI browser: an 80x24 pane is a 1360x851 viewport, which is phone-sized, so zoom-out is the primary tool for fitting desktop layouts. But in the half-block tier, effective resolution is ~17x18 px per cell, and below roughly 75% body text stops resolving at all — so **clamp the fallback tier to 75% minimum** and say why rather than letting the user zoom into an unreadable smear.

The same-origin propagation gotcha from §1.5 is real: with one `BrowserWindow` per tab (B04), zooming tab 1 on `example.com` also zooms tab 3 on `example.com`. Either accept and document it, or set a distinct partition per tab. **UNVERIFIED** — I did not test whether B04's per-tab windows actually share zoom state; it should be probed before the tab strip ships a per-tab zoom indicator.

### 7.8 Error pages

Chromium renders its own net-error page, and we should **suppress it and draw text instead**. A rendered error page is ~348 KiB of wire for a document that is 95% white space (A03 §0.1), takes 3.5 s to arrive on a 10 Mb SSH link, and is unreadable in the half-block tier — a strictly worse outcome than 300 bytes of text that works everywhere.

Sequence on `did-fail-load` where `isMainFrame`:

1. Emit `\x1b_Ga=d,d=A\x1b\\` to delete the stale image — otherwise it stays composited under the text.
2. `\x1b[2J` clear, then draw the centred text block (§5.8).
3. Set the chrome badge to `!!` and offer `^R retry`.

Map the `errorCode` to a plain sentence; keep the raw `net::ERR_*` visible because the audience is developers and the raw code is what they will search for.

| `net::` code | Sentence |
|---|---|
| `ERR_NAME_NOT_RESOLVED` (-105) | The DNS lookup for this host did not resolve. |
| `ERR_CONNECTION_REFUSED` (-102) | The server refused the connection. |
| `ERR_CONNECTION_TIMED_OUT` (-118) | The server did not respond in time. |
| `ERR_INTERNET_DISCONNECTED` (-106) | This machine has no network route. |
| `ERR_CERT_DATE_INVALID` (-201) | The certificate is expired or not yet valid. |
| `ERR_CERT_AUTHORITY_INVALID` (-202) | The certificate was not issued by a trusted authority. |
| `ERR_TOO_MANY_REDIRECTS` (-310) | The server redirected too many times. |

Numeric codes are from Chromium's `net_error_list.h` and are **UNVERIFIED** in this session; the design does not depend on them, since `errorDescription` arrives in the event.

Certificate errors get an interstitial rather than a bare error, with the proceed path deliberately awkward — typed confirmation, per-host, logged, matching A03 P1-9's "must never be a blanket flag".

---

## 8. Keybindings

### 8.1 Hard rules

1. **Never bind `Ctrl+A` or `Ctrl+B`.** `Ctrl+B` is the tmux default prefix; `Ctrl+A` is GNU screen's prefix and the most common tmux rebinding. Both are installed on this box (§1.2).
2. **Never bind `Ctrl+Tab`, `Ctrl+Shift+Tab`, `Alt+Left`, `Alt+Right`, or `Shift+arrow`.** Ghostty consumes or rewrites all of them by default (§1.1).
3. **Never bind `Ctrl+I`, `Ctrl+M`, `Ctrl+J`, `Ctrl+H`, or `Ctrl+[`.** They are C0 aliases of Tab, Enter, LF, Backspace, and Escape, distinguishable only under the kitty keyboard protocol (A06 §2.6, lines 346-348) — so they work in Ghostty and break in Apple Terminal.
4. **Never bind `Ctrl+digit` or `Ctrl+punctuation` as the only path to an action.** Legacy encoding drops the modifier for digits (A06 §2.6, line 343). Tier-A-only, always with a leader fallback.
5. **Invariant: every action reachable by a Tier-A-only chord must also be reachable through the leader.** Nothing may be unreachable on Apple Terminal.

### 8.2 Tier 1 — always intercepted

Chosen so every one is expressible as a single C0 byte or a legacy CSI sequence, i.e. these all work with **no** kitty keyboard protocol.

| Key | Wire (legacy) | Action | Collision evidence |
|---|---|---|---|
| `Ctrl+L` | `0x0C` | Focus omnibox | Free in tmux root + Ghostty defaults. Browser-standard. |
| `Ctrl+F` | `0x06` | Find in page | Free. Browser-standard. |
| `Ctrl+R` | `0x12` | Reload | Free. Already shipped (`main.rs:573`). |
| `Ctrl+G` | `0x07` | Leader | Free. |
| `Ctrl+T` | `0x14` | New tab | Free. Browser-standard. |
| `Ctrl+W` | `0x17` | Close tab (PAGE) / delete word (OMNIBOX) | Free. |
| `Ctrl+N` | `0x0E` | Next tab | Free. `Ctrl+Tab` is unavailable (F8). |
| `Ctrl+P` | `0x10` | Previous tab | Free. |
| `Ctrl+Q` | `0x11` | Quit | Safe **only** because `IXON` is cleared (`tty.rs:110`). Already shipped. |
| `Ctrl+Left` | `CSI 1;5D` | Back | Free in both. Decoder handles it today (`input.rs:436-465`). Replaces the dead `Alt+Left` (F1). |
| `Ctrl+Right` | `CSI 1;5C` | Forward | Same. |
| `Ctrl+D` | `0x04` | Half page down | Free. less/vim muscle memory. |
| `Ctrl+U` | `0x15` | Half page up (PAGE) / clear line (OMNIBOX) | Free. |
| `Ctrl+C` | `0x03` | Copy selection, else forward to page | Reaches us because `ISIG` is cleared. **Cannot** interrupt (F7). |
| `Ctrl+Z` | `0x1A` | Suspend | Needs the `SIGTSTP`/`SIGCONT` work in F6. |
| `Esc` | `0x1B` | Dismiss overlay -> stop load -> forward to page | Ghostty's `escape` bind applies only inside its own search overlay. |

`Alt+Left`/`Alt+Right` remain accepted as aliases for terminals that deliver them. Ghostty's rewrite to `esc:b`/`esc:f` turns out to be convenient rather than merely hostile: those are readline word-motion, so under Ghostty defaults `Alt+arrow` gives the *page* word motion while `Ctrl+arrow` drives browser history. The two schemes compose instead of fighting.

### 8.3 Tier A — kitty keyboard protocol only

Available in Ghostty and kitty; unavailable in Apple Terminal (mission brief matrix). Each has a mandatory leader equivalent.

| Key | Action | Leader fallback |
|---|---|---|
| `Ctrl+=` / `Ctrl++` | Zoom in | `^G +` |
| `Ctrl+-` | Zoom out | `^G -` |
| `Ctrl+0` | Zoom reset | `^G 0` |
| `Ctrl+1..9` | Go to tab N | `^G 1..9` |
| `Ctrl+Shift+T` | Undo close tab | `^G u` |
| `Shift+Enter` | Submit without navigating | none needed |

### 8.4 Leader — `Ctrl+G`, then one key

| Key | Action | Key | Action |
|---|---|---|---|
| `t` | New tab | `+` | Zoom in |
| `w` | Close tab | `-` | Zoom out |
| `u` | Undo close tab | `0` | Zoom reset |
| `n` / `p` | Next / previous tab | `s` | Security details overlay |
| `1`–`9` | Go to tab N | `y` | Copy URL (OSC 52) |
| `g` / `G` | Top / bottom of page | `Y` | Copy page text (text plane) |
| `r` | Hard reload | `?` | Full keymap overlay |
| `d` | Toggle `--stats` | `q` | Quit |
| `Esc` | Cancel | | |

Menu overlay appears after 250 ms of hesitation; pending state times out to `PAGE` after 3 s.

### 8.5 Reserved — never bound, at any tier

`Ctrl+A`, `Ctrl+B`, `Ctrl+I`, `Ctrl+M`, `Ctrl+J`, `Ctrl+H`, `Ctrl+[`, `Ctrl+Tab`, `Ctrl+Shift+Tab`, `Alt+Left`/`Alt+Right` as primaries, `Shift+arrow`, `Shift+PageUp/Down`, `Shift+Home/End`.

### 8.6 tmux remediation

Under tmux, keys reach us intact (§1.2) but graphics and clipboard need configuration. `doctor` should print, and only when the probe fails:

```
set -g allow-passthrough on      # required for graphics (tmux >= 3.3)
set -g set-clipboard on          # required for ^G y  (default 'external' blocks inner apps)
```

For users who want `Alt+arrow` back as browser history in Ghostty specifically:

```
keybind = alt+arrow_left=unbind
keybind = alt+arrow_right=unbind
```

This should be a documentation note, never a default we write into anyone's config.

---

## 9. Chrome render budget

`Renderer::present` currently rewrites the whole status bar on **every** dirty frame (`main.rs:885-896`). At 147 bytes and the measured 60 fps that is 8.8 KB/s — irrelevant locally, but **70 kbit/s, or 3.5% of a 2 Mb hotel link**, spent on a status bar (A03 §0.1 bandwidth tiers).

Three rules:

1. **Track chrome dirtiness separately from frame dirtiness.** Repaint chrome only when a field's rendered text actually changes. A static page at 60 fps should emit **zero** chrome bytes.
2. **Cap the spinner at 8 Hz**, and let it dirty only the single cell it occupies.
3. **Repaint changed spans, not the whole row**, once the damage encoder (C08) exists — the chrome row is one more damage rect.

Ordering within a frame write matters and is already right in `present`: image first, then `\x1b[{rows};1H` and the bar. Keep chrome writes in the same `write_all` as the frame so a slow terminal cannot interleave a half-drawn image with a chrome update.

---

## 10. Security summary

| Risk | Mechanism | Mitigation |
|---|---|---|
| Origin spoofing via bidi override | `U+202E` etc. survive `sanitize_for_terminal` (F4) | Extend the rejected set; punycode mixed-script hosts |
| Origin spoofing via zero-width padding | `U+200B..U+200F` survive | Same |
| Layout corruption via wide chars | Truncation counts chars not columns (F3) | `truncate_to_columns` |
| Row wrap scrolling the image out of place | No clamp to `cols` (F2) | Unconditional final clamp |
| Escape injection through title/URL | Already handled | `sanitize_for_terminal` strips C0/C1 (`unicode.rs:68-72`), test at `unicode.rs:126-133` |
| Clipboard exfiltration via OSC 52 | `^G y` writes the clipboard | Only ever write the *sanitized* URL; never echo page-supplied bytes |
| Credential echo in the omnibox | Omnibox may contain a password in a URL | Never persist `userinfo` to history; mask `://user:pass@` in display |

---

## 11. Acceptance tests

### 11.1 Layout

| ID | Test | Pass |
|---|---|---|
| D06-L1 | Render every §4/§5 state at 200x50, 146x23, 100x30, 80x24, 60x20, 40x12, 30x8 | No line exceeds `cols`; no wrap |
| D06-L2 | Title = 40 CJK chars + emoji, URL = 300 chars | Row still `<= cols` (regression for F2+F3) |
| D06-L3 | `SIGWINCH` 146x23 -> 80x24 -> 146x23 | Chrome rows 2 -> 1 -> 2; page reflows; image re-placed at `(0,0)` |
| D06-L4 | Open/dismiss leader overlay 100x | Viewport never resizes; no reflow; page pixels restored from `Renderer::rgb` |
| D06-L5 | 25 tabs at 80 cols and 200 cols | No overflow; active tab visible in both |

### 11.2 Keybindings

| ID | Test | Pass |
|---|---|---|
| D06-K1 | Feed `\x1b[1;5D` to the decoder | Back fires (regression for F1) |
| D06-K2 | Feed `\x1bb` (what Ghostty sends for Alt+Left) | Forwarded to page as Alt+b; back does **not** fire |
| D06-K3 | Run under tmux, press every Tier-1 chord | All arrive; `Ctrl+B` still opens tmux's prefix |
| D06-K4 | Apple Terminal: exercise every leader command | All reachable (invariant 8.1.5) |
| D06-K5 | `Ctrl+Q` from `OMNIBOX`, `FIND`, `LEADER-PENDING` | Quits from all; terminal restored |
| D06-K6 | Triple-`Esc` within 500 ms with the engine hung | Force quit; terminal restored (F7) |
| D06-K7 | `Ctrl+Z` then `fg` | Terminal restored on suspend, raw mode + repaint on resume (F6) |

### 11.3 Reproducing the mockups

The generator lives outside the repo, per file-ownership rules:

```
/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/mock.py
python3 mock.py    # asserts every line is width-exact and printable-ASCII; 20 blocks pass
```

It should be promoted into `tests/` as a golden-file check so §4 and §5 cannot silently drift from the implementation.

---

## 12. UNVERIFIED / open items

1. **iTerm2 3.6.9** — blocked by macOS TCC (mission brief). Its default keybinds were not enumerated, so §8's collision table covers Ghostty and tmux only. iTerm2 ships many `Cmd`-based binds (harmless) but its `Option`-as-Meta default is unknown and could affect the `Alt` aliases.
2. **kitty terminal's own defaults** — kitty is not installed here. `kitty_mod` defaults to `ctrl+shift`, so `Ctrl+Shift+T` (§8.3) is likely consumed by kitty as `new_tab`. Verify before shipping that bind; the leader fallback `^G u` is unaffected either way.
3. **Zoom propagation across tabs** (§7.7) — the same-origin policy is documented in the Electron typings but not tested against B04's per-window tabs.
4. **`isCurrentlyAudible()`** for the audible-tab indicator (§7.2) — not checked in the typings.
5. **Chromium `net::` numeric codes** (§7.8) — sentences are ours; the numbers were not verified against `net_error_list.h` this session.
6. **Apple Terminal focus (1004) and bracketed paste (2004)** — inherited unknown from A06 §11.5. If focus reporting is absent, the chrome cannot dim on blur; degrade silently rather than assuming.
7. **Overlay repaint cost** — §3.3 asserts overlay dismissal is cheap because `Renderer::rgb` is retained. The re-encode cost of a partial repaint was not measured; C08's damage encoder should be the one to confirm it.
8. **Terminals that do not support reverse video** — the `[...]` brackets are the fallback, but no such terminal was tested.

---

## 13. Licensing

Nothing in this document is derived from third-party source. The Ghostty keybind list and the tmux key table were produced by running installed binaries with their own introspection flags (`+list-keybinds`, `list-keys`) — black-box observation of output, not code reading. Electron API facts come from the MIT-licensed `electron.d.ts` shipped in `apps/engine/node_modules/`. Ghostty is MIT; tmux is ISC; neither was copied. The keybinding scheme is an original design constrained by the measured collision surface.

---

## 14. Handoff checklist

Ordered by ratio of risk removed to effort.

1. **F1** — swap `Alt+Left`/`Right` for `Ctrl+Left`/`Right` in `main.rs:580-592`. ~10 lines. Removes a shipped feature that does not work.
2. **F2 + F3** — priority-ordered field list plus a display-width truncator, clamped to `cols`. Fixes visible corruption in every tmux pane.
3. **F4** — extend the sanitizer's rejected ranges. ~6 lines; closes an origin-spoofing hole.
4. **F7** — top-priority `Ctrl+Q` check plus triple-`Esc`. Cheap insurance against the failure mode the product most needs to avoid.
5. **F6** — `SIGTSTP`/`SIGCONT`. Needed before anyone runs this in a real shell session.
6. Chrome dirty-tracking (§9), then the tab strip (§7.2) once B04 lands, then find (§7.6), then zoom (§7.7).
7. **F5** stays open by design: revisit only if a top tab strip is ever wanted.
