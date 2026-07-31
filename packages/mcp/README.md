# @blackglass/mcp

An MCP server that lets an AI agent drive the BlackGlass terminal browser: a real
Chromium 150, offscreen-rendered, that a human can be watching live in their terminal at
the same time.

stdio transport, newline-delimited JSON-RPC, **zero dependencies**.

```
   MCP client (Claude Code, ...)
        | stdio, JSON-RPC
   [ packages/mcp ]  <-- this package
        |                          \
        | unix socket, 0600         \  CDP (page semantics: what is on the page, where)
        | ACTIONS + frames           \
   [ apps/engine/src/main.js ] --> Chromium (sandboxed) offscreen rendering
        |
   [ blackglass CLI ] --> the same page, drawn as pixels in a terminal
```

## Two channels, on purpose

**Actions go over the engine's own unix socket**, as the same `input` commands the Rust
terminal core sends when a human types. An agent's click and a human's click are the same
event by the time they reach the page — one input pipeline, one set of bugs, and shared
control becomes possible rather than a second implementation to keep in sync.

**Observation goes over CDP**, read-only: the computed accessibility tree and, for the one
element an action targets, its box model.

## The model reads text, not pixels

`browser_snapshot` returns the page as a compact tree in which every actionable element
carries a `[ref=eN]` handle:

```
- RootWebArea "Start" [ref=e1]
  - heading "Live Test" [level=1]
    - text: "Live Test"
  - button "Sign in" [ref=e2]
  - textbox "Search terms" [ref=e3]
  - checkbox "Remember me" [checked=true] [ref=e4]
  - link "Read the docs" [ref=e5]
```

The model then calls `browser_click { element: "Sign in button", ref: "e2" }`. No
coordinate guessing, no vision tokens. For the page above the snapshot is 1442 bytes; the
project's own earlier analysis put a typical accessibility diff at ~2 KB against ~350 KB
for a frame (`artifacts/swarm/A03-user-journeys.md`). Screenshots remain available for the
questions only pixels can answer — did the chart render, did the animation run.

### Prior art

The pattern of "accessibility snapshot + opaque element refs, with a human-readable
description alongside each ref" was popularised by
[Playwright MCP](https://github.com/microsoft/playwright-mcp) (Apache-2.0, Copyright (c)
Microsoft Corporation). **No code from that project is used here.** This is an independent
implementation, and it differs in ways that matter:

| | Playwright MCP | this server |
|---|---|---|
| Snapshot source | Playwright's ARIA snapshot | Chromium's computed AX tree over CDP |
| Coordinates | resolved by the driver | resolved lazily, only for the targeted element |
| Stale refs | invalidated by navigation | **epoch-tagged and refused**, never re-resolved |
| Page text | returned as-is | wrapped in an untrusted-content envelope |
| Input | Playwright's input layer | the browser's own terminal input path |

The `element` description parameter is kept, and given a second job: if it shares no
vocabulary with the target's accessible name, the tool result says so. That catches a model
acting on a ref it has lost track of, and it catches a model that has been talked into
clicking something by text on the page.

## Tools

| Tool | Purpose |
|---|---|
| `browser_navigate` | Open a URL, starting the browser if needed |
| `browser_snapshot` | Accessibility tree with `[ref=eN]` handles |
| `browser_find` | Locate elements by accessible name |
| `browser_click` | Click by ref |
| `browser_type` | Focus by ref and type, optionally clear/submit |
| `browser_press_key` | Enter, Tab, Escape, arrows, function keys, characters |
| `browser_scroll` | Scroll the viewport |
| `browser_click_xy` | Raw coordinate click — canvas/video escape hatch |
| `browser_screenshot` | PNG of the exact frame the terminal is drawing |
| `browser_navigate_back` / `_forward` / `browser_reload` | History |
| `browser_resize` | Match the viewport to the terminal geometry |
| `browser_wait_for` | Wait for text, or for a duration |
| `browser_status` | URL, title, viewport, engine versions, CDP state |
| `browser_close` | Shut the Chromium tree down |

## Install

Nothing to install — no dependencies. It needs Node >= 22 (for the global `WebSocket`) and
the engine at `apps/engine` with its `node_modules` present.

```jsonc
{
  "mcpServers": {
    "blackglass": {
      "command": "node",
      "args": ["/absolute/path/to/blackglass/packages/mcp/index.js"]
    }
  }
}
```

