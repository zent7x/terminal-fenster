# D01 — Keyboard Correctness Audit + IME/Composition Specification

**Mission:** audit `crates/bg-term/src/input.rs` against the Kitty keyboard protocol; enumerate the
functional-key table we are missing; specify how CJK/IME input must reach Chromium.
**Date:** 2026-07-31 · **Host:** macOS 26.1, Apple M4 arm64 · **Auditor:** D01
**Audited revision:** `crates/bg-term/src/input.rs` @ 768 lines (worktree state at `5215e1e`).
**Method:** every defect below was *executed*, not inferred. See §1.

> **File ownership.** This agent wrote only this file. No source under `crates/`, `apps/cli/`, or
> `apps/engine/` was modified. §8 states the required changes as a spec for the commander.

---

## 0. Verdict

The decoder's *framing* is sound — buffering, split-read reassembly, paste containment, and the
`flush_pending_escape` design are all correct and well-tested. The *semantics* are not. `input.rs`
implements roughly one fifth of the Kitty key vocabulary and mis-implements three parts of what it
does cover, and one of those defects types garbage into the page on every modifier keypress.

| # | Defect | Severity | Site |
|---|---|---|---|
| D01-1 | Kitty PUA functional keys (57344–63743) decode to `KeyCode::Char`, and Chromium **inserts them as text** — pressing Shift types `U+E061` into the focused field | **Critical** | `input.rs:431` |
| D01-2 | `57359` is mapped to `F1`. `57359` is `SCROLL_LOCK`; F1 has no `u`-form at all | **High** | `input.rs:424` |
| D01-3 | `split_params` discards any parameter that carries a sub-parameter, so **release and repeat events, and all modifiers, are silently lost on every `~`/letter-final key** (Delete, PageUp, arrows, F5+) | **High** | `input.rs:467`, `436` |
| D01-4 | Only flags `27` are requested (`CSI > 27 u`), not `31` — the base-layout key is never sent, so non-Latin layouts cannot fire shortcuts, and the non-ASCII keycode we do get resolves to `key=""`/`code=""`/`which=0` in Chromium | **High** | `tty.rs:167` (core), `input.rs:386` |
| D01-5 | Kitty key code `0` ("pure text event", the IME/dead-key path) decodes to `Char('\0')` and is sent to Chromium as a keystroke | **High** | `input.rs:431` |
| D01-6 | Modifier bit 8 is labelled `meta`; per spec bit 8 is **super**, bit 32 is meta. Bits 16/32/64/128 (hyper, meta, caps_lock, num_lock) are dropped entirely | **Medium** | `input.rs:28–36` |
| D01-7 | Shift+Tab (`CSI Z`) is `Unknown` → reverse focus traversal is dead | **Medium** | `input.rs:298` |
| D01-8 | Modified F1/F2/F4 (`CSI 1;m P/Q/S`, terminfo `kf13/kf14/kf16`) and `CSI 1;m E` are `Unknown` | **Medium** | `input.rs:443` |
| D01-9 | C0 gaps: `0x00` and `0x1c–0x1f` fall through to the text path and are **sent to the page as control characters**; `0x08` is decoded as plain Backspace, losing Ctrl+H / ctrl+backspace | **Medium** | `input.rs:219–229` |
| D01-10 | Unbounded buffer growth: an unterminated CSI or bracketed paste retains every byte fed. Measured 1 MB in, 1 MB retained, no cap, no resync | **Medium** | `input.rs:248`, `196` |
| D01-11 | A truncated `ESC O` wedges the decoder permanently (`flush_pending_escape` only handles a 1-byte buffer) | **Low** | `input.rs:148`, `306` |
| D01-12 | SS3 keypad forms (`ESC O M` = numpad Enter, `ESC O p`–`y`, `j`–`o`) are `Unknown`; Ghostty's own terminfo ships `kent=\EOM` | **Low** | `input.rs:311` |
| D01-13 | Alternate-key sub-fields (`shifted`, `base-layout`) are parsed off and thrown away | **Medium** | `input.rs:386` |

Downstream, in files this agent does not own: `apps/cli/src/main.rs:566` drops every
`KeyEventKind::Release` before it can be forwarded, so **Chromium never receives a `keyup`** even
though the decoder correctly produces one on the `u` path. That is the single largest functional gap
in the whole input chain, because we pay the full cost of the Kitty protocol (flag `2`) and then
discard the only thing it buys us.

**Single most actionable recommendation:** see §9.

---

## 1. Method and evidence

Three probes were written to the session scratchpad
(`/private/tmp/claude-501/-Users-adeebbashir-projects/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`)
and executed. Full source is in Appendix A so the results are reproducible after the scratchpad is
reaped.

| Probe | What it does | Command |
|---|---|---|
| `decode_probe.rs` | `#[path]`-includes the **real** `input.rs` and feeds it 48 spec-derived byte vectors | `rustc --edition 2021 -O -o decode_probe decode_probe.rs && ./decode_probe` |
| `ime-probe.js` | Electron 43.2.0 OSR window + CDP: `Input.imeSetComposition`, `Input.insertText`, `sendInputEvent` char, DOM composition-event capture | `.../Electron.app/Contents/MacOS/Electron ime-probe.js` |
| `key-probe.js`, `key-probe2.js` | compares `webContents.sendInputEvent` against `Input.dispatchKeyEvent` for the DOM key identity the page observes; enumerates which Electron Accelerator names resolve | same |
| `perf-probe.js` | per-key injection cost of both paths | same |

Electron runs required `dangerouslyDisableSandbox: true`, as documented in the mission brief; the
`bootstrap_look_up` failure is otherwise reproducible. Electron used is the one already vendored at
`apps/engine/node_modules/electron` — version `43.2.0`, matching the ADR. Nothing was downloaded.

Primary sources consulted (fetched 2026-07-31):

* Kitty keyboard protocol, canonical source:
  `https://raw.githubusercontent.com/kovidgoyal/kitty/master/docs/keyboard-protocol.rst` (789 lines).
  Line references below are into that file. **kitty is GPL-3.0** — the protocol is implemented from
  the published spec; no kitty source is reused.
