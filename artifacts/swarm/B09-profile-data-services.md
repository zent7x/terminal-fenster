# B09 — Profiles, Data Services, Downloads & Private Sessions

**Mission:** Design profiles, cookies, cache, history, bookmarks, downloads and private sessions
on top of Electron sessions. Cover partition strings and `persist:` semantics, where Electron
stores data on macOS, `safeStorage` for secrets, download interception via `will-download`, and
the `com.apple.quarantine` xattr we must set on downloads — with the exact command and API.

**Owned output:** this file only. Every change to a core file (`apps/engine/src/main.js`,
`apps/cli/**`, `crates/**`) is **specified here as an instruction for the commander**, not
applied. See §16.

---

## 0. Headline findings

Three results from this mission change the design. All three were measured on this machine, not
recalled.

**0.1 — Electron does not quarantine downloads. VERIFIED.** A file downloaded end-to-end through
Electron 43.2.0's own download stack on macOS 26.1 carries **no** `com.apple.quarantine`
attribute. Gatekeeper therefore never evaluates it, and the "downloaded from the internet"
provenance chain is silently absent. Every other browser on this machine sets it (Safari, Chrome,
even QuickTime). **BlackGlass must set it itself.** This is a security regression against every
mainstream browser and it is invisible until someone downloads a malicious binary. §12.

**0.2 — A missing `setSavePath` is a hang, not a dialog.** Electron's documented fallback when
the app does not set a save path is to "prompt a save dialog". BlackGlass renders offscreen into
a terminal; a modal `NSSavePanel` is a window the user cannot see and, at a lock screen, cannot
dismiss. `setSavePath` is not an optimization here — it is a liveness requirement. §11.3.

**0.3 — `node:sqlite` ships in Electron 43. VERIFIED.** Node 24.18.0 is embedded and
`require('node:sqlite')` resolves. History, bookmarks and the download ledger get a real
transactional store with **zero npm dependencies and no native build** — which matters directly
given the 98%-full disk. §9.

---

## 1. Evidence base

Everything in this table was produced by a command run on this machine during this mission.
`electron.d.ts` is the type definition shipped inside the installed `electron@43.2.0` package —
it is the authoritative API surface for exactly the version we run, not documentation for some
other release.

| Fact | Source | Value |
|---|---|---|
| Electron / Chromium / Node | probe, `process.versions` | `43.2.0` / `150.0.7871.129` / `24.18.0` |
| Host OS | `sw_vers` | macOS `26.1`, build `25B78` |
| `Session.fromPartition(partition, options?)` | `electron.d.ts:12314` | returns existing session for same string |
| `persist:` prefix ⇒ on-disk; no prefix ⇒ in-memory; empty ⇒ default session | `electron.d.ts:12305-12308` | exact doc text |
| Session options are **immutable after first use** | `electron.d.ts:12310-12312` | "no way to change the options of an existing Session" |
| `Session.fromPath(absPath, options?)` exists in 43 | `electron.d.ts:12326` | throws on relative/empty path |
| `FromPartitionOptions` / `FromPathOptions` = `{ cache: boolean }` only | `electron.d.ts:21712-21725` | default `true` |
| `session.on('will-download', (event, item, webContents))` | `electron.d.ts:12839-12841` | `preventDefault()` cancels |
| `item.setSavePath(path)`, dirs created recursively | `electron.d.ts:8298-8303` | only valid inside `will-download` |
| No `setSavePath` ⇒ "usually prompts a save dialog" | `electron.d.ts:8300-8302` | see §0.2 |
| `item.getState()` ∈ `progressing\|completed\|cancelled\|interrupted` | `electron.d.ts:8250` | — |
| `item.getURLChain()`, `getStartTime()`, `hasUserGesture()` | `electron.d.ts:8258-8267` | redirect chain + gesture available |
| `session.downloadURL(url, options?)` | `electron.d.ts:12950` | programmatic download entry point |
| `session.setDownloadPath(path)` | `electron.d.ts:13234` | session-wide default dir |
| `session.clearData(options?)` + `ClearDataOptions.dataTypes` | `electron.d.ts:12904`, `20580-20586` | `cache,cookies,downloads,localStorage,indexedDB,serviceWorkers,fileSystems,backgroundFetch,webSQL` |
| `clearData` supports `origins` / `excludeOrigins` (mutually exclusive) | `electron.d.ts:20588-20596` | per-site forget |
| `session.clearStorageData(options?)` (older, origin-scoped) | `electron.d.ts:12923` | different storage-type spelling |
| `safeStorage.isEncryptionAvailable()` — macOS ⇒ "Keychain available" | `electron.d.ts:11888-11893` | measured `true` |
| `safeStorage` roundtrip works; ciphertext tagged `v10` | probe | 19-byte plaintext ⇒ 35-byte ciphertext |
| Keychain item created: svce `<appName> Safe Storage`, acct `<appName> Key` | `security find-generic-password` | measured |
| `app.getPath('sessionData' \| 'userData' \| 'downloads' \| …)` | `electron.d.ts:1299` | `sessionData` is the profile root |
| `webContents.navigationHistory.getAllEntries()` → `{url,title,pageState?}` | `electron.d.ts:10091-10133` | per-tab only, not global history |
| `node:sqlite` available | probe | `DatabaseSync,StatementSync,Session,constants,backup` |
| `/usr/bin/xattr` is a native Mach-O universal binary (**not** a Python script) | `file /usr/bin/xattr` | 118896 bytes, x86_64 + arm64e |
| `setxattr(2)` signature | `man 2 setxattr` | `(path, name, value, size, position, options)` |
| Quarantine value stored as raw ASCII, **no NUL terminator** | `ls -l@`, `xattr -px` | 61-char string ⇒ 61 bytes |
| Quarantine **survives `rename(2)`** | measured | value byte-identical after `mv` |
| Quarantine is **rewritten by `cp`** (agent cleared, flags `0083`→`0283`) | measured | do not use `cp` in the download path |
| Electron download has **no** `com.apple.quarantine` | measured | only kernel `com.apple.provenance` |
| Electron is MIT licensed | `node_modules/electron/LICENSE` | reuse of API patterns is unencumbered |

