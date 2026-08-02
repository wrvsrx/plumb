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
  return EDITABLE_TASK_PROPERTIES.filter(({ key }) => !taskPropertyHasValue(task, key));
}
