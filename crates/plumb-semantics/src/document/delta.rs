use super::{
    document_title_fact, AnchorRecord, DocumentOutput, LinkRecord, SemanticNode, SemanticNodeOutput,
};
use crate::{EventRecord, RelativeSemanticRecord, SemanticRecordView, SemanticRecords, TaskRecord};
use std::sync::Arc;

/// A location within one snapshot and one typed record collection, not a global id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticRecordAddress {
    pub node_index: usize,
    pub local_record_index: usize,
}

#[cfg(test)]
mod tests {
    use super::super::{analyze_document, analyze_document_incremental, DocumentChange};
    use super::*;
    use std::collections::BTreeMap;

    fn output(source: &str) -> DocumentOutput {
        let parsed = plumb_syntax::parse(source);
        analyze_document(parsed.valid_syntax().expect("valid fixture"))
    }

    fn verify_records<T: RelativeSemanticRecord>(
        previous: &DocumentOutput,
        current: &DocumentOutput,
        changes: &[SemanticRecordChange<'_, T>],
        records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
        expected: &SemanticRecords<T>,
    ) {
        let mut retained = BTreeMap::new();
        for (node_index, node) in previous.root.tree.nodes.iter().enumerate() {
            for (local_record_index, record) in records(&node.output)
                .owned_records()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                retained.insert(
                    SemanticRecordAddress {
                        node_index,
                        local_record_index,
                    },
                    SemanticRecordView {
                        record,
                        offset: node.offset as isize,
                    }
                    .to_owned(),
                );
            }
        }
        let mut added = Vec::new();
        let mut old_addresses = Vec::new();
        let mut new_addresses = Vec::new();
        for change in changes {
            let (old, new) = match change {
                SemanticRecordChange::Added(new) => (None, Some(new)),
                SemanticRecordChange::Removed(old) => (Some(old), None),
                SemanticRecordChange::Changed { previous, current } => {
                    (Some(previous), Some(current))
                }
            };
            if let Some(old) = old {
                assert_eq!(retained.remove(&old.address), Some(old.value.to_owned()));
                old_addresses.push(old.address);
            }
            if let Some(new) = new {
                let node = &current.root.tree.nodes[new.address.node_index];
                let actual =
                    &records(&node.output).owned_records().unwrap()[new.address.local_record_index];
                assert!(std::ptr::eq(actual, new.value.record));
                assert_eq!(new.value.offset, node.offset as isize);
                added.push(new.value.to_owned());
                new_addresses.push(new.address);
            }
        }
        assert!(old_addresses.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(new_addresses.windows(2).all(|pair| pair[0] < pair[1]));
        let mut reconstructed = retained.into_values().chain(added).collect::<Vec<_>>();
        reconstructed.sort_by_key(RelativeSemanticRecord::start);
        assert_eq!(reconstructed, expected.iter().collect::<Vec<_>>());
    }

    fn verify(previous: &DocumentOutput, current: &DocumentOutput) {
        let delta = current.exported_semantic_delta(previous);
        let kinds = current.exported_semantic_change_kinds(previous);
        assert_eq!(kinds.is_empty(), delta.is_empty());
        assert_eq!(kinds.title, delta.title_changed);
        assert_eq!(kinds.anchors, !delta.anchors.is_empty());
        assert_eq!(kinds.links, !delta.links.is_empty());
        assert_eq!(kinds.tasks, !delta.tasks.is_empty());
        assert_eq!(kinds.events, !delta.events.is_empty());
        assert_eq!(
            delta.is_empty(),
            current.exported_semantic_summary() == previous.exported_semantic_summary()
        );
        assert_eq!(
            delta.title_changed,
            document_title_fact(current.metadata()) != document_title_fact(previous.metadata())
        );
        verify_records(
            previous,
            current,
            &delta.anchors,
            |node| &node.records.anchors,
            current.anchors(),
        );
        verify_records(
            previous,
            current,
            &delta.links,
            |node| &node.records.links,
            current.links(),
        );
        verify_records(
            previous,
            current,
            &delta.tasks,
            |node| &node.tasks.tasks,
            &current.tasks().tasks,
        );
        verify_records(
            previous,
            current,
            &delta.events,
            |node| &node.events.events,
            &current.events().events,
        );
    }

    #[test]
    fn exported_delta_reconstructs_fresh_and_incremental_records_for_valid_edits() {
        let source = "`= title Notes\n`= date 2026-09-05\n`= timezone +08:00\n\n`# Heading\n `@ duplicate\n\n`- Parent\n `+ task\n `@ duplicate\n\n `- 14:30 Event\n  `+ event\n  `= tasks #duplicate\n\n  See `->{label #duplicate}\n\n`table\n `- name age\n  `+ header\n `- Alice 10\n\n`- Tail\n `+ task\n";
        let previous = output(source);
        let mut count = 0;
        for (offset, character) in source.char_indices() {
            for replacement in ["", "x", "\n", "\u{4e2d}"] {
                let mut changed = source.to_owned();
                changed.replace_range(offset..offset + character.len_utf8(), replacement);
                let parsed = plumb_syntax::parse(&changed);
                let Some(valid) = parsed.valid_syntax() else {
                    continue;
                };
                let current = analyze_document_incremental(
                    valid,
                    &previous,
                    &DocumentChange {
                        old_range: offset..offset + character.len_utf8(),
                        new_range: offset..offset + replacement.len(),
                    },
                );
                let fresh = analyze_document(valid);
                assert_eq!(current, fresh);
                verify(&previous, &current);
                verify(&previous, &fresh);
                count += 1;
            }
        }
        assert!(count > 500);
    }

    #[test]
    fn shared_shard_context_and_location_changes_have_paired_record_addresses() {
        let source = "`= date 2026-09-05\r\n`= timezone +08:00\r\n\r\n`- 14:30 Event\r\n `+ event\r\n `@ event\r\n\r\n`- Task\r\n `+ task\r\n";
        let previous = output(source);
        for (old_range, replacement) in [(8..18, "2026-09-06"), (0..0, "Prelude \u{1f600}\r\n\r\n")]
        {
            let mut changed = source.to_owned();
            changed.replace_range(old_range.clone(), replacement);
            let parsed = plumb_syntax::parse(&changed);
            let current = analyze_document_incremental(
                parsed.valid_syntax().unwrap(),
                &previous,
                &DocumentChange {
                    new_range: old_range.start..old_range.start + replacement.len(),
                    old_range,
                },
            );
            let delta = current.exported_semantic_delta(&previous);
            assert_eq!(delta.events.len(), 1);
            let SemanticRecordChange::Changed {
                previous: old,
                current: new,
            } = &delta.events[0]
            else {
                panic!("shared event has proven correspondence");
            };
            assert_eq!(old.value.title(), new.value.title());
            verify(&previous, &current);
        }
    }

    #[test]
    fn delta_preserves_duplicate_records_and_skips_semantic_equal_reparses() {
        let source = "`- First\n `+ task\n `@ same\n\n`- Other\n `+ task\n `@ same\n";
        let previous = output(source);
        let changed = source.replacen("First", "Later", 1);
        let parsed = plumb_syntax::parse(&changed);
        let current = analyze_document_incremental(
            parsed.valid_syntax().unwrap(),
            &previous,
            &DocumentChange {
                old_range: 3..8,
                new_range: 3..8,
            },
        );
        let delta = current.exported_semantic_delta(&previous);
        assert_eq!(delta.tasks.len(), 2);
        assert!(matches!(delta.tasks[0], SemanticRecordChange::Removed(_)));
        assert!(matches!(delta.tasks[1], SemanticRecordChange::Added(_)));
        verify(&previous, &current);
        assert!(output(source).exported_semantic_delta(&previous).is_empty());
        verify(&DocumentOutput::default(), &previous);
        verify(&previous, &DocumentOutput::default());
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticRecordEntry<'a, T> {
    pub address: SemanticRecordAddress,
    pub value: SemanticRecordView<'a, T>,
}

#[derive(Debug, Clone)]
pub enum SemanticRecordChange<'a, T> {
    Added(SemanticRecordEntry<'a, T>),
    Removed(SemanticRecordEntry<'a, T>),
    Changed {
        previous: SemanticRecordEntry<'a, T>,
        current: SemanticRecordEntry<'a, T>,
    },
}

#[derive(Debug, Default)]
pub struct ExportedSemanticDelta<'a> {
    pub title_changed: bool,
    pub anchors: Vec<SemanticRecordChange<'a, AnchorRecord>>,
    pub links: Vec<SemanticRecordChange<'a, LinkRecord>>,
    pub tasks: Vec<SemanticRecordChange<'a, TaskRecord>>,
    pub events: Vec<SemanticRecordChange<'a, EventRecord>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportedSemanticChangeKinds {
    pub title: bool,
    pub anchors: bool,
    pub links: bool,
    pub tasks: bool,
    pub events: bool,
}

impl ExportedSemanticChangeKinds {
    pub fn is_empty(self) -> bool {
        !self.title && !self.anchors && !self.links && !self.tasks && !self.events
    }
}

impl ExportedSemanticDelta<'_> {
    pub fn is_empty(&self) -> bool {
        !self.title_changed
            && self.anchors.is_empty()
            && self.links.is_empty()
            && self.tasks.is_empty()
            && self.events.is_empty()
    }
}

impl DocumentOutput {
    /// Classify changed collections without allocating individual record delta entries.
    pub fn exported_semantic_change_kinds(&self, previous: &Self) -> ExportedSemanticChangeKinds {
        if Arc::ptr_eq(&self.root, &previous.root) {
            return ExportedSemanticChangeKinds::default();
        }
        ExportedSemanticChangeKinds {
            title: document_title_fact(self.metadata()) != document_title_fact(previous.metadata()),
            anchors: !self.anchors().absolute_eq(previous.anchors()),
            links: !self.links().absolute_eq(previous.links()),
            tasks: !self.tasks().tasks.absolute_eq(&previous.tasks().tasks),
            events: !self.events().events.absolute_eq(&previous.events().events),
        }
    }

