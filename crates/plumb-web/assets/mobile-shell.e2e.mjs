import assert from 'node:assert/strict';

// Narrow-shell contract: top navigation stays in one row, filters collapse into a
// sheet, detail is pushed (or sheeted for events), and desktop keeps the split.
const port = process.env.PLUMB_CDP_PORT || '9227';
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

async function evaluate(expression) {
  const response = await command('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  assert.equal(response.result.exceptionDetails, undefined, response.result.exceptionDetails?.text);
  return response.result.result.value;
}

const base = process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38922';
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function viewport(width, height, mobile) {
  await command('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 2, mobile });
  await command('Emulation.setTouchEmulationEnabled', { enabled: mobile, maxTouchPoints: mobile ? 5 : 0 });
}

async function waitForReady(ready, attempts = 120) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (await evaluate(`Boolean(${ready})`)) return true;
    await wait(50);
  }
  return false;
}

async function open(path, ready, reloadOnTimeout = true) {
  await command('Page.navigate', { url: `${base}${path}` });
  if (await waitForReady(ready)) return;
  if (reloadOnTimeout) {
    // A viewport change racing a navigation can leave the shell half-initialised.
    await command('Page.reload');
    if (await waitForReady(ready)) return;
  }
  const state = await evaluate(`({
    url: location.pathname,
    rows: document.querySelectorAll('.task-row, .event-row').length,
    notification: document.querySelector('#notification').textContent,
    view: document.body.dataset.view,
    summary: document.querySelector('#task-summary, #summary')?.textContent,
    queryError: document.querySelector('.filters:not([hidden]) .query-error')?.textContent,
  })`);
  throw new Error(`timed out waiting for ${ready} on ${path}: ${JSON.stringify(state)}`);
}

const layout = `(() => {
  const box = (selector) => {
    const element = Array.from(document.querySelectorAll(selector)).find((candidate) => candidate.getClientRects().length > 0);
    if (!element) return null;
    const rect = element.getBoundingClientRect();
    return { x: Math.round(rect.x), y: Math.round(rect.y), w: Math.round(rect.width), h: Math.round(rect.height), position: getComputedStyle(element).position };
  };
  const small = [];
  document.querySelectorAll('button, select').forEach((element) => {
    const rect = element.getBoundingClientRect();
    if (!rect.width || !rect.height || element.getClientRects().length === 0 || element.hidden) return;
    if (rect.width < 24 || rect.height < 24) small.push({ label: (element.textContent || '').trim().slice(0, 16), w: Math.round(rect.width), h: Math.round(rect.height) });
    if (element.matches('.task-disclosure, .preset-add, .filters-toggle, .search-toggle, #shell-more, #new-task:not([hidden]), #new-event-fab:not([hidden]), .detail-back:not([hidden])') && (rect.width < 32 || rect.height < 32)) {
      small.push({ label: 'touch:' + (element.textContent || element.className).trim().slice(0, 14), w: Math.round(rect.width), h: Math.round(rect.height) });
    }
  });
  return {
    overflowX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    toolbar: box('.toolbar'),
    filters: box('.filters:not([hidden])'),
    list: box('.task-list-pane, .event-list-pane, .graph-stage'),
    panel: box('#note-panel, #task-panel, #event-panel'),
    small,
    detailOpen: document.body.classList.contains('detail-open'),
    shellMoreHidden: document.querySelector('#shell-more').hidden,
    backHidden: document.querySelector('#detail-back').hidden,
    scrimHidden: document.querySelector('#scrim').hidden,
  };
})()`;

