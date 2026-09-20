// Task list folding.
//
// A task query returns a flat, server-ordered record list whose parent and
// subtask records stay contiguous: focused document groups come first, and
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

// Build a presentation forest from the already filtered, ordered server page.
// Document pages retain complete trees. Missing parents remain filtered out;
// never invent task records or interpret focus history in the browser.
function taskForest(tasks) {
  const documents = new Map();
  const nodes = new Map(tasks.map((task) => [task.key, {
    task, children: [], total: 1, focusedCount: task.focused ? 1 : 0,
  }]));
  for (const task of tasks) {
    if (!documents.has(task.path)) documents.set(task.path, {
      path: task.path, children: [], total: 0, focusedCount: 0,
    });
    const node = nodes.get(task.key);
    const parent = nodes.get(task.parentKey);
    (parent && parent.task.path === task.path ? parent : documents.get(task.path)).children.push(node);
  }
  // Records are preorder: aggregate descendants before their parents without
  // recursion, including trees deeper than the JS call stack.
  for (let index = tasks.length - 1; index >= 0; index -= 1) {
    const task = tasks[index];
    const node = nodes.get(task.key);
    const candidate = nodes.get(task.parentKey);
    const parent = candidate && candidate.task.path === task.path ? candidate : documents.get(task.path);
    parent.total += node.total;
    parent.focusedCount += node.focusedCount;
  }
  return { documents: [...documents.values()], nodes };
}

function groupKey(parent) {
  return JSON.stringify(parent.task ? ['task', parent.task.key] : ['document', parent.path]);
}
const FILES_GROUP = JSON.stringify(['files']);

// Fold projection only: preserve server order within each focus partition.
// Unfocused sibling subtrees share ONE group, rather than one fold per task.
export function taskListItems(tasks, collapsed = {}) {
  const forest = taskForest(tasks);
  const grouping = forest.documents.some((document) => document.focusedCount > 0);
  const expanded = keySet(collapsed.expandedGroups);
  const items = [];
  const work = [];
  function schedule(nodes, parent = null, documentRow = null) {
    const plain = grouping ? nodes.filter((node) => node.focusedCount === 0) : [];
    const grouped = plain.length > 0;
    const visible = grouped ? nodes.filter((node) => node.focusedCount > 0) : nodes;
    if (grouped) {
      const key = parent ? groupKey(parent) : FILES_GROUP;
      work.push({ group: {
        kind: 'group', key, total: plain.reduce((sum, node) => sum + node.total, 0),
        documents: parent ? 0 : plain.length,
        depth: parent?.task ? parent.task.depth + 1 : 0,
        collapsed: !expanded.has(key),
      }, nodes: plain, documentRow });
    }
    for (let index = visible.length - 1; index >= 0; index -= 1) {
      work.push({ node: visible[index], documentRow });
    }
  }
  schedule(forest.documents);
  while (work.length) {
    const entry = work.pop();
    if (entry.group) {
      items.push(entry.group);
      if (entry.group.collapsed) {
        if (entry.documentRow) entry.documentRow.hidden += entry.group.total;
      } else {
        // Do not wrap the contents of an unfocused group in further automatic
        // groups. Manual subtree folds still apply.
        for (let index = entry.nodes.length - 1; index >= 0; index -= 1) {
          work.push({ node: entry.nodes[index], documentRow: entry.documentRow, plain: true });
        }
      }
      continue;
    }
    const { node, plain } = entry;
    let documentRow = entry.documentRow;
    let folded;
    if (!node.task) {
      folded = keySet(collapsed.documents).has(node.path);
      documentRow = {
        kind: 'document', path: node.path, total: node.total,
        hidden: folded ? node.total : 0, collapsed: folded, focusedCount: node.focusedCount,
      };
      items.push(documentRow);
    } else {
      folded = node.children.length > 0 && keySet(collapsed.tasks).has(node.task.key);
      const hiddenCount = folded ? node.total - 1 : 0;
      documentRow.hidden += hiddenCount;
      items.push({
        kind: 'task', task: node.task, childCount: node.children.length,
        collapsed: folded, hiddenCount, focusedCount: node.focusedCount,
      });
    }
    if (folded) continue;
    if (plain || !grouping) {
      for (let index = node.children.length - 1; index >= 0; index -= 1) {
        work.push({ node: node.children[index], documentRow, plain: true });
      }
    } else schedule(node.children, node, documentRow);
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
  if (task.focused) return changed;
  const forest = taskForest(tasks);
  if (forest.documents.some((document) => document.focusedCount > 0)) {
    const expanded = collapsed.expandedGroups ||= new Set();
    const open = (key) => {
      if (!expanded.has(key)) { expanded.add(key); changed = true; }
    };
    const document = forest.documents.find((candidate) => candidate.path === task.path);
    if (document && document.focusedCount === 0) open(FILES_GROUP);
    let node = forest.nodes.get(task.key);
    while (node && node.focusedCount === 0) {
      const parent = forest.nodes.get(node.task.parentKey);
      if (parent?.focusedCount > 0) open(groupKey(parent));
      else if (!parent && document?.focusedCount > 0) open(groupKey(document));
      node = parent;
    }
  }
  return changed;
}
