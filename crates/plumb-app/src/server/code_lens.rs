use plumb_semantics::{AnchorKind, DocumentOutput};
use plumb_syntax::GreenDocument;
use std::ops::Range;

use crate::position::{position_geometry_change_bound, PositionIndex};

fn source_ranges(output: &DocumentOutput) -> impl Iterator<Item = Range<usize>> + '_ {
    output
        .anchors()
        .views()
        .map(|anchor| {
            if anchor.kind() == AnchorKind::Inline {
                anchor.id_range()
            } else {
                let start = anchor.owner_range().start;
                start..start
            }
        })
        .chain(
            output
                .links()
                .views()
                .flat_map(|link| [link.selection_range(), link.target_source_range()]),
        )
        .chain(
            output
                .tasks()
                .tasks
                .views()
                .flat_map(|task| task.reference_ranges()),
        )
        .chain(
            output
                .events()
                .events
                .views()
                .flat_map(|event| event.task_reference_ranges()),
        )
}

pub(super) fn positions_changed(previous: &DocumentOutput, current: &GreenDocument) -> bool {
    let mut ranges = source_ranges(previous).peekable();
    if ranges.peek().is_none() {
        return false;
    }
    let Some(bound) = position_geometry_change_bound(previous.syntax(), current) else {
        return false;
    };
    let mut positions = None;
    ranges.filter(|range| range.end > bound).any(|range| {
        let (old, new) = positions.get_or_insert_with(|| {
            (
                PositionIndex::new(previous.syntax().source()),
                PositionIndex::new(current.source()),
            )
        });
        old.byte_range_to_lsp(&range) != new.byte_range_to_lsp(&range)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn output(source: &str) -> DocumentOutput {
        let syntax = Arc::new(GreenDocument::parse(source));
        plumb_semantics::analyze_green_document(syntax.valid_syntax().unwrap(), Arc::clone(&syntax))
            .unwrap()
    }

    #[test]
    fn reference_geometry_is_independent_of_semantic_byte_ranges() {
        for (old, new, changed) in [
            (
                "AA\nBB\n\n`# Target\n `@ target\n",
                "ABCDEF\n`# Target\n `@ target\n",
                true,
            ),
            ("`# AAAA\n `@ target\n", "`# 😀\n `@ target\n", false),
            (
                "AA\nBB\n\nSee `->{label target.plumb}\n",
                "ABCDEF\nSee `->{label target.plumb}\n",
                true,
            ),
            (
                "ABCD `->{label target.plumb}\n",
                "😀 `->{label target.plumb}\n",
                true,
            ),
            (
                "See `->{{`aaaa{label}} target.plumb}\n",
                "See `->{{`😀{label}} target.plumb}\n",
                true,
            ),
            ("`aaaa{label `@{id}}\n", "`😀{label `@{id}}\n", true),
            (
                "Old text\n\n`->{label target.plumb}\n",
                "New text\n\n`->{label target.plumb}\n",
                false,
            ),
            (
                "`->{label target.plumb}\n\nABCD\n",
                "`->{label target.plumb}\n\n😀\n",
                false,
            ),
            ("ABCD\n", "😀\n", false),
        ] {
            let previous = output(old);
            let current = output(new);
            assert_eq!(
                previous.exported_semantic_summary(),
                current.exported_semantic_summary(),
                "{old}"
            );
            assert_eq!(
                positions_changed(&previous, current.syntax()),
                changed,
                "{old}"
            );
            let incremental = previous.syntax().reparse(new).document;
            assert_eq!(positions_changed(&previous, &incremental), changed, "{old}");
        }
        for body in [
            "`- Task\n `+ task\n `= prev #previous\n `= depends #dependency\n",
            "`- 10:00 Event\n `+ event\n `= date 2026-09-07\n `= timezone +08:00\n `= tasks #task\n",
        ] {
            let old = format!("AA\nBB\n\n{body}");
            let new = format!("ABCDEF\n{body}");
            let previous = output(&old);
            let current = output(&new);
            assert_eq!(previous.exported_semantic_summary(), current.exported_semantic_summary());
            assert!(positions_changed(&previous, current.syntax()));
        }
    }

    #[test]
    #[ignore = "manual CodeLens geometry invalidation profile"]
    fn profile_reference_geometry_invalidation() {
        let source = format!(
            "AAAA\n\n{}",
            "See `->{label target.plumb#target}\n".repeat(20_000)
        );
        let previous = output(&source);
        for (name, prefix, changed) in [
            ("same_shape", "BBBB", false),
            ("changed_shape", "A\nBB", true),
        ] {
            let next = format!("{prefix}{}", &source[4..]);
            let current = previous.syntax().reparse(next).document;
            let started = std::time::Instant::now();
            for _ in 0..100 {
                assert_eq!(
                    std::hint::black_box(positions_changed(&previous, &current)),
                    changed
                );
            }
            eprintln!(
                "{name}: {:?}/call, {} bytes, 20000 references",
                started.elapsed() / 100,
                source.len()
            );
        }
    }
}
