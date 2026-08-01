# E06 — Record / Replay: capture, export, replay, and secret redaction

**Mission:** E06. **Owner file:** `artifacts/swarm/E06-recorder-replay.md` (this document only).
**Date:** 2026-07-31. **Host:** macOS 26.1, Apple M4 arm64. Electron 43.2.0 / Chromium 150, CDP `1.3`, Node v24.11.1.

**Status of evidence.** Six Electron probes were executed against the workspace's own
`node_modules/electron` with the agent Bash sandbox disabled. Every behavioural claim in §3 is a
**measurement with a transcript**, not a reading of documentation. Probe sources live in the agent
scratchpad and write nothing into the repository. Claims about our code cite `file:line` against the
pinned bytes below. Anything I could not execute is marked **`[UNVERIFIED]`** and collected in §13.

**Pinned revisions.** Core source is under concurrent edit by other agents, so every `file:line`
below is anchored to these exact bytes:

| File | lines | md5 |
|---|---:|---|
| `apps/cli/src/main.rs` | 1039 | `bceb2a511097d93ea17ac90b94fcb077` |
| `crates/tf-proto/src/lib.rs` | 284 | `edbb4c0b2e74e960857f2fc687210fe4` |
| `apps/engine/src/main.js` | 309 | `1520d7ab86e4c69e76508bd6d6bab2ce` |
| `crates/tf-term/src/input.rs` | 768 | `6500d2a886652e8eeac65ff64c38c479` |

**File ownership.** I wrote only this file. Every change described for `apps/engine/src/main.js`,
`apps/cli/`, and `crates/` is described for the commander in §12, not made by me.

---

## 0. The decision, in one paragraph

Record at the **command boundary in the Rust core** — the single choke point every human keystroke
and every agent action already passes through (`apps/cli/src/main.rs:436` `Session::send`) — and
classify secrecy from a **CDP isolated-world focus tracker** that reports *what kind of field has
focus*, never a value. Redaction is not a post-processing pass over a log; **the secret is never
written, in any form, at any point.** The recorder holds a three-state secrecy latch
(`PUBLIC` / `SECRET` / `UNKNOWN`) and treats `UNKNOWN` as `SECRET`, so the failure mode of every
race, every missed event, and every unparsed frame is *over-redaction*, never disclosure. The
exported script is **JSONL with one flat object per line**, which the existing minimal JSON helpers
in `tf-proto` can parse with three added numeric getters and no new dependency. Replay resolves
secrets by *reference* (`{{secret:NAME}}` → env var or Keychain at replay time) and synchronises on
**explicit waits** — including a frame-quiescence wait that Terminal-Fenster can offer and a normal browser
cannot, because we already own the paint stream.

**Single most actionable recommendation** (expanded in §12.1): **`apps/engine/src/main.js` never
calls `webContents.focus()`, and I measured that without it an offscreen page never receives focus at
all** — `document.hasFocus()` stays `false` and **`focus` / `focusin` events never fire**, in every
run I performed (§3.3). That breaks the entire redaction design, because focus events are how the
recorder learns a password field is active. It is also a **present-day input bug independent of
E06**: in one configuration a click failed to move `document.activeElement` and three injected
characters landed nowhere at all (§3.4). Adding `b.webContents.focus()` to `createWindow()` made
focus events fire in **100% of subsequent trials at a median 4.1 ms** after the click. That is one
line, in a file I do not own, and it is a prerequisite for everything else in this document.

---

## 1. What is already true (grounding)

Read directly from the pinned tree.

| Fact | Evidence |
|---|---|
| Every outbound command is framed in exactly one place | `apps/cli/src/main.rs:436-439` — `send(&mut self, json: &str)` |
| Every terminal input event becomes a command in exactly one place | `apps/cli/src/main.rs:562` — `handle_event(&mut self, ev: input::Event) -> bool` |
| Typed text is already serialised as a `text` field on the wire | `apps/cli/src/main.rs:595`, `:647` — `{"t":"input","kind":"key","action":"press","keyCode":"…","text":"…"}` |
| The engine forwards `text` character-by-character into the page | `apps/engine/src/main.js:210-219` — `type:'char'` per code point |
| Engine emits `title` / `url` / `loading` / `loadError` / `crash` / `popup` events | `apps/engine/src/main.js:117-132` |
| Frames carry a seq + geometry + dirty rect | `crates/tf-proto/src/lib.rs:19-30` `FrameHeader` |
| The core's JSON reader handles **flat objects only** | `crates/tf-proto/src/lib.rs:120-170` — `json_get_str`, `json_get_bool`; no arrays, no nesting, no numbers |
| There is **no** recorder, no CDP attach, and no `session`/`webRequest` use in the engine | `apps/engine/src/main.js` — require list is `{ app, BrowserWindow }` at `:17` |
| No `LICENSE` file exists at the repo root | `ls LICENSE*` → no matches; `Cargo.toml` declares `license = "MIT OR Apache-2.0"` |

Two consequences shape the whole design. First, **`send` at `:436` is a perfect tap** — a recorder
placed there sees 100% of actions with zero risk of missing a path, because there is no other path.
Second, **the flat-object constraint is a real constraint**, not a stylistic one: a nested JSON
script format would force a full JSON parser into the core, which `tf-proto:122-124` explicitly
argues against. §6 therefore specifies a format that stays flat.

