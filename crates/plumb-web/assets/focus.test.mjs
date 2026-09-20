import assert from 'node:assert/strict';
import test from 'node:test';

import {
  candidateHeading,
  focusAge,
  focusBadge,
  focusHistory,
  focusInstantLabel,
  focusIntervals,
  formatFocusSpan,
  inFlightHeading,
  isFocused,
  nextSections,
  nextSummary,
} from './focus.js';

const NOW = Date.parse('2026-09-20T12:00:00+08:00');
const at = (value) => new Date(value).toISOString();

test('formats focus spans without implying worked time', () => {
  assert.equal(formatFocusSpan(30 * 1000), 'just now');
  assert.equal(formatFocusSpan(45 * 60 * 1000), '45m');
  assert.equal(formatFocusSpan(2 * 60 * 60 * 1000), '2h');
  assert.equal(formatFocusSpan((2 * 60 + 15) * 60 * 1000), '2h 15m');
  assert.equal(formatFocusSpan(25 * 60 * 60 * 1000), '1d 1h');
  assert.equal(formatFocusSpan(48 * 60 * 60 * 1000), '2d');
  // A negative span is never rendered as a duration.
  assert.equal(formatFocusSpan(-1000), null);
});

test('reports age for a past start and no duration for a future start', () => {
  assert.deepEqual(focusAge('2026-09-20T10:00:00+08:00', NOW), {
    future: false,
    milliseconds: 2 * 60 * 60 * 1000,
    label: '2h',
  });
  assert.deepEqual(focusAge('2026-09-20T13:00:00+08:00', NOW), {
    future: true,
    milliseconds: 60 * 60 * 1000,
    label: null,
  });
  assert.equal(focusAge('not-a-timestamp', NOW), null);
  assert.equal(focusAge(undefined, NOW), null);
});

test('reads only currently focused tasks', () => {
  assert.equal(isFocused({ focused: true }), true);
  assert.equal(isFocused({ focused: false }), false);
  assert.equal(isFocused({}), false);
  assert.equal(isFocused(undefined), false);
  assert.deepEqual(focusIntervals({}), []);
  assert.deepEqual(focusIntervals({ focusIntervals: [{ start: 'a', end: null }] }), [
    { start: 'a', end: null },
  ]);
});

test('builds detail history rows for closed and open intervals', () => {
  const task = {
    focused: true,
    focusedSince: '2026-09-20T10:00:00+08:00',
    focusIntervals: [
      { start: '2026-09-19T09:00:00+08:00', end: '2026-09-19T11:00:00+08:00' },
      { start: '2026-09-20T10:00:00+08:00', end: null },
    ],
  };
  assert.deepEqual(focusHistory(task, NOW), [
    { start: '2026-09-19T09:00:00+08:00', end: '2026-09-19T11:00:00+08:00', open: false, future: false, label: '2h' },
    { start: '2026-09-20T10:00:00+08:00', end: null, open: true, future: false, label: '2h' },
  ]);
  assert.deepEqual(focusHistory({}, NOW), []);
});

test('flags a future start instead of rendering a negative span', () => {
  const rows = focusHistory(
    {
      focusIntervals: [
        { start: '2026-09-21T09:00:00+08:00', end: null },
        { start: '2026-09-20T11:00:00+08:00', end: '2026-09-20T11:30:00+08:00' },
      ],
    },
    NOW,
  );
  assert.equal(rows[0].future, true);
  assert.equal(rows[0].label, null);
  assert.equal(rows[1].future, false);
  assert.equal(rows[1].label, '30m');
});

test('badges only currently focused tasks with an age, never a work duration', () => {
  assert.equal(focusBadge({ focused: false }, NOW), null);
  assert.equal(focusBadge({ focused: true, focusedSince: '2026-09-20T10:00:00+08:00' }, NOW), 'focused 2h');
  assert.equal(focusBadge({ focused: true, focusedSince: '2026-09-20T13:00:00+08:00' }, NOW), 'focused (future start)');
  assert.equal(focusBadge({ focused: true }, NOW), 'focused');
});