### 1.1 Observed real-world quarantine values on this machine

Sampled from `~/Downloads` with `xattr -p com.apple.quarantine`:

| Writer | Value |
|---|---|
| Safari | `0083;6a61937d;Safari;3A12871C-D839-4332-B0C8-119AA4694250` |
| Chrome | `0281;6a409f98;Chrome;61AAE6CF-8590-48E0-B02C-8001AC33E4D0` |
| sharingd (AirDrop) | `0081;6a665708;sharingd;513AB110-EDE2-4DB6-BC73-D210D1FE17EB` |
| QuickTime Player | `0082;6a5798a1;QuickTime Player;` (empty UUID field) |

Timestamps decode as lowercase hex seconds since the Unix epoch — verified:
`0x6a665708 = 1785091848 = 2026-07-27T00:20:48`.

> **UNVERIFIED:** the meaning of the individual flag bits (`0081` vs `0083` vs `0281`). Apple does
> not document `com.apple.quarantine`, and the community reverse-engineered tables I can recall do
> not reconcile with the values above (they would imply Safari sets "user approved" on a fresh
> download, which is implausible). I therefore do **not** assert bit semantics. §12.2 recommends
> copying Safari's `0083` verbatim on the empirical grounds that it is what the system browser
> writes on this exact OS build, and gives the commander a check to confirm behaviour.

---

## 2. Design constraints specific to BlackGlass

These are the things that make a terminal browser's data layer different from a GUI browser's,
and they drive most of the decisions below.

**No GUI affordances.** Any Chromium/AppKit code path that ends in a modal window is a hang, not a
prompt. This rules out the default download-path behaviour (§11.3) and any permission flow that
falls through to a native dialog (§14.3).

**The engine is a subordinate process.** `apps/engine/src/main.js:11-14` states the engine "opens
no listening port of its own" and only connects to a socket path handed to it by the Rust core.
Data-layer decisions must not invert that: the engine should not independently decide where the
profile lives. The core owns policy, the engine executes it. Concretely, the profile root arrives
as a launch argument, exactly like `--bg-socket=`.

**Sessions are immutable once used.** `electron.d.ts:12310-12312` is unambiguous: you cannot change
a session's options after it exists. Profile selection therefore happens **before** the first
`BrowserWindow` is constructed, and switching profiles means a new engine process, not a mutation.
This is the single most consequential API constraint in this document.

**One engine process per profile is the honest model.** Electron can hold several sessions in one
process, but a private session that shares a process with a persistent one shares a Chromium
network service, a GPU process and a crash domain. For a browser whose entire pitch is "a real
browser in your terminal", the cheap and defensible isolation boundary is the OS process. §3.3.

**Disk is at 98%.** Cache sizing is not a footnote here. A default Chromium HTTP cache will grow
into the low hundreds of MB. §8 sets an explicit cap.

---

## 3. Profile model

### 3.1 The three kinds of session

| Kind | Partition string | Disk | Survives restart | Use |
|---|---|---|---|---|
| **Named profile** | `persist:p-<slug>` | yes | yes | the normal case; `default`, `work`, … |
| **Private window** | `bg-private-<nonce>` (no prefix) | no | no | incognito equivalent |
| **Ephemeral task** | `bg-task-<nonce>` (no prefix) | no | no | one-shot scripted fetches |

The `persist:` prefix is the entire mechanism. From `electron.d.ts:12305-12308`: a partition
starting with `persist:` is backed by disk and shared by every page using the same string; without
the prefix the session is in-memory; an empty string returns the app's default session.

**Rule: BlackGlass never uses the default session for web content.** The default session is the one
Chromium also uses for internal fetches, and it writes directly into the profile root rather than
into `Partitions/`, which makes "delete this profile" a messier operation. Every window gets an
explicit partition string. This costs nothing and buys a clean `rm -rf` story (§15.2).

### 3.2 Slug rules

The partition substring after `persist:` becomes a **directory name on disk** (measured, §5). It
is therefore attacker-relevant if profile names are ever taken from anything but the user's own
shell argument. Constrain profile slugs to `[a-z0-9][a-z0-9_-]{0,31}` and reject `.` and `..`
outright. A profile named `../../../../etc` must not be expressible.

### 3.3 Process topology

```
blackglass (Rust core, owns TTY)
  └── engine process, --bg-profile=work      → session persist:p-work
  └── engine process, --bg-private           → session bg-private-<nonce>
```

One engine per profile. The core already manages engine lifetime and the socket; extending it to a
map of `profile → engine` is a smaller change than teaching one engine to host mutually distrusting
sessions. A private engine additionally gets a scrubbed environment and no profile root argument at
all, so it *cannot* write to a profile even if it has a bug.

### 3.4 Where the profile root comes from

The core passes `--bg-profile-root=<abs path>` and the engine calls `app.setPath('sessionData', …)`
before `app.whenReady()`. `sessionData` (not `userData`) is the correct knob: `electron.d.ts:1299`
lists it as a distinct path, and it is the directory Chromium hangs `Partitions/`, `Cookies` and
`Cache` off. Overriding `sessionData` alone relocates browsing data while leaving app-level state
(crash dumps, `Local State`) where it belongs.

Default root: `~/Library/Application Support/BlackGlass`. Honour `$BLACKGLASS_HOME` if set, since a
terminal-native tool should be scriptable and testable without touching the user's real profile.

---

## 4. Partition strings and `persist:` semantics — the sharp edges

**Same string means the same session object.** `fromPartition` is a lookup, not a constructor
(`electron.d.ts:12303-12304`). Two windows opened with `persist:p-work` share cookies, cache,
localStorage and permission grants. That is the point, but it also means a typo silently merges two
profiles instead of failing.

**Options only apply on first creation.** `fromPartition('persist:x', { cache: false })` is a no-op
if `persist:x` already exists in this process. There is no error. Set options at the single point
where the session is first created and never pass options anywhere else — otherwise the codebase
grows a call whose behaviour depends on module import order.