---

## 2. Probe inventory

| # | Probe | Question | Result |
|---|---|---|---|
| P1 | `probe-recorder.js` | Does the isolated-world focus tracker work? | **Deadlocked** — see §3.1 |
| P2 | `probe-cdp-order.js` | Why did it deadlock? | CDP enables hang pre-document (§3.1) |
| P3 | `probe-recorder2.js` | Full capture with corrected ordering | Binding works; focus events absent (§3.2) |
| P4 | `probe-focus3.js` | Is the listener missing, or the focus, or the binding? | Focus events deferred (§3.2) |
| P5 | `probe-focus4/5.js` | A/B: `focus()` vs not | Decisive (§3.3, §3.4) |
| P6 | `probe-classify.js` | Full classifier + latency | 8/8 field kinds, median 4.1 ms (§3.5) |
| P7 | `probe-mo.js` | Can an isolated-world MutationObserver see page edits? | **No — zero events** (§3.6) |

---

## 3. Measured findings

### 3.1 CDP domain enables never resolve before a document exists

`webContents.debugger.attach('1.3')` succeeds on a freshly constructed `BrowserWindow`, but the
first `sendCommand` never resolves. Measured with an 8 s timeout wrapper:

| Case | offscreen | document loaded at attach | `Runtime.enable` | `Page.enable` |
|---|---|---|---|---|
| A | yes | no | **timeout** | **timeout** |
| B | yes | `data:text/html,<b>hi</b>` | **ok** | **ok** |
| C | **no** | no | **timeout** | — |

Case C matters: this is **not** an offscreen-rendering quirk. It reproduces with `offscreen: false`,
so it is a property of attaching before the renderer has a document. In case B, `DOM.enable` and
`Runtime.evaluate` (`1+1` → `2`) also succeeded.

There is no error, no rejection, and no log line — the promise simply never settles. A recorder that
attaches CDP in `createWindow()` before the first `loadURL` would hang the engine silently and
forever. **Rule: load `about:blank`, then attach, then enable.** With that ordering, attach measured
**0.44 ms** and both enables together **1.37 ms**.

### 3.2 The binding works; the focus events were the problem

With the corrected ordering, `Runtime.addBinding` with `executionContextName: 'bg_recorder'`
succeeded (no fallback needed), and `Page.addScriptToEvaluateOnNewDocument` with
`worldName: 'bg_recorder'` ran the tracker in a genuinely separate world. Contexts observed via
`Runtime.executionContextCreated` for the loaded page:

| ctx id | name | origin | isDefault |
|---:|---|---|---|
| 4 | *(empty)* | `http://127.0.0.1:59184` | **true** (main world) |
| 5 | `Electron Isolated Context` | `http://127.0.0.1:59184` | false |
| 6 | `bg_recorder` | `http://127.0.0.1:59184` | false |

All binding callbacks arrived tagged `executionContextId: 6`. Isolation held: evaluating in the main
world returned `{"binding":"undefined","installed":"undefined"}`, and an adversarial page function
enumerating `window` for anything containing `bg` returned only built-in WebGL/XR names — **the page
cannot see the binding or the tracker flag.**

But no `focusin` ever arrived, while `document.activeElement` demonstrably changed. That separation —
DOM focus moves, focus *events* do not fire — is §3.3.

### 3.3 Without `webContents.focus()`, an offscreen page never has focus and never fires focus events

A/B, same page, same coordinates, same injected clicks:

| | `document.hasFocus()` | `activeElement` after click | focus events | typed chars landed |
|---|---|---|---|---|
| **A** never call `focus()` | `false` | `(none)` | **none** | **0 of 3** |
| **B** `win.focus()` + `wc.focus()` at creation | **`true`** | `pw` | `focus:pw`, `focusin:pw` | **3 of 3** |

In case A the page was still `hasFocus() === false` after 3 000 ms and the event log was empty.

This is the load-bearing measurement of the document. Across **all six probes**, the correlation was
perfect and without exception: **focus events fired if and only if `focus()` had been called.**

### 3.4 The input pipeline is non-deterministic today, in a way that is not merely cosmetic

Chasing 3.3 turned up something worse. Whether a click moves `activeElement` and whether injected
characters land **at all** varied between runs when `focus()` was never called:

| Run | URL scheme | CDP attached | `activeElement` after click | chars landed |
|---|---|---|---|---|
| P5 case A | `http://127.0.0.1` | no | `(none)` | **0 of 3** |
| P5 case C | `data:` | no | `pw` | 3 of 3 |
| P5 case D | `data:` | yes | `pw` | 3 of 3 |
| P3 | `http://127.0.0.1` | yes | `pw` | 7 of 7 (`password=hunter2` reached the server) |

I did **not** isolate the responsible variable and I am not going to claim I did; `data:` vs `http:`
is the obvious candidate but one trial each is not evidence. What *is* established is that **the
current engine has at least one configuration in which a click focuses nothing and typing is silently
discarded**, and that calling `focus()` made every trial deterministic. The existing e2e suite
(`tests/e2e/input-injection.js:201-210`) exercises typing over a `data:` URL — the configuration that
happened to work — which is consistent with the suite passing while the bug exists. I did not run
that suite, because it writes `tests/e2e/input-injection-results.json` into a tree I do not own.

