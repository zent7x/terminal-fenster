# E10 — Integrating Coding Agents with Terminal-Fenster

**Mission:** concise integration docs for coding agents (Claude Code, Cursor, generic MCP
clients) — setup, tools, permission boundaries, examples.
**Audience:** a developer wiring Terminal-Fenster into an AI coding agent.
**Author:** swarm agent E10. **Date:** 2026-07-31.
**Files written:** this file only. No repo source was modified.
**Ground truth:** everything below is read from `packages/mcp/` and verified on this machine
(see §11); the design rationale traces to `artifacts/swarm/A03-user-journeys.md`.

---

## 0. What an agent gets

Terminal-Fenster ships an **MCP server** (`packages/mcp/index.js`, package `@terminal-fenster/mcp`) that
lets an AI agent drive the same terminal browser a human is watching. It speaks
Model Context Protocol over **stdio** (newline-delimited JSON-RPC), has **zero npm
dependencies**, and exposes **16 tools** (`browser_navigate`, `browser_snapshot`,
`browser_click`, …; full list in §6).

Two design facts shape every integration:

- **Observe cheap, pixels expensive.** The agent reads a page as a compact accessibility
  tree (~2 KB) with `[ref=eN]` handles, and only asks for a PNG screenshot (~300 KB) when
  appearance actually matters. Prefer `browser_snapshot` over `browser_screenshot`.
- **One input pipeline, shared control.** Agent actions travel the *same* 0600 unix socket
  and the *same* `input` command path as a human's keystrokes from the terminal
  (`packages/mcp/lib/engine.js:1-12`). The page cannot tell agent from human, and both can
  drive the session at once.

---

## 1. Prerequisites

| Requirement | Detail | Check |
|---|---|---|
| Node.js ≥ 22 | `packages/mcp/package.json` `engines`. Verified on `v24.11.1`. | `node --version` |
| Engine deps (once) | Electron 43.2.0 must be installed under `apps/engine/`. This is a one-time ~200 MB download. | `ls apps/engine/node_modules/.bin/electron` |
| A graphics-capable terminal (for the human view) | Kitty-graphics (Ghostty) or Unicode fallback. Not needed for headless agent use. | `terminal-fenster doctor` |

Install the engine dependencies once, with your explicit action — nothing is installed
behind your back:

```bash
cd $REPO/apps/engine
npm install          # pulls Electron 43.2.0 (~200 MB) into ./node_modules
```

> Disk note: this machine is at ~98% (~9 GiB free). Electron is already installed here, so
> `npm install` is a no-op. Do **not** attempt a Chromium/CEF-from-source build.

The MCP server itself needs no install step — it is a single dependency-free Node script.

---

## 2. Verify before you wire it up

Two checks, both runnable from `packages/mcp/`:

```bash
cd $REPO/packages/mcp

npm test          # protocol conformance: starts NO browser, runs anywhere (CI-safe)
npm run test:live # end-to-end: spawns the real Electron engine and drives a page
```

`npm test` exercises the stdio transport and JSON-RPC dispatch for both MCP protocol eras.
Expected tail: `25/25 checks passed`, and a printed tool roster of all 16 tool names.
(Verified in this session — see §11.)

`npm run test:live` needs to spawn Chromium, which fails under a restricted sandbox with
`bootstrap_look_up … Permission denied`; run it in a normal shell.

---

## 3. How the server is launched

The server is a stdio subprocess. The canonical launch command is:

```bash
node $REPO/packages/mcp/index.js
```

The package also declares a `terminal-fenster-mcp` bin, so inside the repo you can run
`node packages/mcp/index.js` or, after `npm link`/install, `terminal-fenster-mcp`. Note: the Rust
`terminal-fenster` CLI today exposes only `doctor` and `open` (`apps/cli/src/main.rs:54-66`) —
there is **no** `terminal-fenster mcp` subcommand yet, so point your client at `index.js`
directly.

An MCP client starts this command, writes JSON-RPC requests to its stdin, and reads
responses from its stdout. stdout is **pure JSON-RPC** — all logs go to stderr — so the
client must never parse stderr as protocol.

---

## 4. Wiring it into each client

Guiding principle for this section, per the mission: **no silent global config.** Every
step below is explicit, shows exactly what file or entry is created and where, prefers
project-local scope, and states how to undo it.

### 4.1 Claude Code

Two equivalent, fully reversible options. Both were checked against the installed
`claude mcp` CLI (§11).

**Option A — one command, project scope (recommended, shareable, consent-gated):**

```bash
cd $REPO
claude mcp add terminal-fenster \
  --scope project \
  -- node $REPO/packages/mcp/index.js
```

