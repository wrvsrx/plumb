use super::*;
use policy_inputs::{NodeIdentity, PolicyNode, PolicySources};
use timeline_index::{EventId, TimelineIndex};

type NodeKey = (PathBuf, NodeIdentity);
type Instant = DateTime<FixedOffset>;

#[derive(Clone, Debug)]
struct CachedNode {
    snapshot: PolicyNode,
    intervals: Vec<EventId>,
    issues: Vec<(Range<usize>, &'static str)>,
}
#[derive(Clone, Debug)]
struct Source {
    node: NodeKey,
    range: Range<usize>,
    start: Instant,
    end: Instant,
}

/// Retains immutable inputs and applies only changed interval occurrences.
#[derive(Clone, Debug, Default)]
pub struct TimelineCheckState {
    inputs: PolicySources,
    nodes: BTreeMap<NodeKey, CachedNode>,
    sources: BTreeMap<EventId, Source>,
    index: TimelineIndex,
    next: usize,
    pub extracted_events: usize,
    pub recomputed_segments: usize,
    pub visited_intervals: usize,
}

impl Workspace {
    pub(super) fn check_timeline_graph(
        &self,
        root: &Path,
        excluded: &[PathBuf],
        state: &mut TimelineCheckState,
    ) -> Result<TimelineCheckReport, String> {
        let input = self.policy_inputs(root, excluded, &mut state.inputs)?;
        let mut report = TimelineCheckReport {
            complete: true,
            checked: 0,
            gaps: Vec::new(),
            overlaps: Vec::new(),
            issues: input.issues,
        };
        let invalid_documents = report.issues.len();
        state.extracted_events = 0;
        let nodes = input
            .nodes
            .into_iter()
            .map(|(path, node)| ((path, node.id()), node))
            .collect::<BTreeMap<_, _>>();
        let mut changes = BTreeMap::new();
        // These are computation tokens, not syntax identities. An equal interval
        // in the replacement reuses a token's topology but always gets new source
        // provenance. Duplicate intervals remain separate occurrences.
        let mut reusable = BTreeMap::<(PathBuf, Instant, Instant, i32, i32), Vec<EventId>>::new();
        let removed = state
            .nodes
            .iter()
            .filter(|(key, old)| {
                nodes
                    .get(*key)
                    .is_none_or(|new| !new.same_facts(&old.snapshot))
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in removed {
            let old = state.nodes.remove(&key).unwrap();
            for id in old.intervals {
                let source = state.sources.remove(&id).unwrap();
                reusable
                    .entry((
                        key.0.clone(),
                        source.start,
                        source.end,
                        source.start.offset().local_minus_utc(),
                        source.end.offset().local_minus_utc(),
                    ))
                    .or_default()
                    .push(id);
                changes.insert(id, None);
            }
        }
        for (key, node) in nodes {
            report.checked += node.event_count();
            if let Some(cached) = state.nodes.get_mut(&key) {
                cached.snapshot = node;
            } else {
                let mut cached = CachedNode {
                    snapshot: node.clone(),
                    intervals: Vec::new(),
                    issues: Vec::new(),
                };
                for event in node.events() {
                    state.extracted_events += 1;
                    if event.at_datetime().is_some() {
                        continue;
                    }
                    let range = event.selection_range.start - node.offset()
                        ..event.selection_range.end - node.offset();
                    let (Some(start), Some(end)) = (event.start_datetime(), event.end_datetime())
                    else {
                        cached
                            .issues
                            .push((range, "event has no valid finite interval"));
                        continue;
                    };
                    if end <= start {
                        cached.issues.push((range, "event end must follow start"));
                        continue;
                    }
                    let id = reusable
                        .get_mut(&(
                            key.0.clone(),
                            start,
                            end,
                            start.offset().local_minus_utc(),
                            end.offset().local_minus_utc(),
                        ))
                        .and_then(|ids| ids.pop())
                        .unwrap_or_else(|| {
                            let id = (0, state.next);
                            state.next += 1;
                            id
                        });
                    changes.insert(id, Some((start, end)));
                    state.sources.insert(
                        id,
                        Source {
                            node: key.clone(),
                            range,
                            start,
                            end,
                        },
                    );
                    cached.intervals.push(id);
                }
                state.nodes.insert(key.clone(), cached);
            }
            let cached = &state.nodes[&key];
            report
                .issues
                .extend(cached.issues.iter().map(|(range, message)| {
                    issue(
                        "agenda.invalid-time",
                        *message,
                        location(
                            &key.0,
                            range.start + cached.snapshot.offset()
                                ..range.end + cached.snapshot.offset(),
                        ),
                    )
                }));
        }
        report.issues[invalid_documents..].sort_by(|a, b| a.source.cmp(&b.source));
        let project = |id: &EventId| {
            let source = &state.sources[id];
            let offset = state.nodes[&source.node].snapshot.offset();
            location(
                &source.node.0,
                source.range.start + offset..source.range.end + offset,
            )
        };
        state.recomputed_segments = state.index.apply_changes(changes, |id| project(id));
        state.visited_intervals = state.index.visited_intervals;
        for (start, segment) in &state.index.segments {
            let mut ids = segment.events.clone();
            ids.sort_by_key(|id| {
                (
                    !segment.overlap && state.sources[id].end != *start,
                    project(id),
                )
            });
            let value = TimelineSegment {
                start: *start,
                end: segment.end,
                events: ids.iter().map(project).collect(),
            };
            if segment.overlap {
                report.overlaps.push(value);
            } else {
                report.gaps.push(value);
            }
        }
        report.complete = report.issues.is_empty();
        Ok(report)
    }
}
