import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import { missingTaskProperties, taskPropertyHasValue } from './task-ui.js';

test('task properties distinguish editable values from missing fields', () => {
  const task = { created: '2026-08-02T12:00:00Z', priority: 0, depends: [], prev: null };
  assert.equal(taskPropertyHasValue(task, 'created'), true);
  assert.equal(taskPropertyHasValue(task, 'priority'), true);
  assert.equal(taskPropertyHasValue(task, 'depends'), false);
  assert.deepEqual(
    missingTaskProperties(task).map(({ key }) => key),
    ['due', 'wait', 'recur', 'prev', 'depends'],
  );
});

test('task creation uses a view toolbar control and sort keys shrink responsively', async () => {
  const [html, css] = await Promise.all([
    readFile(new URL('./index.html', import.meta.url), 'utf8'),
    readFile(new URL('./styles.css', import.meta.url), 'utf8'),
  ]);
  const toolbar = html.slice(html.indexOf('<header class="toolbar">'), html.indexOf('</header>'));
  const listPane = html.slice(html.indexOf('<section class="task-list-pane"'), html.indexOf('</section>', html.indexOf('<section class="task-list-pane"')));
  const detailPanel = html.slice(html.indexOf('<aside id="task-panel"'), html.indexOf('</aside>', html.indexOf('<aside id="task-panel"')));
  assert.match(toolbar, /id="new-task" class="task-control"/);
  assert.doesNotMatch(listPane, /id="new-task"/);
  assert.doesNotMatch(detailPanel, /id="new-task"/);
  assert.match(css, /\.task-sort-key \{ min-width: 0; max-width: 100%;/);
  assert.match(css, /grid-template-columns: 24px minmax\(0, 1fr\) repeat\(3, 24px\)/);
  assert.match(css, /\.task-filters \{ height: auto; min-height: 50px; flex-wrap: wrap;/);
  assert.match(css, /body \{ display: flex; flex-direction: column; overflow: hidden; \}/);
  assert.match(css, /\.task-filters \.task-sort \{ flex-basis: 100%; \}/);
});
