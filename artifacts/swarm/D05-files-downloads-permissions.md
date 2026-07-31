# D05 — Files, Downloads, Notifications and Permission Prompts as Terminal UI

**Mission:** D05. **Owner file:** `artifacts/swarm/D05-files-downloads-permissions.md` (this document only).
**Host:** macOS 26.1, Apple M4, arm64. Electron 43.2.0 / Chromium 150. **Date:** 2026-07-31.

**Status of evidence.** Unlike most of the swarm, this mission was able to *run Chromium*. Four
Electron probes were executed against the workspace's own `node_modules/electron` binary with the
agent Bash sandbox disabled. Every behavioural claim below about Electron, CDP, or macOS is a
**measurement with a transcript**, not a reading of documentation. Probe sources live in the agent
scratchpad (§2.1) and write nothing into the repository. Claims about *our* code are cited to
`file:line` and were read directly. Anything I could not execute is marked **`[UNVERIFIED]`** in §14.

---

## 0. The decision, in one paragraph

**Every one of these five surfaces is a native modal in stock Electron, and a native modal in an
offscreen terminal browser is an invisible deadlock.** That is not a design opinion; it was
measured. A download whose `will-download` handler does not call `setSavePath()` **synchronously**
produced *no* `done` event for 20 seconds and then resolved `cancelled` with an empty save path
(§3.1, probe D) — Electron had raised an `NSSavePanel` that nobody could see or dismiss. So the
architecture is forced: **BlackGlass must pre-empt every native dialog before Chromium can raise
it, convert it into a protocol event, render it as terminal text over the resident page image, and
send the answer back.** Three different mechanisms are required because Electron exposes three
different levels of control — `session` events for downloads, `session` handlers for permissions,
and **CDP for the file chooser and JS dialogs, because Electron has no public API for either**.
All three funnel into one shared *prompt layer* in the Rust core (§9), which is worth building once
rather than five times.

**Single most actionable recommendation** (expanded in §13): **`apps/engine/src/main.js` today
registers no `will-download` listener and no permission handler at all.** Consequences, in
severity order: (1) any page that triggers a download — including a stray `Content-Disposition:
attachment` on a navigation — hangs the engine on an invisible save panel for as long as the
process lives, and the terminal shows a frozen page with no error; (2) `setPermissionRequestHandler`
being unset means Electron's default handler runs, which **grants** several permissions without
asking anyone. A three-line default-deny permission handler and a two-line `will-download` handler
that calls `setSavePath()` into a fixed directory are each a few minutes of work, need no protocol
change, no UI, and no new dependency, and they remove a hard hang and a silent grant. Ship those
first; the terminal UI in §§3.4/4.7/5.4 is the follow-on.

---

## 1. What is already true (grounding)

Read directly from the tree.

| Fact | Evidence |
|---|---|
| Engine registers **no** `will-download` listener | `apps/engine/src/main.js` — `session` is never imported; the require list is `{ app, BrowserWindow }` at `:17` |
| Engine registers **no** permission handler | same; no `setPermissionRequestHandler` / `setPermissionCheckHandler` anywhere in the 309-line file |
| Engine **does** already deny popups and report them | `apps/engine/src/main.js:129-132` — `setWindowOpenHandler` returns `{action:'deny'}` and emits `{t:'popup',url}` |
| Wire framing is `[u8 type][u32 BE len][payload]`; `1`=frame, `2`=event, `10`=command | `crates/bg-proto/src/lib.rs:11-13`, encoder at `:97-103` |
| Events are flat JSON with a `t` discriminator, parsed by hand | `crates/bg-proto/src/lib.rs:125-170`; `Status::apply_event` at `apps/cli/src/main.rs:783` |
| Exactly **one** bottom row is chrome; the page occupies the rest | `apps/cli/src/main.rs:254` — `page_h = vp_h - cell_h` |
| Status bar is drawn to the last row each present, reverse-video, from sanitized text | `apps/cli/src/main.rs:883-897` |
| Page-derived strings are already sanitized before touching the tty | `apps/cli/src/main.rs:887-888` via `unicode::sanitize_for_terminal(_, 40/60)` |
| Mouse events landing below the page are already discarded | `apps/cli/src/main.rs:609-611`, `PointerMap::to_page` at `:712-728`; test at `:1001` |
| Browser-level keys are intercepted before the page sees them (`ctrl+q`, `ctrl+r`, `alt+←/→`) | `apps/cli/src/main.rs:570-592` |
| Measured Ghostty geometry: 2482×851 px, cell 17×37 ⇒ **146 cols × 23 rows**, page 814 px = 22 rows | project brief; `2482/17 = 146`, `851/37 = 23`, `814/37 = 22` |
| Electron is MIT; workspace declares `MIT OR Apache-2.0`; **no `LICENSE` file exists at repo root** | `apps/engine/node_modules/electron/LICENSE`; `Cargo.toml:9`; `ls LICENSE*` → no matches |

Everything in this document is additive to that. **No core file is edited by this mission**; §10
and §13 describe the changes for the commander to make.

---

## 2. Probe methodology and raw results

### 2.1 What was run

Four Electron programs, each launched as:

```
cd /Users/adeebbashir/projects/blackglass/apps/engine
./node_modules/electron/dist/Electron.app/Contents/MacOS/Electron <probe>.js
```

with `dangerouslyDisableSandbox: true` (the project brief records that Chromium child processes
fail under the agent sandbox with `bootstrap_look_up ... Permission denied`; that reproduced, and
disabling the sandbox fixed it). Each probe sets `app.setPath('userData', $TMP/userData)` and
`--disable-gpu`, hides the dock, and serves its own fixtures from an ephemeral
`127.0.0.1` HTTP server so that downloads have a real `http://` origin — `data:` and `file:` URLs
travel a different path through Chromium's download stack and would not have been a fair test.

| Probe | File (scratchpad) | Question |
|---|---|---|
| A | `probe-d05.js` | first pass; surfaced the CDP stall |
| B | `probe-d05b.js` | per-command timeouts; permission strings; CDP-vs-`setSavePath` conflict |
| C | `probe-d05c.js` | honest download path; user-activation gate; quarantine |
| D | `probe-d05d.js` | `/private/tmp` control; the no-`setSavePath` deadlock |

Scratchpad root: `/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`.

### 2.2 The eight findings that drive the design

| # | Finding | Probe |
|---|---|---|
| **F1** | Omitting `setSavePath()` in `will-download` ⇒ **no `done` event for 20 s**, then `cancelled`, `savePath: ""`. Invisible `NSSavePanel`. | D |
| **F2** | Electron 43.2.0 does **not** set `com.apple.quarantine` on downloads. Verified in `/private/tmp` *and* in the real `~/Downloads`. | C, D |
| **F3** | The `Electron Framework` binary has **zero** undefined `qtn_*` symbols — it never links `libquarantine`. Corroborates F2 at the binary level. | `nm -u` |
| **F4** | `Page.enable` and `Page.setInterceptFileChooserDialog` **never settle** on a not-yet-navigated OSR target; they settle on first navigation. | B, C |
| **F5** | `Page.fileChooserOpened` fires only under **user activation**. A plain `executeJavaScript('el.click()')` produces nothing; `userGesture:true` and real `sendInputEvent` clicks both work. | C |
| **F6** | `Browser.setDownloadBehavior` **silently overrides** `item.setSavePath()`, and `item.getSavePath()` then returns a path to a file that does not exist. | B |
| **F7** | Chromium emits permission strings that are **not in Electron's TypeScript union**: `persistent-storage` (request), `web-app-installation` and `automatic-fullscreen` (check). | B |
| **F8** | `Page.javascriptDialogOpening` + `Page.handleJavaScriptDialog` fully control `alert`/`confirm`/`prompt`; `confirm()` returned `true` after a programmatic accept. | B |

---

## 3. Downloads