**`cache: false` is the only option.** `FromPartitionOptions` has exactly one field
(`electron.d.ts:21712-21718`). Everything else — proxy, user agent, permissions, downloads — is a
method call on the returned `Session`, applied after creation.

**`fromPath` is the escape hatch.** New enough to be worth knowing about: `Session.fromPath(abs)`
(`electron.d.ts:12326`) creates a session rooted at an arbitrary absolute directory, bypassing the
`Partitions/<slug>` naming scheme. It throws on a relative or empty path. This is attractive for a
`--profile-dir=/some/path` flag. **Recommendation: do not use it in v1.** It moves path
construction from a slug we validate into a path the user supplies, and it interacts badly with the
"delete this profile" story. Keep `fromPartition` and one validated slug.

**In-memory sessions still touch disk.** A non-`persist:` session keeps cookies and localStorage in
memory, but Chromium may still write to the shared `Cache/` if caching is on, and blob/media
temporaries land in the OS temp dir. Private sessions must therefore be created with
`{ cache: false }` — this is the one place the option genuinely matters. §14.

---

## 5. Where Electron stores data on macOS — measured

Produced by running a probe app named `qtest` that used partition `persist:qtest`, then walking the
tree. This is observed layout, not documentation.

```
~/Library/Application Support/<appName>/          ← app.getPath('userData')
├── Local State                                   ← JSON, app-level
│                                                   measured content:
│                                                   {"uninstall_metrics":{"installation_date2":"1785515112"}}
└── Partitions/
    └── <slug>/                                   ← the string after "persist:"
        ├── Cookies                               ← SQLite 3 database
        ├── Cookies-journal
        ├── Preferences                           ← JSON, per-partition
        ├── Network Persistent State              ← HTTP/2, QUIC, HSTS
        ├── Trust Tokens  /  Trust Tokens-journal
        ├── Cache/
        │   ├── Cache_Data/                       ← HTTP cache, the size driver
        │   └── No_Vary_Search/
        ├── Code Cache/{js,wasm}/                 ← compiled script cache
        ├── Local Storage/leveldb/
        ├── Shared Dictionary/{cache,db,db-journal}
        └── blob_storage/<uuid>/
```

Notes that matter:

- **`Cookies` is a SQLite 3 database** (`file` confirms schema 4, UTF-8). Cookie *values* are
  encrypted at rest by Chromium's `os_crypt` using the same Keychain key as `safeStorage` (§13).
- **The default session does not live under `Partitions/`.** It writes `Cookies`, `Cache`, etc.
  directly into the profile root. This is the concrete reason for the §3.1 rule — with named
  partitions only, `Partitions/<slug>` is a complete, self-contained, deletable profile.
- `IndexedDB/`, `Service Worker/` and `Session Storage/` are absent above only because the probe
  page never used them; they are created lazily as siblings.
- The Keychain item is **also created lazily** — it did not exist after a plain download, and
  appeared only once `safeStorage.encryptString` ran.

### 5.1 BlackGlass layout

```
$BLACKGLASS_HOME (default ~/Library/Application Support/BlackGlass)/
├── profiles.json                 ← profile registry: slug, display name, created, last used
├── bg.sqlite                     ← OUR data: history, bookmarks, download ledger (§9, §10, §11.7)
├── secrets.bin                   ← safeStorage ciphertext blob (§13)
└── chromium/                     ← app.setPath('sessionData', …) points here
    ├── Local State
    └── Partitions/
        ├── p-default/
        └── p-work/
```

Chromium owns everything under `chromium/`; we never parse or write it. Everything we own is in
`bg.sqlite` and `profiles.json`. That separation is what makes a Chromium version bump a non-event.

---

## 6. Session wiring

Specification for the commander. Ordering is load-bearing: `setPath` must precede `whenReady`, and
session configuration must precede the first `BrowserWindow`.

```js
// --- launch args (engine) ---
//   --bg-profile-root=<abs>   absent ⇒ private mode, no disk
//   --bg-profile=<slug>       validated by the CORE, re-validated here
//   --bg-private              explicit private flag

const PROFILE_ROOT = arg('--bg-profile-root=');
const PROFILE_SLUG = arg('--bg-profile=');
const PRIVATE      = process.argv.includes('--bg-private') || !PROFILE_ROOT;

// Defence in depth: the core validates, we validate again. A slug that reaches
// the filesystem unchecked is a directory-traversal primitive.
const SLUG_RE = /^[a-z0-9][a-z0-9_-]{0,31}$/;
if (!PRIVATE && !SLUG_RE.test(PROFILE_SLUG)) {
  console.error('[engine] fatal: invalid profile slug');
  process.exit(2);
}

// MUST run before app.whenReady().
if (!PRIVATE) app.setPath('sessionData', path.join(PROFILE_ROOT, 'chromium'));

app.whenReady().then(() => {
  const partition = PRIVATE
    ? `bg-private-${crypto.randomBytes(8).toString('hex')}`   // no persist: ⇒ memory only
    : `persist:p-${PROFILE_SLUG}`;

  // cache:false only for private. Options are ignored if this partition already
  // exists in-process, so this is the ONLY place fromPartition is called with options.
  const ses = session.fromPartition(partition, PRIVATE ? { cache: false } : undefined);

  configureSession(ses);           // §7, §8, §11, §14.3
  createOffscreenWindow(ses);      // webPreferences.session = ses
});
```

Attach the session to content via `webPreferences.session` (the object) rather than
`webPreferences.partition` (the string). Passing the object makes it impossible for the window to
silently create a *second* session from a mistyped string, and it keeps the "created in exactly one
place" invariant.

---

## 7. Cookies

Chromium already does the hard parts — SameSite, partitioning, expiry, encryption at rest. We need
a policy and three operations.

**Policy.** Named profiles persist cookies. Private sessions hold them in memory and lose them on
exit, which requires no code. Third-party cookie blocking is a Chromium-level setting we do not get
a clean Electron switch for; do not claim we block them until measured.

