export function viewFromPath(pathname) {
  if (/\/tasks(?:\/index\.html)?\/?$/.test(pathname)) return 'tasks';
  if (/\/agenda(?:\/index\.html)?\/?$/.test(pathname)) return 'agenda';
  return 'graph';
}

export function readQueryParameters(search, view = 'graph') {
  const params = new URLSearchParams(search);
  return {
    presets: params.getAll('preset').filter(Boolean),
    presetsSpecified: params.has('preset'),
    query: params.get('q') || '',
    filters: params.getAll('cel'),
    sort: params.get('sort') || (view === 'tasks' ? 'priority' : 'source'),
    selected: params.get('selected'),
    current: params.get('current'),
    depth: params.get('depth') || '1',
    direction: params.get('direction') || 'both',
    kinds: params.getAll('kind'),
  };
}

export function initialPresets(view, query) {
  return view === 'tasks' && !query.presetsSpecified ? ['ready'] : query.presets;
}

export function writeQueryParameters(query) {
  const params = new URLSearchParams();
  query.presets.forEach((preset) => params.append('preset', preset));
  if (query.presetsSpecified && query.presets.length === 0) params.set('preset', '');
  if (query.query) params.set('q', query.query);
  (query.filters || []).filter(Boolean).forEach((filter) => params.append('cel', filter));
  if (query.sort !== 'source') params.set('sort', query.sort);
  if (query.selected) params.set('selected', query.selected);
  if (query.current) {
    params.set('current', query.current);
    params.set('depth', query.depth);
    params.set('direction', query.direction);
  }
  query.kinds.forEach((kind) => params.append('kind', kind));
  return params;
}

export function addPreset(selected, added) {
  return selected.includes(added.id) ? selected : [...selected, added.id];
}

export function addPresetGroup(selected, group, registry) {
  if (registry.some((preset) => preset.group === group && selected.includes(preset.id))) return selected;
  const initial = registry.find((preset) => preset.group === group);
  return initial ? [...selected, initial.id] : selected;
}

export function togglePresetValue(selected, preset, registry) {
  if (!preset.group) return addPreset(selected, preset);
  if (!selected.includes(preset.id)) return [...selected, preset.id];
  const selectedInGroup = registry.filter((item) => item.group === preset.group && selected.includes(item.id));
  return selectedInGroup.length === 1 ? selected : selected.filter((id) => id !== preset.id);
}

export function taskByKey(snapshot, key) {
  if (!snapshot || !key) return null;
  return (snapshot.allTasks || snapshot.tasks || []).find((task) => task.key === key) || null;
}

export function sortTaskTrees(tasks, sort, scores = new Map()) {
  const source = tasks.slice().sort(taskSourceOrder);
  const byDocument = new Map();
  source.forEach((task) => {
    if (!byDocument.has(task.path)) byDocument.set(task.path, []);
    byDocument.get(task.path).push(task);
  });
  const documents = Array.from(byDocument, ([path, items]) => {
    const children = taskForest(items, scores);
    sortTaskForest(children, sort);
    return {
      path,
      children,
      priority: Math.max(0, ...children.map((child) => child.priority)),
      due: minimum(children.map((child) => child.due)),
      relevance: maximum(children.map((child) => child.relevance)),
    };
  });
  documents.sort((left, right) => aggregateOrder(left, right, sort) || left.path.localeCompare(right.path));
  return documents.flatMap((document) => document.children.flatMap(flattenTaskTree));
}

function taskForest(tasks, scores) {
  const forest = [];
  for (let index = 0; index < tasks.length;) {
    const root = tasks[index];
    let end = index + 1;
    while (end < tasks.length && tasks[end].path === root.path && tasks[end].depth > root.depth) end += 1;
    const children = taskForest(tasks.slice(index + 1, end), scores);
    forest.push({
      root,
      children,
      priority: Math.max(root.priority ?? 0, ...children.map((child) => child.priority)),
      due: minimum([root.due, ...children.map((child) => child.due)]),
      relevance: maximum([scores.get(root.key), ...children.map((child) => child.relevance)]),
    });
    index = end;
  }
  return forest;
}

function sortTaskForest(forest, sort) {
  forest.forEach((tree) => sortTaskForest(tree.children, sort));
  forest.sort((left, right) => aggregateOrder(left, right, sort) || taskSourceOrder(left.root, right.root));
}

function aggregateOrder(left, right, sort) {
  if (sort === 'priority') return right.priority - left.priority || compareDue(left.due, right.due);
  if (sort === 'due') return compareDue(left.due, right.due);
  if (sort === 'relevance') return (right.relevance ?? Number.NEGATIVE_INFINITY) - (left.relevance ?? Number.NEGATIVE_INFINITY);
  return 0;
}

function taskSourceOrder(left, right) {
  return left.path.localeCompare(right.path) || left.location.start - right.location.start || left.key.localeCompare(right.key);
}

function compareDue(left, right) {
  if (left && right) return Date.parse(left) - Date.parse(right);
  if (left) return -1;
  if (right) return 1;
  return 0;
}

function minimum(values) {
  const present = values.filter((value) => value !== null && value !== undefined);
  return present.length ? present.reduce((left, right) => compareDue(left, right) <= 0 ? left : right) : null;
}

function maximum(values) {
  const present = values.filter((value) => value !== null && value !== undefined);
  return present.length ? Math.max(...present) : null;
}

function flattenTaskTree(tree) {
  return [tree.root, ...tree.children.flatMap(flattenTaskTree)];
}

export function readyTaskQueryRequest() {
  return {
    view: 'tasks',
    query: '',
    presets: ['ready'],
    filters: [],
    sort: 'source',
    limit: null,
    traversal: {},
  };
}

export function readyTasksFromSnapshot(snapshot) {
  return {
    ...snapshot,
    tasks: (snapshot.tasks || []).filter((task) => task.state === 'ready'),
  };
}