### 3.1 The two hard rules, measured

**Rule 1 — `setSavePath()` must be called synchronously inside the `will-download` listener, on
every single download, without exception.**

Probe D deliberately skipped it. Transcript:

```
PROBE:{"ms":597,  "k":"noSavePathCase","v":{"defaultSavePath":"","filename":"control.bin"}}
PROBE:{"ms":6599, "k":"NATIVE_DIALOG_APPEARS_TO_BLOCK","v":"no done event after 6s"}
PROBE:{"ms":20972,"k":"noSavePathDone","v":{"st":"cancelled","savePath":""}}
```

The `done` event at ms 20972 arrived only because the probe harness `SIGKILL`ed the process at
~21 s. Left alone this never completes. Electron's own type definition states the mechanism
plainly (`electron.d.ts:8294-8299`): if the save path is not set, *"Electron will use the original
routine to determine the save path; this usually prompts a save dialog."* In a windowless OSR app
on a machine at a lock screen, that dialog is unreachable. **This is a hang, not a degradation.**

**Rule 2 — do not combine `Browser.setDownloadBehavior` with `setSavePath()`.**

Probe B armed CDP download behaviour *and* set a save path. The result is the nastiest class of
bug — a successful-looking lie:

```
PROBE:{"k":"savePathAfterSet","v":".../d05tmp/dl/blackglass-probe.bin"}
PROBE:{"k":"downloadDone","v":{"state":"completed","savePath":".../dl/blackglass-probe.bin","exists":false}}
PROBE:{"k":"cdpEvent","v":{"method":"Browser.downloadProgress","params":{
        "filePath":".../d05tmp/cdpdl/7c10e3d3-4111-4ef0-b431-11b5f9d2b4c5","state":"completed"}}}
```

`state: "completed"`, `getSavePath()` returns the path we asked for, and **that file does not
exist**. The bytes went to `cdpdl/<GUID>` — no filename, no extension. Independently confirmed
after the probe exited:

```
$ ls -la .../d05tmp/dl        → empty
$ ls -la .../d05tmp/cdpdl     → -rw-r--r--  11  7c10e3d3-4111-4ef0-b431-11b5f9d2b4c5
$ cat .../cdpdl/7c10e...      → hello world
```

Since §4 requires CDP for the file chooser, and CDP is therefore attached anyway, this is a live
trap for whoever implements both. **BlackGlass must never call `Browser.setDownloadBehavior`.**
Download control belongs entirely to the `session` layer.

With CDP download behaviour *not* set (probe C), everything works as documented:

```
PROBE:{"k":"downloadDone","v":{"state":"completed","savePath":".../d05c/dl/report.pdf",
                               "exists":true,"size":11}}
```

### 3.2 What `will-download` actually gives you

Every accessor below was called on a live `DownloadItem` in probe C and returned the value shown.
This is the real payload available at decision time — **before a single byte is written**:

```
url               http://127.0.0.1:50442/dl.bin
filename          report.pdf                     ← from Content-Disposition, NOT the URL
mime              application/octet-stream
totalBytes        11                             ← 0 when the server omits Content-Length
contentDisposition attachment; filename="report.pdf"
etag              "abc123"
lastModified      Wed, 21 Oct 2026 07:28:00 GMT
hasUserGesture    false                          ← drive-by vs. user-initiated
urlChain          ["http://127.0.0.1:50442/dl.bin"]   ← full redirect chain
state             progressing
```

Two of these carry real security weight and should be surfaced in the UI, not merely logged:

- **`hasUserGesture: false`** is the signature of a drive-by download. Probe C's download was
  started by `webContents.downloadURL()`, which is exactly what a page-initiated navigation to an
  `attachment` response looks like from here. The prompt must say so.
- **`urlChain`** length > 1 means redirects. A download that starts at a domain you trust and ends
  somewhere else is worth showing; only the *last* hop actually served the bytes.

