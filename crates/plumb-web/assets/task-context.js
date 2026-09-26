import { revealTask, revealTasks, taskAncestorKeys, taskListItems } from './task-tree.js';

// Interaction identity is separate from the business-level focused flag.
// Offset locators have no cross-revision identity; never guess by title/offset.
function identity(task) {
  return task.locator?.kind === 'offset'
    ? JSON.stringify([task.key, task.revision]) : JSON.stringify([task.key]);
}

export function captureTaskContext(tasks, selected, collapsed) {
  return {
    tasks,
    selected,
    visible: taskListItems(tasks, collapsed).filter((item) => item.kind === 'task').map((item) => item.task),
    collapsed: {
      documents: new Set(collapsed.documents),
      tasks: new Set(collapsed.tasks),
      expandedGroups: new Set(collapsed.expandedGroups),
    },
  };
}

export function reconcileTaskContext(context, tasks) {
  const current = new Map(tasks.map((task) => [identity(task), task]));
  const previous = new Map(context.tasks.map((task) => [task.key, task]));
  const surviving = (task) => task && current.get(identity(task));
  const collapsed = {
    documents: new Set(context.collapsed.documents),
    tasks: new Set(context.collapsed.tasks),
    expandedGroups: new Set(context.collapsed.expandedGroups),
  };
  // Do not transfer offset-based folding to a different node in a new revision.
  for (const key of collapsed.tasks) {
    const old = previous.get(key);
    if (old?.locator?.kind === 'offset' && !surviving(old)) collapsed.tasks.delete(key);
  }
  for (const key of collapsed.expandedGroups) {
    const [kind, owner] = JSON.parse(key);
    const old = previous.get(owner);
    if (kind === 'task' && old?.locator?.kind === 'offset' && !surviving(old)) collapsed.expandedGroups.delete(key);
  }
  // Preserve the visible frontier when focus grouping inserts new ancestors.
  revealTasks(collapsed, tasks, context.visible.map(surviving).filter(Boolean));
  const previouslyFocused = new Set(context.tasks.filter((task) => task.focused).map(identity));
  revealTasks(collapsed, tasks, tasks.filter((task) => task.focused && !previouslyFocused.has(identity(task))));
  const old = previous.get(context.selected);
  let selected = surviving(old);
  if (!old) selected = tasks.find((task) => task.key === context.selected);
  if (old && !selected) {
    const ancestors = taskAncestorKeys(context.tasks, old);
    const index = context.visible.findIndex((task) => task.key === old.key);
    const neighbors = index < 0 ? [] : [
      ...context.visible.slice(index + 1),
      ...context.visible.slice(0, index).reverse(),
    ].filter((task) => !ancestors.includes(task.key));
    selected = [...neighbors, ...ancestors.map((key) => previous.get(key))]
      .map(surviving).find(Boolean);
  }
  const pathChanged = selected && old && (selected.path !== old.path
    || JSON.stringify(taskAncestorKeys(tasks, selected)) !== JSON.stringify(taskAncestorKeys(context.tasks, old)));
  if (selected && (!old || selected.key !== old.key || pathChanged)) revealTask(collapsed, tasks, selected);
  return { selected: selected?.key || null, collapsed };
}

// Sort belongs to the ongoing tree view, not the membership/search session.
export function taskQueryScope(request) {
  return JSON.stringify([request.query, request.presets, request.filters]);
}

export async function queryTaskContext(request, snapshot, append, execute) {
  const refresh = async (extra = 0) => {
    const limit = Math.max(request.limit, (snapshot?.tasks.length || 0) + extra);
    const result = await execute({
      ...request,
      cursor: null,
      limit,
      retainedDocuments: [...new Set((snapshot?.tasks || []).map((task) => task.documentId))],
    });
    return { ...result, tasks: { ...result.tasks, pageLimit: limit } };
  };
  if (!append || !snapshot?.nextCursor) return refresh();
  try {
    const limit = snapshot.pageLimit || request.limit;
    const result = await execute({ ...request, limit, cursor: snapshot.nextCursor });
    if (result.tasks.revision === snapshot.revision) {
      return { ...result, tasks: { ...result.tasks, pageLimit: limit, tasks: [...snapshot.tasks, ...result.tasks.tasks] } };
    }
  } catch (error) {
    if (error.source !== 'cursor') throw error;
  }
  // A stale page is not a deletion. Rebuild one coherent prefix at the new revision.
  return refresh(request.limit);
}

// Viewport observations are ephemeral; selection is deliberately not the
// reading anchor when a neighboring task can carry the browsing context.
export function reconcileTaskViewport(previous, tasks, observations, selected, operation) {
  const old = new Map(previous.map((task) => [task.key, task]));
  const current = new Map(tasks.map((task) => [task.key, task]));
  const active = old.get(operation || selected);
  const candidates = observations.filter(({ key }) => {
    const before = old.get(key), after = current.get(key);
    return before && after && identity(before) === identity(after)
      && before.parentKey === after.parentKey && Boolean(before.focused) === Boolean(after.focused);
  });
  return candidates.find(({ key }) => key !== selected && old.get(key).path !== active?.path)
    || candidates.find(({ key }) => key !== selected && key !== operation)
    || candidates[0] || null;
}
