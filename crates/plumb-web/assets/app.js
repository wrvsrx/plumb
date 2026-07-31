import {
  addPreset,
  addPresetGroup,
  addSortKey,
  initialPresets,
  normalizeSortKeys,
  moveSortKey,
  readQueryParameters,
  readyTaskQueryRequest,
  readyTasksFromSnapshot,
  taskByKey,
  togglePresetValue,
  viewFromPath,
  writeQueryParameters,
} from './query-state.js';

(function () {
  'use strict';

  const config = JSON.parse(document.body.dataset.plumbConfig);
  const initialView = viewFromPath(location.pathname);
  const state = {
    graph: null,
    graphView: null,
    graphConfigured: false,
    renderedNodes: [],
    renderedEdges: [],
    labelBounds: [],
    current: null,
    hovered: null,
    local: false,
    searchTimer: null,
    view: initialView,
    tasks: null,
    selectedTask: null,
    presets: { graph: [], tasks: ['ready'], agenda: [] },
    presetsSpecified: { graph: false, tasks: false, agenda: false },
    query: { graph: '', tasks: '', agenda: '' },
    filters: { graph: [], tasks: [], agenda: [] },
    sort: { graph: ['source'], tasks: ['priority'], agenda: ['source'] },
    sortsSpecified: { graph: false, tasks: false, agenda: false },
    presetRegistry: { graph: [], tasks: [] },
    selectedGraph: null,
    pendingTask: null,
    events: null,
    selectedEvent: null,
    pendingEvent: false,
  };

  const graphElement = document.getElementById('graph');
  const panel = document.getElementById('note-panel');
  const summary = document.getElementById('summary');
  const empty = document.getElementById('graph-empty');
  const search = document.getElementById('search');
  const depth = document.getElementById('depth');
  const direction = document.getElementById('direction');
  const allLabels = document.getElementById('all-labels');
  const globalMode = document.getElementById('global-mode');
  const localMode = document.getElementById('local-mode');
  const graphWorkspace = document.querySelector('.workspace');
  const taskWorkspace = document.querySelector('.task-workspace');
  const graphViewButton = document.getElementById('graph-view');
  const tasksViewButton = document.getElementById('tasks-view');
  const agendaViewButton = document.getElementById('agenda-view');
  const taskSearch = document.getElementById('task-search');
  const taskSummary = document.getElementById('task-summary');
  const taskList = document.getElementById('task-list');
  const taskEmpty = document.getElementById('task-empty');
  const taskPanel = document.getElementById('task-panel');
  const notification = document.getElementById('notification');
  const agendaWorkspace = document.querySelector('.agenda-workspace');
  const eventList = document.getElementById('event-list');
  const eventEmpty = document.getElementById('event-empty');
  const eventPanel = document.getElementById('event-panel');
  let notificationTimer;

  function readUrlState() {
    state.view = viewFromPath(location.pathname);
    const query = readQueryParameters(location.search, state.view);
    state.presets[state.view] = initialPresets(state.view, query);
    state.presetsSpecified[state.view] = query.presetsSpecified;
    state.query[state.view] = query.query;
    state.filters[state.view] = query.filters;
    state.sort[state.view] = query.sort;
    state.sortsSpecified[state.view] = query.sortsSpecified;
    state.current = query.current || config.current || null;
    state.local = Boolean(query.current);
    if (state.view === 'tasks') state.selectedTask = query.selected;
    if (state.view === 'agenda') state.selectedEvent = query.selected;
    state.selectedGraph = state.view === 'graph' ? query.selected : state.selectedGraph;
    if (state.view === 'graph') {
      depth.value = query.depth;
      direction.value = query.direction;
      if (query.kinds.length) {
        document.querySelectorAll('.graph-filters .edge-options input[value]').forEach((input) => {
          input.checked = query.kinds.includes(input.value);
        });
      }
    }
  }

  function routeFor(view) {
    const route = view === 'graph' ? config.graphRoute : (view === 'tasks' ? config.tasksRoute : config.agendaRoute);
    return new URL(route, location.href);
  }

  function updateUrl(mode = 'replace') {
    const url = routeFor(state.view);
    url.search = writeQueryParameters({
      presets: state.presets[state.view],
      presetsSpecified: state.presetsSpecified[state.view],
      query: state.query[state.view],
      filters: state.filters[state.view],
      sort: state.sort[state.view],
      sortsSpecified: state.sortsSpecified[state.view],
      selected: state.view === 'graph' ? state.selectedGraph : (state.view === 'tasks' ? state.selectedTask : state.selectedEvent),
      current: state.view === 'graph' && state.local ? state.current : null,
      depth: depth.value,
      direction: direction.value,
      kinds: state.view === 'graph' ? selectedKinds() : [],
    });
    history[mode === 'push' ? 'pushState' : 'replaceState']({}, '', url);
  }

  function notify(message, error = false) {
    clearTimeout(notificationTimer);
    notification.textContent = message;
    notification.classList.toggle('error', error);
    notification.hidden = false;
    notificationTimer = setTimeout(() => { notification.hidden = true; }, error ? 8000 : 4000);
  }

  function setQueryError(view, error) {
    const element = document.querySelector(`.${view === 'graph' ? 'graph' : 'task'}-filters .query-error`);
    element.textContent = error ? `${error.source || 'query'}: ${error.message || error}` : '';
    element.title = element.textContent;
  }

  async function loadPresetRegistry() {
    const response = await fetch(config.presetsUrl, { cache: 'no-store' });
    if (!response.ok) throw new Error(await response.text());
    state.presetRegistry = await response.json();
    renderPresetControls('graph');
    renderPresetControls('tasks');
  }

  function groupLabel(group) {
    return `${group.charAt(0).toUpperCase()}${group.slice(1)}`;
  }

  function renderPresetControls(view, { openGroup = null } = {}) {
    const container = document.querySelector(`.${view === 'graph' ? 'graph' : 'task'}-filters`);
    const menu = container.querySelector('.preset-menu');
    const chips = container.querySelector('.preset-chips');
    const registry = state.presetRegistry[view] || [];
    menu.replaceChildren();
    const registeredGroups = Array.from(new Set(registry.map((preset) => preset.group).filter(Boolean)));
    registeredGroups
      .filter((group) => !registry.some((preset) => preset.group === group && state.presets[view].includes(preset.id)))
      .forEach((group) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.role = 'menuitem';
        button.textContent = groupLabel(group);
        button.addEventListener('click', () => {
          state.presetsSpecified[view] = true;
          state.presets[view] = addPresetGroup(state.presets[view], group, registry);
          menu.hidden = true;
          renderPresetControls(view);
          updateUrl();
          runViewQuery(view);
        });
        menu.append(button);
      });
    registry.filter((preset) => !preset.group && !state.presets[view].includes(preset.id)).forEach((preset) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.role = 'menuitem';
      button.textContent = preset.label;
      button.title = preset.expression;
      button.addEventListener('click', () => {
        state.presetsSpecified[view] = true;
        state.presets[view] = addPreset(state.presets[view], preset);
        menu.hidden = true;
        renderPresetControls(view);
        updateUrl();
        runViewQuery(view);
      });
      menu.append(button);
    });
    const addCel = document.createElement('button');
    addCel.type = 'button';
    addCel.role = 'menuitem';
    addCel.textContent = 'Custom CEL';
    addCel.title = 'Add a CEL condition';
    addCel.addEventListener('click', () => {
      state.filters[view].push('');
      menu.hidden = true;
      renderCelClauses(view, { focusLast: true });
    });
    menu.append(addCel);
    chips.replaceChildren();
    const selectedPresets = state.presets[view]
      .map((id) => registry.find((item) => item.id === id))
      .filter(Boolean);
    const groups = new Map();
    selectedPresets.forEach((preset) => {
      const key = preset.group || `preset:${preset.id}`;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(preset);
    });
    groups.forEach((presets, key) => {
      const chip = document.createElement('div');
      chip.className = 'preset-chip';
      const grouped = presets[0].group;
      const label = grouped ? `${groupLabel(grouped)}: ${presets.map((preset) => preset.label).join(', ')}` : presets[0].label;
      if (grouped) {
        chip.dataset.group = grouped;
        const configure = document.createElement('button');
        configure.type = 'button';
        configure.className = 'preset-chip-label';
        configure.textContent = label;
        configure.setAttribute('aria-expanded', String(openGroup === grouped));
        configure.addEventListener('click', () => {
          const values = chip.querySelector('.preset-values');
          values.hidden = !values.hidden;
          configure.setAttribute('aria-expanded', String(!values.hidden));
        });
        const values = document.createElement('div');
        values.className = 'preset-values';
        values.setAttribute('role', 'menu');
        values.hidden = openGroup !== grouped;
        registry.filter((preset) => preset.group === grouped).forEach((preset) => {
          const option = document.createElement('button');
          const selected = state.presets[view].includes(preset.id);
          const onlySelected = selected && presets.length === 1;
          option.type = 'button';
          option.role = 'menuitemcheckbox';
          option.setAttribute('aria-checked', String(selected));
          option.textContent = `${selected ? '✓ ' : ''}${preset.label}`;
          option.disabled = onlySelected;
          option.addEventListener('click', () => {
            state.presetsSpecified[view] = true;
            state.presets[view] = togglePresetValue(state.presets[view], preset, registry);
            renderPresetControls(view, { openGroup: grouped });
            updateUrl();
            runViewQuery(view);
          });
          values.append(option);
        });
        chip.append(configure, values);
      } else {
        const text = document.createElement('span');
        text.textContent = label;
        chip.append(text);
      }
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'remove-filter';
      remove.textContent = '×';
      remove.title = `Remove ${label}`;
      remove.setAttribute('aria-label', `Remove ${label}`);
      remove.addEventListener('click', () => {
        state.presetsSpecified[view] = true;
        const ids = new Set(presets.map((preset) => preset.id));
        state.presets[view] = state.presets[view].filter((id) => !ids.has(id));
        renderPresetControls(view);
        updateUrl();
        runViewQuery(view);
      });
      chip.append(remove);
      chips.append(chip);
    });
  }

  function syncQueryControls(view) {
    search.value = state.query.graph;
    taskSearch.value = state.query.tasks;
    const container = document.querySelector(`.${view === 'graph' ? 'graph' : 'task'}-filters`);
    renderCelClauses(view);
    const sort = container.querySelector('.query-sort');
    if (sort) sort.value = state.sort[view][0] || 'source';
    if (view === 'tasks') renderTaskSortKeys();
    renderPresetControls(view);
  }

  function changeTaskSort(keys) {
    state.sort.tasks = normalizeSortKeys(keys);
    state.sortsSpecified.tasks = true;
    renderTaskSortKeys();
    updateUrl();
    loadTasks();
  }

  function renderTaskSortKeys() {
    const container = document.querySelector('.task-sort-keys');
    if (!container) return;
    container.replaceChildren();
    const labels = { priority: 'Priority ↓', due: 'Due ↑', relevance: 'Relevance ↓' };
    state.sort.tasks.forEach((key, index) => {
      if (key === 'source') return;
      const row = document.createElement('div');
      row.className = 'task-sort-key';
      row.draggable = true;
      row.dataset.key = key;
      const grip = document.createElement('button');
      grip.type = 'button'; grip.className = 'sort-grip'; grip.textContent = '↕';
      grip.title = `Drag ${labels[key]}`; grip.setAttribute('aria-label', grip.title);
      const label = document.createElement('span'); label.textContent = labels[key];
      const up = document.createElement('button'); up.type = 'button'; up.textContent = '↑'; up.title = 'Move up'; up.disabled = index === 0;
      const down = document.createElement('button'); down.type = 'button'; down.textContent = '↓'; down.title = 'Move down'; down.disabled = index === state.sort.tasks.length - 1;
      const remove = document.createElement('button'); remove.type = 'button'; remove.textContent = '×'; remove.title = `Remove ${labels[key]}`;
      up.addEventListener('click', () => { const keys = [...state.sort.tasks]; [keys[index - 1], keys[index]] = [keys[index], keys[index - 1]]; changeTaskSort(keys); });
      down.addEventListener('click', () => { const keys = [...state.sort.tasks]; [keys[index], keys[index + 1]] = [keys[index + 1], keys[index]]; changeTaskSort(keys); });
      remove.addEventListener('click', () => changeTaskSort(state.sort.tasks.filter((item) => item !== key)));
      row.addEventListener('keydown', (event) => {
        if (event.altKey && event.key === 'ArrowUp' && index > 0) { event.preventDefault(); up.click(); }
        if (event.altKey && event.key === 'ArrowDown' && index + 1 < state.sort.tasks.length) { event.preventDefault(); down.click(); }
      });
      row.addEventListener('dragstart', (event) => { row.classList.add('dragging'); event.dataTransfer.setData('text/plain', key); });
      row.addEventListener('dragend', () => row.classList.remove('dragging'));
      row.addEventListener('dragover', (event) => event.preventDefault());
      row.addEventListener('drop', (event) => {
        event.preventDefault();
        const moved = event.dataTransfer.getData('text/plain');
        if (!moved || moved === key) return;
        changeTaskSort(moveSortKey(state.sort.tasks, moved, key));
      });
      row.append(grip, label, up, down, remove);
      container.append(row);
    });
    const add = document.querySelector('.task-sort-add');
    Array.from(add.options).forEach((option) => { option.disabled = state.sort.tasks.includes(option.value); });
  }

  function renderCelClauses(view, { focusLast = false } = {}) {
    const container = document.querySelector(`.${view === 'graph' ? 'graph' : 'task'}-filters .cel-clauses`);
    container.replaceChildren();
    state.filters[view].forEach((value, index) => {
      const clause = document.createElement('label');
      clause.className = 'query-clause cel-clause';
      const kind = document.createElement('span');
      kind.textContent = 'CEL';
      const input = document.createElement('input');
      input.type = 'text';
      input.autocomplete = 'off';
      input.placeholder = 'Boolean expression';
      input.value = value;
      input.setAttribute('aria-label', `CEL condition ${index + 1}`);
      input.addEventListener('input', () => {
        state.filters[view][index] = input.value;
        updateUrl();
        clearTimeout(state.searchTimer);
        state.searchTimer = setTimeout(() => runViewQuery(view), 250);
      });
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'remove-clause';
      remove.textContent = '×';
      remove.title = 'Remove CEL condition';
      remove.setAttribute('aria-label', `Remove CEL condition ${index + 1}`);
      remove.addEventListener('click', () => {
        state.filters[view].splice(index, 1);
        renderCelClauses(view);
        updateUrl();
        runViewQuery(view);
      });
      clause.append(kind, input, remove);
      container.append(clause);
    });
    if (focusLast) container.querySelector('.cel-clause:last-child input')?.focus();
  }

  function runViewQuery(view) {
    if (view === 'graph') return loadGraph();
    if (view === 'tasks') return loadTasks();
    return loadEvents();
  }

  function selectedKinds() {
    return Array.from(document.querySelectorAll('.graph-filters .edge-options input[value]:checked')).map((input) => input.value);
  }

  function queryRequest(view) {
    return {
      view,
      query: state.query[view],
      presets: state.presets[view],
      filters: state.filters[view],
      sort: state.sort[view],
      limit: null,
      traversal: view === 'graph' ? {
        current: state.local ? state.current : null,
        depth: state.local ? Number(depth.value) : null,
        direction: direction.value,
        kinds: selectedKinds(),
        limit: null,
      } : {},
    };
  }

  async function executeQuery(view) {
    const response = await fetch(config.queryUrl, {
      method: 'POST',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(queryRequest(view)),
    });
    if (!response.ok) {
      let failure;
      try { failure = await response.json(); } catch (_) { failure = { source: 'request', message: await response.text() }; }
      const error = new Error(failure.message || `HTTP ${response.status}`);
      error.source = failure.source || 'request';
      throw error;
    }
    return response.json();
  }


  async function loadGraph() {
    try {
      const result = await executeQuery('graph');
      state.graph = result.graph;
      setQueryError('graph', null);
      renderGraph();
    } catch (error) {
      setQueryError('graph', error);
      if (!state.graph) summary.textContent = 'Graph unavailable';
    }
  }

  function renderGraph() {
    const { nodes, edges, topologyChanged } = reconcileGraph(state.graph.nodes, state.graph.edges);
    const byId = new Map(nodes.map((node) => [node.id, node]));
    edges.forEach((edge) => {
      byId.get(endpointId(edge.source)).degree += 1;
      byId.get(endpointId(edge.target)).degree += 1;
    });
    nodes.sort((left, right) => right.degree - left.degree || left.title.localeCompare(right.title));
    const hubs = nodes
      .slice()
      .sort((left, right) => right.degree - left.degree || left.title.localeCompare(right.title))
      .slice(0, 5);

    empty.hidden = nodes.length > 0;
    summary.textContent = `${nodes.length} notes, ${edges.length} connections${state.graph.complete ? '' : ' (truncated)'}`;
    state.renderedNodes = nodes;
    state.renderedEdges = edges;
    ensureGraphView();
    if (!state.graphConfigured || topologyChanged) {
      configureGraphView(nodes, edges, !state.graphConfigured);
    } else {
      refreshStyles();
    }
    renderOverview(hubs);
  }

  function edgeTopologyKey(edge) {
    return [endpointId(edge.source), endpointId(edge.target), edge.kind, edge.targetFragment || ''].join('\u0000');
  }

  function reconcileGraph(nextNodes, nextEdges) {
    const previousNodes = new Map(state.renderedNodes.map((node) => [node.id, node]));
    let topologyChanged = nextNodes.length !== state.renderedNodes.length;
    const nodes = nextNodes.map((next) => {
      const current = previousNodes.get(next.id);
      if (!current) {
        topologyChanged = true;
        return { ...next, degree: 0 };
      }
      previousNodes.delete(next.id);
      Object.assign(current, next, { degree: 0 });
      return current;
    });
    if (previousNodes.size > 0) topologyChanged = true;

    const previousEdges = new Map();
    state.renderedEdges.forEach((edge) => {
      const key = edgeTopologyKey(edge);
      if (!previousEdges.has(key)) previousEdges.set(key, []);
      previousEdges.get(key).push(edge);
    });
    const edges = nextEdges.map((next) => {
      const matches = previousEdges.get(edgeTopologyKey(next));
      const current = matches?.shift();
      if (!current) {
        topologyChanged = true;
        return { ...next };
      }
      const source = current.source;
      const target = current.target;
      Object.assign(current, next, { source, target });
      return current;
    });
    if (Array.from(previousEdges.values()).some((matches) => matches.length > 0)) topologyChanged = true;
    const directions = new Set(edges.map((edge) => `${endpointId(edge.source)}\u0000${endpointId(edge.target)}`));
    edges.forEach((edge) => {
      const reverse = `${endpointId(edge.target)}\u0000${endpointId(edge.source)}`;
      edge.curvature = directions.has(reverse) ? 0.12 : 0;
    });
    return { nodes, edges, topologyChanged };
  }

  function ensureGraphView() {
    if (state.graphView) return;
    state.graphView = ForceGraph()(graphElement)
      .backgroundColor('#f7f7f5')
      .nodeId('id')
      .linkSource('source')
      .linkTarget('target')
      .minZoom(0.1)
      .maxZoom(8)
      .onNodeClick((node) => selectNode(node))
      .onNodeHover(handleNodeHover)
      .onBackgroundClick(() => handleNodeHover(null));
    window.plumbGraph = state.graphView;
    new ResizeObserver(() => {
      state.graphView.width(graphElement.clientWidth).height(graphElement.clientHeight);
    }).observe(graphElement);
  }

  function configureGraphView(nodes, edges, initial) {
    const warmup = initial ? (nodes.length > 500 ? 100 : 260) : 0;
    state.graphView
      .width(graphElement.clientWidth)
      .height(graphElement.clientHeight)
      .warmupTicks(warmup)
      .cooldownTicks(1)
      .d3VelocityDecay(0.48)
      .nodeVal((node) => 2 + Math.sqrt(node.degree) * 2.2)
      .nodeColor(nodeColor)
      .nodeLabel(() => '')
      .nodeCanvasObjectMode(() => 'after')
      .nodeCanvasObject(drawNodeLabel)
      .onRenderFramePre(() => { state.labelBounds = []; })
      .linkColor(linkColor)
      .linkWidth(linkWidth)
      .linkCurvature('curvature')
      .linkDirectionalArrowLength((link) => link.kind === 'task-depends' ? 4 : 3)
      .linkDirectionalArrowRelPos(0.68)
      .linkDirectionalArrowColor(linkColor)
      .graphData({ nodes, links: edges });
    state.graphConfigured = true;
    state.graphView.d3Force('charge').strength(nodes.length > 250 ? -45 : -85);
    state.graphView.d3Force('link').distance(nodes.length > 250 ? 32 : 48);
  }

  function endpointId(endpoint) {
    return typeof endpoint === 'object' ? endpoint.id : endpoint;
  }

  function isHighlightedLink(link) {
    return state.hovered && (endpointId(link.source) === state.hovered || endpointId(link.target) === state.hovered);
  }

  function nodeColor(node) {
    if (node.id === state.current) return '#d94b3d';
    if (node.id === state.hovered) return '#d94b3d';
    if (node.unresolved) return '#a9aaa6';
    return '#188578';
  }

  function linkColor(link) {
    if (isHighlightedLink(link)) return link.kind === 'task-depends' ? '#d94b3d' : '#188578';
    if (link.kind === 'task-depends') return 'rgba(217, 75, 61, 0.32)';
    if (link.kind === 'task-prev') return 'rgba(215, 165, 34, 0.32)';
    if (link.kind === 'autolink') return 'rgba(24, 133, 120, 0.28)';
    return 'rgba(120, 124, 126, 0.22)';
  }

  function linkWidth(link) {
    return isHighlightedLink(link) ? 2.2 : 1;
  }

  function drawNodeLabel(node, context, globalScale) {
    if (!(allLabels.checked || state.query.graph || node.id === state.current || node.id === state.hovered)) return;
    const fontSize = 13 / globalScale;
    const padding = 4 / globalScale;
    context.font = `600 ${fontSize}px Inter, system-ui, sans-serif`;
    const width = context.measureText(node.title).width;
    const x = node.x - width / 2 - padding;
    const y = node.y + 7 / globalScale;
    const bounds = {
      left: x - padding,
      right: x + width + padding * 3,
      top: y - padding,
      bottom: y + fontSize + padding * 3,
    };
    const required = node.id === state.current || node.id === state.hovered || state.query.graph;
    if (!required && state.labelBounds.some((other) =>
      bounds.left < other.right && bounds.right > other.left &&
      bounds.top < other.bottom && bounds.bottom > other.top
    )) return;
    state.labelBounds.push(bounds);
    context.fillStyle = 'rgba(247, 247, 245, 0.92)';
    context.fillRect(x, y, width + padding * 2, fontSize + padding * 2);
    context.fillStyle = '#202124';
    context.textAlign = 'center';
    context.textBaseline = 'top';
    context.fillText(node.title, node.x, y + padding);
  }

  function refreshStyles() {
    if (!state.graphView) return;
    state.graphView
      .nodeColor(nodeColor)
      .nodeCanvasObject(drawNodeLabel)
      .linkColor(linkColor)
      .linkWidth(linkWidth)
      .linkDirectionalArrowColor(linkColor);
  }

  function handleNodeHover(node) {
    state.hovered = node ? node.id : null;
    refreshStyles();
  }

  function renderOverview(hubs) {
    if (state.current) return;
    panel.innerHTML = '<div class="note-empty"><h1>Workspace graph</h1><p></p><h2>Most connected</h2><ol class="hub-list"></ol></div>';
    panel.querySelector('p').textContent = `${state.graph.nodes.length} notes and ${state.graph.edges.length} connections`;
    const list = panel.querySelector('.hub-list');
    hubs.forEach((node) => {
      const item = document.createElement('li');
      const button = document.createElement('button');
      const count = document.createElement('span');
      button.type = 'button';
      button.textContent = node.title;
      count.textContent = `${node.degree} connections`;
      button.addEventListener('click', () => {
        state.graphView.centerAt(node.x, node.y, 0);
        state.graphView.zoom(Math.max(1.4, state.graphView.zoom()), 0);
        selectNode(node);
      });
      item.append(button, count);
      list.append(item);
    });
  }

  function scrollPreviewToFragment(fragment) {
    if (!fragment) return;
    let id;
    try {
      id = decodeURIComponent(fragment.replace(/^#/, ''));
    } catch (_) {
      id = fragment.replace(/^#/, '');
    }
    const target = panel.querySelector('.note-content')?.querySelector(`#${CSS.escape(id)}`);
    if (target) requestAnimationFrame(() => target.scrollIntoView({ block: 'start' }));
  }

  async function selectNode(node, { fragment = '' } = {}) {
    state.current = node.id;
    state.selectedGraph = node.id;
    updateUrl();
    refreshStyles();
    if (node.unresolved) {
      panel.innerHTML = '<div class="note-empty"><h1></h1><p>Unresolved target</p></div>';
      panel.querySelector('h1').textContent = node.title;
      return;
    }
    panel.innerHTML = '<div class="note-empty"><p>Loading note...</p></div>';
    try {
      const response = await fetch(`${config.noteApiBase}${encodeURIComponent(node.id)}${config.noteApiSuffix}`, { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      const note = await response.json();
      panel.innerHTML = `
        <article class="document">
          <header class="document-header"><p class="document-path"></p><h1></h1></header>
          <div class="note-actions"><a class="command full-note">Open full note</a><button class="local-note" type="button">Show local graph</button></div>
          <div class="note-content"></div>
        </article>`;
      panel.querySelector('.document-path').textContent = note.path;
      panel.querySelector('h1').textContent = note.title;
      panel.querySelector('.note-content').innerHTML = note.html;
      panel.querySelector('.full-note').href = `${config.notePageBase}${encodeURIComponent(node.id)}${config.notePageSuffix}`;
      panel.querySelector('.local-note').addEventListener('click', () => { setLocal(true); });
      scrollPreviewToFragment(fragment);
    } catch (error) {
      panel.innerHTML = '<div class="note-empty"><h1>Cannot render note</h1><p></p></div>';
      panel.querySelector('p').textContent = String(error);
    }
  }

  async function selectDocument(documentId, fragment) {
    let node = state.renderedNodes.find((candidate) => candidate.id === documentId);
    if (!node && state.query.graph) {
      state.query.graph = '';
      search.value = '';
      await loadGraph();
      node = state.renderedNodes.find((candidate) => candidate.id === documentId);
    }
    if (!node) {
      state.current = documentId;
      state.local = true;
      await setLocal(true);
      node = state.renderedNodes.find((candidate) => candidate.id === documentId);
    }
    if (!node) return;
    state.graphView.centerAt(node.x, node.y, 300);
    selectNode(node, { fragment });
  }

  function setLocal(local) {
    state.local = local && Boolean(state.current);
    globalMode.classList.toggle('active', !state.local);
    localMode.classList.toggle('active', state.local);
    localMode.disabled = !state.current;
    updateUrl();
    return loadGraph();
  }

  async function refreshWorkspace() {
    const current = state.current;
    await Promise.all([loadGraph(), loadTasks(), loadEvents()]);
    if (!current || state.current !== current) return;
    const node = state.graph?.nodes.find((candidate) => candidate.id === current);
    if (node) {
      selectNode(node);
      return;
    }
    panel.innerHTML = '<div class="note-empty"><h1>Note unavailable</h1><p>This note is no longer in the workspace.</p></div>';
  }

  function showView(view, { historyMode = null, load = true } = {}) {
    state.view = view;
    const graphActive = view === 'graph';
    const tasksActive = view === 'tasks';
    const agendaActive = view === 'agenda';
    graphWorkspace.hidden = !graphActive;
    taskWorkspace.hidden = !tasksActive;
    agendaWorkspace.hidden = !agendaActive;
    document.querySelectorAll('.graph-control, .graph-filters').forEach((element) => { element.hidden = !graphActive; });
    document.querySelectorAll('.task-control, .task-filters').forEach((element) => { element.hidden = !tasksActive; });
    graphViewButton.classList.toggle('active', graphActive);
    tasksViewButton.classList.toggle('active', tasksActive);
    agendaViewButton.classList.toggle('active', agendaActive);
    graphViewButton.setAttribute('aria-selected', String(graphActive));
    tasksViewButton.setAttribute('aria-selected', String(tasksActive));
    agendaViewButton.setAttribute('aria-selected', String(agendaActive));
    if (graphActive) {
      state.graphView?.width(graphElement.clientWidth).height(graphElement.clientHeight);
    }
    if (!agendaActive) syncQueryControls(view);
    if (historyMode) updateUrl(historyMode);
    if (load) {
      if (graphActive) loadGraph();
      else if (tasksActive) loadTasks();
      else loadEvents();
    }
  }

  async function loadTasks() {
    try {
      const result = await executeQuery('tasks');
      state.tasks = result.tasks;
      setQueryError('tasks', null);
      renderTasks();
    } catch (error) {
      setQueryError('tasks', error);
      if (!state.tasks) taskSummary.textContent = 'Tasks unavailable';
    }
  }

  async function loadEvents() {
    if (!config.eventSnapshotUrl) return;
    try {
      const [response, tasks] = await Promise.all([
        fetch(config.eventSnapshotUrl, { cache: 'no-store' }),
        loadAgendaTasks(),
      ]);
      if (!response.ok) throw new Error(await response.text());
      state.events = await response.json();
      state.tasks = tasks.tasks;
      renderEvents();
    } catch (error) {
      if (!state.events) eventPanel.innerHTML = '<div class="note-empty"><h1>Agenda unavailable</h1></div>';
      notify(String(error), true);
    }
  }

  async function loadAgendaTasks() {
    const response = await fetch(config.queryUrl, {
      method: 'POST',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(readyTaskQueryRequest()),
    });
    if (!response.ok) throw new Error(await response.text());
    return response.json();
  }

  function renderEvents() {
    if (!state.events) return;
    eventList.replaceChildren();
    eventEmpty.hidden = state.events.events.length > 0 || Boolean(state.tasks?.tasks?.length);
    let previousDate = null;
    state.events.events.forEach((event) => {
      const parsedStart = event.start ? new Date(event.start) : null;
      const date = parsedStart && !Number.isNaN(parsedStart.getTime())
        ? parsedStart.toLocaleDateString()
        : 'Invalid date';
      if (date !== previousDate) {
        const heading = document.createElement('div');
        heading.className = 'task-document-group';
        heading.textContent = date;
        eventList.append(heading);
        previousDate = date;
      }
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'event-row';
      button.classList.toggle('selected', event.key === state.selectedEvent);
      const time = document.createElement('time');
      time.textContent = eventTimeLabel(event);
      const identity = document.createElement('span');
      identity.className = 'task-identity';
      const title = document.createElement('strong');
      title.textContent = event.title || '(untitled event)';
      const source = document.createElement('small');
      source.textContent = event.path;
      identity.append(title, source);
      button.append(time, identity);
      button.addEventListener('click', () => selectEvent(event));
      eventList.append(button);
    });
    if (state.tasks?.tasks?.length) {
      const heading = document.createElement('div');
      heading.className = 'task-document-group';
      heading.textContent = 'Ready tasks';
      eventList.append(heading);
      state.tasks.tasks.forEach((task) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'event-row agenda-task-row';
        const stateLabel = document.createElement('span');
        stateLabel.className = 'task-state state-ready';
        stateLabel.textContent = 'Ready';
        const identity = document.createElement('span');
        identity.className = 'task-identity';
        const title = document.createElement('strong');
        title.textContent = task.title || '(untitled task)';
        const source = document.createElement('small');
        source.textContent = task.id ? `${task.path}#${task.id}` : task.path;
        identity.append(title, source);
        button.append(stateLabel, identity);
        button.addEventListener('click', () => {
          state.selectedTask = task.key;
          showView('tasks', { historyMode: 'push' });
          renderTasks();
        });
        eventList.append(button);
      });
    }
    const selected = state.events.events.find((event) => event.key === state.selectedEvent);
    if (selected) renderEventDetail(selected);
    else renderNewEventPrompt();
  }

  function eventTimeLabel(event) {
    if (!event.start) return 'Invalid';
    const start = new Date(event.start);
    const startLabel = start.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    if (!event.end) return startLabel;
    const end = new Date(event.end);
    return `${startLabel}-${end.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
  }

  function selectEvent(event) {
    state.selectedEvent = event.key;
    updateUrl();
    renderEvents();
  }

  function renderNewEventPrompt() {
    eventPanel.innerHTML = '<div class="note-empty"><h1>Workspace agenda</h1><p>Select an event or create a new one.</p><button id="new-event" type="button">New event</button></div>';
    eventPanel.querySelector('#new-event').disabled = !config.eventMutations || !state.events.documents.length;
    eventPanel.querySelector('#new-event').addEventListener('click', () => renderEventForm());
  }

  function localDateTimeValue(value) {
    if (!value) return '';
    const date = new Date(value);
    const local = new Date(date.getTime() - date.getTimezoneOffset() * 60000);
    return local.toISOString().slice(0, 16);
  }

  function renderEventDetail(event) {
    eventPanel.innerHTML = `
      <article class="event-detail">
        <header><p class="document-path"></p><h1></h1></header>
        <div class="task-actions"><button class="edit-event" type="button">Edit</button><button class="delete-event" type="button">Delete</button><button class="new-event" type="button">New event</button></div>
        <dl class="task-fields"></dl>
        <p class="event-details"></p>
      </article>`;
    eventPanel.querySelector('.document-path').textContent = event.path;
    eventPanel.querySelector('h1').textContent = event.title || '(untitled event)';
    const fields = eventPanel.querySelector('.task-fields');
    addTaskField(fields, 'Start', event.start);
    addTaskField(fields, 'End', event.end || 'Point event');
    addTaskField(fields, 'Tasks', event.tasks);
    addTaskField(fields, 'UID', event.uid);
    eventPanel.querySelector('.event-details').textContent = event.details;
    eventPanel.querySelector('.edit-event').disabled = !config.eventMutations || state.pendingEvent;
    eventPanel.querySelector('.delete-event').disabled = !config.eventMutations || state.pendingEvent;
    eventPanel.querySelector('.new-event').disabled = !config.eventMutations || !state.events.documents.length;
    eventPanel.querySelector('.edit-event').addEventListener('click', () => renderEventForm(event));
    eventPanel.querySelector('.delete-event').addEventListener('click', () => mutateEvent('delete', event));
    eventPanel.querySelector('.new-event').addEventListener('click', () => renderEventForm());
  }

  function renderEventForm(event = null) {
    const documents = state.events.documents.map((document) => `<option value="${document.id}"></option>`).join('');
    eventPanel.innerHTML = `
      <form class="event-form">
        <h1>${event ? 'Edit event' : 'New event'}</h1>
        <label>Document<select name="document">${documents}</select></label>
        <label>Title<input name="title" type="text" required></label>
        <label>Start<input name="start" type="datetime-local" required></label>
        <label>End<input name="end" type="datetime-local"></label>
        <label>Task references<textarea name="tasks" rows="4" placeholder="One reference per line"></textarea></label>
        <div class="task-actions"><button type="submit">Save</button><button class="cancel-event" type="button">Cancel</button></div>
      </form>`;
    const form = eventPanel.querySelector('form');
    const select = form.elements.document;
    state.events.documents.forEach((document, index) => {
      select.options[index].textContent = document.path;
    });
    if (event) {
      select.value = event.documentId;
      select.disabled = true;
      form.elements.title.value = event.title;
      form.elements.start.value = localDateTimeValue(event.start);
      form.elements.end.value = localDateTimeValue(event.end);
      form.elements.tasks.value = event.tasks.join('\n');
    }
    form.addEventListener('submit', (submit) => {
      submit.preventDefault();
      mutateEvent(event ? 'update' : 'create', event, form);
    });
    form.querySelector('.cancel-event').addEventListener('click', () => event ? renderEventDetail(event) : renderNewEventPrompt());
  }

  async function mutateEvent(action, event = null, form = null) {
    if (state.pendingEvent) return;
    const document = event
      ? state.events.documents.find((item) => item.id === event.documentId)
      : state.events.documents.find((item) => item.id === form.elements.document.value);
    if (!document) return;
    const fields = form ? {
      title: form.elements.title.value,
      start: new Date(form.elements.start.value).toISOString(),
      end: form.elements.end.value ? new Date(form.elements.end.value).toISOString() : null,
      tasks: form.elements.tasks.value.split('\n').map((value) => value.trim()).filter(Boolean),
    } : null;
    state.pendingEvent = true;
    try {
      const response = await fetch(`${config.eventActionBase}${encodeURIComponent(document.id)}/${action}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          revision: event ? event.revision : document.revision,
          locator: event?.locator || null,
          event: fields,
        }),
      });
      const body = await response.text();
      if (!response.ok) throw new Error(body || `HTTP ${response.status}`);
      state.events = JSON.parse(body);
      if (action === 'delete') state.selectedEvent = null;
      if (action === 'create') {
        const created = state.events.events.find((candidate) => (
          candidate.documentId === document.id
          && candidate.title === fields.title
          && candidate.start === fields.start
        ));
        state.selectedEvent = created?.key || null;
      }
      renderEvents();
      updateUrl();
      notify(`Event ${action}d.`);
    } catch (error) {
      await loadEvents();
      notify(String(error), true);
    } finally {
      state.pendingEvent = false;
    }
  }

  function taskStateLabel(task) {
    return task.state.charAt(0).toUpperCase() + task.state.slice(1);
  }

  function renderTasks() {
    if (!state.tasks) return;
    const tasks = state.tasks.tasks;
    taskList.replaceChildren();
    taskEmpty.hidden = tasks.length > 0;
    taskSummary.textContent = `${tasks.length} tasks${state.tasks.complete ? '' : ' (truncated)'}`;
    let previousPath = null;
    tasks.forEach((task) => {
      if (task.path !== previousPath) {
        const heading = document.createElement('div');
        heading.className = 'task-document-group';
        heading.textContent = task.path;
        taskList.append(heading);
        previousPath = task.path;
      }
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'task-row';
      button.classList.toggle('selected', task.key === state.selectedTask);
      button.style.setProperty('--task-depth', Math.min(task.depth, 5));
      const stateLabel = document.createElement('span');
      stateLabel.className = `task-state state-${task.state}${task.blocked ? ' blocked' : ''}`;
      stateLabel.textContent = taskStateLabel(task);
      const identity = document.createElement('span');
      identity.className = 'task-identity';
      const title = document.createElement('strong');
      title.textContent = task.title || '(untitled task)';
      const source = document.createElement('small');
      source.textContent = task.id ? `${task.path}#${task.id}` : task.path;
      identity.append(title, source);
      const due = document.createElement('time');
      due.textContent = task.due ? task.due.slice(0, 10) : 'No due date';
      if (task.due) due.dateTime = task.due;
      button.append(stateLabel, identity, due);
      button.addEventListener('click', () => selectTask(task));
      taskList.append(button);
    });
    if (state.selectedTask) {
      const selected = taskByKey(state.tasks, state.selectedTask);
      if (selected) renderTaskDetail(selected);
      else clearTaskDetail('Task unavailable', 'This task is no longer in the workspace.');
    }
  }

  function selectTask(task) {
    state.selectedTask = task.key;
    updateUrl();
    renderTasks();
    renderTaskDetail(task);
  }

  function clearTaskDetail(title, message) {
    taskPanel.innerHTML = '<div class="note-empty"><h1></h1><p></p></div>';
    taskPanel.querySelector('h1').textContent = title;
    taskPanel.querySelector('p').textContent = message;
  }

  function addTaskField(list, label, value) {
    if (value === null || value === undefined || value === '' || (Array.isArray(value) && value.length === 0)) return;
    const term = document.createElement('dt');
    const detail = document.createElement('dd');
    term.textContent = label;
    detail.textContent = Array.isArray(value) ? value.join(', ') : value;
    list.append(term, detail);
  }

  function renderTaskDetail(task) {
    taskPanel.innerHTML = `
      <article class="task-detail">
        <header><p class="document-path"></p><h1></h1><span class="task-detail-state"></span></header>
        <div class="task-actions"><button class="complete-task" type="button">Complete</button><button class="cancel-task" type="button">Cancel</button><button class="open-note" type="button">Open note</button></div>
        <dl class="task-fields"></dl>
        <section class="task-children" hidden><h2>Child tasks</h2><div></div></section>
      </article>`;
    taskPanel.querySelector('.document-path').textContent = task.id ? `${task.path}#${task.id}` : task.path;
    taskPanel.querySelector('h1').textContent = task.title || '(untitled task)';
    const stateLabel = taskPanel.querySelector('.task-detail-state');
    stateLabel.textContent = taskStateLabel(task);
    stateLabel.className = `task-detail-state state-${task.state}`;
    const fields = taskPanel.querySelector('.task-fields');
    addTaskField(fields, 'Created', task.created);
    addTaskField(fields, 'Due', task.due);
    addTaskField(fields, 'Priority', task.priority);
    addTaskField(fields, 'Wait', task.wait);
    addTaskField(fields, 'Done', task.done);
    addTaskField(fields, 'Canceled', task.canceled);
    addTaskField(fields, 'Recurrence', task.recur);
    addTaskField(fields, 'Dependencies', task.depends);
    addTaskField(fields, 'Waiting for', task.waitReasons);
    const mutable = Boolean(config.taskMutations && task.locator && ['ready', 'waiting'].includes(task.state));
    const pending = state.pendingTask === task.key;
    taskPanel.querySelector('.complete-task').disabled = !mutable || task.blocked || pending;
    taskPanel.querySelector('.cancel-task').disabled = !mutable || pending;
    taskPanel.querySelector('.complete-task').addEventListener('click', () => updateTask(task, 'complete'));
    taskPanel.querySelector('.cancel-task').addEventListener('click', () => updateTask(task, 'cancel'));
    taskPanel.querySelector('.open-note').addEventListener('click', () => {
      showView('graph', { historyMode: 'push' });
      selectDocument(task.documentId, '');
    });
    const children = (state.tasks.allTasks || state.tasks.tasks)
      .filter((candidate) => candidate.parentKey === task.key);
    if (children.length) {
      const section = taskPanel.querySelector('.task-children');
      const list = section.querySelector('div');
      section.hidden = false;
      children.forEach((child) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.textContent = `${taskStateLabel(child)}  ${child.title || '(untitled task)'}`;
        button.addEventListener('click', () => selectTask(child));
        list.append(button);
      });
    }
  }

  async function updateTask(task, action) {
    const verb = action === 'complete' ? 'Complete' : 'Cancel';
    if (state.pendingTask) return;
    const url = `${config.taskActionBase}${encodeURIComponent(task.documentId)}/${action}`;
    state.pendingTask = task.key;
    renderTaskDetail(task);
    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: task.revision, locator: task.locator }),
      });
      const body = await response.text();
      if (!response.ok) throw new Error(body || `HTTP ${response.status}`);
      await loadTasks();
      notify(`${verb}d task.`);
    } catch (error) {
      await loadTasks();
      notify(String(error), true);
    } finally {
      state.pendingTask = null;
      const selected = taskByKey(state.tasks, state.selectedTask);
      if (selected) renderTaskDetail(selected);
    }
  }

  search.addEventListener('input', () => {
    state.query.graph = search.value;
    updateUrl();
    clearTimeout(state.searchTimer);
    state.searchTimer = setTimeout(loadGraph, 140);
  });
  taskSearch.addEventListener('input', () => {
    state.query.tasks = taskSearch.value;
    updateUrl();
    clearTimeout(state.searchTimer);
    state.searchTimer = setTimeout(loadTasks, 140);
  });
  graphViewButton.addEventListener('click', () => showView('graph', { historyMode: 'push' }));
  tasksViewButton.addEventListener('click', () => showView('tasks', { historyMode: 'push' }));
  agendaViewButton.addEventListener('click', () => showView('agenda', { historyMode: 'push' }));
  document.querySelectorAll('.graph-filters .edge-options input[value]').forEach((input) => input.addEventListener('change', () => {
    updateUrl();
    loadGraph();
  }));
  document.querySelectorAll('.filters').forEach((container) => {
    const view = container.classList.contains('graph-filters') ? 'graph' : 'tasks';
    const menu = container.querySelector('.preset-menu');
    container.querySelector('.preset-add').addEventListener('click', () => { menu.hidden = !menu.hidden; });
    const sort = container.querySelector('.query-sort');
    if (sort) sort.addEventListener('change', (event) => {
      state.sort[view] = [event.target.value];
      state.sortsSpecified[view] = true;
      updateUrl(); runViewQuery(view);
    });
  });
  document.querySelector('.task-sort-add').addEventListener('change', (event) => {
    if (event.target.value) changeTaskSort(addSortKey(state.sort.tasks, event.target.value));
    event.target.value = '';
  });
  document.querySelector('.task-sort-reset').addEventListener('click', () => changeTaskSort(['priority']));
  allLabels.addEventListener('change', refreshStyles);
  depth.addEventListener('change', () => { updateUrl(); loadGraph(); });
  direction.addEventListener('change', () => { updateUrl(); loadGraph(); });
  globalMode.addEventListener('click', () => setLocal(false));
  localMode.addEventListener('click', () => setLocal(true));
  document.getElementById('fit').addEventListener('click', () => state.graphView && state.graphView.zoomToFit(0, 48));
  panel.addEventListener('click', (event) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest('.note-content a[data-plumb-document]');
    if (!link || link.download || (link.target && link.target !== '_self')) return;
    event.preventDefault();
    selectDocument(link.dataset.plumbDocument, new URL(link.href, location.href).hash);
  });

  if (config.eventsUrl && window.EventSource) {
    const events = new EventSource(config.eventsUrl);
    events.addEventListener('workspace', refreshWorkspace);
  }
  window.addEventListener('popstate', () => {
    readUrlState();
    showView(state.view, { load: true });
  });

  readUrlState();
  loadPresetRegistry().then(() => {
    showView(state.view, { load: false });
    const initialLoad = runViewQuery(state.view);
    if (state.view === 'graph' && (state.selectedGraph || state.current)) {
      initialLoad.then(() => {
        const id = state.selectedGraph || state.current;
        const node = state.renderedNodes.find((candidate) => candidate.id === id);
        if (node) selectNode(node);
      });
    }
  }).catch((error) => notify(`Cannot load query presets: ${error}`, true));
})();
