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

const focusedTasks = tasks.map((task) => ({ ...task, focused: task.key === 'c' }));
const visibleKeys = (items) => items.filter((item) => item.kind === 'task').map((item) => item.task.key);

test('focus retains its ancestor path and groups entire unfocused sibling trees and files', () => {
  const items = taskListItems(focusedTasks);
  assert.deepEqual(visibleKeys(items), ['a', 'b', 'c']);
  const groups = items.filter((item) => item.kind === 'group');
  assert.deepEqual(groups.map((item) => [item.total, item.documents, item.collapsed]), [[1, 0, true], [2, 1, true]]);
  assert.equal(items[0].focusedCount, 1);
  assert.equal(items[0].hidden, 1);
  assert.equal(items.find((item) => item.task?.key === 'a').focusedCount, 1);
  assert.equal(focusedTasks[0].focused, false, 'ancestor aggregation does not focus the ancestor');
});

test('one unfocused group includes multiple siblings and all their descendants', () => {
  const input = [...focusedTasks.slice(0, 4), task('notes/one.plumb', 'x', 1, 'a'), task('notes/one.plumb', 'y', 2, 'x')];
  const group = taskListItems(input).find((item) => item.kind === 'group');
  assert.equal(group.total, 3);
  const folds = { expandedGroups: new Set([group.key]) };
  assert.deepEqual(visibleKeys(taskListItems(input, folds)), ['a', 'b', 'c', 'd', 'x', 'y']);
  // Re-query with fresh objects keeps the manual choice and source order.
  assert.deepEqual(visibleKeys(taskListItems(structuredClone(input), folds)), ['a', 'b', 'c', 'd', 'x', 'y']);
});

test('manually expanded unfocused files do not create recursive automatic groups', () => {
  const items = taskListItems(focusedTasks);
  const files = items.find((item) => item.kind === 'group' && item.documents);
  const folds = { expandedGroups: new Set([files.key]) };
  const expanded = taskListItems(focusedTasks, folds);
  assert.deepEqual(visibleKeys(expanded), ['a', 'b', 'c', 'e', 'f']);
  assert.equal(expanded.filter((item) => item.kind === 'group').length, 2);
});

test('file focus counts survive manual collapse and omit filtered-out tasks', () => {
  const folds = { documents: new Set(['notes/one.plumb']) };
  assert.equal(taskListItems(focusedTasks, folds)[0].focusedCount, 1);
  const filtered = focusedTasks.filter((task) => task.key !== 'c');
  assert.equal(taskListItems(filtered)[0].focusedCount, 0);
  assert.equal(taskListItems(filtered).some((item) => item.kind === 'group'), false);
});

test('selection reveals an unfocused task through its group and manual ancestor folds', () => {
  const folds = { documents: new Set(['notes/one.plumb']), tasks: new Set(['a']) };
  assert.equal(revealTask(folds, focusedTasks, focusedTasks[3]), true);
  assert.deepEqual(visibleKeys(taskListItems(focusedTasks, folds)), ['a', 'b', 'c', 'd']);
  assert.equal(revealTask(folds, focusedTasks, focusedTasks[3]), false);
  revealTask(folds, focusedTasks, focusedTasks[5]);
  assert.deepEqual(visibleKeys(taskListItems(focusedTasks, folds)), ['a', 'b', 'c', 'd', 'e', 'f']);
});

test('focus transitions reveal the new branch, retain manual folds on refresh, and restore the plain tree', () => {
  const folds = { tasks: new Set(['b']) };
  assert.deepEqual(visibleKeys(taskListItems(focusedTasks, folds)), ['a', 'b']);
  revealTask(folds, focusedTasks, focusedTasks[2]);
  assert.deepEqual(visibleKeys(taskListItems(focusedTasks, folds)), ['a', 'b', 'c']);
  folds.tasks.add('b');
  assert.deepEqual(visibleKeys(taskListItems(structuredClone(focusedTasks), folds)), ['a', 'b']);
  folds.tasks.clear();
  assert.deepEqual(visibleKeys(taskListItems(tasks, folds)), ['a', 'b', 'c', 'd', 'e', 'f']);
});

test('appending an unfocused page keeps one outer group and its manual expansion', () => {
  const folds = {};
  const first = taskListItems(focusedTasks, folds);
  const files = first.find((item) => item.kind === 'group' && item.documents);
  folds.expandedGroups = new Set([files.key]);
  const appended = [...focusedTasks, task('notes/three.plumb', 'g', 0)];
  const items = taskListItems(appended, folds);
  assert.equal(items.filter((item) => item.kind === 'group' && item.documents).length, 1);
  assert.equal(items.find((item) => item.kind === 'group' && item.documents).documents, 2);
  assert.deepEqual(visibleKeys(items), ['a', 'b', 'c', 'e', 'f', 'g']);
});

test('display depth comes from tree edges and groups add one parent level', () => {
  const initial = taskListItems(focusedTasks);
  const parent = initial.find((item) => item.task?.key === 'a');
  const branch = initial.find((item) => item.task?.key === 'b');
  const group = initial.find((item) => item.kind === 'group' && !item.documents);
  assert.equal(parent.depth, 1);
  assert.equal(branch.depth, 2);
  assert.equal(group.depth, branch.depth);
  const folds = { expandedGroups: new Set([group.key]) };
  const expanded = taskListItems(focusedTasks, folds);
  assert.equal(expanded.find((item) => item.task?.key === 'd').depth, group.depth + 1);
  const filtered = taskListItems([focusedTasks[2]]);
  assert.equal(filtered.find((item) => item.kind === 'task').depth, 1);
  assert.equal(focusedTasks[2].depth, 2, 'source depth is unchanged');
});

test('document task replaces the virtual file row and retains foldable children', () => {
  const records = [
    { ...task('project.plumb', 'document', 0), locator: { kind: 'document' } },
    task('project.plumb', 'child', 1, 'document'),
    task('other.plumb', 'other', 0),
  ];
  const rows = taskListItems(records);
  assert.deepEqual(rows.map((row) => [row.kind, row.task?.key || row.path, row.depth]), [
    ['task', 'document', 0], ['task', 'child', 1],
    ['document', 'other.plumb', 0], ['task', 'other', 1],
  ]);
  const collapsed = { tasks: new Set(['document']) };
  assert.equal(taskListItems(records, collapsed)[0].hiddenCount, 1);
  assert.equal(revealTask(collapsed, records, records[1]), true);
  assert.equal(collapsed.tasks.size, 0);
});

test('focused document task groups unfocused children at their sibling depth', () => {
  const records = [
    { ...task('project.plumb', 'document', 0), locator: { kind: 'document' }, focused: true },
    task('project.plumb', 'child', 1, 'document'),
  ];
  const rows = taskListItems(records);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].task.key, 'document');
  assert.equal(rows[0].depth, 0);
  assert.equal(rows[1].kind, 'group');
  assert.equal(rows[1].depth, 1);
  const collapsed = {};
  revealTask(collapsed, records, records[1]);
  assert.equal(taskListItems(records, collapsed).at(-1).task.key, 'child');
});
