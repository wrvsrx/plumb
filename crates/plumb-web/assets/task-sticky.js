// Parent links over the actual presentation rows, including focus groups and
// compact context rows. Build once per render; never rebuild on scroll.
export function taskRowParents(items) {
  const parents = [], stack = [];
  items.forEach((item, index) => {
    while (stack.length && items[stack.at(-1)].depth >= item.depth) stack.pop();
    parents.push(stack.at(-1) ?? -1);
    stack.push(index);
  });
  return parents;
}

export function firstRowBelow(length, bottom, boundary) {
  let low = 0, high = length;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (bottom(mid) <= boundary) low = mid + 1;
    else high = mid;
  }
  return low;
}

export function taskStickySegments(items, parents, index) {
  const ancestors = [];
  for (let parent = parents[index]; parent >= 0; parent = parents[parent]) ancestors.push(parent);
  return ancestors.reverse().flatMap((row) => {
    const item = items[row];
    if (item.kind === 'group') return [];
    const labels = item.kind === 'task'
      ? [item.task.title || '(untitled task)'] : item.segments.map((segment) => segment.title);
    return labels.map((title, segment) => ({ title, row, segment }));
  });
}

export function installTaskStickyContext(list, nav) {
  const pane = list.parentElement;
  let items = [], parents = [], rows = [], frame = 0, signature = '';
  // Fixed one-line height keeps showing/hiding the overlay from moving rows.
  const inset = () => nav.getBoundingClientRect().height;
  const buttonFor = ({ title, row, segment }) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = title;
    button.title = title;
    button.addEventListener('click', () => {
      const element = rows[row];
      pane.scrollTop += element.getBoundingClientRect().top - pane.getBoundingClientRect().top - inset();
      const target = items[row].kind === 'task' ? element.querySelector('.task-row')
        : element.querySelectorAll('.task-context-segment')[segment];
      target?.focus({ preventScroll: true });
      nav.querySelector('details')?.removeAttribute('open');
      schedule();
    });
    return button;
  };
  function update() {
    frame = 0;
    if (!pane.clientHeight) return;
    const boundary = pane.getBoundingClientRect().top + inset();
    const index = firstRowBelow(rows.length, (i) => rows[i].getBoundingClientRect().bottom, boundary);
    const segments = pane.scrollTop > 0 ? taskStickySegments(items, parents, index) : [];
    const next = JSON.stringify(segments);
    if (signature === next) return;
    signature = next;
    nav.replaceChildren();
    nav.classList.toggle('is-visible', segments.length > 0);
    nav.inert = segments.length === 0;
    const separator = () => {
      const span = document.createElement('span');
      span.textContent = '/';
      span.setAttribute('aria-hidden', 'true');
      nav.append(span);
    };
    if (segments.length <= 3) {
      segments.forEach((segment, i) => { if (i) separator(); nav.append(buttonFor(segment)); });
    } else {
      nav.append(buttonFor(segments[0]));
      separator();
      const details = document.createElement('details');
      const summary = document.createElement('summary');
      summary.textContent = '…';
      summary.setAttribute('aria-label', 'Show intermediate ancestors');
      const menu = document.createElement('div');
      menu.className = 'task-sticky-ancestors';
      segments.slice(1, -1).forEach((segment) => menu.append(buttonFor(segment)));
      details.append(summary, menu);
      details.addEventListener('keydown', (event) => {
        if (event.key === 'Escape') { details.open = false; summary.focus(); }
      });
      nav.append(details);
      separator();
      nav.append(buttonFor(segments.at(-1)));
    }
  }
  function schedule() {
    if (!frame) frame = requestAnimationFrame(update);
  }
  pane.addEventListener('scroll', schedule, { passive: true });
  const resize = new ResizeObserver(schedule);
  resize.observe(pane);
  resize.observe(list);
  return {
    reset(nextItems) {
      items = nextItems;
      parents = taskRowParents(items);
      rows = Array.from(list.children).slice(0, items.length);
      signature = '';
      // Old buttons must not remain clickable between DOM replacement and RAF.
      nav.replaceChildren();
      nav.classList.remove('is-visible');
      nav.inert = true;
      schedule();
    },
  };
}
