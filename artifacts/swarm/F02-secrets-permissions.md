# F02 — Secrets, Permissions, and Redaction Policy

**Mission:** F02. Threat-test secrets and permissions across profile/cookie storage, auth tokens,
clipboard, downloads, file chooser, and agent access. Specify redaction rules for logs and traces.
Deliver a concrete policy.

**Owned output:** `artifacts/swarm/F02-secrets-permissions.md` (this file only). Every defect below
lives in commander-owned code (`apps/engine/src/main.js`, `apps/cli/src/main.rs`, `packages/mcp/`).
Per the swarm rules I **describe** each change with exact `file:line`, exact patch shape, and exact
test vector — I applied none of them. Probe sources live outside the repo, in the agent scratchpad
at `/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/f02/`,
and are reproduced verbatim in §10 so they can be re-run or promoted into the test suite by whoever
owns those files.

**Host:** macOS 26.1 (25B78), Apple M4 arm64. Electron 43.2.0 / Chromium 150. Date: 2026-07-31.

---

## 0. Executive summary

Twelve findings. **Eight were measured on this machine against the shipped code**, two are cited to
the primary-source documentation for the exact Electron version we run, and two are marked
`[UNVERIFIED]` with the command that would settle them.

The headline is not any single bug. It is that **BlackGlass currently has no secret-handling layer
at all** — not a weak one, not a partial one. There is no permission handler, no redactor, no
classification of what may reach a log, no scheme allowlist, and no isolation of the profile that
accumulates cookies. Every protection that exists today is one Chromium happens to provide by
default, and §3.1 shows that Chromium's default covers exactly one field type out of the six that
hold secrets.

| # | Severity | Finding | Evidence |
|---|---|---|---|
| **F02-01** | **CRITICAL** | The agent snapshot emits the **plaintext value of every editable field except `type=password`** into the model's context and the MCP response. Measured leaks: an OTP code, an API key, a card number, and a textarea. `snapshot.js:121-122` has no exclusion of any kind. | §3.1, **measured** |
| **F02-02** | **CRITICAL** | No permission handler exists, so **Electron auto-approves every permission request**. A page silently gets camera, microphone, geolocation, screen capture — and `clipboard-read`, which in a terminal browser means reading the clipboard of the shell the user is about to paste into. | §3.2, primary source + code |
| **F02-03** | **HIGH** | Cookies persist in the **shared, generic `~/Library/Application Support/Electron/` profile**, co-tenanted with every other unpackaged Electron app on the machine. Not designed — already happening: **10 cookies across 6 hosts** are sitting there now. | §3.3, **measured** |
| **F02-04** | **HIGH** | **The security opt-out makes storage worse.** Setting `BLACKGLASS_MCP_CDP=0` to close the unauthenticated DevTools port also drops `--user-data-dir`, silently relocating the agent's browsing from a throwaway profile into the shared persistent one. | §3.4, code |
| **F02-05** | **HIGH** | `BLACKGLASS_LOG` records **every URL and every page title verbatim**, in a file created **mode 0644**. URLs are where password-reset tokens, magic links, OAuth codes and session IDs live. | §3.5, **measured** |
| **F02-06** | **HIGH** | The MCP audit log defaults to a **predictable path** in `os.tmpdir()`, mode **0644**, containing **full navigation URLs**. A live 9,661-byte file already exists. On Linux that directory is world-readable and the name is guessable. | §3.6, **measured** |
| **F02-07** | **MEDIUM** | The target URL is passed as a **command-line argument** (`--bg-url=`), readable from `ps` by any process running as this user — and cross-user on Linux. | §3.7, **measured** |
| **F02-08** | **MEDIUM** | **The obvious fix for F02-01 is a trap.** `DOM.describeNode`, the API you need to *identify* a password field, returns that password's **plaintext** in its attribute list — handing back the one secret Chromium had masked. | §3.8, **measured** |
| **F02-09** | **MEDIUM** | Chromium's password masking preserves length exactly: a 17-character password renders as 17 bullets. The snapshot leaks password length to the model and to any log that captures it. | §3.9, **measured** |
| **F02-10** | **MEDIUM** | **No scheme allowlist anywhere.** `browser_navigate` and the CLI's `normalize_url` both accept `file://`, so an agent — or a prompt-injected agent — can read local files and exfiltrate them as snapshot text. | §3.10, code |
| **F02-11** | **MEDIUM** | With `BLACKGLASS_MCP_PROFILE` pointed at a real profile, the unauthenticated loopback CDP port becomes a **cookie-theft endpoint** for any same-uid process (`Network.getAllCookies`). | §3.11, code |
| **F02-12** | **LOW** | Log sinks perform **no control-character sanitization**. Raw event JSON is incidentally safe because `JSON.stringify` escapes `ESC`; the `start url=` record at `main.rs:257` is not, and nothing enforces the property. | §3.12, **measured** |

**Single most actionable recommendation** (expanded in §9): **fix F02-01 first, and fix it by
default-deny.** Change `packages/mcp/lib/snapshot.js:122` so that a value on an *editable* node is
never emitted verbatim — emit `value=<redacted:17 chars>` — and gate real values behind an explicit
per-call opt-in. It is a two-line change, it needs no CDP round trip, and it is the only finding
here where the secret leaves the user's machine entirely: snapshot text is transmitted to a
third-party model provider and retained in a conversation transcript. F02-02 (a nine-line
default-deny permission handler) is the immediate second, because it is the only finding that
reaches *outside* the browser into the user's camera, microphone, and clipboard. Everything else
in this document is recoverable; those two are not.

---

## 1. Evidence base

Every command in this section was run on the target machine during this mission. Nothing here is
recalled from training.

### 1.1 What was measured

| Claim | Command | Result |
|---|---|---|
| AX tree leaks non-password field values | `electron scratchpad/f02/axprobe.js` (§10.1) | 4 of 5 values plaintext; §3.1 |
| Password fields are masked, and only those | same | `"•••••••••••••••••"` |
| AX tree cannot identify a password field | `electron scratchpad/f02/axprobe2.js` (§10.2) | identical `properties` on both nodes |
| `DOM.describeNode` returns password plaintext | same | `["type","password","id","p","value","HUNTER2-CANARY-PW"]` |
| Electron auto-approves permissions by default | `curl raw.githubusercontent.com/electron/electron/v43.2.0/docs/tutorial/security.md` | line 283 |
| Cookies persist in the shared default profile | `sqlite3 ~/Library/Application\ Support/Electron/Cookies` | 10 rows, 6 distinct hosts |
| Log/audit files are created world-readable | `umask` + append-create probe | `022` → `-rw-r--r--` |
| MCP audit file exists with full URLs | `ls -l "$TMPDIR/blackglass-mcp-audit.jsonl"` | 9,661 bytes, `-rw-r--r--` |
| argv is visible in `ps` | `exec -a` + `ps -eo user,args` | full `--bg-url=…?token=…` visible |
| `JSON.stringify` escapes `ESC` | `node -e` (§3.12) | `contains_raw_ESC: false` |

### 1.2 What was read

All `file:line` references are to the tree as of this mission. Line counts:
`apps/cli/src/main.rs` 1039, `apps/engine/src/main.js` 309, `packages/mcp/index.js` 706,
`packages/mcp/lib/engine.js` 325, `packages/mcp/lib/snapshot.js` 206.

### 1.3 Relationship to sibling artifacts

This mission deliberately does **not** re-derive work already done. Where I overlap, I cite and
extend rather than restate, and I flag one place where I add a fact a sibling did not have.

