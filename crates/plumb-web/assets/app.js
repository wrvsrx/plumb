import { parse as parseCel } from './vendor/cel-js.min.js';
import {
  addPreset,
  readQueryParameters,
  taskByKey,
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
    presets: { graph: [], tasks: [] },
    query: { graph: '', tasks: '' },
    filter: { graph: '', tasks: '' },
    sort: { graph: 'source', tasks: 'source' },
    presetRegistry: { graph: [], tasks: [] },
    selectedGraph: null,
    pendingTask: null,
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
  const taskSearch = document.getElementById('task-search');
  const taskSummary = document.getElementById('task-summary');
  const taskList = document.getElementById('task-list');
  const taskEmpty = document.getElementById('task-empty');
  const taskPanel = document.getElementById('task-panel');
  const notification = document.getElementById('notification');
  let notificationTimer;

  function readUrlState() {
    const query = readQueryParameters(location.search);
    state.view = viewFromPath(location.pathname);
    state.presets[state.view] = query.presets;
    state.query[state.view] = query.query;
    state.filter[state.view] = query.filter;
    state.sort[state.view] = query.sort;
    state.current = query.current || config.current || null;
    state.local = Boolean(query.current);
    state.selectedTask = query.selected;
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
    return new URL(view === 'graph' ? config.graphRoute : config.tasksRoute, location.href);
  }

  function updateUrl(mode = 'replace') {
    const url = routeFor(state.view);
    url.search = writeQueryParameters({
      presets: state.presets[state.view],
      query: state.query[state.view],
      filter: state.filter[state.view],
      sort: state.sort[state.view],
      selected: state.view === 'graph' ? state.selectedGraph : state.selectedTask,
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
    if (config.presets) {
      state.presetRegistry = config.presets;
    } else {
      const response = await fetch(config.presetsUrl, { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      state.presetRegistry = await response.json();
    }
    renderPresetControls('graph');
    renderPresetControls('tasks');
  }

  function renderPresetControls(view) {
    const container = document.querySelector(`.${view === 'graph' ? 'graph' : 'task'}-filters`);
    const menu = container.querySelector('.preset-menu');
    const chips = container.querySelector('.preset-chips');
    const registry = state.presetRegistry[view] || [];
    menu.replaceChildren();
    registry.filter((preset) => !state.presets[view].includes(preset.id)).forEach((preset) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.role = 'menuitem';
      button.textContent = preset.label;
      button.title = preset.expression;
      button.addEventListener('click', () => {
        state.presets[view] = addPreset(state.presets[view], preset, registry);
        menu.hidden = true;
        renderPresetControls(view);
        updateUrl();
        runViewQuery(view);
      });
      menu.append(button);
    });
    chips.replaceChildren();
    state.presets[view].forEach((id) => {
      const preset = registry.find((item) => item.id === id);
      if (!preset) return;
      const chip = document.createElement('span');
      chip.className = 'preset-chip';
      chip.textContent = preset.label;
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.textContent = '×';
      remove.title = `Remove ${preset.label}`;
      remove.setAttribute('aria-label', `Remove ${preset.label}`);
      remove.addEventListener('click', () => {
        state.presets[view] = state.presets[view].filter((selected) => selected !== id);
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
    container.querySelector('.cel-filter input').value = state.filter[view];
    container.querySelector('.query-sort').value = state.sort[view];
    renderPresetControls(view);
  }

  function runViewQuery(view) {
    return view === 'graph' ? loadGraph() : loadTasks();
  }

  function selectedKinds() {
    return Array.from(document.querySelectorAll('.graph-filters .edge-options input[value]:checked')).map((input) => input.value);
  }

  function queryRequest(view) {
    return {
      view,
      query: state.query[view],
      presets: state.presets[view],
      filter: state.filter[view],
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
    if (!config.queryUrl) return executeStaticQuery(view);
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

  const staticData = {};

  async function staticSnapshot(kind) {
    if (!staticData[kind]) {
      const response = await fetch(kind === 'graph' ? config.graphUrl : config.tasksUrl, { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      staticData[kind] = await response.json();
    }
    return staticData[kind];
  }

  function fuzzyScore(candidate, query) {
    const source = Array.from(candidate.toLocaleLowerCase());
    const wanted = Array.from(query.toLocaleLowerCase());
    if (!wanted.length) return 0;
    let position = 0;
    let previous = -2;
    let score = 0;
    for (const character of wanted) {
      const relative = source.slice(position).indexOf(character);
      if (relative < 0) return null;
      const found = position + relative;
      score += 20 - Math.min(relative, 20);
      if (previous + 1 === found) score += 15;
      if (found === 0 || /[\s/_-]/.test(source[found - 1])) score += 10;
      previous = found;
      position = found + 1;
    }
    if (source.length === wanted.length && source.every((character, index) => character === wanted[index])) score += 1000;
    else if (wanted.every((character, index) => source[index] === character)) score += 500;
    return score;
  }

  function bestFuzzyScore(fields, query) {
    if (!query) return 0;
    const scores = fields.map((field) => fuzzyScore(field || '', query)).filter((score) => score !== null);
    return scores.length ? Math.max(...scores) : null;
  }

  function compileBrowserCel(source) {
    const evaluate = parseCel(source);
    return (facts) => {
      const value = evaluate(facts);
      if (typeof value !== 'boolean') throw new Error(`CEL query must return bool, got ${typeof value}`);
      return value;
    };
  }

  function queryPredicates(view) {
    const registry = state.presetRegistry[view] || [];
    const predicates = state.presets[view].map((id) => {
      const preset = registry.find((item) => item.id === id);
      if (!preset) { const error = new Error(`unknown query preset '${id}'`); error.source = `preset:${id}`; throw error; }
      try { return compileBrowserCel(preset.expression); } catch (failure) { failure.source = `preset:${id}`; throw failure; }
    });
    if (state.filter[view].trim()) {
      try { predicates.push(compileBrowserCel(state.filter[view])); } catch (failure) { failure.source = 'custom'; throw failure; }
    }
    return predicates;
  }

  function taskFacts(task) {
    const timestamp = (value) => value ? new Date(value) : null;
    return {
      path: task.path, id: task.id, title: task.title, created: timestamp(task.created), due: timestamp(task.due),
      wait: timestamp(task.wait), done: timestamp(task.done), canceled: timestamp(task.canceled),
      recur: task.recur, prev: task.prev, depends_on: task.dependsOn,
      directly_blocking: task.directlyBlocking, state: task.state, wait_reasons: task.waitReasons,
      blocked: task.blocked, actionable: task.actionable, now: new Date(),
    };
  }

  function staticTaskQuery(snapshot) {
    const predicates = queryPredicates('tasks');
    const roots = new Map();
    let root = null;
    snapshot.tasks.forEach((task) => {
      if (task.depth === 0 || !root || root.path !== task.path) root = task;
      roots.set(task.key, root);
    });
    const scores = new Map();
    const tasks = snapshot.tasks.filter((task) => {
      try { if (!predicates.every((predicate) => predicate(taskFacts(task)))) return false; }
      catch (failure) { failure.source ||= 'custom'; throw failure; }
      const score = bestFuzzyScore([task.title, task.id || '', task.path], state.query.tasks);
      if (score === null) return false;
      scores.set(task.key, score);
      return true;
    });
    const groups = new Map();
    tasks.forEach((task) => {
      const taskRoot = roots.get(task.key) || task;
      if (!groups.has(taskRoot.key)) groups.set(taskRoot.key, { root: taskRoot, tasks: [] });
      groups.get(taskRoot.key).tasks.push(task);
    });
    const grouped = Array.from(groups.values());
    grouped.sort((left, right) => {
      const source = left.root.path.localeCompare(right.root.path) || left.root.location.start - right.root.location.start;
      if (state.sort.tasks === 'due') return (left.root.due || '9999').localeCompare(right.root.due || '9999') || source;
      if (state.sort.tasks === 'relevance' && state.query.tasks) {
        const leftScore = Math.max(...left.tasks.map((task) => scores.get(task.key)));
        const rightScore = Math.max(...right.tasks.map((task) => scores.get(task.key)));
        return rightScore - leftScore || source;
      }
      return source;
    });
    return { ...snapshot, tasks: grouped.flatMap((group) => group.tasks), complete: true };
  }

  function staticGraphQuery(snapshot, tasks) {
    let nodes = snapshot.nodes.slice();
    let edges = snapshot.edges.filter((edge) => selectedKinds().includes(edge.kind));
    if (state.local && state.current && nodes.some((node) => node.id === state.current)) {
      const included = new Set([state.current]);
      let frontier = [state.current];
      for (let distance = 0; distance < Number(depth.value); distance += 1) {
        const next = [];
        frontier.forEach((id) => edges.forEach((edge) => {
          const source = endpointId(edge.source); const target = endpointId(edge.target);
          let neighbor = null;
          if (direction.value !== 'incoming' && source === id) neighbor = target;
          if (direction.value !== 'outgoing' && target === id) neighbor = source;
          if (neighbor && !included.has(neighbor)) { included.add(neighbor); next.push(neighbor); }
        }));
        frontier = next;
      }
      nodes = nodes.filter((node) => included.has(node.id));
      edges = edges.filter((edge) => included.has(endpointId(edge.source)) && included.has(endpointId(edge.target)));
    }
    const metrics = new Map(nodes.map((node) => [node.id, { degree: 0, incoming: 0, outgoing: 0, task_count: 0, open_task_count: 0 }]));
    edges.forEach((edge) => {
      const source = metrics.get(endpointId(edge.source)); const target = metrics.get(endpointId(edge.target));
      if (source) { source.degree += 1; source.outgoing += 1; }
      if (target) { target.degree += 1; target.incoming += 1; }
    });
    tasks.tasks.forEach((task) => {
      const metric = metrics.get(task.documentId);
      if (metric) { metric.task_count += 1; if (['ready', 'waiting'].includes(task.state)) metric.open_task_count += 1; }
    });
    const predicates = queryPredicates('graph');
    const scores = new Map();
    nodes = nodes.filter((node) => {
      const metric = metrics.get(node.id);
      const facts = {
        path: node.path, title: node.title, unresolved: node.unresolved,
        degree: BigInt(metric.degree), incoming: BigInt(metric.incoming), outgoing: BigInt(metric.outgoing),
        task_count: BigInt(metric.task_count), open_task_count: BigInt(metric.open_task_count),
      };
      try { if (!predicates.every((predicate) => predicate(facts))) return false; }
      catch (failure) { failure.source ||= 'custom'; throw failure; }
      const score = bestFuzzyScore([node.title, node.path || ''], state.query.graph);
      if (score === null) return false;
      scores.set(node.id, score); return true;
    });
    const visible = new Set(nodes.map((node) => node.id));
    edges = edges.filter((edge) => visible.has(endpointId(edge.source)) && visible.has(endpointId(edge.target)));
    nodes.sort((left, right) => {
      if (state.sort.graph === 'relevance' && state.query.graph) {
        const relevance = scores.get(right.id) - scores.get(left.id);
        if (relevance) return relevance;
      }
      return (left.path || '\uffff').localeCompare(right.path || '\uffff') || left.id.localeCompare(right.id);
    });
    return { ...snapshot, nodes, edges, complete: true };
  }

  async function executeStaticQuery(view) {
    if (view === 'tasks') return { view, tasks: staticTaskQuery(await staticSnapshot('tasks')) };
    const [graph, tasks] = await Promise.all([staticSnapshot('graph'), staticSnapshot('tasks')]);
    return { view, graph: staticGraphQuery(graph, tasks) };
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
    await Promise.all([loadGraph(), loadTasks()]);
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
    graphWorkspace.hidden = !graphActive;
    taskWorkspace.hidden = graphActive;
    document.querySelectorAll('.graph-control, .graph-filters').forEach((element) => { element.hidden = !graphActive; });
    document.querySelectorAll('.task-control, .task-filters').forEach((element) => { element.hidden = graphActive; });
    graphViewButton.classList.toggle('active', graphActive);
    tasksViewButton.classList.toggle('active', !graphActive);
    graphViewButton.setAttribute('aria-selected', String(graphActive));
    tasksViewButton.setAttribute('aria-selected', String(!graphActive));
    if (graphActive) {
      state.graphView?.width(graphElement.clientWidth).height(graphElement.clientHeight);
    }
    syncQueryControls(view);
    if (historyMode) updateUrl(historyMode);
    if (load) {
      if (graphActive) loadGraph();
      else loadTasks();
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
    if (!value || (Array.isArray(value) && value.length === 0)) return;
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
  document.querySelectorAll('.graph-filters .edge-options input[value]').forEach((input) => input.addEventListener('change', () => {
    updateUrl();
    loadGraph();
  }));
  document.querySelectorAll('.filters').forEach((container) => {
    const view = container.classList.contains('graph-filters') ? 'graph' : 'tasks';
    const menu = container.querySelector('.preset-menu');
    container.querySelector('.preset-add').addEventListener('click', () => { menu.hidden = !menu.hidden; });
    container.querySelector('.cel-filter input').addEventListener('input', (event) => {
      state.filter[view] = event.target.value;
      updateUrl();
      clearTimeout(state.searchTimer);
      state.searchTimer = setTimeout(() => runViewQuery(view), 250);
    });
    container.querySelector('.query-sort').addEventListener('change', (event) => {
      state.sort[view] = event.target.value;
      updateUrl();
      runViewQuery(view);
    });
  });
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
