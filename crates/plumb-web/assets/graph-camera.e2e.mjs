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
      const count = () => graph.graphData().nodes.length;
      const camera = () => ({ ...graph.centerAt(), zoom: graph.zoom() });
      const search = async (query, test) => {
        input.value = query;
        input.dispatchEvent(new Event('input'));
        await waitFor(test);
        await new Promise((resolve) => setTimeout(resolve, 150));
      };
      const fullCount = count();
      // Exercise the library's autoscale condition using its default node-count zoom.
      graph.zoom(4 / Math.cbrt(fullCount), 0);
      graph.centerAt(17, -23, 0);
      const before = camera();
      const values = () => graph.graphData().nodes.map((node) => [node.id, graph.nodeVal()(node)])
        .sort(([left], [right]) => left.localeCompare(right));
      const beforeValues = values();
      await search('syntax', () => count() > 0 && count() < fullCount);
      const filtered = camera();
      await search('', () => count() === fullCount);
      const cleared = camera();
      const afterValues = values();
      await search('zzzznonexistent', () => count() === 0);
      await search('', () => count() === fullCount);
      const afterEmpty = camera();
      await search('syntax', () => count() > 0 && count() < fullCount);
      document.querySelector('#fit').click();
      const fitted = camera();
      await search('', () => count() === fullCount);
      return { before, filtered, cleared, beforeValues, afterValues, afterEmpty,
        fitted, afterFit: camera(), fullCount };
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
  assert.notEqual(result.fitted.zoom, result.before.zoom, 'Fit changes the filtered camera');
  console.log(`Graph camera checks passed (${result.fullCount} notes)`);
} finally {
  socket.close();
}
