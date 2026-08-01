# @terminal-fenster/mcp

An MCP server that lets an AI agent drive an isolated Terminal-Fenster engine: real Chromium 150,
offscreen-rendered through the same frame and input protocols as the terminal browser.

stdio transport, newline-delimited JSON-RPC, **zero dependencies**.

```
   MCP client (Claude Code, ...)
        | stdio, JSON-RPC
   [ packages/mcp ]  <-- this package
        | unix socket, 0600
        | ACTIONS + frames + in-process CDP proxy
   [ apps/engine/src/main.js ] --> Chromium (sandboxed) offscreen rendering
```

## Two logical channels, one private transport

**Actions go over the engine's own unix socket**, as the same `input` commands the Rust
terminal core sends when a human types. An agent click and an interactive-terminal click are
the same event by the time they reach Chromium — one input implementation and one set of bugs.
This release starts a separate agent session; it does not attach to an already-running CLI.

**Observation goes over CDP**, read-only: the computed accessibility tree and, for the one
element an action targets, its box model. Electron's in-process debugger executes those CDP
commands inside the engine; requests and results use the same private Unix socket as actions.

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

Editable AX values are always rendered as `<redacted:N chars>`. Chromium does not reliably mark
password fields in its accessibility tree, so Terminal-Fenster treats every textbox/contenteditable
value as sensitive rather than risking a password appearing in model context.

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
| `browser_screenshot` | PNG reconstructed from the engine's exact damage-frame stream |
| `browser_navigate_back` / `_forward` / `browser_reload` | History |
| `browser_resize` | Match the viewport to the terminal geometry |
| `browser_wait_for` | Wait for text, or for a duration |
| `browser_status` | URL, title, viewport, engine versions, CDP state |
| `browser_close` | Shut the Chromium tree down |

## Install

After `./install.sh`, run:

```bash
terminal-fenster mcp-config --cursor   # or --claude, or --json
```

Or wire any MCP client to stdio:

```jsonc
{
  "mcpServers": {
    "terminal-fenster": {
      "command": "terminal-fenster",
      "args": ["mcp"]
    }
  }
}
```

Nothing else to install — zero npm dependencies. Node ≥ 22 and a materialized Electron
runtime under `apps/engine` (or the installed `engine/` directory) are required.

### Environment

| Variable | Default | Meaning |
|---|---|---|
| `TERMINAL_FENSTER_ENGINE` | `../../apps/engine` | Engine directory |
| `TERMINAL_FENSTER_MCP_WIDTH` / `_HEIGHT` | 1280 / 800 | Initial viewport |
| `TERMINAL_FENSTER_MCP_CDP` | `1` | `0` disables in-process CDP — coordinate-only mode |
| `TERMINAL_FENSTER_MCP_PROFILE` | throwaway temp dir | Persistent browser profile |
| `TERMINAL_FENSTER_MCP_LOG` | — | Mirror stderr diagnostics to a file |
| `TERMINAL_FENSTER_MCP_AUDIT` | per-user 0700 state directory | Action provenance log |

Every action appends a JSONL record (timestamp, actor, tool, target role and name,
resolved coordinates) to the audit log, so an agent run can be replayed and a disputed
click attributed — required by `A03-user-journeys.md`. The file is mode 0600, refuses symlinks,
redacts URL values/fragments/local paths, and records printable keys by class rather than value.

## Protocol support

Dual-era, decided per request:

* **2026-07-28** (modern): `server/discover`, per-request `_meta` version, `resultType`,
  `UnsupportedProtocolVersionError` (`-32022`) listing supported versions.
* **2025-11-25 / 2025-06-18 / 2025-03-26 / 2024-11-05** (legacy): the `initialize`
  handshake, echoing the client's version when supported.

A client using either era gets the same tool set. `stdout` carries JSON-RPC and nothing
else; diagnostics go to `stderr`.

## Security

The engine opens no listening port. CDP runs through `webContents.debugger` in-process and
is proxied as request/response messages over the existing 0600 socket inside a 0700 temporary
directory. There is no `--remote-debugging-port`, `DevToolsActivePort`, or unauthenticated
localhost endpoint for another process to attach to. `TERMINAL_FENSTER_MCP_CDP=0` can still disable
semantic inspection entirely; coordinate tools and screenshots continue to work.

Actions are appended to the configured JSONL audit log. Page-derived accessibility text is
fenced as untrusted content, snapshot refs expire across navigation, and descriptions are
cross-checked against the chosen element's accessible name before a click is sent.
Agent sessions receive their initial URL only after the private socket handshake (never in `ps`),
deny local-file/blob/custom-scheme navigation, and cap inline `data:` URLs at 64 KiB.

## Known limitations

* **One tab.** The engine hosts a single `BrowserWindow`; popups are reported and denied.
* **Separate session.** MCP does not yet attach to an already-running interactive CLI.
* **Refs are per-snapshot.** Any real navigation invalidates them by design.
* **No file uploads or downloads**, no cookie/storage tools yet.

## Tests

```bash
npm test            # damage compositor + protocol conformance, CI-safe
npm run test:live   # end-to-end against real Chromium
```

`test/frame-compositor.test.js` proves dirty BGRA rectangles reconstruct the full screenshot
without wiping untouched pixels. `test/handshake.js` covers both protocol eras, error codes,
and stdout discipline without launching a browser. `test/live-browser.js` launches Chromium
and asserts against what the page actually did — a click is proven by the page's own
`document.title` changing, typing by the redacted value length in the next snapshot, resize by the
new screenshot geometry, and stale-ref rejection by refusing a ref from an older document.

Latest run on macOS 26.1 / Apple M4, Electron 43.2.0 / Chrome 150.0.7871.129:
**14/14 compositor/discovery/privacy tests, 24/24 protocol checks, 28/28 live checks.**