`filename` is fully attacker-controlled (it is the server's `Content-Disposition`) and must be
treated as hostile — see §11.1.

### 3.3 Proposed engine → core events

Additive; no change to framing. All are `T_EVENT` (type 2) JSON, matching the existing flat-object
convention that `crates/bg-proto/src/lib.rs:125` can parse.

```jsonc
// on 'will-download', BEFORE calling setSavePath()
{"t":"downloadAsk","id":"d1","filename":"report.pdf","mime":"application/octet-stream",
 "bytes":1048576,"url":"https://ex.co/report.pdf","host":"ex.co","gesture":false,"hops":1,
 "suggested":"/Users/me/Downloads/report.pdf"}

{"t":"downloadProgress","id":"d1","received":524288,"total":1048576,"bps":183500,"paused":false}
{"t":"downloadDone","id":"d1","state":"completed","path":"/Users/me/Downloads/report.pdf",
 "quarantined":true}
{"t":"downloadDone","id":"d1","state":"interrupted","canResume":true}
```

Core → engine commands (`T_COMMAND`, type 10), handled in the `switch` at
`apps/engine/src/main.js:227`:

```jsonc
{"t":"downloadAnswer","id":"d1","action":"save","path":"/Users/me/Downloads/report.pdf"}
{"t":"downloadAnswer","id":"d1","action":"cancel"}
{"t":"downloadControl","id":"d1","action":"pause"|"resume"|"cancel"}
```

**The one hard constraint on this design:** `setSavePath()` must be called *synchronously* inside
the listener, but the user's answer arrives *asynchronously* over a socket. These are irreconcilable.
The resolution is: **always call `setSavePath()` immediately to a private staging directory, then
move the file on `done` once the answer is known.**

```js
// SKETCH for apps/engine/src/main.js — commander's call, not written by this mission.
const staging = path.join(app.getPath('userData'), 'downloads-staging');
session.defaultSession.on('will-download', (event, item) => {
  const id = 'd' + (++dlSeq);
  fs.mkdirSync(staging, { recursive: true });
  item.setSavePath(path.join(staging, id + '.part'));   // SYNCHRONOUS — never omit (F1)
  pending.set(id, item);
  sendEvent({ t: 'downloadAsk', id, filename: item.getFilename(), /* ... */ });
  item.on('updated', (_e, st) => sendEvent({ t: 'downloadProgress', id, /* ... */ }));
  item.once('done', (_e, st) => finalize(id, st));      // move + quarantine here (§8)
});
```

If the user cancels while bytes are still arriving, call `item.cancel()` and unlink the `.part`
file. If the user cancels after `done`, unlink. Staging also means the answer prompt is never on
the critical path of the transfer — the download proceeds while the user reads the prompt, which
is the right behaviour for a 200 MB file on a slow link.

`item.pause()`, `item.resume()` and `item.canResume()` exist (`electron.d.ts:8271-8286`), but
Electron's own note is explicit that resume only preserves received bytes if the server sent both
`Last-Modified` and `ETag` and supports ranges — otherwise `resume()` silently restarts from zero.
Surface `canResume()` in the UI rather than offering a resume that quietly re-downloads 200 MB.

### 3.4 Terminal UI — the download prompt

Drawn as an overlay in the prompt layer (§9), centred, 72 columns, over the resident page image.

```
        ┌─ Download ──────────────────────────────────────────────────────┐
        │                                                                 │
        │   report.pdf                                          1.0 MB    │
        │   application/pdf                                               │
        │                                                                 │
        │   from  ex.co                                                   │
        │   to    ~/Downloads/report.pdf                                  │
        │                                                                 │
        │   ! not started by a click on this page                         │
        │                                                                 │
        │   [S] save     [A] save as…     [C] cancel                      │
        └─────────────────────────────────────────────────────────────────┘
```

The `!` line appears only when `gesture:false`. When `hops > 1` a second advisory line reads
`! redirected 3 times, last hop cdn.other.example`. Both are advisories, not blockers — the design
principle throughout §5 and §3 is *inform, default to the safe answer, never nag*.

Progress replaces the status bar's right-hand segment rather than opening a second overlay; a
download should not steal the page. The bar at `apps/cli/src/main.rs:890` gains one optional field:

```
 ... Example Domain  |  https://ex.co/  |  60fps 52KB 0.7ms  ↓report.pdf 47% 1.2MB/s  ctrl+q quit
```

With more than one active download the segment collapses to `↓3 files 61%`, and `ctrl+j` opens the
list:

```
        ┌─ Downloads ─────────────────────────────────────────────────────┐
        │                                                                 │
        │  ▸ report.pdf          47%  ████████▒▒▒▒▒▒▒▒  1.2 MB/s   0:41   │
        │    dataset.csv        100%  ████████████████  done              │
        │    video.mp4            —   interrupted · cannot resume         │
        │                                                                 │
        │   [p] pause   [r] resume   [x] cancel   [o] reveal   [q] close  │
        └─────────────────────────────────────────────────────────────────┘
```

`[o] reveal` runs `shell.showItemInFolder`, which is the one place a *native* affordance is
correct: the user has explicitly asked to leave the terminal.

**Status-bar width is a live constraint, not a theoretical one.** The existing bar is built with an
unclamped `format!` at `apps/cli/src/main.rs:890-895`; C06 §12 already flags that a long title plus
URL can exceed the measured 146 columns and wrap, which scrolls the screen and desynchronises the
kitty image placement from the cell grid. Adding a download segment makes that worse. **Whoever
adds the segment must also clamp the whole bar to `cols`**, or the download indicator becomes the
trigger for an existing latent rendering bug.

---

## 4. The file chooser

### 4.1 There is no Electron API for this

`session` has `setPermissionRequestHandler`, `setPermissionCheckHandler`,
`setDevicePermissionHandler`, `setDisplayMediaRequestHandler` and `setCertificateVerifyProc`
(`electron.d.ts:13202-13255`). It has **nothing** for `<input type="file">`. Chromium's
`FileSelectHelper` goes straight to the platform dialog. On macOS that is an `NSOpenPanel` with
exactly the same invisibility problem as F1.

The only lever is the Chrome DevTools Protocol, reachable from `webContents.debugger`
(`electron.d.ts:18528` → `class Debugger` at `:7476`, `sendCommand` at `:7606`). That is a public,
supported Electron API — this is not a hack, but it does come with the constraints in §4.3.

### 4.2 It works — measured

Probe C, with interception armed and a real synthesized mouse click through the exact input path
`apps/engine/src/main.js:178-193` already uses:

```
PROBE:{"ms":8223, "k":"armIntercept","v":{"ok":true,"r":{}}}
PROBE:{"ms":9027, "k":"FILE_CHOOSER_EVENT","v":{"backendNodeId":2,"frameId":"6862D8…","mode":"selectSingle"}}
PROBE:{"ms":9828, "k":"synthClickAt","v":{"x":641,"y":19}}
PROBE:{"ms":9830, "k":"FILE_CHOOSER_EVENT","v":{"backendNodeId":3,"frameId":"6862D8…","mode":"selectMultiple"}}
```

No native panel appeared, the process never blocked, and the events carry the distinction between
single and multiple selection. `Page.setInterceptFileChooserDialog` also accepted the newer
`{enabled, cancel}` signature (`{"ok":true}`), so Chromium 150 supports the cancel variant.

### 4.3 Ordering constraint — arm after navigation, not at window creation (F4)

This is the subtlest finding and it will cost someone an afternoon if it is not written down.

Probe C issued `Page.enable` on a freshly created OSR window still on `about:blank`:

```
PROBE:{"ms":5003,"k":"Page.enable issued (about:blank target)","v":true}
PROBE:{"ms":7505,"k":"Page.enable still pending after 2500ms?","v":true}
PROBE:{"ms":7610,"k":"Page.enable RESOLVED","v":7610}     ← navigation had just begun
PROBE:{"ms":7621,"k":"loadURL","v":{"ok":true}}
```

Probe B rules out a fixed timeout as the explanation: there, `Page.enable` was issued at t≈0 and
two subsequent `Page.setInterceptFileChooserDialog` calls issued at t≈5 s and t≈10 s **each still
timed out after 5 s** (i.e. were pending at t=10 s and t=15 s). Had a ~2.6 s timer been unblocking
the pipe, the call issued at 5 s would have settled by 7.6 s. It did not. **The unblocking event is
the first navigation, not elapsed time.**

Meanwhile `Browser.setDownloadBehavior` — a *browser*-domain command — returned `{"ok":true}`
immediately on the same un-navigated target. So the stall is specific to page-domain commands on an
OSR target that has never committed a navigation.

**Consequence for the engine:** do not attach and arm CDP inside `createWindow()`
(`apps/engine/src/main.js:101`). Arm on the first `did-finish-load`, and re-arm on every
subsequent one. Probe C also confirms re-arming after a commit is cheap and succeeds
(`armIntercept` at ms 8223 returned `{"ok":true}` immediately). Any `sendCommand` in this path must
carry a timeout in the engine, because a hung promise here is indistinguishable from a hung browser.

A corollary worth stating plainly: **`Schema.getDomains` is not a capability oracle.** Probe A got
back a list including `ApplicationCache` — removed from Chromium years ago — and *excluding*
`Browser`, which demonstrably works. Feature-detect by calling the command and catching, never by
reading the domain list.

### 4.4 The chooser is user-activation gated (F5)

```
PROBE:{"ms":9025,"k":"chooserAfterNoGestureClick","v":0}   ← executeJavaScript(..., userGesture=false)
PROBE:{"ms":9827,"k":"chooserAfterGestureClick","v":1}     ← executeJavaScript(..., userGesture=true)
PROBE:{"ms":11029,"k":"chooserAfterSynthClick","v":{"count":2, …}}  ← sendInputEvent mouseDown/Up
```

This is good news twice over. It means BlackGlass's real input path (`sendInputEvent`) *does*
create the activation needed to open a chooser, so the feature works for users. And it means a page
cannot spam file choosers without a click — the platform already enforces that. **Do not add a rate
limiter for choosers; Chromium's activation gate is the correct one and a second one would only
break legitimate flows.**

### 4.5 What the event does *not* tell you

```json
{"backendNodeId": 2, "frameId": "6862D8BD…", "mode": "selectSingle"}
```

That is the entire payload. Modes observed: `selectSingle`, `selectMultiple`. CDP also defines
`selectDirectory`, which the `webkitdirectory` input in the fixture would produce
**`[UNVERIFIED]`** — that input existed in the page but was not clicked in the run that logged
modes.

Critically absent: the input's `accept` attribute, its `name`, and any label. A picker that cannot
say *"choose an image"* is a worse picker. Resolve it yourself from the `backendNodeId`:

```js
const { object } = await dbg.sendCommand('DOM.resolveNode', { backendNodeId });
const { result } = await dbg.sendCommand('Runtime.callFunctionOn', {
  objectId: object.objectId,
  functionDeclaration: 'function(){return JSON.stringify({accept:this.accept,name:this.name,id:this.id})}',
  returnByValue: true,
});
```

Treat the returned `accept` as a **display hint and a default filter only**. It is page-controlled,
so it must never be the thing that decides what BlackGlass is allowed to read (§11.2).

### 4.6 Answering the chooser

`DOM.setFileInputFiles` works and the page genuinely observes the file. Probe B set a file and read
it back from page script:

```
PROBE:{"k":"cdp:DOM.setFileInputFiles","v":{"ok":true,"r":{}}}
PROBE:{"k":"readBackFiles","v":{"ok":true,"r":"{\"n\":1,\"name\":\"pick-me.txt\",\"size\":20,\"type\":\"text/plain\"}"}}
```

Probe C repeated it against the `multiple` input and got `{"n":1,"name":"pick-me.png"}`. Note
Chromium sniffed `type` correctly (`text/plain`) — the page sees a fully normal `File`.

`DOM.setFileInputFiles` accepts `nodeId`, `backendNodeId` or `objectId`. Since the chooser event
hands you a `backendNodeId`, pass that directly and skip the `DOM.getDocument` /
`DOM.querySelector` dance the probes used.

To **cancel**, call `DOM.setFileInputFiles` with `files: []`. Do not simply ignore the event: the
page's promise/`change` handler may be waiting, and a chooser that never resolves is a page that
looks broken.

### 4.7 Terminal UI — the file picker

This is the one prompt that needs real interaction rather than a keypress, and the terminal is
genuinely good at it. A single-line fuzzy path input with completion, plus a list.

```
        ┌─ Choose a file ─────────────────────────────────────────────────┐
        │  ex.co wants a file · images only · one file                    │
        │                                                                 │
        │  ~/Pictures/█                                                   │
        │                                                                 │
        │    holiday-2026.jpg          2.1 MB   12 Jun                    │
        │  ▸ profile.png             418.0 KB   03 Jul                    │
        │    screenshot-01.png         1.4 MB   29 Jul                    │
        │    scans/                         —   14 May                    │
        │                                                                 │
        │  ↑↓ move   → enter dir   tab complete   ⏎ choose   esc cancel   │
        └─────────────────────────────────────────────────────────────────┘
```

Multi-select (`mode: "selectMultiple"`) adds a checked column and a count; directory mode
(`selectDirectory`) hides files and the footer reads `⏎ choose this directory`:

```
        │  ▣ holiday-2026.jpg          2.1 MB   12 Jun                    │
        │  ▣ profile.png             418.0 KB   03 Jul                    │
        │  ☐ screenshot-01.png         1.4 MB   29 Jul                    │
        │                                                2 files selected │
        │  space toggle   ⏎ send 2 files   esc cancel                     │
```

Design notes that matter:

- **`esc` must always cancel**, and cancelling must call `DOM.setFileInputFiles` with `files: []`.
  Escape that does nothing is the worst outcome, because the page is still waiting.
- The `images only` subtitle is derived from `accept` via §4.5 and is **advisory**. Files not
  matching are dimmed, not hidden — `accept` is routinely wrong on real sites and a picker that
  refuses to show the file the user wants is a picker they will fight.
- Filenames are drawn through `unicode::sanitize_for_terminal`
  (`apps/cli/src/main.rs:887`) exactly as the title and URL already are. A local filename is not
  page-controlled, but it *is* arbitrary bytes, and RTL-override or C1 bytes in a filename would
  corrupt the prompt frame just as effectively as a hostile page title.
- The picker starts in the user's home or last-used directory, and **paths are resolved and
  canonicalised in the Rust core, never in the engine** (§11.2).

---

## 5. Permissions

### 5.1 Two handlers, both required

`setPermissionRequestHandler` fires when a page *asks* (`navigator.geolocation.getCurrentPosition`,
`Notification.requestPermission`). `setPermissionCheckHandler` is **synchronous** and fires when
Chromium wants to know the current state without prompting — including from `navigator.permissions.query()`
and from internal checks before a feature runs. Electron's own docs are explicit that you need both
(`electron.d.ts:13238-13241`, `:13251-13254`).

The asymmetry matters for BlackGlass: the *request* handler can be async (its `callback` may be
invoked later, which is exactly what a round-trip to the terminal needs), but the **check handler
must return a boolean immediately** and therefore can only consult an in-memory decision cache.
It can never prompt.

### 5.2 What Chromium actually sends (F7)

Probe B installed both handlers and exercised six web APIs. Observed:

| Handler | Strings observed |
|---|---|
| request | `notifications`, `geolocation`, `media`, **`persistent-storage`** |
| check | `media`, `geolocation`, **`web-app-installation`**, **`automatic-fullscreen`** |

Diffed mechanically against the TypeScript unions in `electron.d.ts:13246` (check, 19 members) and
`:13255` (request, 20 members):

```
OBSERVED request strings NOT in the d.ts union: ['persistent-storage']
OBSERVED check   strings NOT in the d.ts union: ['web-app-installation', 'automatic-fullscreen']
```

**Three of the seven strings we saw in a five-minute probe are absent from Electron's own type
definition.** The unions are not exhaustive and must not be treated as a closed set. Any
`switch` over permission strings therefore needs a `default` arm, and that arm must **deny**. A
`default: allow` — or worse, a TypeScript `never` exhaustiveness check that a future Chromium quietly
falsifies — is how a browser grants a capability nobody designed for.

The `details` payloads observed, verbatim:

```json
{"permission":"notifications","details":{"isMainFrame":true,"requestingUrl":"http://127.0.0.1:50442/"}}
{"permission":"geolocation",  "details":{"isMainFrame":true,"requestingUrl":"http://127.0.0.1:50442/"}}
{"permission":"media",        "details":{"isMainFrame":true,"mediaTypes":["audio","video"],
                                          "securityOrigin":"http://127.0.0.1:50442/",
                                          "requestingUrl":"http://127.0.0.1:50442/"}}
{"permission":"persistent-storage","details":{"isMainFrame":true,"requestingUrl":"http://127.0.0.1:50442/"}}
```

`isMainFrame` and `requestingUrl` are always present (`PermissionRequest`, `electron.d.ts:10785`).
`media` additionally carries `mediaTypes` and `securityOrigin` (`MediaAccessPermissionRequest`,
`:9414`). `openExternal` carries `externalURL` (`:10728`); `fileSystem` carries `filePath`,
`isDirectory` and `fileAccessType` (`FilesystemPermissionRequest`, `:8484`) — neither was triggered
in the probes **`[UNVERIFIED]`**, but the shapes are read from the shipped type definition.

One measured subtlety: `navigator.clipboard.readText()` returned `NotAllowedError` **without ever
reaching either handler** (`"clip:NotAllowedError"` in the probe result). Clipboard read is
activation-gated upstream of the permission layer. Do not build a clipboard prompt expecting the
handler to fire — it will not, absent a user gesture.

### 5.3 The default policy

The correct default for a terminal browser is stricter than for a desktop browser, because the
consequences of a mistake are worse: BlackGlass is frequently driven over SSH (A07), where "share
your camera" means a camera in a datacentre, and "share your location" means the geolocation of a
server.

| Permission | Default | Rationale |
|---|---|---|
| `notifications` | **prompt** | Renderable as terminal UI (§6); genuinely useful |
| `geolocation` | **prompt**, warn if remote | Over SSH the answer is the server's location, which is misinformation, not privacy |
| `media` (audio/video) | **deny**, prompt only if a device exists | No camera on a headless host; a prompt for a device that cannot work is user-hostile |
| `display-capture` | **deny**, no prompt | Capturing the screen of an OSR browser that has no screen is meaningless. Also requires `setDisplayMediaRequestHandler` (`electron.d.ts:13229`) or `getDisplayMedia` fails outright |
| `fullscreen`, `pointerLock`, `keyboardLock`, `window-management` | **deny**, no prompt | No window; no meaning |
| `openExternal` | **prompt**, always show `externalURL` | Launches a program outside the terminal. Highest-consequence item on this list |
| `fileSystem` | **prompt**, show `filePath` + `fileAccessType` | File System Access API; `writable` is a genuinely dangerous grant |
| `midi`, `midiSysex`, `hid`, `serial`, `usb` | **deny**, no prompt | Device access from a terminal browser is not a use case worth the attack surface |
| `persistent-storage`, `storage-access`, `top-level-storage-access` | **allow** silently | Low consequence; prompting is pure noise. Note `persistent-storage` is undocumented (F7) |
| `clipboard-read` | **prompt** | Reaching into the user's clipboard. See §5.2 — often never fires |
| `clipboard-sanitized-write` | **allow** | Standard, low-risk |
| `idle-detection`, `speaker-selection`, `mediaKeySystem` | **deny**, no prompt | No use case here |
| **anything unrecognised** | **deny**, log once | The F7 rule. Non-negotiable |

"Deny, no prompt" is a deliberate category. A prompt the user must dismiss to learn the answer is
always "no" is worse than a silent denial — it trains people to hit *allow*.

### 5.4 Terminal UI — the permission prompts (ASCII mockups)

All prompts share one frame, one keymap, and one rule: **the safe answer is the default, and `esc`
picks it.**

**Geolocation** — the extra line appears only when the engine is not on the same host as the tty:

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   ex.co  wants your location                                    │
        │                                                                 │
        │   ! this session runs on build-07.internal — the site would     │
        │     receive that machine's location, not yours                  │
        │                                                                 │
        │   [a] allow    [d] deny    [A] always    [D] never              │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**Camera and microphone** — `mediaTypes` is rendered literally, because "camera and microphone" and
"microphone" are very different requests and a browser that blurs them is lying:

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   meet.ex.co  wants your camera and microphone                  │
        │                                                                 │
        │   ! no camera detected on this host                             │
        │                                                                 │
        │   [a] allow    [d] deny    [A] always    [D] never              │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**Notifications** — no warning line; the capability is fully renderable (§6):

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   news.ex.co  wants to send notifications                       │
        │                                                                 │
        │   shown in your status bar; ctrl+n to review                    │
        │                                                                 │
        │   [a] allow    [d] deny    [A] always    [D] never              │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**Open external** — the highest-consequence prompt, so the URL gets its own line and is never
truncated below the host; deny is emphasised:

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   ex.co  wants to open an application outside the terminal      │
        │                                                                 │
        │   zoommtg://ex.co/join?confno=1234567890&pwd=…                  │
        │                                                                 │
        │   [a] allow once    [d] deny    [D] never for ex.co             │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**File System Access, writable** — `filePath` and `fileAccessType` come straight from
`FilesystemPermissionRequest` (`electron.d.ts:8484-8497`):

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   editor.ex.co  wants to WRITE to a folder on this machine      │
        │                                                                 │
        │   ~/projects/notes/            (directory, read + write)        │
        │                                                                 │
        │   [a] allow    [d] deny    [A] always for this folder           │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**Sub-frame requests** — when `isMainFrame` is `false`, the requesting origin differs from the page
the user thinks they are on, and that must be visible or the prompt is actively misleading:

```
        ┌─ Permission ────────────────────────────────────────────────────┐
        │                                                                 │
        │   an embedded frame wants your location                         │
        │                                                                 │
        │   frame  ads.tracker.example                                    │
        │   page   news.ex.co                                             │
        │                                                                 │
        │   [a] allow    [d] deny    [D] never                            │
        │                                                       esc = deny│
        └─────────────────────────────────────────────────────────────────┘
```

**Denied-and-remembered feedback.** After `[D] never`, subsequent requests from that origin must
not re-prompt, but silence is confusing. A one-second status-bar flash is enough:

```
 ... news.ex.co  |  60fps 52KB 0.7ms  |  ⊘ location blocked (ctrl+, to change)  ctrl+q quit
```

Origin strings are truncated with an ellipsis at a **label boundary from the left**, never the
right: `verylongsubdomain.ex.co` → `…ex.co`, so the registrable domain is always the part that
survives. Truncating from the right (`verylongsubdomain.ex…`) hides the only part that identifies
who is asking, and is a phishing aid.

### 5.5 Persistence

`[A] always` / `[D] never` write to a per-origin decision store owned by the Rust core, alongside
whatever B09 defines for profile data. Three requirements:

1. The store is keyed by **origin** (scheme + host + port), not host. `http://ex.co` and
   `https://ex.co` are different principals.
2. The **check handler's** in-memory cache is populated from this store at startup, since it cannot
   do I/O on the synchronous path (§5.1).
3. There must be a way to see and revoke decisions (`ctrl+,`), or "always" becomes a trap. A
   permission you cannot un-grant is worse than one you must re-approve.

---

## 6. Notifications

Two separable things share the word. §5 covers the *permission*. This covers *displaying* a
notification once granted.

Electron delivers web `Notification`s to the OS notification centre. On a headless or SSH host that
is either invisible or, worse, appears on the wrong machine's desktop. BlackGlass should render
them in the terminal instead.

There is no `session` hook for web notifications. The available levers are CDP —
`Browser.setPermission` and the `Notification` domain — or, more robustly, **a preload script that
replaces `window.Notification` with a shim posting to the engine**. The preload route is more work
but survives CDP detach and does not depend on an experimental domain. **`[UNVERIFIED]`** — no probe
was run for notification *delivery*; only the permission request was measured. §12 gives the test.

Presentation: a toast occupying the status row for ~4 seconds, then reverting. Never an overlay —
a notification that covers the page the user is reading is the behaviour everyone hates in a GUI
browser, and it is worse in 23 rows.

```
 ⓘ news.ex.co · Breaking: markets close higher                              [ctrl+n] 3 more
```

`ctrl+n` opens the log, which is the honest way to handle a burst without stealing 23 rows:

```
        ┌─ Notifications ─────────────────────────────────────────────────┐
        │  22:31  news.ex.co    Breaking: markets close higher            │
        │  22:29  chat.ex.co    alice: are you seeing this?               │
        │  22:14  ci.ex.co      build #4821 failed                        │
        │                                                                 │
        │   [d] dismiss all   [m] mute news.ex.co   [q] close             │
        └─────────────────────────────────────────────────────────────────┘
```

Notification titles and bodies are wholly page-controlled and go through
`unicode::sanitize_for_terminal` before reaching the tty — same rule as the page title at
`apps/cli/src/main.rs:887`, same reason.

---

## 7. JavaScript dialogs — in scope, and free

`alert()`, `confirm()`, `prompt()` and `beforeunload` are native modals with the F1 problem, and
D05 would be incomplete without them. They turned out to be the easiest surface of all, because CDP
handles them completely. Probe B:

```
PROBE:{"k":"cdpEvent","v":{"method":"Page.javascriptDialogOpening","params":{
        "defaultPrompt":"","frameId":"16291C26…","hasBrowserHandler":true,
        "message":"proceed?","type":"confirm","url":"http://127.0.0.1:50340/"}}}
PROBE:{"k":"cdp:Page.handleJavaScriptDialog","v":{"ok":true,"r":{}}}
PROBE:{"k":"confirmResult","v":{"ok":true,"r":"true"}}
```

The page's `confirm()` returned **`true`** after a programmatic accept. Everything needed is in the
event: `type` (`alert`/`confirm`/`prompt`/`beforeunload`), `message`, `defaultPrompt`, and the
originating `url`.

`hasBrowserHandler: true` confirms the browser-side handler is engaged, so the native path is
suppressed. But note carefully: **the renderer's main thread is blocked for the entire time the
dialog is open** — that is what `confirm()` being synchronous means. The page will not repaint. The
prompt overlay must therefore be drawn from the *last received frame* plus terminal text, which is
exactly what the compositor already does (C06: one pixel layer, one text layer, terminal composites)
— so this costs nothing architecturally, but a prompt that waits for a fresh frame before drawing
would hang forever. Draw immediately.

```
        ┌─ ex.co says ────────────────────────────────────────────────────┐
        │                                                                 │
        │   Your session will expire in 60 seconds. Continue?             │
        │                                                                 │
        │   [⏎] ok    [esc] cancel                                        │
        └─────────────────────────────────────────────────────────────────┘
```

`prompt()` adds an input line seeded with `defaultPrompt`, and the reply goes back as
`Page.handleJavaScriptDialog({accept:true, promptText:"…"})`. `beforeunload` is worded as
*"Leave this page?"* with **cancel as the default**, because the destructive answer is leaving.

Rate-limit: after three dialogs from one origin within five seconds, add a
`[b] block dialogs from ex.co` action. Unlike the file chooser (§4.4) there is **no** platform
activation gate on `alert()`, so a hostile page really can loop it, and the loop would be
unbreakable — the renderer is blocked, so `ctrl+q` needs to be handled by the core, not the page.
It already is (`apps/cli/src/main.rs:572`), which is a small piece of luck worth not breaking.

---

## 8. macOS quarantine — the requirement

### 8.1 The measurement: Electron does not quarantine downloads

Three independent lines of evidence.

**(a) Download into `/private/tmp` (probe C):**

```
PROBE:{"k":"QUARANTINE_ANSWER","v":{"allXattrs":"com.apple.provenance: ",
                                     "quarantine":"ABSENT"}}
```

**(b) Control download into the real `~/Downloads` (probe D)** — eliminating any suspicion that
`/private/tmp` is special-cased:

```
PROBE:{"k":"defaultDownloadPath","v":"/Users/adeebbashir/Downloads"}
PROBE:{"k":"CONTROL_blackglass-probe-CONTROL.bin","v":{
        "path":"/Users/adeebbashir/Downloads/blackglass-probe-CONTROL.bin",
        "exists":true,"allXattrs":"com.apple.provenance: ","quarantine":"ABSENT"}}
```

Re-checked from the shell after the process exited, to rule out a reporting artefact:

```
$ xattr -l ~/Downloads/blackglass-probe-CONTROL.bin
com.apple.provenance:
```

(The control file was deleted afterwards; `~/Downloads` is back to its prior state.)

**(c) Binary evidence:**

```
$ nm -u ".../Electron Framework.framework/Versions/A/Electron Framework" | grep -i 'qtn_\|Quarantine'
(no output)
```

The 192 MB framework has **no undefined `qtn_*` symbols** — it never links `libquarantine`. It does
link `CoreServices`, so the deprecated `LSSetItemAttributes` path is reachable, but the modern
quarantine API is simply not called.

The only xattr present is `com.apple.provenance`, which macOS applies automatically to files written
by non-App-Store binaries. **It is not Gatekeeper's quarantine and confers none of its protection.**

**Therefore: applying `com.apple.quarantine` is BlackGlass's job.** A browser that downloads an
installer to `~/Downloads` without quarantining it has stripped a platform safety control that every
other macOS browser on the machine applies (§8.2 shows Safari and Chrome both doing it). That is a
genuine security regression shipped under the word "browser", and it is the single most important
platform-specific requirement in this document.

### 8.2 The xattr format, decoded from real data on this machine

Rather than trusting a blog post, I read the attribute off files that real browsers downloaded:

```
Tailscale-1.98.9-macos.pkg   0083;6a61937d;Safari;3A12871C-D839-4332-B0C8-119AA4694250
anydesk.dmg                  0083;6a60f2a2;Safari;89A4FA82-D9DC-44CA-95EF-79598B83CBE7
github-recovery-codes.txt    0281;6a59a249;Chrome;0A6DC34B-D4E2-421D-B4E6-97EB7A545F24
claude-skills-main.zip       0281;6a409f98;Chrome;61AAE6CF-8590-48E0-B02C-8001AC33E4D0
IMG_9040.jpg                 0081;6a665708;sharingd;513AB110-EDE2-4DB6-BC73-D210D1FE17EB
keelcode-loop-engineering.mp4 0082;6a5798a1;QuickTime Player;
```

Four semicolon-separated fields: **flags;timestamp;agent;event-UUID**.

The timestamp is **Unix epoch seconds in lowercase hex**, verified against file mtime:

| xattr hex | decoded | file mtime | match |
|---|---|---|---|
| `6a61937d` | 1784779645 → 2026-07-23T09:37:25 | 2026-07-23T09:37:25 | ✔ |
| `6a59a249` | 1784259145 → 2026-07-17T09:02:25 | 2026-07-17T09:02:25 | ✔ |
| `6a409f98` | 1782620056 → 2026-06-28T09:44:16 | 2026-06-28T09:44:16 | ✔ |

Three for three, to the second.

The fourth field is the `LSQuarantineEventIdentifier`, a foreign key into the LaunchServices
quarantine event database. That database exists on this machine and its schema confirms the model
(read-only query):

```
$ sqlite3 "file:$HOME/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2?mode=ro" .schema
CREATE TABLE LSQuarantineEvent (
  LSQuarantineEventIdentifier TEXT PRIMARY KEY NOT NULL, LSQuarantineTimeStamp REAL,
  LSQuarantineAgentBundleIdentifier TEXT, LSQuarantineAgentName TEXT,
  LSQuarantineDataURLString TEXT, LSQuarantineSenderName TEXT, LSQuarantineSenderAddress TEXT,
  LSQuarantineTypeNumber INTEGER, LSQuarantineOriginTitle TEXT,
  LSQuarantineOriginURLString TEXT, LSQuarantineOriginAlias BLOB );
```

`LSQuarantineOriginURLString` and `LSQuarantineDataURLString` are where *"downloaded from
https://…"* lives — the xattr itself carries only the UUID. The matching public constants are in the
SDK's LaunchServices headers (`kLSQuarantineOriginURLKey`, `kLSQuarantineDataURLKey`,
`kLSQuarantineAgentNameKey`, `kLSQuarantineAgentBundleIdentifierKey`, `kLSQuarantineTypeKey`, and
type constants including `kLSQuarantineTypeWebDownload`).

