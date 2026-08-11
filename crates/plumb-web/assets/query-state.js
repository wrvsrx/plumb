export function viewFromPath(pathname) {
  if (/\/tasks\/?$/.test(pathname)) return 'tasks';
  if (/\/agenda\/?$/.test(pathname)) return 'agenda';
  return 'graph';
}

export function readQueryParameters(search, view = 'graph') {
  const params = new URLSearchParams(search);
  const sortsSpecified = params.has('sort');
  const sort = normalizeSortKeys(params.getAll('sort'));
  return {
    presets: params.getAll('preset').filter(Boolean),
    presetsSpecified: params.has('preset'),
    query: params.get('q') || '',
    filters: params.getAll('cel'),
    sort: sortsSpecified ? sort : [view === 'tasks' ? 'priority' : 'source'],
    sortsSpecified,
    selected: params.get('selected'),
    current: params.get('current'),
    depth: params.get('depth') || '1',
    direction: params.get('direction') || 'both',
    kinds: params.getAll('kind'),
  };
}

export function initialPresets(view, query) {
  return view === 'tasks' && !query.presetsSpecified ? ['ready', 'blocked'] : query.presets;
}

export function writeQueryParameters(query) {
  const params = new URLSearchParams();
  query.presets.forEach((preset) => params.append('preset', preset));
  if (query.presetsSpecified && query.presets.length === 0) params.set('preset', '');
  if (query.query) params.set('q', query.query);
  (query.filters || []).filter(Boolean).forEach((filter) => params.append('cel', filter));
  (query.sort || []).forEach((sort) => params.append('sort', sort));
  if (query.sortsSpecified && query.sort.length === 0) params.set('sort', '');
  if (query.selected) params.set('selected', query.selected);
  if (query.current) {
    params.set('current', query.current);
    params.set('depth', query.depth);
    params.set('direction', query.direction);
  }
  query.kinds.forEach((kind) => params.append('kind', kind));
  return params;
}

export function normalizeSortKeys(keys) {
  const valid = new Set(['source', 'priority', 'due', 'relevance']);
  return keys.filter((key, index) => key && valid.has(key) && keys.indexOf(key) === index);
}

export function addSortKey(keys, key) {
  return normalizeSortKeys([...keys, key]);
}

export function moveSortKey(keys, key, target) {
  if (key === target || !keys.includes(key) || !keys.includes(target)) return [...keys];
  const moved = keys.filter((item) => item !== key);
  moved.splice(moved.indexOf(target), 0, key);
  return moved;
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
  return (snapshot.tasks || []).find((task) => task.key === key)
    || (snapshot.allTasks || []).find((task) => task.key === key) || null;
}
