# E08 — Chromium Extension Support in Electron 43.2.0

**Mission:** Investigate Chromium extension support: `session.loadExtension`, which Manifest V3
features work, which Chrome-specific services do not (sync, webstore), and the security
implications of loading unpacked extensions. State clearly what BlackGlass can and cannot support.

**Owned output:** this file only. No core file was modified. Every change to
`apps/engine/src/main.js`, `apps/cli/**` or `crates/**` is **specified here as an instruction for
the commander**, not applied. See §13.

---

## 0. Headline findings

Four results change the design. All were measured on this machine against the Electron 43.2.0
already installed in `apps/engine/node_modules/electron`, not recalled from training and not taken
on the docs' word.

**0.1 — `chrome.webRequest` is completely broken in 43.2.0. VERIFIED.** Electron's own
documentation says "All features of this API are supported." Empirically
`chrome.webRequest.onBeforeRequest` is `undefined` and `addListener` throws
`Cannot read properties of undefined (reading 'addListener')`. The renderer logs
`NOTREACHED hit. Module resource registered as "webRequest" not found` and
`No source for require(webRequestEvent)`. The API schema is registered (`typeof chrome.webRequest
=== 'object'`) but the binding JS is absent from the shipped resource bundle. This reproduces
upstream issue [electron#52265](https://github.com/electron/electron/issues/52265) (labeled
`43-x-y`, `component/extensions`). §8.3.

