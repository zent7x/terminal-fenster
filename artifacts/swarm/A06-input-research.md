# A06 — Terminal Input Protocols: Implementation Spec

**Mission:** exact, implementable decoding spec for Terminal-Fenster terminal input.
**Date:** 2026-07-31 · **Host:** macOS 26.1, Apple M4 arm64
**Terminals under test:** Ghostty 1.3.1, iTerm2 3.6.9, Apple Terminal 465

Byte notation throughout: `ESC` = `0x1B`, `CSI` = `ESC [` = `0x1B 0x5B`, `SS3` = `ESC O` = `0x1B 0x4F`,
`OSC` = `ESC ]` = `0x1B 0x5D`, `ST` = `ESC \` = `0x1B 0x5C`, `BEL` = `0x07`, `DCS` = `ESC P` = `0x1B 0x50`.

---

## 0. TL;DR for implementers

| Capability | Ghostty 1.3.1 | iTerm2 3.6.9 | Apple Terminal 465 |
|---|---|---|---|
| Kitty keyboard (`CSI ? u`) | YES (src+terminfo `fullkbd`) | **YES — verified at runtime** | **NO — verified (no reply)** |
| DECRQM (`CSI ? Ps $ p`) | YES (src) | **YES — verified** | **NO — verified (zero replies)** |
| SGR mouse 1006 | YES | **YES (verified `;2$y`)** | YES (terminfo `xm`, unverifiable via DECRQM) |
| **SGR-Pixels mouse 1016** | **YES (src verified)** | **NO — verified `CSI ?1016;4$y` (permanently reset)** | **NO** |
| Focus events 1004 | YES | **YES (verified `;2$y`)** | Partial/unknown — no DECRQM |
| Bracketed paste 2004 | YES | **YES (verified `;2$y`)** | YES (documented, no DECRQM) |
| OSC 52 write | YES (default `allow`) | Yes (guarded by pref) | **NO — verified (no effect/reply)** |
| OSC 52 **read** | **Default = `ask` → modal prompt** | **Blocked by default — verified (no reply)** | **NO — verified** |
| XTVERSION `CSI > 0 q` | YES | **YES — `DCS >\|iTerm2 3.6.9 ST`** | **NO — verified** |
| `CSI 16 t` (cell px) | YES (src) | **NO — verified** | **NO — verified** |
| `CSI 14 t` (window px) | YES | **NO — verified** | **YES — `CSI 4;467;860t`** |
| `TIOCGWINSZ` ws_xpixel | YES (src) | UNVERIFIED | **YES — verified 840×450 @120×30 → 7×15 pt/cell** |

**The single most important finding: pixel-accurate mouse is NOT portable.** iTerm2 3.6.9 explicitly
reports mode 1016 as *permanently reset* (value 4) — it will never enable. Apple Terminal has no mode
1016 at all. Terminal-Fenster must implement a **sub-cell coordinate synthesis fallback** (§3.6).

---

## 1. Kitty Keyboard Protocol

Primary source: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/> ·
repo doc: <https://github.com/kovidgoyal/kitty/blob/master/docs/keyboard-protocol.rst>
**License of kitty: GPL-3.0.** The *protocol* is a published spec and free to implement; **do not copy
kitty C source into Terminal-Fenster.** Ghostty is **MIT** — its Zig source is safe to learn from and, with
attribution, to adapt.

### 1.1 Control sequences

| Purpose | Bytes | Hex |
|---|---|---|
| Query current flags | `CSI ? u` | `1B 5B 3F 75` |
| Query **reply** | `CSI ? <flags> u` | e.g. `1B 5B 3F 30 75` = flags 0 |
| Set flags | `CSI = <flags> ; <mode> u` | `1B 5B 3D … 3B … 75` |
| **Push** flags (recommended) | `CSI > <flags> u` | `1B 5B 3E … 75` |
| **Pop** N frames | `CSI < <N> u` | `1B 5B 3C … 75` |

* `CSI = flags ; mode u` — `mode` defaults to `1`.
  `1` = set all set-bits, reset all unset-bits (assign).
  `2` = OR (set bits only).
  `3` = AND-NOT (reset the given bits).
* `CSI > flags u` — push; `flags` omitted ⇒ `0`.
* `CSI < N u` — pop; `N` omitted ⇒ `1`.

**Stack depth:** the spec does not fix a depth. Ghostty implements a **fixed 8-entry ring** that
*evicts the oldest entry on overflow* and resets to all-disabled if `N >= 8`
(`ghostty/src/terminal/kitty/key.zig`, `FlagStack.len = 8`). Treat depth 8 as the portable ceiling;
never nest more than 2–3 frames.

### 1.2 Flag bits — VERIFIED

| Bit | Value | Name | Effect |
|---|---|---|---|
| `0b00001` | 1 | Disambiguate escape codes | Esc, `alt+key`, `ctrl+key`, `ctrl+alt+key`, `shift+alt+key` become `CSI … u` |
| `0b00010` | 2 | Report event types | adds `:press/repeat/release` sub-param; enables **release** events |
| `0b00100` | 4 | Report alternate keys | adds `:shifted:base-layout` sub-params to the key field |
| `0b01000` | 8 | Report all keys as escape codes | even plain text keys become `CSI … u`; modifier keys themselves (`57441`+) report |
| `0b10000` | 16 | Report associated text | appends `;<cp>:<cp>…` text field |

`31` = all five. Verified end-to-end on iTerm2 3.6.9: `CSI > 31 u` then `CSI ? u` → `CSI ? 31 u`;
`CSI < 1 u` then `CSI ? u` → `CSI ? 0 u`.

**Terminal-Fenster should request `1|2|4|8|16 = 31`.** A browser needs keydown/keyup separation (bit 2 + 8),
layout-independent shortcuts (bit 4), and `KeyboardEvent.key` text (bit 16).

### 1.3 Key event wire format

```
CSI <key>[:<shifted>[:<base>]] [ ; <mods>[:<event>] ] [ ; <text-cp>[:<text-cp>…] ] <final>
```

* `<final>` is `u` for Unicode/text keys, `~` for VT-style numbered functional keys, and
  `A B C D E F H P Q S` for the legacy-shaped keys (arrows, Home/End, KP_Begin, F1/F2/F4).
* Omitted `<mods>` ⇒ `1`. Omitted `<event>` ⇒ `1` (press). Omitted `<shifted>`/`<base>` ⇒ absent.
* **Empty sub-fields are legal and load-bearing.** `CSI 97 ; ; 229 u` = key 97, *no* modifier field,
  text = U+00E5. `CSI 65 :: 97 ; 2 u` = key 65, no shifted-key, base-layout-key 97.
  A decoder that splits on `;`/`:` **must tolerate zero-length components** and substitute defaults.
* Text codepoints exclude all control characters (< U+0020 and the C1 block).

Concrete, taken from Ghostty's own unit tests (`src/input/key_encode.zig`), i.e. bytes a real
terminal emits:

| Input | Emitted |
|---|---|
| backspace | `\x1b[127u` |
| backspace + text 'A' | `\x1b[127;;65u` |
| enter release | `\x1b[13;1:3u` |
| backspace release | `\x1b[127;1:3u` |
| tab release | `\x1b[9;1:3u` |
| shift+backspace | `\x1b[127;2u` |
| shift+enter | `\x1b[13;2u` |
| shift+tab | `\x1b[9;2u` |
| left-shift key itself | `\x1b[57441u` |
| shift+left-shift | `\x1b[57441;2u` |
| shift+a (alternates on) | `\x1b[97:65;2u` |
| shift+a (base-layout only) | `\x1b[65::97;2u` |
| shift+j with text | `\x1b[106;65;74u` |
| `;`/`:` key | `\x1b[59:58;2;58u` |
| alt+w → `∑` (opt-as-alt off) | `\x1b[119;;8721u` |
| alt+w (opt-as-alt on) | `\x1b[119;3u` |
| ctrl+j | `\x1b[106;5u` |
| shift+j alternates+text | `\x1b[106:74;2;74u` |
| Delete | `\x1b[3~` |
| Up (no kitty flags) | `\x1b[A` |

Note the ordering quirk in `\x1b[59:58;2;58u`: **shifted-key `58` appears before the mods**, and the
associated text repeats it. Do not assume the text field mirrors `<key>`.

### 1.4 Modifier bitmask — VERIFIED

| Modifier | Bit | Value |
|---|---|---|
| shift | `0b00000001` | 1 |
| alt | `0b00000010` | 2 |
| ctrl | `0b00000100` | 4 |
| super (⌘ on macOS) | `0b00001000` | 8 |
| hyper | `0b00010000` | 16 |
| meta | `0b00100000` | 32 |
| caps_lock | `0b01000000` | 64 |
| num_lock | `0b10000000` | 128 |

**Transmitted value = bitmask + 1.** shift alone → `2`; ctrl+shift → `1 + 0b101` = `6`; no mods → `1`.
Decoder: `mods = param - 1` (clamp `param >= 1`; treat `0`/missing as no mods).

Practical caveat (Ghostty `KittyMods.fromInput`): Ghostty **never sets `hyper` (16) or `meta` (32)** —
it maps only shift/alt/ctrl/super/caps_lock/num_lock. Do not depend on hyper/meta being distinguishable.

### 1.5 Event types

`1` = press (default when absent), `2` = repeat, `3` = release. Only delivered when flag bit `2` is on.
Kitty *omits* `:1` for press; Ghostty **includes** it in some paths (see `KittySequence.Event`, comment:
"Kitty omits the `:1` for the press event but other terminals include it. We'll include it."). Accept both.

Note `\x1b[13;1:3u` — a release with **no modifiers** still carries `mods=1`, because the event sub-param
cannot exist without its parent.

### 1.6 Functional key codes

CSI-number / final-byte form (legacy-shaped):

```
ESCAPE 27u  ENTER 13u  TAB 9u  BACKSPACE 127u
INSERT 2~   DELETE 3~  PAGE_UP 5~  PAGE_DOWN 6~
HOME  1H or 7~     END  1F or 8~
UP 1A  DOWN 1B  RIGHT 1C  LEFT 1D  KP_BEGIN 1E or 57427~
F1 1P or 11~   F2 1Q or 12~   F3 13~   F4 1S or 14~
F5 15~  F6 17~  F7 18~  F8 19~  F9 20~  F10 21~  F11 23~  F12 24~
```

**`F3` has no `CSI 1;m R` form** — `CSI … R` collides with CPR (cursor position report). Terminals emit
`CSI 13;m ~` for modified F3. Ghostty's table confirms: `.f3 = pcStyle("\x1b[13;{}~")` with unmodified
`\x1bOR`. A decoder that sees `CSI 1;2R` must decide from context whether it is shift+F3 or a CPR reply —
**always disambiguate by whether you have an outstanding CPR query.**

Private Use Area codes (final byte `u`):

```
CAPS_LOCK 57358  SCROLL_LOCK 57359  NUM_LOCK 57360  PRINT_SCREEN 57361
PAUSE 57362  MENU 57363
F13–F35 57376–57398
KP_0–KP_9 57399–57408   KP_DECIMAL 57409  KP_DIVIDE 57410  KP_MULTIPLY 57411
KP_SUBTRACT 57412  KP_ADD 57413  KP_ENTER 57414  KP_EQUAL 57415  KP_SEPARATOR 57416
KP_LEFT 57417  KP_RIGHT 57418  KP_UP 57419  KP_DOWN 57420
KP_PAGE_UP 57421  KP_PAGE_DOWN 57422  KP_HOME 57423  KP_END 57424
KP_INSERT 57425  KP_DELETE 57426  KP_BEGIN 57427
MEDIA_PLAY 57428  MEDIA_PAUSE 57429  MEDIA_PLAY_PAUSE 57430  MEDIA_REVERSE 57431
MEDIA_STOP 57432  MEDIA_FAST_FORWARD 57433  MEDIA_REWIND 57434
MEDIA_TRACK_NEXT 57435  MEDIA_TRACK_PREVIOUS 57436  MEDIA_RECORD 57437
LOWER_VOLUME 57438  RAISE_VOLUME 57439  MUTE_VOLUME 57440
LEFT_SHIFT 57441   LEFT_CONTROL 57442  LEFT_ALT 57443  LEFT_SUPER 57444
LEFT_HYPER 57445   LEFT_META 57446
RIGHT_SHIFT 57447  RIGHT_CONTROL 57448 RIGHT_ALT 57449 RIGHT_SUPER 57450
RIGHT_HYPER 57451  RIGHT_META 57452
ISO_LEVEL3_SHIFT 57453  ISO_LEVEL5_SHIFT 57454
```

Whole functional PUA range: **U+E000–U+F8FF (57344–63743)**. Any `CSI <n> u` with `n` in that range is a
functional key, not text — never route it to the page as character input.

### 1.7 Feature detection (do this, in this order)

```
1. Write:  CSI ? u   (1B 5B 3F 75)
2. Write:  CSI c     (1B 5B 63)     ← DA1, the "fence"
3. Read until you see a DA1 reply (CSI ? … c).
   - If a CSI ? <flags> u arrived first → kitty keyboard supported.
   - If only the DA1 reply arrived    → NOT supported. No timeout needed.
```

The DA1 fence is essential: every terminal in scope answers DA1, so you never block on a timeout.
Verified DA1 replies: Apple Terminal `CSI ?1;2c`; iTerm2 `CSI ?64;1;2;4;6;17;18;21;22;52c`.

Enable/disable with push/pop, never bare `CSI = … u`, so you can restore whatever the parent app set:

```
enter:  ESC [ > 31 u          (1B 5B 3E 33 31 75)
leave:  ESC [ < 1 u           (1B 5B 3C 31 75)
```

Ghostty's shipped terminfo (`/Applications/Ghostty.app/Contents/Resources/terminfo/78/xterm-ghostty`)
carries the boolean cap `fullkbd`, the conventional terminfo marker for kitty-keyboard support. Reading
`fullkbd` is a cheap static hint; the `CSI ? u` probe remains authoritative.

---

## 2. Legacy key encoding (fallback path)

Primary source: xterm `ctlseqs` (Thomas Dickey), <https://invisible-island.net/xterm/ctlseqs/ctlseqs.txt>
(fetched 2026-07-31, 3835 lines). **Apple Terminal 465 gives you only this path.**

### 2.1 Cursor / editing keys

DECCKM (`CSI ? 1 h` / `l`) selects normal vs application cursor keys. Home and End are cursor keys.

| Key | Normal | Application |
|---|---|---|
| Up | `CSI A` = `1B 5B 41` | `SS3 A` = `1B 4F 41` |
| Down | `CSI B` | `SS3 B` |
| Right | `CSI C` | `SS3 C` |
| Left | `CSI D` | `SS3 D` |
| Home | `CSI H` | `SS3 H` |
| End | `CSI F` | `SS3 F` |

Ghostty's shipped terminfo declares `kcuu1=\EOA kcud1=\EOB kcuf1=\EOC kcub1=\EOD khome=\EOH kend=\EOF`
— i.e. **SS3 form by default**, because `smkx` (`\E[?1h\E=`) is applied on entering full-screen apps.
Decode both forms unconditionally.

| Key | Bytes |
|---|---|
| Insert | `CSI 2 ~` |
| Delete | `CSI 3 ~` |
| PageUp | `CSI 5 ~` |
| PageDown | `CSI 6 ~` |
| Home (alt) | `CSI 1 ~` or `CSI 7 ~` |
| End (alt) | `CSI 4 ~` or `CSI 8 ~` |

### 2.2 Function keys

```
F1  SS3 P    (1B 4F 50)      F5  CSI 15 ~     F9  CSI 20 ~
F2  SS3 Q                    F6  CSI 17 ~     F10 CSI 21 ~
F3  SS3 R                    F7  CSI 18 ~     F11 CSI 23 ~
F4  SS3 S                    F8  CSI 19 ~     F12 CSI 24 ~
```
Deprecated `oldXtermFKeys` variant: F1–F4 = `CSI 11~ 12~ 13~ 14~`. Ghostty's terminfo ships
`kf1=\EOP … kf4=\EOS`, `kf5=\E[15~ … kf12=\E[24~` — the modern form.

Gaps in the `~` numbering (16, 22) are historical; never emit them, always accept-and-ignore.

### 2.3 Modified keys — the xterm parameter table

Modifiers are appended as a parameter **before the final byte**. `SS3` becomes `CSI` when a modifier
parameter is present.

```
CSI 1 ; <m> A      (modified Up)
CSI 15 ; <m> ~     (modified F5)
CSI 1 ; <m> P      (modified F1)     ← but F3 uses CSI 13 ; <m> ~
```

| `<m>` | Modifiers |
|---|---|
| 2 | Shift |
| 3 | Alt |
| 4 | Shift+Alt |
| 5 | Control |
| 6 | Shift+Control |
| 7 | Alt+Control |
| 8 | Shift+Alt+Control |
| 9 | Meta |
| 10 | Meta+Shift |
| 11 | Meta+Alt |
| 12 | Meta+Alt+Shift |
| 13 | Meta+Ctrl |
| 14 | Meta+Ctrl+Shift |
| 15 | Meta+Ctrl+Alt |
| 16 | Meta+Ctrl+Alt+Shift |

Same "+1" convention: `m - 1` is the bitmask `shift=1, alt=2, ctrl=4, meta=8`.
Verified against Ghostty's `CsiUMods.seqInt()` unit tests: `{}`→1, `{shift}`→2, `{alt}`→3,
`{ctrl}`→5, `{alt,shift}`→4, `{ctrl,shift}`→6, `{alt,ctrl}`→7, `{alt,ctrl,shift}`→8.

Ghostty terminfo confirms the shape at runtime: `kUP=\E[1;2A`, `kLFT5=\E[1;5D`, `kNXT7=\E[6;7~`,
`kf25=\E[1;5P` (ctrl+F1), `kf13=\E[1;2P` (shift+F1), `kf27=\E[1;5R` (ctrl+F3 — the `R` collision again),
`kDC=\E[3;2~`.

### 2.4 Backspace vs Delete — the classic trap

| Key | Byte |
|---|---|
| **Backspace** (the big key above Return) | `0x7F` (DEL) |
| **Delete** (forward delete) | `CSI 3 ~` = `1B 5B 33 7E` |
| `ctrl+backspace` | `0x08` (BS) |
| `alt+backspace` | `1B 7F` |
| `ctrl+alt+backspace` | `1B 08` |

Ghostty ships `kbs=\177` (0x7F) and `kdch1=\E[3~`. **`0x08` is Ctrl+H / ctrl+backspace, not Backspace.**
DECBKM (`CSI ? 67 h`) swaps `0x7F`↔`0x08`; Ghostty has a `backarrow_key_mode` option honouring it.

### 2.5 Special keys with modifiers (kitty spec's legacy table — verified)

| Key | none | Ctrl | Alt | Shift | Ctrl+Shift | Alt+Shift | Ctrl+Alt |
|---|---|---|---|---|---|---|---|
| Enter | `0x0D` | `0x0D` | `1B 0D` | `0x0D` | `0x0D` | `1B 0D` | `1B 0D` |
| Escape | `0x1B` | `0x1B` | `1B 1B` | `0x1B` | `0x1B` | `1B 1B` | `1B 1B` |
| Backspace | `0x7F` | `0x08` | `1B 7F` | `0x7F` | `0x08` | `1B 7F` | `1B 08` |
| Tab | `0x09` | `0x09` | `1B 09` | `CSI Z` | `CSI Z` | `1B CSI Z` | `1B 09` |
| Space | `0x20` | `0x00` | `1B 20` | `0x20` | `0x00` | `1B 20` | `1B 00` |

`CSI Z` = `1B 5B 5A` = back-tab (shift+tab). Ghostty terminfo: `kcbt=\E[Z`, `cbt=\E[Z`.

**Legacy cannot express** shift+Enter, ctrl+Enter, shift+Escape, ctrl+Tab, or *any* key release.
This is exactly why Terminal-Fenster must prefer the kitty protocol.

### 2.6 Ctrl+letter → C0

Full table (kitty spec, matches VT100):

```
SPC→0   @→0   2→0
a..z → 1..26   (ctrl+a=0x01 … ctrl+z=0x1A)
[ → 27 (0x1B)  3 → 27
\ → 28 (0x1C)  4 → 28
] → 29 (0x1D)  5 → 29
^ → 30 (0x1E)  6 → 30   ~ → 30
_ → 31 (0x1F)  7 → 31   / → 31
8 → 127 (0x7F) ? → 127
1 → 49 ('1')   9 → 57 ('9')   0 → 48 ('0')     ← i.e. ctrl is dropped
```

**Irrecoverable collisions in legacy mode:** `ctrl+i` ≡ `Tab` (0x09), `ctrl+m` ≡ `Enter` (0x0D),
`ctrl+j` ≡ LineFeed (0x0A), `ctrl+[` ≡ `Escape` (0x1B), `ctrl+h` ≡ ctrl+backspace (0x08),
`ctrl+@`/`ctrl+space`/`ctrl+2` all ≡ NUL. Do not try to be clever: in legacy mode emit
`KeyboardEvent{key:"Tab"}` for 0x09 and accept the loss. Kitty mode resolves all of these.

### 2.7 Alt/Meta: ESC-prefix vs 8th-bit

xterm supports two encodings, controlled by `metaSendsEscape` / `altSendsEscape`:
1. **ESC prefix** — `ESC x` (2 bytes). This is the modern default everywhere in scope.
2. **8th bit set** — `x | 0x80` (1 byte, "meta mode"). Ghostty terminfo declares `km` (has meta key)
   but the shipped default is ESC-prefix. **Only decode 8-bit meta if you explicitly enabled it**
   (SMM/`CSI ? 1034 h`), otherwise `0x80..0xFF` is UTF-8 continuation data and you will corrupt text.

**macOS-specific:** Option is a *compose* modifier by default.
* Ghostty `macos-option-as-alt` default is **`false`** (`src/input/key_encode.zig`,
  `OptionAsAlt = .false`); values `left` / `right` / `true`. With `false`, `alt+w` produces `∑` (U+2211)
  and the kitty encoding is `\x1b[119;;8721u` — **no alt bit in the modifier field**. With `true`,
  the same keystroke is `\x1b[119;3u`.
* Apple Terminal: "Use Option as Meta key" is off by default → Option composes; no ESC prefix.

**Consequence for Terminal-Fenster:** you cannot reliably observe the Alt/Option modifier on macOS unless the
user opts in at the terminal. Do not bind anything critical to Alt. Surface a first-run hint telling the
user to set `macos-option-as-alt = true` if they want Alt shortcuts.

### 2.8 The ESC ambiguity and the timeout heuristic

A lone `0x1B` is ambiguous between (a) the Escape key, (b) the start of a CSI/SS3/OSC sequence, and
(c) an Alt prefix. The only legacy resolution is a timer:

```rust
// Legacy mode only. Never apply this when kitty flags are active.
const ESC_TIMEOUT: Duration = Duration::from_millis(25); // local pty; see note

enum EscState { Idle, SawEsc { at: Instant } }

// On read():
//   - buffer ends with a bare 0x1B and no further bytes  -> arm timer, DO NOT emit
//   - more bytes arrive before deadline                  -> parse as sequence / Alt+key
//   - deadline expires                                   -> emit Escape keydown
```

Tuning: 25–50 ms is right for a local pty (Ghostty/iTerm2/Terminal all deliver a full sequence in one
`read()` in practice). Over SSH or mosh, sequences fragment; 100 ms is the common ceiling (neovim's
`ttimeoutlen` default is 50 ms). **Make it configurable and default to 50 ms.**
Better: disambiguate structurally — if the byte after `ESC` is `[`, `O`, `P`, `]`, `_`, `^`, or `\`, it
is a sequence, never Alt. Only `ESC` + printable-ASCII-that-isn't-`[`/`O`/`P`/`]` is Alt+key.

**Once the kitty protocol is on with bit 1 (disambiguate), delete the timer entirely** — Escape becomes
`CSI 27 u` and Alt becomes an explicit modifier bit. This alone justifies §1.

### 2.9 modifyOtherKeys (xterm middle ground)

`CSI > 4 ; <Pv> m` sets modifyOtherKeys; `CSI > 4 m` (no `Pv`) resets it. Query: `CSI ? 4 m` →
reply formatted as `CSI > 4 ; <Pv> m`.

* `Pv = 1`: alt/meta-modified ordinary keys become function-key-shaped. `alt+Tab` → `CSI 27 ; 3 ; 9 ~`.
* `Pv = 2`: **all** modifiers apply. `shift+Tab` → `CSI 27 ; 2 ; 9 ~` (instead of `CSI Z`).
* `Pv = 3`: even unmodified keys. `space` → `CSI 27 ; 1 ; 32 ~`.

Format: `CSI 27 ; <modifier> ; <codepoint> ~`, modifier from the §2.3 table. Ghostty implements state 2
(`opts.modify_other_keys_state_2`). **Use this only as a second-tier fallback where kitty is absent but
xterm-compatible modifyOtherKeys exists.** Apple Terminal does not support it (no XTVERSION, no DECRQM,
no XTQMODKEYS reply observed).

---

## 3. Mouse

### 3.1 Mode constants (xterm `xcharmouse.h`, verbatim)

```c
#define SET_X10_MOUSE               9
#define SET_VT200_MOUSE             1000
#define SET_VT200_HIGHLIGHT_MOUSE   1001
#define SET_BTN_EVENT_MOUSE         1002
#define SET_ANY_EVENT_MOUSE         1003
#define SET_FOCUS_EVENT_MOUSE       1004
#define SET_ALTERNATE_SCROLL        1007
#define SET_EXT_MODE_MOUSE          1005
#define SET_SGR_EXT_MODE_MOUSE      1006
#define SET_URXVT_EXT_MODE_MOUSE    1015
#define SET_PIXEL_POSITION_MOUSE    1016
```

Two orthogonal axes: **protocol** (9 / 1000 / 1002 / 1003) and **encoding** (default X10 / 1005 / 1006 / 1015 / 1016).
Encoding modes are mutually exclusive: in Ghostty, setting 1016 sets `mouse_format = .sgr_pixels`;
**resetting 1016 sets `mouse_format = .x10`, not `.sgr`** (`src/termio/stream_handler.zig:758`).
So `CSI ?1016l` after `CSI ?1006h; CSI ?1016h` silently drops you to X10, not SGR. **Never DECRST an
encoding mode — re-set the one you want instead.**

### 3.2 Enable/disable — exact byte order

```
enable:   ESC [ ? 1003 h   ESC [ ? 1006 h   ESC [ ? 1016 h
          1B 5B 3F 31 30 30 33 68
          1B 5B 3F 31 30 30 36 68
          1B 5B 3F 31 30 31 36 68

disable:  ESC [ ? 1016 l   ESC [ ? 1006 l   ESC [ ? 1003 l
```

Order matters on the enable path: set the **protocol** (1003) first, then 1006, then attempt 1016.
Because 1016 is a *superset encoding* of 1006, if 1016 is unsupported you are still left in valid SGR
mode. This is the safe degradation path and is what Terminal-Fenster must do.

`1000` = press+release only. `1002` = adds motion **while a button is held** (drag). `1003` = **all
motion**, button or not. A browser needs `1003` (hover, `:hover`, `mousemove`, tooltips).

### 3.3 SGR (1006) report format

```
CSI < <Cb> ; <Cx> ; <Cy> M      press / motion   (final 'M' = 0x4D)
CSI < <Cb> ; <Cx> ; <Cy> m      release          (final 'm' = 0x6D)
```
`Cx`/`Cy` are **1-based cell column/row**. `Cb` has **no +32 offset** (unlike X10).
Ghostty (`src/input/mouse_encode.zig`):
```zig
.sgr => try writer.print("\x1B[<{d};{d};{d}{c}", .{
    button_code, cell.x + 1, cell.y + 1,
    @as(u8, if (event.action == .release) 'm' else 'M'),
}),
```

### 3.4 Button code `Cb` — VERIFIED against two independent implementations

Base button (low bits):
| Value | Button |
|---|---|
| 0 | left (MB1) |
| 1 | middle (MB2) |
| 2 | right (MB3) |
| 3 | release (legacy X10 only) / motion with no button held |

Additive flags:
| Add | Meaning |
|---|---|
| +4 | Shift |
| +8 | Meta/Alt |
| +16 | Control |
| +32 | Motion (drag or hover) |

Extended buttons (replace the base value, not additive with 0–2):
| Value | Button |
|---|---|
| 64 | wheel up (btn 4) |
| 65 | wheel down (btn 5) |
| 66 | wheel left / tilt (btn 6) |
| 67 | wheel right / tilt (btn 7) |
| 128 | button 8 (back) |
| 129 | button 9 (forward) |
| 130, 131 | buttons 10, 11 |

Ghostty `buttonCode()`: `.left=>0 .middle=>1 .right=>2 .four=>64 .five=>65 .six=>66 .seven=>67
.eight=>128 .nine=>129`; then `+4 shift, +8 alt, +16 ctrl, +32 motion`.
kitty `mouse.c`: `SHIFT_INDICATOR (1<<2)=4`, `ALT_INDICATOR (1<<3)=8`, `CONTROL_INDICATOR (1<<4)=16`,
`MOTION_INDICATOR (1<<5)=32`, `SCROLL_BUTTON_INDICATOR (1<<6)=64`, `EXTRA_BUTTON_INDICATOR (1<<7)=128`,
with `(button-4)|64` for 4–7 and `(button-8)|128` for 8–11. **Identical.**

Decoding recipe:
```rust
let motion = cb & 32 != 0;
let shift  = cb & 4  != 0;
let alt    = cb & 8  != 0;
let ctrl   = cb & 16 != 0;
let base   = cb & !(4 | 8 | 16 | 32);   // strip mods+motion
let button = match base {
    0 => Some(Button::Left), 1 => Some(Button::Middle), 2 => Some(Button::Right),
    3 => None,                                        // motion, no button
    64 => Some(Button::WheelUp),   65 => Some(Button::WheelDown),
    66 => Some(Button::WheelLeft), 67 => Some(Button::WheelRight),
    128 => Some(Button::Back),     129 => Some(Button::Forward),
    130 => Some(Button::Ext10),    131 => Some(Button::Ext11),
    _ => None,
};
```
Note `!(4|8|16|32)` = `!0x3C`. This preserves bits 6 and 7 so 64/65/128/129 survive. Past button 11 the
encoding is genuinely ambiguous (xterm says so explicitly) — drop those events.

**Wheel events have no release.** xterm: "Release events for the wheel buttons are not reported."

### 3.5 SGR-Pixels (1016) — the critical mode

xterm ctlseqs, verbatim: *"Use the same mouse response format as the 1006 control, but report position
in pixels rather than character cells."*

Same framing, `CSI < Cb ; Px ; Py M/m`. The differences that matter and are **not** in the spec text:

| Property | 1006 (cells) | 1016 (pixels) |
|---|---|---|
| Origin | 1-based (`cell + 1`) | **0-based — no +1 added** |
| Reference frame | grid | **top-left of the text area, padding excluded** |
| Clamping | clamped into the grid | **not clamped; may be negative or exceed the terminal** |
| Motion dedup | only when the cell changes | **every motion event is reported** |
| Rounding | n/a | `round()` to nearest integer |

Verified in both implementations:
```zig
// ghostty/src/input/mouse_encode.zig
.sgr_pixels => {
    const pixels = posToPixels(event.pos, opts.size);
    try writer.print("\x1B[<{d};{d};{d}{c}", .{ button_code, pixels.x, pixels.y,
        @as(u8, if (event.action == .release) 'm' else 'M') });
},
// posToPixels: terminal-space = surface.x - padding.left, then @round(). Not clamped.
// and the dedup guard: `if (event.action == .motion and opts.format != .sgr_pixels) { dedup }`
```
```c
/* kitty/mouse.c */
case SGR_PIXEL_PROTOCOL:
    x = (int)round(mpos->global_x);
    y = (int)round(mpos->global_y);
    /* fallthrough */
case SGR_PROTOCOL:
    return snprintf(buf, sizeof(buf), "<%d;%d;%d%s", cb, x, y, action == RELEASE ? "m" : "M");
/* mpos->global_x = mouse_x - g->left;  ← also padding-excluded, 0-based */
/* and: if (mouse_cell_changed || protocol == SGR_PIXEL_PROTOCOL) { report } */
```

**kitty-only extension — mouse LEAVE.** kitty defines `LEAVE_INDICATOR (1<<8) = 256`, emitted **only**
under 1016: `cb = LEAVE_INDICATOR | MOTION_INDICATOR` = **288**, i.e. `CSI <288;x;yM` when the pointer
leaves the window. Ghostty does **not** emit this. Treat `cb & 256` as `mouseleave` if present; do not
depend on it. (This makes `Cb` a value that can exceed 255 — size your parser field accordingly.)

**Support matrix — verified by DECRQM:**
* **iTerm2 3.6.9: `CSI ? 1016 $ p` → `CSI ? 1016 ; 4 $ y`. Value 4 = permanently reset. NOT SUPPORTED,
  and will never become supported at runtime.** (For contrast, 1006 returned `;2` = reset-but-supported.)
* **Apple Terminal 465: no DECRQM support at all; no 1016.**
* Ghostty: `mouse_format_sgr_pixels = 1016` present in `src/terminal/modes.zig:273` and fully wired in
  `mouse_encode.zig`. Runtime DECRQM confirmation **UNVERIFIED** (see §7).
* Windows Terminal: open issue microsoft/terminal#18591 — treat as unsupported.

### 3.6 REQUIRED fallback: sub-cell coordinate synthesis

Because 1016 is unavailable on 2 of your 3 target terminals, Terminal-Fenster needs pixel coordinates
derived from cell coordinates. Two inputs are needed: cell size in pixels, and a sub-cell estimate.

**Getting cell pixel size, in order of preference:**

1. **`TIOCGWINSZ` ioctl** — `ws_xpixel` / `ws_ypixel`. Zero syscall cost, no escape-sequence round trip,
   and it updates on `SIGWINCH`.
   *Verified on Apple Terminal 465:* `rows=30 cols=120 ws_xpixel=840 ws_ypixel=450` → **7.0 × 15.0 per
   cell**. **These are logical points, not device pixels** — on this M4 Retina display the backing scale
   is 2×, so device cell size is 14 × 30. Multiply by the DPR you determine for graphics output.
   *iTerm2 3.6.9: UNVERIFIED (probe blocked, see §7) — test this first.*
2. **`CSI 16 t`** → `CSI 6 ; <height> ; <width> t`. *Verified: iTerm2 3.6.9 does NOT answer. Apple
   Terminal 465 does NOT answer.* Ghostty implements it. Low value on macOS.
3. **`CSI 14 t`** (window px) ÷ `CSI 18 t` (text area in cells). *Verified on Apple Terminal:*
   `CSI 14t` → `CSI 4;467;860t` (h=467, w=860) and `CSI 18t` → `CSI 8;30;120t` (30 rows, 120 cols).
   860/120 = 7.17, 467/30 = 15.57 — note these **disagree with `TIOCGWINSZ`'s 7.0/15.0** because
   `CSI 14t` includes window chrome/insets. **Prefer `TIOCGWINSZ`.** iTerm2 answers neither.
4. Kitty-graphics probe: place a known-size image and read back the reported cell footprint. Expensive;
   last resort. (Coordinate with A0x graphics mission — the same cell metric serves both.)

**Sub-cell estimate.** With cell-only reports you have quantisation error of one cell (7×15 pt =
14×30 device px here — enormous for a browser). Mitigations, in order:

* **Report the cell center**, not the corner: `px = (col - 1) * cell_w + cell_w/2`. Halves worst-case
  error versus using the corner, and makes hit-testing on ordinary-sized DOM targets work.
* **Velocity-based interpolation between motion events.** With 1003 enabled you get one event per cell
  crossing. Interpolate along the motion vector and time-stamp; for drag operations (text selection,
  canvas drawing) this is the difference between usable and unusable.
* **Zoom the page.** The honest engineering answer: on cell-only terminals, force a minimum effective
  CSS pixel ratio so that no interactive target is smaller than one cell. Expose it as
  `--min-hit-target-cells=1`.
* **Do not fake precision in the DOM.** Set `event.movementX/Y` from the cell delta, and expose a
  `Terminal-Fenster.pointerPrecision` value of `"pixel" | "cell"` so page-side shims can adapt.

### 3.7 Runtime detection of 1016

```
CSI ? 1016 h        enable (optimistically)
CSI ? 1016 $ p      DECRQM query
CSI c               DA1 fence
```
Read until the DA1 reply. Interpret the DECRQM `Ps` value:

| `Ps` | Meaning | Action |
|---|---|---|
| 0 | not recognized | 1016 unsupported → cell fallback |
| 1 | set | **pixel mode active** |
| 2 | reset | recognized but off — retry the `h`, then treat as unsupported |
| 3 | permanently set | pixel mode active |
| 4 | permanently reset | **unsupported forever** (iTerm2's answer) |
| *(no reply)* | terminal has no DECRQM | cell fallback |

Do not rely on a timeout to detect "no reply" — the DA1 fence is deterministic and free.

---

## 4. Focus events (mode 1004)

```
enable:   ESC [ ? 1004 h    (1B 5B 3F 31 30 30 34 68)
disable:  ESC [ ? 1004 l
focus in:  CSI I            (1B 5B 49)
focus out: CSI O            (1B 5B 4F)
```
xterm ctlseqs, verbatim: *"FocusIn/FocusOut can be combined with any of the mouse events since it uses
a different protocol. When set, it causes xterm to send `CSI I` when the terminal gains focus, and
`CSI O` when it loses focus."*

Ghostty terminfo declares `kxIN=\E[I` and `kxOUT=\E[O`.
**iTerm2 3.6.9: VERIFIED supported** (`CSI ?1004 $p` → `CSI ?1004;2$y`).
Apple Terminal 465: no DECRQM; support UNVERIFIED — assume absent and drive `document.hasFocus()` from
your own signal.

Parser trap: `CSI O` is one byte from `SS3` (`ESC O`). They differ only in the second byte
(`0x5B` vs `0x4F`) — a state machine that folds CSI and SS3 into one state will mis-fire. Keep them
distinct. `CSI I` also collides with nothing, but note that a naive "CSI + single uppercase letter"
handler will confuse `CSI I` with a cursor-forward-tab.

---

## 5. Bracketed paste (mode 2004)

```
enable:   ESC [ ? 2004 h    (1B 5B 3F 32 30 30 34 68)
disable:  ESC [ ? 2004 l
paste:    ESC [ 200 ~  <payload bytes>  ESC [ 201 ~
          1B 5B 32 30 30 7E   …   1B 5B 32 30 31 7E
```
Ghostty terminfo: `BE=\E[?2004h`, `BD=\E[?2004l`, `PS=\E[200~`, `PE=\E[201~`.
**iTerm2 3.6.9: VERIFIED supported** (`CSI ?2004 $p` → `CSI ?2004;2$y`).

Implementation rules for Terminal-Fenster:
* The payload is **arbitrary bytes**, including `ESC`. **Suspend all escape-sequence parsing between
  `CSI 200~` and `CSI 201~`** and scan only for the literal terminator. This is a security boundary:
  a paste containing `\x1b]52;c;...\x07` must never be executed.
* Terminals filter differently. kitty and Ghostty strip the `CSI 201~` terminator from the payload;
  none guarantee stripping of `\r`/`\n`. Normalise `\r\n`→`\n` and lone `\r`→`\n` yourself before
  delivering to the page as a `paste` ClipboardEvent.
* Cap payload size and stream it. A 50 MB paste arriving as one blocking read will stall your event
  loop; Ghostty has `src/input/paste.zig` for exactly this class of problem.
* Deliver as a DOM `paste` event with `clipboardData.setData("text/plain", …)`. There is **no way** to
  receive `text/html` or images through bracketed paste — the channel is bytes only.

---

## 6. Clipboard — OSC 52

### 6.1 Syntax (xterm ctlseqs, `Ps = 52`, verbatim excerpts)

```
write:  ESC ] 52 ; <Pc> ; <base64> BEL      1B 5D 35 32 3B 63 3B … 07
        ESC ] 52 ; <Pc> ; <base64> ESC \    (ST form; prefer this)
read:   ESC ] 52 ; <Pc> ; ?     BEL         1B 5D 35 32 3B 63 3B 3F 07
clear:  ESC ] 52 ; <Pc> ; <neither base64 nor '?'>
```

`Pc` is zero or more of `c p q s 0 1 2 3 4 5 6 7` — clipboard, primary, secondary, select, cut-buffers
0–7, **in the order given**. Empty `Pc` defaults to `s0`. Use **`c`** (system clipboard) on macOS.

Spec text: *"If the second parameter is a `?`, xterm replies to the host with the selection data encoded
using the same protocol. It uses the first selection found by asking successively for each item from the
list of selection parameters."* And: *"These controls may be disabled using the allowWindowOps resource."*

Ghostty's reply construction (`src/Surface.zig:5983`) is exactly:
```
ESC ] 52 ; <kind> ; <base64-standard-with-padding> ESC \
```
with `kind` ∈ `{'c' standard, 's' selection, 'p' primary}`. Note it uses **`ESC \` (ST), not BEL**, and
it **replies even when the clipboard is empty** (`ESC ] 52 ; c ; ESC \`). Your parser must accept a
zero-length base64 body. Base64 is RFC-4648 standard alphabet **with** `=` padding.

Ghostty terminfo advertises `Ms=\E]52;%p1%s;%p2%s\007` — the **BEL** terminator on the request side.
Accept both terminators on both sides.

### 6.2 Read policy — the part that will bite you

| Terminal | Write | Read (`?`) |
|---|---|---|
| **Ghostty 1.3.1** | `clipboard-write = allow` (default) | **`clipboard-read = ask` (default)** → modal permission dialog |
| **iTerm2 3.6.9** | permitted (pref-gated) | **denied by default — VERIFIED: no reply at all** |
| **Apple Terminal 465** | **not supported — VERIFIED: no reply, no effect** | not supported |
| kitty (reference) | `write-clipboard write-primary` | `read-clipboard-ask` — prompts |

Ghostty `src/config/Config.zig:2419-2420`:
```zig
@"clipboard-read": ClipboardAccess = .ask,
@"clipboard-write": ClipboardAccess = .allow,
```
kitty `kitty/options/definition.py:2957`, default:
`'write-clipboard write-primary read-clipboard-ask read-primary-ask'`, with the note that disabling the
read confirmation *"is a security risk as it means that any program, even the ones running on a remote
server via SSH can read your clipboard."*

**Operational hazard, learned the hard way during this research:** issuing `OSC 52 ; c ; ?` under
Ghostty raises a **modal confirmation dialog that blocks the entire application**, including its
AppleScript interface and new-window creation. An automated harness that fires a clipboard read will
appear to hang. **Never issue an OSC 52 read speculatively, never at startup, and never on a timer.**

**Recommended Terminal-Fenster clipboard design:**
* `navigator.clipboard.writeText()` → OSC 52 write with `Pc = c`. Works on Ghostty and iTerm2.
  Fail closed on Apple Terminal and surface a `NotAllowedError` to the page.
* `navigator.clipboard.readText()` → **do not** map to OSC 52 read. Map it to a
  **synthetic paste**: prompt the user to press ⌘V, receive the data via bracketed paste (§5), resolve
  the promise. This is both more portable and closer to the browser's own user-gesture requirement.
  Offer OSC 52 read only behind an explicit opt-in flag, with a hard 2-second deadline and a DA1 fence.
* Respect size limits: kitty's `clipboard_max_size` defaults to 512 MiB but many terminals and
  multiplexers cap OSC payloads far lower (tmux historically ~74 KB per chunk). Chunk writes and
  degrade gracefully.

---

## 7. macOS IME / composition

**There is no terminal protocol for preedit. None. Not in kitty's protocol, not in xterm's.** This is a
hard architectural constraint, not an implementation gap.

What actually happens on macOS:
* The terminal is an `NSView` conforming to `NSTextInputClient`. The IME (Japanese Kotoeri, Korean
  2-Set, Pinyin, dead-key accents on ABC-Extended) owns the composition. `setMarkedText:` gives the
  terminal an **uncommitted preedit string**; `insertText:` gives it the **committed** result.
* The terminal renders the preedit **itself**, inline, overlaid on the grid at the cursor. Ghostty does
  exactly this: `Surface.zig` has `renderer_state.preedit`, `preeditCallback` ("Called to set the
  preedit state for character input… should be called with null to reset"), and computes
  `preedit_width * cell.width` to place the IME candidate window via
  `firstRectForCharacterRange`-equivalent logic.
* **Only the committed text reaches the pty**, as UTF-8. The application sees a burst of printable
  bytes with no warning that composition occurred.

Consequences for Terminal-Fenster:
1. **You cannot implement `compositionstart` / `compositionupdate` / `compositionend` with real preedit
   text.** The best you can do is synthesise `compositionstart` + `compositionend` around a committed
   burst — which breaks any page that renders its own IME-aware editor.
2. **The IME candidate window will be positioned at the terminal's cursor**, which has nothing to do
   with where the focused `<input>` is on your rendered page. You must keep the terminal's *text*
   cursor parked at the pixel location of the focused DOM element. Emit `CSI <row> ; <col> H` to move
   the reported cursor to the caret cell on every focus/caret change. This is the only lever you have,
   and it is essential for CJK users.
3. **Kitty-protocol interaction:** with flag 16 (report associated text), committed IME text arrives as
   `CSI 0 ; ; <cp>[:<cp>…] u` — key code **`0`** means "no known key is associated with this text".
   Route `key == 0` straight to text input, never to a `KeyboardEvent.code`.
4. **Escape/Backspace during composition are swallowed by the IME**, and terminals know it. Ghostty
   guards this explicitly in `key_encode.zig::legacy()`:
   > *"If we have UTF-8 text, then we never emit PC style function keys… Japanese: escape clears the
   > dead key state; Korean: escape commits the dead key state; Korean: backspace should delete a
   > single preedit char"* — and `if (event.composing) return;` suppresses output entirely.
   So during composition you will observe **nothing** for Escape/Backspace. Do not treat the silence as
   a dropped event or attempt to resynchronise.
5. Dead keys (Option+E then E → `é` on ABC-Extended) behave identically: no preedit visible to you,
   one committed codepoint.

**Recommendation:** ship `pointerPrecision`-style capability reporting for IME too —
`Terminal-Fenster.imeSupport = "committed-only"` — and make the DOM shim synthesise a single
`compositionstart`/`compositionend` pair around bursts that arrive with `key == 0`. Document the
limitation rather than pretending to support it.

---

## 8. Reference: unified enable/disable preamble

```
# ---- enter Terminal-Fenster input mode ----
ESC [ ? 1049 h              alt screen           1B 5B 3F 31 30 34 39 68
ESC [ ? 2004 h              bracketed paste      1B 5B 3F 32 30 30 34 68
ESC [ ? 1004 h              focus events         1B 5B 3F 31 30 30 34 68
ESC [ ? 1003 h              any-motion mouse     1B 5B 3F 31 30 30 33 68
ESC [ ? 1006 h              SGR encoding         1B 5B 3F 31 30 30 36 68
ESC [ ? 1016 h              SGR-Pixels (try)     1B 5B 3F 31 30 31 36 68
ESC [ > 31 u                kitty kbd, push      1B 5B 3E 33 31 75
ESC [ ? 1016 $ p            DECRQM probe         1B 5B 3F 31 30 31 36 24 70
ESC [ ? u                   kitty kbd probe      1B 5B 3F 75
ESC [ c                     DA1 fence            1B 5B 63

# ---- leave (exact reverse, minus the probes) ----
ESC [ < 1 u                 kitty kbd, pop       1B 5B 3C 31 75
ESC [ ? 1006 h              restore SGR (NOT 1016l — see §3.1)
ESC [ ? 1003 l
ESC [ ? 1004 l
ESC [ ? 2004 l
ESC [ ? 1049 l
```

Install an `atexit` + `SIGINT`/`SIGTERM`/`SIGSEGV` handler that writes the leave block. A crashed
Terminal-Fenster that leaves 1003+1016 on renders the user's shell unusable — every mouse move floods stdin.

---

## 9. Verified empirical data (raw)

Method: Python probe with `tty.setraw()`, per-query drain, 350 ms adaptive read window, driven into each
terminal via AppleScript. Full script and logs in
`/private/tmp/claude-501/-Users-builder/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`
(`probe.py`, `term-out.txt`, `iterm-out.txt`, `wsz.py`).

### iTerm2 3.6.9 — 2026-07-31
```
TERM=xterm-256color  TERM_PROGRAM=iTerm.app  VER=3.6.9
CSI c        -> 1b5b3f36343b313b323b343b363b31373b31383b32313b32323b353263   ESC[?64;1;2;4;6;17;18;21;22;52c
CSI > c      -> 1b5b3e36343b323530303b3063                                   ESC[>64;2500;0c
CSI > 0 q    -> 1b503e7c695465726d3220332e362e391b5c                         DCS >|iTerm2 3.6.9 ST
CSI ? u      -> 1b5b3f3075                                                   ESC[?0u      << KITTY KBD SUPPORTED
CSI >31u;?u  -> 1b5b3f333175                                                 ESC[?31u     << push works, all 5 flags accepted
CSI <1u;?u   -> 1b5b3f3075                                                   ESC[?0u      << pop works
DECRQM ?9    -> ESC[?9;4$y      (4 = permanently reset)
DECRQM ?1000 -> ESC[?1000;2$y   DECRQM ?1002 -> ESC[?1002;2$y
DECRQM ?1003 -> ESC[?1003;2$y   DECRQM ?1004 -> ESC[?1004;2$y
DECRQM ?1005 -> ESC[?1005;2$y   DECRQM ?1006 -> ESC[?1006;2$y
DECRQM ?1015 -> ESC[?1015;2$y
DECRQM ?1016 -> ESC[?1016;4$y   << 4 = PERMANENTLY RESET. NO PIXEL MOUSE.
DECRQM ?2004 -> ESC[?2004;2$y   DECRQM ?2026 -> ESC[?2026;2$y
DECRQM ?2027 -> ESC[?2027;4$y   DECRQM ?1049 -> ESC[?1049;2$y
DECRQM ?7    -> ESC[?7;1$y      (1 = set)
OSC 52 write -> (no reply, expected)
OSC 52 read  -> (NO REPLY — blocked by default)
XTGETTCAP    -> (no reply)
CSI 16 t     -> (no reply)   CSI 14 t -> (no reply)   CSI 18 t -> (no reply)
```

### Apple Terminal 465 — 2026-07-31
```
TERM=xterm-256color  TERM_PROGRAM=Apple_Terminal  VER=465
CSI c        -> 1b5b3f313b3263            ESC[?1;2c        (VT100 + AVO)
CSI > c      -> 1b5b3e313b39353b3063      ESC[>1;95;0c     (VT220, firmware 95)
CSI > 0 q    -> (none)                    << NO XTVERSION
CSI ? u      -> (none)                    << NO KITTY KEYBOARD
DECRQM ?9/1000/1002/1003/1004/1005/1006/1015/1016/2004/2026/2027/1049/7 -> (ALL none)
                                          << NO DECRQM AT ALL. Cannot probe modes.
CSI >31u;?u  -> (none)     CSI <1u;?u -> (none)
OSC 52 write -> (none, and no clipboard effect)
OSC 52 read  -> (none)     XTGETTCAP -> (none)
CSI 16 t     -> (none)                    << NO CELL PIXEL SIZE
CSI 14 t     -> 1b5b343b3436373b38363074  ESC[4;467;860t   (h=467 w=860 pt)
CSI 18 t     -> 1b5b383b33303b31323074    ESC[8;30;120t    (30 rows, 120 cols)
TIOCGWINSZ   -> rows=30 cols=120 ws_xpixel=840 ws_ypixel=450  -> 7.0 x 15.0 pt/cell
```

### Ghostty 1.3.1 — runtime probe **BLOCKED**
`ghostty -e …`, `open -na Ghostty --args -e …`, `open -na --args --command=…`,
`launchctl asuser <uid> ghostty -e …`, and AppleScript (`new window with configuration` /
`input text to terminal 1`) **all failed to spawn a surface** from this agent's non-GUI session; the
`ghostty` processes persisted without creating a window. A contributing factor: the first probe's
`OSC 52 ; c ; ?` triggered Ghostty's `clipboard-read = ask` modal, which wedged the app (§6.2).

**All Ghostty facts in this document are therefore SOURCE-verified, not runtime-verified**, from:
* the shipped terminfo at `/Applications/Ghostty.app/Contents/Resources/terminfo/78/xterm-ghostty`
  (this is genuinely from the installed 1.3.1 build), and
* a shallow clone of `ghostty-org/ghostty` **`main`** (not the `v1.3.1` tag) at
  `…/scratchpad/ghostty-src`. Files cited: `src/terminal/modes.zig`, `src/terminal/kitty/key.zig`,
  `src/input/mouse_encode.zig`, `src/input/key_encode.zig`, `src/input/function_keys.zig`,
  `src/renderer/size.zig`, `src/termio/stream_handler.zig`, `src/config/Config.zig`, `src/Surface.zig`.

**ACTION REQUIRED before shipping:** re-run `probe.py` inside a hand-opened Ghostty window and confirm
`CSI ?1016$p → CSI ?1016;1$y` (or `;2`), `CSI ?u → CSI ?0u`, and `TIOCGWINSZ` pixel fields. Approve the
clipboard prompt when it appears. Also run it in iTerm2 to capture `TIOCGWINSZ`, which is the only
remaining unknown blocking the cell-size fallback there.

Terminfo caps read directly from the installed 1.3.1 bundle (authoritative for that build):
```
fullkbd, km, XT, Su, Tc, AX
kmous=\E[<        xm=\E[<%i%p3%d;%p1%d;%p2%d;%?%p4%tM%em%;
XM=\E[?1006;1000%?%p1%{1}%=%th%el%;      << note: advertises 1006, NOT 1016
Ms=\E]52;%p1%s;%p2%s\007
BE=\E[?2004h  BD=\E[?2004l  PS=\E[200~  PE=\E[201~
kxIN=\E[I  kxOUT=\E[O
kbs=\177  kdch1=\E[3~  kcbt=\E[Z
kcuu1=\EOA kcud1=\EOB kcuf1=\EOC kcub1=\EOD khome=\EOH kend=\EOF
kf1=\EOP kf2=\EOQ kf3=\EOR kf4=\EOS kf5=\E[15~ … kf12=\E[24~
kUP=\E[1;2A  kLFT5=\E[1;5D  kNXT7=\E[6;7~  kDC=\E[3;2~
u6=\E[%i%d;%dR  u7=\E[6n  u8=\E[?%[;0123456789]c  u9=\E[c
```

---

## 10. Licenses of referenced material

| Source | License | Use permitted |
|---|---|---|
| kitty keyboard protocol **spec** (`docs/keyboard-protocol.rst`) | GPL-3.0 (repo) — but a **published interoperability spec** | Implement from the spec. **Do not copy prose verbatim into product docs.** |
| kitty **source** (`kitty/mouse.c`, `kitty/options/definition.py`) | **GPL-3.0** | **Read for behavior only. Do not copy any code into Terminal-Fenster.** |
| Ghostty source (`ghostty-org/ghostty`) | **MIT** | Safe to adapt with attribution + license notice. |
| xterm `ctlseqs` (Thomas Dickey / invisible-island.net) | MIT-style (xterm license) | Safe to reference and adapt. |
| ncurses terminfo databases | MIT-style (X11) | Safe. |
| iTerm2 (`gnachman/iTerm2`) | **GPL-2.0** | Behavior observation only. **No code.** |

Everything in this document was derived from published specs, public documentation, MIT-licensed source,
or black-box runtime observation of installed binaries. **No GPL code has been or should be copied.**

---

## 11. Open items / UNVERIFIED

1. **Ghostty 1.3.1 runtime confirmation of 1016, kitty-kbd flags, `TIOCGWINSZ`, `CSI 16 t`** — §7.
2. **iTerm2 3.6.9 `TIOCGWINSZ` pixel fields** — probe blocked. This is the *only* remaining path to a
   cell-pixel metric on iTerm2 (both `CSI 16 t` and `CSI 14 t` are confirmed dead). **Highest-priority
   unknown.**
3. **Which of the five kitty flags iTerm2 actually honours.** It echoes back `31` after a push, but
   echoing is not implementing. Specifically unverified: flag 8 (report all keys) and flag 16
   (associated text). Test by pressing a plain `a` with flags=31 and checking for `CSI 97;;97u` vs a
   bare `0x61`. There is a known notcurses issue (dankamongmen/notcurses#2818) alleging iTerm2 sends
   non-conforming codes for F1–F4 under the kitty protocol — **verify F1–F4 explicitly.**
4. **xterm's own 1016 origin convention.** kitty and Ghostty both agree on 0-based/padding-excluded, but
   xterm's `ctlseqs` text does not state it and xterm source was not read. If you ever target real
   xterm, verify before trusting the origin.
5. **Apple Terminal focus (1004) and bracketed paste (2004) support** — cannot be probed (no DECRQM).
   Determine behaviourally: enable 1004 and Cmd-Tab away/back; enable 2004 and paste.
6. **Terminal multiplexer passthrough.** tmux and screen rewrite or drop 1016, kitty-keyboard, and
   OSC 52. Untested. `tmux` needs `allow-passthrough` and `terminal-features` configured. If Terminal-Fenster
   is expected to run under tmux this is a whole separate mission.
7. **`ws_xpixel` units and DPR.** Apple Terminal reports logical points (7×15) while the M4 display is
   2× — confirm the scale factor at runtime (via `CSI 14 t` ÷ `TIOCGWINSZ`, or by coordinating with the
   graphics mission's known image dimensions) rather than assuming 2.
