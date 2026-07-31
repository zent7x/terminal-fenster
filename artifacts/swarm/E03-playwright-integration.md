# E03 — Playwright attachment to a running BlackGlass session

**Mission:** determine exactly how Playwright can attach to our running visible session.
**Status:** answered and empirically verified end-to-end on this machine.
**Author:** swarm agent E03. **Date:** 2026-07-31.
**Files written:** this file only. No repo source was modified. All experiments ran in
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/e03/`.

---

## 1. Bottom line

Playwright attaches to our exact offscreen (OSR) Electron configuration and drives it
correctly. I ran it against a real offscreen `BrowserWindow` built with the *same*
`webPreferences` as `apps/engine/src/main.js:101-119` (`offscreen: true`, `sandbox: true`,
`contextIsolation: true`, `nodeIntegration: false`, `webSecurity: true`) and got
**16 pass / 2 fail**, where both failures are the two documented Electron limits and
neither blocks us.

Three things had to be true, and all three are measured true, not assumed:

1. Electron 43.2.0 accepts `--remote-debugging-port` and starts a real Chromium DevTools
   HTTP server.
2. `/json/version` reports `"Browser": "Chrome/150.0.7871.129"` — **not** `Electron/…` — so
   no `/json/version` rewriting proxy is needed. (This hack is widely recommended online.
   It is unnecessary here. See §4.)
3. An **offscreen, `show: false`** window still appears in `/json/list` as
   `"type": "page"` with its own `webSocketDebuggerUrl`. Being offscreen does not hide the
   target.

Cost to adopt: one opt-in flag in the engine and one in the CLI spawn. Both described in
§9 — **not implemented**, per the file-ownership rule.

---

## 2. Verified environment

| Item | Value | How established |
|---|---|---|
| Electron | 43.2.0 | `electron --version` |
| Chromium | 150.0.7871.129 | engine `ready` event + `/json/version` |
| `playwright-core` | 1.62.1 | `npm install playwright-core` in scratchpad, 13 MB, **no browser download** |
| Node | v24.11.1 | `node -v` |
| Disk headroom | 5.7 GiB free | `df -h`; install fit comfortably |

`playwright-core` is the right dependency, not `playwright`. The full `playwright` package
triggers a browser download of several hundred MB; `playwright-core` does not, and
`connectOverCDP` never needs a bundled browser because it drives *ours*. This matters given
the 98 %-full disk constraint.

---

## 3. How `connectOverCDP` actually works

From the Playwright docs (`browserType.connectOverCDP`, added v1.9):

- "Connecting over the Chrome DevTools Protocol is only supported for Chromium-based
  browsers." Electron qualifies.
- `endpointURL` accepts "a CDP websocket endpoint or http url", e.g. `http://localhost:9222/`
  or `ws://127.0.0.1:9222/devtools/browser/<uuid>`.
- "The default browser context is accessible via `browser.contexts()`."
- Explicit caveat: "This connection is significantly lower fidelity than the Playwright
  protocol connection via `browserType.connect()`."
- Explicit caveat: "Playwright maintains a curated list of arguments for launching the
  browser. If you launch the browser without Playwright and do not pass the exact same
  arguments, some of Playwright functionality may be broken upon connecting." We launch
  Electron ourselves, so we are permanently in this "not the curated arg list" regime. In
  practice the only fallout I could measure is §6.

Mechanically, from `playwright-core` source
(`packages/playwright-core/src/server/chromium/chromium.ts`, `urlToWSEndpoint`):

```
if (endpointURL.startsWith('ws'))
    return endpointURL;          // used as-is, no HTTP probe
...
url.pathname += '/';
url.pathname += 'json/version/'; // note the trailing slash
...
return JSON.parse(json).webSocketDebuggerUrl;
```

So an `http://` endpoint causes exactly one GET of **`/json/version/`** (trailing slash),
and the **only** field Playwright reads is `webSocketDebuggerUrl`. On a non-200 it throws
`Unexpected status ${code} when connecting to ${httpURL}. This does not look like a
DevTools server, try connecting via ws://.`

---

## 4. What `/json/version` must return — measured, not assumed

Live response from our offscreen Electron on port 9412:

