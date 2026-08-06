/**
 * One source of truth for every claim the landing page makes.
 *
 * Everything here is copied from README.md / RELEASE.md / packages/mcp/README.md
 * — the site is for an experimental alpha, so it is allowed to be confident but
 * not to invent capability. If a claim changes upstream, change it here once.
 */

export const PRODUCT = {
  name: 'Terminal-Fenster',
  headline: { lead: 'A real browser', trail: 'inside your terminal.' },
  blurb:
    'Not Lynx. Not a screenshot viewer. Chromium 150 renders offscreen, a Rust core paints Kitty graphics, and your keyboard and mouse go straight back to the page.',
  repo: 'https://github.com/zent7x/terminal-fenster',
  docs: '/docs/',
  license: 'MIT',
} as const;

export const NAV = [
  { href: '#how', label: 'How it works' },
  { href: '#terminals', label: 'Terminals' },
  { href: '#automation', label: 'Automation' },
  { href: '#honest', label: 'Known gaps' },
  { href: '#faq', label: 'FAQ' },
  { href: '#install', label: 'Install' },
] as const;

export const REQUIREMENTS = ['Rust 1.80+', 'Node 22.12+', 'macOS or Linux'] as const;

/* ---------------------------------------------------------------- proof -- */

export const PROOF = [
  { value: '202', label: 'Rust checks', note: 'no terminal needed' },
  { value: '16', label: 'MCP tools', note: 'over stdio JSON-RPC' },
  { value: '16', label: 'live tabs', note: 'terminal-native strip' },
  { value: '281 MB', label: 'idle RSS', note: 'about:blank upper bound' },
] as const;

/* ----------------------------------------------------------- architecture */

export const STACK = [
  {
    id: 'tty',
    name: 'your terminal',
    lang: null,
    body: 'TTY in raw mode, owned by an RAII guard that restores it on every exit path — including ctrl+q, panic, SIGINT, SIGTERM and SIGHUP.',
  },
  {
    id: 'core',
    name: 'terminal-fenster',
    lang: 'Rust',
    body: 'Capability detection, the hybrid compositor, input decoding and Kitty encoding. Retains an RGB canvas and ships only the tiles that actually changed.',
  },
  {
    id: 'engine',
    name: 'engine host',
    lang: 'Electron',
    body: 'Offscreen Chromium with the sandbox on. No Node integration in web content, context isolation enforced, every privileged permission denied by default.',
  },
] as const;

export const LINKS_BETWEEN = [
  'unix socket, 0600, inside a 0700 dir — no network listener is ever opened',
  'JSON for control, length-prefixed binary for frames',
] as const;

export const PILLARS = [
  {
    id: 'chromium',
    tag: 'ENGINE',
    title: 'Real Chromium',
    body: 'Electron 43 / Chromium 150 rendering offscreen. Frames arrive as raw BGRA from Chromium’s paint event — the same engine your desktop browser runs.',
  },
  {
    id: 'input',
    tag: 'INPUT',
    title: 'Your input, unchanged',
    body: 'Click, hover, drag, scroll and ordered typing map back to the live page. Capability is probed by protocol query, never by matching $TERM.',
  },
  {
    id: 'mcp',
    tag: 'AGENTS',
    title: 'Automatable',
    body: 'Sixteen browser tools over stdio JSON-RPC. Semantic inspection goes through Electron’s in-process debugger — no DevTools TCP port is ever opened.',
  },
] as const;

/* -------------------------------------------------------------- terminals */

export type TerminalStatus = 'verified' | 'protocol' | 'capability' | 'expected';

export const STATUS_LABEL: Record<TerminalStatus, string> = {
  verified: 'Verified end-to-end',
  protocol: 'Protocol-verified',
  capability: 'Capability-verified',
  expected: 'Expected, untested',
};

/**
 * `logo` points at the project's real mark, not an approximation of one.
 * See `public/logos/README.md` for provenance.
 */