### 3.5 With focus fixed, the classifier is complete and fast

Eight field kinds, click → classification report, `webContents.focus()` called once at start:

| Field | `attrType` | `autocomplete` | detected | latency |
|---|---|---|---|---:|
| `<input type=text name=username>` | `text` | — | yes | 28.78 ms |
| `<input type=password>` | `password` | `current-password` | yes | 2.96 ms |
| `<input type=text>` + `current-password` | `text` | `current-password` | yes | 5.25 ms |
| `<input type=search>` | `search` | — | yes | 2.63 ms |
| `<div contenteditable>` | — | — | yes (`editable: true`) | 15.25 ms |
| `<input>` + `cc-number` | `text` | `cc-number` | yes | 2.53 ms |
| `<input>` + `one-time-code` | `text` | `one-time-code` | yes | 5.99 ms |
| password (refocus) | `password` | `current-password` | yes | 2.58 ms |

**8/8 detected. Median 4.11 ms, min 2.53 ms, max 28.78 ms** (the max is the first report of the
session; every subsequent one was under 16 ms). The tracker reports `tag`, `attrType` (content
attribute), `propType` (IDL property), `autocomplete`, `name`, `id`, and `isContentEditable` — every
signal §5 needs — and reports **no values**.

### 3.6 An isolated-world MutationObserver observes nothing — do not build on it

I tried to catch the "show password" eye toggle (`input.type = 'text'` on a focused password field)
with a `MutationObserver` in the `bg_recorder` world. It never fired. P7 isolated it:

| Mutation performed by main-world script | isolated-world MO events |
|---|---:|
| `setAttribute('class','x')` | **0** |
| `setAttribute('type','text')` | **0** |
| `el.type = 'text'` (IDL setter) | **0** |

Zero for all three, including a plain `class` change, while the mutation demonstrably took effect
(`document.getElementById('pw').type` → `"text"`). The likely mechanism is that MutationObserver
callbacks are delivered at a microtask checkpoint and the isolated world runs no tasks of its own, so
the checkpoint never arrives — but **I did not confirm the mechanism**, only the behaviour.

The behaviour alone is sufficient and it drives a design rule with teeth. In P6 the page flipped a
**focused** password field to `type="text"` and the recorder received **no report at all**
(`sawMutation: false`, `activeStillPw: "pw"`, `pwTypeNow: "text"`). Had classification been a live
read of the current type, that field would have silently de-classified from SECRET to PUBLIC **while
the user was still typing into it**. Hence §5.4: **secrecy is sticky and can only ever escalate.**

### 3.7 Credential surfaces on the wire

`session.defaultSession.webRequest.onBeforeSendHeaders` observed, for a form POST:

```
Accept, Accept-Encoding, Accept-Language, Content-Type, Cookie, Origin, Referer,
Sec-Fetch-Dest, Sec-Fetch-Mode, Sec-Fetch-Site, Sec-Fetch-User,
Upgrade-Insecure-Requests, User-Agent
```

with `Cookie: sid=SUPERSECRETSESSION; theme=dark` **fully readable in plaintext**. Separately,
`session.defaultSession.cookies.get({})` returned **12 cookies with `.value` in cleartext** — and
notably they were not all mine: the default session had persisted cookies from prior swarm activity
(`_octo` from github.com, several `WMF-*` from wikimedia). That is a live demonstration that a
recorder with naive access to the cookie jar exfiltrates **other sessions' credentials**, not just
the ones from the recording. The POST body reached the server as
`username=&password=hunter2&pin=&q=&cardnumber=` — plaintext, as expected.

---

## 4. Architecture: where the recorder taps in

```
 tty ──> input::Decoder ──> handle_event() ──┐
                          (main.rs:562)      │
                                             ├──> RECORDER TAP ──> send()  ──> engine
 agent RPC (E05) ────────────────────────────┘    (classify+redact)  (:436)
                                                        ▲
                                   secrecy latch ───────┤
                                                        │
 engine ──> T_EVENT (url/title/load/…) ─────────────────┤
 engine ──> T_EVENT {"t":"focus",…} ← NEW, from the CDP isolated-world tracker
```

Three properties make this placement correct rather than merely convenient.

**It is the only path.** `send` at `:436` is the sole producer of `T_COMMAND` frames. A tap there
cannot miss an action, and — critically for E05's shared-control model — it records human and agent
actions through the *same* code with the same redaction, so a mixed-provenance session yields one
coherent script.

**It is upstream of the wire, downstream of the intent.** At `:562` we still know the action was
`KeyPress('a')` rather than a serialised JSON blob, so classification is a comparison against the
latch rather than a re-parse.

**The latch is fed by the engine, so ordering is explicit.** The focus tracker's report is an event
on the existing socket, subject to the same framing and the same single-threaded poll loop
(`main.rs:474-477`). There is no second thread and no lock, matching E05's arbitration argument.

The engine-side additions are: attach CDP after `about:blank` (§3.1), register the binding and the
tracker script, call `webContents.focus()` (§12.1), and forward each report as
`{"t":"focus","secrecy":"secret|public","reason":"…"}`. **The tracker must never send field values,
and the engine must never forward a field value**, so that even a compromised engine cannot leak one
through this channel.

### 4.1 Relationship to neighbouring missions

