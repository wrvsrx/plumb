(function () {
  'use strict';

  const config = JSON.parse(document.body.dataset.plumbConfig);
  const state = {
    graph: null,
    graphView: null,
    renderedNodes: [],
    labelBounds: [],
    current: config.current || new URLSearchParams(location.search).get('current'),
    hovered: null,
    local: Boolean(config.current || new URLSearchParams(location.search).get('current')),
    query: '',
    searchTimer: null,
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
    const nodes = state.graph.nodes
      .filter((node) => matched.has(node.id))
      .map((node) => ({ ...node, degree: 0 }));
    const edges = state.graph.edges
      .filter((edge) => matched.has(edge.source) && matched.has(edge.target))
      .map((edge) => ({ ...edge }));
    const byId = new Map(nodes.map((node) => [node.id, node]));
    edges.forEach((edge) => {
      byId.get(edge.source).degree += 1;
      byId.get(edge.target).degree += 1;
    });
    nodes.sort((left, right) => right.degree - left.degree || left.title.localeCompare(right.title));
    const hubs = nodes
      .slice()
      .sort((left, right) => right.degree - left.degree || left.title.localeCompare(right.title))
      .slice(0, 5);

    empty.hidden = nodes.length > 0;
    summary.textContent = `${nodes.length} notes, ${edges.length} connections${state.graph.complete ? '' : ' (truncated)'}`;
    state.renderedNodes = nodes;
    ensureGraphView();
    configureGraphView(nodes, edges);
    renderOverview(hubs);
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
      .onNodeClick(selectNode)
      .onNodeHover(handleNodeHover)
      .onBackgroundClick(() => handleNodeHover(null));
    window.plumbGraph = state.graphView;
    new ResizeObserver(() => {
      state.graphView.width(graphElement.clientWidth).height(graphElement.clientHeight);
    }).observe(graphElement);
  }

  function configureGraphView(nodes, edges) {
    const warmup = nodes.length > 500 ? 100 : 260;
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
      .linkDirectionalArrowLength((link) => link.kind === 'task-depends' ? 4 : 0)
      .linkDirectionalArrowColor(() => '#d94b3d')
      .graphData({ nodes, links: edges });
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
      .linkWidth(linkWidth);
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

  async function selectNode(node) {
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
    } catch (error) {
      panel.innerHTML = '<div class="note-empty"><h1>Cannot render note</h1><p></p></div>';
      panel.querySelector('p').textContent = String(error);
    }
  }

  function setLocal(local) {
    state.local = local && Boolean(state.current);
    globalMode.classList.toggle('active', !state.local);
    localMode.classList.toggle('active', state.local);
    localMode.disabled = !state.current;
    return loadGraph();
  }

  search.addEventListener('input', () => {
    state.query = search.value;
    clearTimeout(state.searchTimer);
    state.searchTimer = setTimeout(renderGraph, 140);
  });
  document.querySelectorAll('.filters input[value]').forEach((input) => input.addEventListener('change', loadGraph));
  allLabels.addEventListener('change', refreshStyles);
  depth.addEventListener('change', loadGraph);
  direction.addEventListener('change', loadGraph);
  globalMode.addEventListener('click', () => setLocal(false));
  localMode.addEventListener('click', () => setLocal(true));
  document.getElementById('fit').addEventListener('click', () => state.graphView && state.graphView.zoomToFit(0, 48));

  if (config.eventsUrl && window.EventSource) {
    const events = new EventSource(config.eventsUrl);
    events.addEventListener('workspace', loadGraph);
  }
  const initialLoad = setLocal(state.local);
  if (state.current) {
    initialLoad.then(() => {
      const node = state.renderedNodes.find((candidate) => candidate.id === state.current);
      if (node) selectNode(node);
    });
  }
})();
