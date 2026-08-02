export function currentTimeInsertionIndex(events, now = new Date()) {
  const nowTime = now.getTime();
  if (Number.isNaN(nowTime)) return events.length;
  const index = events.findIndex((event) => {
    const value = event.at || event.start;
    if (!value) return false;
    const eventTime = new Date(value).getTime();
    return !Number.isNaN(eventTime) && eventTime >= nowTime;
  });
  return index === -1 ? events.length : index;
}

export function localDateKey(value) {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return [date.getFullYear(), date.getMonth() + 1, date.getDate()]
    .map((part, index) => String(part).padStart(index === 0 ? 4 : 2, '0'))
    .join('-');
}
