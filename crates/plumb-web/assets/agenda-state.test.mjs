import assert from 'node:assert/strict';
import test from 'node:test';

import { currentTimeInsertionIndex, localDateKey } from './agenda-state.js';

test('current time is inserted before the first future event', () => {
  const events = [
    { at: '2026-08-02T08:00:00Z' },
    { start: '2026-08-02T10:00:00Z' },
    { at: '2026-08-03T08:00:00Z' },
  ];
  assert.equal(currentTimeInsertionIndex(events, new Date('2026-08-02T09:00:00Z')), 1);
});

test('current time follows all past events and ignores invalid dates', () => {
  const events = [{ at: 'invalid' }, { at: '2026-08-01T08:00:00Z' }];
  assert.equal(currentTimeInsertionIndex(events, new Date('2026-08-02T09:00:00Z')), 2);
});

test('local date keys use calendar dates instead of UTC dates', () => {
  const local = new Date(2026, 7, 2, 0, 30);
  assert.equal(localDateKey(local), '2026-08-02');
  assert.equal(localDateKey('invalid'), null);
});
