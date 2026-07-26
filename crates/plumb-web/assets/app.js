(function () {
  'use strict';

  const config = JSON.parse(document.body.dataset.plumbConfig);
  const state = {
    graph: null,
    graphView: null,
    graphConfigured: false,
    renderedNodes: [],
    renderedEdges: [],
    labelBounds: [],
    current: config.current || new URLSearchParams(location.search).get('current'),
    hovered: null,
    local: Boolean(config.current || new URLSearchParams(location.search).get('current')),
    query: '',
    searchTimer: null,
    view: new URLSearchParams(location.search).get('view') === 'tasks' ? 'tasks' : 'graph',
    tasks: null,
    taskQuery: '',
    taskState: 'ready',
    selectedTask: null,
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
  const taskState = document.getElementById('task-state');
  const taskSummary = document.getElementById('task-summary');
  const taskList = document.getElementById('task-list');
  const taskEmpty = document.getElementById('task-empty');
  const taskPanel = document.getElementById('task-panel');
  const notification = document.getElementById('notification');
  let notificationTimer;

  function notify(message, error = false) {
    clearTimeout(notificationTimer);
    notification.textContent = message;
    notification.classList.toggle('error', error);
    notification.hidden = false;
    notificationTimer = setTimeout(() => { notification.hidden = true; }, error ? 8000 : 4000);
  }

  function selectedKinds() {
    return Array.from(document.querySelectorAll('.filters input[value]:checked')).map((input) => input.value);
  }

  function graphUrl() {
    const url = new URL(config.graphUrl, location.href);
    selectedKinds().forEach((kind) => url.searchParams.append('kinds', kind));
    if (state.local && state.current) {
      url.searchParams.set('current', state.current);
      url.searchParams.set('depth', depth.value);
      url.searchParams.set('direction', direction.value);
    }
    return url;
  }

  async function loadGraph() {
    try {
      const response = await fetch(graphUrl(), { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      state.graph = await response.json();
      renderGraph();
    } catch (error) {
      summary.textContent = 'Graph unavailable';
      panel.innerHTML = '<div class="note-empty"><h1>Graph unavailable</h1><p></p></div>';
      panel.querySelector('p').textContent = String(error);
    }
  }

  function renderGraph() {
    const query = state.query.trim().toLocaleLowerCase();
    const matched = new Set(
      state.graph.nodes
        .filter((node) => !query || node.title.toLocaleLowerCase().includes(query) || (node.path || '').toLocaleLowerCase().includes(query))
        .map((node) => node.id)
    );
    const nextNodes = state.graph.nodes.filter((node) => matched.has(node.id));
    const nextEdges = state.graph.edges.filter((edge) => matched.has(edge.source) && matched.has(edge.target));
    const { nodes, edges, topologyChanged } = reconcileGraph(nextNodes, nextEdges);
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
    if (!(allLabels.checked || state.query || node.id === state.current || node.id === state.hovered)) return;
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
    const required = node.id === state.current || node.id === state.hovered || state.query;
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
    if (!node && state.query) {
      state.query = '';
      search.value = '';
      renderGraph();
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
    return loadGraph();
  }

  async function refreshWorkspace() {
    const current = state.current;
    await Promise.all([loadGraph(), config.tasksUrl ? loadTasks() : Promise.resolve()]);
    if (!current || state.current !== current) return;
    const node = state.graph?.nodes.find((candidate) => candidate.id === current);
    if (node) {
      selectNode(node);
      return;
    }
    panel.innerHTML = '<div class="note-empty"><h1>Note unavailable</h1><p>This note is no longer in the workspace.</p></div>';
  }

  function showView(view) {
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
    } else if (!state.tasks) {
      loadTasks();
    }
  }

  async function loadTasks() {
    if (!config.tasksUrl) return;
    try {
      const response = await fetch(config.tasksUrl, { cache: 'no-store' });
      if (!response.ok) throw new Error(await response.text());
      state.tasks = await response.json();
      renderTasks();
    } catch (error) {
      taskSummary.textContent = 'Tasks unavailable';
      taskPanel.innerHTML = '<div class="note-empty"><h1>Tasks unavailable</h1><p></p></div>';
      taskPanel.querySelector('p').textContent = String(error);
    }
  }

  function filteredTasks() {
    const query = state.taskQuery.trim().toLocaleLowerCase();
    return state.tasks.tasks
      .filter((task) => {
        if (state.taskState !== 'all' && task.state !== state.taskState) return false;
        return !query || [task.title, task.id || '', task.path]
          .some((value) => value.toLocaleLowerCase().includes(query));
      });
  }

  function taskStateLabel(task) {
    return task.state.charAt(0).toUpperCase() + task.state.slice(1);
  }

  function renderTasks() {
    if (!state.tasks) return;
    const tasks = filteredTasks();
    taskList.replaceChildren();
    taskEmpty.hidden = tasks.length > 0;
    taskSummary.textContent = `${tasks.length} of ${state.tasks.tasks.length} tasks`;
    tasks.forEach((task) => {
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
      const selected = state.tasks.tasks.find((task) => task.key === state.selectedTask);
      if (selected) renderTaskDetail(selected);
      else clearTaskDetail('Task unavailable', 'This task is no longer in the workspace.');
    }
  }

  function selectTask(task) {
    state.selectedTask = task.key;
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
    addTaskField(fields, 'Recurrence', task.recur);
    addTaskField(fields, 'Dependencies', task.depends);
    addTaskField(fields, 'Waiting for', task.waitReasons);
    const mutable = Boolean(config.taskMutations && task.id && ['ready', 'waiting'].includes(task.state));
    taskPanel.querySelector('.complete-task').disabled = !mutable || task.blocked;
    taskPanel.querySelector('.cancel-task').disabled = !mutable;
    taskPanel.querySelector('.complete-task').addEventListener('click', () => updateTask(task, 'complete'));
    taskPanel.querySelector('.cancel-task').addEventListener('click', () => updateTask(task, 'cancel'));
    taskPanel.querySelector('.open-note').addEventListener('click', () => {
      showView('graph');
      selectDocument(task.documentId, '');
    });
  }

  async function updateTask(task, action) {
    const verb = action === 'complete' ? 'Complete' : 'Cancel';
    if (!window.confirm(`${verb} “${task.title}”?`)) return;
    const url = `${config.taskActionBase}${encodeURIComponent(task.documentId)}/${encodeURIComponent(task.id)}/${action}`;
    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: task.revision }),
      });
      const body = await response.text();
      if (!response.ok) throw new Error(body || `HTTP ${response.status}`);
      state.tasks = JSON.parse(body);
      renderTasks();
      notify(`${verb}d task.`);
    } catch (error) {
      await loadTasks();
      notify(String(error), true);
    }
  }

  search.addEventListener('input', () => {
    state.query = search.value;
    clearTimeout(state.searchTimer);
    state.searchTimer = setTimeout(renderGraph, 140);
  });
  taskSearch.addEventListener('input', () => {
    state.taskQuery = taskSearch.value;
    renderTasks();
  });
  taskState.addEventListener('change', () => {
    state.taskState = taskState.value;
    renderTasks();
  });
  graphViewButton.addEventListener('click', () => showView('graph'));
  tasksViewButton.addEventListener('click', () => showView('tasks'));
  document.querySelectorAll('.filters input[value]').forEach((input) => input.addEventListener('change', loadGraph));
  allLabels.addEventListener('change', refreshStyles);
  depth.addEventListener('change', loadGraph);
  direction.addEventListener('change', loadGraph);
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
  if (!config.tasksUrl) tasksViewButton.hidden = true;
  if (state.view === 'tasks' && config.tasksUrl) showView('tasks');
  const initialLoad = setLocal(state.local);
  if (state.current) {
    initialLoad.then(() => {
      const node = state.renderedNodes.find((candidate) => candidate.id === state.current);
      if (node) selectNode(node);
    });
  }
})();