**Durability.** Chromium flushes the cookie store lazily. A terminal browser is killed with
`Ctrl-C` far more often than a GUI browser is quit cleanly, so an unflushed store is a realistic
data-loss path. Call `ses.cookies.flushStore()` (`electron.d.ts:7327`) on a debounced timer (≈5 s
after the last cookie change) and unconditionally in the shutdown path.

**Operations.** `ses.cookies.get(filter)` for a `bg cookies list` command,
`ses.cookies.remove(url, name)` for targeted deletion, and `clearData({ dataTypes:['cookies'],
origins:[…] })` for per-site forget (§15.1).

**Do not read `Cookies` directly.** It is SQLite and it is tempting. The values are `os_crypt`-
encrypted and the schema is Chromium's to change. Use the API.

---

## 8. Cache

**Enabled for named profiles, disabled for private.** `{ cache: false }` at creation is the only
supported way to turn it off (§4).

**Cap it.** Disk is at 98% with ~9 GiB free; an uncapped Chromium cache is a real liability on this
machine. Chromium accepts `--disk-cache-size=<bytes>` as a command-line switch, applied via
`app.commandLine.appendSwitch('disk-cache-size', String(64 * 1024 * 1024))` before `whenReady`.
Recommend **64 MiB**.

> **UNVERIFIED:** that `--disk-cache-size` is still honoured in Chromium 150 via Electron's
> `commandLine`. It is a long-standing switch, but I did not measure steady-state cache size against
> it in this mission. §17 gives the check: fill the cache, then `du -sh Partitions/<slug>/Cache`.

**Report it, don't hide it.** `bg profile info` should show cache bytes on disk from `du`, and
`bg profile clear-cache` maps to `ses.clearCache()` (`electron.d.ts:12876`).

---

## 9. History — we own this

Chromium's History database lives in `//chrome`, not `//content`. Electron does not embed it and
exposes no equivalent. `webContents.navigationHistory.getAllEntries()` (`electron.d.ts:10133`)
returns `{url, title, pageState?}` for **one tab's back/forward list only** — it is not global
history and it dies with the tab.

So BlackGlass implements history itself, from navigation events.

**Storage: `node:sqlite`.** Verified available in Electron 43 (§0.3). Zero npm dependencies, no
native compile, transactional. Given the disk constraint and the project's minimal-dependency
posture, this is strictly better than adding `better-sqlite3` (native build) or hand-rolling JSONL
compaction.

```sql
CREATE TABLE IF NOT EXISTS visits (
  id        INTEGER PRIMARY KEY,
  profile   TEXT NOT NULL,
  url       TEXT NOT NULL,
  title     TEXT,
  visited_at INTEGER NOT NULL,          -- ms since epoch
  transition TEXT NOT NULL              -- 'typed' | 'link' | 'reload' | 'in-page'
);
CREATE INDEX IF NOT EXISTS visits_time    ON visits(profile, visited_at DESC);
CREATE INDEX IF NOT EXISTS visits_url     ON visits(profile, url);
```

**Capture points.** `did-navigate` (main-frame commit) is the primary signal;
`did-navigate-in-page` records SPA transitions with `transition='in-page'`;
`page-title-updated` (`electron.d.ts:4563`) backfills the title on the most recent visit row for
that tab, because the title is not known at commit time.

**Rules.**
- **Private sessions write nothing.** Enforce this by not passing the DB handle into a private
  engine at all, rather than by an `if` at the call site. A missing capability cannot be bypassed
  by a later bug.
- Never record credentials embedded in a URL. Strip `user:pass@` from the authority before insert.
- Do not record `about:`, `devtools:` or `data:` URLs. `data:` URLs in particular can be megabytes.
- Cap the stored URL at 2048 bytes and the title at 512.

---

## 10. Bookmarks

Small, user-authored, and the one dataset whose loss would actually upset someone. Same
`bg.sqlite`, but treated as precious rather than derived.

```sql
CREATE TABLE IF NOT EXISTS bookmarks (
  id         INTEGER PRIMARY KEY,
  profile    TEXT NOT NULL,
  url        TEXT NOT NULL,
  title      TEXT NOT NULL,
  folder     TEXT NOT NULL DEFAULT '',
  position   INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  UNIQUE(profile, url, folder)
);
```

`folder` as a flat path string (`dev/rust`) rather than a parent-id tree. A terminal UI renders a
flat, filterable list far better than a tree, and it avoids an entire class of orphaned-node bugs.

**Import/export in Netscape bookmark HTML format**, because it is what every browser reads and
writes, and it means BlackGlass is never a roach motel for a user's data. Export must
HTML-escape titles and URLs — a bookmark title is attacker-controlled text if it was captured from
a page's `<title>`.

Bookmarks are per-profile, and private sessions may **read** the current profile's bookmarks but
must not write them. Reading is a deliberate exception to §9's rule: a private window that cannot
open your bookmarks is useless, and reading leaks nothing to disk.

---

## 11. Downloads

### 11.1 Interception

`will-download` fires on the **session** (`electron.d.ts:12839-12841`) with
`(event, item, webContents)`. `event.preventDefault()` cancels the download, after which the docs
warn `item` is unavailable from the next tick — so any inspection must happen synchronously in the
handler.

### 11.2 The handler must be synchronous

`setSavePath` "is only available in session's `will-download` callback function"
(`electron.d.ts:8299-8302`). There is no way to await a user's decision in the terminal and then set
the path. Therefore: **decide the path synchronously from policy, always.** If BlackGlass wants to
prompt the user, the correct shape is to accept the download to a staging path immediately and
prompt afterwards about whether to keep it — never to hold the callback open.

### 11.3 A missing save path is a hang