| Artifact | Overlap | F02's delta |
|---|---|---|
| **A09** (threat model) | `T15` argv token exposure; `T20` password-field redaction rated H/C | A09 *modeled* T20. F02 **measured** it, and found the model was optimistic in one direction (passwords *are* masked) and badly pessimistic in another (**every other field is not**). A09's proposed `REDACT_SELECTOR` (A09:845) is necessary but not sufficient — §3.8 shows why the selector approach needs care. |
| **D05** (files/downloads/permissions) | Independently found the missing permission handler and the invisible-save-panel hang | D05 owns the **terminal UI** for prompts. F02 owns the **policy** that runs before any UI exists: the default-deny matrix in §5, and the clipboard-read chain D05 did not trace. I concur with D05's finding and do not restate its probe. |
| **B09** (profiles/data services) | Designed partitions, `persist:` semantics, `safeStorage`, quarantine xattr | B09 designed the profile system correctly but **did not catch that the current code already writes cookies to the shared generic `Electron` directory** (I grepped: no hit for `Application Support/Electron` or `getName` in B09). F02-03 is the live-state finding B09's design would fix. |
| **D04** (focus/clipboard) | Clipboard via terminal `OSC 52`; found the decoder leaks clipboard replies as keystrokes | D04 owns the **terminal side** of the clipboard. F02 owns the **Chromium side** — `clipboard-read` being auto-granted (§3.2). These are two independent paths to the same secret and both must be closed. |
| **E01 / E09** (CDP broker, devtools) | CDP exposure | F02 adds the profile-coupling consequence (F02-11) and the opt-out inversion (F02-04). |

**LICENSE note (swarm rule 4).** No third-party code is proposed for reuse in this document. The
repo still has **no `LICENSE` file at root** while `Cargo.toml:9` declares `MIT OR Apache-2.0` —
first noted by D05; re-confirmed here (`ls LICENSE*` → no matches). Unchanged, still worth fixing.

---

## 2. Asset inventory — what is actually secret here

A redaction policy is only as good as its inventory. This is every secret-bearing datum that
crosses a BlackGlass boundary today, with where it lives and which sinks can see it.

| # | Asset | Lives in | Sinks that can see it today |
|---|---|---|---|
| A1 | Session cookies, auth cookies | shared `Electron` profile (F02-03) | disk (persistent), CDP (F02-11) |
| A2 | Bearer/reset/OAuth tokens **in URLs** | `state.url`, `did-navigate` events | `BLACKGLASS_LOG` (F02-05), MCP audit (F02-06), `ps` (F02-07), status bar, model context |
| A3 | OTP / 2FA codes | page fields (`type=text`) | **model context** (F02-01) |
| A4 | API keys, recovery codes | page fields (`type=text`) | **model context** (F02-01) |
| A5 | Card numbers / PAN | page fields (`type=tel`/`text`) | **model context** (F02-01) |
| A6 | Passwords | page fields (`type=password`) | length only (F02-09); plaintext if F02-08 trap is hit |
| A7 | Free-text notes, message drafts | `<textarea>` | **model context** (F02-01) |
| A8 | System clipboard (SSH keys, tokens) | OS pasteboard | page JS via auto-granted `clipboard-read` (F02-02); terminal path per D04 |
| A9 | Camera / microphone / location | OS devices | page JS via auto-granted permissions (F02-02) |
| A10 | Browsing history (URL + title) | events, logs | `BLACKGLASS_LOG` (F02-05), MCP audit (F02-06) |
| A11 | Local filesystem contents | disk | agent via `file://` navigation (F02-10) |
| A12 | Downloaded file contents | disk | per B09/D05 — no quarantine, no save path |

Note the shape of this table: **five of twelve assets have the model's context window as a sink.**
That is the boundary A09 calls TB5, and it is the one where a leak is irreversible — a cookie can be
rotated, a log can be deleted, but a secret that entered a third-party model provider's transcript
cannot be recalled.

---

## 3. Findings

### 3.1 F02-01 · CRITICAL · Agent snapshot leaks plaintext secrets from every non-password field

**The code.** `packages/mcp/lib/snapshot.js:121-122`, verbatim:

```js
const value = node.value && node.value.value;
if (value !== undefined && value !== '' && value !== null) states.push(`value=${quote(String(value))}`);
```

There is no role check, no field-type check, no secret pattern check, no length cap, and no opt-in.
Whatever the accessibility tree reports as a node's value is concatenated into the snapshot text,
which `index.js:389` wraps in the untrusted-content fence and returns as an MCP tool result — i.e.
straight into the model's context, and onward to the model provider.

**The measurement.** I built a page with one canary per realistic secret-bearing field type and ran
the *exact* extraction expression above against `Accessibility.getFullAXTree` (probe §10.1):

| Field | Canary planted | What `snapshot.js:122` emits |
|---|---|---|
| `input[type=password]` | `HUNTER2-CANARY-PW` | `value="•••••••••••••••••"` — **masked** |
| `input[type=text]` (OTP) | `OTP-CANARY-123456` | `value="OTP-CANARY-123456"` — **LEAKED** |
| `input[type=text]` (`aria-label="API key"`) | `sk-live-CANARY-APIKEY` | `value="sk-live-CANARY-APIKEY"` — **LEAKED** |
| `input[type=tel]` (card) | `4111-1111-CANARY` | `value="4111-1111-CANARY"` — **LEAKED** |
| `<textarea>` | `NOTES-CANARY-SECRET` | `value="NOTES-CANARY-SECRET"` — **LEAKED** |
| `input[type=hidden]` | `CSRF-CANARY-HIDDEN` | *(absent — not in the AX tree)* |

**Why this is the worst finding in the document.** The single protection in that table —
`type=password` masking — is Chromium's, not ours, and it covers the one credential users are
*least* likely to hand an agent. The fields that leak are exactly the ones an agent workflow
touches: one-time codes during a login flow, API keys during a setup wizard, card numbers at
checkout, and the free text of a draft message. And unlike a log file, the destination is off-host
and unrecoverable.

**Aggravating factor.** `browser_type` is already careful — `index.js:445` audits `length: value.length`
rather than the value, and the tool's own reply says "Typed N character(s)". So the model types a
secret in without it being recorded, and then the very next `browser_snapshot` reads it straight back
out in plaintext. The discipline exists on the write path and is absent on the read path.

**Fix.** §6.2. Default-deny on editable-node values; do **not** reach for `DOM.describeNode` (§3.8).

---

### 3.2 F02-02 · CRITICAL · No permission handler ⇒ Electron grants everything

**The code.** `apps/engine/src/main.js` imports `{ app, BrowserWindow }` at `:17`. `session` is
never imported; `setPermissionRequestHandler` and `setPermissionCheckHandler` appear nowhere in the
309-line file. Confirmed by grep across `apps/engine/`, `apps/cli/`, and `packages/mcp/` — zero hits.

**Primary source**, Electron **v43.2.0** (the exact version we ship), `docs/tutorial/security.md`
lines 283-285 — fetched during this mission, not recalled:

> By default, Electron will automatically approve all permission requests unless the developer has
> manually configured a custom handler.

**What that grants.** The permission vocabulary for this version, read from the shipped typings at
`apps/engine/node_modules/electron/electron.d.ts:13255`, includes: `clipboard-read`,
`clipboard-sanitized-write`, `display-capture`, `fullscreen`, `geolocation`, `idle-detection`,
`media` (camera **and** microphone), `mediaKeySystem`, `midi`, `midiSysex`, `notifications`,
`pointerLock`, `keyboardLock`, `openExternal`, `speaker-selection`, `storage-access`,
`top-level-storage-access`, `window-management`, `fileSystem`, `unknown`. All auto-approved.

**The chain that matters most.** `clipboard-read` is ordinary in a GUI browser and dangerous here:

```
attacker page loads in BlackGlass
  → navigator.clipboard.readText()          (auto-granted: no handler)
  → returns the clipboard of the terminal the user is working in
  → which routinely holds: an SSH private key, a pasted API token,
    a password copied from a password manager, a production DB URL
  → page POSTs it to attacker origin
```

A GUI browser at least renders a permission chip the user can see. BlackGlass composites a single
terminal row of chrome (`main.rs:254`) and has no prompt layer at all yet, so **there is no pixel on
screen that could ever tell the user this happened.** D05 designs that prompt layer; until it lands,
default-deny is not a preference, it is the only correct posture.

**Two caveats, stated honestly.**
1. Electron's typings at `:13238-13241` note that a *request* handler alone is insufficient — most
   web APIs do a permission **check** first and only request if the check fails. Both handlers must
   be set or the policy has a hole. My §5 matrix sets both.
