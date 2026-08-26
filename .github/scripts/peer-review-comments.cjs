'use strict';

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm;

const inRange = (line, start, count) => (
  count > 0 && line >= start && line < start + count
);

const reviewCommentSide = (diff, line) => {
  if (typeof diff !== 'string' || !Number.isSafeInteger(line) || line < 1) {
    return undefined;
  }

  let match;
  let matchesLeft = false;
  HUNK_HEADER.lastIndex = 0;
  while ((match = HUNK_HEADER.exec(diff)) !== null) {
    const oldStart = Number(match[1]);
    const oldCount = match[2] === undefined ? 1 : Number(match[2]);
    const newStart = Number(match[3]);
    const newCount = match[4] === undefined ? 1 : Number(match[4]);
    if (inRange(line, newStart, newCount)) return 'RIGHT';
    if (inRange(line, oldStart, oldCount)) matchesLeft = true;
  }
  return matchesLeft ? 'LEFT' : undefined;
};

const reviewCommentPosition = (diff, line) => {
  if (typeof diff !== 'string' || !diff.trim()) return undefined;
  if (line == null) return { subject_type: 'file' };
  if (!Number.isSafeInteger(line) || line < 1) return undefined;
  const side = reviewCommentSide(diff, line);
  return side ? { line, side } : undefined;
};

module.exports = { reviewCommentPosition, reviewCommentSide };