Restating §0.2 because it is the failure that will actually bite. If `setSavePath` is not called,
Electron falls back to its "original routine", which "usually prompts a save dialog"
(`electron.d.ts:8300-8302`). In an offscreen terminal browser that is an invisible modal window.
Mitigate twice: call `ses.setDownloadPath(dir)` (`electron.d.ts:13234`) as a session-wide backstop,
**and** call `item.setSavePath()` on every item. Belt and braces, because the failure mode is a hung
browser with no visible cause.

### 11.4 Filename safety

`item.getFilename()` is attacker-controlled. Sanitize before it touches the filesystem:

- Strip `/`, `\`, and NUL. Reject `.` and `..` outright.
- Strip C0/C1 control characters.
- **Strip Unicode bidirectional overrides** — `U+202E` and friends are the classic download-spoof
  trick that renders `exe.txt` as `txt.exe`. Also strip `U+200B`–`U+200F`, `U+2066`–`U+2069`.
- Normalize to NFC (APFS is normalization-preserving but not normalization-insensitive across all
  paths; NFC avoids duplicate-looking names).
- Truncate to 255 **bytes** in UTF-8, not characters, preserving the extension.
- Empty after sanitizing ⇒ `download`.
- Resolve collisions as `name (1).ext`, `name (2).ext`, capped at 999 then fail loudly.

Finally, `realpath` the parent directory and assert the result is still inside the download root
before writing. Sanitizing the name and checking the resolved path are different defences; do both.

### 11.5 The staging-and-rename flow

This is the core of the download path, and it exists to close the window in which a complete,
unquarantined file sits at its final name.

```
1. will-download fires
2. sanitize filename                              → "installer.dmg"
3. setSavePath(<downloads>/installer.dmg.bgpart)  ← staging name, in the SAME directory
4. Chromium writes bytes to the .bgpart file
5. item 'done' with state === 'completed'
6. set com.apple.quarantine on the .bgpart file   ← §12
7. rename(.bgpart → installer.dmg)                ← atomic; xattr survives (VERIFIED)
8. record in the download ledger
```

Steps 6 and 7 are in this order deliberately. `rename(2)` is atomic within a directory and
**preserves extended attributes** — measured: the value was byte-identical after `mv`. So the file
never appears at its final name without quarantine already attached.

Staging must be in the **same directory** as the final file, or the rename crosses a filesystem
boundary and degrades to copy-and-delete — which, as measured, *mutates* the quarantine value
(agent name cleared, flags `0083` → `0283`). Never use `cp`, `fs.copyFile`, or a temp dir on
another volume anywhere in this path.

On `state !== 'completed'`, unlink the `.bgpart` file and record the failure.

### 11.6 Progress reporting

`item.on('updated', …)` with `getReceivedBytes()` / `getTotalBytes()` (`getTotalBytes()` returns 0
when the length is unknown — render a byte counter, not a bogus 0 % bar). Throttle progress events
to ≈4 Hz before putting them on the wire; the frame path is the latency-critical one and download
progress must not compete with it.

### 11.7 Ledger

```sql
CREATE TABLE IF NOT EXISTS downloads (
  id          INTEGER PRIMARY KEY,
  profile     TEXT NOT NULL,
  url         TEXT NOT NULL,
  final_url   TEXT NOT NULL,          -- last entry of getURLChain()
  filename    TEXT NOT NULL,
  save_path   TEXT NOT NULL,
  bytes       INTEGER NOT NULL,
  mime        TEXT,
  state       TEXT NOT NULL,          -- completed | cancelled | interrupted
  user_gesture INTEGER NOT NULL,      -- hasUserGesture()
  quarantined INTEGER NOT NULL,       -- 1 = xattr set and verified
  started_at  INTEGER NOT NULL,
  finished_at INTEGER
);
```

Store the **whole redirect chain's endpoint** (`getURLChain()`, `electron.d.ts:8262`) alongside the
initiating URL — "where did this actually come from" is the first question after a bad download, and
it is also what the quarantine record is supposed to answer.

`hasUserGesture()` (`electron.d.ts:8266`) is worth persisting: a download with no user gesture is a
drive-by, and surfacing that in `bg downloads list` is cheap, honest signal.

Private sessions get no ledger row and no `.bgpart` in the profile — private downloads go to the
OS downloads directory and are recorded nowhere.

---

## 12. macOS `com.apple.quarantine` — the exact command and API

### 12.1 Why this section exists

**Measured, on this machine:**

```
$ xattr <electron-downloaded-file>
com.apple.provenance

$ xattr -p com.apple.quarantine <electron-downloaded-file>
xattr: ...: No such xattr: com.apple.quarantine
```

Electron 43.2.0 downloaded a file end-to-end through its own `will-download` path and attached
**no quarantine attribute**. The only attribute present is `com.apple.provenance`, which the kernel
applies and which is **not** a Gatekeeper input.

Corroborating static evidence from the shipped framework:

- `nm -u "Electron Framework"` shows `_setxattr` imported but **no** LaunchServices quarantine
  symbols (`_LSSetItemAttribute`, `_kLSQuarantineAgentNameKey` are absent).
- `strings` finds `quarantine.mojom.Quarantine` — the Chromium quarantine *service interface* is
  compiled in — but the only concrete quarantine command string in the binary is a **removal**:
  `/usr/bin/xattr -d -r com.apple.quarantine %@`.
- No `kMDItemWhereFroms` / `com.apple.metadata` strings, so the "Where from" Finder field is not
  populated either.

The consistent reading: the quarantine *service* exists in the tree, but the code that *calls* it on
download completion lives in `//chrome`'s download delegate, which Electron replaces with its own.
Every mainstream browser sets this attribute. If BlackGlass does not, it is the one browser on the
machine whose downloads bypass Gatekeeper.

### 12.2 The value format

```
<flags>;<hex-unix-seconds>;<agent-name>;<UUID>
```

Verified properties:

- Four semicolon-separated fields. The UUID field may be empty (QuickTime writes a trailing `;`).
- Timestamp is **lowercase hex, no `0x`**, seconds since the Unix epoch. Verified:
  `0x6a665708 = 1785091848 = 2026-07-27T00:20:48`.