2. `[UNVERIFIED]` I did not execute a page calling `navigator.clipboard.readText()` end-to-end
   through the offscreen window. The auto-approval is documented for this exact version and the
   handler is provably absent, so the grant is certain; what I have not measured is whether the
   *offscreen, never-shown, dock-hidden* window additionally fails Chromium's transient-activation
   or document-focus preconditions and throws `NotAllowedError` anyway. D04 §5 finds focus events are
   decoded and then discarded (`main.rs:653`), which suggests the window may never be considered
   focused — an accidental mitigation that would evaporate the moment focus forwarding is
   implemented. Settle it with the probe in §10.3. **Do not treat a `NotAllowedError` as a fix.**

**Fix.** §5.

---

### 3.3 F02-03 · HIGH · Cookies persist in the shared, generic `Electron` profile

**The code.** `apps/cli/src/main.rs:402-411` spawns the engine with `--bg-socket`, `--bg-width`,
`--bg-height`, `--bg-url` and nothing else. No `--user-data-dir`. `apps/engine/src/main.js` never
calls `app.setPath('userData', …)` and never sets a `partition` on the `webPreferences` at
`:106-113`. Grep for `setPath|partition|user-data-dir` across `apps/engine/` returns zero hits.

Because the engine runs as an **unpackaged** Electron app, `app.getName()` resolves to `Electron`
and userData resolves to the generic `~/Library/Application Support/Electron/`.

**The measurement.** That directory is not hypothetical — it is populated by this project's own runs:

```
$ ls -la ~/Library/Application\ Support/Electron/
-rw-------  20480 31 Jul 22:11 Cookies
-rw-------      0 31 Jul 22:11 Cookies-journal
drwx------           Local Storage
drwx------           Session Storage
-rw-------  36864 31 Jul 23:00 DIPS
-rw-r--r--     59 31 Jul 22:52 DevToolsActivePort

$ sqlite3 ~/Library/Application\ Support/Electron/Cookies 'select count(*) from cookies;'
10
$ sqlite3 ~/Library/Application\ Support/Electron/Cookies 'select count(distinct host_key) from cookies;'
6
```

**Ten persistent cookies across six hosts, plus Local Storage and Session Storage,** accumulated
during development browsing, in a directory whose name is shared by every unpackaged Electron app on
the machine.

**Three distinct problems, in severity order.**

1. **Cross-application co-tenancy.** Any other unpackaged Electron app — a colleague's prototype,
   an `npx` one-liner, a tutorial project, a malicious postinstall that spawns Electron — resolves to
   the *same* `userData` path and therefore the *same* cookie jar. It can read BlackGlass's session
   cookies and write its own. This is not a sandbox escape; it is two apps agreeing to share a
   database because neither named itself.
2. **Unconsented persistence.** A user who runs `blackglass open https://mail.example.com`, logs in,
   and quits reasonably expects a terminal tool to be ephemeral. It is not: the session survives, on
   disk, indefinitely, with no private mode and no way to clear it from the product.
3. **A dev/prod blast radius.** Every swarm probe and e2e run in this repo has been writing into the
   same jar as any real browsing. `tests/e2e/` and `apps/engine/spike/` share state with the user.

**Honest scoping.** The directory itself is `drwx------` (0700) and `Cookies` is `-rw-------` (0600),
so this is **not** cross-*user* exposure on this host. Cookie values are additionally encrypted at
rest (the schema at `.schema cookies` carries `encrypted_value BLOB NOT NULL`) under a key in the
login Keychain. The exposure is cross-*application*, same-user — which is precisely the threat model
that matters on a developer workstation running untrusted npm lifecycle scripts.

**Note on `DevToolsActivePort` in that directory.** It is present and mode `0644`. Its provenance is
**not established** — `packages/mcp/lib/engine.js:113-116` only enables `--remote-debugging-port` in
the same breath as `--user-data-dir`, so this file should not be here from the MCP path, and it may
belong to an unrelated Electron app (which is itself an illustration of problem 1). I am not
claiming a BlackGlass bug from it. The 0644 mode is contained by the 0700 parent directory.

**Fix.** §7.1 — adopt B09's partition design; the minimal stop-gap is one `app.setPath` call.

---

### 3.4 F02-04 · HIGH · The security opt-out inverts storage isolation

**The code.** `packages/mcp/lib/engine.js:113-116`:

```js
if (this.useCdp) {
  args.push('--remote-debugging-port=0', `--user-data-dir=${this.profileDir}`);
}
```

Both flags are inside one conditional. `useCdp` is false when `BLACKGLASS_MCP_CDP=0`
(`index.js:86`) — which is the documented, security-motivated opt-out. The file's own header comment
(`engine.js:14-22`) presents it exactly that way: the DevTools listener is "UNAUTHENTICATED: any
process running as this user can attach and drive the browser… It is therefore: opt-out via
`BLACKGLASS_MCP_CDP=0`".

**The inversion.** A user who reads that comment and sets `BLACKGLASS_MCP_CDP=0` closes the
unauthenticated port and, in the same action and without any indication, **moves all agent browsing
out of the throwaway `mkdtemp` profile and into the shared persistent `Electron` profile of F02-03.**
They traded a local-attach surface for permanent, co-tenanted cookie storage. The safer-looking
configuration is the one that persists credentials.

This is a one-character class of bug — the two flags are independent and were coupled for
convenience — but it is the kind that survives review because the conditional reads as intentional.

**Fix.** §7.2 — hoist `--user-data-dir` out of the `useCdp` branch; it must be unconditional.

---

### 3.5 F02-05 · HIGH · `BLACKGLASS_LOG` records every URL and title, world-readable

**The code.** `apps/cli/src/main.rs:32-41`:

```rust
fn log_line(msg: &str) {
    let Ok(path) = std::env::var("BLACKGLASS_LOG") else { return };
    …
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{ts} {msg}");
    }
}
```

Four call sites. Two of them are the problem:

- **`main.rs:257`** — `log_line(&format!("start url={url} term={:?} …"))`. The **full initial URL,
  verbatim**, including query string and fragment.
- **`main.rs:549`** — `log_line(&format!("event {s}"))` where `s` is the **entire raw event JSON**
  from the engine. That stream carries `{"t":"url","v":<full URL>}` on every `did-navigate` and
  `did-navigate-in-page` (`main.js:118-119`), `{"t":"title","v":<title>}` on every title change
  (`:117`), **and** `{"t":"loadError",…,"url":…}` (`:122-124`) and `{"t":"popup","url":…}` (`:130`).

So enabling the diagnostic log produces a **complete, timestamped browsing history with full URLs
and page titles**.

**Why full URLs are credential material.** This is not a theoretical concern; it is where the
industry keeps its tokens: password-reset links (`?token=`), email magic links, OAuth authorization
codes (`?code=`), OAuth implicit-flow access tokens (which live in the **fragment**, `#access_token=`),
pre-signed S3 URLs (`?X-Amz-Signature=`), Zoom/Meet join links, unlisted document URLs, invite links,
and every search query the user typed. Titles are barely better: `"Inbox (37) — alice@corp.example"`
discloses identity and correspondent volume.

**The mode.** `OpenOptions::create(true)` requests `0o666`, which the process umask reduces.
Measured on this host:

```
$ umask
022
$ python3 -c "...os.open('/tmp/bgrust.log', O_CREAT|O_APPEND|O_WRONLY, 0o666)...print mode"
0o644
```

**`-rw-r--r--` — world-readable.** Compare with the care taken two hundred lines later, where the
socket is deliberately `0600` inside a `0700` directory (`main.rs:387-400`) with the comment "so no
other local user can connect — an open control socket would be full browser takeover." The socket is
hardened; the file containing the user's entire session-token-bearing history is not. If the user
sets `BLACKGLASS_LOG=/tmp/bg.log` on Linux, that history is readable by every account on the box.

**Fix.** §4 (the redaction policy) and §7.3 (mode 0600 + `O_NOFOLLOW`).

---

### 3.6 F02-06 · HIGH · MCP audit log: predictable path, world-readable, full URLs

**The code.** `packages/mcp/index.js:40-45`:

