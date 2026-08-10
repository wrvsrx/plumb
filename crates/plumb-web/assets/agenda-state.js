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

export function agendaPageRange(total, anchor, pageSize = 240) {
  if (total <= 0 || pageSize <= 0) return { start: 0, end: 0 };
  const size = Math.min(total, pageSize);
  const boundedAnchor = Math.max(0, Math.min(anchor, total - 1));
  const start = Math.max(0, Math.min(boundedAnchor - Math.floor(size / 2), total - size));
  return { start, end: start + size };
}

export function adjacentAgendaPage(range, direction, total) {
  const size = Math.max(0, range.end - range.start);
  if (size === 0 || total <= size) return { start: 0, end: Math.max(0, total) };
  const start = direction === 'earlier'
    ? Math.max(0, range.start - size)
    : Math.min(total - size, range.start + size);
  return { start, end: Math.min(total, start + size) };
}