- UUID is **uppercase** in every observed sample. `crypto.randomUUID()` returns lowercase — call
  `.toUpperCase()`.
- Stored as **raw ASCII with no NUL terminator**. A 61-character value stores as exactly 61 bytes
  (`ls -l@` and `xattr -px` both confirm; the hex dump ends at the final `C`, no trailing `00`).
  This matters for the `setxattr(2)` call: pass `strlen(value)`, not `strlen(value) + 1`.

**Recommended value for BlackGlass:** flags `0083`, matching Safari on this OS build.

```
0083;<hex-time>;BlackGlass;<UPPERCASE-UUID>
```

with the §1.1 caveat that flag-bit semantics are undocumented and I do not assert them. `0083` is
recommended because it is empirically what the system browser writes on macOS 26.1, which is the
strongest available signal in the absence of documentation.

### 12.3 The exact command

```bash
xattr -w com.apple.quarantine "0083;6a6ccc81;BlackGlass;BB987AF2-A86D-47BD-B14D-6ADE813A68FC" /path/to/file
```

Verified round-trip on this machine:

```
$ xattr -p com.apple.quarantine q1.bin
0083;6a6ccc81;BlackGlass;BB987AF2-A86D-47BD-B14D-6ADE813A68FC
```

For a directory tree (e.g. if BlackGlass ever expands an archive), add `-r`:

```bash
xattr -w -r com.apple.quarantine "<value>" /path/to/dir
```

Generating the two dynamic fields in shell:

```bash
printf '%x' "$(date +%s)"    # 6a6ccc81
uuidgen                      # BB987AF2-... (already uppercase)
```

**`/usr/bin/xattr` is safe to shell out to on macOS 26.1.** It is a native Mach-O universal binary
(x86_64 + arm64e, 118896 bytes), not the Python script it was on older releases — so it does not
depend on a system Python that no longer exists. This was worth checking; it would have been a
silent runtime failure.

### 12.4 The API — Node (recommended)

Node has no `xattr` binding. Shell out with `execFile`, **never** `exec`, so the attacker-controlled
filename never reaches a shell:

```js
const { execFile } = require('node:child_process');
const crypto = require('node:crypto');

function quarantineValue(agent = 'BlackGlass') {
  const ts = Math.floor(Date.now() / 1000).toString(16);      // lowercase hex, no 0x
  const uuid = crypto.randomUUID().toUpperCase();             // observed samples are uppercase
  return `0083;${ts};${agent};${uuid}`;
}

function setQuarantine(filePath) {
  return new Promise((resolve, reject) => {
    execFile(
      '/usr/bin/xattr',                                        // absolute path, no PATH lookup
      ['-w', 'com.apple.quarantine', quarantineValue(), filePath],
      { timeout: 5000 },
      (err) => (err ? reject(err) : resolve())
    );
  });
}
```

`execFile` with an argument array performs no shell interpretation, so a filename containing `;`,
`$(…)` or a newline is inert. Using an absolute binary path avoids `PATH` hijacking — which is not
theoretical here, given the environment note that this machine's sandboxed shells drop `PATH`.

**Verification is mandatory, not optional.** Read the attribute back and only then set
`quarantined = 1` in the ledger and perform the rename:

```js
execFile('/usr/bin/xattr', ['-p', 'com.apple.quarantine', filePath], (err, stdout) => {
  const ok = !err && stdout.trim().startsWith('0083;');
  // if (!ok) -> do NOT rename into place; surface a loud error
});
```

A download that silently failed to be quarantined is worse than one that failed to download, because
the user believes it was checked.

### 12.5 The API — native `setxattr(2)`

If the quarantine step ever moves into the Rust core (which owns no Chromium and would avoid a
process spawn per download), the syscall is:

```c
#include <sys/xattr.h>

int setxattr(const char *path, const char *name, void *value,
             size_t size, u_int32_t position, int options);
```

From `man 2 setxattr` on this machine. For quarantine:

- `name` = `"com.apple.quarantine"`
- `value` = the ASCII string, `size` = `strlen(value)` — **no NUL terminator** (verified §12.2)
- `position` = `0` (reserved except for resource forks)
- `options` = `XATTR_NOFOLLOW` so a symlinked download target cannot redirect the attribute onto
  another file. `XATTR_CREATE` additionally fails if the attribute already exists, which is the
  right choice if we want to detect a Chromium behaviour change rather than paper over it.

Rust, no new crate — `libc` is already transitively available:

```rust
use std::ffi::CString;

const XATTR_NOFOLLOW: libc::c_int = 0x0001;

fn set_quarantine(path: &str, value: &str) -> std::io::Result<()> {
    let c_path = CString::new(path)?;
    let c_name = CString::new("com.apple.quarantine")?;
    let rc = unsafe {
        libc::setxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            value.as_ptr() as *const libc::c_void,
            value.len(),          // strlen, NOT len + 1
            0,                    // position
            XATTR_NOFOLLOW,
        )
    };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}
```

**Recommendation: do it in the engine (Node) for v1.** The engine is where `will-download` and the
completion event already are; moving the file's final state across the socket to the core adds a
round trip and a new failure mode for no benefit. Revisit if download volume ever makes the
per-download `execFile` spawn measurable, which at human download rates it will not.

### 12.6 What we must never do

The Electron framework contains the string `/usr/bin/xattr -d -r com.apple.quarantine %@` — an
attribute *removal*, used by app-update flows. **BlackGlass must never expose quarantine removal.**
No `--no-quarantine` flag, no "trust this download" button that strips it. Stripping quarantine is
the single action that converts a downloaded binary into an unchecked one, and a browser that offers
it as a convenience has defeated the purpose of setting it. Removing quarantine is the user's
decision to make with their own tools.

---

## 13. `safeStorage` for secrets

### 13.1 Measured behaviour

```
isEncryptionAvailable() = true
encryptString('bg-secret-token-123') → 35 bytes, prefix "v10"
decryptString(...)                   → 'bg-secret-token-123'
```