```js
const AUDIT_FILE = process.env.BLACKGLASS_MCP_AUDIT || path.join(os.tmpdir(), 'blackglass-mcp-audit.jsonl');
function audit(entry) {
  try { fs.appendFileSync(AUDIT_FILE, JSON.stringify({ ts: Date.now(), actor: 'agent', ...entry }) + '\n'); }
  catch { /* never let auditing break a tool call */ }
}
```

`index.js:373` writes `params: { url }` — the **full URL**, unredacted.

**The measurement.** The file already exists on this machine from prior swarm work:

```
$ ls -l "$TMPDIR/blackglass-mcp-audit.jsonl"
-rw-r--r--@ 1 adeebbashir  staff  9661 31 Jul 23:08 …/T//blackglass-mcp-audit.jsonl
```

Mode **0644**, 9,661 bytes, and its first record is a `browser_navigate` carrying a fully
URL-encoded `data:` document — i.e. the audit log captured the entire page body.

**Platform split, and it matters.** On macOS `os.tmpdir()` is the per-user `$TMPDIR`
(`/var/folders/…/T/`, measured `drwx------`), so the 0644 file is contained by a 0700 parent — a
latent bug, not a live exposure. **On Linux `os.tmpdir()` is `/tmp`**, mode `1777`, world-readable
and world-writable. There, the same code produces:

1. **Cross-user history disclosure.** The filename is a compile-time constant, so any local account
   reads the agent's complete navigation history including tokens in URLs.
2. **A pre-creation squat.** An attacker who creates `/tmp/blackglass-mcp-audit.jsonl` first — as a
   file they own, or as a **symlink to a file they want appended to** — wins, because
   `appendFileSync` follows symlinks and the `catch {}` swallows every error silently. The audit
   trail then either lands in the attacker's file or vanishes with no signal. An audit log that can
   be silently disabled by an unprivileged local user is not an audit log.

**Design tension worth naming.** The `catch {}` is deliberate and correct for availability — the
comment says "never let auditing break a tool call". But combined with a predictable world-writable
path it means the provenance log A03 requires can be turned off remotely and invisibly. Availability
and integrity are both required here; §7.4 gets both by creating the file `O_EXCL|O_NOFOLLOW` in a
0700 directory once at startup, then keeping the swallow-on-write behavior.

---

### 3.7 F02-07 · MEDIUM · The URL is passed in argv and is visible in `ps`

**The code.** `apps/cli/src/main.rs:407` — `.arg(format!("--bg-url={url}"))`; identically at
`packages/mcp/lib/engine.js:110`.

**The measurement.**

```
$ /bin/sh -c 'exec -a "electron --bg-url=https://mail.example.com/?token=SECRET_TOKEN_XYZ" /bin/sleep 20' &
$ ps -eo user,args | grep SECRET_TOKEN_XYZ
adeebbashir      electron --bg-url=https://mail.example.com/?token=SECRET_TOKEN_XYZ 20
```

**Scoping, honestly.** On macOS, `KERN_PROCARGS2` restricts full argv to the same uid or root, so
this is **not** cross-user here. On Linux, `/proc/<pid>/cmdline` is world-readable unless `hidepid`
is set, so it **is** cross-user there. Either way it is readable by every process running as the
user, which on a developer machine includes npm lifecycle scripts, editor extensions, and any
compromised CLI tool — and it is captured by process-listing telemetry agents and crash reporters.

This is A09's **T15** ("Token leaked via `ps`/env/logs… Token only over the authenticated socket;
never argv/env"). A09 scoped T15 to the capability token. The finding generalizes: the *navigation
URL* is credential material for exactly the reasons in §3.5, and it is on the command line today.

**Fix.** §7.5 — send the initial URL as a `navigate` command over the existing 0600 socket
immediately after `ready`, rather than as an argument. The engine already has the code path
(`main.js:228-230`); `--bg-url` becomes unnecessary.

---

### 3.8 F02-08 · MEDIUM · The obvious fix for F02-01 hands back the password

This is the most useful thing I measured, because it invalidates the natural first patch.

To redact secret fields you must first identify them. The AX tree cannot: probe §10.2 put a
`type=password` and a `type=text` node side by side and dumped their AX properties.

```
AXNODE {"role":"textbox", …, "properties":["invalid=\"false\"","focusable=true","editable=\"plaintext\"",
        "settable=true","multiline=false","readonly=false","required=false"], "value":"•••••••••••••••••"}
AXNODE {"role":"textbox", …, "properties":["invalid=\"false\"","focusable=true","editable=\"plaintext\"",
        "settable=true","multiline=false","readonly=false","required=false"], "value":"sk-CANARY"}
```

**Byte-identical role and properties.** Nothing in the accessibility node says "password". So the
obvious next move is to ask the DOM. That is the trap:

```
DOMATTRS ["type","password","id","p","value","HUNTER2-CANARY-PW"]
DOMATTRS ["type","text","id","k","aria-label","API key","autocomplete","one-time-code","value","sk-CANARY"]
```

**`DOM.describeNode` returns the password's plaintext in the attribute list.** A patch that calls
`DOM.describeNode` to discover `type=password` and then logs, caches, or passes the attribute array
around has just materialized the one secret Chromium was masking — and it will do so in the code
path whose stated purpose is protecting it. This is a live risk for a well-intentioned fix, and it
is exactly the shape of A09's `REDACT_SELECTOR` approach if implemented naively.

Two precise qualifications, so this is not overstated:

- The `value` **attribute** is the HTML attribute (the initial/server-rendered value), not
  necessarily the live IDL property. It is plaintext for server-prefilled forms, saved-credential
  autofill in markup, and any framework that reflects state to the attribute; it is empty for a
  password the user just typed into an initially-empty field.
- `autocomplete="one-time-code"` **is** present in the attribute list. That is a genuinely useful
  positive signal for OTP redaction and worth consuming — carefully.

**Consequence for the fix.** Identification must never carry the payload. §6.2 therefore recommends
default-deny *without* identification as the primary control (no DOM call at all, so no trap), and
treats attribute-based classification as a strictly optional refinement subject to the hard rule in
§6.3: **never read, retain, or pass the `value` attribute pair; project to `type`/`autocomplete`/
`name`/`aria-label` at the point of extraction.**

---

### 3.9 F02-09 · MEDIUM · Password length leaks through the mask

Measured in §3.1: `HUNTER2-CANARY-PW` is 17 characters and renders as exactly 17 bullets
(`•••••••••••••••••`). Chromium's mask is length-preserving.

`snapshot.js:122` emits that string verbatim, so the exact password length reaches the model context
and every downstream sink. Password length is a real, if modest, secret: it narrows an offline
attack and, combined with a known site policy, can be distinguishing.

**Fix.** Collapse rather than pass through: emit `value=<redacted:password>` with no length. Note
that this also means the bullet-run is a usable *detection* signal — a value matching `^[••]+$`
is a masked credential field and can be redacted with **no DOM call at all**, sidestepping §3.8
entirely. Use it as a belt-and-braces detector alongside default-deny, never as the only one (a page
can trivially render literal bullets in a normal text input to spoof it in either direction).

---

### 3.10 F02-10 · MEDIUM · No scheme allowlist on navigation

**The code.** `packages/mcp/index.js:367-372` — `browser_navigate` validates only
`typeof url !== 'string' || !url` and passes the value to `s.navigate(url)` → `loadURL`. No scheme
check anywhere in `packages/mcp/`.

`apps/cli/src/main.rs:293-304` — `normalize_url` explicitly *constructs* `file://` URLs for
arguments starting with `/` or `./`, and passes `data:` and any `scheme://` through untouched.

**The agent-side chain.** `browser_navigate("file:///Users/…/.ssh/id_ed25519")` renders the file,
and `browser_snapshot` returns its text inside the untrusted-content fence — into the model context
and off the machine. Same for `~/.aws/credentials`, `.env`, shell history, and Keychain-adjacent
plaintext. Nothing in the MCP server prevents it.

**Why the fence does not help.** `index.js:52-61` fences page text as untrusted *data* so the model
will not obey instructions inside it. That defends against the page **commanding** the agent. It
does nothing about the agent **reading** something it should not — the text is still in the context
window regardless of how it is labeled. A prompt-injected agent (or simply a confused one) that is
told "check the config file at /Users/x/.aws/credentials" will comply, and the fence will faithfully
mark the stolen credentials as untrusted.