**0.2 — The fix exists but is not in any released Electron. VERIFIED.**
[PR #52503](https://github.com/electron/electron/pull/52503)
(`build: add extensions_renderer_generated_resources.pak`) merged into the `43-x-y` branch on
2026-07-29. The newest published stable is **43.2.0, released 2026-07-21** — npm `dist-tags.latest`
is `43.2.0`, so we are already on the newest release and the fix ships in the *next* 43.x patch.
Blocking extensions cannot rely on `webRequest` today. §8.3.

**0.3 — Manifest V3 background service workers DO run. Electron's docs are wrong.** The supported
manifest-keys list marks `background` as "(Manifest V2)". Empirically an MV3
`background.service_worker` registers, executes, logs to `serviceWorkers`, writes to
`chrome.storage.local`, and appears in `ses.serviceWorkers.getAllRunning()` with its
`chrome-extension://` scriptUrl. MV3 is the viable target, not a fallback. §7.

**0.4 — Content blocking is possible, but only via DNR *dynamic* rules.**
`declarativeNetRequest` is entirely undocumented by Electron yet fully present, and a dynamic rule
**actually blocked a real subresource** end-to-end in an offscreen window. However, **static
rulesets declared in the manifest silently never load** — `getEnabledRulesets()` returns `[]` with
no warning and no error. Since shipping MV3 blockers (uBlock Origin Lite, AdGuard MV3) distribute
their filter lists *as static rulesets*, they will install cleanly and block nothing. §8.

---

## 1. Evidence base

Every row was produced by a command run on this machine during this mission. The harness lives in
the session scratchpad, not in the repo, and touches no owned path.

| Fact | Source | Value |
|---|---|---|
| Electron version under test | `node_modules/electron/package.json` + `dist/version` | `43.2.0` |
| Chromium | `process.versions.chrome` at runtime | reported by harness |
| Newest published Electron | `registry.npmjs.org/electron` `dist-tags.latest` | `43.2.0` (2026-07-21) |
| Extensions API typings | `electron.d.ts:8364-8460` (`class Extensions`) | `ses.extensions.*` |
| Deprecated aliases | `electron.d.ts:13101-13144` | `session.loadExtension` etc. |
| `LoadExtensionOptions` | `electron.d.ts:21963-21970` | one field: `allowFileAccess` |
| Official supported-API list | `raw.githubusercontent.com/electron/electron/v43.2.0/docs/api/extensions.md` | 178 lines |
| Upstream bug | GitHub API `repos/electron/electron/issues/52265` | closed *completed* 2026-07-30 |
| Fix PR | GitHub API `repos/electron/electron/pulls/52503` | merged 2026-07-29 into `43-x-y` |
| Electron license | `node_modules/electron/LICENSE` | MIT |

Harness invocation, for reproduction:

```
"/Users/adeebbashir/projects/blackglass/apps/engine/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron" <harness-dir>
```

Two environment concessions, both stated plainly because they bound the claims:

- The harness passes `--no-sandbox`. Chromium child processes cannot complete mach bootstrap under
  the agent Bash sandbox (`bootstrap_look_up … Permission denied`). **Production BlackGlass keeps
  the Chromium sandbox ON** (`apps/engine/src/main.js:111`). Extension *loading* is a browser-process
  operation and the renderer results below were reproduced consistently, but "identical behavior
  with `sandbox:true` in the packaged app" is **UNVERIFIED**.
- All probe windows used `offscreen: true`, matching the real engine, so the OSR interaction is
  covered rather than assumed.

### 1.1 A false lead, retracted

An early run showed page loads failing with `ERR_FAILED (-2)` after an extension page window was
destroyed, which looked like an extension teardown bug. **It was my harness, not Electron.**
Destroying the only window let `window-all-closed` quit the app. The control proved it: an
ordinary `http://` window reproduced the identical failure with no extension involved. With a
keeper window held open, destroying a `chrome-extension://` offscreen window is clean and the next
load succeeds. Recorded here so nobody re-derives the phantom.

---

## 2. The loading API

Electron 43 moved extensions onto `ses.extensions`; `session.loadExtension` still works but is
marked deprecated in the typings. Five constraints are hard, and three of them are enforced with
errors we captured verbatim.

`loadExtension(path, options?)` takes a **directory**. `.crx` is not supported — Electron never
implements CRX3 signature verification, so there is no publisher identity and no revocation path.
Loading is **per-session** and **not remembered**: nothing persists across boots, so the host must
re-load on every start. Calling it before `app.ready` throws. Loading into a non-persistent
partition throws `Extensions cannot be loaded in a temporary session` (measured), which means
**BlackGlass private/incognito sessions structurally cannot carry extensions** — that is a design
fact, not a policy choice. The only option is `allowFileAccess`, which additionally permits
injection into `file://` pages.

**Extension IDs are derived from the absolute directory path.** Our MV3 probe produced
`molbimeodpikohkpgglpedhmdjmocoal` on two separate runs from the same path, with the user-data
directory deleted and the manifest's permission list changed in between. Same path, same ID; a
different directory yields a different ID. Moving or renaming an extension folder therefore
rotates its identity and orphans its `chrome.storage.local` data.

---

## 3. What loads

| Manifest | Result | Evidence |
|---|---|---|
| MV3, unpacked | **loads** | `{"ok":true,"id":"molbi…","manifest_version":3}` |
| MV2, unpacked | **loads** | `{"ok":true,"id":"igiol…","manifest_version":2}` |
| MV2 + `webRequest` + event page | **rejected** | `The 'webRequest' API cannot be used with event pages.` |
| Non-persistent session | **rejected** | `Extensions cannot be loaded in a temporary session` |
| `.crx` file | **rejected** | `Extension directory not found: …/fake.crx` |

MV2 still loads in 43.2.0. It should not be the target: upstream Chromium has removed MV2 and the
capability will disappear on an Electron roll we do not control.

---

## 4. The real `chrome.*` surface, versus the docs

Enumerated by running `Object.keys(chrome)` inside a live `chrome-extension://` page. **The
documentation understates the surface substantially** — six namespaces exist that the docs never
mention. Everything below is what the runtime actually exposed.

| Namespace | In Electron docs? | Present at runtime | Note |
|---|---|---|---|
| `runtime` | yes | yes | broader than documented (`getContexts`, `getVersion`, `openOptionsPage`) |
| `scripting` | yes | yes | full surface |
| `storage` | yes | yes | keys `local`, `session`, `sync`, `managed` all present — but see §5 |
| `tabs` | yes (partial) | yes | `query` returned `count=1` |
| `extension` | yes | yes | only `getBackgroundPage`, `inIncognitoContext` |
| `management` | yes | **narrower** | only `getSelf`, `uninstallSelf`, `getPermissionWarningsByManifest` — docs claim `getAll`/`get` |
| `webRequest` | yes ("all features") | **broken** | schema only, no bindings — §8.3 |
| `declarativeNetRequest` | **no** | **yes, full** | the working blocking path — §8 |
| `action` | **no** | **yes** | badge/title/popup/icon setters |
| `alarms` | **no** | **yes** | |
| `i18n` | **no** | **yes** | `getMessage`, `getUILanguage`, `detectLanguage` |
| `idle` | **no** | **yes** | |
| `offscreen` | **no** | **yes** | |
| `power` | **no** | **yes** | |
| `clipboard` | **no** | **yes** | |
| `devtools.*` | yes | not enumerated here | only injected into devtools pages — **UNVERIFIED** |

Electron's docs warn that anything unlisted is "provisional and may be removed." The six
undocumented namespaces are therefore **usable but unsupported**: fine to exploit, unsafe to make
load-bearing without a pinned Electron and a regression test.

---

## 5. Permission acceptance

Electron warns on unrecognized permissions at load. Feeding it 24 permissions in one manifest maps
the boundary exactly.

**Accepted:** `storage`, `scripting`, `tabs`, `declarativeNetRequest`, `alarms`, `idle`,
`management`, `webRequest`, `unlimitedStorage`, `activeTab`, `clipboardRead`, `offscreen`,
`userScripts`, `power`.

**Rejected — `Permission '<x>' is unknown`:** `cookies`, `webNavigation`, `contextMenus`,
`notifications`, `downloads`, `history`, `bookmarks`, `proxy`, `privacy`, `sidePanel`, `favicon`,
`search`, `topSites`, `tts`.

Two traps. First, `webRequest` is *accepted* and then does not work (§8.3) — acceptance is not
support. Second, `userScripts` is accepted but `chrome.userScripts` never appeared in the
enumeration, so it is accepted-and-absent too. **Rejection is a warning, not an error**: an
extension needing `cookies` loads happily and fails at runtime, so BlackGlass must surface these
warnings rather than swallow them.

`chrome.storage.sync` deserves its own line because it is the most common silent breakage:
`chrome.storage.local.set` returned `ok`, while `chrome.storage.sync.set` returned
`"sync" is not available in this instance of Chrome`. The key *exists* on the object, so
feature-detection via `if (chrome.storage.sync)` passes and the write then fails.

---

## 6. Content scripts

**Working, including under offscreen rendering.** A `document_start` content script matching
`<all_urls>` injected into a real `http://127.0.0.1` page inside an `offscreen: true` window and
its DOM mutation was read back (`data-bg-cs = "ran"`). This is the single most valuable extension
capability for BlackGlass and it is solid.

`file://` injection needs `loadExtension(path, { allowFileAccess: true })`.

---

## 7. Manifest V3 service workers

**Working — contradicting Electron's own manifest-key table.** A `background.service_worker`:

```
sw_console                 : "BG_SW_ALIVE"
sw_running_after_load      : { "0": { "scriptUrl": "chrome-extension://ggkhe…/sw.js",
                                       "scope": "chrome-extension://ggkhe…/",
                                       "renderProcessId": 4 } }
sw_ever_ran_storage        : {"swRan":true}
```

The worker ran, logged through `ses.serviceWorkers`, and its `chrome.storage.local` write was read
back from a separate extension page.

One timing caveat worth propagating: an earlier probe reported *no* running workers because it
sampled `getAllRunning()` too late — MV3 workers idle out and stop, exactly as in Chrome. Liveness
must be judged by observable effects, never by a point-in-time `getAllRunning()`. Allow roughly
2–3 s after `loadExtension` before assuming a worker has done its startup work.

---

## 8. Content blocking — the decisive question

The main reason a terminal browser wants extensions is blocking. The answer is nuanced.

### 8.1 DNR dynamic rules — WORKS

With the rule registered from the service worker and a **single** page window (no teardown
confound):

```
sw_console    : "SW_READY dynamic=set staticEnabled=[]"
page_load     : "OK"
dnr_effective : "BLOCKED"
```

The subresource was genuinely blocked — the page's `window.__blockme` was never defined.
`updateDynamicRules` returned `ok`, `getDynamicRules` read the rule back, and `testMatchOutcome`
independently confirmed the match (`{"matchedRules":[{"ruleId":99,"rulesetId":"_dynamic"}]}`).
**This is a real, working ad-blocking primitive.**

### 8.2 DNR static rulesets — SILENTLY DO NOT LOAD

The same extension declared `declarative_net_request.rule_resources` with `"enabled": true`.
`getEnabledRulesets()` returned `[]` in every run, with **no load warning and no error**. An
earlier isolation run confirmed a static-only extension blocked nothing (`NOT_BLOCKED`) while the
page loaded fine.

The consequence is specific and severe: MV3 content blockers ship their filter lists as static
rulesets. Such an extension will load, report healthy, show its badge, and block **nothing**.
BlackGlass must detect this rather than let users conclude blocking is on.

### 8.3 `chrome.webRequest` — BROKEN

```
NOTREACHED hit. Module resource registered as "webRequest" not found
NOTREACHED hit. Module resource registered as "webRequestEvent" not found
sw_console : "No source for require(webRequest)"
sw_console : "SW_WREQ listener_FAILED Cannot read properties of undefined (reading 'addListener')"
result     : {"listenerErr":"Cannot read properties of undefined (reading 'addListener')","wrCount":0}
```

Zero requests observed. The `NOTREACHED` is non-fatal in this release build — page loads still
succeeded and content scripts still ran — so this degrades rather than crashes, which makes it
*harder* to notice. Matches upstream #52265; fixed by #52503, merged 2026-07-29, **unreleased**.

Note that Electron's own `session.webRequest` (the module we already control from
`apps/engine/src/main.js`) is a **separate, working** API and takes precedence over the extension
one. BlackGlass's first-party blocking should be built on it and does not depend on any of this.