### Environment

| Variable | Default | Meaning |
|---|---|---|
| `BLACKGLASS_ENGINE` | `../../apps/engine` | Engine directory |
| `BLACKGLASS_MCP_WIDTH` / `_HEIGHT` | 1280 / 800 | Initial viewport |
| `BLACKGLASS_MCP_CDP` | `1` | `0` disables CDP — coordinate-only mode |
| `BLACKGLASS_MCP_PROFILE` | throwaway temp dir | Persistent browser profile |
| `BLACKGLASS_MCP_LOG` | — | Mirror stderr diagnostics to a file |
| `BLACKGLASS_MCP_AUDIT` | `$TMPDIR/blackglass-mcp-audit.jsonl` | Action provenance log |

Every action appends a JSONL record (timestamp, actor, tool, target role and name,
resolved coordinates) to the audit log, so an agent run can be replayed and a disputed
click attributed — required by `A03-user-journeys.md`.

## Protocol support

Dual-era, decided per request:

* **2026-07-28** (modern): `server/discover`, per-request `_meta` version, `resultType`,
  `UnsupportedProtocolVersionError` (`-32022`) listing supported versions.
* **2025-11-25 / 2025-06-18 / 2025-03-26 / 2024-11-05** (legacy): the `initialize`
  handshake, echoing the client's version when supported.

A client using either era gets the same tool set. `stdout` carries JSON-RPC and nothing
else; diagnostics go to `stderr`.

## Security

**The engine's stated posture is that it opens no listening port.** Enabling CDP changes
that: Chromium starts a DevTools listener. Measured on this machine:

```
$ lsof -nP -iTCP:56402 -sTCP:LISTEN
Electron 671 adeebbashir 33u IPv4 ... TCP 127.0.0.1:56402 (LISTEN)
```

Loopback only, so not reachable off-host — but **unauthenticated**: any process running as
this user can attach and drive the browser. Current mitigations: ephemeral port read
race-free from `DevToolsActivePort`, throwaway profile directory, and `BLACKGLASS_MCP_CDP=0`
to switch it off entirely (semantic tools then return an actionable error and the
coordinate tools keep working).

### Removing the DevTools port

This is fixable, and the fix belongs in `apps/engine/src/main.js`, which this package does
not own. Electron can speak CDP **in-process**, with no socket of any kind, via
`webContents.debugger`. Verified working on Electron 43.2.0 / Chrome 150.0.7871.129:

```js
win.webContents.debugger.attach('1.3');
await win.webContents.debugger.sendCommand('Accessibility.enable');
const ax = await win.webContents.debugger.sendCommand('Accessibility.getFullAXTree', { depth: -1 });
// -> 8 nodes, roles and names identical to the port-based path
```

Proxying that through the existing 0600 socket as one new command type would let this
package drop `--remote-debugging-port` completely and restore the original posture. See the
E04 report for the proposed command shape.

## Known limitations

* **Resize lag (engine-side).** After a `resize`, the CSS viewport changes immediately but
  the engine does not reliably emit a frame at the new geometry until the page repaints
  again. Measured: requested 900x700, page reported `innerWidth 900`, latest frame still
  `1280x800 seq=0`. This package detects the mismatch and warns in `browser_screenshot`,
  `browser_resize` and `browser_click_xy` rather than letting an agent reason about stale
  pixels — but the underlying repaint belongs to `main.js`.
* **One tab.** The engine hosts a single `BrowserWindow`; popups are reported and denied.
* **Refs are per-snapshot.** Any real navigation invalidates them by design.
* **No file uploads or downloads**, no cookie/storage tools yet.

## Tests

```bash
npm test        # protocol conformance -- no browser, CI-safe
npm run test:live   # end-to-end against real Chromium
```

`test/handshake.js` covers both protocol eras, error codes, and stdout discipline without
launching a browser. `test/live-browser.js` launches Chromium and asserts against what the
page actually did — a click is proven by the page's own `document.title` changing, typing
by the value that comes back in the next snapshot, and stale-ref rejection by a ref from a
previous document being refused.

Latest run on macOS 26.1 / Apple M4, Electron 43.2.0 / Chrome 150.0.7871.129:
**24/24 protocol checks, 28/28 live checks.**