**For the CLI path** `file://` is a legitimate feature — a terminal browser that cannot open a local
HTML file is worse for it. The asymmetry is the point: **a human typing `blackglass open ./report.html`
has consented; an agent calling `browser_navigate("file:///…")` has not.** The policy in §6.4 splits
on that.

---

### 3.11 F02-11 · MEDIUM · CDP + a real profile = a cookie-theft endpoint

`packages/mcp/lib/engine.js:14-22` already documents the exposure honestly and I credit that: the
DevTools listener is loopback-only ("verified with lsof") but **unauthenticated**, so any same-uid
process can attach.

The finding F02 adds is the interaction with `BLACKGLASS_MCP_PROFILE` (`engine.js:88`). The
throwaway-profile mitigation is what keeps the unauthenticated port boring — an attacker who attaches
to a blank profile gets a blank browser. The moment a user sets `BLACKGLASS_MCP_PROFILE` to a real,
logged-in profile (which is the natural thing to do the first time an agent hits a login wall), the
same port becomes a **credential exfiltration endpoint**: attach, `Network.getAllCookies`, done —
returning decrypted cookie values for every host in the jar, bypassing the at-rest encryption of
§3.3 entirely, with no user-visible signal.

Also note `DevToolsActivePort` is written mode 0644 (measured, §3.3). Contained by the 0700 profile
directory when the profile is a `mkdtemp` one; **not** necessarily contained for a user-chosen
`BLACKGLASS_MCP_PROFILE` path, whose mode nobody checks.

**Fix.** §7.6 — refuse to enable CDP and a non-throwaway profile simultaneously; fail loudly rather
than silently combining them. The durable fix remains the one `engine.js:21` already names: move
`eval` onto the existing 0600 socket and delete the port.

---

### 3.12 F02-12 · LOW · No control-character sanitization on any log sink

`log_line` (`main.rs:32-41`) writes `msg` raw. `audit()` (`index.js:43`) writes `JSON.stringify(...)`.
Log files are routinely replayed through a terminal (`cat`, `tail -f`, `less -R`), which is the exact
TB3 boundary A09 §1 is about — so an attacker-controlled title in a log file is a delayed-action
escape injection that fires when the developer reads the log.

**Measured, and the news is good — accidentally.** The engine serializes events with
`JSON.stringify` (`main.js:61`), which escapes control characters below `0x20`:

```
$ node -e 'const esc=String.fromCharCode(27); const p=JSON.stringify({t:"title",v:"pwn"+esc+"]52;c;SGk="});
           console.log(p); console.log("contains_raw_ESC:", p.includes(esc));'
{"t":"title","v":"pwn]52;c;SGk="}
contains_raw_ESC: false
```

So `main.rs:549`'s raw-JSON logging does **not** currently emit a live `ESC`. Two reasons that is not
a defense:

1. It is **incidental**. Nothing states or tests the property. Any future refactor that logs
   `event.v` as a field instead of logging the JSON envelope reintroduces the injection instantly,
   and the reviewer has no test to fail.
2. **`main.rs:257` already does exactly that.** `start url={url}` interpolates the raw URL from argv
   with no JSON envelope and no sanitization. A URL containing an escape sequence — trivially
   arrived at by pasting one, and `data:` URLs make it easy — lands raw in the log.

`crates/bg-term/src/unicode.rs` already provides `sanitize_for_terminal`, used for the status bar at
`main.rs:887-888`. The log sink should use the same function. The property is cheap to hold and
cheap to test (§8, gate G5).

---

## 4. THE POLICY, part 1 — data classification and log redaction

This section is the mission's core deliverable. It is written to be implementable and testable, not
aspirational.

### 4.1 Classification

Every datum that can reach a sink is assigned exactly one class. **Unclassified data is S1 by
default** — the classification is deny-by-default, so forgetting to classify a new field makes it
*more* redacted, not less.

| Class | Name | Rule | Examples |
|---|---|---|---|
| **S0** | **SECRET** | **Never written to any sink, at any log level, ever.** Not truncated, not hashed-with-recovery, not "debug only". | Password/OTP/API-key/card field values, clipboard contents, cookie values, `Authorization` headers, URL query and fragment **values**, typed text, downloaded file contents |
| **S1** | **SENSITIVE** | Redacted by default. Emitted only in transformed form (§4.2), or in full only under an explicit, per-run, documented opt-in that prints a warning. | Full URLs, page titles, AX node accessible names, local file paths, search terms, hostnames |
| **S2** | **OPERATIONAL** | Free to log. Must contain no S0/S1 substring by construction. | Geometry, frame counts, byte counts, timings, backend name, Electron/Chromium versions, error codes, event type discriminators |

**The construction rule (the one that actually prevents leaks).** Log records must be **built from
typed fields, never formatted from free strings**. `format!("event {s}")` (`main.rs:549`) is the
anti-pattern: it cannot be audited, because whether it leaks depends on what the engine happens to
put in `s` today. A redactor that filters strings after the fact is a permanent game of catch-up; a
constructor that only accepts classified fields cannot leak a field nobody classified.

### 4.2 URL redaction — the single most important rule

URLs are S1 in structure and S0 in content. The transform:

```
KEEP     scheme, host, port
KEEP     path, with each segment length-classed if it looks like an identifier
DROP     query — entirely. Emit only the parameter COUNT and the KEY NAMES.
DROP     fragment — entirely. Emit only its length.
DROP     userinfo (user:pass@) — entirely, unconditionally.
```

Dropping the **fragment** is non-negotiable and is the rule most implementations miss: the OAuth 2
implicit flow returns `#access_token=…` in the fragment, and fragments are never sent to servers, so
a log is one of the few places that token can be captured.

Emitting query **key names** but not values is a deliberate balance: `?token=&redirect_uri=` is
enormously useful for debugging a redirect loop and discloses nothing. Key names must themselves be
capped and character-classed so a hostile page cannot smuggle a payload through a parameter name.

Worked examples — these are the exact expected outputs for gate G1 in §8:

| Input | Logged form |
|---|---|
| `https://example.com/reset?token=abc123XYZ&uid=99` | `https://example.com/reset?<2 params: token,uid>` |
| `https://app.example.com/#access_token=eyJhbGciOi…` | `https://app.example.com/#<redacted:len=241>` |
| `https://mail.example.com/u/0/inbox/FMfcgz…QjTbW` | `https://mail.example.com/u/0/inbox/<id:22>` |
| `https://user:hunter2@intranet.example/` | `https://<userinfo redacted>@intranet.example/` |
| `file:///Users/alice/.ssh/id_ed25519` | `file:///<path redacted:4 segments>` |
| `data:text/html;base64,PGh0bWw+…` | `data:text/html;base64,<redacted:len=8214>` |
| `https://duckduckgo.com/?q=how+to+…` | `https://duckduckgo.com/?<1 param: q>` |
| `about:blank` | `about:blank` (allowlisted verbatim) |

Note the last-but-one: search queries are the user's private thoughts and belong in S0 by value even
though the URL is S1 by structure. The rule handles it for free, because `q`'s *value* is dropped
like every other query value. That is the advantage of a structural rule over a denylist of
parameter names — no list to keep current.

### 4.3 Title redaction

Page titles are S1 and must not be logged verbatim by default. Log
`title_len=<n> title_hash=<first 8 hex of SHA-256>`. The hash makes "did the title change?" — the
only question a log is actually asked about titles — answerable, without disclosing
`"Inbox (37) — alice@corp.example"`. Under the opt-in level of §4.4, log the title after
`sanitize_for_terminal` truncation to 60 chars, matching the status bar's existing treatment
(`main.rs:887-888`).

### 4.4 Log levels

| Level | Env | Contents | Default |
|---|---|---|---|
| `off` | unset | nothing; sink never created | ✅ |
| `ops` | `BLACKGLASS_LOG_LEVEL=ops` | S2 only. No URLs, no titles. Enough to debug a rendering or protocol fault. | |
| `nav` | `…=nav` | S2 + S1-redacted (§4.2, §4.3). The useful default for a bug report. | |
| `full` | `…=full` | S1 verbatim. **S0 still never appears.** Prints a one-line warning to stderr at startup naming the file and stating it will contain URLs and titles. | |

