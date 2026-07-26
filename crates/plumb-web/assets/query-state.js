export function viewFromPath(pathname) {
  return /\/tasks(?:\/index\.html)?\/?$/.test(pathname) ? 'tasks' : 'graph';
}

export function readQueryParameters(search) {
  const params = new URLSearchParams(search);
  return {
    presets: params.getAll('preset'),
    query: params.get('q') || '',
    filter: params.get('cel') || '',
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
  if (query.filter) params.set('cel', query.filter);
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

export function addPreset(selected, added, registry) {
  const replaced = added.group
    ? new Set(registry.filter((preset) => preset.group === added.group).map((preset) => preset.id))
    : new Set();
  return [...selected.filter((id) => !replaced.has(id) && id !== added.id), added.id];
}
