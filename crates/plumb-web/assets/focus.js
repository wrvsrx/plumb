// Task focus presentation.
//
// `focused` is a document fact: an open focus interval on a task whose closure
// is still open. The interval history is what the task detail expands, and the
// shared `next` query splits results into "in flight" and "ready to start".
// Everything here is a pure projection over the records the server returned —
// the Web client never re-filters or re-sorts the shared query result, and it
// never writes focus state without going through the mutation endpoint.
//
// Durations describe how long a task has been *marked as in flight*, never how
// long anyone worked on it, and a future start never renders as a negative age.

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function parseInstant(value) {
  if (typeof value !== 'string' || value === '') return null;
  const millis = Date.parse(value);
  return Number.isFinite(millis) ? millis : null;
}

export function focusIntervals(task) {
  return Array.isArray(task?.focusIntervals) ? task.focusIntervals : [];
}

export function isFocused(task) {
  return Boolean(task?.focused);
}

export function focusedSince(task) {
  return typeof task?.focusedSince === 'string' ? task.focusedSince : null;
}

// "just now", "45m", "2h 15m", "3d 4h" for a non-negative span.
export function formatFocusSpan(milliseconds) {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return null;
  if (milliseconds < MINUTE) return 'just now';
  if (milliseconds < HOUR) return `${Math.floor(milliseconds / MINUTE)}m`;
  if (milliseconds < DAY) {
    const hours = Math.floor(milliseconds / HOUR);
    const minutes = Math.floor((milliseconds % HOUR) / MINUTE);
    return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
  }
  const days = Math.floor(milliseconds / DAY);
  const hours = Math.floor((milliseconds % DAY) / HOUR);
  return hours === 0 ? `${days}d` : `${days}d ${hours}h`;
}

// Age of a focus start. A start in the future carries no duration: the caller
// shows the absolute timestamp plus an anomaly hint instead.
export function focusAge(start, now = Date.now()) {
  const millis = parseInstant(start);
  if (millis === null) return null;
  if (millis > now) return { future: true, milliseconds: millis - now, label: null };
  return { future: false, milliseconds: now - millis, label: formatFocusSpan(now - millis) };
}

// Rows for the expandable history: one per recorded interval, oldest first,
// with the open interval last.
export function focusHistory(task, now = Date.now()) {
  return focusIntervals(task).map((interval) => {
    const start = interval?.start ?? null;
    const end = typeof interval?.end === 'string' && interval.end !== '' ? interval.end : null;
    const open = end === null;
    const startMillis = parseInstant(start);
    const endMillis = open ? now : parseInstant(end);
    const future = startMillis !== null && startMillis > now;
    const span =
      startMillis === null || endMillis === null || future ? null : endMillis - startMillis;
    return {
      start,
      end,
      open,
      future,
      label: span === null ? null : formatFocusSpan(span),
    };
  });
}

// Row/detail badge: only a currently focused task is marked, and the label is
// an age rather than a work duration.
export function focusBadge(task, now = Date.now()) {
  if (!isFocused(task)) return null;
  const since = focusedSince(task);
  const age = since === null ? null : focusAge(since, now);
  if (age === null) return 'focused';
  if (age.future) return 'focused (future start)';
  return `focused ${age.label}`;
}

// The shared `next` query returns two independent sections; this only reads the
// server's split and completeness signals.
export function nextSections(result) {
  const inFlight = Array.isArray(result?.focused) ? result.focused : [];
  const candidates = Array.isArray(result?.candidates) ? result.candidates : [];
  return {
    inFlight,
    candidates,
    inFlightTotal: Number.isInteger(result?.focusedTotal) ? result.focusedTotal : inFlight.length,
    inFlightTruncated: result?.focusedComplete === false,
    candidateLimit: Number.isInteger(result?.candidateLimit)
      ? result.candidateLimit
      : candidates.length,
    candidatesComplete: result?.candidatesComplete !== false,
    skippedInvalid: Array.isArray(result?.skippedInvalid) ? result.skippedInvalid : [],
    complete: result?.complete !== false,
  };
}

// Timezone-preserving label for one recorded instant. Invalid input renders as
// an empty string rather than a fabricated date.
export function focusInstantLabel(value) {
  const match =
    /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})(?::\d{2})?(?:\.\d+)?(Z|[+-]\d{2}:\d{2})?$/.exec(
      typeof value === 'string' ? value : '',
    );
  if (!match) return '';
  const [, date, time, zone] = match;
  return zone ? `${date} ${time} ${zone}` : `${date} ${time}`;
}

// In-flight heading: the total is shown whenever the page hides older focus
// intervals, so a paged section is never mistaken for the whole section.
export function inFlightHeading(sections) {
  const shown = sections.inFlight.length;
  if (sections.inFlightTruncated || sections.inFlightTotal > shown) {
    return `In flight (${shown} of ${sections.inFlightTotal})`;
  }
  return `In flight (${sections.inFlightTotal})`;
}

// Candidate heading states the requested limit, so a short list is never
// confused with a complete one.
export function candidateHeading(sections) {
  return `Ready to start (limit ${sections.candidateLimit})`;
}

// One status line for the Tasks view: both section sizes, plus the two
// independent completeness signals and skipped invalid-focus tasks.
export function nextSummary(sections) {
  const parts = [
    `${sections.inFlightTotal} in flight`,
    `${sections.candidates.length} ready to start`,
  ];
  if (!sections.candidatesComplete) parts.push('limit applied');
  if (!sections.complete) parts.push('index incomplete');
  if (sections.skippedInvalid.length > 0) {
    parts.push(`${sections.skippedInvalid.length} skipped (invalid focus)`);
  }
  return parts.join(' · ');
}