The `v10` prefix is Chromium's `os_crypt` version tag; 19 bytes of plaintext produce 3 + 32 bytes,
consistent with AES-128-CBC and PKCS#7 padding.

On macOS `isEncryptionAvailable()` returns true "if Keychain is available"
(`electron.d.ts:11888-11892`) — note it does **not** require the `ready` event on macOS, unlike
Linux and Windows. Call it anyway after `whenReady` so the behaviour is uniform if we ever port.

### 13.2 The Keychain item — and the trap

Verified by `security find-generic-password` after the probe ran:

```
class: "genp"
"svce" = "bg-b09-probe Safe Storage"
"acct" = "bg-b09-probe Key"
```

The service name is `<appName> Safe Storage` and the account is `<appName> Key`. This matches every
other Electron/Chromium app on the machine (`Chrome Safe Storage`, `Code Safe Storage`,
`discord Safe Storage`, `Claude Safe Storage`).

**The trap: the Keychain item name is derived from the app name.** Rename the app and every
previously encrypted blob — including Chromium's own cookie values — becomes undecryptable, because
the key is looked up under a name that no longer exists. There is no migration path and no error
that says "you renamed your app".

**Instruction: fix the Electron app name to exactly `BlackGlass` now, before any release,** and
treat it as a compatibility constant with a comment saying so. It is currently `@blackglass/engine`
in `apps/engine/package.json:2`, which would produce a Keychain item named
`@blackglass/engine Safe Storage` — a name containing a `/`, which is at best untidy and at worst a
future bug. This is a one-line change that gets much more expensive after the first user stores a
credential.

### 13.3 What we store

`safeStorage` is for **secrets only** — proxy credentials, sync tokens if we ever add sync. It is
not a general config store: every `encryptString`/`decryptString` is a Keychain round trip, and
`decryptString` is synchronous and will block the engine's main loop, which is the loop that ships
frames.

Format `secrets.bin` as a single JSON object, encrypted once as a whole:

```js
// write
fs.writeFileSync(secretsPath, safeStorage.encryptString(JSON.stringify(secrets)), { mode: 0o600 });
// read
const secrets = JSON.parse(safeStorage.decryptString(fs.readFileSync(secretsPath)));
```

One blob, one Keychain operation, read once at startup and cached in memory. `mode: 0o600`
regardless — the ciphertext is not a reason to be careless with permissions.

**Degrade honestly.** If `isEncryptionAvailable()` is false, do **not** fall back to plaintext.
Refuse to store the secret and tell the user why. `setUsePlainTextEncryption(true)`
(`electron.d.ts:11900`) exists but is documented as a no-op on macOS and Windows — it is a Linux
affordance and must never be called on our path.

Private sessions get no access to `secrets.bin` at all.

---

## 14. Private sessions

### 14.1 What makes it private

1. Partition string with **no** `persist:` prefix ⇒ cookies, localStorage, IndexedDB in memory only.
2. `{ cache: false }` at creation ⇒ no HTTP cache on disk. This is the one place the single
   `FromPartitionOptions` field earns its keep.
3. Separate engine **process** ⇒ separate network service and separate crash domain (§3.3).
4. No profile root argument ⇒ the process is structurally incapable of writing to a profile.
5. No history rows, no download ledger rows, no `secrets.bin` handle.

Point 4 is the design principle worth restating: privacy enforced by *absence of capability* rather
than by conditional logic. An `if (!isPrivate)` guard is one refactor away from being wrong; a
process that was never given the path cannot write to it.

### 14.2 What it does not protect against

State this in user-facing docs, because overclaiming here is how browsers lose trust. Private mode
does not hide traffic from the network, the ISP, or the destination site. It does not defeat
fingerprinting. DNS queries still leave the machine and may be cached by the OS resolver. Downloads
deliberately persist — a downloaded file is a file.

### 14.3 Permissions

Install `setPermissionRequestHandler` (`electron.d.ts:13255`) **and**
`setPermissionCheckHandler` (`electron.d.ts:13246`) on every session. The typings note that
implementing only one leaves permission handling incomplete.

Default-deny everything in private sessions. In named profiles, default-deny the permissions that
have no meaning or no safe path in an offscreen terminal browser — `media`, `geolocation`,
`notifications`, `midi`, `midiSysex`, `hid`, `usb`, `serial`, `display-capture`, `window-management`,
`idle-detection`. `openExternal` in particular must be denied or routed through explicit user
confirmation in the terminal: it is a direct "make the OS open this URL" primitive.

The reason to be exhaustive rather than permissive-by-default is §2 — an unhandled permission may
fall through to a native prompt, which in an offscreen app is an invisible window.

---

## 15. Clearing data

### 15.1 Per-site forget

`clearData` (`electron.d.ts:12904`) is the modern API and takes `origins` / `excludeOrigins`
(mutually exclusive, `electron.d.ts:20588-20596`):

```js
await ses.clearData({
  origins: ['https://example.com'],
  dataTypes: ['cookies', 'localStorage', 'indexedDB', 'serviceWorkers', 'cache'],
});
```

`avoidClosingConnections` and `originMatchingMode` are also available; leave both at default unless
a measurement says otherwise.

Prefer `clearData` over the older `clearStorageData` (`electron.d.ts:12923`). They use different
spellings for the same concepts (`localStorage` vs `localstorage`, `indexedDB` vs `indexdb`) and
mixing them is a latent bug. Pick `clearData` and use it everywhere.

### 15.2 Whole-profile delete

`rm -rf $BLACKGLASS_HOME/chromium/Partitions/p-<slug>` plus the profile's rows in `bg.sqlite` plus
its entry in `profiles.json`. This is clean precisely because of the §3.1 rule that no web content
ever uses the default session — with named partitions only, one directory is the whole profile.

Do this with the engine for that profile **stopped**. Deleting a live Chromium profile directory
produces corrupted LevelDB rather than a clean slate.

### 15.3 Shutdown

`ses.cookies.flushStore()` and a `bg.sqlite` checkpoint on the normal exit path, and on `SIGTERM`.
`SIGKILL` cannot be handled — which is the argument for the §7 debounced periodic flush rather than
relying on shutdown alone.