**`BLACKGLASS_LOG` being set must no longer imply a level.** Today, setting the path enables full URL
and title logging (§3.5). It should default to `nav`. There is no level at which S0 is written; the
`full` level is about S1, and that distinction must be enforced by the type system rather than by
reviewer discipline (§4.1's construction rule).

### 4.5 Sink hygiene

Applies to `BLACKGLASS_LOG`, `BLACKGLASS_MCP_LOG`, and `BLACKGLASS_MCP_AUDIT` alike:

1. **Mode `0600` on creation**, set atomically via `OpenOptionsExt::mode(0o600)` on Unix (Rust) and
   `fs.openSync(path, 'a', 0o600)` (Node). Do not create-then-`chmod`: that races.
2. **`O_NOFOLLOW`**, so a symlink planted at the path fails the open instead of appending to the
   target (§3.6).
3. **Default location must not be world-writable.** Replace `os.tmpdir()` with a `0700`
   application directory (`$XDG_STATE_HOME/blackglass/` or `~/Library/Application Support/BlackGlass/`).
   If a tmpdir path is kept for compatibility, create it `O_EXCL` once at startup and fail loudly if
   it exists and is not a 0600 regular file owned by the current uid.
4. **Sanitize on write.** Every string reaching a sink passes `unicode::sanitize_for_terminal`
   (§3.12), because logs are replayed through terminals.
5. **Bound it.** Rotate at 8 MiB, keep one generation. An unbounded append in a `0700` directory is
   the friendlier failure, but on a machine at 98% disk (project constraint) it is still a real one.
6. **Never log to stdout while browsing.** Already correct and correctly commented at
   `main.rs:30-31`; restated so the property survives refactors.

### 4.6 The last-resort secret net

Structural rules (§4.1) are the primary control. A regex scrub is the backstop for the case they
miss — a secret that arrives inside a field nobody classified. It runs on **every** string entering
**every** sink, replacing matches with `<redacted:{kind}>`:

| Kind | Pattern (illustrative; keep the live list in one shared module) |
|---|---|
| `jwt` | `eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}` |
| `bearer` | `(?i)\b(bearer|token|api[_-]?key|secret|passwd|password)\b\s*[:=]\s*\S+` |
| `vendor-key` | `\b(sk|pk|rk)-(live|test)-[A-Za-z0-9]{16,}\b`, `\bgh[pousr]_[A-Za-z0-9]{16,}\b`, `\bAKIA[0-9A-Z]{16}\b`, `\bAIza[0-9A-Za-z_-]{20,}\b`, `\bxox[baprs]-[0-9A-Za-z-]{10,}\b` |
| `private-key` | `-----BEGIN [A-Z ]*PRIVATE KEY-----` |
| `pan` | 13–19 digits with optional separators **passing a Luhn check** (Luhn keeps the false-positive rate near zero) |
| `masked` | `^[••*]{4,}$` → `<redacted:masked-field>` (§3.9) |
| `high-entropy` | token ≥ 24 chars, Shannon entropy ≥ 3.5 bits/char, not a known-safe shape (hex hash, base64 image data, UUID) |

Two honest caveats. **This list will always be incomplete** — it is a net, not a wall, and must never
be the reason a structural rule was skipped. And **the scrubber must be fast and non-allocating on
the common path**, because it sits in the frame loop; a compiled `aho-corasick` prefilter on literal
markers (`eyJ`, `sk-`, `ghp_`, `AKIA`, `AIza`, `xox`, `-----BEGIN`) before any regex runs keeps the
cost at a memchr scan for the 99.9% of lines that contain no secret.

### 4.7 Traces, crash reports, and the sinks nobody remembers

Redaction that covers the log and forgets the crash dump has redacted nothing. The same rules apply,
by class not by sink, to:

| Sink | Status today | Requirement |
|---|---|---|
| Crash reports (B08) | S1 in the URL/title of the crashed page | Same S0/S1 rules. Never attach the frame buffer: a screenshot **is** the rendered secret. |
| Recorder/replay traces (E06) | typed text is S0 by definition | Record `length` + field ref, never the text — mirror `index.js:445`'s existing discipline |
| MCP tool **responses** | `statusLine()` (`index.js:112-117`) emits the **full URL and title on every single tool call** | These go to the model provider. Apply §4.2 unless the run opts in. **This is the highest-volume S1 egress in the product and is currently unredacted.** |
| `process.env` dumps in diagnostics | would capture `BLACKGLASS_MCP_PROFILE`, proxy creds | Allowlist `BLACKGLASS_*` non-secret keys; never dump env wholesale |
| Terminal scrollback | the rendered page is in the user's scrollback | Out of scope for redaction; note it in docs — TB3 |
| `ps` / argv | §3.7 | Fixed by moving the URL to the socket (§7.5) |

The third row deserves emphasis. `statusLine()` runs on the return path of essentially every tool in
`packages/mcp/index.js`. Whatever `BLACKGLASS_LOG` does or does not record, the **full URL and title
are already being sent to the model provider on every action** — which makes it a larger egress
channel than any file on disk.

---

## 5. THE POLICY, part 2 — permissions

Default **deny**, with a three-tier model. Tiers 1 and 2 are implementable today in nine lines and
require no UI; tier 3 lands with D05's prompt layer.

| Permission | Tier | Policy until D05's prompt layer exists |
|---|---|---|
| `media` (camera, microphone) | 3 | **DENY** |
| `display-capture` | 3 | **DENY** |
| `geolocation` | 3 | **DENY** |
| `clipboard-read`, `deprecated-sync-clipboard-read` | 3 | **DENY** (§3.2; and see D04 for the terminal-side path) |
| `midi`, `midiSysex`, `hid`, `serial`, `usb` | 1 | **DENY** — no plausible terminal-browser use |
| `notifications` | 2 | **DENY** — no surface to render one |
| `idle-detection`, `window-management`, `keyboardLock`, `pointerLock`, `speaker-selection` | 1 | **DENY** |
| `openExternal` | 1 | **DENY** — Electron security guide item 15 (`security.md:688`); handing a URL to the OS shell from an offscreen page is a host-compromise primitive |
| `fileSystem` | 3 | **DENY** (File System Access API = arbitrary local read/write) |
| `mediaKeySystem` | 2 | **DENY** initially; revisit if DRM playback is ever in scope |
| `fullscreen` | 1 | **ALLOW** — meaningless offscreen, harmless |
| `clipboard-sanitized-write` | 2 | **ALLOW** — sanitized write is the low-risk direction and "copy" is a core browsing verb |
| `storage-access`, `top-level-storage-access` | 2 | **ALLOW** — denying breaks legitimate embedded logins; it is not a device-access grant |
| `unknown` and anything unlisted | — | **DENY** — the default branch must deny, so a future Chromium permission is denied on arrival rather than granted by omission |

**Both handlers are mandatory.** Per `electron.d.ts:13238-13241`, most web APIs perform a permission
*check* first and only *request* if the check fails; setting only `setPermissionRequestHandler`
leaves the check path at its permissive default. Set both, backed by one shared table.

**When D05's prompt layer lands**, tier 3 becomes ASK, scoped per-origin and per-session, with the
grant expiring at navigation away from the origin and never persisting to disk without explicit
user action. Tier 1 stays DENY permanently — it is not worth a prompt.

**Agent interaction.** An agent-driven session must **never** be able to answer a tier-3 prompt.
If a prompt is raised while the MCP server is driving, the answer is an automatic deny plus an
event the agent can observe, so the model can report "this page wants your camera" to the human
rather than click through it. Prompt-injection resistance is worthless if the injected instruction
can grant itself the microphone.

---

## 6. THE POLICY, part 3 — agent access and context redaction

### 6.1 Principle

The agent is **not** a trusted subject. It is a semi-trusted actor whose instructions may originate
from the page it is reading — this is A09's TB5, and `index.js:52-61`'s fence is a partial control
(§3.10). Capability, not trust, must bound it.

### 6.2 Field values in snapshots — the fix for F02-01

Replace `snapshot.js:121-122` with a default-deny tier:

- **Tier C — emit verbatim.** Values on **non-editable** nodes. A `slider` at `value="30"`, a
  `combobox` showing `"United States"`, a progress meter. These are page state, not user secrets,
  and the agent genuinely needs them.
- **Tier B — redact by default, opt-in per call.** Values on **editable** nodes
  (`editable` property present, or role in `textbox`/`searchbox`/`combobox`). Default emission:
  `value=<redacted:17 chars>` — enough for the model to know the field is non-empty and roughly how
  full, which is what it needs to decide whether to clear before typing. Real values only when the
  caller passes `browser_snapshot({ includeFieldValues: true })`, which must be documented as
  "sends the contents of form fields to the model".
- **Tier A — always redact, never opt-inable.** Any value that is bullet-masked (`^[••]{2,}$`,
  §3.9), matches the §4.6 secret net, or sits in a field classified sensitive by the optional
  refinement below. `includeFieldValues: true` must **not** override tier A. A user opting into form
  values has not opted into their password manager's contents.

This ordering matters: **tier B alone fixes F02-01 completely**, with no CDP round trip, no
`DOM.describeNode`, and therefore no §3.8 trap. Ship it first and alone.

### 6.3 Optional refinement, with a hard rule

To promote specific fields from tier B to tier A — better UX, because it lets `includeFieldValues`
stay useful on ordinary forms while never exposing credential fields — classify by DOM attributes:
`type` ∈ {`password`}, `autocomplete` ∈ {`current-password`, `new-password`, `one-time-code`,
`cc-number`, `cc-csc`}, or `name`/`id`/`aria-label` matching
`(?i)pass|pwd|otp|totp|mfa|2fa|secret|token|api[_-]?key|cvv|cvc|ssn|routing|iban`.

**Hard rule, from the §3.8 measurement.** The attribute array returned by `DOM.describeNode` (or
`DOM.getAttributes`) **contains the field's plaintext value**. The classifier must project to the
keys it needs *at the point of extraction* and let the rest go out of scope immediately. Never log
the array, never return it from the classifier, never include it in an error message or an exception
payload, never cache it. Prefer `DOM.querySelectorAll` with a selector list — it returns *node ids
only*, never attributes, and so cannot leak by construction. That is the shape to implement.

### 6.4 Navigation

| Scheme | Human (CLI) | Agent (MCP) |
|---|---|---|
| `https:` | allow | allow |
| `http:` | allow + warn | allow + warn |
| `about:blank` | allow | allow |
| `file:` | **allow** — explicit human action | **DENY by default** (§3.10). Opt-in via `BLACKGLASS_MCP_ALLOW_FILE=1`, and even then confined to a configured root, resolved with symlinks followed **before** the prefix check |
| `data:` | allow | allow, capped at 64 KiB |
| `blob:`, `filesystem:` | allow | deny |
| `javascript:` | **deny both** | **deny both** — a `javascript:` navigation is script injection into whatever origin is loaded |
| everything else (`chrome:`, `devtools:`, custom app schemes) | deny | deny |

Enforce at **both** entry points: `main.rs:293` (`normalize_url`) and `index.js:367`
(`browser_navigate`) — and additionally in a `will-navigate` handler in the engine, since a page can
initiate navigation without either front end being consulted.

### 6.5 Audit integrity

The audit log is a security control (A03 requires action attribution), so it needs the S0/S1 rules
*and* integrity: created `O_EXCL|O_NOFOLLOW` mode 0600 in a 0700 app directory (§4.5), URLs redacted
per §4.2, `browser_type` keeping its existing `length`-only discipline (`index.js:445` — already
correct, do not regress it), and `browser_press_key` (`index.js:470`) switching to a **class** rather
than the literal key: a model that types a password one `browser_press_key` at a time currently
reconstructs it perfectly in the audit trail, one character per line.

---

## 7. Implementation checklist for the commander

I own no core file; these are specifications, ordered by (severity × inverse effort). Effort is my
estimate of the diff, not of review.

| # | File:line | Change | Effort |
|---|---|---|---|
| **7.0** | `packages/mcp/lib/snapshot.js:121-122` | **F02-01.** Emit `value=<redacted:N chars>` for editable nodes; verbatim only for non-editable; add `includeFieldValues` opt-in (§6.2). **Do this first.** | ~15 lines |
| **7.1** | `apps/engine/src/main.js` (new, near `:288`) | **F02-02 + F02-03.** `session.defaultSession.setPermissionRequestHandler` **and** `setPermissionCheckHandler` from the §5 table; `app.setPath('userData', …)` to a BlackGlass-specific dir before `whenReady` resolves. Adopt B09's partition design as the full version. | ~20 lines |
| **7.2** | `packages/mcp/lib/engine.js:113-116` | **F02-04.** Hoist `--user-data-dir` out of the `if (this.useCdp)` block. | 3 lines |
| **7.3** | `apps/cli/src/main.rs:32-41, 257, 549` | **F02-05 + F02-12.** `OpenOptionsExt::mode(0o600)` + `custom_flags(O_NOFOLLOW)`; route both call sites through the §4.2/§4.3 redactor; `sanitize_for_terminal` on write; add `BLACKGLASS_LOG_LEVEL` (§4.4). | ~60 lines + tests |
| **7.4** | `packages/mcp/index.js:40-45, 373` | **F02-06.** Move default out of `os.tmpdir()` to a 0700 app dir; create `O_EXCL|O_NOFOLLOW` 0600 at startup; redact the URL at `:373`. Keep the `catch {}` on **write** only. | ~25 lines |
| **7.5** | `apps/cli/src/main.rs:402-411`; `packages/mcp/lib/engine.js:104-111`; `apps/engine/src/main.js:23` | **F02-07.** Drop `--bg-url`; send `{t:'navigate',url}` over the 0600 socket after `ready`. The engine handler already exists (`main.js:228-230`). | ~15 lines |
| **7.6** | `packages/mcp/lib/engine.js:88, 113` | **F02-11.** If `BLACKGLASS_MCP_PROFILE` is set **and** `useCdp`, refuse to start with an explanatory error. Verify the profile dir is 0700 and uid-owned. | ~10 lines |
| **7.7** | `apps/cli/src/main.rs:293`; `packages/mcp/index.js:367`; engine `will-navigate` | **F02-10.** The §6.4 scheme allowlist at all three points. | ~30 lines |
| **7.8** | shared module (new) | §4.6 secret net + §4.2 URL redactor, with the `aho-corasick` prefilter. Needed by 7.3, 7.4, and 7.0. | ~120 lines + tests |
| **7.9** | `packages/mcp/index.js:112-117` | §4.7. Redact `statusLine()`'s URL/title unless opted in — the highest-volume S1 egress in the product. | ~5 lines |
| **7.10** | repo root | `LICENSE` file matching `Cargo.toml:9` (`MIT OR Apache-2.0`). Cross-ref D05. | 1 file |

**Dependency note:** 7.8 is a prerequisite for the full versions of 7.3 and 7.4, but **not** for 7.0
— which is the argument for shipping 7.0 first and alone. It has no dependencies, it is the highest
severity, and it is the smallest diff in the table.

---

## 8. Test gates (CI-able, no screenshots required)

All of these are protocol/log/canary assertions, which suits the constraint that screenshot
verification is unavailable at a lock screen.

| Gate | Assertion | Fails today? |
|---|---|---|
| **G1** | For each row of the §4.2 table, `redact_url(input) == expected`. Table-driven unit test. | n/a (no redactor) |
| **G2** | **Canary sweep.** Drive a page holding one canary per field type from §3.1; then `grep -c 'CANARY' <every sink>` — MCP response, `BLACKGLASS_LOG`, audit JSONL, MCP stderr log — **must be 0**. Passwords must appear as neither plaintext nor a length-preserving bullet run. | **YES — measured, §3.1** |
| **G3** | Load a page calling `getUserMedia`, `geolocation.getCurrentPosition`, `navigator.clipboard.readText`, and `getDisplayMedia`; assert **every** one rejects and each emits a deny event. | **YES — §3.2** |
| **G4** | After a full browse-and-quit cycle, assert no new rows in `~/Library/Application Support/Electron/Cookies` and that the profile path used is BlackGlass-specific. | **YES — §3.3** |
| **G5** | Write a title/URL containing `ESC`, `BEL`, `CSI`, and an `OSC 52` payload; assert no byte `< 0x20` other than `\n` reaches any log file. | **YES for `main.rs:257` — §3.12** |
| **G6** | `stat` every sink after a run: mode must be `0600`, owner the current uid, and the parent directory `0700`. | **YES — measured 0644, §3.5/§3.6** |
| **G7** | `ps -eo args` during a session must not match the canary token embedded in the URL. | **YES — measured, §3.7** |
| **G8** | `browser_navigate('file:///etc/passwd')` must be refused by the MCP server, and `browser_navigate('javascript:1')` refused by both front ends. | **YES — §3.10** |
| **G9** | Pre-create the audit path as a symlink to a canary file; assert BlackGlass refuses and the canary is unmodified. | **YES — §3.6** |
| **G10** | Grep gate: no `format!`/template literal interpolating `url`, `title`, or `value` into a log call outside the redactor module. Enforces §4.1's construction rule mechanically. | **YES — `main.rs:257`, `:549`** |

G2 is the one to build first: it is a single harness that would have caught F02-01, F02-05, F02-06,
and F02-09 simultaneously, and it is the gate that keeps them fixed.

---

## 9. Recommendation

**Ship 7.0 today, alone, ahead of everything else in this document.**

Rationale, in the order it matters:

1. **It is the only finding whose leak leaves the machine.** F02-03's cookies are on a 0600 file
   under a 0700 directory. F02-05's log is world-readable but local. F02-01's secrets go into a
   model provider's transcript. A cookie can be rotated and a log deleted; an OTP, an API key, and a
   card number that entered a third-party context window cannot be recalled.
2. **It is measured, not modeled.** §3.1 has four named canaries recovered in plaintext through the
   exact expression in shipped code.
3. **It is the smallest diff in §7** and has no dependency on the redactor module.
4. **The naive version of the fix is actively harmful**, and §3.8 is the only place that is
   documented. A patch that reaches for `DOM.describeNode` to find password fields retrieves the
   password. Tier B default-deny avoids the trap entirely by never asking the DOM anything.

Then 7.1 (the nine-line default-deny permission handler), because it is the only finding that reaches
outside the browser to the camera, microphone, and clipboard, and because D05 arrived at the same
conclusion independently — two missions converging on the same three lines is a strong signal.

Then 7.2, which is three lines and removes a trap where the security-conscious configuration is the
less safe one.

---

## 10. Probe sources

Both probes are outside the repository, write nothing into it, and were run against the workspace's
own `apps/engine/node_modules/.bin/electron` with the agent Bash sandbox disabled (Chromium child
processes fail under it — a known constraint of this environment). Canaries are synthetic values I
planted in a local `data:` page; no real credential was involved.

### 10.1 `axprobe.js` — does the snapshot leak secret values? (§3.1)

```js
// Mirrors packages/mcp/lib/snapshot.js:121-122 exactly.
const { app, BrowserWindow } = require('electron');
const PAGE = 'data:text/html,' + encodeURIComponent(`<!doctype html><body>
<input type="password" id="p" value="HUNTER2-CANARY-PW">
<input type="text" id="t" value="OTP-CANARY-123456">
<input type="text" id="k" aria-label="API key" value="sk-live-CANARY-APIKEY">
<input type="tel" id="c" value="4111-1111-CANARY">
<textarea id="ta">NOTES-CANARY-SECRET</textarea>
<input type="hidden" id="h" value="CSRF-CANARY-HIDDEN">
</body>`);
app.on('window-all-closed', () => {});
app.whenReady().then(async () => {
  if (app.dock) app.dock.hide();
  const w = new BrowserWindow({ show: false, width: 800, height: 600,
    webPreferences: { offscreen: true, nodeIntegration: false, contextIsolation: true, sandbox: true } });
  await w.loadURL(PAGE);
  await new Promise(r => setTimeout(r, 900));
  const dbg = w.webContents.debugger; dbg.attach('1.3');
  await dbg.sendCommand('Accessibility.enable');
  const { nodes } = await dbg.sendCommand('Accessibility.getFullAXTree', { depth: -1 });
  const out = [];
  for (const n of nodes) {
    const value = n.value && n.value.value;                                   // snapshot.js:121
    if (value !== undefined && value !== '' && value !== null) {
      out.push({ role: (n.role||{}).value, name: ((n.name||{}).value||'').trim(),
                 emitted: `value=${JSON.stringify(String(value))}` });        // snapshot.js:122
    }
  }
  console.log('AXPROBE_RESULT ' + JSON.stringify(out, null, 2));
  app.exit(0);
});
```

Run: `cd apps/engine && ./node_modules/.bin/electron <path>/axprobe.js`. Output in §3.1.

### 10.2 `axprobe2.js` — can a fix identify a secret field? (§3.8)

Same scaffold; the loop additionally calls
`dbg.sendCommand('DOM.describeNode', { backendNodeId: n.backendDOMNodeId })` and prints
`d.node.attributes` alongside the AX `properties`. Output in §3.8 — identical AX properties for
password and text nodes, and the password's plaintext present in the DOM attribute array.

### 10.3 `[UNVERIFIED]` — clipboard-read end-to-end (§3.2)

Not run. To settle it, load a page calling `navigator.clipboard.readText()` in the offscreen window
with no permission handler registered and observe whether it resolves with clipboard contents or
rejects `NotAllowedError` for want of document focus. **A rejection is not a fix** — it is an
accident of D04's finding that focus events are decoded and discarded (`main.rs:653`), and it
disappears the moment focus forwarding is implemented. Register the §5 handler regardless.

---

## 11. Unverified claims, stated plainly

| Claim | Why unverified | How to settle |
|---|---|---|
| A page can actually read the clipboard today (vs. failing on focus) | not executed | §10.3 |
| `will-download` with no handler raises an invisible `NSSavePanel` | **D05 measured this** (probe D: no `done` event for 20 s, then `cancelled` with an empty save path). I did not re-run it and I accept D05's result. | D05 §3.1 |
| File chooser (`<input type=file>`) behavior offscreen | not executed; D05 reports Electron has no public API for it and that CDP is required | D05 §5 |
| Cross-user `/proc/<pid>/cmdline` visibility on Linux | this host is macOS; the macOS half is measured (§3.7) | run G7 on a Linux CI runner |
| Provenance of `DevToolsActivePort` in the shared profile | file present, writer not identified; may belong to an unrelated Electron app | `lsof` during a clean BlackGlass run |
| iTerm2 behavior for any of the above | macOS TCC blocks automation (project constraint) | manual run |

---

## 12. One-paragraph summary for the commander

BlackGlass has no secret-handling layer today: no permission handler, no redactor, no data
classification, no scheme allowlist, and no profile isolation. Eight of the twelve findings here were
measured on this machine against shipped code. The worst is that the agent snapshot emits the
plaintext of every editable field except `type=password` — I recovered a planted OTP, API key, card
number, and textarea through the exact expression at `packages/mcp/lib/snapshot.js:122` — and that
data leaves the machine for a model provider, which makes it the one leak that cannot be undone.
Second is that Electron auto-approves every permission request because no handler is registered
(primary source: Electron v43.2.0 `security.md:283`), which hands any page the camera, microphone,
geolocation, and the clipboard of the shell the user is about to paste into. Behind those, cookies
are quietly accumulating in the generic shared `Electron` profile (10 cookies, 6 hosts, measured),
`BLACKGLASS_LOG` writes every URL and title into a 0644 file, and the documented security opt-out
`BLACKGLASS_MCP_CDP=0` silently makes storage isolation worse. §4 gives the redaction policy — S0
never logged, S1 redacted by default, URLs stripped of query *values* and the entire fragment, log
records built from typed fields rather than formatted from free strings — and §5 gives the
default-deny permission matrix. Fix `snapshot.js:122` first: it is fifteen lines, it depends on
nothing else, and §3.8 documents why the obvious version of that fix hands back the very password
Chromium was protecting.