    /// Compare valid snapshots of the same document, borrowing both revisions' records.
    pub fn exported_semantic_delta<'a>(&'a self, previous: &'a Self) -> ExportedSemanticDelta<'a> {
        let changed = self.exported_semantic_change_kinds(previous);
        let mut delta = ExportedSemanticDelta {
            title_changed: changed.title,
            ..Default::default()
        };
        if !changed.anchors && !changed.links && !changed.tasks && !changed.events {
            return delta;
        }
        let old = &previous.root.tree.nodes;
        let new = &self.root.tree.nodes;
        let prefix = old
            .iter()
            .zip(new)
            .take_while(|(old, new)| Arc::ptr_eq(&old.syntax, &new.syntax))
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(old, new)| Arc::ptr_eq(&old.syntax, &new.syntax))
            .count();
        let mut collect = |old: Option<(usize, &'a SemanticNode)>,
                           new: Option<(usize, &'a SemanticNode)>| {
            if let (Some((_, old)), Some((_, new))) = (old, new) {
                if old.offset == new.offset && Arc::ptr_eq(&old.output, &new.output) {
                    return;
                }
            }
            if changed.anchors {
                collect_records(old, new, |node| &node.records.anchors, &mut delta.anchors);
            }
            if changed.links {
                collect_records(old, new, |node| &node.records.links, &mut delta.links);
            }
            if changed.tasks {
                collect_records(old, new, |node| &node.tasks.tasks, &mut delta.tasks);
            }
            if changed.events {
                collect_records(old, new, |node| &node.events.events, &mut delta.events);
            }
        };
        for index in 0..prefix {
            collect(Some((index, &old[index])), Some((index, &new[index])));
        }
        for (index, node) in old.iter().enumerate().take(old.len() - suffix).skip(prefix) {
            collect(Some((index, node)), None);
        }
        for (index, node) in new.iter().enumerate().take(new.len() - suffix).skip(prefix) {
            collect(None, Some((index, node)));
        }
        for index in 0..suffix {
            let old_index = old.len() - suffix + index;
            let new_index = new.len() - suffix + index;
            collect(
                Some((old_index, &old[old_index])),
                Some((new_index, &new[new_index])),
            );
        }
        delta
    }
}

fn collect_records<'a, T: RelativeSemanticRecord>(
    old: Option<(usize, &'a SemanticNode)>,
    new: Option<(usize, &'a SemanticNode)>,
    records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
    changes: &mut Vec<SemanticRecordChange<'a, T>>,
) {
    let values = |node: Option<(usize, &'a SemanticNode)>| -> &'a [T] {
        node.and_then(|(_, node)| records(&node.output).owned_records())
            .unwrap_or_default()
    };
    let before = values(old);
    let after = values(new);
    let entry = |(node_index, node): (usize, &'a SemanticNode), local_record_index, record| {
        SemanticRecordEntry {
            address: SemanticRecordAddress {
                node_index,
                local_record_index,
            },
            value: SemanticRecordView {
                record,
                offset: node.offset as isize,
            },
        }
    };
    if let (Some(old), Some(new)) = (old, new) {
        if before.len() == after.len() {
            for (index, (before, after)) in before.iter().zip(after).enumerate() {
                if old.1.offset != new.1.offset || before != after {
                    changes.push(SemanticRecordChange::Changed {
                        previous: entry(old, index, before),
                        current: entry(new, index, after),
                    });
                }
            }
            return;
        }
    }
    if let Some(old) = old {
        changes.extend(
            before
                .iter()
                .enumerate()
                .map(|(index, value)| SemanticRecordChange::Removed(entry(old, index, value))),
        );
    }
    if let Some(new) = new {
        changes.extend(
            after
                .iter()
                .enumerate()
                .map(|(index, value)| SemanticRecordChange::Added(entry(new, index, value))),
        );
    }
}