The flags field is a hex bitfield. Values observed here (`0081` sharingd, `0082` QuickTime,
`0083` Safari, `0281` Chrome) differ between agents; the exact bit semantics are **not** documented
in any public header on this machine, and I will not guess them. **`[UNVERIFIED]`** — see §14 for
what this means in practice.

### 8.3 Writing it — measured, and the API to prefer

Writing the attribute works and round-trips byte-exactly (probe C):

```
PROBE:{"k":"manualQuarantineApplied","v":{
  "wrote":   "0181;6a6cd605;BlackGlass;233F0BDB-0BE6-406B-80C9-7283BBCE1C1D",
  "readBack":"0181;6a6cd605;BlackGlass;233F0BDB-0BE6-406B-80C9-7283BBCE1C1D",
  "matches": true}}
```

That probe shelled out to `/usr/bin/xattr`. **Do not ship that.** `libquarantine` is a system
library with a full public API; its exported symbols are in the SDK stub
(`$SDK/usr/lib/system/libquarantine.tbd`) and include exactly what is needed:

```
_qtn_file_alloc          _qtn_file_init_with_path   _qtn_file_set_flags
_qtn_file_free           _qtn_file_set_identifier   _qtn_file_apply_to_path
_qtn_file_get_flags      _qtn_file_set_metadata     _qtn_file_apply_to_fd
_qtn_file_get_identifier _qtn_file_set_timestamp    _qtn_xattr_name
```

