use plumb_semantics::{SemanticRecords, TaskRecord, TaskState};

pub(crate) fn physical_line_ranges(
    source: &str,
    range: &std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let newline = source[start..range.end]
            .find('\n')
            .map(|offset| start + offset);
        let end = newline.unwrap_or(range.end);
        let line = &source[start..end];
        let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
        let trailing = line.len() - line.trim_end_matches([' ', '\t', '\r']).len();
        if start + leading < end.saturating_sub(trailing) {
            ranges.push(start + leading..end - trailing);
        }
        let Some(newline) = newline else {
            break;
        };
        start = newline + 1;
    }
    ranges
}

pub(crate) fn closed_task_token_ranges(
    tasks: &SemanticRecords<TaskRecord>,
) -> Vec<(std::ops::Range<usize>, u32)> {
    let tasks = tasks.iter().collect::<Vec<_>>();
    let mut children = vec![Vec::new(); tasks.len()];
    let mut ancestors: Vec<usize> = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        while ancestors
            .last()
            .is_some_and(|ancestor| tasks[*ancestor].depth >= task.depth)
        {
            ancestors.pop();
        }
        if let Some(parent) = ancestors.last() {
            children[*parent].push(task.range.clone());
        }
        ancestors.push(index);
    }

    let mut output = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let modifiers = match task.state() {
            TaskState::Open => continue,
            TaskState::Done => 1,
            TaskState::Canceled => 2,
            TaskState::Conflicted => 3,
        };
        let mut owned = vec![task.range.clone()];
        for child in &children[index] {
            owned = owned
                .into_iter()
                .flat_map(|range| subtract_range(range, child))
                .collect();
        }
        output.extend(owned.into_iter().map(|range| (range, modifiers)));
    }
    output.sort_by_key(|(range, _)| range.start);
    output
}

fn subtract_range(
    range: std::ops::Range<usize>,
    excluded: &std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    if excluded.end <= range.start || excluded.start >= range.end {
        return vec![range];
    }
    let mut output = Vec::with_capacity(2);
    if range.start < excluded.start {
        output.push(range.start..excluded.start);
    }
    if excluded.end < range.end {
        output.push(excluded.end..range.end);
    }
    output
}