```json
{
   "Browser": "Chrome/150.0.7871.129",
   "Protocol-Version": "1.3",
   "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.7871.129 Electron/43.2.0 Safari/537.36",
   "V8-Version": "15.0.1240245",
   "WebKit-Version": "537.36 (@e69b30bba288603e514cffb4c79c359cac68e923)",
   "webSocketDebuggerUrl": "ws://127.0.0.1:9412/devtools/browser/a26770b2-2243-4f31-82ed-64bb388a82be"
}
```

**The load-bearing field is `webSocketDebuggerUrl` and nothing else.** The `Browser` field
already says `Chrome/150.0.7871.129`; the `Electron/43.2.0` token appears only inside
`User-Agent`, which Playwright does not gate on. `browser.version()` returned
`150.0.7871.129`.

This is worth stating plainly because the internet's standard advice — stand up a proxy
that rewrites `/json/version` so `Browser` says `Chrome/...` instead of `Electron/...` — is
**not needed for us**. Do not build that proxy.

Path handling, all measured on port 9415:

| Path | HTTP |
|---|---|
| `/json/version` | 200 |
| `/json/version/` (what Playwright requests) | 200 |
| `/json/list` | 200 |
| `/json/protocol` | 200 |

The trailing-slash form works, so the trailing-slash bug reported against some Chrome builds
does not affect Electron 43.

`/json/list` for the offscreen window:

```json
[ {
   "id": "8735CCAF7BBCD90A8A0B79D55E2A97FA",
   "title": "BG OSR",
   "type": "page",
   "url": "data:text/html,<title>BG OSR</title>...",
   "webSocketDebuggerUrl": "ws://127.0.0.1:9415/devtools/page/8735CCAF7BBCD90A8A0B79D55E2A97FA"
} ]
```

---

## 5. Does it work against Electron? Yes — and a widely-repeated claim is wrong

