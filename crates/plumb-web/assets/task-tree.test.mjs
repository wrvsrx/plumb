import assert from 'node:assert/strict';
import test from 'node:test';

import { revealTask, taskAncestorKeys, taskChildCounts, taskListItems } from './task-tree.js';

function task(path, key, depth, parentKey = null) {
  return { path, key, depth, parentKey };
}

// notes/one.plumb
//   A
//     B
//       C
//     D
// notes/two.plumb
//   E
//     F
const tasks = [
  task('notes/one.plumb', 'a', 0),
  task('notes/one.plumb', 'b', 1, 'a'),
  task('notes/one.plumb', 'c', 2, 'b'),
  task('notes/one.plumb', 'd', 1, 'a'),
  task('notes/two.plumb', 'e', 0),
  task('notes/two.plumb', 'f', 1, 'e'),
];

function shape(items) {
  return items.map((item) => (item.kind === 'document'
    ? `doc:${item.path}:${item.hidden}/${item.total}:${item.collapsed}`
    : `${item.task.key}:${item.collapsed}:${item.hiddenCount}`));
}

test('child counts only describe subtasks in the rendered record list', () => {
  assert.deepEqual(Array.from(taskChildCounts(tasks)), [['a', 2], ['b', 1], ['e', 1]]);
  // Dropping the root leaves stale parent pointers behind; they must not turn
  // an unrendered record into a foldable row.
  const partial = tasks.filter((candidate) => candidate.key !== 'a');
  assert.deepEqual(Array.from(taskChildCounts(partial)), [['b', 1], ['e', 1]]);
});

test('an unfolded list keeps every document and task in source order', () => {
  assert.deepEqual(shape(taskListItems(tasks)), [
    'doc:notes/one.plumb:0/4:false',
    'a:false:0', 'b:false:0', 'c:false:0', 'd:false:0',
    'doc:notes/two.plumb:0/2:false',
    'e:false:0', 'f:false:0',
  ]);
});

test('collapsing a document hides its tasks and reports the hidden count', () => {
  const items = taskListItems(tasks, { documents: new Set(['notes/one.plumb']) });
  assert.deepEqual(shape(items), [
    'doc:notes/one.plumb:4/4:true',
    'doc:notes/two.plumb:0/2:false',
    'e:false:0', 'f:false:0',
  ]);
});

test('collapsing a task hides its whole subtree and counts hidden descendants', () => {
  assert.deepEqual(
    shape(taskListItems(tasks, { tasks: new Set(['a']) })),
    [
      'doc:notes/one.plumb:3/4:false',
      'a:true:3',
      'doc:notes/two.plumb:0/2:false',
      'e:false:0', 'f:false:0',
    ],
  );
  assert.deepEqual(
    shape(taskListItems(tasks, { tasks: new Set(['b']) })),
    ['doc:notes/one.plumb:1/4:false', 'a:false:0', 'b:true:1', 'd:false:0', 'doc:notes/two.plumb:0/2:false', 'e:false:0', 'f:false:0'],
  );
});

test('outer collapse wins while nested collapse state is retained', () => {
  const collapsed = { tasks: new Set(['a', 'b']) };
  assert.deepEqual(shape(taskListItems(tasks, collapsed)), [
    'doc:notes/one.plumb:3/4:false',
    'a:true:3',
    'doc:notes/two.plumb:0/2:false',
    'e:false:0', 'f:false:0',
  ]);
  collapsed.tasks.delete('a');
  assert.deepEqual(shape(taskListItems(tasks, collapsed)).slice(0, 5), [
    'doc:notes/one.plumb:1/4:false', 'a:false:0', 'b:true:1', 'd:false:0', 'doc:notes/two.plumb:0/2:false',
  ]);
});

test('a collapsed task without rendered subtasks folds nothing', () => {
  const items = taskListItems(tasks, { tasks: new Set(['c', 'f']) });
  assert.deepEqual(shape(items), [
    'doc:notes/one.plumb:0/4:false', 'a:false:0', 'b:false:0', 'c:false:0', 'd:false:0',
    'doc:notes/two.plumb:0/2:false', 'e:false:0', 'f:false:0',
  ]);
});

test('a collapsed subtree never leaks into the next document', () => {
  const items = taskListItems(tasks, { tasks: new Set(['d']) });
  const two = items.filter((item) => item.kind === 'task' && item.task.path === 'notes/two.plumb');
  assert.deepEqual(two.map((item) => item.task.key), ['e', 'f']);
});

test('ancestors resolve through the rendered records and reveal clears them', () => {
  const collapsed = { documents: new Set(['notes/one.plumb']), tasks: new Set(['a', 'b']) };
  const deepest = tasks.find((candidate) => candidate.key === 'c');
  assert.deepEqual(taskAncestorKeys(tasks, deepest), ['b', 'a']);
  assert.equal(revealTask(collapsed, tasks, deepest), true);
  assert.equal(collapsed.documents.has('notes/one.plumb'), false);
  assert.deepEqual(Array.from(collapsed.tasks), []);
  assert.equal(revealTask(collapsed, tasks, deepest), false);
});

test('missing fold state means everything is expanded', () => {
  assert.deepEqual(shape(taskListItems(tasks, {})), shape(taskListItems(tasks)));
  assert.deepEqual(shape(taskListItems(tasks)), [
    'doc:notes/one.plumb:0/4:false', 'a:false:0', 'b:false:0', 'c:false:0', 'd:false:0',
    'doc:notes/two.plumb:0/2:false', 'e:false:0', 'f:false:0',
  ]);
});