---

## 9. Chrome services that simply do not exist

These are not bugs and will not be fixed; they are Google services, not Chromium code.

**Chrome Web Store**: no install path, no store UI, no CRX3 verification, no update manifest
polling. **Chrome Sync**: `chrome.storage.sync` throws; no account, no cross-device state.
**Extension auto-update**: nothing pulls new versions — whatever is on disk runs forever.
**Identity/OAuth** (`chrome.identity`), **cookies**, **webNavigation**, **contextMenus**,
**notifications**, **downloads**, **history**, **bookmarks**, **proxy**, **privacy**,
**sidePanel**, **topSites**: all rejected as unknown permissions (§5). **Enterprise policy**
(`chrome.storage.managed`) has no policy provider behind it.

Electron states plainly that arbitrary Web Store extensions are a **non-goal**. Any BlackGlass
roadmap promising "Chrome extensions work" is promising something upstream has declined to build.

---

## 10. Security implications of loading unpacked extensions

This is where the mission matters most, because the terminal context makes several of these worse
than in a desktop browser.

**There is no consent step.** Chrome shows an install prompt enumerating requested host
permissions. `loadExtension()` shows nothing — it loads. If BlackGlass exposes an
`extensions load <dir>` command, the *entire* permission-consent story is ours to build. A
terminal browser has no puzzle-piece toolbar to fall back on.

