import assert from 'node:assert/strict';
import test from 'node:test';
import { taskRowParents, firstRowBelow, taskStickySegments } from './task-sticky.js';

const doc = (title, depth = 0) => ({ kind: 'document', depth, segments: [{ title }] });
const context = (title, depth) => ({ kind: 'context', depth, segments: [{ title }] });
const task = (title, depth) => ({ kind: 'task', depth, task: { title } });
const labels = (items, index) => taskStickySegments(items, taskRowParents(items), index).map(s => s.title);

test('branches, document tasks and focus groups preserve real ancestry', () => {
  const items = [doc('Notebook'), context('Company', 1), task('Parent', 2), task('Child', 3),
    { kind: 'group', depth: 3 }, task('Unfocused child', 4), context('Sibling', 1), task('Other', 2),
    task('Document task', 0), task('Document child', 1)];
  assert.deepEqual(labels(items, 0), []);
  assert.deepEqual(labels(items, 3), ['Notebook', 'Company', 'Parent']);
  assert.deepEqual(labels(items, 5), ['Notebook', 'Company', 'Parent']);
  assert.deepEqual(labels(items, 6), ['Notebook']);
  assert.deepEqual(labels(items, 7), ['Notebook', 'Sibling']);
  assert.deepEqual(labels(items, 8), []);
  assert.deepEqual(labels(items, 9), ['Document task']);
  assert.deepEqual(labels(items, items.length), []);
});

test('compact chains retain separate jump targets; collapsed and empty lists clear stale ancestry', () => {
  const items = [{ ...doc('Notebook'), segments: [{ title: 'Notebook' }, { title: 'Section' }] }, task('Child', 1)];
  assert.deepEqual(taskStickySegments(items, taskRowParents(items), 1), [
    { title: 'Notebook', row: 0, segment: 0 }, { title: 'Section', row: 0, segment: 1 },
  ]);
  assert.deepEqual(labels([items[0], doc('Next')], 1), []);
  assert.deepEqual(labels([], 0), []);
});

test('scroll search uses logarithmic geometry reads with exact row boundaries', () => {
  let reads = 0;
  const length = 1000000;
  const bottom = index => { reads++; return (index + 1) * 40; };
  assert.equal(firstRowBelow(length, bottom, 20000000), 500000);
  assert.ok(reads <= 20, `${reads} geometry reads`);
  assert.equal(firstRowBelow(3, i => [30, 75, 120][i], 74), 1);
  assert.equal(firstRowBelow(3, i => [30, 75, 120][i], 75), 2);
  assert.equal(firstRowBelow(3, i => [30, 75, 120][i], 120), 3);
  assert.equal(firstRowBelow(0, () => assert.fail('empty list read'), 0), 0);
});