- **E01 (CDP broker):** the recorder needs `Runtime` + `Page` on the *engine's own* debugger session,
  not through the broker. It must be usable with automation off. Both can coexist — but two
  `attach()` calls on one `webContents` is a conflict I did not test **`[UNVERIFIED]`**.
- **E05 (handoff audit log):** adjacent, not the same. The audit log answers *what happened, by whom,
  irrevocably*; the recording answers *what must happen again*. The audit log should retain entries
  the recorder deliberately drops (e.g. "a secret was entered here"), because "a credential was typed
  at 14:02" is exactly what an audit trail needs and exactly what a replay script must not contain.
- **A09 (threat model, TB3):** every string a recording renders back to the tty — URLs, titles,
  assertion text — is attacker-controlled and must go through TB3 sanitisation before display.
  §7.4 restates this obligation for the replay reporter.

---

## 5. Redaction — the precise rules

This is the core of the mission, so the rules are stated as rules, with the exact patterns.

### 5.0 The two invariants

> **I1 — Never-written.** A value classified secret is never written to disk, to a log, to a
> terminal, to an assertion, or to a frame buffer that is persisted. There is no "encrypted secrets
> section", no "redact on export", and no `--include-secrets` flag. The plaintext never enters the
> recorder's memory as *recordable state* at all.
>
> **I2 — Fail closed.** Every ambiguity resolves to secret. `UNKNOWN` is treated as `SECRET`. A parse
> failure, a dropped event, an unrecognised field, a cross-origin frame we cannot inspect, or a
> classification older than the freshness bound (§5.5) all mean *redact*.

I1 is what makes the CI test in §9.1 possible: the sentinel must appear **nowhere** in the artifact,
which is a byte-level assertion, not a review.

### 5.1 What is never recorded, under any configuration

No flag enables these. This is the list a reader should be able to check the implementation against.

1. **Any character typed while secrecy is `SECRET` or `UNKNOWN`.** Not the character, not its length
   per-keystroke, not its key code, not its timing beyond a coarse bucket.
2. **Clipboard paste payloads** into any field, secret or not. Pasted content is unclassifiable at
   the point of paste, and pastes into login forms are the single most common credential path.
3. **Cookie values.** `Cookie` and `Set-Cookie` header values, and every `.value` from
   `session.cookies` — measured cleartext and cross-origin in §3.7.
4. **`Authorization` / `Proxy-Authorization` values**, and the parameters of
   `WWW-Authenticate` / `Proxy-Authenticate` challenges.
5. **Any header whose name is not on the §5.6 allowlist** — value replaced, name retained.
6. **URL userinfo** (`https://user:pass@host/`) — stripped from every URL, in every context,
   including titles, referers, and error strings.
7. **Request bodies** for any field classified secret, and **all response bodies**, always.
8. **Pixel frames** (§5.7) — not recorded by default, because a revealed password is legible in them.
9. **OS clipboard contents**, ever, for any reason.
10. **The `text` field of any key command issued while the latch is not `PUBLIC`.**

### 5.2 Secret field classification — the predicate

Evaluated against the tracker's report (§3.5). The field is **SECRET** if **any** clause holds.

```
S1  attrType == "password"  OR  propType == "password"
S2  autocomplete token ∈ SECRET_AC
S3  name or id matches SECRET_NAME_RE
S4  the field is inside a <form> that has ever contained a type=password field
S5  the field has been SECRET at any earlier point in this document's life   (sticky, §5.4)
S6  autocomplete token ∈ PII_AC                                             (tier 2, §5.3)
```

```
SECRET_AC = { current-password, new-password, one-time-code,
              cc-number, cc-exp, cc-exp-month, cc-exp-year, cc-csc, cc-type }
PII_AC    = { cc-name, tel, tel-national, tel-local, email,
              street-address, address-line1, address-line2, postal-code, bday, sex }

SECRET_NAME_RE = (?i)(pass(word|wd|phrase)?|pwd|secret|token|otp|mfa|2fa|totp|
                      auth|apikey|api[_-]?key|credential|creditcard|card[_-]?number|
                      cardnum|cvv|cvc|csc|ssn|sin|pin|seed|mnemonic|private[_-]?key)
```

`S4` is what catches the common "username and password in one form" case where the username field
carries no marking but is still a credential half. `S3` is deliberately broad: a false positive costs
one over-redacted field in a replay script (fixable by an explicit `--reveal-field` at *record* time,
which is an operator decision made before the value exists), while a false negative is a leaked
credential.

The predicate runs on **structure only**. It never inspects a value. A rule like "redact anything
that looks high-entropy" is explicitly rejected: it requires reading the secret to decide, which
violates I1.

### 5.3 Two tiers

**Tier 1 (SECRET)** — S1–S5. Never recorded; replay requires a `{{secret:NAME}}` reference.

**Tier 2 (PII)** — S6. Never recorded **by default**; replay uses `{{value:NAME}}`. The distinction
is not privacy weight but *replay economics*: a shipping address must usually be supplied for a
script to be useful, and it is not a credential, so it is reasonable to let an operator promote a
tier-2 field to a literal with an explicit, per-field, record-time flag. **Tier 1 has no such flag.**

### 5.4 Sticky taint — secrecy escalates and never decays

Forced by §3.6. Classification is keyed to **field identity**, not to the field's momentary state.