**Unpacked means unverifiable and mutable.** No signature, no publisher identity, no integrity
check at load and none between loads. Because `.crx` is unsupported, there is no code-signing path
even in principle. Any process that can write the extension directory — a malicious `npm
postinstall`, a compromised dotfiles repo, another agent on the box — silently owns the browser on
next start. On a 98%-full shared dev machine this is not hypothetical.

**A content script with `<all_urls>` is total compromise of browsing.** It reads and rewrites every
page: session tokens, form values, one-time codes. It is *verified working* (§6), so this is the
capability we are actually shipping. `chrome.scripting` compounds it with arbitrary injection into
arbitrary tabs.

**No auto-update means vulnerabilities are permanent.** A blocker with a known RCE stays exploitable
until a human notices. There is no revocation channel and nothing phones home.

**The blast radius is bounded in exactly two useful ways**, both of which we should keep. Extensions
inherit the engine's `webPreferences` posture — `nodeIntegration: false`, `contextIsolation: true`,
`sandbox: true` (`apps/engine/src/main.js:109-111`) — so extension code does **not** get Node.
And because extensions are per-session and never persisted, every boot is an explicit re-grant:
the default state is "no extensions," which is the right default and should not be optimized away
with a convenience auto-loader.

**Private sessions are safe by construction** — non-persistent partitions cannot load extensions at
all (§2), so a tracking extension cannot follow a user into private browsing.

**Licensing (rule 4):** Electron is MIT (`node_modules/electron/LICENSE`), so the loading machinery
is unencumbered. Bundling third-party extensions is a different question — uBlock Origin and
uBlock Origin Lite are **GPLv3**, which is incompatible with shipping them inside a proprietary
BlackGlass binary. Loading a user-supplied directory at runtime carries no such obligation.
**Load, do not bundle.**

