use std::fmt;
use std::sync::Arc;

pub trait RelativeSemanticRecord: Clone + fmt::Debug + PartialEq + Eq {
    fn shift(&mut self, delta: isize);
}

#[derive(Clone)]
pub struct SemanticRecords<T> {
    storage: RecordStorage<T>,
}

#[derive(Clone)]
enum RecordStorage<T> {
    Empty,
    Owned(Arc<Vec<T>>),
    Segmented(Arc<[RecordSegment<T>]>),
}

#[derive(Debug, Clone)]
pub(crate) struct RecordSegment<T> {
    pub(crate) offset: isize,
    pub(crate) records: Arc<Vec<T>>,
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

impl<T: fmt::Debug> fmt::Debug for SemanticRecords<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.storage {
            RecordStorage::Empty => formatter.debug_tuple("SemanticRecords").finish(),
            RecordStorage::Owned(records) => formatter
                .debug_tuple("SemanticRecords")
                .field(records)
                .finish(),
            RecordStorage::Segmented(segments) => formatter
                .debug_struct("SemanticRecords")
                .field("segments", segments)
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
            RecordStorage::Segmented(segments) => {
                segments.iter().map(|segment| segment.records.len()).sum()
            }
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
            RecordStorage::Segmented(segments) => Box::new(segments.iter().flat_map(|segment| {
                segment
                    .records
                    .iter()
                    .map(move |record| SemanticRecordView {
                        record,
                        offset: segment.offset,
                    })
            })),
        }
    }

    pub(crate) fn push(&mut self, record: T) {
        Arc::make_mut(self.owned_mut()).push(record);
    }

    pub(crate) fn from_owned(records: Vec<T>) -> Self {
        if records.is_empty() {
            Self::default()
        } else {
            Self {
                storage: RecordStorage::Owned(Arc::new(records)),
            }
        }
    }

    pub(crate) fn sort_by_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        if matches!(self.storage, RecordStorage::Empty) {
            return;
        }
        Arc::make_mut(self.owned_mut()).sort_by_key(|record| key(record));
    }

    pub(crate) fn owned_arc(&self) -> Option<Arc<Vec<T>>> {
        match &self.storage {
            RecordStorage::Empty => None,
            RecordStorage::Owned(records) => Some(Arc::clone(records)),
            RecordStorage::Segmented(_) => {
                panic!("a segmented semantic collection cannot become a child segment")
            }
        }
    }

    pub(crate) fn from_segments(segments: Vec<RecordSegment<T>>) -> Self {
        Self {
            storage: RecordStorage::Segmented(segments.into()),
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