try {
  // ---- narrow portrait: tasks ----
  await viewport(390, 844, true);
  await open('/tasks', `document.querySelector('.task-row')`);
  const list = await evaluate(layout);
  assert.equal(list.overflowX, 0, 'tasks list must not scroll horizontally');
  assert.ok(list.toolbar.h <= 48, `narrow toolbar must stay a single row (got ${list.toolbar.h})`);
  assert.ok(list.filters.h <= 96, `narrow filters must stay compact (got ${list.filters.h})`);
  assert.ok(list.list.h >= 600, `narrow list must own the screen (got ${list.list.h})`);
  assert.equal(list.shellMoreHidden, true, 'tasks view has no overflow entries yet');
  assert.deepEqual(list.small, [], 'narrow tasks has no sub-24px targets');
  const historyBefore = await evaluate('history.length');

  await evaluate(`document.querySelector('.task-row').click()`);
  await wait(350);
  const detail = await evaluate(layout);
  assert.equal(detail.detailOpen, true, 'selecting a task opens the pushed detail');
  assert.equal(detail.panel.position, 'fixed', 'narrow detail is an overlay, not a split pane');
  assert.equal(detail.panel.h, 844, 'narrow detail covers the viewport');
  assert.equal(detail.backHidden, false, 'pushed detail exposes a back affordance');
  assert.equal(await evaluate('history.length'), historyBefore + 1, 'pushing detail adds a history entry');

  await evaluate(`document.querySelector('#detail-back').click()`);
  await wait(350);
  const back = await evaluate(layout);
  assert.equal(back.detailOpen, false, 'back closes the pushed detail');
  assert.equal(back.backHidden, true, 'back affordance hides with the detail');

  await evaluate(`document.querySelector('.task-filters .filters-toggle').click()`);
  await wait(200);
  const sheet = await evaluate(`(() => {
    const visible = Array.from(document.querySelector('#sheet-body').children).filter((element) => !element.hidden && getComputedStyle(element).display !== 'none');
    return { open: !document.querySelector('#sheet').hidden, scrim: !document.querySelector('#scrim').hidden, labels: visible.map((element) => (element.textContent || '').trim().slice(0, 12)), hasGraphEdges: visible.some((element) => element.classList.contains('edge-options')) };
  })()`);
  assert.equal(sheet.open, true, 'filters button opens the sheet');
  assert.equal(sheet.scrim, true, 'sheet has a dismissing scrim');
  assert.equal(sheet.hasGraphEdges, false, 'task sheet only carries task controls');
  assert.ok(sheet.labels.some((label) => label.includes('Priority')), 'task sheet carries the sort keys');
  await evaluate(`document.querySelector('#scrim').click()`);
  await wait(200);
  assert.equal(await evaluate(`document.querySelector('#sheet').hidden`), true, 'scrim closes the sheet');

  // ---- narrow portrait: graph ----
  await open('/graph', `window.plumbGraph && window.plumbGraph.graphData().nodes.length`);
  const graph = await evaluate(layout);
  assert.equal(graph.overflowX, 0, 'graph view must not scroll horizontally');
  assert.ok(graph.list.h >= 700, `narrow canvas owns the screen (got ${graph.list.h})`);
  assert.equal(graph.shellMoreHidden, false, 'graph keeps its controls behind the overflow button');
  await evaluate(`document.querySelector('#shell-more').click()`);
  await wait(200);
  const menu = await evaluate(`(() => {
    const visible = Array.from(document.querySelector('#shell-menu').children).filter((element) => !element.hidden);
    return { open: !document.querySelector('#shell-menu').hidden, labels: visible.map((element) => (element.textContent || '').trim().slice(0, 10)) };
  })()`);
  assert.equal(menu.open, true, 'overflow button opens the menu');
  assert.ok(menu.labels.some((label) => label.startsWith('Direction')), 'menu carries the graph controls');
  await evaluate(`document.querySelector('#scrim').click()`);
  // A note selection uses the same pushed layer as tasks.
  const nodeId = await evaluate(`window.plumbGraph.graphData().nodes[0].id`);
  await open(`/graph?selected=${encodeURIComponent(nodeId)}`, `document.body.classList.contains('detail-open')`);
  const note = await evaluate(layout);
  assert.equal(note.panel.position, 'fixed', 'note detail is a pushed layer');
  assert.equal(note.panel.h, 844, 'note detail covers the viewport');
  assert.equal(note.backHidden, false, 'note detail exposes the shared back affordance');
  assert.equal(note.scrimHidden, true, 'a pushed detail needs no scrim');
  await evaluate(`document.querySelector('#detail-back').click()`);
  await wait(350);
  assert.equal(await evaluate(`document.body.classList.contains('detail-open')`), false, 'back closes the note detail');

  // ---- narrow portrait: agenda pushes its detail too ----
  await open('/agenda', `document.querySelector('.event-row')`);
  const agenda = await evaluate(layout);
  assert.equal(agenda.overflowX, 0, 'agenda must not scroll horizontally');
  assert.ok(agenda.filters === null, 'narrow agenda has no filter row');
  const agendaHistory = await evaluate('history.length');
  await evaluate(`document.querySelector('.event-row').click()`);
  await wait(350);
  const event = await evaluate(layout);
  assert.equal(event.detailOpen, true, 'selecting an event pushes its detail');
  assert.equal(event.panel.position, 'fixed', 'event detail is a pushed layer');
  assert.equal(event.panel.h, 844, 'event detail covers the viewport');
  assert.equal(event.backHidden, false, 'event detail exposes the same back affordance');
  assert.equal(event.scrimHidden, true, 'a pushed detail needs no scrim');
  assert.equal(await evaluate('history.length'), agendaHistory + 1, 'pushing the event detail adds a history entry');
  await evaluate(`document.querySelector('#detail-back').click()`);
  await wait(350);
  assert.equal(await evaluate(`document.body.classList.contains('detail-open')`), false, 'back closes the event detail');

  // ---- narrow creation flows use the pushed detail / sheet ----
  await open('/tasks', `document.querySelector('.task-row')`);
  await evaluate(`document.querySelector('#new-task').click()`);
  await wait(400);
  const taskForm = await evaluate(`(() => {
    const panel = document.querySelector('#task-panel');
    return { open: document.body.classList.contains('detail-open'), form: Boolean(document.querySelector('#task-panel .task-form')), position: getComputedStyle(panel).position, h: Math.round(panel.getBoundingClientRect().height) };
  })()`);
  assert.equal(taskForm.open, true, 'FAB opens the task form as a pushed detail');
  assert.equal(taskForm.form, true, 'task form renders inside the pushed detail');
  assert.equal(taskForm.position, 'fixed', 'task form panel is a fixed layer');
  assert.equal(taskForm.h, 844, 'task form panel covers the viewport');
  await evaluate(`document.querySelector('.task-form .cancel-task-form').click()`);
  await wait(300);
  assert.equal(await evaluate(`document.body.classList.contains('detail-open')`), false, 'cancelling the task form closes the detail');

  await open('/agenda', `document.querySelector('.event-row')`);
  await evaluate(`document.querySelector('#new-event-fab').click()`);
  await wait(400);
  const eventForm = await evaluate(`(() => {
    const panel = document.querySelector('#event-panel');
    return { open: document.body.classList.contains('detail-open'), form: Boolean(document.querySelector('#event-panel .event-form')), scrim: !document.querySelector('#scrim').hidden, h: Math.round(panel.getBoundingClientRect().height) };
  })()`);
  assert.equal(eventForm.open, true, 'agenda FAB pushes the event form');
  assert.equal(eventForm.form, true, 'event form renders inside the pushed layer');
  assert.equal(eventForm.h, 844, 'event form gets the full height, not a sheet');
  assert.equal(eventForm.scrim, false, 'a pushed form needs no scrim');
  await evaluate(`document.querySelector('.event-form .cancel-event').click()`);
  await wait(300);
  assert.equal(await evaluate(`document.body.classList.contains('detail-open')`), false, 'cancelling the event form closes the sheet');

  // ---- narrow landscape: same shell ----
  await viewport(844, 390, true);
  await open('/tasks', `document.querySelector('.task-row')`);
  const landscape = await evaluate(layout);
  assert.equal(landscape.overflowX, 0, 'landscape must not scroll horizontally');
  assert.ok(landscape.toolbar.h <= 48, `landscape keeps the one-row toolbar (got ${landscape.toolbar.h})`);
  assert.ok(landscape.list.h >= 250, `landscape list keeps usable height (got ${landscape.list.h})`);

  // ---- desktop: split layout, controls back in the toolbar ----
  await viewport(1440, 900, false);
  await open('/tasks', `document.querySelector('.task-row')`);
  const desktop = await evaluate(layout);
  assert.equal(desktop.overflowX, 0, 'desktop must not scroll horizontally');
  assert.equal(desktop.toolbar.h, 58, 'desktop toolbar keeps its full-height row');
  assert.equal(desktop.panel.position, 'static', 'desktop keeps the inline detail pane');
  assert.ok(desktop.panel.x > 700, 'desktop detail pane stays on the right');
  assert.equal(desktop.backHidden, true, 'desktop has no pushed-detail back affordance');
  assert.deepEqual(desktop.small, [], 'desktop has no sub-24px targets');
  await evaluate(`document.querySelector('.task-row').click()`);
  await wait(300);
  assert.equal(await evaluate(`document.body.classList.contains('detail-open')`), false, 'desktop selection stays inline');
  assert.equal(await evaluate(`document.querySelector('#summary').parentElement.className`), 'filters graph-filters', 'desktop restores the graph summary into its filter row');

  socket.close();
  console.log('narrow shell browser E2E passed');
} catch (error) {
  socket.close();
  throw error;
}
