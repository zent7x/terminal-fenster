#!/usr/bin/env node
// End-to-end test against a REAL Chromium. Launches the engine, drives it through the MCP
// tool handlers, and asserts on what the page actually did.
//
// Everything is asserted against page state, never against "the tool returned 200": a
// click is proven by the page's own title changing, typing is proven by the value that
// comes back in the next accessibility snapshot.
//
// Needs a working Electron install (apps/engine/node_modules) and, in a sandboxed agent
// shell, permission to spawn Chromium child processes.
//
// Run:  node test/live-browser.js
'use strict';

const { server } = require('../index.js');

// Go through the server's own tool dispatch rather than calling handlers directly: that is
// the path a real MCP client takes, including the conversion of a thrown error into an
// isError result. Testing the shortcut would not test what clients actually experience.
const HANDLERS = new Proxy({}, {
  get: (_t, name) => (args) => server.callTool(String(name), args || {}),
});

// A page with one of each thing worth targeting. Clicking the button rewrites the title,
// which is observable through the engine's own event channel -- no CDP required to believe
// the assertion.
const PAGE = `<!doctype html><html><head><title>Start</title></head><body>
<h1>Live Test</h1>
<button id="go" onclick="document.title='CLICKED'">Sign in</button>
<label for="q">Search terms</label>
<input id="q" type="text">
<input id="chk" type="checkbox" aria-label="Remember me">
<a href="#second" id="lnk">Read the docs</a>
<div style="height:2000px"></div>
<p id="deep">Bottom marker</p>
<script>
  document.getElementById('lnk').addEventListener('click', () => { document.title = 'LINKED'; });
</script>
</body></html>`;

const URL1 = 'data:text/html;charset=utf-8,' + encodeURIComponent(PAGE);
const URL2 = 'data:text/html;charset=utf-8,' + encodeURIComponent('<title>Second</title><h1>Second page</h1><button>Elsewhere</button>');

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  -- ' + String(detail).replace(/\n/g, ' ').slice(0, 160) : ''}`);
}
const textOf = (r) => r.content.filter((c) => c.type === 'text').map((c) => c.text).join('\n');
const refFor = (snapText, name) => {
  const line = snapText.split('\n').find((l) => l.includes(name) && l.includes('ref='));
  return line ? (line.match(/ref=(e\d+)/) || [])[1] : null;
};

