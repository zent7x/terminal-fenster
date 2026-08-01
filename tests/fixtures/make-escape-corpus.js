#!/usr/bin/env node
/*
 * Generates escape-corpus.json and injects the identical text into escape-injection.html.
 *
 *   node tests/fixtures/make-escape-corpus.js
 *
 * The corpus is the input side of the tty-safe projection test (A09 s1.3). Every entry is
 * a string that a hostile page can put into a sink Terminal-Fenster renders to the terminal:
 * document.title, location, a link, a filename, a console message, an alert.
 *
 * Encoding: payloads are UTF-16 code unit sequences, expressed as repeated segments so a
 * 64 KiB flood costs one segment instead of 65536 array entries. A code-unit encoding is
 * used rather than a JSON string because two of the cases (lone surrogate, NUL) cannot
 * survive a round trip through a JSON string in every parser.
 *
 *   segments: [ { u: "001b005d" }, { u: "0041", n: 65536 } ]
 *   u        = hex, exactly 4 digits per UTF-16 code unit, big-endian, no separator
 *   payload  = concat over segments of (decode(u) repeated n times)
 *
 * Canaries are all `echo`-only. Nothing in this file destroys anything if it is ever
 * pasted into a shell and run by accident -- that is deliberate, and any future addition
 * must keep that property.
 */
'use strict';
const fs = require('fs');
const path = require('path');

const b64 = (s) => Buffer.from(s, 'utf8').toString('base64');

/* helpers to build segment lists */
const S = (str) => ({ u: Array.from(str, (c) => c.charCodeAt(0)), n: 1 });
const U = (arr, n) => ({ u: arr, n: n === undefined ? 1 : n });

const ESC = 0x1b, BEL = 0x07;