* Chrome DevTools Protocol, `Input` domain: `https://chromedevtools.github.io/devtools-protocol/tot/Input/`.
* Electron `KeyboardInputEvent`: `https://www.electronjs.org/docs/latest/api/structures/keyboard-input-event`.
* Local terminfo, read with `infocmp`: `xterm-256color` (system) and `xterm-ghostty`
  (`/Applications/Ghostty.app/Contents/Resources/terminfo`).

This audit builds on `A06-input-research.md` and does not contradict it. Where A06 and the upstream
spec disagree, the disagreement is called out explicitly (§2.6, §4.3).

---

## 2. Kitty `CSI u` conformance

### 2.1 The wire format vs. what we parse

Spec (`keyboard-protocol.rst:136`):

```
CSI unicode-key-code:alternate-key-codes ; modifiers:event-type ; text-as-codepoints u
```

`decode_kitty_key` (`input.rs:380`) splits on `;` into three sections and on `:` within them. That
shape is right, and the handling of empty sub-fields is right — `\x1b[127;;65u` decodes correctly to
Backspace with text `"A"` (measured). Three things are then dropped or mishandled.

### 2.2 Modifiers — bit 8 is `super`, not `meta`

Spec (`:193`): `shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32, caps_lock 64, num_lock 128`,
transmitted as `1 + bitmask`. `Modifiers::from_kitty_param` gets the `+1` right and then assigns
bit 8 to a field called `meta` and ignores everything above it.

Measured, against the real decoder:

```
super(cmd)+a  mods=9      ESC[97;9u     -> shift:false alt:false ctrl:false meta:true
hyper+a       mods=17     ESC[97;17u    -> shift:false alt:false ctrl:false meta:false
meta+a        mods=33     ESC[97;33u    -> shift:false alt:false ctrl:false meta:false
caps_lock+a   mods=65     ESC[97;65u    -> shift:false alt:false ctrl:false meta:false
num_lock+KP_1 mods=129    ESC[57400;129u-> shift:false alt:false ctrl:false meta:false
```

The naming collision is *accidentally harmless today* on macOS, because Electron's modifier string
`'meta'` also means Command — `apps/engine/src/main.js:148` maps our `meta` → `'meta'`, and a probe
confirms `sendInputEvent({keyCode:'l', modifiers:['meta']})` yields `metaKey=true, code='KeyL'`. But
the field is mislabelled relative to the spec, real `meta` (32) is indistinguishable from no
modifier, and caps/num lock never reach the page. Chromium exposes
`KeyboardEvent.getModifierState('CapsLock')`, and pages use it; a probe shows Electron's
`sendInputEvent` accepts a `'capsLock'` modifier and the DOM reports it, so the information is
forwardable — we simply never decode it.

Ghostty never emits hyper (16) or meta (32) (A06 §1.4), so those two are cosmetic. Caps lock and
num lock are not.

### 2.3 Event types — lost on every non-`u` key

Spec (`:224`): the event type is a sub-field of the modifier field, and applies to *all* forms,
including the legacy-shaped ones:

```
CSI key-code;modifier:2   # repeat
CSI key-code;modifier:3   # release
```

`decode_kitty_key` handles this. `decode_legacy_csi` does not, and worse, the loss is silent:
`split_params` (`input.rs:467`) runs `parse_u32` over each `;`-separated field and `parse_u32`
rejects any field containing a `:`. The offending field is not defaulted — it is **removed from the
list**, so positional indices shift.

Measured:

```
Up release            ESC[1;1:3A  -> Key { code: Up,       ..., Press }     # should be Release
ctrl+Up release       ESC[1;5:3A  -> Key { code: Up,       ..., Press }     # ctrl also lost
Delete release        ESC[3;1:3~  -> Key { code: Delete,   ..., Press }     # should be Release
PageDown repeat ctrl  ESC[6;5:2~  -> Key { code: PageDown, ..., Press }     # ctrl + repeat lost
Enter release         ESC[13;1:3u -> Key { code: Enter,    ..., Release }   # correct ('u' path)
```

The user-visible failure is a stuck arrow key in any page that tracks keydown/keyup pairs (games,
canvas apps, drag interactions, `Mousetrap`-style shortcut libraries), plus Ctrl-modified navigation
keys (Ctrl+Home, Ctrl+ArrowLeft = word-jump in every text field) arriving unmodified.

### 2.4 Associated text — correct except for key code `0`

Spec (`:246`, `:256`): text codepoints are `:`-separated in the third section, must exclude control
codes, and *"if no known key is associated with the text the key number `0` must be used"*. The
spec's own example is `alt+a -> CSI 0 ; ; 229 u`.

Measured: multi-codepoint text works (`ESC[0;;72:105u` → `text: Some("Hi")`), but the key code is
mishandled — `char::from_u32(0)` is `Some('\0')`, so we emit `KeyCode::Char('\0')` and
`apps/cli/src/main.rs:741` turns that into `keyCode: "\u{0}"`. A probe of what Chromium does with
that:

```
sendInputEvent keyCode='\0'  ->  keydown key='' code='' which=0 ; keyup key='' code='' which=0
```

A bogus `keydown` with an empty `key` fires on the page ahead of every dead-key or IME commit.
This is the seam between §2 and §7: **key code 0 is not a key, it is the text channel.**

### 2.5 Alternate keys — requested? no. Used? no.

Spec (`:155`): with flag `0b100`, the terminal appends the *shifted key* and the *base layout key*
as `:`-sub-fields, and the base layout key is what makes shortcuts layout-independent —
*"if the user is using a Cyrillic keyboard … pressing ctrl+С will be ctrl+c in the standard layout.
So the terminal should send the base layout key as 99."*

Two independent problems:

1. `crates/bg-term/src/tty.rs:167` pushes `\x1b[>27u`. `27 = 1|2|8|16`. **Flag `4` is not set**, so
   Ghostty never sends the alternates in the first place. A06 §1.2 recommends `31`; the code says
   27 and the comment enumerates only four flags, so this reads as an oversight rather than a
   decision.
2. Even when they arrive, `decode_kitty_key` reads `key_parts[0]` and discards the rest.

Measured against the real decoder, with a hand-fed spec-shaped vector:

