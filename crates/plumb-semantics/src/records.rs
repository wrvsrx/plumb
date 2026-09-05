use std::fmt;
use std::sync::Arc;

use plumb_syntax::Diagnostic;

use crate::document::{SemanticNodeOutput, SemanticTree};

pub trait RelativeSemanticRecord: Clone + fmt::Debug + PartialEq + Eq {
    fn shift(&mut self, delta: isize);
}

#[derive(Clone)]
pub struct SemanticRecords<T> {
    storage: RecordStorage<T>,
}

#[derive(Clone)]
pub struct SemanticDiagnostics {
    storage: DiagnosticStorage,
}

#[derive(Clone)]
enum DiagnosticStorage {
    Empty,
    Owned(Arc<Vec<Diagnostic>>),
    Segmented(SegmentedDiagnosticStorage),
}

#[derive(Debug, Clone)]
struct SegmentedDiagnosticStorage {
    tree: Arc<SemanticTree>,
    segments: Arc<[DiagnosticSegment]>,
    diagnostics: fn(&SemanticNodeOutput) -> &SemanticDiagnostics,
    len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DiagnosticSegment {
    pub(crate) node_index: usize,
}

#[derive(Clone)]
enum RecordStorage<T> {
    Empty,
    Owned(Arc<Vec<T>>),
    Segmented(SegmentedRecordStorage<T>),
}

#[derive(Debug, Clone)]
struct SegmentedRecordStorage<T> {
    tree: Arc<SemanticTree>,
    segments: Arc<[RecordSegment]>,
    records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
    len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordSegment {
    pub(crate) node_index: usize,
}

#[derive(Debug)]
pub struct SemanticRecordView<'a, T> {
    pub(crate) record: &'a T,
    pub(crate) offset: isize,
}

impl<T> Clone for SemanticRecordView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SemanticRecordView<'_, T> {}

impl<T> Default for SemanticRecords<T> {
    fn default() -> Self {
        Self {
            storage: RecordStorage::Empty,
        }
    }
}

impl Default for SemanticDiagnostics {
    fn default() -> Self {
        Self {
            storage: DiagnosticStorage::Empty,
        }
    }
}

impl fmt::Debug for SemanticDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SemanticDiagnostics")
            .field(&self.iter().collect::<Vec<_>>())
            .finish()
    }
}