const CASES = [
  {
    id: 'osc52-write-7bit',
    threat: 'A09 T1 / T-ESC-1',
    severity: 'critical',
    title: 'OSC 52 clipboard write, 7-bit introducer',
    description:
      'ESC ] 5 2 ; c ; <base64> BEL. On Ghostty 1.3.1 clipboard-write defaults to allow, ' +
      'with no prompt, so an unsanitised title is a silent clipboard-write primitive.',
    canary: 'TERMINAL_FENSTER_CANARY_OSC52',
    segments: [U([ESC]), S(']52;c;' + b64('echo TERMINAL_FENSTER_CANARY_OSC52')), U([BEL]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b, 0x07],
    mustSurviveSubstring: 'SAFE-TAIL',
    verify: 'pbcopy </dev/null before the run; pbpaste after must be empty'
  },
  {
    id: 'osc52-write-c1',
    threat: 'A09 T2 / T-ESC-2',
    severity: 'critical',
    title: 'OSC 52 via 8-bit C1 introducer',
    description:
      'U+009D (OSC) ... U+009C (ST). Defeats any sanitiser that only strips 0x1B. ' +
      'Whether a UTF-8 terminal decodes C2 9D as C1 is terminal-specific and UNVERIFIED ' +
      'on Ghostty/iTerm2/Apple Terminal; strip it regardless, it costs nothing.',
    canary: 'TERMINAL_FENSTER_CANARY_C1',
    segments: [U([0x9d]), S('52;c;' + b64('echo TERMINAL_FENSTER_CANARY_C1')), U([0x9c]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x9d, 0x9c],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'osc52-write-st',
    threat: 'A09 T1',
    severity: 'critical',
    title: 'OSC 52 terminated by 7-bit ST instead of BEL',
    description: 'Same primitive, ESC \\ terminator. Parsers that only look for BEL miss it.',
    canary: 'TERMINAL_FENSTER_CANARY_ST',
    segments: [U([ESC]), S(']52;c;' + b64('echo TERMINAL_FENSTER_CANARY_ST')), U([ESC, 0x5c]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'osc52-read',
    threat: 'A09 T4 / T-ESC-4',
    severity: 'high',
    title: 'OSC 52 clipboard READ request',
    description:
      'ESC ] 5 2 ; c ; ? BEL. The terminal answers by writing the clipboard, base64 ' +
      'encoded, into our stdin. Our stdin parser is therefore a security boundary too.',
    canary: null,
    segments: [U([ESC]), S(']52;c;?'), U([BEL]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b, 0x07],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'osc2-title-set-and-report',
    threat: 'A09 T3 / T-ESC-3',
    severity: 'critical',
    title: 'Set window title, then request a title report (CSI 21 t)',
    description:
      'The reply lands on the shell stdin as if typed. CyberArk documented this across ' +
      'PuTTY CVE-2021-33500, MobaXterm CVE-2021-28847, MinTTY CVE-2021-28848, ' +
      'ZOC CVE-2021-32198, Xshell CVE-2021-42095. Ghostty ships title-report=false, but ' +
      'that is the terminal being careful, not us.',
    canary: 'TERMINAL_FENSTER_CANARY_T3',
    segments: [U([ESC]), S(']2;echo TERMINAL_FENSTER_CANARY_T3'), U([BEL]), U([ESC]), S('[21t')],
    mustStripCodeUnits: [0x1b, 0x07]
  },
  {
    id: 'bracketed-paste-escape',
    threat: 'CVE-2021-31701 / CVE-2021-37326 / CVE-2021-40147 class',
    severity: 'high',
    title: 'Break out of bracketed paste mode',
    description:
      'CSI 201~ ends a bracketed paste early, so the remainder is treated as typed input ' +
      'rather than pasted text.',
    canary: 'TERMINAL_FENSTER_CANARY_BP',
    segments: [U([ESC]), S('[201~echo TERMINAL_FENSTER_CANARY_BP'), U([0x0a])],
    mustStripCodeUnits: [0x1b, 0x0a]
  },
  {
    id: 'dcs-sixel',
    threat: 'Terminal-Fenster-specific: collides with our own graphics stream',
    severity: 'high',
    title: 'DCS / sixel introducer inside untrusted text',
    description:
      'ESC P q ... ESC \\. A page-controlled DCS can desynchronise the sixel backend ' +
      'mid-image, because our own image data and this text share one byte stream.',
    canary: null,
    segments: [U([ESC]), S('Pq#0;2;100;0;0#0~~@@vv@@~~@@~~$'), U([ESC, 0x5c]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'apc-kitty-graphics',
    threat: 'Terminal-Fenster-specific: collides with the kitty graphics backend',
    severity: 'high',
    title: 'APC / kitty graphics introducer inside untrusted text',
    description:
      'ESC _ G ... ESC \\. The kitty backend is our primary renderer, so a page that can ' +
      'emit APC can inject or truncate an image transmission. This case is not in the ' +
      'usual terminal-injection literature; it exists because of what Terminal-Fenster is.',
    canary: null,
    segments: [U([ESC]), S('_Ga=T,f=24,s=1,v=1;AAAA'), U([ESC, 0x5c]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'osc8-hyperlink-spoof',
    threat: 'URL-bar / link-preview spoofing',
    severity: 'medium',
    title: 'OSC 8 hyperlink with a target that differs from its label',
    description:
      'ESC ] 8 ; ; <uri> ST <label> ESC ] 8 ; ; ST. The rendered label and the actual ' +
      'target differ, which is exactly the deception a browser chrome exists to prevent.',
    canary: null,
    segments: [U([ESC]), S(']8;;file:///etc/passwd'), U([ESC, 0x5c]), S('https://example.com'),
               U([ESC]), S(']8;;'), U([ESC, 0x5c])],
    mustStripCodeUnits: [0x1b]
  },
  {
    id: 'osc1337-iterm2',
    threat: 'A09 s1.2 H',
    severity: 'medium',
    title: 'iTerm2 proprietary OSC 1337 sequences',
    description:
      'StealFocus and SetProfile. Exploitability on iTerm2 3.6.9 is UNVERIFIED here ' +
      '(TCC blocks automation of iTerm2 on this host), but stripping ESC makes it moot.',
    canary: null,
    segments: [U([ESC]), S(']1337;StealFocus'), U([ESC, 0x5c]),
               U([ESC]), S(']1337;SetProfile=Default'), U([ESC, 0x5c])],
    mustStripCodeUnits: [0x1b]
  },
  {
    id: 'csi-clear-and-home',
    threat: 'A09 T6 (availability) / chrome spoofing',
    severity: 'medium',
    title: 'Clear screen and home the cursor',
    description: 'CSI 2 J CSI H. Erases the user shell scrollback context around us.',
    canary: null,
    segments: [U([ESC]), S('[2J'), U([ESC]), S('[H'), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x1b],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'csi-sgr-reverse',
    threat: 'chrome spoofing',
    severity: 'low',
    title: 'SGR reverse video and colour',
    description:
      'CSI 7 m / CSI 31 m. Lets page text imitate our own reverse-video status bar.',
    canary: null,
    segments: [U([ESC]), S('[7m'), S('FAKE STATUS BAR'), U([ESC]), S('[0m')],
    mustStripCodeUnits: [0x1b]
  },
  {
    id: 'charset-designate-linedraw',
    threat: 'A09 s1.2 E',
    severity: 'low',
    title: 'Designate the DEC line-drawing charset',
    description: 'ESC ( 0 makes subsequent ASCII render as box-drawing glyphs.',
    canary: null,
    segments: [U([ESC]), S('(0lqqqk'), U([ESC]), S('(B')],
    mustStripCodeUnits: [0x1b]
  },
  {
    id: 'so-si-shift',
    threat: 'A09 s1.2 E',
    severity: 'medium',
    title: 'SO / SI charset shift without any ESC',
    description:
      '0x0E / 0x0F. Defeats a sanitiser that only looks for ESC, because no ESC is present.',
    canary: null,
    segments: [U([0x0e]), S('LINEDRAW'), U([0x0f]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x0e, 0x0f],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'enq-answerback',
    threat: 'A09 s1.2 E',
    severity: 'medium',
    title: 'ENQ triggers an answerback string',
    description: '0x05. Some terminals transmit their answerback into the host, unsolicited.',
    canary: null,
    segments: [S('BEFORE'), U([0x05]), S('AFTER')],
    mustStripCodeUnits: [0x05]
  },
  {
    id: 'cr-overwrite',
    threat: 'A09 s1.2 E',
    severity: 'medium',
    title: 'Carriage return rewrites the status line',
    description: '0x0D returns to column 0 so the rest of the string overwrites what we drew.',
    canary: null,
    segments: [S('SAFE-PREFIX'), U([0x0d]), S('EVIL-OVERWRITE')],
    mustStripCodeUnits: [0x0d]
  },
  {
    id: 'bs-url-spoof',
    threat: 'A09 s1.2 E',
    severity: 'medium',
    title: 'Backspace rewrites a rendered origin',
    description:
      'Eleven 0x08 bytes erase "example.com" from the display and leave "evil.com" in its ' +
      'place, with no escape sequence involved at all.',
    canary: null,
    segments: [S('example.com'), U([0x08], 11), S('evil.com')],
    mustStripCodeUnits: [0x08]
  },
  {
    id: 'c0-zoo',
    threat: 'A09 s1.3 stage 2',
    severity: 'high',
    title: 'Every C0 control plus DEL',
    description:
      'U+0000 through U+001F and U+007F, each between visible markers. The sanitiser must ' +
      'replace all of them, with no exceptions carved out for TAB, LF or CR.',
    canary: null,
    segments: (function () {
      const out = [S('[')];
      for (let c = 0x00; c <= 0x1f; c++) { out.push(U([c])); }
      out.push(U([0x7f]));
      out.push(S(']SAFE-TAIL'));
      return out;
    })(),
    mustStripCodeUnits: Array.from({ length: 32 }, (_, i) => i).concat([0x7f]),
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'c1-zoo',
    threat: 'A09 s1.3 stage 2',
    severity: 'high',
    title: 'Every C1 control',
    description: 'U+0080 through U+009F, including DCS 0090, CSI 009B, ST 009C, OSC 009D.',
    canary: null,
    segments: (function () {
      const out = [S('[')];
      for (let c = 0x80; c <= 0x9f; c++) { out.push(U([c])); }
      out.push(S(']SAFE-TAIL'));
      return out;
    })(),
    mustStripCodeUnits: Array.from({ length: 32 }, (_, i) => 0x80 + i),
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'bidi-trojan-source',
    threat: 'A09 T5 / CVE-2021-42574',
    severity: 'high',
    title: 'Right-to-left override reverses a rendered origin',
    description:
      'U+202E makes "https://example.com/gro.live//:sptth" read as if it ends at ' +
      'evil.org. No control bytes are involved, so a C0/C1 filter alone does not help.',
    canary: null,
    segments: [S('https://example.com'), U([0x202e]), S('/gro.live//:sptth')],
    mustStripCodeUnits: [0x202e]
  },
  {
    id: 'bidi-isolates',
    threat: 'A09 T5',
    severity: 'medium',
    title: 'Directional isolates and marks',
    description: 'U+2066..U+2069, U+200E, U+200F, U+061C.',
    canary: null,
    segments: [S('a'), U([0x2066]), S('b'), U([0x2067]), S('c'), U([0x2068]), S('d'),
               U([0x2069]), U([0x200e]), U([0x200f]), U([0x061c]), S('SAFE-TAIL')],
    mustStripCodeUnits: [0x2066, 0x2067, 0x2068, 0x2069, 0x200e, 0x200f, 0x061c],
    mustSurviveSubstring: 'SAFE-TAIL'
  },
  {
    id: 'zero-width',
    threat: 'A09 T5 / homograph',
    severity: 'medium',
    title: 'Zero-width characters inside a hostname',
    description:
      'U+200B..U+200D and U+FEFF split a hostname invisibly, so a naive prefix match on ' +
      '"example.com" fails while the user reads exactly that.',
    canary: null,
    segments: [S('ex'), U([0x200b]), S('am'), U([0x200c]), S('ple'), U([0x200d]), S('.com'),
               U([0xfeff])],
    mustStripCodeUnits: [0x200b, 0x200c, 0x200d, 0xfeff]
  },
  {
    id: 'private-use-area',
    threat: 'A09 s1.3 stage 2',
    severity: 'low',
    title: 'Private Use Area glyphs',
    description:
      'U+E000 and U+F8FF render as whatever the user font decides, including a convincing ' +
      'padlock. Recommended for stripping in a security-indicator context.',
    canary: null,
    segments: [U([0xe000]), S('secure'), U([0xf8ff])],
    mustStripCodeUnits: [0xe000, 0xf8ff]
  },
  {
    id: 'zalgo-combining',
    threat: 'A09 s1.3 stage 3 (width)',
    severity: 'medium',
    title: 'Forty combining marks on one base character',
    description:
      'Overflows a status line vertically in terminals that do not clamp combining runs. ' +
      'The width rule, not the filter, is what defends here.',
    canary: null,
    segments: [S('A'), U([0x0301], 40), S('SAFE-TAIL')],
    mustSurviveSubstring: 'SAFE-TAIL',
    note: 'combining marks are legal text; the requirement is a width clamp, not removal'
  },
  {
    id: 'lone-surrogate',
    threat: 'A09 s1.3 stage 1',
    severity: 'medium',
    title: 'Unpaired UTF-16 high surrogate',
    description:
      'U+D800 with no low surrogate. Chromium hands us UTF-16; a naive transcode can ' +
      'produce invalid UTF-8 or panic. Must become U+FFFD.',
    canary: null,
    segments: [S('before'), U([0xd800]), S('after')],
    expectReplacementChar: true
  },
  {
    id: 'line-paragraph-separator',
    threat: 'A09 s1.3 stage 2',
    severity: 'low',
    title: 'U+2028 and U+2029',
    description: 'Line and paragraph separators; must become spaces, not newlines.',
    canary: null,
    segments: [S('A'), U([0x2028]), S('B'), U([0x2029]), S('C')],
    mustStripCodeUnits: [0x2028, 0x2029]
  },
  {
    id: 'overlong-title',
    threat: 'A09 T6 / T-DOS-1',
    severity: 'medium',
    title: '16 KiB single-line title',
    description:
      'Exceeds the 8 KiB per-string cap. The cap must be applied before the formatter, ' +
      'and truncation must never split a UTF-8 sequence.',
    canary: null,
    segments: [S('LONG-'), U([0x41], 16384), S('-END')],
    maxBytesAfterSanitize: 8192
  },
  {
    id: 'unterminated-osc',
    threat: 'A09 T6',
    severity: 'high',
    title: 'OSC introducer followed by 64 KiB with no terminator',
    description:
      'A terminal buffers an unterminated OSC string unboundedly. Never forward a partial ' +
      'sequence, and never forward the introducer at all.',
    canary: null,
    segments: [U([ESC]), S(']'), U([0x41], 65536)],
    mustStripCodeUnits: [0x1b],
    maxBytesAfterSanitize: 8192
  },
  {
    id: 'benign-control',
    threat: 'control case',
    severity: 'none',
    title: 'Ordinary text that must pass through unchanged',
    description:
      'Latin, CJK, an emoji with a variation selector, and an accented character. A ' +
      'sanitiser that mangles this is over-aggressive, which is its own bug.',
    canary: null,
    segments: [S('Hello, '), S('世界'), S(' café '), S('✅'), S(' ok')],
    mustSurviveSubstring: 'Hello, ',
    note: 'expected output is identical to the input'
  }
];

/* ---- build ---- */
function toEscaped(segments) {
  let out = '';
  for (const seg of segments) {
    const n = seg.n === undefined ? 1 : seg.n;
    let piece = '';
    for (const u of seg.u) {
      if (u >= 0x20 && u < 0x7f && u !== 0x5c) piece += String.fromCharCode(u);
      else if (u <= 0xff) piece += '\\x' + u.toString(16).padStart(2, '0');
      else piece += '\\u' + u.toString(16).padStart(4, '0');
    }
    out += n > 1 ? '(' + piece + ')*' + n : piece;
  }
  return out;
}
function hex(units) {
  return units.map((u) => u.toString(16).padStart(4, '0')).join('');
}
function unitCount(segments) {
  return segments.reduce((a, s) => a + s.u.length * (s.n === undefined ? 1 : s.n), 0);
}

const corpus = {
  schema: 'terminal-fenster.escape-corpus/1',
  generatedBy: 'tests/fixtures/make-escape-corpus.js',
  encoding: 'payload = concat over segments of (UTF-16 code units, repeated n times). ' +
            'u is a hex string, 4 hex digits per UTF-16 code unit, big-endian, no separator.',
  safety: 'every canary is an echo-only string; nothing here is destructive if executed',
  sinks: ['document.title', 'location.hash', 'anchor href', 'anchor text',
          'download filename', 'console.log', 'window.alert', 'page text'],
  cases: CASES.map((c) => ({
    id: c.id,
    severity: c.severity,
    threat: c.threat,
    title: c.title,
    description: c.description,
    canary: c.canary || null,
    codeUnitCount: unitCount(c.segments),
    escaped: toEscaped(c.segments),
    segments: c.segments.map((s) => (s.n === undefined || s.n === 1 ? { u: hex(s.u) } : { u: hex(s.u), n: s.n })),
    expect: {
      mustStripCodeUnits: c.mustStripCodeUnits || [],
      mustSurviveSubstring: c.mustSurviveSubstring || null,
      maxBytesAfterSanitize: c.maxBytesAfterSanitize || null,
      expectReplacementChar: !!c.expectReplacementChar,
      note: c.note || null
    }
  }))
};

const json = JSON.stringify(corpus, null, 2);
const dir = __dirname;
fs.writeFileSync(path.join(dir, 'escape-corpus.json'), json + '\n');

const htmlPath = path.join(dir, 'escape-injection.html');
if (fs.existsSync(htmlPath)) {
  let html = fs.readFileSync(htmlPath, 'utf8');
  const open = '<script type="application/json" id="bg-corpus">';
  const close = '</' + 'script>';
  const i = html.indexOf(open);
  if (i < 0) { console.error('escape-injection.html has no #bg-corpus block'); process.exit(1); }
  const j = html.indexOf(close, i);
  html = html.slice(0, i + open.length) + '\n' + json + '\n' + html.slice(j);
  fs.writeFileSync(htmlPath, html);
  console.log('injected into escape-injection.html');
}

console.log('cases:', corpus.cases.length, 'json bytes:', json.length);