Using it avoids spawning a process per download, avoids quoting bugs, and lets the flags be set
symbolically rather than by writing a hex literal nobody can justify (§8.2). It is a system library:
**linking it raises no licensing question**, unlike vendoring third-party code. The `libc` crate is
already a workspace dependency (`Cargo.toml`), so the `extern "C"` declarations need no new crate.

Applying quarantine belongs in the **Rust core**, not the engine, and specifically at the moment the
staged file is moved to its final destination (§3.3):

1. `will-download` → `setSavePath(staging/<id>.part)` (engine, synchronous — F1)
2. `done` with `state: "completed"` → engine reports the staged path
3. core moves staged → final destination
4. core applies `com.apple.quarantine` **before** the file is visible at its final path
5. core writes the LaunchServices event row so Finder can show the origin URL

Step 4 before step 3's rename would be better still — quarantine the `.part` file, then rename —
so there is no window in which a fully-written, unquarantined file exists at a path the user might
open. Renaming preserves xattrs within a volume. **`[UNVERIFIED]`** across volumes; if the staging
directory and `~/Downloads` can be on different filesystems, re-apply after the copy and verify.

Agent name should be `BlackGlass` (it is what the Gatekeeper dialog shows the user); the UUID should
be a fresh v4 per download.

### 8.4 A negative result that prevents a bogus test