There is a live Playwright issue, [#39008](https://github.com/microsoft/playwright/issues/39008)
(opened 2026-01-28, still open), titled *"electron.launch() fails with Electron 30+ —
bad option: --remote-debugging-port"*. Secondary sources summarise it as **"Electron 30+
rejects `--remote-debugging-port` as a CLI argument; it only works via
`app.commandLine.appendSwitch()`."**

**That generalisation is false for our configuration, and I tested all four permutations on
Electron 43.2.0:**

| # | Invocation | Result |
|---|---|---|
| B | `electron --remote-debugging-port=9412 app.js` (flag **before** script) | works, server on 9412 |
| C | `electron --remote-debugging-port=0 app.js` (port 0, the exact value Playwright passes) | works, ephemeral port 54838 |
| D | `electron app.js --remote-debugging-port=9413` (flag **after** script) | works, server on 9413 |
| E | `app.commandLine.appendSwitch('remote-debugging-port', '9414')` in main | works, server on 9414 |

Case D matters most: `apps/cli/src/main.rs:402-411` already passes the script path first and
`--bg-*` flags after, so appending one more flag in that position is proven to work.

I then reproduced the real failure exactly, and it is an **environment** problem, not an
Electron-version problem:

```
$ ELECTRON_RUN_AS_NODE=1 electron --remote-debugging-port=0 ./osr-app.js
.../Electron.app/Contents/MacOS/Electron: bad option: --remote-debugging-port=0
```

That is the verbatim error string from #39008. With `ELECTRON_RUN_AS_NODE=1` set, the binary
hands argv to **Node's** option parser, which has never heard of a Chromium switch and
rejects it. Without that variable, Chromium's parser handles it fine.

**Actionable consequence for BlackGlass:** if `ELECTRON_RUN_AS_NODE` is ever present in the
environment the Rust core inherits, engine startup breaks — with or without CDP. `Command`
in `apps/cli/src/main.rs:402` inherits the parent environment. Explicitly clearing that
variable at the spawn site is a cheap, permanent immunisation.

---

## 6. Capability matrix — measured against a real offscreen window

Full run, `playwright-core` 1.62.1 → Electron 43.2.0 OSR window on `http://127.0.0.1:9416`:

```
PASS  connectOverCDP                242ms
PASS  browser.version()             150.0.7871.129
PASS  browser.isConnected()         true
PASS  browser.contexts().length     1
PASS  contexts[0].pages().length    1
PASS  page.url()                    data:text/html,<title>BG OSR</title>...
PASS  page.title()                  BG OSR
PASS  page.evaluate()               2
PASS  locator.textContent()         hello osr
PASS  viewport from page            1280x800 dpr=1
PASS  page.goto()                   318ms -> NAV OK
PASS  page.click()                  title became CLICKED
PASS  page.fill()                   typed-by-playwright
PASS  page.screenshot()             7849 bytes in 74ms, PNG=true
FAIL  browser.newContext()          Protocol error (Target.createBrowserContext): Failed to create browser context.
FAIL  context.newPage()             Protocol error (Target.createTarget): Not supported
PASS  newCDPSession + raw CDP       cssContentSize={"x":0,"y":0,"width":1280,"height":800}
PASS  browser.close() [disconnect]  returned
--- 16 pass / 2 fail ---
```

Notable confirmations:

- **`page.screenshot()` works on an offscreen window** — 7849-byte real PNG in 74 ms. This
  gives CI a pixel oracle for the *browser* side, independent of the terminal side, which
  directly addresses the "machine is at a lock screen, screenshots unavailable" constraint
  in our environment notes. Playwright's screenshot goes through CDP, not the OS
  compositor, so a locked screen does not block it.
- **`browser.close()` disconnects without killing the engine.** I checked the engine PID
  immediately after and it was alive and still painting. This is the single most important
  safety property for attaching to a *live user session* and it holds.
- **`newCDPSession()` works**, so anything Playwright's API does not expose is still
  reachable as raw CDP.

### The two hard limitations

1. **`browser.newContext()` fails** — `Target.createBrowserContext` → "Failed to create
   browser context". Electron does not implement multi-context. So: no isolated
   incognito-style contexts, no per-test storage isolation, no `storageState` juggling
   across contexts.
2. **`context.newPage()` fails** — `Target.createTarget` → "Not supported". **Playwright
   cannot open tabs.** Every page it drives must already have been created by our engine.

Both are inherent to Electron, not to our code, and neither is fatal: BlackGlass's tab
lifecycle is owned by the Rust core by design, so Playwright driving pages the core created
is the correct division of labour anyway. Tests that need a new tab must ask the core for
one over our type-10 command channel, then re-enumerate `context.pages()`.

---

## 7. Two hazards specific to us

### 7.1 `page.setViewportSize()` silently resizes the terminal's frame surface — REAL

This is the finding I would most want the commander to see. Measured directly from the
engine's `paint` handler:

```
[osr] SIZE-CHANGE -> 1280x800 (at paint 1)
[pw] --- now calling setViewportSize(640,480) ---
[pw] setViewportSize returned
[pw] inner= 640x480
[osr] SIZE-CHANGE -> 640x480 (at paint 1)
```

A Playwright script calling `setViewportSize()` issues `Emulation.setDeviceMetricsOverride`,
which **changes the dimensions of the frames the engine emits**. The terminal core is
holding a size it negotiated from the real cell grid; it would start receiving frames of a
different geometry with no `resize` command having been sent. Depending on how the encoder
trusts the negotiated size, that is a corrupted render or worse.

Mitigation, cheapest first: document `setViewportSize()` as forbidden in BlackGlass tests
(the frame header already carries authoritative width/height at
`apps/engine/src/main.js:106-108`, so the core *can* be made to trust the header and
re-assert). A test-lint rule banning the call is close to free.

### 7.2 The debug port is an unauthenticated local control channel

`--remote-debugging-port` binds `127.0.0.1` only, and Chromium's DNS-rebinding guard works —
a request with `Host: evil.example` returned **HTTP 500** while `Host: 127.0.0.1` returned
200. So remote and browser-based attacks are blocked.

But **any local process running as the user can attach and take full control of the
browser** — read cookies, exfiltrate session state, navigate, execute script. This
contradicts the security posture stated in the engine header comment
(`apps/engine/src/main.js:15-16`: "it opens no listening port of its own"). Therefore CDP
must be **opt-in, off by default**, and the header comment updated when it lands.

Note `--remote-debugging-pipe` is the safer transport (fd-based, no TCP), but Playwright's
`connectOverCDP` only accepts `http://` or `ws://` URLs, so pipe is **not** usable here.
*(Doc-derived inference, not separately tested.)*

### 7.3 Frame-rate impact: none measured

Concern: does an attached CDP client disturb the 60 fps OSR pipeline? Measured with a CSS
animation driving continuous repaints:

| Condition | paints/sec |
|---|---|
| Baseline, no CDP client | 49, 62, 72 |
| Playwright attached + `screenshot()` every 1 s | 59, 60, 74, 61, 59, 60 |

No degradation. Attachment is safe for a live session.

*(Method note: an earlier run using `requestAnimationFrame` showed ~0–1 paints/sec in
**both** conditions. That is rAF throttling in an occluded offscreen window, a known
Electron OSR behaviour unrelated to CDP — not a frame-rate regression and not a challenge
to the project's measured 60 fps. I switched to a CSS animation, which is not throttled, to
get the comparison above.)*

---

## 8. Working example

Verified working against our OSR config. Requires `npm i -D playwright-core` (~13 MB, no
browser download).

```js
// tests/e2e/attach.mjs — drive a *running* BlackGlass engine over CDP.
import { chromium } from 'playwright-core';
import { readFileSync } from 'node:fs';

// Prefer port 0 + DevToolsActivePort over a fixed port (see §9.2).
function endpointFromUserDataDir(dir) {
  const [port, path] = readFileSync(`${dir}/DevToolsActivePort`, 'utf8').split('\n');
  return `ws://127.0.0.1:${port}${path}`;   // ws:// skips the /json/version probe entirely
}