---

## 16. Instructions for the commander

Core files are yours. In priority order:

**16.1 — `apps/engine/package.json:2`: change `"name"` to `"BlackGlass"`.** One line, and it must
happen before any release. It determines the Keychain item name for every encrypted secret and for
Chromium's own cookie encryption; changing it later orphans that data with no migration path and no
useful error (§13.2). The current value `@blackglass/engine` would create a Keychain service named
`@blackglass/engine Safe Storage`.

**16.2 — `apps/engine/src/main.js`: add profile arguments and session creation** per §6. Ordering is
load-bearing: `app.setPath('sessionData', …)` before `whenReady`, session created with options
exactly once, `webPreferences.session` set to the session object rather than
`webPreferences.partition` to a string. Today the window presumably uses the default session; §3.1
argues for moving off it.

**16.3 — `apps/engine/src/main.js`: add the `will-download` handler** per §11, with the
staging-and-rename flow of §11.5 and the quarantine call of §12.4. Note the two independent
requirements: `ses.setDownloadPath()` as a session-wide backstop **and** `item.setSavePath()` per
item, because a missed path is a hung browser (§0.2).

**16.4 — Add a `bg-data` module owning `bg.sqlite`** (history, bookmarks, download ledger) on
`node:sqlite`. Verified available; no new dependency (§9).

**16.5 — Protocol additions.** New type-10 commands from core to engine — `profile.info`,
`history.query`, `bookmark.add/remove/list`, `download.list/cancel`, `data.clear` — and new type-2
events from engine to core — `download.started/progress/done`, `history.visit`. Keep the existing
`[u8 type][u32 BE len][payload]` framing; nothing here needs a new frame type. Throttle
`download.progress` to ≈4 Hz so it never competes with the frame path (§11.6).

**16.6 — Do not implement quarantine removal** in any form (§12.6).

---

## 17. Test plan

CI-able without a display, which matters given the lock-screen constraint.

| # | Test | Assertion |
|---|---|---|
| 1 | Launch with `--bg-profile=work`, navigate, exit | `Partitions/p-work/Cookies` exists and is SQLite |
| 2 | Launch private, navigate, exit | no new dir under `Partitions/`; `Cache` unchanged in size |
| 3 | Set a cookie in `p-work`, restart, read it | cookie survives |
| 4 | Set a cookie in private, restart, read it | cookie is gone |
| 5 | Download a file | `xattr -p com.apple.quarantine <file>` starts with `0083;` |
| 6 | Download a file | no `.bgpart` file remains in the download dir |
| 7 | Download named `evil‮txt.exe` | on-disk name has no bidi override; resolved path inside download root |
| 8 | Download named `../../etc/passwd` | rejected; nothing written outside the download root |
| 9 | Two downloads of the same name | second is `name (1).ext`; neither is truncated |
| 10 | `safeStorage` roundtrip | plaintext recovered; `security find-generic-password -s "BlackGlass Safe Storage"` exits 0 |
| 11 | History after 3 navigations | 3 `visits` rows, titles backfilled |
| 12 | History in private mode | 0 rows added |
| 13 | `clearData({origins:[…]})` | that origin's cookies gone, others intact |
| 14 | Profile slug `../evil` | engine exits 2, nothing created on disk |
| 15 | Cache cap (§8, UNVERIFIED) | after sustained browsing, `du -s Partitions/<slug>/Cache` ≤ 64 MiB + slack |

Tests 5–9 are the ones that would catch a real security regression, and 5 is the one that is failing
today by default.

---

## 18. Open questions and unverified claims

Marked honestly rather than guessed.

1. **Quarantine flag-bit semantics — UNVERIFIED.** I measured four real writers producing `0081`,
   `0082`, `0083` and `0281`, but Apple does not document the field and I will not assert a bit
   table I cannot support. Recommendation to use `0083` rests on it being what Safari writes on this
   exact OS build.
2. **End-to-end Gatekeeper behaviour — BLOCKED, not unverified.** Confirming that a
   BlackGlass-quarantined app bundle produces the correct "downloaded from the internet" dialog
   requires observing a GUI dialog. The machine is at a lock screen, so this is not observable in
   this session. What *is* verified is that the attribute is written, has the same shape as
   Safari's, and survives the rename. The commander should run this check manually once.
3. **`--disk-cache-size` in Chromium 150 — UNVERIFIED.** Long-standing switch, not measured here.
   Test 15 covers it.
4. **Third-party cookie blocking.** No clean Electron-level switch identified. Do not claim it in
   user-facing docs until measured.
5. **`com.apple.provenance`.** Observed on the downloaded file; kernel-applied. I did not
   investigate its semantics and it should not be treated as any kind of substitute for quarantine.
6. **`node:sqlite` stability.** Verified present and loadable in Electron 43. It is a relatively
   young Node built-in; if it is still flagged experimental in Node 24.18.0 it may emit a warning on
   `require`, which the engine should suppress rather than let onto the wire.

---

## 19. Summary

Profiles map to `persist:p-<slug>` partitions, one engine process each, with the profile root
injected by the core and Chromium's data confined to `chromium/Partitions/<slug>` — measured layout,
§5. Private sessions are non-`persist:` partitions with `cache:false` in a process that was never
handed a profile path, so privacy is enforced by absent capability rather than by a conditional.
Cookies and cache are Chromium's; history, bookmarks and the download ledger are ours, in
`bg.sqlite` via the `node:sqlite` built-in — verified available, so no new dependency and no native
build against a 98%-full disk. Secrets go through `safeStorage`, which works, and whose Keychain
item is named from the app name — making the one-line rename in §16.1 urgent rather than cosmetic.

Downloads are the part that is actually broken today. Electron ships no quarantine on macOS —
verified by downloading a file and finding the attribute absent — so BlackGlass must write
`com.apple.quarantine` itself, via `execFile('/usr/bin/xattr', ['-w', …])`, verify the read-back,
and only then rename the staged file into place.