- **Identity** = `(frameId, backendNodeId)`, falling back to `(frameId, structural CSS path)` when
  the backend node id is unavailable. Identity is scoped to the document; a navigation clears it.
- Once a field is SECRET, it is SECRET **for the life of the document**, regardless of any later
  observation. A "show password" toggle, an `autocomplete` rewrite, or a `type` swap cannot
  de-classify it.
- Escalation is immediate on any observation satisfying §5.2.
- A form that ever contained a password field taints every field in it (S4) for that document's life.

This is the rule that survives P7. Because the isolated-world observer sees nothing, the recorder
must assume it will *never* be told about a downgrade, so it must never depend on being told.

### 5.5 The latch, and the freshness bound

```
        navigation / focusout / click / Tab / unknown
                       │
                       ▼
   ┌──────────────────────────────────────────────┐
   │  UNKNOWN   (treated as SECRET — invariant I2) │
   └──────────────────────────────────────────────┘
            │ focus report, age ≤ FRESHNESS
            ▼
   PUBLIC ◄────────────► SECRET
        (only via a fresh report)   (sticky per §5.4)
```

Transitions to `UNKNOWN` are mandatory on: any navigation or in-page navigation, any `focusout`, any
mouse click (which may move focus anywhere, including into a cross-origin iframe), any `Tab`
keystroke, engine restart or crash, and any gap in the event stream.

**`FRESHNESS = 250 ms.`** A classification older than this is `UNKNOWN`. The bound is set from
measurement, not taste: the observed classification latency was **median 4.1 ms, max 28.78 ms**
(§3.5), so 250 ms is roughly 9× the worst observation and leaves ample headroom for a loaded page,
while remaining far below human retype latency. The cost of the bound expiring is over-redaction.

**The race, stated honestly.** A user can click a password field and begin typing before the focus
report arrives. With the latch, those keystrokes fall in `UNKNOWN` → treated as `SECRET` → redacted.
That is the correct direction. The residual risk is the inverse — a *stale* `PUBLIC` classification
surviving a focus change the recorder was not told about — which is exactly what the mandatory
`UNKNOWN` transitions above exist to close. Every event that could plausibly move focus invalidates
the latch, so a stale `PUBLIC` requires focus to move with *no* click, *no* Tab, *no* navigation and
*no* focusout, which leaves scripted `element.focus()`. Scripted focus does fire `focusin`
(P4 measured `focusin` from a scripted `.focus()` call), so it escalates normally — **provided
`webContents.focus()` was called** (§3.3). Without that one line, none of this holds.

### 5.6 Header rules — allowlist, not denylist

Recording is **name-only by default**. A header's value is recorded **only** if its name is on this
allowlist, which was derived from the header set actually observed in §3.7:

```
ALLOW_VALUE = { accept, accept-encoding, accept-language, content-type, content-length,
                origin, sec-fetch-dest, sec-fetch-mode, sec-fetch-site, sec-fetch-user,
                upgrade-insecure-requests }
```

Everything else is recorded as `name: "<redacted:len=N>"`. Two names get special handling beyond
redaction:

- **`user-agent`** — recorded as the literal token `<pinned>`. The value is a fingerprint and a
  version string; replay substitutes the running engine's own UA, which is also what makes the script
  survive an Electron upgrade.
- **`referer`** — recorded only after §5.8 URL sanitisation (origin + path, no query, no fragment).

A denylist was considered and rejected. The measured header set (§3.7) contained `Cookie` — but a
denylist has to enumerate `x-api-key`, `x-amz-security-token`, `x-csrf-token`, `x-auth-token`,
`x-access-token`, and whatever a given SaaS invents next week. Fail-closed means the unknown header
is redacted, and an unknown header is the likeliest place for a novel credential.

For completeness, the name-pattern rule below is applied **in addition**, so that a header on the
allowlist can still be redacted if its name looks credential-bearing (a defensive redundancy; no
current allowlist entry matches it):

```
SECRET_HEADER_RE = (?i)(^|-)(auth|authorization|token|secret|key|session|sid|
                            sig|signature|credential|password|passwd|pwd|otp|jwt|bearer)(-|$)
```

### 5.7 Frames are a leak channel — no pixels by default

The recorder must **not** persist BGRA frames by default. A password field renders as bullets, but
the "show password" toggle measured in §3.6 renders the plaintext, and autofill, error toasts
("your password `hunter2` is incorrect"), and one-time codes all render legibly. A frame is a
screenshot; a directory of screenshots of a login flow is a credential store with extra steps.

When `--record-frames` is explicitly passed, the following apply and are not optional: frames are
captured **only** while the latch is `PUBLIC`; the last frame before any transition out of `PUBLIC`
is dropped as well (the reveal is visible *before* focus lands); and the bounding box of every field
classified SECRET or PII in the current document is filled with opaque black **in the recorder**
before the frame is written. Even so, this remains a documented risk, not a solved problem — a page
can render a secret outside any field we know about.

### 5.8 URL rules

Applied to every URL the recorder stores, in navigations, referers, assertions, and error strings.

1. **Userinfo stripped, always.** `https://u:p@h/x` → `https://h/x`. Non-negotiable, no flag.
2. **Query parameters:** a parameter whose *name* matches `SECRET_PARAM_RE` has its value replaced
   with `<redacted>`; the name is kept so the script stays readable and replayable.