export const TERMINALS: ReadonlyArray<{
  id: string;
  name: string;
  version: string;
  logo: string;
  graphics: string;
  keyboard: boolean;
  pixelMouse: boolean;
  status: TerminalStatus;
  note: string;
}> = [
  {
    id: 'ghostty',
    name: 'Ghostty',
    version: '1.3.1',
    logo: '/logos/ghostty.png',
    graphics: 'Kitty',
    keyboard: true,
    pixelMouse: true,
    status: 'verified',
    note: 'The primary target, and the only interactive configuration verified end-to-end today.',
  },
  {
    id: 'kitty',
    name: 'kitty',
    version: '—',
    logo: '/logos/kitty.png',
    graphics: 'Kitty',
    keyboard: true,
    pixelMouse: true,
    status: 'expected',
    note: 'Speaks the same protocol and should work through the same path. Needs a community run.',
  },
  {
    id: 'wezterm',
    name: 'WezTerm',
    version: '—',
    logo: '/logos/wezterm.png',
    graphics: 'Kitty',
    keyboard: true,
    pixelMouse: true,
    status: 'expected',
    note: 'Graphics path implemented against the protocol, not yet verified on real hardware.',
  },
  {
    id: 'iterm2',
    name: 'iTerm2',
    version: '3.6.9',
    logo: '/logos/iterm2.svg',
    graphics: 'Kitty',
    keyboard: true,
    pixelMouse: false,
    status: 'protocol',
    note: 'Reports SGR-Pixels mouse permanently reset, so coordinates arrive as cells. Treating those as pixels would collapse the page into its top-left corner — the pointer mapping handles both, with tests pinning the difference.',
  },
  {
    id: 'apple-terminal',
    name: 'Apple Terminal',
    version: '465',
    logo: '/logos/apple-terminal.png',
    graphics: 'none',
    keyboard: false,
    pixelMouse: false,
    status: 'capability',
    note: 'No graphics protocol at all, so interactive open refuses it and points you at --headless. Still drivable over MCP.',
  },
];

export const DETECTION_NOTE =
  'Every capability above comes from the terminal answering a protocol query, not from matching $TERM. Detection lives in crates/tf-term/src/caps.rs.';

/* ------------------------------------------------------------ performance */

export const BENCH_ENV =
  'Ghostty 1.3.1 · macOS 26.1 · Apple M4 (10 cores, 24 GB) · release build';

export const BENCH = [
  {
    id: 'local',
    workload: 'Local 80×80 animation, default 4×4-cell mosaic',
    viewport: '2108×1332',
    fps: '46.5',
    trailing: '54',
    wire: '2,540 B',
    encode: '0.18 ms',
    good: true,
  },
  {
    id: 'repaint',
    workload: 'Full-viewport repaint, direct/zlib monolithic baseline',
    viewport: '2108×1406',
    fps: '7.2',
    trailing: '7',
    wire: '82,086 B',
    encode: '2.68 ms',
    good: false,
  },
] as const;

export const BENCH_NOTE =
  'Real core-to-terminal runs, not engine-only paint rates. The localized workload is the intended fast path — Chromium sends damage, the core updates a retained RGB canvas, and only intersecting tiles cross the terminal boundary. The full-viewport row is the old bottleneck shown honestly: encoding is only 2.68 ms, but base64/APC presentation held throughput near 7 fps.';

/* ------------------------------------------------------------- automation */

export const MCP_TOOLS = [
  { name: 'browser_navigate', desc: 'Open a URL, starting the browser if needed' },
  { name: 'browser_snapshot', desc: 'Accessibility tree with [ref=eN] handles' },
  { name: 'browser_find', desc: 'Locate elements by accessible name' },
  { name: 'browser_click', desc: 'Click by ref' },
  { name: 'browser_type', desc: 'Focus by ref and type, optionally clear or submit' },
  { name: 'browser_press_key', desc: 'Enter, Tab, Escape, arrows, function keys' },
  { name: 'browser_scroll', desc: 'Scroll the viewport' },
  { name: 'browser_click_xy', desc: 'Raw coordinate click — canvas and video escape hatch' },
  { name: 'browser_screenshot', desc: 'PNG rebuilt from the exact damage-frame stream' },
  { name: 'browser_navigate_back', desc: 'History back' },
  { name: 'browser_navigate_forward', desc: 'History forward' },
  { name: 'browser_reload', desc: 'Reload the page' },
  { name: 'browser_resize', desc: 'Match the viewport to terminal geometry' },
  { name: 'browser_wait_for', desc: 'Wait for text, or for a duration' },
  { name: 'browser_status', desc: 'URL, title, viewport, engine versions, CDP state' },
  { name: 'browser_close', desc: 'Shut the Chromium tree down' },
] as const;

export const KEYS = [
  { k: ['ctrl', 'q'], action: 'Quit' },
  { k: ['ctrl', 'c'], action: 'Copy selection' },
  { k: ['ctrl', 'r'], action: 'Reload' },
  { k: ['ctrl', 'l'], action: 'Open URL — omnibox on the status row' },
  { k: ['ctrl', 'f'], action: 'Find in page — ctrl+n / ctrl+p to step' },
  { k: ['ctrl', '='], action: 'Zoom in — ctrl+- out, ctrl+0 reset' },
  { k: ['ctrl', '←'], action: 'Back — ctrl+→ forward' },
  { k: ['mouse'], action: 'Click, hover, drag, scroll — forwarded to the page' },
] as const;