(async () => {
  // ---- 1. start + navigate ----------------------------------------------------------
  const nav = await HANDLERS.browser_navigate({ url: URL1 });
  const navText = textOf(nav);
  check('browser_navigate starts the engine and loads the page', !nav.isError && /Navigated to/.test(navText), navText.split('\n')[1]);

  const status = textOf(await HANDLERS.browser_status({}));
  check('engine reports Chromium version', /Chrome \d+\./.test(status), (status.match(/engine: .*/) || [])[0]);
  const cdpUp = /page semantics \(CDP\): available/.test(status);
  check('page semantics (CDP) attached', cdpUp, (status.match(/page semantics.*/) || [])[0]);

  if (!cdpUp) {
    console.log('\nCDP unavailable -- the semantic half of the suite cannot run. Aborting.');
    await HANDLERS.browser_close({});
    process.exit(1);
  }

  // ---- 2. snapshot ------------------------------------------------------------------
  const snap = textOf(await HANDLERS.browser_snapshot({}));
  check('snapshot contains the button with a ref', /button "Sign in" \[ref=e\d+\]/.test(snap), (snap.match(/.*Sign in.*/) || [])[0]);
  check('snapshot computes the accessible name from a <label>', /"Search terms"/.test(snap), (snap.match(/.*Search terms.*/) || [])[0]);
  check('snapshot picks up aria-label on the checkbox', /checkbox "Remember me"/.test(snap), (snap.match(/.*Remember me.*/) || [])[0]);
  check('snapshot includes the link', /link "Read the docs" \[ref=e\d+\]/.test(snap));
  check('snapshot exposes heading text', /heading "Live Test"/.test(snap) || /"Live Test"/.test(snap));
  check('page content is fenced as untrusted', snap.includes('<untrusted-page-content'), 'envelope present');
  check('snapshot is compact (< 4 KB for this page)', snap.length < 4096, snap.length + ' bytes');

  console.log('\n--- snapshot, verbatim ---\n' + snap + '\n--- end ---\n');

  // ---- 3. find ----------------------------------------------------------------------
  const found = textOf(await HANDLERS.browser_find({ text: 'sign' }));
  check('browser_find locates the button case-insensitively', /button "Sign in" \[ref=e\d+\]/.test(found), (found.match(/.*Sign in.*/) || [])[0]);

  // ---- 4. click by ref --------------------------------------------------------------
  const btnRef = refFor(snap, 'Sign in');
  const clicked = await HANDLERS.browser_click({ element: 'Sign in button', ref: btnRef });
  const clickedText = textOf(clicked);
  check('click by ref reports the element it hit', !clicked.isError && /Clicked button "Sign in"/.test(clickedText), clickedText.split('\n')[0]);
  // The page's own onclick rewrote document.title. That is the proof the click landed.
  check('the page reacted to the click (title changed to CLICKED)', /title: "CLICKED"/.test(clickedText), (clickedText.match(/title: "[^"]*"/) || [])[0]);
  check('no false mismatch warning for a matching description', !/WARNING/.test(clickedText));

  // ---- 5. description cross-check ---------------------------------------------------
  const mismatch = textOf(await HANDLERS.browser_click({ element: 'Delete account permanently', ref: btnRef }));
  check('a description unrelated to the target raises a warning', /WARNING: description/.test(mismatch), (mismatch.match(/WARNING:.*/) || [])[0]);

  // ---- 6. typing --------------------------------------------------------------------
  const snap2 = textOf(await HANDLERS.browser_snapshot({}));
  const boxRef = refFor(snap2, 'Search terms');
  const typed = await HANDLERS.browser_type({ element: 'Search terms field', ref: boxRef, text: 'hello world' });
  check('browser_type reports success', !typed.isError && /Typed 11 character/.test(textOf(typed)), textOf(typed).split('\n')[0]);
  const snap3 = textOf(await HANDLERS.browser_snapshot({}));
  const fieldLine = (snap3.match(/.*textbox.*Search terms.*/) || [])[0] || '';
  check(
    'editable field value is present only as a redacted length',
    /value=<redacted:11 chars>/.test(fieldLine) && !fieldLine.includes('hello world'),
    fieldLine
  );

  // ---- 7. checkbox state -------------------------------------------------------------
  const chkRef = refFor(snap3, 'Remember me');
  await HANDLERS.browser_click({ element: 'Remember me checkbox', ref: chkRef });
  const snap4 = textOf(await HANDLERS.browser_snapshot({}));
  check('checkbox state flips to checked in the snapshot', /checkbox "Remember me".*checked=true/.test(snap4), (snap4.match(/.*Remember me.*/) || [])[0]);

  // ---- 8. screenshot -----------------------------------------------------------------
  const shot = await HANDLERS.browser_screenshot({ maxDimension: 400 });
  const img = shot.content.find((c) => c.type === 'image');
  check('screenshot returns image content', !!img && img.mimeType === 'image/png');
  let pngOk = false;
  let dims = '';
  if (img) {
    const buf = Buffer.from(img.data, 'base64');
    const sig = buf.subarray(0, 8).toString('hex');
    const w = buf.readUInt32BE(16);
    const h = buf.readUInt32BE(20);
    dims = `${w}x${h}, ${buf.length} bytes`;
    pngOk = sig === '89504e470d0a1a0a' && buf.subarray(12, 16).toString() === 'IHDR' && w > 0 && h > 0;
  }
  check('screenshot is a structurally valid PNG', pngOk, dims);
  check('screenshot honoured maxDimension', !!img && Math.max(Buffer.from(img.data, 'base64').readUInt32BE(16), Buffer.from(img.data, 'base64').readUInt32BE(20)) === 400, dims);

  // ---- 9. scrolling ------------------------------------------------------------------
  const scrolled = textOf(await HANDLERS.browser_scroll({ direction: 'down', amount: 600 }));
  check('browser_scroll succeeds', /Scrolled down 600px/.test(scrolled));

  // ---- 10. stale refs ----------------------------------------------------------------
  const staleRef = refFor(snap4, 'Sign in');
  await HANDLERS.browser_navigate({ url: URL2 });
  const stale = await HANDLERS.browser_click({ element: 'Sign in button', ref: staleRef });
  check('a ref from the previous page is refused, not silently mis-clicked',
    stale.isError === true && /stale|Unknown ref/.test(textOf(stale)),
    textOf(stale));

  // ---- 11. history -------------------------------------------------------------------
  const back = textOf(await HANDLERS.browser_navigate_back({}));
  check('browser_navigate_back returns to the first page', /CLICKED|Start/.test(back), (back.match(/title: "[^"]*"/) || [])[0]);

  // ---- 12. resize ---------------------------------------------------------------------
  // `resize` invalidates the OSR view and the engine promotes a geometry change to a full
  // frame. Pin both the page's CSS viewport and the screenshot canvas to the new size.
  const resized = textOf(await HANDLERS.browser_resize({ width: 900, height: 700 }));
  check('browser_resize takes effect in the page (viewport reported from the page itself)',
    /Viewport is now 900x700/.test(resized), resized.split('\n')[0]);

  const shot2 = await HANDLERS.browser_screenshot({ maxDimension: 0 });
  const buf2 = Buffer.from(shot2.content.find((c) => c.type === 'image').data, 'base64');
  const shotW = buf2.readUInt32BE(16);
  const shotH = buf2.readUInt32BE(20);
  const shot2Text = textOf(shot2);
  const frameMatches = shotW === 900 && shotH === 700;
  check('screenshot catches up to the resized viewport', frameMatches, `${shotW}x${shotH}`);
  check('current resize produces no stale-frame warning',
    frameMatches && !shot2Text.includes('predates the last resize'),
    frameMatches ? 'current frame' : shot2Text);

  // ---- 13. shutdown ------------------------------------------------------------------
  const closed = textOf(await HANDLERS.browser_close({}));
  check('browser_close shuts the engine down', /Browser closed/.test(closed));

  const failed = results.filter((r) => !r.pass);
  console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
  process.exit(failed.length ? 1 : 0);
})().catch(async (e) => {
  console.error('harness error:', e && e.stack);
  try { await HANDLERS.browser_close({}); } catch { /* already gone */ }
  process.exit(2);
});