```
cyrillic ctrl+s -> base c   ESC[1089::99;5u -> Key { code: Char('с'), ctrl:true }
```

and measured against Chromium, that is a dead key:

```
sendInputEvent keyCode='с' modifiers=['control'] -> keydown key='' code='' which=0 mods=ctrl
```

whereas the CDP path, given the same base-layout information, produces exactly what a page needs:

```
Input.dispatchKeyEvent {key:'с', code:'KeyC', windowsVirtualKeyCode:67, modifiers:2}
  -> keydown key='с' code='KeyC' which=67 ctrlKey=true
```

So: Ctrl+C, Ctrl+V, Ctrl+T and every other shortcut are unreachable for any user on a non-Latin
layout, and the fix requires *both* asking for flag 4 and threading `base-layout-key` through to
`code`.

### 2.6 Two spec quirks worth encoding as tests

* `KP_BEGIN` is the only PUA code that uses a `~` final: the table row reads
  ``"KP_DELETE", "``57426 u``", "KP_BEGIN", "``1 E or 57427 ~``"`` (`keyboard-protocol.rst:643`).
  A06 lists it in both the `~` and the `u` group; the upstream file is authoritative and says `~`.
* F3 has **no** `CSI 1;m R` form. Upstream note at `:667`: the original spec allowed it, *"However,
  CSI R conflicts with the Cursor Position Report, so it was removed."* Modified F3 is `CSI 13;m ~`,
  which our decoder already handles correctly (measured: `ESC[13;2~ -> F(3), shift:true`). Local
  terminfo still ships the colliding form as `kf15=\E[1;2R` in both `xterm-256color` and
  `xterm-ghostty`, so a `CSI 1;2R` on the wire remains genuinely ambiguous. Current behaviour —
  `Unknown` — is the safe choice and should be kept, with a comment, unless a CPR query is
  outstanding.

---

## 3. The functional-key table we are missing

Today `decode_kitty_key` recognises **10** key codes (`13, 9, 127, 27, 57359, 2, 3, 5, 6, 7, 8`), one
of which (`57359`) is wrong and five of which (`2,3,5,6,7,8`) belong to the `~` form, not the `u`
form. Everything else in the Private Use Area falls into `KeyCode::Char`.

The **entire** PUA block is below, transcribed from `keyboard-protocol.rst:606–655`, with the DOM
identity each one must be given. `winVK` values marked ✓ were measured in this session; the rest are
the standard Windows virtual-key values and are marked `std`. "Accel" is the Electron Accelerator
string that `sendInputEvent` accepts — `—` means **no accelerator exists** and the key is
unreachable without CDP.

### 3.1 Locks, system keys

| Kitty | Code | DOM `key` | DOM `code` | winVK | Accel |
|---|---|---|---|---|---|
| CAPS_LOCK | 57358 | `CapsLock` | `CapsLock` | 20 std | `Capslock` |
| SCROLL_LOCK | 57359 | `ScrollLock` | `ScrollLock` | 145 ✓ | `Scrolllock` |
| NUM_LOCK | 57360 | `NumLock` | `NumLock` | 144 std | `Numlock` |
| PRINT_SCREEN | 57361 | `PrintScreen` | `PrintScreen` | 44 ✓ | `PrintScreen` |
| PAUSE | 57362 | `Pause` | `Pause` | 19 std | — |
| MENU | 57363 | `ContextMenu` | `ContextMenu` | 93 std | — (measured: `Menu` yields `key=''`) |

### 3.2 F13–F35

| Kitty | Codes | DOM `key`/`code` | winVK | Accel |
|---|---|---|---|---|
| F13…F24 | 57376–57387 | `F13`…`F24` | 124–135 (F13=124 ✓, F24=135 ✓) | `F13`…`F24` |
| F25…F35 | 57388–57398 | `F25`…`F35` | no Windows VK — send `0` | — |

F25–F35 have no Accelerator and no VK; they are only expressible through
`Input.dispatchKeyEvent` with `key`/`code` set and `windowsVirtualKeyCode: 0`. Whether Chromium
surfaces `code: "F25"` unchanged is **UNVERIFIED**.

### 3.3 Keypad

`location` must be `3` (`DOM_KEY_LOCATION_NUMPAD`) for every row here.

| Kitty | Code | `key` | `code` | winVK | Accel |
|---|---|---|---|---|---|
| KP_0…KP_9 | 57399–57408 | `0`…`9` | `Numpad0`…`Numpad9` | 96–105 (Numpad0=96 ✓) | `num0`…`num9` |
| KP_DECIMAL | 57409 | `.` | `NumpadDecimal` | 110 std | `numdec` |
| KP_DIVIDE | 57410 | `/` | `NumpadDivide` | 111 std | `numdiv` |
| KP_MULTIPLY | 57411 | `*` | `NumpadMultiply` | 106 std | `nummult` |
| KP_SUBTRACT | 57412 | `-` | `NumpadSubtract` | 109 std | `numsub` |
| KP_ADD | 57413 | `+` | `NumpadAdd` | 107 ✓ | `numadd` |
| KP_ENTER | 57414 | `Enter` | `NumpadEnter` | 13 std | — (`Return` loses the numpad location) |
| KP_EQUAL | 57415 | `=` | `NumpadEqual` | 187 std, UNVERIFIED | — |
| KP_SEPARATOR | 57416 | `,` | `NumpadComma` | 194 std | — |
| KP_LEFT | 57417 | `ArrowLeft` | `Numpad4` | 37 std | — |
| KP_RIGHT | 57418 | `ArrowRight` | `Numpad6` | 39 std | — |
| KP_UP | 57419 | `ArrowUp` | `Numpad8` | 38 std | — |
| KP_DOWN | 57420 | `ArrowDown` | `Numpad2` | 40 std | — |
| KP_PAGE_UP | 57421 | `PageUp` | `Numpad9` | 33 std | — |
| KP_PAGE_DOWN | 57422 | `PageDown` | `Numpad3` | 34 std | — |
| KP_HOME | 57423 | `Home` | `Numpad7` | 36 std | — |
| KP_END | 57424 | `End` | `Numpad1` | 35 std | — |
| KP_INSERT | 57425 | `Insert` | `Numpad0` | 45 std | — |
| KP_DELETE | 57426 | `Delete` | `NumpadDecimal` | 46 std | — |
| KP_BEGIN | 57427 (`~`) or `CSI 1 E` | `Clear` | `Numpad5` | 12 std | — |