impl PartialEq for SemanticDiagnostics {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for SemanticDiagnostics {}

impl SemanticDiagnostics {
    pub fn len(&self) -> usize {
        match &self.storage {
            DiagnosticStorage::Empty => 0,
            DiagnosticStorage::Owned(diagnostics) => diagnostics.len(),
            DiagnosticStorage::Segmented(storage) => storage.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Diagnostic> {
        self.iter().nth(index)
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = Diagnostic> + '_> {
        match &self.storage {
            DiagnosticStorage::Empty => Box::new(std::iter::empty()),
            DiagnosticStorage::Owned(diagnostics) => Box::new(diagnostics.iter().cloned()),
            DiagnosticStorage::Segmented(storage) => {
                Box::new(storage.segments.iter().flat_map(|segment| {
                    let (offset, output) = storage.tree.record_node(segment.node_index);
                    (storage.diagnostics)(output)
                        .owned_diagnostics()
                        .expect("a diagnostic segment references owned local diagnostics")
                        .iter()
                        .cloned()
                        .map(move |mut diagnostic| {
                            shift_diagnostic(&mut diagnostic, offset);
                            diagnostic
                        })
                }))
            }
        }
    }

    pub fn to_vec(&self) -> Vec<Diagnostic> {
        self.iter().collect()
    }

    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        Arc::make_mut(self.owned_mut()).push(diagnostic);
    }

    pub(crate) fn from_segments(
        tree: Arc<SemanticTree>,
        segments: Vec<DiagnosticSegment>,
        diagnostics: fn(&SemanticNodeOutput) -> &SemanticDiagnostics,
        len: usize,
    ) -> Self {
        if len == 0 {
            return Self::default();
        }
        Self {
            storage: DiagnosticStorage::Segmented(SegmentedDiagnosticStorage {
                tree,
                segments: segments.into(),
                diagnostics,
                len,
            }),
        }
    }

    pub(crate) fn from_owned(diagnostics: Vec<Diagnostic>) -> Self {
        if diagnostics.is_empty() {
            Self::default()
        } else {
            Self {
                storage: DiagnosticStorage::Owned(Arc::new(diagnostics)),
            }
        }
    }

    pub(crate) fn rebind_tree(&self, tree: Arc<SemanticTree>) -> Option<Self> {
        match &self.storage {
            DiagnosticStorage::Empty => Some(Self::default()),
            DiagnosticStorage::Segmented(storage) => Some(Self {
                storage: DiagnosticStorage::Segmented(SegmentedDiagnosticStorage {
                    tree,
                    segments: Arc::clone(&storage.segments),
                    diagnostics: storage.diagnostics,
                    len: storage.len,
                }),
            }),
            DiagnosticStorage::Owned(_) => None,
        }
    }

    pub(crate) fn is_tree_rebindable(&self) -> bool {
        matches!(
            self.storage,
            DiagnosticStorage::Empty | DiagnosticStorage::Segmented(_)
        )
    }

    #[cfg(test)]
    pub(crate) fn shares_segment_topology(&self, other: &Self) -> bool {
        match (&self.storage, &other.storage) {
            (DiagnosticStorage::Empty, DiagnosticStorage::Empty) => true,
            (DiagnosticStorage::Segmented(left), DiagnosticStorage::Segmented(right)) => {
                Arc::ptr_eq(&left.segments, &right.segments)
            }
            (DiagnosticStorage::Empty | DiagnosticStorage::Owned(_), _)
            | (_, DiagnosticStorage::Empty | DiagnosticStorage::Owned(_)) => false,
        }
    }

    fn owned_diagnostics(&self) -> Option<&[Diagnostic]> {
        match &self.storage {
            DiagnosticStorage::Owned(diagnostics) => Some(diagnostics),
            DiagnosticStorage::Empty | DiagnosticStorage::Segmented(_) => None,
        }
    }

    fn owned_mut(&mut self) -> &mut Arc<Vec<Diagnostic>> {
        if matches!(self.storage, DiagnosticStorage::Empty) {
            self.storage = DiagnosticStorage::Owned(Arc::new(Vec::new()));
        }
        match &mut self.storage {
            DiagnosticStorage::Owned(diagnostics) => diagnostics,
            DiagnosticStorage::Empty => unreachable!("empty storage was initialized"),
            DiagnosticStorage::Segmented(_) => {
                panic!("cannot mutate analyzed segmented semantic diagnostics")
            }
        }
    }
}

fn shift_diagnostic(diagnostic: &mut Diagnostic, delta: isize) {
    shift_range(&mut diagnostic.range, delta);
    for related in &mut diagnostic.related {
        shift_range(related, delta);
    }
}

fn shift_range(range: &mut std::ops::Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}

impl<T: fmt::Debug> fmt::Debug for SemanticRecords<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.storage {
            RecordStorage::Empty => formatter.debug_tuple("SemanticRecords").finish(),
            RecordStorage::Owned(records) => formatter
                .debug_tuple("SemanticRecords")
                .field(records)
                .finish(),
            RecordStorage::Segmented(storage) => formatter
                .debug_struct("SemanticRecords")
                .field("segments", &storage.segments)
                .field("len", &storage.len)
                .finish(),
        }
    }
}

impl<T: RelativeSemanticRecord> PartialEq for SemanticRecords<T> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl<T: RelativeSemanticRecord> Eq for SemanticRecords<T> {}

impl<T: RelativeSemanticRecord> SemanticRecordView<'_, T> {
    pub fn to_owned(self) -> T {
        let mut record = self.record.clone();
        record.shift(self.offset);
        record
    }
}

