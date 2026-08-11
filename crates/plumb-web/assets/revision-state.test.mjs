import assert from 'node:assert/strict';
import test from 'node:test';
import {
  beginMutation, createRevisionState, endMutation, finishRefresh,
  observeRevision, queueRevision, takeQueuedRevision,
} from './revision-state.js';

test('originating mutation notification is consumed by its query revision', () => {
  const state = createRevisionState(4);
  beginMutation(state);
  queueRevision(state, 5);
  assert.equal(takeQueuedRevision(state), null);
  observeRevision(state, 5);
  endMutation(state);
  assert.equal(takeQueuedRevision(state), null);
});

test('external revisions coalesce while a refresh is running', () => {
  const state = createRevisionState(4);
  queueRevision(state, 5);
  assert.equal(takeQueuedRevision(state), 5);
  queueRevision(state, 6);
  queueRevision(state, 7);
  assert.equal(takeQueuedRevision(state), null);
  observeRevision(state, 5);
  finishRefresh(state);
  assert.equal(takeQueuedRevision(state), 7);
});