The `KP_*` navigation rows are the num-lock-off aliases; the correct DOM shape is the navigation
`key` with the numpad `code` and `location: 3`. Accelerators cannot express that split at all —
`sendInputEvent({keyCode:'1', modifiers:['numLock','isKeypad']})` was measured to produce
`code='Digit1'`, i.e. the main-row key, which is wrong.

### 3.4 Media and volume

| Kitty | Code | `key` | `code` | winVK | Accel |
|---|---|---|---|---|---|
| MEDIA_PLAY | 57428 | `MediaPlay` | `MediaPlayPause` | 179 std | — |
| MEDIA_PAUSE | 57429 | `MediaPause` | `MediaPlayPause` | 179 std | — |
| MEDIA_PLAY_PAUSE | 57430 | `MediaPlayPause` | `MediaPlayPause` | 179 ✓ | `MediaPlayPause` |
| MEDIA_REVERSE | 57431 | `MediaRewind` | — | 0 | — |
| MEDIA_STOP | 57432 | `MediaStop` | `MediaStop` | 178 std | `MediaStop` |
| MEDIA_FAST_FORWARD | 57433 | `MediaFastForward` | — | 0 | — |
| MEDIA_REWIND | 57434 | `MediaRewind` | — | 0 | — |
| MEDIA_TRACK_NEXT | 57435 | `MediaTrackNext` | `MediaTrackNext` | 176 std | `MediaNextTrack` |
| MEDIA_TRACK_PREVIOUS | 57436 | `MediaTrackPrevious` | `MediaTrackPrevious` | 177 std | `MediaPreviousTrack` |
| MEDIA_RECORD | 57437 | `MediaRecord` | — | 0 | — |
| LOWER_VOLUME | 57438 | `AudioVolumeDown` | `AudioVolumeDown` | 174 std | `VolumeDown` |
| RAISE_VOLUME | 57439 | `AudioVolumeUp` | `AudioVolumeUp` | 175 std | `VolumeUp` |
| MUTE_VOLUME | 57440 | `AudioVolumeMute` | `AudioVolumeMute` | 173 std | `VolumeMute` |

These are low priority for a browser, but they must at minimum stop being typed into the page.

### 3.5 Modifier keys themselves — the ones causing D01-1

Flag `8` (which we *do* request) makes the terminal report modifier keypresses. Every one of these
currently becomes a `KeyCode::Char` in the PUA and is inserted as text.

| Kitty | Code | `key` | `code` | winVK | `location` |
|---|---|---|---|---|---|
| LEFT_SHIFT / RIGHT_SHIFT | 57441 / 57447 | `Shift` | `ShiftLeft` / `ShiftRight` | 16 ✓ | 1 / 2 |
| LEFT_CONTROL / RIGHT_CONTROL | 57442 / 57448 | `Control` | `ControlLeft` / `ControlRight` | 17 ✓ | 1 / 2 |
| LEFT_ALT / RIGHT_ALT | 57443 / 57449 | `Alt` | `AltLeft` / `AltRight` | 18 std | 1 / 2 |
| LEFT_SUPER / RIGHT_SUPER | 57444 / 57450 | `Meta` | `MetaLeft` / `MetaRight` | 91 / 92 std | 1 / 2 |
| LEFT_HYPER / RIGHT_HYPER | 57445 / 57451 | — | — | — | drop |
| LEFT_META / RIGHT_META | 57446 / 57452 | — | — | — | drop |
| ISO_LEVEL3_SHIFT | 57453 | `AltGraph` | `AltRight` | 225 std | 2 |
| ISO_LEVEL5_SHIFT | 57454 | `AltGraph` | — | 0 | drop |

Measured proof that the left/right split survives CDP, and that Electron accelerators cannot express
it:

```
Input.dispatchKeyEvent {key:'Shift', code:'ShiftRight', location:2}
  -> keydown key='Shift' code='ShiftRight' which=16 loc=2
sendInputEvent {keyCode:'Shift'}
  -> keydown key='Shift' code='ShiftLeft'  which=16 loc=0     # always left, location lost
```

### 3.6 Measured proof of D01-1

```
LEFT_SHIFT 57441      ESC[57441u -> Key { code: Char('\u{e061}'), Press }
RIGHT_CONTROL 57448   ESC[57448u -> Key { code: Char('\u{e068}'), Press }
KP_ENTER 57414        ESC[57414u -> Key { code: Char('\u{e046}'), Press }
F13 57376             ESC[57376u -> Key { code: Char('\u{e020}'), Press }
CAPS_LOCK 57358       ESC[57358u -> Key { code: Char('\u{e00e}'), Press }
SCROLL_LOCK 57359     ESC[57359u -> Key { code: F(1), Press }              # D01-2
MEDIA_PLAY 57428      ESC[57428u -> Key { code: Char('\u{e054}'), Press }
```

and end-to-end in Chromium (`key-probe.js`, step `sendInputEvent_pua_57441`), no exception is
thrown and the page receives:

```
keydown  key='' code='' which=0
keypress key='' code='' which=57441
input                                value=''
```

The user presses Shift in a text box and a private-use glyph is typed. `char::from_u32` succeeding
is exactly what makes this silent: the whole PUA range is valid `char`.

---

## 4. Legacy path defects

### 4.1 C0 controls sent to the page as text

`step_plain` handles `0x01..=0x1a` and falls through for the rest. Measured:

```
ctrl+space / ctrl+@ (0x00)  -> Key { code: Char('\0'),      text: Some("\0") }
ctrl+backslash (0x1c)       -> Key { code: Char('\u{1c}'),  text: Some("\u{1c}") }
ctrl+] (0x1d)               -> Key { code: Char('\u{1d}'),  text: Some("\u{1d}") }
ctrl+^ (0x1e)               -> Key { code: Char('\u{1e}'),  text: Some("\u{1e}") }
ctrl+_ (0x1f)               -> Key { code: Char('\u{1f}'),  text: Some("\u{1f}") }
```

