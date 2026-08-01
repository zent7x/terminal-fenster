// Accessibility snapshot with stable element references.
//
// The design idea -- give the model a compact text tree of the page in which every
// actionable element carries an opaque handle it can pass back to a click/type tool -- is
// the one Playwright MCP popularised. That project is Apache-2.0 (Copyright (c) Microsoft
// Corporation); no code from it is used here. This is an independent implementation and it
// differs in three substantive ways:
//
//   1. Source of truth. We read Chromium's *computed* accessibility tree via CDP
//      (Accessibility.getFullAXTree) rather than walking the DOM and re-deriving roles and
//      accessible names in injected JavaScript. Name computation (aria-labelledby, label
//      association, alt/title fallback ordering) is a specification unto itself and
//      Chromium already implements it correctly.
//   2. Lazy geometry. A snapshot resolves no coordinates. A ref stores a backendNodeId;
//      the box model is fetched only for the one element an action actually targets. A
//      200-element page costs one round trip to snapshot, not 201.
//   3. Epoch-guarded refs. Every ref records the navigation epoch it was minted in. Acting
//      on a ref from a previous page is refused rather than silently mis-clicking -- the
//      failure mode called out as c-F2 in artifacts/swarm/A03-user-journeys.md.
'use strict';

// Roles a model can meaningfully act on. Anything here gets a ref; everything else is
// structure or prose and is rendered without one, which keeps the ref namespace small
// enough for a model to hold in working memory.
const ACTIONABLE_ROLES = new Set([
  'button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio', 'combobox', 'listbox',
  'option', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'tab', 'switch', 'slider',
  'spinbutton', 'textarea', 'searchBox', 'colorwell', 'date', 'datetime', 'inputTime',
  'menuButton', 'popUpButton', 'toggleButton', 'disclosureTriangle', 'scrollbar',
]);

// Structural roles worth showing for orientation even though they are not actionable.
const SKIP_ROLES = new Set([
  'none', 'presentation', 'generic', 'InlineTextBox', 'LineBreak', 'Iframe',
  'GenericContainer', 'Pre', 'LabelText', 'Legend', 'Abbr', 'Emphasis', 'Strong',
]);

const STATE_PROPS = ['checked', 'disabled', 'expanded', 'selected', 'pressed', 'required', 'invalid', 'level', 'focused', 'multiselectable', 'readonly'];
const EDITABLE_ROLES = new Set(['textbox', 'searchbox', 'textarea', 'searchBox']);

function propValue(node, name) {
  if (!node.properties) return undefined;
  const p = node.properties.find((x) => x.name === name);
  if (!p || !p.value) return undefined;
  return p.value.value;
}

function roleOf(node) {
  return (node.role && node.role.value) || '';
}

function nameOf(node) {
  return ((node.name && node.name.value) || '').replace(/\s+/g, ' ').trim();
}

function isActionable(node) {
  const role = roleOf(node);
  if (ACTIONABLE_ROLES.has(role)) return true;
  // A focusable element with a name is actionable even if its role is unusual (custom
  // widgets with role="none" but tabindex, for instance).
  if (propValue(node, 'focusable') === true && nameOf(node) && !SKIP_ROLES.has(role)) return true;
  return false;
}

function valueState(node) {
  const value = node.value && node.value.value;
  if (value === undefined || value === '' || value === null) return null;
  const text = String(value);
  const editable = EDITABLE_ROLES.has(roleOf(node)) || propValue(node, 'editable') === 'plaintext' ||
    propValue(node, 'editable') === true;
  // Chromium's AX tree does not reliably identify password inputs. Redact every editable value
  // so a password cannot be handed to the model merely because it asked for a snapshot.
  return editable ? `value=<redacted:${Array.from(text).length} chars>` : `value=${quote(text)}`;
}

function quote(s) {
  return '"' + s.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
}

