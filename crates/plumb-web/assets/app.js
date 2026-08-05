import {
  addPreset,
  addPresetGroup,
  addSortKey,
  initialPresets,
  normalizeSortKeys,
  moveSortKey,
  readQueryParameters,
  taskByKey,
  togglePresetValue,
  viewFromPath,
  writeQueryParameters,
} from './query-state.js';
import { currentTimeInsertionIndex, localDateKey } from './agenda-state.js';
import { EDITABLE_TASK_PROPERTIES, missingTaskProperties } from './task-ui.js';

(function () {
  'use strict';

  const config = JSON.parse(document.body.dataset.plumbConfig);
  const initialView = viewFromPath(location.pathname);
  const state = {
    graph: null,
    graphView: null,
    graphConfigured: false,
    graphScope: null,
    graphFitRevision: 0,
    graphLoadRevision: 0,
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
    presets: { graph: [], tasks: ['ready', 'blocked'], agenda: [] },
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
    agendaPositioned: false,
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
  const newTaskButton = document.getElementById('new-task');
  const notification = document.getElementById('notification');
  const agendaWorkspace = document.querySelector('.agenda-workspace');
  const eventList = document.getElementById('event-list');
  const agendaNowButton = document.getElementById('agenda-now');
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
    const loadRevision = ++state.graphLoadRevision;
    const graphScope = state.local ? `local\u0000${state.current}` : 'workspace';
    try {
      const result = await executeQuery('graph');
      if (loadRevision !== state.graphLoadRevision) return;
      state.graph = result.graph;
      setQueryError('graph', null);
      renderGraph(graphScope);
    } catch (error) {
      if (loadRevision !== state.graphLoadRevision) return;
      setQueryError('graph', error);
      if (!state.graph) summary.textContent = 'Graph unavailable';
    }
  }

  function renderGraph(graphScope) {
    const scopeChanged = graphScope !== state.graphScope;
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
    state.graphScope = graphScope;
    ensureGraphView();
    if (!state.graphConfigured || topologyChanged || scopeChanged) {
      configureGraphView(nodes, edges, !state.graphConfigured, scopeChanged);
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

  function configureGraphView(nodes, edges, initial, fitScope) {
    const warmup = initial ? (nodes.length > 500 ? 100 : 260) : 0;
    const fitRevision = ++state.graphFitRevision;
    let shouldFitScope = fitScope;
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
      .onEngineStop(() => {
        if (!shouldFitScope || fitRevision !== state.graphFitRevision) return;
        shouldFitScope = false;
        state.graphView.zoomToFit(250, 48);
      })
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
      const response = await fetch(config.eventSnapshotUrl, { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      state.events = await response.json();
      const positionNow = state.view === 'agenda' && !state.agendaPositioned;
      if (positionNow) state.agendaPositioned = true;
      renderEvents({ positionNow });
    } catch (error) {
      if (!state.events) eventPanel.innerHTML = '<div class="note-empty"><h1>Agenda unavailable</h1></div>';
      notify(String(error), true);
    }
  }

  function renderEvents({ positionNow = false } = {}) {
    if (!state.events) return;
    eventList.replaceChildren();
    eventEmpty.hidden = state.events.events.length > 0;
    const now = new Date();
    const nowIndex = currentTimeInsertionIndex(state.events.events, now);
    const todayKey = localDateKey(now);
    let previousDateKey = null;

    function appendDateHeading(date, dateKey) {
      const heading = document.createElement('div');
      heading.className = 'task-document-group agenda-date-heading';
      heading.textContent = date.toLocaleDateString();
      eventList.append(heading);
      previousDateKey = dateKey;
    }

    function appendCurrentTime() {
      if (previousDateKey !== todayKey) appendDateHeading(now, todayKey);
      const marker = document.createElement('div');
      marker.id = 'agenda-current-time';
      marker.className = 'agenda-current-time';
      const label = document.createElement('time');
      label.dateTime = now.toISOString();
      label.textContent = now.toLocaleTimeString([], EVENT_TIME_OPTIONS);
      const rule = document.createElement('span');
      rule.setAttribute('aria-hidden', 'true');
      marker.append(label, rule);
      eventList.append(marker);
    }

    state.events.events.forEach((event, index) => {
      if (index === nowIndex) appendCurrentTime();
      const eventTime = event.at || event.start;
      const parsedStart = eventTime ? new Date(eventTime) : null;
      const dateKey = parsedStart ? localDateKey(parsedStart) : null;
      if (dateKey !== previousDateKey) {
        const heading = document.createElement('div');
        heading.className = 'task-document-group agenda-date-heading';
        heading.textContent = dateKey ? parsedStart.toLocaleDateString() : 'Invalid date';
        eventList.append(heading);
        previousDateKey = dateKey;
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
    if (nowIndex === state.events.events.length) appendCurrentTime();
    const selected = state.events.events.find((event) => event.key === state.selectedEvent);
    if (selected) renderEventDetail(selected);
    else renderNewEventPrompt();
    if (positionNow) requestAnimationFrame(() => scrollAgendaToNow());
  }

  const EVENT_TIME_OPTIONS = { hour: '2-digit', minute: '2-digit', hour12: false };

  function scrollAgendaToNow(behavior = 'auto') {
    document.getElementById('agenda-current-time')?.scrollIntoView({ behavior, block: 'center' });
  }

  function eventTimeLabel(event) {
    const value = event.at || event.start;
    if (!value) return 'Invalid';
    const start = new Date(value);
    const startLabel = start.toLocaleTimeString([], EVENT_TIME_OPTIONS);
    if (event.at) return startLabel;
    if (!event.end) return `${startLabel}-running`;
    const end = new Date(event.end);
    return `${startLabel}-${end.toLocaleTimeString([], EVENT_TIME_OPTIONS)}`;
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
    addTaskField(fields, 'At', event.at);
    addTaskField(fields, 'Start', event.start);
    addTaskField(fields, 'End', event.end || (event.start ? 'Running' : null));
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
        <label>Time shape<select name="timeShape"><option value="point">Point</option><option value="interval">Interval</option></select></label>
        <label class="event-at">At<input name="at" type="datetime-local"></label>
        <label class="event-start" hidden>Start<input name="start" type="datetime-local"></label>
        <label class="event-end" hidden>End<input name="end" type="datetime-local"></label>
        <label>Task references<textarea name="tasks" rows="4" placeholder="One reference per line"></textarea></label>
        <div class="task-actions"><button type="submit">Save</button><button class="cancel-event" type="button">Cancel</button></div>
      </form>`;
    const form = eventPanel.querySelector('form');
    const select = form.elements.document;
    state.events.documents.forEach((document, index) => {
      select.options[index].textContent = document.path;
    });
    const setTimeShape = (shape) => {
      const point = shape === 'point';
      form.querySelector('.event-at').hidden = !point;
      form.querySelector('.event-start').hidden = point;
      form.querySelector('.event-end').hidden = point;
      form.elements.at.required = point;
      form.elements.start.required = !point;
    };
    form.elements.timeShape.addEventListener('change', () => setTimeShape(form.elements.timeShape.value));
    if (event) {
      select.value = event.documentId;
      select.disabled = true;
      form.elements.title.value = event.title;
      form.elements.timeShape.value = event.at ? 'point' : 'interval';
      form.elements.at.value = localDateTimeValue(event.at);
      form.elements.start.value = localDateTimeValue(event.start);
      form.elements.end.value = localDateTimeValue(event.end);
      form.elements.tasks.value = event.tasks.join('\n');
    }
    setTimeShape(form.elements.timeShape.value);
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
      at: form.elements.timeShape.value === 'point' ? new Date(form.elements.at.value).toISOString() : null,
      start: form.elements.timeShape.value === 'interval' ? new Date(form.elements.start.value).toISOString() : null,
      end: form.elements.timeShape.value === 'interval' && form.elements.end.value ? new Date(form.elements.end.value).toISOString() : null,
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
          && candidate.at === fields.at
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
    newTaskButton.disabled = !config.taskMutations || !state.tasks.documents?.length || Boolean(state.pendingTask);
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

  function addTaskField(list, label, value, { editable = false, property = null, task = null } = {}) {
    if (value === null || value === undefined || value === '' || (Array.isArray(value) && value.length === 0)) return;
    const term = document.createElement('dt');
    const detail = document.createElement('dd');
    term.textContent = label;
    if (editable) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'task-property-value';
      button.dataset.property = property;
      button.textContent = Array.isArray(value) ? value.join(', ') : value;
      button.title = `Edit ${label}`;
      button.addEventListener('click', () => renderTaskPropertyEditor(task, property));
      detail.append(button);
    } else {
      detail.textContent = Array.isArray(value) ? value.join(', ') : value;
    }
    list.append(term, detail);
  }

  function renderTaskDetail(task) {
    taskPanel.innerHTML = `
      <article class="task-detail">
        <header><p class="document-path"></p><h1></h1><span class="task-detail-state"></span></header>
        <div class="task-actions"><button class="complete-task" type="button">Complete</button><button class="cancel-task" type="button">Cancel</button><button class="edit-task" type="button">Edit</button><button class="open-note" type="button">Open note</button></div>
        <dl class="task-fields"></dl>
        <div class="task-property-actions"><button class="add-task-property" type="button">Add property</button><select class="task-property-picker" aria-label="Property to add" hidden></select></div>
        <section class="task-children" hidden><h2>Child tasks</h2><div></div></section>
      </article>`;
    taskPanel.querySelector('.document-path').textContent = task.id ? `${task.path}#${task.id}` : task.path;
    taskPanel.querySelector('h1').textContent = task.title || '(untitled task)';
    const stateLabel = taskPanel.querySelector('.task-detail-state');
    stateLabel.textContent = taskStateLabel(task);
    stateLabel.className = `task-detail-state state-${task.state}`;
    const fields = taskPanel.querySelector('.task-fields');
    addTaskField(fields, 'Created', task.created, { editable: true, property: 'created', task });
    addTaskField(fields, 'Due', task.due, { editable: true, property: 'due', task });
    addTaskField(fields, 'Priority', task.priority, { editable: true, property: 'priority', task });
    addTaskField(fields, 'Wait', task.wait, { editable: true, property: 'wait', task });
    addTaskField(fields, 'Done', task.done);
    addTaskField(fields, 'Canceled', task.canceled);
    addTaskField(fields, 'Recurrence', task.recur, { editable: true, property: 'recur', task });
    addTaskField(fields, 'Previous task', task.prev, { editable: true, property: 'prev', task });
    addTaskField(fields, 'Dependencies', task.depends, { editable: true, property: 'depends', task });
    addTaskField(fields, 'Waiting for', task.waitReasons);
    const mutable = Boolean(config.taskMutations && task.locator && ['ready', 'waiting'].includes(task.state));
    const pending = state.pendingTask === task.key;
    taskPanel.querySelector('.complete-task').disabled = !mutable || task.blocked || pending;
    taskPanel.querySelector('.cancel-task').disabled = !mutable || pending;
    taskPanel.querySelector('.edit-task').disabled = !config.taskMutations || pending;
    taskPanel.querySelector('.complete-task').addEventListener('click', () => updateTask(task, 'complete'));
    taskPanel.querySelector('.cancel-task').addEventListener('click', () => updateTask(task, 'cancel'));
    taskPanel.querySelector('.edit-task').addEventListener('click', () => renderTaskForm(task));
    taskPanel.querySelector('.open-note').addEventListener('click', () => {
      showView('graph', { historyMode: 'push' });
      selectDocument(task.documentId, '');
    });
    const missing = missingTaskProperties(task);
    const addProperty = taskPanel.querySelector('.add-task-property');
    const propertyPicker = taskPanel.querySelector('.task-property-picker');
    addProperty.disabled = !config.taskMutations || pending || missing.length === 0;
    missing.forEach(({ key, label }) => propertyPicker.add(new Option(label, key)));
    addProperty.addEventListener('click', () => {
      propertyPicker.hidden = false;
      addProperty.hidden = true;
      propertyPicker.focus();
    });
    propertyPicker.addEventListener('change', () => renderTaskPropertyEditor(task, propertyPicker.value));
    propertyPicker.addEventListener('blur', () => {
      if (!propertyPicker.value) {
        propertyPicker.hidden = true;
        addProperty.hidden = false;
      }
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

  function taskReferenceByIdentity(identity) {
    return (state.tasks.allTasks || state.tasks.tasks).find((candidate) => (
      candidate.id && `${candidate.path}#${candidate.id}` === identity
    ));
  }

  function renderTaskPropertyEditor(task, property) {
    const definition = EDITABLE_TASK_PROPERTIES.find((candidate) => candidate.key === property);
    if (!definition || state.pendingTask) return;
    const detail = taskPanel.querySelector(`.task-property-value[data-property="${property}"]`)?.closest('dd');
    const host = detail || taskPanel.querySelector('.task-property-actions');
    const form = document.createElement('form');
    form.className = 'task-property-editor';
    form.dataset.property = property;
    const label = document.createElement('label');
    label.textContent = definition.label;
    let control;
    if (['created', 'due', 'wait'].includes(property)) {
      control = document.createElement('input');
      control.type = 'datetime-local';
      control.value = localDateTimeValue(task[property]);
    } else if (property === 'priority') {
      control = document.createElement('input');
      control.type = 'number'; control.step = '1'; control.min = '-2147483648'; control.max = '2147483647';
      control.value = task.priority ?? '';
    } else if (property === 'recur') {
      control = document.createElement('select');
      [['', 'None'], ['P1D', 'Daily'], ['P1W', 'Weekly'], ['P1M', 'Monthly'], ['P1Y', 'Yearly']]
        .forEach(([value, text]) => control.add(new Option(text, value)));
      control.value = task.recur || '';
    } else {
      control = document.createElement('select');
      control.multiple = property === 'depends';
      if (!control.multiple) control.add(new Option('None', ''));
      (state.tasks.allTasks || state.tasks.tasks)
        .filter((candidate) => candidate.id && candidate.key !== task.key)
        .forEach((candidate) => control.add(new Option(taskOptionLabel(candidate), candidate.key)));
      if (property === 'prev') {
        const previous = taskReferenceByIdentity(task.prevOn);
        control.value = previous?.key || '';
      } else {
        const selected = new Set(task.dependsOn || []);
        Array.from(control.options).forEach((option) => {
          const candidate = taskByKey(state.tasks, option.value);
          option.selected = candidate ? selected.has(`${candidate.path}#${candidate.id}`) : false;
        });
      }
    }
    control.name = 'value';
    label.append(control);
    const error = document.createElement('span'); error.className = 'field-error'; error.setAttribute('role', 'alert');
    const actions = document.createElement('span'); actions.className = 'task-property-editor-actions';
    const save = document.createElement('button'); save.type = 'submit'; save.textContent = 'Save';
    const cancel = document.createElement('button'); cancel.type = 'button'; cancel.textContent = 'Cancel';
    actions.append(save, cancel); form.append(label, error, actions);
    host.replaceChildren(form);
    control.focus();
    cancel.addEventListener('click', () => renderTaskDetail(task));
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      mutateTaskProperty(task, property, control, form);
    });
  }

  function taskFields(task) {
    const previous = taskReferenceByIdentity(task.prevOn);
    return {
      title: task.title,
      created: task.created,
      due: task.due,
      wait: task.wait,
      recur: task.recur,
      prev: previous ? taskIdentity(previous) : null,
      depends: (task.dependsOn || []).map(taskReferenceByIdentity).filter(Boolean).map(taskIdentity),
      priority: task.priority,
    };
  }

  async function mutateTaskProperty(task, property, control, form) {
    if (state.pendingTask) return;
    const fields = taskFields(task);
    if (['created', 'due', 'wait'].includes(property)) fields[property] = formDate(control.value);
    else if (property === 'priority') fields.priority = control.value === '' ? null : Number(control.value);
    else if (property === 'recur') fields.recur = control.value || null;
    else if (property === 'prev') {
      const previous = taskByKey(state.tasks, control.value);
      fields.prev = previous ? taskIdentity(previous) : null;
    } else if (property === 'depends') {
      fields.depends = Array.from(control.selectedOptions)
        .map((option) => taskByKey(state.tasks, option.value)).filter(Boolean).map(taskIdentity);
    }
    if (fields.recur && !fields.due) {
      form.querySelector('.field-error').textContent = 'Recurring tasks require a due date.';
      return;
    }
    const listScroll = taskList.parentElement.scrollTop;
    const panelScroll = taskPanel.scrollTop;
    state.pendingTask = task.key;
    form.querySelectorAll('button, input, select').forEach((element) => { element.disabled = true; });
    try {
      const response = await fetch(`${config.taskActionBase}${encodeURIComponent(task.documentId)}/update`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: task.revision, locator: task.locator, task: fields, placement: null }),
      });
      const body = await response.text();
      if (!response.ok) throw new Error(body || `HTTP ${response.status}`);
      state.pendingTask = null;
      await loadTasks();
      const latest = taskByKey(state.tasks, task.key);
      if (latest) renderTaskDetail(latest);
      taskList.parentElement.scrollTop = listScroll;
      taskPanel.scrollTop = panelScroll;
      taskPanel.querySelector(`.task-property-value[data-property="${property}"]`)?.focus();
      notify(`${EDITABLE_TASK_PROPERTIES.find((candidate) => candidate.key === property).label} updated.`);
    } catch (error) {
      state.pendingTask = null;
      await loadTasks();
      const latest = taskByKey(state.tasks, task.key);
      if (latest) renderTaskPropertyEditor(latest, property);
      taskPanel.querySelector('.task-property-editor .field-error').textContent = String(error);
      notify(String(error), true);
    } finally {
      state.pendingTask = null;
    }
  }

  function taskIdentity(task) {
    return { documentId: task.documentId, locator: task.locator };
  }

  function taskOptionLabel(task) {
    return `${task.title || '(untitled)'} — ${task.path}${task.id ? `#${task.id}` : ''}`;
  }

  function renderTaskForm(task = null) {
    const documents = state.tasks.documents || [];
    if (!documents.length) return clearTaskDetail('Tasks unavailable', 'No valid task document is writable.');
    taskPanel.innerHTML = `
      <form class="task-form" novalidate>
        <h1>${task ? 'Edit task' : 'New task'}</h1>
        <label>Document<select name="document"></select><span class="field-error" data-field="document"></span></label>
        <label>Title<input name="title" type="text" required><span class="field-error" data-field="title"></span></label>
        <div class="task-form-grid">
          <label>Parent<select name="parent"></select></label>
          <label>Position<select name="after"></select></label>
        </div>
        <div class="task-form-grid">
          <label>Created<input name="created" type="datetime-local"></label>
          <label>Due<input name="due" type="datetime-local"><span class="field-error" data-field="due"></span></label>
          <label>Wait until<input name="wait" type="datetime-local"></label>
          <label>Priority<input name="priority" type="number" step="1" min="-2147483648" max="2147483647"></label>
        </div>
        <label>Recurrence<select name="recur"><option value="">None</option><option value="P1D">Daily</option><option value="P1W">Weekly</option><option value="P1M">Monthly</option><option value="P1Y">Yearly</option></select><span class="field-error" data-field="recur"></span></label>
        <label>Previous task<select name="prev"><option value="">None</option></select></label>
        <label>Dependencies<select name="depends" multiple size="6"></select><span class="field-error" data-field="depends"></span></label>
        <p class="form-error" role="alert"></p>
        <div class="task-actions"><button type="submit">Save</button><button class="cancel-task-form" type="button">Cancel</button></div>
      </form>`;
    const form = taskPanel.querySelector('form');
    documents.forEach((document) => form.elements.document.add(new Option(document.path, document.id)));
    if (task) { form.elements.document.value = task.documentId; form.elements.document.disabled = true; }
    const all = state.tasks.allTasks || state.tasks.tasks;
    const updatePlacement = () => {
      const documentId = form.elements.document.value;
      const parentValue = form.elements.parent.value;
      const isDescendant = (candidate) => {
        let current = candidate;
        while (current?.parentKey) {
          if (current.parentKey === task?.key) return true;
          current = taskByKey(state.tasks, current.parentKey);
        }
        return false;
      };
      const candidates = all.filter((candidate) => candidate.documentId === documentId && candidate.key !== task?.key && !isDescendant(candidate));
      form.elements.after.replaceChildren(new Option('End of list', ''));
      candidates.filter((candidate) => (candidate.parentKey || '') === parentValue).forEach((candidate) => {
        form.elements.after.add(new Option(`After ${candidate.title || '(untitled)'}`, candidate.key));
      });
    };
    const updateParents = () => {
      const documentId = form.elements.document.value;
      form.elements.parent.replaceChildren(new Option('Top level', ''));
      all.filter((candidate) => candidate.documentId === documentId && candidate.key !== task?.key).forEach((candidate) => {
        form.elements.parent.add(new Option(taskOptionLabel(candidate), candidate.key));
      });
      updatePlacement();
    };
    updateParents();
    const references = all.filter((candidate) => candidate.id && candidate.key !== task?.key);
    references.forEach((candidate) => {
      form.elements.prev.add(new Option(taskOptionLabel(candidate), candidate.key));
      form.elements.depends.add(new Option(taskOptionLabel(candidate), candidate.key));
    });
    if (task) {
      form.elements.title.value = task.title;
      form.elements.created.value = localDateTimeValue(task.created);
      form.elements.due.value = localDateTimeValue(task.due);
      form.elements.wait.value = localDateTimeValue(task.wait);
      form.elements.priority.value = task.priority ?? '';
      form.elements.recur.value = task.recur || '';
      form.elements.parent.value = task.parentKey || '';
      updatePlacement();
      const siblings = all.filter((candidate) => candidate.parentKey === task.parentKey && candidate.path === task.path);
      const ownIndex = siblings.findIndex((candidate) => candidate.key === task.key);
      const previous = ownIndex > 0 ? siblings[ownIndex - 1] : null;
      form.elements.after.value = previous?.key || '';
      form.dataset.originalParent = task.parentKey || '';
      form.dataset.originalAfter = previous?.key || '';
      const resolved = new Set(task.dependsOn || []);
      Array.from(form.elements.depends.options).forEach((option) => {
        const candidate = taskByKey(state.tasks, option.value);
        option.selected = resolved.has(`${candidate.path}#${candidate.id}`);
      });
      const prev = references.find((candidate) => (
        task.prevOn === `${candidate.path}#${candidate.id}`
      ));
      if (prev) form.elements.prev.value = prev.key;
    }
    form.elements.document.addEventListener('change', updateParents);
    form.elements.parent.addEventListener('change', updatePlacement);
    form.addEventListener('submit', (event) => { event.preventDefault(); mutateTaskForm(task, form); });
    form.querySelector('.cancel-task-form').addEventListener('click', () => task ? renderTaskDetail(task) : clearTaskDetail('Workspace tasks', 'Select a task to inspect its fields and dependencies.'));
  }

  function formDate(value) {
    return value ? new Date(value).toISOString() : null;
  }

  async function mutateTaskForm(task, form) {
    form.querySelectorAll('.field-error').forEach((field) => { field.textContent = ''; });
    form.querySelector('.form-error').textContent = '';
    if (!form.elements.title.value.trim()) {
      form.querySelector('[data-field="title"]').textContent = 'Title is required.';
      return;
    }
    if (form.elements.recur.value && !form.elements.due.value) {
      form.querySelector('[data-field="recur"]').textContent = 'Recurring tasks require a due date.';
      return;
    }
    const document = state.tasks.documents.find((item) => item.id === (task?.documentId || form.elements.document.value));
    if (!document || state.pendingTask) return;
    const identity = (key) => { const candidate = taskByKey(state.tasks, key); return candidate ? taskIdentity(candidate) : null; };
    const fields = {
      title: form.elements.title.value.trim(), created: formDate(form.elements.created.value),
      due: formDate(form.elements.due.value), wait: formDate(form.elements.wait.value),
      recur: form.elements.recur.value || null,
      prev: identity(form.elements.prev.value),
      depends: Array.from(form.elements.depends.selectedOptions).map((option) => identity(option.value)).filter(Boolean),
      priority: form.elements.priority.value === '' ? null : Number(form.elements.priority.value),
    };
    const parent = identity(form.elements.parent.value);
    const after = identity(form.elements.after.value);
    const changedPlacement = !task
      || form.elements.parent.value !== (form.dataset.originalParent || '')
      || form.elements.after.value !== (form.dataset.originalAfter || '');
    const placement = changedPlacement ? { parent: parent?.locator || null, after: after?.locator || null } : null;
    const action = task ? 'update' : 'create';
    state.pendingTask = task?.key || 'create';
    form.querySelectorAll('button, input, select').forEach((control) => { control.disabled = true; });
    try {
      const response = await fetch(`${config.taskActionBase}${encodeURIComponent(document.id)}/${action}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: task?.revision || document.revision, locator: task?.locator || null, task: fields, placement }),
      });
      const body = await response.text();
      if (!response.ok) throw new Error(body || `HTTP ${response.status}`);
      await loadTasks();
      const selected = (state.tasks.allTasks || state.tasks.tasks).find((candidate) => candidate.title === fields.title && candidate.documentId === document.id);
      state.selectedTask = selected?.key || task?.key || null;
      renderTasks(); updateUrl(); notify(`Task ${action}d.`);
    } catch (error) {
      await loadTasks();
      const latest = task ? taskByKey(state.tasks, task.key) : null;
      renderTaskForm(latest);
      const activeForm = taskPanel.querySelector('.task-form');
      const message = String(error);
      const field = message.includes('RFC 3339') ? 'due'
        : (message.includes('recur') ? 'recur'
          : (message.includes('reference') || message.includes('cycle') ? 'depends'
            : (message.includes('parent') || message.includes('position') ? 'document' : null)));
      (field ? activeForm.querySelector(`[data-field="${field}"]`) : activeForm.querySelector('.form-error')).textContent = message;
      notify(String(error), true);
    } finally {
      state.pendingTask = null;
      if (form.isConnected) {
        form.querySelectorAll('button, input, select').forEach((control) => { control.disabled = false; });
      } else {
        const selected = taskByKey(state.tasks, state.selectedTask);
        if (selected) renderTaskDetail(selected);
      }
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
  newTaskButton.addEventListener('click', () => renderTaskForm());
  agendaViewButton.addEventListener('click', () => showView('agenda', { historyMode: 'push' }));
  agendaNowButton.addEventListener('click', () => {
    renderEvents();
    requestAnimationFrame(() => scrollAgendaToNow('smooth'));
  });
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