`apps/cli/src/main.rs:741` forwards `text`, and `main.js:214` issues a `char` event per code unit, so
these become literal control characters in the DOM value. The spec's own ctrl table
(`keyboard-protocol.rst:696`) gives the inverse mapping to apply: `0x00 → space/@/2`,
`0x1c → \`, `0x1d → ]`, `0x1e → ^`, `0x1f → _`, `0x7f → 8/?`.

### 4.2 Backspace vs Ctrl+H

`input.rs:222` treats `0x7f | 0x08` as Backspace with no modifier. Per A06 §2.4 and the kitty legacy
table, `0x7f` is Backspace and `0x08` is **ctrl+backspace / Ctrl+H**. Web pages bind
ctrl+backspace to delete-word; we currently deliver a plain Backspace. `0x0a` likewise collapses
Ctrl+J into Enter — that collision is irrecoverable in legacy mode and should simply be commented,
since Kitty mode resolves it (`CSI 106;5u`, which we already decode correctly).

### 4.3 Missing legacy shapes

Measured `Unknown` for all of these:

```
shift+Tab (CSI Z)            ESC[Z      -> Unknown([27, 91, 90])
shift+F1  (CSI 1;2P)         ESC[1;2P   -> Unknown(...)      # terminfo kf13
shift+F2  (CSI 1;2Q)         ESC[1;2Q   -> Unknown(...)      # terminfo kf14
shift+F4  (CSI 1;2S)         ESC[1;2S   -> Unknown(...)      # terminfo kf16
KP_Begin  (CSI E)            ESC[E      -> Unknown(...)
MENU      (CSI 29~)          ESC[29~    -> Unknown(...)
F15/F17 xterm (CSI 28~/31~)  ESC[28~    -> Unknown(...)
shift+F3 vs CPR (CSI 1;2R)   ESC[1;2R   -> Unknown(...)      # keep as Unknown, see §2.6
SS3 keypad Enter (ESC O M)   ESCOM      -> Unknown(...)      # ghostty terminfo kent=\EOM
```

`kcbt=\E[Z` is present in both `xterm-256color` and `xterm-ghostty` terminfo on this machine, so
Shift+Tab is guaranteed to arrive in legacy mode and is guaranteed to be dropped.

On the `~`-number range: `decode_legacy_csi` covers `11..=15, 17..=21, 23..=26`. Local terminfo
(`infocmp xterm-ghostty`) shows Ghostty encodes F13–F24 as *modified* F1–F12
(`kf13=\E[1;2P … kf24=\E[24;2~`), not as `CSI 25~`/`28~`. The bare numbers `25, 26, 28, 29, 31–34`
therefore come from other terminfos (rxvt, linux console, vt220 lineage). kitty's spec assigns
`CSI 29 ~` to **MENU** (terminfo name `kf16`). Recommended policy, stated so it is a decision rather
than a gap: accept `29~` as `ContextMenu`, accept `25/26/28/31/32/33/34~` as F13–F20 in the
xterm-legacy numbering, and document that a terminal which uses `1;2P`-style modified-F1 encodings
will produce F1+Shift instead — which is the same physical key.

### 4.4 SS3 coverage

`step_ss3` accepts `A B C D H F P Q R S`. It should also accept `E` (KP_Begin) and the DECKPAM
application-keypad set that terminfo advertises: `M` (KP Enter), `j k l m n o` (`* + , - . /`),
`p`–`y` (KP 0–9), and `X` (KP `=`). Everything else stays `Unknown`.

---

## 5. Robustness

Measured:

```
unterminated CSI:   1_000_002 bytes fed -> 0 events, 1000002 bytes buffered
unterminated paste: 1_000_006 bytes fed -> 0 events (all held in paste_buf)
truncated SS3 'ESC O': pending=2, flush_pending_escape() -> false
```

Three consequences. First, a remote page or a hostile SSH peer that emits `ESC [` followed by an
endless digit run makes `blackglass` grow without bound; there is no ceiling and no resync. Second,
the same applies to a bracketed paste whose `ESC [201~` never arrives — relevant because §A09's
threat model treats paste as attacker-controlled. Third, a stream that ends mid-`ESC O` wedges the
decoder forever, because `flush_pending_escape` only fires on a 1-byte buffer.

Recommended, all cheap: a `MAX_SEQ` of 128 bytes (the longest legal Kitty sequence is well under
that) after which the decoder emits `Unknown` for the buffered prefix and resyncs at the next `ESC`;
a `MAX_PASTE` of 8 MiB after which the paste is truncated and flagged; and a
`flush_pending_sequence(timeout)` that generalises the existing escape-timeout policy to any stalled
partial sequence. The existing `garbage_bytes_do_not_panic` fuzz test does not catch these because
random bytes terminate a CSI almost immediately — a growth-bounded assertion is the missing test.

---

## 6. What Chromium actually receives today

Measured with `key-probe.js` / `key-probe2.js` against Electron 43.2.0 OSR. The good news first:
Electron's Accelerator resolution is better than expected, and the existing `electron_key` mapping in
`apps/cli/src/main.rs:741` produces correct DOM identities for everything it covers.

```
sendInputEvent keyCode='a'       -> keydown key='a'         code='KeyA'      which=65
sendInputEvent keyCode='Return'  -> keydown key='Enter'      code='Enter'     which=13
sendInputEvent keyCode='Escape'  -> keydown key='Escape'     code='Escape'    which=27
sendInputEvent keyCode='Left'    -> keydown key='ArrowLeft'  code='ArrowLeft' which=37
sendInputEvent keyCode='F5'      -> keydown key='F5'         code='F5'        which=116
sendInputEvent keyCode='Delete'  -> keydown key='Delete'     code='Delete'    which=46
sendInputEvent keyCode='F13'     -> keydown key='F13'        code='F13'       which=124
sendInputEvent keyCode='num0'    -> keydown key='0'          code='Numpad0'   which=96
sendInputEvent modifiers=['shift','capsLock'] -> shiftKey=true, getModifierState('CapsLock')=true
```

The bad news is the shape of the ceiling:

| Capability | `sendInputEvent` | `Input.dispatchKeyEvent` |
|---|---|---|
| non-ASCII key character (any non-Latin layout) | **no** — `key=''`, `code=''`, `which=0` (measured, Cyrillic `с`) | yes (measured) |
| `code` decoupled from `key` (base-layout shortcuts) | no | yes (measured: `key:'с', code:'KeyC'`) |
| `KeyboardEvent.repeat` (Kitty event type 2) | no field exists | yes, `autoRepeat` (measured `repeat=true`) |
| left/right modifier `location` | no — always `location=0`/`ShiftLeft` | yes, `location` (measured `loc=2`) |
| shifted `key` (`'A'` vs `'a'`) on keydown | no — stays `'a'` even with `shift` (measured) | yes, pass `key:'A'` (measured) |
| F25–F35, MENU, KP_EQUAL, KP_SEPARATOR, keypad-located nav | no accelerator exists | yes |
| caps lock / num lock modifier state | **yes** (`'capsLock'`, `'numLock'`) | **no** — CDP modifiers are only Alt=1, Ctrl=2, Meta=4, Shift=8 |
| synchronous, no debugger attach | yes | requires `webContents.debugger.attach` |

Neither is a superset. The recommendation in §8 is a hybrid, and the cost measurements say the
hybrid is affordable:

```
sendInputEvent            82.2 µs per key (keyDown+char+keyUp)
CDP dispatchKeyEvent      675.1 µs per key (awaited round-trip, keyDown+keyUp)
CDP dispatchKeyEvent      199.0 µs per key (pipelined, not awaited)
CDP imeSetComposition    1143.0 µs per call
```

675 µs is 4% of a 16.65 ms frame. At any human typing rate this is free; key injection is not on the
frame budget path, and the 60 fps figure from the OSR benchmark is unaffected.

---

## 7. IME and composition

### 7.1 What a terminal can actually deliver

There is no preedit protocol in any terminal specification — not kitty's, not xterm's. A06 §7
establishes this and this audit confirms it against the upstream spec: the only IME-adjacent
statement in `keyboard-protocol.rst` is the key-code-`0` rule at `:256`, which exists precisely
because *"the alt modifier is consumed by the OS itself to produce the text å and not sent to the
terminal emulator, which gets only a 'text input' event and no information about modifiers"*.

On macOS the terminal is an `NSTextInputClient`. The IME hands it `setMarkedText:` (uncommitted
preedit) and `insertText:` (committed). The terminal draws the preedit itself, inline, and forwards
**only the committed text** to the pty. So the two shapes we will observe are:

| Mode | Bytes on the wire for a CJK commit |
|---|---|
| Legacy (Apple Terminal 465) | a bare UTF-8 burst, e.g. `E6 97 A5 E6 9C AC` for 日本 — indistinguishable from fast typing |
| Kitty flags 27/31 (Ghostty, iTerm2) | `CSI 0 ; ; 26085 : 26412 u` — key code `0`, empty modifier field, text section carrying the codepoints |

Both are *committed-only*. We will never see `compositionupdate`-grade information. Escape and
Backspace during composition are swallowed by the IME and never reach us at all (A06 §7.4) — the
silence is correct behaviour, not a dropped event.

Our decoder handles the *text* half of the Kitty shape correctly today (measured:
`ESC[0;;72:105u -> text: Some("Hi")`) and the *key* half incorrectly (D01-5).

### 7.2 Measured: what Chromium accepts

`ime-probe.js`, Electron 43.2.0, `offscreen: true`, `webContents.debugger.attach('1.3')` → **ok**.
Attaching the debugger does not disturb `sendInputEvent`; both paths were exercised in the same
window.

`Input.imeSetComposition` produces genuine composition events under OSR:

```
Input.imeSetComposition {text:'にほn', selectionStart:3, selectionEnd:3}
  compositionstart  data=''
  compositionupdate data='にほn'
  beforeinput       data='にほn'  isComposing=true  inputType='insertCompositionText'
  input             data='にほn'  isComposing=true  inputType='insertCompositionText'  value='にほn'
```

A second call updates in place (`compositionupdate 'にほん'`, one composition, no spurious
`compositionend`). `Input.insertText` while a composition is live **commits** it:

```
Input.insertText {text:'日本'}
  compositionupdate data='日本'
  beforeinput       data='日本'  isComposing=true
  textInput         data='日本'
  input             data='日本'  value='日本'
  compositionend    data='日本'
```

`Input.imeSetComposition` with an empty string cancels, exactly as the CDP description says
(*"Use imeSetComposition with empty string as text to cancel composition"*) — measured
`compositionend data=''` and `value` back to `''`.

`Input.insertText` with no composition active is the clean committed-only path:

```
Input.insertText {text:'中文'}
  beforeinput data='中文' isComposing=false inputType='insertText'
  textInput   data='中文'
  input       data='中文' isComposing=false inputType='insertText'  value='中文'
```

For comparison, what we do today — `sendInputEvent({type:'char', keyCode:'中'})` — also inserts the
character, but wraps it in a fabricated `keydown`/`keyup` pair with `key=''`, `code=''`, `which=0`,
and no composition events at all:

```
keydown key='' code='' which=0
beforeinput data='中' inputType='insertText'
input       data='中' value='中'
keyup   key='' code='' which=0
```

Also measured: a multi-codepoint `keyCode` works for BMP-plus characters
(`sendInputEvent({type:'char', keyCode:'😀'})` inserts 😀), so the surrogate pair is not the problem.
The empty `key`/`code` on the surrounding key events is.

### 7.3 Specification

**Decision: use `Input.insertText` for committed IME text, not `sendInputEvent` `char`, and do not
synthesise composition events.**

Rationale. The measured behaviour of `Input.insertText` is precisely "text that did not come from a
key press" — which is the truth about what a terminal gives us. Synthesising a
`compositionstart`/`compositionend` pair around the burst (A06's fallback suggestion) would be a lie
with a real cost: `isComposing=true` makes editors such as CodeMirror, ProseMirror, Monaco and Slate
suppress their own input handling and wait for `compositionend`, and getting the pairing wrong on a
fast burst corrupts their document model. `insertText` gives those editors the same event shape they
get from an emoji picker or a paste, which they all handle correctly. So:

1. **Route by key code, not by heuristic.** Kitty key code `0` *is* the text channel. Add
   `Event::Text(String)` to the decoder's event enum (or `KeyCode::None`), emitted whenever
   `keynum == 0` and a text section is present. Never emit a `Key` event for key code 0.
2. **Legacy path (Apple Terminal).** There is no key code, so use a coalescing rule: printable
   characters outside ASCII that arrive with no preceding escape and no modifier are batched with a
   short flush window (recommend 8 ms, configurable, same knob family as the ESC timeout) and sent
   as one `insertText`. ASCII stays on the key path so that shortcuts and `keydown` handlers keep
   working. This is a heuristic and must be labelled as one in the code.
3. **Wire.** Add a command kind alongside `key` and `mouse`:
   `{"t":"input","kind":"text","text":"日本"}` → engine calls
   `wc.debugger.sendCommand('Input.insertText', {text})`.
4. **Preedit is out of scope, and say so.** Expose `imeSupport: "committed-only"` in the capability
   report the CLI already prints (`apps/cli/src/main.rs:147`), so the limitation is visible rather
   than mysterious. Keep `Input.imeSetComposition` wired but unused behind a flag: if we ever add a
   BlackGlass-native terminal shim, or if a terminal ever ships a preedit escape code, the
   measured-working call is already there and §7.2 documents its exact semantics.

### 7.4 Cursor parking — the part that decides whether CJK is usable

The macOS IME candidate window is positioned by the *terminal's* text cursor, which has nothing to do
with where the focused `<input>` is on our rendered page. Unless we move it, Japanese and Chinese
users will type into a candidate palette floating in the wrong place.

The lever, per A06 §7.2, is to keep the terminal's reported cursor parked at the caret cell. Concrete
design:

1. Inject a shim with `Page.addScriptToEvaluateOnNewDocument` that listens for `focusin`,
   `selectionchange`, and `scroll`, computes the caret rectangle
   (`getSelection().getRangeAt(0).getClientRects()[0]`, falling back to
   `document.activeElement.getBoundingClientRect()`), and reports it. The fallback was measured
   working under OSR: `{"tag":"INPUT","x":8,"y":8,"w":153,"h":21}`.
2. The engine forwards `{t:'caret', x, y, h}` in page pixels.
3. The CLI converts to cells using the already-known cell size (17×37 px on this display, from
   `CSI 16 t`) and emits `CSI <row> ; <col> H`.
4. **Open question, must be tested:** we currently hide the cursor with `\x1b[?25l`
   (`tty.rs:151`). Whether Ghostty still answers `firstRectForCharacterRange:` from a hidden cursor
   position is **UNVERIFIED**. If it does not, the options are to keep the cursor visible and paint
   over it, or to accept a mispositioned candidate window. This single test gates CJK usability and
   should be run by a human at an unlocked screen with a Japanese IME active.

### 7.5 Dead keys

Option+E then E on ABC-Extended behaves identically to an IME commit: no preedit, one committed
codepoint, arriving as `CSI 0 ; ; 233 u`. It needs no separate handling once key code 0 is routed to
text.

---

## 8. Required changes (spec for the commander — this agent did not edit these files)

In `crates/bg-term/src/input.rs`:

1. Add `KeyCode` variants for the full functional set in §3, and a `functional_key(u32) -> Option<KeyCode>`
   lookup covering 57358–57454 plus the `~`-form numbers. Then make the fallback arm reject the PUA
   range outright: `if (57344..=63743).contains(&keynum) { return Some(Unknown) }` rather than
   `KeyCode::Char`. This kills D01-1 and D01-2 together.
2. Delete `57359 => KeyCode::F(1)` and the `2,3,5,6,7,8` arms from the `u` path (they are `~`-form
   codes; on the `u` path those numbers are C0 controls).
3. Emit `Event::Text` for `keynum == 0`.
4. Replace `split_params` with a sub-parameter-aware parser returning `Vec<Vec<Option<u32>>>`, so
   `1;5:3` yields `[[Some(1)], [Some(5), Some(3)]]` and empty fields survive as `None`. Feed
   `decode_legacy_csi` from it so event type and modifiers work on `~`/letter finals.
5. Rename `Modifiers::meta` → `super_`, add `hyper`, `meta`, `caps_lock`, `num_lock`, and decode
   bits 16/32/64/128.
6. Keep the shifted key and base-layout key: extend `Event::Key` with
   `shifted: Option<char>, base: Option<char>`.
7. Legacy fixes: `CSI Z` → Tab+shift; `CSI 1;m {P,Q,S,E}`; `CSI 29~` → ContextMenu; `25/26/28/31–34~`
   → F13–F20; SS3 `E M j k l m n o p..y X`; `0x00` and `0x1c–0x1f` → the correct ctrl+char; `0x08` →
   Backspace **with `ctrl`**.
8. Bound the buffers (§5) and generalise `flush_pending_escape` to any stalled partial sequence.

In `crates/bg-term/src/tty.rs:167`: push `\x1b[>31u`, not `\x1b[>27u`, and update the comment to
list flag 4. Without this, item 6 above has nothing to parse.

In `apps/cli/src/main.rs`: stop dropping `KeyEventKind::Release` at line 566 — forward it as
`action:"up"`, and forward `Repeat` as a `keyDown` with `autoRepeat`. Emit `code` from the base
layout key when present, falling back to the primary key code. Emit the new `kind:"text"` command.

In `apps/engine/src/main.js`: attach `webContents.debugger` once at startup and route key injection
through `Input.dispatchKeyEvent` when `code`/`location`/`autoRepeat`/non-ASCII `key` is present,
keeping `sendInputEvent` for the simple ASCII path (it is 8× cheaper and it is the only path that can
carry caps-lock/num-lock state). Add `Input.insertText` for `kind:"text"`.

---

## 9. Recommendation

**Fix D01-1 first, in isolation, today: make `decode_kitty_key` refuse the Private Use Area.** It is
a five-line change, it has a one-line test, and it is the only defect on this list that actively
corrupts what the user types — with flag `8` already enabled in `tty.rs`, every Shift, Ctrl and Alt
press in Ghostty inserts a private-use glyph into the focused field. Everything else in this audit
degrades capability; this one produces wrong output. Ship it ahead of the larger table work.

## 10. Test vectors to add

All verified against the spec; expected values are what the *fixed* decoder should produce.

```
ESC[57441u        -> Key{ LeftShift,  Press }            # not Char(U+E061)
ESC[57359u        -> Key{ ScrollLock, Press }            # not F(1)
ESC[57414u        -> Key{ KpEnter,    Press }
ESC[0;;229u       -> Text("å")                           # not Key{Char('\0')}
ESC[0;;72:105u    -> Text("Hi")
ESC[1;5:3A        -> Key{ Up, ctrl, Release }
ESC[3;1:3~        -> Key{ Delete, Release }
ESC[6;5:2~        -> Key{ PageDown, ctrl, Repeat }
ESC[97;9u         -> Key{ Char('a'), super }
ESC[97;65u        -> Key{ Char('a'), caps_lock }
ESC[1089::99;5u   -> Key{ Char('с'), base=Some('c'), ctrl }
ESC[97:65;2u      -> Key{ Char('a'), shifted=Some('A'), shift }
ESC[Z             -> Key{ Tab, shift }
ESC[1;2P          -> Key{ F(1), shift }
ESC[29~           -> Key{ ContextMenu }
ESCOM             -> Key{ KpEnter }
0x00              -> Key{ Char(' '), ctrl }
0x1c              -> Key{ Char('\\'), ctrl }
0x08              -> Key{ Backspace, ctrl }
ESC[ + 200_000 digits, no final byte  -> Unknown emitted, pending() <= MAX_SEQ
ESC[200~ + 16 MiB, no terminator      -> paste truncated at MAX_PASTE, buffer bounded
```

## 11. Open items / UNVERIFIED

* Whether Ghostty answers `firstRectForCharacterRange:` while the cursor is hidden (`\x1b[?25l`).
  Gates §7.4. Needs an unlocked screen and a Japanese IME.
* Whether Chromium accepts `code: "F25".."F35"` unchanged (no Windows VK exists).
* `NumpadEqual` Windows VK value (187 is the main-row `=`; Chromium may use a different constant).
* iTerm2 3.6.9 Kitty-keyboard behaviour at runtime — A06 verified flag push/pop, but no key event
  vectors were captured. TCC blocks automation.
* Whether any terminal in scope actually emits `CSI 29 ~` for MENU; Ghostty's terminfo maps `kf16`
  to `\E[1;2S` instead.

## 12. Licenses

* kitty — GPL-3.0. Protocol implemented from the published spec; **no kitty source reused**.
* Ghostty — MIT. Test vectors quoted via A06 are byte strings from its encoder tests, which are
  facts about the wire, not code.
* Electron — MIT. Chrome DevTools Protocol is a published interface specification.
* terminfo data read locally with `infocmp` from system and Ghostty-shipped databases; no
  redistribution.

---

## Appendix A — probe sources

Reproduce with `rustc --edition 2021 -O -o decode_probe decode_probe.rs && ./decode_probe`, and
`apps/engine/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron <probe>.js`.

**`decode_probe.rs`** (abbreviated to its mechanism; the vector list is §10 plus §3.6/§4.1/§4.3):

```rust
#[path = "/Users/adeebbashir/projects/blackglass/crates/bg-term/src/input.rs"]
mod input;
use input::{Decoder, Event};

fn show(label: &str, bytes: &[u8]) {
    let mut d = Decoder::new(true);
    for e in d.decode(bytes) { println!("{label:<34} -> {e:?}"); }
}
// main() calls show(..) for every vector, then feeds
//   b"\x1b[" + 1_000_000 * b'1'         -> prints d.pending()
//   b"\x1b[200~" + 1_000_000 * b'A'     -> prints event count
//   b"\x1bO"                            -> prints pending() and flush_pending_escape()
```

**`ime-probe.js`** (mechanism; full listing recreated from §7.2 expectations):

```js
const win = new BrowserWindow({ show: false, webPreferences: { offscreen: true, sandbox: false } });
const wc = win.webContents;
await wc.loadURL(pageWithInputAndCompositionListeners);
wc.focus();
wc.debugger.attach('1.3');
const cdp = (m, p) => wc.debugger.sendCommand(m, p);

await cdp('Input.imeSetComposition', { text: 'にほn', selectionStart: 3, selectionEnd: 3 });
await cdp('Input.imeSetComposition', { text: 'にほん', selectionStart: 3, selectionEnd: 3 });
await cdp('Input.insertText', { text: '日本' });                 // commits, fires compositionend
await cdp('Input.imeSetComposition', { text: '', selectionStart: 0, selectionEnd: 0 }); // cancels
await cdp('Input.insertText', { text: '中文' });                 // committed-only path
wc.sendInputEvent({ type: 'char', keyCode: '中' });              // what we do today
```

The listener page records `compositionstart|compositionupdate|compositionend|beforeinput|input|
keydown|keyup|textInput` with `data`, `key`, `code`, `isComposing`, `inputType`, and the field value,
into `window.log`, read back with `executeJavaScript('JSON.stringify(window.log)')`.

**`key-probe.js` / `key-probe2.js`**: same harness, iterating `sendInputEvent` over
`['a','Return','Escape','Left','F5','Backspace','Tab','Delete','PageUp','F13','F24','num0','numadd',
'PrintScreen','MediaPlayPause','Shift','Control','Insert','Menu','Scrolllock','с','\u{0}','\u{e061}']`
and `Input.dispatchKeyEvent` over the `{key, code, windowsVirtualKeyCode, location, autoRepeat,
modifiers}` combinations in §6. Listeners must be attached to `document`, not the `<input>` — a `Tab`
keystroke moves focus and silently ends capture otherwise.

**`perf-probe.js`**: 300 iterations of each injection path, timed with `process.hrtime.bigint()`,
plus 50 `Input.imeSetComposition` calls; results in §6.