3. **Fragment:** redacted **in full** by default. The OAuth 2 implicit flow returns `access_token` in
   the fragment, and fragments are not otherwise load-bearing for replay. `--keep-fragment` exists
   for SPA routing, and is a documented risk.
4. **Path segments:** a segment of length ≥ 32 matching `^[A-Za-z0-9_-]+$` is replaced with
   `<opaque:N>`, N being its length. This catches session ids and signed URLs in paths. Length 32 is
   a deliberate compromise: it clears ordinary slugs and UUIDs-with-hyphens (36 chars **do** match —
   see the caveat below) while catching most bearer material.

```
SECRET_PARAM_RE = (?i)^(access_?token|id_?token|refresh_?token|auth_?token|client_?secret|
                        api_?key|apikey|key|secret|password|passwd|pwd|token|otp|code|
                        session|sid|sig|signature|assertion|saml_?response|jwt|state|nonce)$
```

**Two honest caveats.** `code` and `state` are matched because they carry OAuth authorization codes,
which are single-use credentials; this will also redact an innocuous `?code=US` country parameter.
And rule 4 will redact a 36-character UUID path segment, which is usually an object id, not a secret.
Both are over-redaction, both are visible in the script as `<redacted>` / `<opaque:N>`, and both are
correctable at record time. Neither is silent.

### 5.9 Worked example

A user navigates to a login page, types `alice`, tabs, types `hunter2`, and submits.

| Step | Latch | Recorded |
|---|---|---|
| navigate | — | `{"op":"navigate","url":"https://example.test/login"}` |
| click username | UNKNOWN → PUBLIC (S4 taints it) | `{"op":"click","x":150,"y":20,"target":"input[name=username]"}` |
| type `alice` | SECRET *(S4: form contains a password field)* | `{"op":"type","target":"input[name=username]","value":"{{secret:EXAMPLE_USER}}"}` |
| `Tab` | → UNKNOWN | `{"op":"key","key":"Tab"}` |
| focus password | → SECRET (S1) | *(latch update only; nothing recorded)* |
| type `hunter2` | SECRET | `{"op":"type","target":"input[name=password]","value":"{{secret:EXAMPLE_PASS}}"}` |
| submit | — | `{"op":"click","target":"button[type=submit]"}` |
| response sets `sid` | — | *(cookie never recorded)* |

The string `hunter2` exists nowhere in the artifact. Neither does `alice` — S4 is doing real work
here, and the resulting script needs two secret references rather than one, which is correct: a
username is half a credential.

---

## 6. The recording format

**`.bgscript` — JSONL, one flat JSON object per line.** No nesting, no arrays. This is not
aesthetics: `crates/tf-proto/src/lib.rs:120-170` provides `json_get_str` / `json_get_bool` for flat
objects and argues (`:122-124`) against pulling in a real JSON parser. A flat-per-line format is
parseable by adding `json_get_u64` / `json_get_i64` / `json_get_f64` alongside them — three small
functions, no new dependency, consistent with a workspace whose only deps are `libc` and `flate2`.

Line 1 is always a header:

```json
{"op":"meta","v":1,"created":"2026-07-31T22:00:00Z","page_w":1280,"page_h":800,"engine":"electron-43.2.0","chrome":"150.0.7871.129","redaction":"v1","frames":false}
```

`page_w`/`page_h` are mandatory and come from `FrameHeader` (`tf-proto:19-30`). A script recorded at
one viewport and replayed at another will click the wrong things; replay **must** resize to the
recorded geometry (`{"t":"resize"}`, `main.js:232-238`) and refuse to proceed if it cannot.

Operations:

| `op` | Fields | Notes |
|---|---|---|
| `navigate` | `url` | post-§5.8 sanitisation |
| `click` | `x`,`y`,`button`,`clicks`,`target` | `target` is the resilient selector (§6.1) |
| `type` | `target`,`value` | `value` may be `{{secret:NAME}}` |
| `key` | `key`,`mods` | non-text keys only; text keys become `type` |
| `scroll` | `x`,`y`,`dx`,`dy` | |
| `wait` | `kind`,`arg`,`timeout_ms` | §7.1 |
| `assert` | `kind`,`arg`,`op` | §7.2 |
| `comment` | `text` | provenance, e.g. `recorded by agent` (E05) |

Timing is recorded as a coarse `after_ms` bucket (25 ms granularity) and is **advisory** — replay
uses waits, never sleeps (§7.1). Fine-grained inter-keystroke timing is deliberately discarded: it is
a biometric, and for a redacted secret field its *length* is recoverable from the keystroke count.

### 6.1 Element targeting

Coordinates alone do not survive a re-render, and CSS paths do not survive a redeploy. Record
**both**, in priority order, and let replay fall back:

1. stable attributes — `data-testid`, `id`, `name`, `aria-label`, `role` + accessible name
2. a structural CSS path from the nearest stable ancestor
3. the recorded `(x, y)`, valid only at the recorded viewport

E02 reached the same conclusion for automation targeting from a different direction; the two should
share one implementation rather than growing two dialects.

---

## 7. Replay

### 7.1 Waits

Replay never sleeps. Every step is gated on a condition with an explicit timeout (default 10 000 ms),
and a timeout is a hard failure with the condition named.