/* ------------------------------------------------------------------ trust */

export const TESTS = [
  { n: '202', what: 'Rust checks', cmd: 'cargo test --workspace --locked' },
  { n: '14', what: 'engine unit tests', cmd: 'cd apps/engine && npm test' },
  { n: '20', what: 'real-pixel E2E checks', cmd: 'node tests/e2e/input-injection.js' },
  { n: '14', what: 'browser fixtures', cmd: 'electron tests/fixtures/verify-fixtures.js' },
  { n: '40', what: 'MCP unit + protocol checks', cmd: 'cd packages/mcp && npm test' },
  { n: '28', what: 'live tools vs real Chromium', cmd: 'npm run test:live' },
] as const;

export const TESTS_NOTE =
  'The E2E suite speaks the engine wire protocol directly, so it runs without a graphics terminal and works in CI. It asserts on frame pixels: a click must actually change the colour under the cursor, and clicking one target must not activate another.';

export const SECURITY = [
  {
    title: 'The sandbox stays on',
    body: 'Web content gets no Node integration and context isolation is enforced. Camera, microphone, location, clipboard and device permissions are denied by explicit session handlers.',
  },
  {
    title: 'Page text cannot drive your terminal',
    body: 'Titles and URLs are sanitized before they reach the TTY — a malicious title otherwise smuggles escape sequences through and can drive OSC 52 to overwrite your clipboard. C0/C1 controls, bidi overrides and invisible formatting are stripped.',
  },
  {
    title: 'No network listener, ever',
    body: 'The control socket is 0600 inside a 0700 directory. MCP inspection goes through Electron’s in-process debugger over that same socket; no DevTools TCP endpoint is exposed.',
  },
  {
    title: 'Bounded by construction',
    body: 'Shared-memory frame objects are 0600 with collision-checked names, capped at four outstanding before direct fallback. The frame reader caps message size, so a bad length prefix cannot become a 4 GiB allocation.',
  },
] as const;

/** The nine known gaps from README.md, ordered by how much they matter. */
export const GAPS = [
  {
    title: 'The hybrid compositor still needs eyes on a graphics terminal',
    body: 'Capture-side damage, retained RGB, the dense-base/sparse-overlay switch, encoder round trips, real POSIX shared memory and bounded fallback are all tested. On-screen placement and teardown after switching between shared bases and tile overlays still need a Ghostty run before release.',
  },
  {
    title: 'Full-viewport motion needs a fresh measurement',
    body: 'The direct fallback measured about 7 fps at 2108×1406. The new local shared-memory path removes pixels, compression, base64 and APC chunking from the PTY, but no honest displayed-FPS number exists yet. RELEASE.md makes 20 displayed fps and a 2× improvement the go/no-go threshold.',
  },
  {
    title: 'SSH adaptive transport is wired but unmeasured on a WAN',
    body: 'MBDT, byte-credit, the fps ladder and OSR scale-downs are implemented on the direct Kitty path. WAN measurement is still open; TERMINAL_FENSTER_LAG_BUDGET_MS (default 100) tunes the credit window.',
  },
  {
    title: 'Tabs are intentionally basic',
    body: 'The interactive CLI has a tab strip and supports up to 16 live tabs, but there is no persisted restore, no bookmarks and no richer management. MCP sessions stay single-tab and isolated from interactive ones.',
  },
  {
    title: 'Sixel and iTerm2 backends are unimplemented',
    body: 'If detection picks one, the CLI explicitly degrades to Unicode half-blocks and doctor says so, rather than silently faking it.',
  },
  {
    title: 'onPaint still copies via toBitmap()',
    body: 'The shared-texture path is not shipped.',
  },
  {
    title: 'Public artifacts are not signed or notarized',
    body: 'The macOS arm64 archive pipeline is proven locally, but other target archives and a genuinely clean-machine install remain release gates.',
  },
  {
    title: 'Terminal coverage is narrow',
    body: 'Ghostty on macOS is verified end-to-end. kitty, WezTerm, Linux interactive rendering and the iTerm2 fallback need community testing before their support can be promoted.',
  },
  {
    title: 'The Electron memory floor is still high',
    body: 'A short 1280×800 about:blank probe measured 280.6 MB peak/steady summed RSS — an upper bound, because shared pages are double-counted. The idle frame throttle works, but a separately verified low-memory mode is not shipped.',
  },
] as const;

