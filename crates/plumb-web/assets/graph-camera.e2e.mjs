import assert from 'node:assert/strict';

const port = process.env.PLUMB_CDP_PORT || '9225';
const pages = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
const page = pages.find((candidate) => candidate.type === 'page');
assert.ok(page, 'Chromium page target is available');
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
let sequence = 0;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  const callback = pending.get(message.id);
  if (callback) {
    pending.delete(message.id);
    callback(message);
  }
});
function command(method, params = {}) {
  const id = ++sequence;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

try {
  await command('Page.navigate', {
    url: process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38924/graph',
  });
  const response = await command('Runtime.evaluate', {
    awaitPromise: true,
    returnByValue: true,
    expression: `(async () => {
      const waitFor = async (test) => {
        for (let attempt = 0; attempt < 100; attempt += 1) {
          if (test()) return;
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        throw new Error('timed out waiting for graph state');
      };
      await waitFor(() => window.plumbGraph?.graphData().nodes.length > 0);
      await new Promise((resolve) => setTimeout(resolve, 300));
      const graph = window.plumbGraph;
      const input = document.querySelector('#search');
      const basis = graph.graphData();
      const count = () => basis.nodes.filter(graph.nodeVisibility()).length;
      const positions = () => basis.nodes.map((node) => [node.id, node.x, node.y, graph.nodeVal()(node)]);
      const initialPositions = positions();
      let basisStable = true;
      const queries = [];
      const originalFetch = window.fetch;
      let completedQuery = null;
      window.fetch = async (...args) => {
        const response = await originalFetch(...args);
        if (args[1]?.body) {
          const request = JSON.parse(args[1].body);
          if (request.view === 'graph') {
            const result = await response.clone().json();
            queries.push({ query: request.query, graph: result.graph, revision: result.revision });
            completedQuery = request.query;
          }
        }
        return response;
      };
      const camera = () => ({ ...graph.centerAt(), zoom: graph.zoom() });
      const search = async (query, test) => {
        completedQuery = null;
        input.value = query;
        input.dispatchEvent(new Event('input'));
        await waitFor(() => completedQuery === query && test());
        await new Promise((resolve) => setTimeout(resolve, 150));
        basisStable &&= graph.graphData() === basis &&
          JSON.stringify(positions()) === JSON.stringify(initialPositions);
      };
      const fullCount = count();
      // Exercise the library's autoscale condition using its default node-count zoom.
      graph.zoom(4 / Math.cbrt(fullCount), 0);
      graph.centerAt(17, -23, 0);
      const before = camera();
      const values = () => graph.graphData().nodes.map((node) => [node.id, graph.nodeVal()(node)])
        .sort(([left], [right]) => left.localeCompare(right));
      const beforeValues = values();
      for (const text of ['s', 'sy', 'syn']) await search(text, () => count() > 0);
      await search('syntax', () => count() > 0 && count() < fullCount);
      const filtered = camera();
      const visibleIds = new Set(basis.nodes.filter(graph.nodeVisibility()).map((node) => node.id));
      const linksCorrect = basis.links.every((link) => graph.linkVisibility()(link) ===
        (visibleIds.has(link.source.id) && visibleIds.has(link.target.id)));
      const hidden = basis.nodes.find((node) => !visibleIds.has(node.id));
      const previousUrl = location.href;
      graph.onNodeClick()(hidden);
      graph.onNodeHover()(hidden);
      const hiddenInteractionIgnored = location.href === previousUrl && graph.nodeColor()(hidden) !== '#d94b3d';
      await search('', () => count() === fullCount);
      const cleared = camera();
      const afterValues = values();
      await search('zzzznonexistent', () => count() === 0);
      await search('', () => count() === fullCount);
      const afterEmpty = camera();
      await search('syntax', () => count() > 0 && count() < fullCount);
      document.querySelector('#fit').click();
      const fitted = camera();
      const bounds = graph.getGraphBbox(graph.nodeVisibility());
      const expectedFit = Math.min(8, Math.max(0.1, Math.min(
        (graph.width() - 96) / (bounds.x[1] - bounds.x[0]),
        (graph.height() - 96) / (bounds.y[1] - bounds.y[0])
      )));
      await search('', () => count() === fullCount);
      const afterFit = camera();
      let releaseDelayed;
      let delayedReceived = false;
      const fetchBeforeDelay = window.fetch;
      window.fetch = async (...args) => {
        const response = await fetchBeforeDelay(...args);
        if (args[1]?.body && JSON.parse(args[1].body).query === 'zzzzdelayed') {
          delayedReceived = true;
          await new Promise((resolve) => { releaseDelayed = resolve; });
        }
        return response;
      };
      input.value = 'zzzzdelayed';
      input.dispatchEvent(new Event('input'));
      await waitFor(() => delayedReceived);
      await search('syntax', () => count() > 0 && count() < fullCount);
      const acceptedVisible = basis.nodes.filter(graph.nodeVisibility()).map((node) => node.id);
      releaseDelayed();
      await new Promise((resolve) => setTimeout(resolve, 150));
      const staleIgnored = JSON.stringify(acceptedVisible) ===
        JSON.stringify(basis.nodes.filter(graph.nodeVisibility()).map((node) => node.id));
      await search('', () => count() === fullCount);
      window.fetch = originalFetch;
      return { before, filtered, cleared, beforeValues, afterValues, afterEmpty,
        fitted, expectedFit, afterFit, fullCount, basisStable, linksCorrect, hiddenInteractionIgnored,
        staleIgnored, queries };
    })()`,
  });
  assert.equal(response.result.exceptionDetails, undefined, response.result.exceptionDetails?.text);
  const result = response.result.result.value;
  const equalCamera = (actual, expected) => {
    for (const key of ['x', 'y', 'zoom']) {
      assert.ok(Math.abs(actual[key] - expected[key]) < 1e-9,
        `${key}: ${actual[key]} differs from ${expected[key]}`);
    }
  };
  equalCamera(result.filtered, result.before);
  equalCamera(result.cleared, result.before);
  equalCamera(result.afterEmpty, result.before);
  equalCamera(result.afterFit, result.fitted);
  assert.deepEqual(result.afterValues, result.beforeValues, 'clearing restores node sizing inputs');
  assert.ok(result.basisStable, 'typing and clearing preserve graph data identity, positions and sizes');
  assert.ok(result.linksCorrect, 'links are visible exactly when both endpoints are visible');
  assert.ok(result.hiddenInteractionIgnored, 'hidden nodes ignore stale click and hover callbacks');
  assert.ok(result.staleIgnored, 'delayed search response cannot overwrite newer visibility');
  assert.ok(result.queries.every((query) => query.graph === null), 'search transfers identities without the basis');
  assert.ok(Math.abs(result.fitted.zoom - result.expectedFit) < 1e-9, 'Fit uses visible-node bounds');
  assert.notEqual(result.fitted.zoom, result.before.zoom, 'Fit changes the filtered camera');
  const initialUrl = new URL(process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38924/graph');
  initialUrl.searchParams.set('q', 'syntax');
  await command('Page.navigate', { url: initialUrl.toString() });
  const initial = await command('Runtime.evaluate', {
    awaitPromise: true,
    returnByValue: true,
    expression: `(async () => {
      const waitFor = async (test) => {
        for (let attempt = 0; attempt < 100; attempt += 1) {
          if (test()) return;
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        throw new Error('timed out waiting for initial search');
      };
      await waitFor(() => document.querySelector('#search')?.value === 'syntax' &&
        window.plumbGraph?.graphData().nodes.some(window.plumbGraph.nodeVisibility()));
      await new Promise((resolve) => setTimeout(resolve, 300));
      const graph = window.plumbGraph;
      const basis = graph.graphData();
      const visible = () => basis.nodes.filter(graph.nodeVisibility()).length;
      const filteredCount = visible();
      const input = document.querySelector('#search');
      input.value = '';
      input.dispatchEvent(new Event('input'));
      await waitFor(() => visible() === basis.nodes.length);
      return { baseCount: basis.nodes.length, filteredCount, sameBasis: graph.graphData() === basis };
    })()`,
  });
  assert.equal(initial.result.exceptionDetails, undefined);
  const initialResult = initial.result.result.value;
  assert.equal(initialResult.baseCount, result.fullCount, 'initial URL search installs the complete basis');
  assert.ok(initialResult.filteredCount < initialResult.baseCount);
  assert.ok(initialResult.sameBasis, 'clearing an initial URL search only changes visibility');
  console.log(`Graph camera checks passed (${result.fullCount} notes)`);
} finally {
  socket.close();
}