I expected `spctl` to serve as the verification oracle. **It does not**, and this is worth recording
so nobody writes a test around it. Two byte-identical files, one quarantined, one not:

```
$ spctl -a -vv clean.pdf → rejected / source=no usable signature   (rc=3)
$ spctl -a -vv quar.pdf  → rejected / source=no usable signature   (rc=3)
```

Identical. `spctl` assesses code signatures, not quarantine state, and returns the same rejection
for any unsigned file regardless. **The only valid check is reading the xattr back.** The
verification in §12 does exactly that and nothing else.

### 8.5 Other platforms

Linux has `user.xdg.origin.url` and `user.xdg.referrer.url` (freedesktop convention, honoured by
several file managers). Windows has the NTFS `Zone.Identifier` alternate data stream, which is the
one that genuinely gates execution via SmartScreen. **`[UNVERIFIED]`** on both — this host is macOS
only. The design should put quarantine behind a small platform trait with a no-op default, so the
Linux and Windows implementations are additive rather than a refactor.

---

## 9. One prompt layer, not five

Five surfaces (download, file picker, permission, notification log, JS dialog) all need: draw a box
over the page, capture input, return one answer, disappear. Building that five times guarantees five
different escape behaviours. The core needs one modal layer with a single state machine.

```rust
// SKETCH for apps/cli/src/main.rs — described, not written by this mission.
enum Prompt {
    Permission { id: String, kind: PermKind, origin: String, detail: Option<String>, sub_frame: bool },
    Download   { id: String, file: String, bytes: u64, origin: String, gesture: bool, hops: u8 },
    FilePicker { id: String, mode: PickMode, accept: Vec<String>, cwd: PathBuf, sel: Vec<PathBuf> },
    JsDialog   { id: String, kind: DialogKind, message: String, default: String, origin: String },
    List       { which: ListKind },   // downloads / notifications
}
```