test('splits the shared next result into its two sections', () => {
  const sections = nextSections({
    focused: [{ key: 'a' }, { key: 'b' }, { key: 'c' }],
    focusedTotal: 7,
    focusedComplete: false,
    candidates: [{ key: 'd' }],
    candidateLimit: 3,
    candidatesComplete: false,
    skippedInvalid: [{ id: 'bad' }],
    complete: false,
  });
  assert.equal(sections.inFlight.length, 3);
  assert.equal(sections.inFlightTotal, 7);
  assert.equal(sections.inFlightTruncated, true);
  assert.equal(sections.candidates.length, 1);
  assert.equal(sections.candidateLimit, 3);
  assert.equal(sections.candidatesComplete, false);
  assert.equal(sections.skippedInvalid.length, 1);
  assert.equal(sections.complete, false);
});

test('tolerates a missing or partial next result', () => {
  assert.deepEqual(nextSections(undefined), {
    inFlight: [],
    candidates: [],
    inFlightTotal: 0,
    inFlightTruncated: false,
    candidateLimit: 0,
    candidatesComplete: true,
    skippedInvalid: [],
    complete: true,
  });
  assert.deepEqual(nextSections({ focused: [{ key: 'a' }] }), {
    inFlight: [{ key: 'a' }],
    candidates: [],
    inFlightTotal: 1,
    inFlightTruncated: false,
    candidateLimit: 0,
    candidatesComplete: true,
    skippedInvalid: [],
    complete: true,
  });
});

test('states the in-flight total and the requested candidate limit', () => {
  const full = nextSections({ focused: [{ key: 'a' }], focusedTotal: 1, candidateLimit: 3 });
  assert.equal(inFlightHeading(full), 'In flight (1)');
  assert.equal(candidateHeading(full), 'Ready to start (limit 3)');
  const paged = nextSections({
    focused: [{ key: 'a' }, { key: 'b' }],
    focusedTotal: 9,
    focusedComplete: false,
    candidateLimit: 5,
  });
  assert.equal(inFlightHeading(paged), 'In flight (2 of 9)');
  assert.equal(candidateHeading(paged), 'Ready to start (limit 5)');
  // A truncated section is named even when the flag is the only signal.
  const flagged = nextSections({ focused: [{ key: 'a' }], focusedComplete: false });
  assert.equal(inFlightHeading(flagged), 'In flight (1 of 1)');
});

test('summarizes both sections and every completeness signal', () => {
  assert.equal(
    nextSummary(nextSections({ focused: [{ key: 'a' }], focusedTotal: 4, candidates: [{ key: 'b' }], candidateLimit: 3, candidatesComplete: false })),
    '4 in flight · 1 ready to start · limit applied',
  );
  assert.equal(
    nextSummary(nextSections({
      focused: [],
      focusedTotal: 0,
      candidates: [],
      candidateLimit: 3,
      candidatesComplete: true,
      complete: false,
      skippedInvalid: [{ id: 'bad' }, { id: 'worse' }],
    })),
    '0 in flight · 0 ready to start · index incomplete · 2 skipped (invalid focus)',
  );
});

test('labels instants without dropping the recorded offset', () => {
  assert.equal(focusInstantLabel('2026-09-20T09:00:00+08:00'), '2026-09-20 09:00 +08:00');
  assert.equal(focusInstantLabel('2026-09-20T09:00:00Z'), '2026-09-20 09:00 Z');
  assert.equal(focusInstantLabel('2026-09-20T09:00'), '2026-09-20 09:00');
  assert.equal(focusInstantLabel(''), '');
  assert.equal(focusInstantLabel(undefined), '');
  assert.equal(focusInstantLabel('not-a-timestamp'), '');
});

test('keeps instants in the order the document recorded them', () => {
  const rows = focusHistory(
    {
      focusIntervals: [
        { start: at('2026-09-01T00:00:00Z'), end: at('2026-09-01T01:00:00Z') },
        { start: at('2026-09-02T00:00:00Z'), end: at('2026-09-02T02:30:00Z') },
      ],
    },
    NOW,
  );
  assert.equal(rows[0].label, '1h');
  assert.equal(rows[1].label, '2h 30m');
});