| `kind` | Condition | Source |
|---|---|---|
| `load` | `did-stop-loading` | `main.js:121` |
| `url` | current URL matches (glob) | `main.js:118-119` |
| `title` | title matches | `main.js:117` |
| `selector` | selector resolves | `Runtime.evaluate` in the isolated world |
| `text` | visible text contains | isolated world |
| `quiet` | **no paint with dirty area > 0.5% of the viewport for 200 ms** | `FrameHeader.dirty_*` |

`quiet` is the interesting one and it is ours for free. We already receive a dirty rect on every
frame (`tf-proto:24-27`), so "the page has stopped changing" is directly observable without asking
the page anything, without a network heuristic, and without CDP. It subsumes most of what people use
`networkidle` for and it is robust against pages that keep a socket open forever. E02 measured
`did-finish-load` firing 855 ms before network idle on a trivial local fixture, which is precisely
the failure `quiet` avoids.

`load` alone must **not** be the default wait, for that reason.

### 7.2 Assertions

`assert` ops carry `kind` ∈ {`url`, `title`, `selector`, `text`, `count`, `no_console_error`,
`pixel`} and an `op` ∈ {`eq`, `contains`, `matches`, `gt`, `lt`}. A failed assertion aborts with a
non-zero exit and prints the expected/actual pair — **through TB3 sanitisation** (§7.4).

`pixel` asserts a hash over a named rectangle of the frame. It is genuinely useful for "did the chart
render" and genuinely brittle for anything involving text: font hinting and antialiasing differ
across machines. Specify it as **tolerance-based** (mean absolute difference over the region, default
threshold 2%), not exact-hash, and document it as best-effort. I did not measure cross-machine
stability and will not pretend otherwise **`[UNVERIFIED]`**.

### 7.3 Secret resolution

`{{secret:NAME}}` resolves at replay, in order: `TERMINAL_FENSTER_SECRET_<NAME>` env var, then macOS
Keychain item `terminal-fenster/<NAME>`, then — only if a tty is attached and `--interactive` was passed —
a prompt with terminal echo disabled. If none resolve, replay **fails**; it does not proceed with an
empty string, because submitting a login form with a blank password is an authentication attempt that
can trip lockout counters.

Resolved secrets are written into the input command and **never** into the replay log, the assertion
output, or a failure dump. A replay failure that occurs while the latch is SECRET prints the step
number and the target, never the value.

### 7.4 Replay output goes through TB3

A09 §1.1 lists `location.href`, `document.title`, and error strings as attacker-controlled paths onto
the tty. A replay reporter prints all three by design. Every string a replay run emits — URLs in
assertion diffs, titles, selector text, timeout messages — is untrusted and must be escape-sanitised
before it reaches the terminal. This is not a new mechanism; it is the existing TB3 obligation, and
E06 must not become the path that bypasses it.

---

## 8. Determinism hazards

| Hazard | Effect | Mitigation |
|---|---|---|
| CSRF / nonce in forms | Replayed value rejected | Never record hidden-input values; let the page regenerate |
| Viewport differs | Coordinate fallback clicks wrong element | `page_w`/`page_h` in `meta`; resize or refuse (§6) |
| Time/date rendering | Text assertions drift | Prefer `matches` over `eq`; document the pattern |
| Animations | `pixel` assertions flap | Gate on `quiet` first |
| Network variance | Timing-based scripts flake | Waits only, never sleeps (§7.1) |
| Autofill | Fields pre-populate; `type` appends | Replay clears the field before typing |
| One-time codes | Unrepeatable by construction | `{{secret:}}` with an interactive prompt; there is no honest alternative |
| Cross-origin iframes | Focus tracker cannot see inside | Latch → `UNKNOWN` → redact (§5.5) |
| Scroll anchoring | Post-load scroll shifts | Record scroll as a target, re-derive on replay |

The cross-origin iframe row deserves emphasis. `Page.addScriptToEvaluateOnNewDocument` installs per
frame, but I verified the tracker only in a **single-frame, same-origin** document. Whether the
binding is delivered from an OOPIF's isolated world is **`[UNVERIFIED]`** and is the highest-priority
follow-up measurement, because a cross-origin login iframe is exactly where credentials get typed.
Until it is verified, treat any focus inside a subframe as `UNKNOWN`.

---

## 9. Test plan

### 9.1 The redaction test that actually proves something

```
1. Serve a local login fixture (no network dependency; §3 probes show this works).
2. Record: click username, type SENTINEL_U, Tab, type SENTINEL_P, submit.
3. Assert: grep -r for SENTINEL_P over the ENTIRE artifact directory — script,
   logs, frames, temp files — finds ZERO bytes.  Same for SENTINEL_U.
4. Assert: the script contains exactly two {{secret:…}} references.
5. Replay with the secrets in env; assert the fixture server received the
   correct plaintext (proving redaction did not break the flow).
6. Repeat with the "show password" toggle clicked mid-entry; step 3 must still hold.
```

Step 6 is the regression test for §3.6 and would have caught the de-classification bug directly.
Step 3 is a byte-level assertion, which is why I1 is stated as "never written" — a weaker invariant
could not be tested this way.

### 9.2 Additional coverage