`--scope project` writes a `.mcp.json` file in the repo root (see Option B for its exact
contents). Because it lives in the project, a teammate who checks it out sees the server as
**"⏸ Pending approval"** and is *not connected to it* until they explicitly approve it in
`/mcp` — that pending-approval gate is the built-in consent mechanism. `--scope local`
(the default) keeps it private to you for this project; `--scope user` would make it global
across all your projects — only choose that deliberately.

Add environment variables with `-e`, e.g. `-e TERMINAL_FENSTER_MCP_WIDTH=1600 -e TERMINAL_FENSTER_MCP_HEIGHT=1000`.

**Option B — write the file yourself (fully inspectable):**

Create `$REPO/.mcp.json`:

```json
{
  "mcpServers": {
    "terminal-fenster": {
      "command": "node",
      "args": ["$REPO/packages/mcp/index.js"],
      "env": {}
    }
  }
}
```

**Verify / inspect / undo:**

```bash
claude mcp list                       # shows status; .mcp.json entries read "Pending approval" until approved
claude mcp get terminal-fenster             # full details of the configured server
claude mcp remove terminal-fenster --scope project   # removes it; or just delete the .mcp.json entry
```

Nothing is written to your global `~/.claude.json` unless you pass `--scope user`
on purpose.

### 4.2 Cursor

Cursor reads MCP servers from a JSON file. Use the **project-local** path so the config
lives in the repo and is trivially removable — do not edit the global `~/.cursor/mcp.json`
unless you intend a machine-wide server.

Create `$REPO/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "terminal-fenster": {
      "command": "node",
      "args": ["$REPO/packages/mcp/index.js"],
      "env": {}
    }
  }
}
```

Then enable it in Cursor's Settings → MCP. **Undo:** delete `.cursor/mcp.json` (or that
one block) and toggle it off in Settings. The exact settings location can move between
Cursor releases — confirm against your version's MCP docs; the `command`/`args`/`env`
shape is stable and identical to Claude Code's.

### 4.3 Generic MCP client / raw stdio

Any MCP client that speaks stdio can launch `node …/packages/mcp/index.js` and talk to it.
The server answers **both** protocol eras and decides per request
(`packages/mcp/lib/rpc.js:1-31`):

- **Modern (2026-07-28):** stateless `server/discover`, protocol version in each request's
  `_meta["io.modelcontextprotocol/protocolVersion"]`.
- **Legacy (2024-11-05 → 2025-11-25):** classic `initialize` → `notifications/initialized`
  → `tools/list` handshake.

