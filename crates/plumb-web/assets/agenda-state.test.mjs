import assert from 'node:assert/strict';
import test from 'node:test';

import {
  adjacentAgendaPage,
  agendaPageRange,
  currentTimeInsertionIndex,
  localDateKey,
} from './agenda-state.js';

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

test('agenda pages stay bounded while centering an anchor', () => {
  assert.deepEqual(agendaPageRange(33_512, 20_000), { start: 19_880, end: 20_120 });
  assert.deepEqual(agendaPageRange(100, 99), { start: 0, end: 100 });
  assert.deepEqual(agendaPageRange(0, 0), { start: 0, end: 0 });
});

test('agenda page navigation clamps at collection boundaries', () => {
  assert.deepEqual(adjacentAgendaPage({ start: 120, end: 360 }, 'earlier', 1_000), { start: 0, end: 240 });
  assert.deepEqual(adjacentAgendaPage({ start: 720, end: 960 }, 'later', 1_000), { start: 760, end: 1_000 });
});
