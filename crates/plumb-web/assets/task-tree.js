// Task list folding.
//
// A task query returns a flat, source-ordered record list whose parent and
// subtask records stay contiguous: document groups are ordered by path, and
// within one document every parent is followed by its complete subtree. The
// Web app renders that list as document headings plus indented task rows, so
// folding is a pure projection over the records rather than a re-query.
//
// A collapsed document hides every task in that file. A collapsed task hides
// its whole descendant subtree and reports how many rows it hides, so the
// collapsed row can still show what it stands for.

function keySet(value) {
  return value instanceof Set ? value : new Set();
}

// Count direct subtasks per parent among the records the current query
// returned. Parents filtered out of the result set are not parents here: the
// records the server returned are the only rows the list can fold.
export function taskChildCounts(tasks) {
  const rendered = new Set(tasks.map((task) => task.key));
  const counts = new Map();
  for (const task of tasks) {
    if (!task.parentKey || !rendered.has(task.parentKey)) continue;
    counts.set(task.parentKey, (counts.get(task.parentKey) || 0) + 1);
  }
  return counts;
}

// Flatten task records into rendered list items:
//
//   { kind: 'document', path, total, hidden, collapsed }
//   { kind: 'task', task, childCount, collapsed, hiddenCount }
//
// Items stay in source order, including the document headings, so the caller
// can append them directly.
export function taskListItems(tasks, collapsed = {}) {
  const collapsedDocuments = keySet(collapsed.documents);
  const collapsedTasks = keySet(collapsed.tasks);
  const childCounts = taskChildCounts(tasks);
  const items = [];
  let group = null;
  let documentHidden = false;
  let hiddenDepth = null;
  let hiddenRow = null;
  let previousPath = null;

  for (const task of tasks) {
    if (task.path !== previousPath) {
      previousPath = task.path;
      documentHidden = collapsedDocuments.has(task.path);
      group = { kind: 'document', path: task.path, total: 0, hidden: 0, collapsed: documentHidden };
      items.push(group);
      hiddenDepth = null;
      hiddenRow = null;
    }
    group.total += 1;
    if (documentHidden) {
      group.hidden += 1;
      continue;
    }
    if (hiddenDepth !== null && task.depth > hiddenDepth) {
      group.hidden += 1;
      hiddenRow.hiddenCount += 1;
      continue;
    }
    hiddenDepth = null;
    hiddenRow = null;
    const childCount = childCounts.get(task.key) || 0;
    const row = {
      kind: 'task',
      task,
      childCount,
      collapsed: childCount > 0 && collapsedTasks.has(task.key),
      hiddenCount: 0,
    };
    if (row.collapsed) {
      hiddenDepth = task.depth;
      hiddenRow = row;
    }
    items.push(row);
  }
  return items;
}

// Ancestor keys of a task, nearest parent first, resolved against the rendered
// record list.
export function taskAncestorKeys(tasks, task) {
  const byKey = new Map(tasks.map((candidate) => [candidate.key, candidate]));
  const ancestors = [];
  let current = task;
  while (current && current.parentKey && byKey.has(current.parentKey)) {
    ancestors.push(current.parentKey);
    current = byKey.get(current.parentKey);
  }
  return ancestors;
}

// Drop the fold state that hides a task, so selection can always reveal the
// row it selected. Returns true when anything actually changed.
export function revealTask(collapsed, tasks, task) {
  if (!task) return false;
  const collapsedDocuments = keySet(collapsed.documents);
  const collapsedTasks = keySet(collapsed.tasks);
  let changed = collapsedDocuments.delete(task.path);
  for (const key of taskAncestorKeys(tasks, task)) {
    if (collapsedTasks.delete(key)) changed = true;
  }
  return changed;
}