Minimal legacy handshake you can pipe straight into the process (one JSON object per line):

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"my-agent","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_navigate","arguments":{"url":"https://example.com"}}}
```

Framing rules the client must honor: newline-delimited, one JSON object per line, no
embedded newlines; unknown protocol version → error `-32022` (with a `supported` list);
unknown method → `-32601`. Closing the server's stdin is the graceful-shutdown signal and
takes the Chromium process tree down with it.

---

## 5. The agent workflow

The server's own `instructions` field states the loop:

> `browser_navigate → browser_snapshot → act on [ref=eN] handles → re-snapshot.`

A worked example (tool name + arguments):

1. `browser_navigate` `{ "url": "https://example.com" }` — starts the engine if needed,
   waits for load, returns a one-line status.
2. `browser_snapshot` `{}` — returns the accessibility tree; each actionable element carries
   `[ref=eN]`. The text is wrapped in an `<untrusted-page-content>` fence (see §7).
3. `browser_type` `{ "element": "Search box", "ref": "e7", "text": "hello", "submit": true }`
   — focuses the field, types real key events, presses Enter, waits for the load.
4. `browser_snapshot` `{}` — re-read; refs from step 2 are now stale and will be rejected.
5. `browser_click` `{ "element": "First result link", "ref": "e3" }`.

Two rules the tools enforce so the agent fails safely rather than acting on stale beliefs:

- **Refs expire on navigation.** Each snapshot mints `e1..eN` against the current document
  and carries a navigation epoch. A ref from an earlier epoch is **rejected loudly**, never
  silently remapped to whatever now sits at that index (`packages/mcp/index.js:127-138`).
  If a tool says a ref is stale, snapshot again.
- **Below-the-fold isn't in the tree.** `browser_scroll` then re-snapshot to see newly
  visible elements.

Use `browser_click_xy` (raw coordinates) only for canvas/video/maps with no accessibility
representation, and `browser_screenshot` only when appearance is the question (layout,
charts, "did the animation run").

---

## 6. Tool reference

All 16 tools. Required args in **bold**; the rest are optional.

| Tool | Purpose | Arguments |
|---|---|---|
| `browser_navigate` | Open a URL; starts the browser if down; waits for load | **`url`** |
| `browser_snapshot` | Accessibility tree with `[ref=eN]` handles (preferred read) | `maxLines` (default 1200) |
| `browser_find` | Find actionable elements whose name contains text | **`text`** |
| `browser_click` | Click an element by ref (through the human input path) | **`element`**, **`ref`**, `doubleClick`, `button` |
| `browser_type` | Focus a field by ref and type real key events | **`element`**, **`ref`**, **`text`**, `clear`, `submit` |
| `browser_press_key` | Press one key (Enter, Tab, arrows, Fn, char) + modifiers | **`key`**, `ctrl`, `alt`, `shift`, `meta` |
| `browser_scroll` | Scroll the page to reveal off-screen content | `direction` (default down), `amount` (default 400) |
| `browser_click_xy` | Click a raw viewport coordinate (escape hatch) | **`element`**, **`x`**, **`y`** |
| `browser_screenshot` | PNG of the current frame (use only for appearance) | `maxDimension` (default 1024, 0 = full) |
| `browser_navigate_back` | Back one history entry | — |
| `browser_navigate_forward` | Forward one history entry | — |
| `browser_reload` | Reload the page | — |
| `browser_resize` | Resize the viewport (match terminal geometry) | **`width`**, **`height`** |
| `browser_wait_for` | Wait for text to appear, or a fixed time | `text`, `timeMs` (default 5000) |
| `browser_status` | URL, title, load state, viewport, engine versions, CDP availability, audit path | — |
| `browser_close` | Shut the browser down and release the Chromium tree | — |

`element` on the action tools is a human-readable description ("Sign in button"). It is
logged for audit **and** cross-checked against the resolved ref: a mismatch returns a
`WARNING` in the tool output so a ref mix-up surfaces instead of mis-clicking.

Tool-execution failures come back as a normal result with `isError: true` and a readable
message (e.g. "No browser session. Call browser_navigate first."), so the model can correct
itself rather than crashing the session.

---

## 7. Permission boundaries and the safety model

This is the part to read before granting an agent access.

**Actions vs. observation are separated.** Every *action* (click, type, scroll, key) is sent
over the engine's **0600 unix socket** as an `input` command — the identical path a human's
keystrokes take. Page *observation* (the accessibility tree, box models) comes over a
separate, **read-only** CDP channel. CDP is never used to act
(`packages/mcp/lib/engine.js:1-12`).

**CDP is an unauthenticated loopback listener — know what you're enabling.** Turning on page
semantics starts a Chromium DevTools server on `127.0.0.1` at an ephemeral port. It is
loopback-only (not reachable off-host) but **unauthenticated**: any process running as the
same local user can attach and drive the browser (`packages/mcp/lib/engine.js:14-22`).
Controls:

- `TERMINAL_FENSTER_MCP_CDP=0` disables CDP entirely. Semantic tools then degrade to
  coordinate-only — `browser_snapshot`/`browser_find`/`browser_click` by ref stop working;
  `browser_click_xy` and `browser_screenshot` still do.
- The DevTools profile is a **throwaway `--user-data-dir`** unless you set
  `TERMINAL_FENSTER_MCP_PROFILE`, so by default there are no persistent cookies or credentials in
  the automated browser.

**Every action is audited.** All tool calls that touch the page are appended as JSONL to the
audit log — actor, method, params, target role/name, navigation epoch, timestamp — so an
agent run is replayable and any disputed click is attributable
(`packages/mcp/index.js:37-45`). Default path: `${TMPDIR}/terminal-fenster-mcp-audit.jsonl`;
override with `TERMINAL_FENSTER_MCP_AUDIT`. `browser_status` prints the active path.

**Page text is untrusted input, not instructions.** Snapshot and find output is wrapped in
an `<untrusted-page-content>` … fence that explicitly labels it as data and tells the model
to ignore any embedded commands, prompts, or role changes (`packages/mcp/index.js:47-61`).
Agents and client prompts should treat fenced content as attacker-controlled and never
concatenate it into an instruction.

**Filesystem posture.** The per-session socket dir is `0700` and the socket itself `0600`,
so no other local user can reach it (`packages/mcp/lib/engine.js:80-101`).

**A human may be watching.** The server's instructions remind the model that a person can be
viewing the same live session; match `browser_resize` to the terminal geometry when sharing
a screen.

---

## 8. Environment variables

All optional. Read at server start (`packages/mcp/index.js`, `lib/engine.js`).

| Variable | Default | Effect |
|---|---|---|
| `TERMINAL_FENSTER_MCP_WIDTH` | `1280` | Initial viewport width (px). |
| `TERMINAL_FENSTER_MCP_HEIGHT` | `800` | Initial viewport height (px). |
| `TERMINAL_FENSTER_MCP_CDP` | enabled | Set to `0` to disable CDP → coordinate-only mode. |
| `TERMINAL_FENSTER_MCP_PROFILE` | throwaway dir | Persistent Chromium profile dir (persists cookies/creds — use deliberately). |
| `TERMINAL_FENSTER_MCP_AUDIT` | `${TMPDIR}/terminal-fenster-mcp-audit.jsonl` | Action audit log path. |
| `TERMINAL_FENSTER_MCP_LOG` | none | Extra log file (stderr always gets logs regardless). |
| `TERMINAL_FENSTER_ENGINE` | `../../../apps/engine` | Engine directory (must contain `node_modules/.bin/electron` and `src/main.js`). |

---

## 9. Troubleshooting

- **"Could not find the Terminal-Fenster engine."** The server looked in `apps/engine` relative to
  itself and found no `node_modules/.bin/electron`. Run `npm install` in `apps/engine`, or
  point `TERMINAL_FENSTER_ENGINE` at the engine directory.
- **"Page semantics unavailable (no CDP)."** CDP is off (`TERMINAL_FENSTER_MCP_CDP=0`) or failed to
  attach. Ref-based tools won't work; use `browser_click_xy`/`browser_screenshot`, or
  re-enable CDP. `browser_status` shows the CDP state and any attach error.
- **`bootstrap_look_up … Permission denied` when a frame should appear.** Chromium child
  processes can't launch under a restricted agent sandbox. Run the server (and
  `npm run test:live`) in a normal shell with the sandbox disabled.
- **Screenshot geometry looks stale after a resize.** The engine doesn't always emit a frame
  at the new size immediately; the tools surface this as an explicit `WARNING` and tell you
  the frame predates the resize. Re-snapshot, or act by ref instead of by coordinate.
- **Client parses garbage.** It's reading stderr as protocol. Only stdout is JSON-RPC.

---

## 10. Copy-paste quickstart (Claude Code, project scope)

```bash
cd $REPO
[ -x apps/engine/node_modules/.bin/electron ] || (cd apps/engine && npm install)
node packages/mcp/index.js < /dev/null   # smoke-check it starts (Ctrl-C to exit)
claude mcp add terminal-fenster --scope project -- node "$PWD/packages/mcp/index.js"
claude mcp get terminal-fenster                # confirm; approve via /mcp inside Claude Code
# undo any time:  claude mcp remove terminal-fenster --scope project
```

---

## 11. Verified vs. unverified

**Verified in this session (2026-07-31, this machine):**

- `node --version` → `v24.11.1` (≥ 22).
- `apps/engine/node_modules/.bin/electron` present.
- `npm test` in `packages/mcp` → **25/25 checks passed**; tool roster printed = the 16 tools
  in §6; both legacy and modern handshakes answered; `-32022`/`-32601` error paths correct.
- `claude mcp add/remove --help` → confirmed `-s, --scope <local|user|project>`, `-e --env`,
  stdio default, and that `.mcp.json` servers show as "⏸ Pending approval" until approved.
- CLI subcommands are only `doctor` and `open` (`apps/cli/src/main.rs:54-66`); no `mcp`
  subcommand exists.

**Not verified by me here (blocked or out of scope):**

- The **live MCP → engine → page** round trip (`npm run test:live`): needs to spawn Chromium,
  which the agent sandbox blocks (`bootstrap_look_up … Permission denied`). The underlying
  engine end-to-end and the CDP/Playwright attach were verified by other agents
  (mission brief; `artifacts/swarm/E03-playwright-integration.md`), not re-run here.
- Cursor's exact settings-file location across versions — the `command`/`args`/`env` shape is
  stable; the path can move, so confirm against your Cursor build.

---

## 12. One recommendation for the commander

Ship a thin `terminal-fenster mcp` subcommand in `apps/cli` that just `exec`s
`node packages/mcp/index.js` (resolving the path relative to the installed binary). Today
every client config must hard-code an absolute path to `index.js`, which is brittle across
machines and checkouts; a stable `terminal-fenster mcp` entry point makes all three integrations
in §4 a single portable line (`claude mcp add terminal-fenster -- terminal-fenster mcp`) and removes the
only machine-specific detail from these docs.
