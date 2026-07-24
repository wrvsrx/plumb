(function () {
  'use strict';

  const config = JSON.parse(document.body.dataset.plumbConfig);
  const state = {
    graph: null,
    cy: null,
    current: config.current || new URLSearchParams(location.search).get('current'),
    local: Boolean(config.current || new URLSearchParams(location.search).get('current')),
    query: '',
  };

  const graphElement = document.getElementById('graph');
  const panel = document.getElementById('note-panel');
  const summary = document.getElementById('summary');
  const empty = document.getElementById('graph-empty');
  const search = document.getElementById('search');
  const depth = document.getElementById('depth');
  const direction = document.getElementById('direction');
  const globalMode = document.getElementById('global-mode');
  const localMode = document.getElementById('local-mode');

  function selectedKinds() {
    return Array.from(document.querySelectorAll('.filters input:checked')).map((input) => input.value);
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
    const nodes = state.graph.nodes.filter((node) => matched.has(node.id));
    const edges = state.graph.edges.filter((edge) => matched.has(edge.source) && matched.has(edge.target));
    empty.hidden = nodes.length > 0;
    summary.textContent = `${nodes.length} notes, ${edges.length} connections${state.graph.complete ? '' : ' (truncated)'}`;

    const elements = nodes.map((node) => ({ data: node })).concat(edges.map((edge) => ({ data: edge })));
    if (state.cy) state.cy.destroy();
    state.cy = cytoscape({
      container: graphElement,
      elements,
      minZoom: 0.12,
      maxZoom: 3.5,
      wheelSensitivity: 0.18,
      style: [
        { selector: 'node', style: {
          'background-color': '#188578', 'border-color': '#ffffff', 'border-width': 2,
          'label': 'data(title)', 'font-size': 11, 'color': '#202124', 'text-valign': 'bottom',
          'text-margin-y': 7, 'text-background-color': '#f7f7f5', 'text-background-opacity': 0.86,
          'text-background-padding': 2, 'width': 18, 'height': 18,
        }},
        { selector: 'node[?unresolved]', style: { 'background-color': '#b9bbb7', 'border-color': '#d94b3d', 'border-style': 'dashed' } },
        { selector: 'node:selected', style: { 'background-color': '#d94b3d', 'width': 25, 'height': 25 } },
        { selector: 'edge', style: { 'width': 1.4, 'line-color': '#a9aaa6', 'curve-style': 'bezier', 'opacity': 0.72 } },
        { selector: 'edge[kind = "task-depends"]', style: { 'line-color': '#d94b3d', 'target-arrow-color': '#d94b3d', 'target-arrow-shape': 'triangle' } },
        { selector: 'edge[kind = "task-prev"]', style: { 'line-color': '#d7a522', 'line-style': 'dashed' } },
        { selector: 'edge[kind = "autolink"]', style: { 'line-color': '#188578' } },
      ],
      layout: {
        name: 'circle',
        animate: false,
        avoidOverlap: true,
        nodeDimensionsIncludeLabels: true,
        padding: 48,
        sort: (left, right) => left.data('title').localeCompare(right.data('title')),
      },
    });
    window.plumbGraph = state.cy;
    state.cy.on('tap', 'node', (event) => selectNode(event.target.data()));
    if (state.current) {
      const selected = state.cy.getElementById(state.current);
      if (selected.length) selected.select();
    }
  }

  async function selectNode(node) {
    state.current = node.id;
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

  search.addEventListener('input', () => { state.query = search.value; renderGraph(); });
  document.querySelectorAll('.filters input').forEach((input) => input.addEventListener('change', loadGraph));
  depth.addEventListener('change', loadGraph);
  direction.addEventListener('change', loadGraph);
  globalMode.addEventListener('click', () => setLocal(false));
  localMode.addEventListener('click', () => setLocal(true));
  document.getElementById('fit').addEventListener('click', () => state.cy && state.cy.fit(undefined, 48));

  if (config.eventsUrl && window.EventSource) {
    const events = new EventSource(config.eventsUrl);
    events.addEventListener('workspace', loadGraph);
  }
  const initialLoad = setLocal(state.local);
  if (state.current) {
    initialLoad.then(() => {
      const node = state.graph && state.graph.nodes.find((candidate) => candidate.id === state.current);
      if (node) selectNode(node);
    });
  }
})();