Six rules that make it coherent:

1. **One prompt at a time.** Additional requests queue FIFO with a `(2 more)` badge. Stacked modals
   in 23 rows are unusable.
2. **The prompt owns the keyboard.** `handle_event` (`apps/cli/src/main.rs:562`) routes to the
   prompt and returns *without* forwarding to the engine when a prompt is active. This is a
   modification to an existing `match`, which is why it is the commander's to make.
3. **`ctrl+q` still quits** (`:572`) — checked before prompt routing. §7 explains why: a page can
   loop `alert()` and the renderer is blocked; the only escape must live in the core.
4. **Mouse is discarded while a prompt is up.** `PointerMap::to_page` (`:712`) already returns
   `None` outside the page; the prompt adds a second exclusion. Forwarding clicks to a page hidden
   behind a modal is a clickjacking primitive.
5. **The overlay is text, not pixels.** Drawn in the same `present` pass as the status bar
   (`:883-897`), over the resident kitty image. C06 establishes that text composites above a
   negative-`z` placement, so no pixel work and no image retransmission is needed — the overlay
   costs a few hundred bytes, not 8 MB.
6. **Every string from the page or the filesystem passes `unicode::sanitize_for_terminal`**
   before it reaches the tty (`:887-888`). Filenames, origins, dialog messages, notification
   bodies, `accept` hints. No exceptions.

Rendering is a **pure function** `fn render(&Prompt, cols: u16, rows: u16) -> Vec<String>`. That is
deliberate: this host cannot do visual verification (lock screen), so the only testable thing is a
function whose output is a `Vec<String>` that CI can assert on — box width, clamping, truncation,
sanitisation. §12 depends on this shape.

---

## 10. Protocol additions, consolidated

Nothing here changes framing; every message is a flat JSON object with a `t` field, parseable by
the existing hand-rolled reader (`crates/bg-proto/src/lib.rs:125`). **This is the complete list of
what the commander would add to `apps/engine/src/main.js`.**

| Direction | `t` | Payload | Trigger |
|---|---|---|---|
| E→C | `downloadAsk` | id, filename, mime, bytes, url, host, gesture, hops, suggested | `will-download` |
| E→C | `downloadProgress` | id, received, total, bps, paused | `item.on('updated')` |
| E→C | `downloadDone` | id, state, path, canResume | `item.once('done')` |
| E→C | `filePickerAsk` | id, mode, accept, name, frameOrigin | `Page.fileChooserOpened` |
| E→C | `permissionAsk` | id, permission, origin, isMainFrame, pageOrigin, mediaTypes?, externalURL?, filePath?, fileAccessType? | `setPermissionRequestHandler` |
| E→C | `dialogAsk` | id, kind, message, default, origin | `Page.javascriptDialogOpening` |
| E→C | `notification` | origin, title, body, tag | preload shim (§6) |
| C→E | `downloadAnswer` | id, action (`save`/`cancel`) | user |
| C→E | `downloadControl` | id, action (`pause`/`resume`/`cancel`) | user |
| C→E | `filePickerAnswer` | id, files[] (empty = cancel) | user |
| C→E | `permissionAnswer` | id, granted, remember | user |
| C→E | `dialogAnswer` | id, accept, text | user |

Three protocol-hygiene requirements, all of which follow from B06's threading of the same argument:

- **Every `id` is generated by the engine and echoed back.** The core must never invent one, and the
  engine must reject an unknown or already-answered id. An answer replayed against a recycled id is
  a permission grant the user never made.
- **Every ask needs a timeout in the engine.** If the core dies mid-prompt, the pending
  `callback(false)` / `DOM.setFileInputFiles([])` / `handleJavaScriptDialog({accept:false})` must
  still fire, or the page hangs forever and we have reinvented F1 in our own code.
- **Deny is the timeout answer.** Always.

---

## 11. Security analysis

### 11.1 Attacker-controlled filenames

`item.getFilename()` is derived from the server's `Content-Disposition`. Three distinct attacks:

- **Path traversal.** `filename="../../.ssh/authorized_keys"`. The staging design (§3.3) blunts this
  because the staged name is `<id>.part`, generated by us. The final move must still `basename()`
  the suggestion and reject any result containing a separator or equal to `.`/`..`.
- **Extension spoofing.** `invoice.pdf.command`, or an RTL override making `annexe.exe` render as
  `annexexe.pdf`. The prompt in §3.4 must show the **real, sanitized** filename, and
  `sanitize_for_terminal` must strip bidi controls specifically — a length clamp alone does not
  address this.
- **Terminal escape injection.** A filename containing `\x1b]0;…\x07` or a kitty graphics command
  reaching the tty raw would let a downloaded file's *name* drive the terminal. `apps/cli/src/main.rs:884`
  already records exactly this reasoning for page titles; downloads and file listings inherit it.

### 11.2 Path handling belongs in the core

The engine is the process that talks to hostile web content. If it also resolved filesystem paths,
a bug there would be a filesystem bug. So: the core sends **absolute, already-canonicalised** paths
to the engine, and the engine treats them as opaque. Specifically, the engine never resolves `~`,
never joins a page-supplied string onto a path, and never accepts a relative path in
`filePickerAnswer`. The picker's `accept` filter (§4.5) is page-controlled and therefore a display
hint only — it must never widen what the core is willing to read.

### 11.3 Prompt spoofing

A page renders whatever it likes, including a convincing picture of the permission prompt in §5.4,
positioned exactly where the real one appears. The mitigations available in a terminal are better
than in a GUI browser:

- The prompt sits in the **text layer**, which the page cannot draw into at all (C06). A page can
  only paint pixels that look like text.
