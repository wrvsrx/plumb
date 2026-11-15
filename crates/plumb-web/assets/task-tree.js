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
// Each page retains complete filtered documents. The server supplies real owner
// identities and labels: context rows never invent tasks or matching results.
function taskForest(tasks) {
  const documents = new Map();
  const nodes = new Map(tasks.map((task) => [task.key, {
    task, children: [], total: 1, focusedCount: task.focused ? 1 : 0,
  }]));
  const parents = new Map();
  const owners = new Map(tasks.filter((task) => task.sourceStart !== undefined && task.locator?.kind !== 'document')
    .map((task) => [JSON.stringify([task.path, task.sourceStart]), nodes.get(task.key)]));
  const contexts = new Map();
  const tasksByDocument = new Map(tasks.filter((task) => task.locator?.kind === 'document').map((task) => [task.path, nodes.get(task.key)]));
  const attach = (parent, node) => {
    if (parents.has(node)) return;
    parents.set(node, parent);
    parent.children.push(node);
  };
  for (const task of tasks) {
    if (!documents.has(task.path)) documents.set(task.path, {
      path: task.path, children: [], total: 0, focusedCount: 0,
      segments: [{ title: task.ancestry?.[0] || task.path, documentId: task.documentId, revision: task.revision, start: null }],
    });
    const document = documents.get(task.path);
    const node = nodes.get(task.key);
    if (task.context) {
      let parent = document;
      const documentTask = tasksByDocument.get(task.path);
      if (documentTask && documentTask !== node) {
        attach(document, documentTask);
        parent = documentTask;
      }
      const ancestry = [...task.context];
      // Empty heads are omitted from context labels, but a retained task still
      // owns its descendants and must remain an independent actionable row.
      const taskParent = nodes.get(task.parentKey)?.task;
      if (taskParent && taskParent.locator?.kind !== 'document'
        && !ancestry.some((ancestor) => ancestor.start === taskParent.sourceStart)) {
        ancestry.push({ start: taskParent.sourceStart });
        ancestry.sort((a, b) => a.start - b.start);
      }
      for (const ancestor of ancestry) {
        let owner = owners.get(JSON.stringify([task.path, ancestor.start]));
        if (!owner) {
          const key = JSON.stringify([task.path, task.revision, ancestor.start]);
          owner = contexts.get(key);
          if (!owner) {
            owner = { kind: 'context', key, path: task.path, facet: ancestor.facet,
              segments: [{ ...ancestor, documentId: task.documentId, revision: task.revision }],
              children: [], total: 0, focusedCount: 0 };
            contexts.set(key, owner);
          }
        }
        attach(parent, owner);
        parent = owner;
      }
      attach(parent, node);
    } else {
      const parent = nodes.get(task.parentKey);
      attach(parent && parent.task.path === task.path ? parent : document, node);
    }
  }
  // Postorder without recursion; context depth is independent of task depth.
  const ordered = [], pending = [...documents.values()];
  while (pending.length) {
    const node = pending.pop();
    ordered.push(node);
    pending.push(...node.children);
  }
  for (const node of ordered.reverse()) {
    const parent = parents.get(node);
    if (parent) { parent.total += node.total; parent.focusedCount += node.focusedCount; }
  }
  return { documents: [...documents.values()], nodes, parents };
}

function groupKey(parent) {
  if (parent.groupOwner) return JSON.stringify(['context', parent.groupOwner]);
  return JSON.stringify(parent.task ? ['task', parent.task.key] : parent.kind === 'context' ? ['context', parent.key] : ['document', parent.path]);
}
const FILES_GROUP = JSON.stringify(['files']);

