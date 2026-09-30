export const EDITABLE_TASK_PROPERTIES = [
  { key: 'created', label: 'Created' },
  { key: 'due', label: 'Due' },
  { key: 'priority', label: 'Priority' },
  { key: 'wait', label: 'Wait' },
  { key: 'recur', label: 'Recurrence' },
  { key: 'prev', label: 'Previous task' },
  { key: 'depends', label: 'Dependencies' },
];

export function taskPropertyHasValue(task, key) {
  const value = task[key];
  return Array.isArray(value) ? value.length > 0 : value !== null && value !== undefined && value !== '';
}

export function missingTaskProperties(task) {
  return EDITABLE_TASK_PROPERTIES.filter(({ key }) => !(task.locator?.kind === 'document' && key === 'recur') && !taskPropertyHasValue(task, key));
}

// Format server-accounted totals only; focus history and the clock are unrelated.
export function taskTimeSpent(task) {
  const seconds = task?.timeSpentSeconds;
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) {
    return 'Unavailable (incomplete)';
  }
  if (seconds > 0 && seconds < 1) return '<1s';
  const rounded = Math.round(seconds);
  const parts = [];
  if (rounded >= 3600) parts.push(`${Math.floor(rounded / 3600)}h`);
  if (rounded % 3600 >= 60) parts.push(`${Math.floor(rounded % 3600 / 60)}m`);
  if (rounded % 60 || parts.length === 0) parts.push(`${rounded % 60}s`);
  return parts.join(' ');
}