const endpoint = process.env.BG_CDP_WS
  ?? (process.env.BG_USER_DATA_DIR
      ? endpointFromUserDataDir(process.env.BG_USER_DATA_DIR)
      : 'http://127.0.0.1:9222');

const browser = await chromium.connectOverCDP(endpoint);

// Electron exposes exactly one context and cannot create more.
const [context] = browser.contexts();

// Playwright CANNOT create pages here (Target.createTarget: Not supported).
// The engine owns tab lifecycle; we attach to what already exists.
const [page] = context.pages();
if (!page) throw new Error('engine exposed no page target');

await page.goto('https://example.com', { waitUntil: 'domcontentloaded' });
console.log('title:', await page.title());

await page.click('a');
console.log('url after click:', page.url());

// Works on an offscreen window; gives CI a pixel oracle with no display attached.
await page.screenshot({ path: 'artifacts/attach.png' });

// Escape hatch for anything Playwright's API does not cover.
const cdp = await context.newCDPSession(page);
console.log(await cdp.send('Page.getLayoutMetrics'));
await cdp.detach();

// DO NOT call page.setViewportSize() — it resizes the frames the terminal receives (§7.1).

// close() only DISCONNECTS for connectOverCDP; the engine keeps running. Verified.
await browser.close();
```

---

## 9. Recommended engine/CLI changes — described, not made

Per the ownership rule I did not touch `apps/engine/src/main.js` or `apps/cli/src/main.rs`.

**9.1 Engine (`apps/engine/src/main.js`).** Parse an opt-in `--bg-cdp-port=<n>` alongside the
existing `--bg-*` flags near line 20, and when present call
`app.commandLine.appendSwitch('remote-debugging-port', String(port))` **before**
`app.whenReady()` at line 288 (docs require it before `ready`). Default off. The header
comment at lines 15-16 needs amending, since the claim "it opens no listening port of its
own" stops being true whenever the flag is set.

Passing the switch through the CLI spawn instead (case D, verified) works equally well and
requires no engine edit at all — the commander may prefer that, as it keeps CDP entirely
outside the engine source.

**9.2 CLI (`apps/cli/src/main.rs:402-411`).** Add `--remote-debugging-port=0` plus
`--user-data-dir=<dir>` behind a `--devtools` / `BLACKGLASS_CDP` opt-in, then read the port
from `<dir>/DevToolsActivePort`. Verified working:

```
$ electron --remote-debugging-port=0 --user-data-dir=./ud ./osr-app.js
$ cat ./ud/DevToolsActivePort
56549
/devtools/browser/db8e5d56-78de-4525-b7c0-cdc85ef90637
$ curl -s http://127.0.0.1:56549/json/version | grep Browser
   "Browser": "Chrome/150.0.7871.129",
