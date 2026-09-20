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
    let mut output = Vec::new();
    let mut ancestors: Vec<(usize, std::ops::Range<usize>, u32)> = Vec::new();
    for task in tasks.views() {
        if task.owner() == plumb_semantics::TaskOwner::Document {
            let modifiers = match task.state() {
                TaskState::Open => 0,
                TaskState::Done => 1,
                TaskState::Canceled => 2,
                TaskState::Conflicted => 3,
            };
            if modifiers != 0 {
                output.push((task.selection_range(), modifiers));
            }
            continue;
        }
        while ancestors
            .last()
            .is_some_and(|(depth, _, _)| *depth >= task.depth())
        {
            let (_, remaining, modifiers) = ancestors.pop().unwrap();
            if modifiers != 0 && !remaining.is_empty() {
                output.push((remaining, modifiers));
            }
        }
        let range = task.range();
        // Direct child subtrees override the parent's state, including open children.
        if let Some((_, remaining, modifiers)) = ancestors.last_mut() {
            if *modifiers != 0 && remaining.start < range.start {
                output.push((remaining.start..range.start, *modifiers));
            }
            remaining.start = range.end;
        }
        let modifiers = match task.state() {
            TaskState::Open => 0,
            TaskState::Done => 1,
            TaskState::Canceled => 2,
            TaskState::Conflicted => 3,
        };
        ancestors.push((task.depth(), range, modifiers));
    }
    while let Some((_, remaining, modifiers)) = ancestors.pop() {
        if modifiers != 0 && !remaining.is_empty() {
            output.push((remaining, modifiers));
        }
    }
    output.sort_by_key(|(range, _)| range.start);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_document_task_highlights_selection_without_body_or_children() {
        let source = "正文保持普通高亮。\n\n`- Open child\n `+ task\n\n`+ task\n`= title Project\n`= done 2026-09-21T10:00:00+08:00\n";
        let parsed = plumb_syntax::parse(source);
        let output = plumb_semantics::analyze_document(parsed.valid_syntax().unwrap());
        let tasks = output.tasks();
        let ranges = closed_task_token_ranges(&tasks.tasks);
        assert_eq!(ranges, vec![(tasks.document_task().unwrap().selection_range(), 1)]);
        assert_eq!(&source[ranges[0].0.clone()], "Project");
    }

    #[test]
    fn token_ranges_follow_deepest_task_state_for_all_nested_closure_combinations() {
        for combination in 0..256 {
            let mut source = String::from("Prelude\n\n");
            for (index, depth) in [0, 1, 2, 1].into_iter().enumerate() {
                let indent = " ".repeat(depth);
                source.push_str(&format!("{indent}`- Task {index}\n{indent} `+ task\n"));
                let state = (combination >> (2 * index)) & 3;
                for (bit, name) in [(1, "done"), (2, "canceled")] {
                    if state & bit != 0 {
                        source.push_str(&format!("{indent} `= {name} 2026-09-07T09:00:00+08:00\n"));
                    }
                }
                source.push('\n');
            }
            source.push_str("Tail\n");
            let parsed = plumb_syntax::parse(&source);
            let output = plumb_semantics::analyze_document(parsed.valid_syntax().unwrap());
            let tasks = &output.tasks().tasks;
            assert_eq!(tasks.len(), 4);
            let ranges = closed_task_token_ranges(tasks);
            assert!(ranges
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start));
            let records = tasks.iter().collect::<Vec<_>>();
            for offset in 0..source.len() {
                let expected = records
                    .iter()
                    .filter(|task| task.range.contains(&offset))
                    .max_by_key(|task| task.depth)
                    .map_or(0, |task| match task.state() {
                        TaskState::Open => 0,
                        TaskState::Done => 1,
                        TaskState::Canceled => 2,
                        TaskState::Conflicted => 3,
                    });
                let actual = ranges
                    .iter()
                    .find(|(range, _)| range.contains(&offset))
                    .map_or(0, |(_, modifiers)| *modifiers);
                assert_eq!(actual, expected, "combination {combination}, byte {offset}");
            }
        }
    }
}