impl<T: RelativeSemanticRecord> SemanticRecords<T> {
    pub fn len(&self) -> usize {
        match &self.storage {
            RecordStorage::Empty => 0,
            RecordStorage::Owned(records) => records.len(),
            RecordStorage::Segmented(storage) => storage.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<T> {
        self.views().nth(index).map(SemanticRecordView::to_owned)
    }

    pub fn first(&self) -> Option<T> {
        self.get(0)
    }

    pub fn last(&self) -> Option<T> {
        self.views().last().map(SemanticRecordView::to_owned)
    }

    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        self.views().map(SemanticRecordView::to_owned)
    }

    pub fn views(&self) -> Box<dyn Iterator<Item = SemanticRecordView<'_, T>> + '_> {
        match &self.storage {
            RecordStorage::Empty => Box::new(std::iter::empty()),
            RecordStorage::Owned(records) => Box::new(
                records
                    .iter()
                    .map(|record| SemanticRecordView { record, offset: 0 }),
            ),
            RecordStorage::Segmented(storage) => {
                Box::new(storage.segments.iter().flat_map(|segment| {
                    let (offset, output) = storage.tree.record_node(segment.node_index);
                    (storage.records)(output)
                        .owned_records()
                        .expect("a projection segment references owned local records")
                        .iter()
                        .map(move |record| SemanticRecordView { record, offset })
                }))
            }
        }
    }

    pub(crate) fn push(&mut self, record: T) {
        Arc::make_mut(self.owned_mut()).push(record);
    }

    pub(crate) fn sort_by_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        if matches!(self.storage, RecordStorage::Empty) {
            return;
        }
        Arc::make_mut(self.owned_mut()).sort_by_key(|record| key(record));
    }

    pub(crate) fn from_segments(
        tree: Arc<SemanticTree>,
        segments: Vec<RecordSegment>,
        records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
        len: usize,
    ) -> Self {
        if len == 0 {
            return Self::default();
        }
        Self {
            storage: RecordStorage::Segmented(SegmentedRecordStorage {
                tree,
                segments: segments.into(),
                records,
                len,
            }),
        }
    }

    pub(crate) fn rebind_tree(&self, tree: Arc<SemanticTree>) -> Option<Self> {
        match &self.storage {
            RecordStorage::Empty => Some(Self::default()),
            RecordStorage::Segmented(storage) => Some(Self {
                storage: RecordStorage::Segmented(SegmentedRecordStorage {
                    tree,
                    segments: Arc::clone(&storage.segments),
                    records: storage.records,
                    len: storage.len,
                }),
            }),
            RecordStorage::Owned(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_segment_topology(&self, other: &Self) -> bool {
        match (&self.storage, &other.storage) {
            (RecordStorage::Empty, RecordStorage::Empty) => true,
            (RecordStorage::Segmented(left), RecordStorage::Segmented(right)) => {
                Arc::ptr_eq(&left.segments, &right.segments)
            }
            (RecordStorage::Empty | RecordStorage::Owned(_), _)
            | (_, RecordStorage::Empty | RecordStorage::Owned(_)) => false,
        }
    }

    pub(crate) fn owned_arc(&self) -> Option<Arc<Vec<T>>> {
        match &self.storage {
            RecordStorage::Empty => None,
            RecordStorage::Owned(records) => Some(Arc::clone(records)),
            RecordStorage::Segmented(_) => None,
        }
    }

    fn owned_records(&self) -> Option<&[T]> {
        match &self.storage {
            RecordStorage::Owned(records) => Some(records),
            RecordStorage::Empty | RecordStorage::Segmented(_) => None,
        }
    }

    fn owned_mut(&mut self) -> &mut Arc<Vec<T>> {
        if matches!(self.storage, RecordStorage::Empty) {
            self.storage = RecordStorage::Owned(Arc::new(Vec::new()));
        }
        match &mut self.storage {
            RecordStorage::Empty => unreachable!("empty storage was initialized"),
            RecordStorage::Owned(records) => records,
            RecordStorage::Segmented(_) => {
                panic!("cannot mutate an analyzed segmented semantic collection")
            }
        }
    }
}

impl<'a, T: RelativeSemanticRecord> IntoIterator for &'a SemanticRecords<T> {
    type Item = T;
    type IntoIter = Box<dyn Iterator<Item = T> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}