// Transform the presentation tree before flattening. Groups are real display
// parents; source task parentKey/depth never changes.
export function taskDisplayTree(tasks) {
  const forest = taskForest(tasks);
  // A document task owns the file's tree; it replaces the virtual file row.
  const documents = forest.documents.map((document) => (
    document.children.find((node) => node.task?.locator?.kind === 'document') || document
  ));
  // Compact only the retained structural tree, before focus groups hide branches.
  const pending = [...documents];
  while (pending.length) {
    const node = pending.pop();
    if (!node.task && !node.facet) {
      while (node.children.length === 1 && node.children[0].kind === 'context' && !node.children[0].facet) {
        const child = node.children[0];
        node.segments.push(...child.segments);
        (node.contextKeys ||= []).push(child.key, ...(child.contextKeys || []));
        node.children = child.children;
        node.groupOwner = child.key;
      }
    }
    pending.push(...node.children);
  }
  if (!documents.some((node) => node.focusedCount > 0)) return documents;
  const root = { children: documents };
  const work = [root];
  while (work.length) {
    const parent = work.pop();
    const focused = parent.children.filter((node) => node.focusedCount > 0);
    const plain = parent.children.filter((node) => node.focusedCount === 0);
    parent.children = [...focused];
    if (plain.length) parent.children.push({
      kind: 'group', key: parent === root ? FILES_GROUP : groupKey(parent),
      children: plain, total: plain.reduce((sum, node) => sum + node.total, 0),
      documents: parent === root ? plain.length : 0, focusedCount: 0,
    });
    work.push(...focused);
  }
  return root.children;
}

export function taskListItems(tasks, collapsed = {}) {
  const items = [];
  const work = taskDisplayTree(tasks).map((node) => ({ node, depth: 0 })).reverse();
  while (work.length) {
    const { node, depth, documentRow: ownerDocument } = work.pop();
    let documentRow = ownerDocument;
    let folded;
    if (node.kind === 'group') {
      folded = !keySet(collapsed.expandedGroups).has(node.key);
      items.push({ ...node, depth, collapsed: folded });
      if (folded && documentRow) documentRow.hidden += node.total;
    } else if (node.kind === 'context') {
      folded = [node.key, ...(node.contextKeys || [])].some((key) => keySet(collapsed.contexts).has(key));
      items.push({ ...node, depth, collapsed: folded, hidden: folded ? node.total : 0 });
      if (folded && documentRow) documentRow.hidden += node.total;
    } else if (!node.task) {
      folded = keySet(collapsed.documents).has(node.path)
        || (node.contextKeys || []).some((key) => keySet(collapsed.contexts).has(key));
      documentRow = {
        kind: 'document', path: node.path, total: node.total, depth,
        segments: node.segments, contextKeys: node.contextKeys,
        hidden: folded ? node.total : 0, collapsed: folded, focusedCount: node.focusedCount,
      };
      items.push(documentRow);
    } else {
      let childCount = 0;
      const descendants = [...node.children];
      while (descendants.length) {
        const child = descendants.pop();
        if (child.task) childCount += 1;
        else descendants.push(...child.children);
      }
      folded = childCount > 0 && keySet(collapsed.tasks).has(node.task.key);
      const hiddenCount = folded ? node.total - 1 : 0;
      if (documentRow) documentRow.hidden += hiddenCount;
      items.push({ kind: 'task', task: node.task, depth, childCount,
        collapsed: folded, hiddenCount, focusedCount: node.focusedCount });
    }
    if (!folded) for (let index = node.children.length - 1; index >= 0; index -= 1) {
      work.push({ node: node.children[index], depth: depth + 1, documentRow });
    }
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
  return revealTasks(collapsed, tasks, task ? [task] : []);
}

// Share the forest/index for a whole visible frontier during reconciliation.
export function revealTasks(collapsed, tasks, targets) {
  if (!targets.length) return false;
  const forest = taskForest(tasks);
  const documents = new Map(forest.documents.map((node) => [node.path, node]));
  const grouped = forest.documents.some((document) => document.focusedCount > 0);
  const collapsedDocuments = keySet(collapsed.documents);
  const collapsedTasks = keySet(collapsed.tasks);
  const expanded = collapsed.expandedGroups ||= new Set();
  let changed = false;
  const open = (key) => {
    if (!expanded.has(key)) { expanded.add(key); changed = true; }
  };
  for (const task of targets) {
    if (collapsedDocuments.delete(task.path)) changed = true;
    let node = forest.nodes.get(task.key);
    let ancestor = forest.parents.get(node);
    while (ancestor) {
      if (ancestor.task && collapsedTasks.delete(ancestor.task.key)) changed = true;
      if (ancestor.kind === 'context' && keySet(collapsed.contexts).delete(ancestor.key)) changed = true;
      ancestor = forest.parents.get(ancestor);
    }
    if (!grouped || task.focused) continue;
    const document = documents.get(task.path);
    if (document && document.focusedCount === 0) open(FILES_GROUP);
    while (node && node.focusedCount === 0) {
      const parent = forest.parents.get(node);
      if (parent?.focusedCount > 0) open(groupKey(parent));
      node = parent;
    }
  }
  return changed;
}
