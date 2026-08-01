'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { valueState } = require('../lib/snapshot');

test('editable accessibility values are represented only by their length', () => {
  const node = { role: { value: 'textbox' }, value: { value: 'correct horse' } };
  const rendered = valueState(node);
  assert.equal(rendered, 'value=<redacted:13 chars>');
  assert.doesNotMatch(rendered, /correct|horse/);
});

test('non-editable accessibility values remain useful', () => {
  const node = { role: { value: 'slider' }, value: { value: '75' } };
  assert.equal(valueState(node), 'value="75"');
});