/// Capture a snapshot. Returns { text, refs, count, truncated }.
async function capture(session, opts = {}) {
  const maxLines = opts.maxLines || 1200;
  const { nodes } = await session.cdpSend('Accessibility.getFullAXTree', { depth: -1 });
  const byId = new Map();
  for (const n of nodes) byId.set(n.nodeId, n);

  const refs = new Map();
  let refCounter = 0;
  const lines = [];
  let truncated = false;

  const emit = (depth, text) => {
    if (lines.length >= maxLines) { truncated = true; return false; }
    lines.push('  '.repeat(depth) + '- ' + text);
    return true;
  };

  const walk = (node, depth) => {
    if (!node || truncated) return;
    const role = roleOf(node);
    const name = nameOf(node);

    // Ignored nodes contribute nothing themselves but their subtree still can (a
    // presentational wrapper around real content is extremely common).
    const hidden = node.ignored === true || SKIP_ROLES.has(role);
    let childDepth = depth;

    if (!hidden) {
      if (role === 'StaticText' || role === 'text') {
        if (name) emit(depth, 'text: ' + quote(name));
        return; // StaticText children are InlineTextBoxes: pure noise.
      }
      const parts = [role || 'node'];
      if (name) parts.push(quote(name));

      let ref = null;
      if (isActionable(node) && node.backendDOMNodeId !== undefined) {
        ref = 'e' + ++refCounter;
        refs.set(ref, {
          backendNodeId: node.backendDOMNodeId,
          role,
          name,
          epoch: session.navEpoch,
        });
      }

      const states = [];
      for (const p of STATE_PROPS) {
        const v = propValue(node, p);
        if (v === undefined || v === false || v === 'false') continue;
        states.push(v === true ? p : `${p}=${v}`);
      }
      const value = valueState(node);
      if (value) states.push(value);
      if (ref) states.push(`ref=${ref}`);

      let line = parts.join(' ');
      if (states.length) line += ' [' + states.join('] [') + ']';
      if (!emit(depth, line)) return;
      childDepth = depth + 1;
    }

    for (const id of node.childIds || []) walk(byId.get(id), childDepth);
  };

  const root = nodes.find((n) => !n.parentId) || nodes[0];
  walk(root, 0);

  return {
    text: lines.join('\n'),
    refs,
    count: refCounter,
    truncated,
  };
}

/// Turn a ref into a viewport coordinate, or explain precisely why it cannot.
async function resolveRef(session, refs, ref, opts = {}) {
  const entry = refs.get(ref);
  if (!entry) {
    throw new Error(
      `Unknown ref ${ref}. Refs come from browser_snapshot and are only valid for the ` +
        'snapshot that produced them. Call browser_snapshot and use a ref from its output.'
    );
  }
  if (entry.epoch !== session.navEpoch) {
    throw new Error(
      `Ref ${ref} is stale: the page navigated after that snapshot was taken ` +
        `(snapshot epoch ${entry.epoch}, current ${session.navEpoch}). Call browser_snapshot again.`
    );
  }

  if (opts.scrollIntoView !== false) {
    try {
      await session.cdpSend('DOM.scrollIntoViewIfNeeded', { backendNodeId: entry.backendNodeId });
    } catch {
      // Non-fatal: some nodes (detached, display:none) cannot be scrolled to, and the box
      // model call below will produce the real diagnostic.
    }
  }

  let model;
  try {
    ({ model } = await session.cdpSend('DOM.getBoxModel', { backendNodeId: entry.backendNodeId }));
  } catch (e) {
    throw new Error(
      `Ref ${ref} (${entry.role} ${JSON.stringify(entry.name)}) no longer has a layout box ` +
        `-- it was probably removed or hidden by a page update. Re-snapshot. (${e.message})`
    );
  }
  if (!model || !model.content || model.content.length < 8) {
    throw new Error(`Ref ${ref} has no content box; it may be invisible.`);
  }
  // content quad is [x1,y1, x2,y2, x3,y3, x4,y4] in CSS pixels of the layout viewport.
  const q = model.content;
  const x = (q[0] + q[2] + q[4] + q[6]) / 4;
  const y = (q[1] + q[3] + q[5] + q[7]) / 4;
  return { x: Math.round(x), y: Math.round(y), entry, width: model.width, height: model.height };
}

/// Cross-check the model's own words against what the ref actually points at.
///
/// Playwright MCP asks for a human-readable `element` alongside the ref so a UI can show
/// the user what is about to be clicked. We keep that and give it a second job: if the
/// description shares no vocabulary with the element's accessible name, the model has
/// probably lost track of which ref is which (or is acting on injected page text), and the
/// caller is told so rather than finding out from a wrong click.
function describeMismatch(entry, description) {
  if (!description || !entry.name) return null;
  const norm = (s) => s.toLowerCase().replace(/[^a-z0-9 ]+/g, ' ').split(/\s+/).filter((w) => w.length > 2);
  const a = new Set(norm(entry.name));
  const b = new Set(norm(description));
  if (!a.size || !b.size) return null;
  for (const w of b) if (a.has(w)) return null;
  return `description ${JSON.stringify(description)} shares no words with the element's accessible name ${JSON.stringify(entry.name)} (role ${entry.role})`;
}

module.exports = { capture, resolveRef, describeMismatch, valueState, ACTIONABLE_ROLES };
