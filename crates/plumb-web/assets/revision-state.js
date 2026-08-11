export function createRevisionState(initialRevision = 0) {
  return { installed: Number(initialRevision) || 0, queued: 0, mutations: 0, refreshing: false };
}

export function observeRevision(state, revision) {
  state.installed = Math.max(state.installed, Number(revision) || 0);
  if (state.queued <= state.installed) state.queued = 0;
}

export function queueRevision(state, revision) {
  state.queued = Math.max(state.queued, Number(revision) || 0);
}

export function beginMutation(state) { state.mutations += 1; }
export function endMutation(state) { state.mutations = Math.max(0, state.mutations - 1); }

export function takeQueuedRevision(state) {
  if (state.mutations || state.refreshing || state.queued <= state.installed) return null;
  const revision = state.queued;
  state.queued = 0;
  state.refreshing = true;
  return revision;
}

export function finishRefresh(state) { state.refreshing = false; }
