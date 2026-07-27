export function viewFromPath(pathname) {
  return /\/tasks(?:\/index\.html)?\/?$/.test(pathname) ? 'tasks' : 'graph';
}

export function readQueryParameters(search) {
  const params = new URLSearchParams(search);
  return {
    presets: params.getAll('preset'),
    query: params.get('q') || '',
    filters: params.getAll('cel'),
    sort: params.get('sort') || 'source',
    selected: params.get('selected'),
    current: params.get('current'),
    depth: params.get('depth') || '1',
    direction: params.get('direction') || 'both',
    kinds: params.getAll('kind'),
  };
}

export function writeQueryParameters(query) {
  const params = new URLSearchParams();
  query.presets.forEach((preset) => params.append('preset', preset));
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