- The status bar is reverse-video (`\x1b[7m`, `apps/cli/src/main.rs:886`) using the terminal's own
  palette, which the page does not know.
- **Strongest available signal:** while a prompt is up, dim or desaturate the page image. A page
  cannot dim itself relative to the overlay, because it does not control the overlay. This costs one
  pass over the frame buffer in the encoder and is the single highest-value anti-spoofing measure
  here. It should be non-optional.

### 11.4 The CDP attack surface

Attaching `webContents.debugger` gives our main process very broad control over the renderer. Two
consequences worth stating: DevTools cannot be opened on a `webContents` while we hold the debugger
(Electron emits `detach`, `electron.d.ts:7480-7482`), so any future devtools feature must coordinate;
and the `detach` event must be handled — if CDP silently detaches, file choosers revert to native
`NSOpenPanel` and F1 returns. **On `detach`, re-attach and re-arm; if re-attach fails, disable file
inputs and tell the user, rather than letting a native panel hang the session.**

### 11.5 Drive-by downloads

`hasUserGesture: false` was measured (§3.2). Surfacing it (§3.4) is the mitigation; blocking on it
is not, because plenty of legitimate flows are JS-initiated. What *should* be blocked without a
gesture is *auto-saving without a prompt* — i.e. any future "always save to Downloads" preference
must apply only when `hasUserGesture` is true.

---

## 12. Test plan

CI on this host cannot use a tty and cannot see a screen. Everything below is either a pure function
or a headless assertion, so it all runs in CI.

**Pure-function tests (Rust, alongside the existing suite at `apps/cli/src/main.rs:905+`):**

1. `render(Prompt::Permission{..}, 146, 23)` produces lines all `<= 146` display columns.
2. Same at `cols = 40` — the box degrades rather than wrapping.
3. A 300-character origin truncates **from the left** to `…ex.co` (§5.4). Assert the registrable
   domain survives.
4. A filename containing `\x1b]0;pwn\x07` renders with no `\x1b` in the output.
5. A filename containing U+202E (RTL override) renders with the override removed.
6. `basename` hardening: `../../etc/passwd`, `..`, `.`, `a/b` are all rejected as final names.
7. Permission policy: `unknown-future-permission` maps to `Deny` (F7). This is the regression test
   for the §5.3 default arm.
8. `mediaTypes: ["audio"]` renders "microphone", `["audio","video"]` renders "camera and
   microphone" — not the same string.
9. While a prompt is active, `handle_event` returns without emitting an engine command, except for
   `ctrl+q`.

**Headless Electron tests (Node, no tty, ~10 s):**

10. `will-download` fires and `done` reports `completed` within 5 s for a local fixture. *This is
    the F1 regression test* — if someone deletes the `setSavePath` call, this goes from 5 s to a
    20 s timeout and fails.
11. After `done`, `xattr -p com.apple.quarantine <path>` **exits 0 and matches**
    `^[0-9a-f]{4};[0-9a-f]+;BlackGlass;[0-9A-F-]{36}$`. Do **not** assert on `spctl` (§8.4).
12. `Page.setInterceptFileChooserDialog` is armed only after `did-finish-load`, and a synthesized
    `mouseDown`/`mouseUp` on a file input produces `Page.fileChooserOpened` within 2 s (F4 + F5).
13. `DOM.setFileInputFiles` with `files: []` resolves the chooser and leaves `input.files.length === 0`.
14. `confirm()` returns `true` after `handleJavaScriptDialog({accept:true})` — probe B already
    demonstrates this passes.
15. **Assert `Browser.setDownloadBehavior` is never called** — grep the engine source in CI. It is a
    one-line test that prevents F6, which is otherwise invisible until a user reports a missing file.

---

## 13. Implementation order

Ordered by (risk removed) ÷ (effort), not by section number.

1. **`will-download` + `setSavePath` into a fixed directory.** Removes the F1 hang. No UI, no
   protocol change, ~5 lines. **Do this first.**
2. **Default-deny `setPermissionRequestHandler` + `setPermissionCheckHandler`** with the §5.3 table
   and a denying `default` arm. Stops Electron's built-in handler granting things silently.
   ~30 lines, no UI.
3. **Quarantine on completion** (§8.3), via `libquarantine` from the core. Closes the security
   regression in §8.1. Testable by item 11 above.
4. **The prompt layer** (§9) with the permission prompt as its first and only consumer. This is the
   big one; everything after it is cheap.
5. **Download prompt + status-bar segment** (§3.4) — *and* the status-bar width clamp that C06 §12
   already flagged, which becomes load-bearing here.
6. **JS dialogs** (§7). Highest value per line of the CDP work: the mechanism is already proven and
   it removes another native-modal hang.
7. **File picker** (§4.7). The most UI work of any item.
8. **Notifications** (§6). Needs the preload-shim spike first (§14).

Steps 1–3 change no protocol, need no terminal UI, and could go in today.

---

## 14. Explicitly UNVERIFIED

- **Quarantine flag-bit semantics.** I read real values off this disk (`0081`, `0082`, `0083`,
  `0281`) but no public header on this machine documents the bits, and I will not guess. Practical
  consequence: **do not invent a flags value.** Start by writing the same literal Chrome uses
  (`0281`) or the one Safari uses (`0083`), or set flags symbolically via `qtn_file_set_flags` and
  read back what the system produced. Whether the value affects whether Gatekeeper prompts on first
  open is untested — that requires a GUI login, and this machine is at a lock screen.
- **Whether writing the `LSQuarantineEvent` DB row is required, or whether LaunchServices creates it.**
  The schema is confirmed; the write path is not. If BlackGlass writes only the xattr, Finder may
  show "downloaded from (unknown)".
- **`selectDirectory` chooser mode.** A `webkitdirectory` input existed in the fixture but was never
  clicked in the run that logged modes. Only `selectSingle` and `selectMultiple` were observed.
- **Notification *delivery*.** Only the `notifications` *permission request* was measured. The
  preload-shim approach in §6 is a design, not a measurement. Spike it before scheduling item 8.
- **`fileSystem` and `openExternal` permission payloads.** Shapes read from `electron.d.ts:8484`
  and `:10728`; neither was triggered at runtime. The §5.4 mockups assume those fields are populated.
- **Whether `did-finish-load` is early enough to arm CDP**, or whether `dom-ready` /
  `did-frame-finish-load` is needed for sub-frames. Probe C armed after an awaited `loadURL` and it
  worked; the exact earliest safe hook is untested.
- **xattr preservation across a cross-volume move.** Same-volume rename is assumed to preserve;
  cross-volume is untested, which matters if staging and `~/Downloads` differ.
- **Everything about Linux and Windows** (§8.5). This host is macOS 26.1 only.
- **Everything about iTerm2** — blocked by TCC per the project brief, as for every other mission.
- The probes ran with `--disable-gpu` and `app.dock.hide()`. Behaviour with the GPU process live is
  assumed identical for these code paths but was not re-measured.

## 15. Licence

No third-party code is reused or proposed for vendoring by this mission. The four probe programs are
original and live in the agent scratchpad, outside the repository. `libquarantine` (§8.3) is a macOS
system library reached via `extern "C"` declarations against symbols published in the platform SDK
stub — linking a system library raises no licensing obligation, and `libc` is already a workspace
dependency (`Cargo.toml`), so **no new crate is proposed**. CDP is a protocol, not code. Electron is
MIT (`apps/engine/node_modules/electron/LICENSE`) and is already a dependency.

One incidental observation for the commander, outside this mission's scope: **there is no `LICENSE`
file at the repository root**, although `Cargo.toml:9` declares `MIT OR Apache-2.0`. Publishing a
Chromium-embedding browser without the licence texts present — including Chromium's own, which
Electron ships at `node_modules/electron/dist/LICENSES.chromium.html` and which must be
redistributed with any binary — is worth fixing before B10 packaging ships anything.
