//! Incremental interval topology, independent of source positions and adapters.
use chrono::{DateTime, FixedOffset};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};

type Instant = DateTime<FixedOffset>;
pub(super) type EventId = (usize, usize);

#[derive(Clone, Debug, Default)]
struct Boundary {
    starting: BTreeSet<EventId>,
    ending: BTreeSet<EventId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Segment {
    pub end: Instant,
    pub overlap: bool,
    pub events: Vec<EventId>,
}

/// Ids are supplied by the caller; geometry changes need not change topology.
#[derive(Clone, Debug, Default)]
pub(super) struct TimelineIndex {
    intervals: BTreeMap<EventId, (Instant, Instant)>,
    boundaries: BTreeMap<Instant, Boundary>,
    pub segments: BTreeMap<Instant, Segment>,
}

impl TimelineIndex {
    /// Returns the number of elementary intervals recomputed, not inspected ids.
    #[cfg(test)]
    pub fn update(&mut self, intervals: BTreeMap<EventId, (Instant, Instant)>) -> usize {
        self.update_with_order(intervals, |id| id.1)
    }

    pub fn update_with_order(
        &mut self,
        intervals: BTreeMap<EventId, (Instant, Instant)>,
        order: impl Fn(&EventId) -> usize,
    ) -> usize {
        let mut changed = BTreeSet::new();
        for (id, old) in &self.intervals {
            if !same_interval(intervals.get(id), old) {
                changed.extend([old.0, old.1]);
                self.boundaries.get_mut(&old.0).unwrap().starting.remove(id);
                self.boundaries.get_mut(&old.1).unwrap().ending.remove(id);
            }
        }
        for (id, new) in &intervals {
            assert!(
                new.0 < new.1,
                "index accepts finite positive intervals only"
            );
            if !same_interval(self.intervals.get(id), new) {
                changed.extend([new.0, new.1]);
                self.boundaries
                    .entry(new.0)
                    .or_default()
                    .starting
                    .insert(id.clone());
                self.boundaries
                    .entry(new.1)
                    .or_default()
                    .ending
                    .insert(id.clone());
            }
        }
        self.intervals = intervals;
        let Some(first) = changed.first().copied() else {
            return 0;
        };
        let last = *changed.last().unwrap();
        for instant in &changed {
            let Some(boundary) = self.boundaries.remove(instant) else {
                continue;
            };
            // DateTime equality ignores the displayed offset. Match a fresh
            // source-ordered sweep's choice of the first contributing endpoint.
            let first = boundary
                .starting
                .iter()
                .chain(&boundary.ending)
                .min_by_key(|id| order(id))
                .cloned();
            if let Some(id) = first {
                let interval = self.intervals[&id];
                let key = if boundary.starting.contains(&id) {
                    interval.0
                } else {
                    interval.1
                };
                self.boundaries.insert(key, boundary);
            }
        }
        // Include neighboring segments: removing a boundary can join two gaps,
        // and gap ownership depends on both its ending and starting boundary.
        let lower = self
            .boundaries
            .range(..first)
            .next_back()
            .map_or(first, |(t, _)| *t);
        let upper = self
            .boundaries
            .range((Excluded(last), Unbounded))
            .next()
            .map_or(last, |(t, _)| *t);
        let stale = self
            .segments
            .range(lower..=upper)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>();
        for instant in stale {
            self.segments.remove(&instant);
        }
        let mut active = self
            .intervals
            .iter()
            .filter(|(_, (start, end))| *start <= lower && lower < *end)
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let mut count = 0;
        let mut boundaries = self.boundaries.range(lower..).peekable();
        while let Some((instant, boundary)) = boundaries.next() {
            if *instant > upper {
                break;
            }
            if *instant != lower {
                for id in &boundary.ending {
                    active.remove(id);
                }
                active.extend(boundary.starting.iter().cloned());
            }
            let Some((next, next_boundary)) = boundaries.peek() else {
                break;
            };
            count += 1;
            let overlap = active.len() > 1;
            let events = if active.is_empty() {
                boundary
                    .ending
                    .iter()
                    .chain(&next_boundary.starting)
                    .cloned()
                    .collect()
            } else if overlap {
                active.iter().cloned().collect()
            } else {
                continue;
            };
            self.segments.insert(
                *instant,
                Segment {
                    end: **next,
                    overlap,
                    events,
                },
            );
        }
        count
    }
}

fn same_interval(old: Option<&(Instant, Instant)>, new: &(Instant, Instant)) -> bool {
    old.is_some_and(|old| {
        old == new && old.0.offset() == new.0.offset() && old.1.offset() == new.1.offset()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(n: usize) -> EventId {
        (0, n)
    }
    fn t(n: i64) -> Instant {
        DateTime::from_timestamp(n, 0).unwrap().fixed_offset()
    }
    fn intervals(items: &[(usize, i64, i64)]) -> BTreeMap<EventId, (Instant, Instant)> {
        items
            .iter()
            .map(|(id, a, b)| (key(*id), (t(*a), t(*b))))
            .collect()
    }
    fn oracle(items: &BTreeMap<EventId, (Instant, Instant)>) -> BTreeMap<Instant, Segment> {
        let times = items
            .values()
            .flat_map(|(a, b)| [*a, *b])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut result = BTreeMap::new();
        for pair in times.windows(2) {
            let active = items
                .iter()
                .filter(|(_, (a, b))| *a <= pair[0] && pair[0] < *b)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if active.len() == 1 {
                continue;
            }
            let overlap = active.len() > 1;
            let events = if overlap {
                active
            } else {
                items
                    .iter()
                    .filter(|(_, (_, b))| *b == pair[0])
                    .map(|(id, _)| id.clone())
                    .chain(
                        items
                            .iter()
                            .filter(|(_, (a, _))| *a == pair[1])
                            .map(|(id, _)| id.clone()),
                    )
                    .collect()
            };
            result.insert(
                pair[0],
                Segment {
                    end: pair[1],
                    overlap,
                    events,
                },
            );
        }
        result
    }
    #[test]
    fn incremental_topology_matches_full_coverage_through_insert_move_delete() {
        let mut index = TimelineIndex::default();
        let mut items = intervals(&[(0, 0, 100), (1, 10, 20), (2, 40, 50), (3, 110, 120)]);
        index.update(items.clone());
        assert_eq!(index.segments, oracle(&items));
        assert_eq!(index.update(items.clone()), 0);
        // Deterministic mutations cover long spanning intervals, equal endpoints,
        // gaps at either extreme, deletions and ordering changes.
        for step in 0..200 {
            let id = step % 13;
            if step % 4 == 0 {
                items.remove(&key(id));
            } else {
                let start = ((step * 37) % 140) as i64;
                items.insert(key(id), (t(start), t(start + 1 + (step % 35) as i64)));
            }
            index.update(items.clone());
            assert_eq!(index.segments, oracle(&items), "mutation {step}");
        }
        index.update(BTreeMap::new());
        assert!(index.segments.is_empty());
    }
    #[test]
    fn isolated_interval_edit_does_not_rescan_unrelated_boundaries() {
        let mut items = (0..1000)
            .map(|i| (key(i), (t(i as i64 * 10), t(i as i64 * 10 + 5))))
            .collect::<BTreeMap<_, _>>();
        let mut index = TimelineIndex::default();
        index.update(items.clone());
        items.insert(key(500), (t(5001), t(5006)));
        assert!(index.update(items.clone()) < 10);
        assert_eq!(index.segments, oracle(&items));
    }
}