---

## 11. What we CAN and CANNOT support

Stated flatly, because the mission asked for exactly this.

**CAN** — verified on this machine: load unpacked MV3 (and, for now, MV2) extensions into a
persistent session; content scripts including `document_start` under offscreen rendering; MV3
background service workers; `chrome.storage.local`/`.session`; `chrome.scripting`;
`chrome.tabs` (partial — `query`, `update`, `reload`, `sendMessage`); `chrome.runtime` messaging;
`chrome.action` state; `alarms`, `i18n`, `idle`, `offscreen`, `power`, `clipboard`; and network
blocking through `declarativeNetRequest` **dynamic/session** rules.

**CANNOT** — verified or structural: Chrome Web Store or `.crx` installs; extension auto-update or
signature verification; `chrome.storage.sync`; `chrome.webRequest` (until the next 43.x patch);
DNR **static** rulesets from the manifest; extensions in private/non-persistent sessions;
`cookies`, `webNavigation`, `contextMenus`, `notifications`, `downloads`, `history`, `bookmarks`,
`proxy`, `privacy`, `sidePanel`, `topSites`, `identity`; and any promise of general Chrome Web
Store compatibility.

**Honest framing for users:** BlackGlass supports *developer-mode, user-supplied, unpacked
extensions* — a useful subset, roughly what Electron built for DevTools extensions plus a working
DNR blocker. It does not support "Chrome extensions."

---

## 12. Recommendation

**Do not ship extension loading in v1. Ship first-party blocking on `session.webRequest` instead.**

The reasoning is that every user-visible benefit of extensions in v1 is ad/tracker blocking, and
the extension route to it is currently the worst of the three options: `chrome.webRequest` is
broken until an unreleased patch, DNR static rulesets — the form every real blocker ships — silently
do nothing, and the working path (DNR dynamic rules) requires us to parse filter lists and inject
them ourselves. If we are converting filter lists either way, converting them into Electron's own
`session.webRequest` (already in our process, already working, unaffected by all of §8) is strictly
less machinery and carries none of the §10 security surface.

Extension support then becomes a v2 feature landing on a 43.x that fixes #52265, behind an explicit
opt-in with a real consent prompt.

---

## 13. Instructions for the commander (not applied)

1. **Pin Electron exactly.** `apps/engine/package.json` currently has `"electron": "^43.2.0"`. The
   caret will silently pull the patch that changes `chrome.webRequest` from broken to working —
   behavior a lockfile-only bump should never smuggle in. Pin `43.2.0` and bump deliberately.
2. **Watch for the 43.x patch** carrying PR #52503, then re-run the §8.3 probe before believing
   `webRequest` works.
3. **Never swallow `ExtensionLoadWarning`.** If extension loading ships, surface unknown-permission
   warnings to the user — they are the only signal that an extension will fail at runtime.
4. **Add a static-ruleset guard.** After `loadExtension`, if the manifest declares
   `declarative_net_request.rule_resources` but `getEnabledRulesets()` is empty, warn loudly that
   the extension is loaded but blocking nothing (§8.2).
5. **Keep the security posture** at `apps/engine/src/main.js:109-111` unchanged; it is what keeps
   Node away from extension code.
6. **Do not bundle GPLv3 blockers** into the binary (§10).

---

## 14. Threats to validity

Measured on macOS 26.1 / Apple M4 with Electron 43.2.0 only; Linux and Windows are **UNVERIFIED**.
The harness ran with `--no-sandbox` for the reason in §1, so behavior under the production
`sandbox: true` posture is **UNVERIFIED**, though extension loading is browser-process work and no
result here depended on renderer sandbox state. `chrome.devtools.*`, `devtools_page`, extension
popup rendering through `chrome.action`, and `chrome.userScripts` were **not** exercised. No real
third-party extension (uBlock Origin Lite, React DevTools) was installed — the §8.2 conclusion
about shipping blockers is an inference from the static-ruleset measurement, and should be
confirmed against an actual blocker before it is quoted externally. Screenshot verification was
unavailable (lock screen); all evidence is protocol- and log-based, which is CI-able.