```

Port 0 is strongly preferred over a fixed port, for two reasons. First, `stdout`/`stderr` are
`Stdio::null()` at lines 409-410, so the `DevTools listening on ws://…` banner is discarded —
the file is the only way to recover an ephemeral port. Second, fixed ports collide: **during
this very mission my first run failed with `bind() failed: Address already in use (48)` /
`Cannot start http server for devtools` because another agent's Electron already held 9333**,
and my `curl` silently received *their* browser's `/json/version` (`cdptest/1.0.0` in the
User-Agent gave it away). A fixed default port would let one BlackGlass session hijack
another's automation. The `DevToolsActivePort` file also gives the CLI a reliable readiness
signal.

**9.3** Clear `ELECTRON_RUN_AS_NODE` from the spawn environment (§5).

---

## 10. Verified vs assumed

**Verified on this machine, this Electron, this Playwright:**

- `connectOverCDP` connects to Electron 43.2.0 in 242 ms and drives it.
- `/json/version` returns `Browser: Chrome/150.0.7871.129`; no rewrite proxy needed.
- Playwright reads only `webSocketDebuggerUrl`; both `/json/version` and `/json/version/` 200.
- An offscreen `show:false` window is a `type: "page"` target in `/json/list`.
- `goto`, `click`, `fill`, `evaluate`, locators, `title`, `screenshot`, `newCDPSession` all work on OSR.
- `newContext()` and `newPage()` fail with the quoted protocol errors.
- `browser.close()` disconnects and leaves the engine alive and painting.
- `--remote-debugging-port` works before-script, after-script, as port 0, and via `appendSwitch`.
- `ELECTRON_RUN_AS_NODE=1` reproduces #39008's `bad option:` error verbatim.
- `Host: evil.example` → HTTP 500 (rebinding guard); `Host: 127.0.0.1` → 200.
- `page.setViewportSize()` changes emitted OSR frame dimensions.
- No paint-rate degradation while attached.
- `DevToolsActivePort` discovery works with port 0.

**Assumed / not verified:**

- Behaviour against a **packaged** BlackGlass app. Everything here used unpackaged
  `node_modules/.bin/electron`. Electron fuses (notably `EnableNodeCliInspectArguments`) can
  change CLI-argument handling in packaged builds; #39008 may yet bite there. Re-test at
  packaging time (relevant to B10).
- `--remote-debugging-pipe` being unusable with `connectOverCDP` — inferred from the docs'
  `endpointURL` contract (`http://` or `ws://` only), not separately tested.
- Multi-tab behaviour. I tested one page target because the engine creates one window. How
  `context.pages()` enumerates several OSR windows is untested — likely fine, but it
  interacts with B04's tab lifecycle and should be checked there.
- Linux/Windows. macOS 26.1 / Apple M4 only.
- Playwright's own test runner (`@playwright/test`) integration; I used the raw
  `playwright-core` API. The runner adds fixtures that assume it can create contexts and
  pages, which Electron refuses — expect to need a custom fixture.

**Not reproduced:** the blanket claim that Electron 30+ rejects `--remote-debugging-port` on
the command line. False for 43.2.0 unpackaged; the real trigger is `ELECTRON_RUN_AS_NODE`.

---

## 11. Reproduction

```bash
cd /private/tmp/claude-501/-Users-adeebbashir/.../scratchpad/e03
npm install playwright-core
ELECTRON=/Users/adeebbashir/projects/blackglass/apps/engine/node_modules/.bin/electron
"$ELECTRON" --remote-debugging-port=9416 ./osr-app.js &   # offscreen, same webPreferences as the engine
curl -s http://127.0.0.1:9416/json/version
curl -s http://127.0.0.1:9416/json/list
node ./test-connect.js http://127.0.0.1:9416               # -> 16 pass / 2 fail
```

Scratchpad files: `osr-app.js` (OSR stand-in), `osr-metrics.js` / `osr-css.js` (paint-rate
instrumentation), `test-connect.js` (capability matrix), `probe.js` (viewport hazard),
`probe2.js` (frame-rate under load).

**Sources:** [connectOverCDP docs](https://playwright.dev/docs/api/class-browsertype) ·
[Playwright Electron docs](https://playwright.dev/docs/api/class-electron) ·
[chromium.ts source](https://raw.githubusercontent.com/microsoft/playwright/main/packages/playwright-core/src/server/chromium/chromium.ts) ·
[Playwright issue #39008](https://github.com/microsoft/playwright/issues/39008) ·
[Electron command-line switches](https://www.electronjs.org/docs/latest/api/command-line-switches)