/* ---------------------------------------------------------------- install */

export type InstallPathId = 'source' | 'archive';

/**
 * The two install paths that actually exist in the repo. `curl | bash` is
 * deliberately absent — README.md is explicit that it must not be presented
 * as a reproducible release until a signed tag exists.
 */
export const INSTALL_PATHS: ReadonlyArray<{
  id: InstallPathId;
  label: string;
  blurb: string;
  facts: ReadonlyArray<{ k: string; v: string }>;
  lines: ReadonlyArray<{ kind: 'cmd' | 'comment'; text: string }>;
}> = [
  {
    id: 'source',
    label: 'From source',
    blurb:
      'Builds with the lockfile, stages and materializes Electron, then atomically swaps the prefix. A failed upgrade restores the previous one.',
    facts: [
      { k: 'Needs', v: 'Rust 1.80+, Node 22.12+' },
      { k: 'Installs to', v: '~/.local/share/terminal-fenster' },
      { k: 'Links', v: '~/.local/bin/terminal-fenster' },
      { k: 'Uninstall', v: './uninstall.sh' },
    ],
    lines: [
      { kind: 'cmd', text: 'git clone https://github.com/zent7x/terminal-fenster.git' },
      { kind: 'cmd', text: 'cd terminal-fenster' },
      { kind: 'cmd', text: './install.sh' },
    ],
  },
  {
    id: 'archive',
    label: 'Prebuilt archive',
    blurb:
      'Produces a host-native archive plus a SHA-256 sidecar under dist/. Installing from it needs neither Rust, Node, npm, a checkout, nor network access.',
    facts: [
      { k: 'Needs', v: 'Nothing at install time' },
      { k: 'Produces', v: 'dist/*.tar.gz + .sha256' },
      { k: 'Contains', v: 'Launcher + pinned Electron' },
      { k: 'Signing', v: 'Ad-hoc only, not notarized' },
    ],
    lines: [
      { kind: 'cmd', text: 'tools/package-release.sh' },
      { kind: 'cmd', text: 'shasum -a 256 -c dist/*.tar.gz.sha256' },
    ],
  },
];

export const RUN_LINES = [
  { kind: 'comment', text: '# interactive — Ghostty, kitty, WezTerm' },
  { kind: 'cmd', text: 'terminal-fenster setup' },
  { kind: 'cmd', text: 'terminal-fenster open news.ycombinator.com' },
  { kind: 'comment', text: '# headless anywhere, and how agents get wired in' },
  { kind: 'cmd', text: 'terminal-fenster open example.com --headless' },
  { kind: 'cmd', text: 'terminal-fenster mcp-config' },
] as const;

/**
 * FAQ, for the ScrollFAQAccordion at the foot of the page. Answers are short
 * because the component renders them in a fixed-width bubble — anything longer
 * than ~2 lines starts to fight the layout.
 */
export const FAQ = [
  {
    id: 1,
    question: 'Is this just Lynx with extra steps?',
    answer:
      'No. Lynx renders a text approximation of the DOM. This is Chromium compositing real frames, sent to your terminal as pixels.',
  },
  {
    id: 2,
    question: 'Which terminals actually work today?',
    answer:
      'Ghostty on macOS is verified end-to-end. kitty and WezTerm should work through the same protocol but are untested.',
  },
  {
    id: 3,
    question: 'Does it work over SSH?',
    answer:
      'The adaptive transport is wired — byte-credit, an fps ladder and OSR scale-downs. WAN performance is not measured yet.',
  },
  {
    id: 4,
    question: 'What if my terminal has no graphics protocol?',
    answer:
      'It runs headless and refuses to fake it. Apple Terminal takes that path, and agents drive it over MCP instead.',
  },
  {
    id: 5,
    question: 'How much memory does it take?',
    answer:
      'About 281 MB idle for about:blank — an upper bound, since shared pages get double-counted. The floor is still high.',
  },
  {
    id: 6,
    question: 'Is it safe to point at untrusted pages?',
    answer:
      'The sandbox stays on, permissions are denied by default, page text is sanitized before reaching your TTY, and no network listener is ever opened.',
  },
  {
    id: 7,
    question: 'Can I download a binary?',
    answer:
      'Not yet. Nothing is signed or notarized, so you build from source or from a local archive until the release gates pass.',
  },
] as const;

export const ALPHA_NOTICE =
  'Experimental source alpha. Ghostty on macOS is verified end-to-end; everything else needs community testing. No signed binaries yet.';