- **Latch fuzz:** random interleavings of click/Tab/navigate/type; assert the latch is never `PUBLIC`
  within `FRESHNESS` of any focus-invalidating event.
- **Header allowlist:** fixture returns 30 exotic headers; assert only the §5.6 set has values.
- **CDP ordering regression:** assert the engine attaches only after a document exists — a direct
  guard against the §3.1 deadlock, which is silent and therefore needs a test.
- **Focus regression:** assert `document.hasFocus()` is `true` after engine startup. One line, and it
  pins §12.1 so the fix cannot silently regress.
- **URL sanitiser:** table-driven unit tests in `tf-proto` style; userinfo, fragment, param, path.

All of these run headless and need no graphics-capable terminal, matching the existing e2e harness's
design (`tests/e2e/input-injection.js:5-7`).

---

## 10. Licensing

There is **no `LICENSE` file** at the repo root, though `Cargo.toml` declares
`license = "MIT OR Apache-2.0"`. Nothing in this specification requires third-party code: the format
is ours, the parser extends existing helpers, and the only external surface is CDP, which is a
protocol rather than a dependency. If the commander later considers reusing an existing recorder
(Playwright's codegen is Apache-2.0, Selenium IDE is Apache-2.0), that is compatible with the
declared dual licence — but the absence of the actual `LICENSE` file should be fixed regardless.

---

## 11. Rollout

| Phase | Deliverable | Depends on |
|---|---|---|
| 0 | `webContents.focus()` + focus regression test | nothing (§12.1) |
| 1 | CDP attach-after-document, isolated world, tracker, `{"t":"focus"}` event | 0 |
| 2 | Latch + tap at `send()`; `--record` writing `.bgscript` | 1 |
| 3 | Redaction rules §5 + the §9.1 sentinel test | 2 |
| 4 | Replay: waits, `{{secret:}}` resolution | 3 |
| 5 | Assertions incl. `quiet` and `pixel` | 4 |
| 6 | Frames behind `--record-frames` with §5.7 masking | 5 |

Phase 3 must not ship before the §9.1 test is green, and phase 6 should not ship at all until someone
argues successfully that it is worth the risk in §5.7.

---

## 12. Changes for the commander (I made none of these)

### 12.1 The one-line fix — do this first

`apps/engine/src/main.js:101-134`, `createWindow()`, never calls `focus()`. Measured consequences
(§3.3, §3.4): `document.hasFocus()` is permanently `false`, `focus`/`focusin` events **never fire**,
and in at least one configuration clicks focus nothing and injected characters are silently
discarded.

```js
// in createWindow(), after b.webContents.on('paint', onPaint):
b.webContents.focus();
```

**Caveat, stated plainly:** my probes called `win.focus()` **and** `wc.focus()` together, so I have
**not** isolated whether `webContents.focus()` alone is sufficient **`[UNVERIFIED]`**. Test both. Prefer
`webContents.focus()` alone if it suffices, because `win.focus()` on a window belonging to a
dock-hidden app is a plausible route to stealing OS focus from the user's terminal — which would be a
bad regression in a product whose entire premise is *not* leaving the terminal.

### 12.2 Other engine changes

- Attach CDP **after** a document exists (§3.1) — otherwise the engine deadlocks silently.
- Register the isolated world (`bg_recorder`) + `Runtime.addBinding`; forward focus reports as
  `{"t":"focus",…}` carrying **classification only, never values**.
- Do **not** build on `MutationObserver` in that world; it observes nothing (§3.6).
- Independently of E06: `session.cookies.get({})` returned 12 cleartext cookies including other
  origins' (§3.7). Whatever E06 does, that jar is worth a look from whoever owns session isolation.

### 12.3 Core changes

- Recorder tap at `apps/cli/src/main.rs:436` (`send`) with the latch fed from the new focus event.
- `crates/tf-proto`: add `json_get_u64` / `json_get_i64` / `json_get_f64` beside the existing flat
  helpers (`:120-170`) — enough to parse `.bgscript`, no new dependency.
- URL sanitiser (§5.8) belongs in `tf-proto` next to the escaping helpers, so the recorder, the
  status line, and the replay reporter all use one implementation.

---

## 13. What I could not verify

| Claim | Status |
|---|---|
| `webContents.focus()` **alone** (without `win.focus()`) suffices | **`[UNVERIFIED]`** — probes called both (§12.1) |
| The focus tracker works in **cross-origin iframes** | **`[UNVERIFIED]`** — single-frame only; highest-priority follow-up (§8) |
| The recorder's CDP session coexists with E01's broker attach | **`[UNVERIFIED]`** — two `attach()` on one `webContents` untested |
| Which variable drives the §3.4 input non-determinism | **`[UNVERIFIED]`** — `data:` vs `http:` suspected, one trial each |
| Mechanism behind the isolated-world MutationObserver silence | **`[UNVERIFIED]`** — behaviour measured (0/3), cause not confirmed |
| `pixel` assertion stability across machines/fonts | **`[UNVERIFIED]`** — single host |
| Behaviour of any of this over SSH or in tmux | **`[UNVERIFIED]`** — not exercised |
| `tests/e2e/input-injection.js` currently passes on this host | **not run** — it writes into a tree I do not own (§3.4) |

Everything in §3 that is **not** in this table was measured on this host, and the probe transcripts
are reproducible from the scratchpad sources listed in §2.
