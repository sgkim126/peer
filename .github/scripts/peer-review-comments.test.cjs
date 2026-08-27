'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const {
  publishWithoutRenderedRange,
  removeRanges,
  renderedItemRanges,
  reviewCommentPosition,
  reviewCommentSide,
  standaloneQuestionBody,
} = require('./peer-review-comments.cjs');

test('selects the right side for added lines', () => {
  const diff = '@@ -10,0 +11,2 @@\n+first\n+second\n';
  assert.equal(reviewCommentSide(diff, 11), 'RIGHT');
  assert.equal(reviewCommentSide(diff, 12), 'RIGHT');
});

test('selects the left side for deleted lines', () => {
  const diff = '@@ -4,2 +3,0 @@\n-first\n-second\n';
  assert.equal(reviewCommentSide(diff, 4), 'LEFT');
  assert.equal(reviewCommentSide(diff, 5), 'LEFT');
});

test('prefers the right side when a line is present on both sides', () => {
  const diff = '@@ -5 +5 @@\n-old\n+new\n';
  assert.equal(reviewCommentSide(diff, 5), 'RIGHT');
});

test('checks every hunk and handles omitted counts', () => {
  const diff = '@@ -1 +1 @@\n-old\n+new\n@@ -20 +22 @@\n-later\n+replacement\n';
  assert.equal(reviewCommentSide(diff, 22), 'RIGHT');
  assert.equal(reviewCommentSide(diff, 20), 'LEFT');
});

test('rejects lines outside changed hunks', () => {
  const diff = '@@ -5,2 +5,2 @@\n-old\n-lines\n+new\n+lines\n';
  assert.equal(reviewCommentSide(diff, 4), undefined);
  assert.equal(reviewCommentSide(diff, 7), undefined);
  assert.equal(reviewCommentSide('', 5), undefined);
});

test('uses a file position only when the line is absent', () => {
  const diff = '@@ -5 +5 @@\n-old\n+new\n';
  assert.deepEqual(reviewCommentPosition(diff, undefined), { subject_type: 'file' });
  assert.deepEqual(reviewCommentPosition(diff, null), { subject_type: 'file' });
  assert.equal(reviewCommentPosition(diff, 0), undefined);
  assert.equal(reviewCommentPosition(diff, -1), undefined);
  assert.equal(reviewCommentPosition(diff, 1.5), undefined);
  assert.equal(reviewCommentPosition(diff, '5'), undefined);
});

test('returns a line position only for a changed line', () => {
  const diff = '@@ -5 +5 @@\n-old\n+new\n';
  assert.deepEqual(reviewCommentPosition(diff, 5), { line: 5, side: 'RIGHT' });
  assert.equal(reviewCommentPosition(diff, 6), undefined);
  assert.equal(reviewCommentPosition('', undefined), undefined);
});

test('appends collapsed provenance to a standalone question', () => {
  assert.equal(
    standaloneQuestionBody('Why is this required?', 'peer version: 0.13.1\nprovider: openai'),
    [
      'Why is this required?',
      '',
      '<details>',
      '<summary>Review provenance</summary>',
      '',
      'peer version: 0.13.1',
      'provider: openai',
      '</details>',
    ].join('\n'),
  );
});

test('removes a published item by identity when rendered text is duplicated', () => {
  const rendered = '- **question/rationale** — Why?';
  const output = [
    '## Review questions',
    rendered,
    rendered,
  ].join('\n');
  const ranges = renderedItemRanges(output, [rendered, rendered]);

  assert.equal(
    removeRanges(output, [ranges[1]]),
    ['## Review questions', rendered, ''].join('\n'),
  );
});

test('publishes a question without a matching aggregate range', () => {
  const rendered = '- **question/rationale** — Why?';
  const [range] = renderedItemRanges(
    '## Review questions\n- **question/rationale** — Different text',
    [rendered],
  );

  assert.equal(range, undefined);
  assert.equal(publishWithoutRenderedRange('question'), true);
  assert.equal(publishWithoutRenderedRange('finding'), false);
});
