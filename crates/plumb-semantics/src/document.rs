use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use plumb_syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent, ValidDocument, ValidGreenDocument,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::lists::{ListGroupSegment, ReducedListGroup};
use crate::records::RecordSegment;
use crate::{
    analyze_citations, analyze_events, analyze_headings, analyze_inline_styles, analyze_lists,
    analyze_math, analyze_quotes, analyze_tables, analyze_tasks, CitationOutput, EventOutput,
    HeadingOutput, InlineStyleOutput, ListGroups, ListKind, ListOutput, MathOutput, MetadataOutput,
    QuoteOutput, RelativeSemanticRecord, SemanticRecords, TableOutput, TaskOutput,
};
use crate::{headings::analyze_green_headings, metadata::analyze_green_metadata};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBacked<T> {
    pub value: T,
    pub raw: String,
    pub range: Range<usize>,
    decoded_boundaries: Vec<usize>,
}

impl SourceBacked<String> {
    pub fn source_range(&self, decoded: Range<usize>) -> Option<Range<usize>> {
        Some(
            *self.decoded_boundaries.get(decoded.start)?
                ..*self.decoded_boundaries.get(decoded.end)?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKind {
    Heading,
    Block,
    Inline,
    VerbatimBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorRecord {
    pub id: SourceBacked<String>,
    pub kind: AnchorKind,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

impl RelativeSemanticRecord for AnchorRecord {
    fn shift(&mut self, delta: isize) {
        shift_source_backed(&mut self.id, delta);
        shift_range(&mut self.range, delta);
        shift_range(&mut self.selection_range, delta);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkTarget {
    Anchor {
        path: Option<String>,
        fragment: String,
    },
    Document {
        path: String,
    },
    External,
    File {
        path: String,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkSpelling {
    Positional,
    Verbatim {
        envelope: Range<usize>,
        quote_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub target: SourceBacked<String>,
    pub target_kind: LinkTarget,
    pub spelling: LinkSpelling,
    pub target_range: Range<usize>,
    pub target_element_count: usize,
    pub target_declaration_ranges: Vec<Range<usize>>,
    pub path_range: Option<Range<usize>>,
    pub fragment_range: Option<Range<usize>>,
}

impl RelativeSemanticRecord for LinkRecord {
    fn shift(&mut self, delta: isize) {
        shift_range(&mut self.range, delta);
        shift_range(&mut self.selection_range, delta);
        shift_source_backed(&mut self.target, delta);
        if let LinkSpelling::Verbatim { envelope, .. } = &mut self.spelling {
            shift_range(envelope, delta);
        }
        shift_range(&mut self.target_range, delta);
        for range in &mut self.target_declaration_ranges {
            shift_range(range, delta);
        }
        for range in [&mut self.path_range, &mut self.fragment_range]
            .into_iter()
            .flatten()
        {
            shift_range(range, delta);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageTarget {
    External,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub source: SourceBacked<String>,
    pub target_kind: ImageTarget,
}

impl RelativeSemanticRecord for ImageRecord {
    fn shift(&mut self, delta: isize) {
        shift_range(&mut self.range, delta);
        shift_range(&mut self.selection_range, delta);
        shift_source_backed(&mut self.source, delta);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTarget {
    External,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub source: SourceBacked<String>,
    pub target_kind: FileTarget,
}

impl RelativeSemanticRecord for FileRecord {
    fn shift(&mut self, delta: isize) {
        shift_range(&mut self.range, delta);
        shift_range(&mut self.selection_range, delta);
        shift_source_backed(&mut self.source, delta);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLinkRange {
    pub event_start: usize,
    pub links: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct DocumentOutput {
    root: Arc<SemanticRoot>,
}

#[derive(Debug, Clone)]
pub struct SemanticRoot {
    tree: Arc<SemanticTree>,
    document_declaration_end: usize,
    pub(crate) headings: HeadingOutput,
    pub(crate) metadata: MetadataOutput,
    pub(crate) citations: CitationOutput,
    pub(crate) inline_styles: InlineStyleOutput,
    pub(crate) lists: ListOutput,
    pub(crate) math: MathOutput,
    pub(crate) quotes: QuoteOutput,
    pub(crate) tasks: TaskOutput,
    pub(crate) events: EventOutput,
    pub(crate) tables: TableOutput,
    pub(crate) anchors: SemanticRecords<AnchorRecord>,
    pub(crate) links: SemanticRecords<LinkRecord>,
    first_link_start: Option<usize>,
    pub(crate) images: SemanticRecords<ImageRecord>,
    pub(crate) files: SemanticRecords<FileRecord>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct SemanticTree {
    syntax: Arc<plumb_syntax::GreenDocument>,
    nodes: Vec<SemanticNode>,
    cache_hits: usize,
}

impl SemanticTree {
    pub(crate) fn record_node(&self, node_index: usize) -> (isize, &SemanticNodeOutput) {
        let node = &self.nodes[node_index];
        (node.offset as isize, &node.output)
    }

    pub(crate) fn list_group_segment(
        &self,
        node_index: usize,
        group_index: usize,
    ) -> (isize, &crate::ListGroup) {
        let node = &self.nodes[node_index];
        (
            node.offset as isize,
            node.output
                .lists
                .groups
                .owned_group(group_index)
                .expect("a reduced list segment references local owned storage"),
        )
    }
}

#[derive(Debug, Clone)]
struct SemanticNode {
    syntax: Arc<plumb_syntax::GreenShard>,
    offset: usize,
    output: Arc<SemanticNodeOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticNodeOutput {
    citations: CitationOutput,
    inline_styles: InlineStyleOutput,
    math: MathOutput,
    quotes: QuoteOutput,
    tasks: TaskOutput,
    events: EventOutput,
    lists: ListOutput,
    tables: TableOutput,
    records: RecordOutput,
    association_diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct RecordProjectionIndex {
    spans: Vec<Range<usize>>,
    len: usize,
}

#[derive(Default)]
struct RootProjectionIndex {
    citations: RecordProjectionIndex,
    inline_styles: RecordProjectionIndex,
    math: RecordProjectionIndex,
    quotes: RecordProjectionIndex,
    tasks: RecordProjectionIndex,
    events: RecordProjectionIndex,
    tables: RecordProjectionIndex,
    anchors: RecordProjectionIndex,
    links: RecordProjectionIndex,
    images: RecordProjectionIndex,
    files: RecordProjectionIndex,
    citation_diagnostics: Vec<Diagnostic>,
    math_diagnostics: Vec<Diagnostic>,
    task_diagnostics: Vec<Diagnostic>,
    event_diagnostics: Vec<Diagnostic>,
    table_diagnostics: Vec<Diagnostic>,
    record_diagnostics: Vec<Diagnostic>,
    association_diagnostics: Vec<Diagnostic>,
}

impl Default for SemanticRoot {
    fn default() -> Self {
        Self {
            tree: Arc::new(SemanticTree::empty()),
            document_declaration_end: 0,
            headings: HeadingOutput::default(),
            metadata: MetadataOutput::default(),
            citations: CitationOutput::default(),
            inline_styles: InlineStyleOutput::default(),
            lists: ListOutput::default(),
            math: MathOutput::default(),
            quotes: QuoteOutput::default(),
            tasks: TaskOutput::default(),
            events: EventOutput::default(),
            tables: TableOutput::default(),
            anchors: SemanticRecords::default(),
            links: SemanticRecords::default(),
            first_link_start: None,
            images: SemanticRecords::default(),
            files: SemanticRecords::default(),
            diagnostics: Vec::new(),
        }
    }
}

impl SemanticTree {
    fn empty() -> Self {
        Self {
            syntax: Arc::new(plumb_syntax::GreenDocument::parse(String::new())),
            nodes: Vec::new(),
            cache_hits: 0,
        }
    }
}

impl PartialEq for DocumentOutput {
    fn eq(&self, other: &Self) -> bool {
        self.headings() == other.headings()
            && self.metadata() == other.metadata()
            && self.citations() == other.citations()
            && self.inline_styles() == other.inline_styles()
            && self.lists() == other.lists()
            && self.math() == other.math()
            && self.quotes() == other.quotes()
            && self.tasks() == other.tasks()
            && self.events() == other.events()
            && self.tables() == other.tables()
            && self.anchors() == other.anchors()
            && self.links() == other.links()
            && self.images() == other.images()
            && self.files() == other.files()
            && self.diagnostics() == other.diagnostics()
    }
}

impl Eq for DocumentOutput {}

impl Default for DocumentOutput {
    fn default() -> Self {
        Self {
            root: Arc::new(SemanticRoot::default()),
        }
    }
}

impl std::ops::Deref for DocumentOutput {
    type Target = SemanticRoot;

    fn deref(&self) -> &Self::Target {
        &self.root
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RecordOutput {
    anchors: SemanticRecords<AnchorRecord>,
    links: SemanticRecords<LinkRecord>,
    images: SemanticRecords<ImageRecord>,
    files: SemanticRecords<FileRecord>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentChange {
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
}

impl DocumentOutput {
    pub fn headings(&self) -> &HeadingOutput {
        &self.root.headings
    }

    pub fn metadata(&self) -> &MetadataOutput {
        &self.root.metadata
    }

    pub fn citations(&self) -> &CitationOutput {
        &self.root.citations
    }

    pub fn inline_styles(&self) -> &InlineStyleOutput {
        &self.root.inline_styles
    }

    pub fn lists(&self) -> &ListOutput {
        &self.root.lists
    }

    pub fn math(&self) -> &MathOutput {
        &self.root.math
    }

    pub fn quotes(&self) -> &QuoteOutput {
        &self.root.quotes
    }

    pub fn tasks(&self) -> &TaskOutput {
        &self.root.tasks
    }

    pub fn events(&self) -> &EventOutput {
        &self.root.events
    }

    pub fn tables(&self) -> &TableOutput {
        &self.root.tables
    }

    pub fn anchors(&self) -> &SemanticRecords<AnchorRecord> {
        &self.root.anchors
    }

    pub fn links(&self) -> &SemanticRecords<LinkRecord> {
        &self.root.links
    }

    pub fn event_link_ranges(&self) -> Vec<EventLinkRange> {
        build_event_link_ranges(&self.root.events.events, &self.root.links)
    }

    pub fn images(&self) -> &SemanticRecords<ImageRecord> {
        &self.root.images
    }

    pub fn files(&self) -> &SemanticRecords<FileRecord> {
        &self.root.files
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.root.diagnostics
    }

    pub fn semantic_node_count(&self) -> usize {
        self.root.tree.nodes.len()
    }

    pub fn reused_semantic_node_count(&self) -> usize {
        self.root.tree.cache_hits
    }

    pub fn link_at_node_start(&self, start: usize) -> Option<LinkRecord> {
        self.links.iter().find(|link| link.range.start == start)
    }

    pub fn image_at_node_start(&self, start: usize) -> Option<ImageRecord> {
        self.images.iter().find(|image| image.range.start == start)
    }

    pub fn file_at_node_start(&self, start: usize) -> Option<FileRecord> {
        self.files.iter().find(|file| file.range.start == start)
    }

    pub fn links_contained_by_event(&self, event_start: usize) -> Option<Vec<LinkRecord>> {
        let node_index = self
            .root
            .tree
            .nodes
            .partition_point(|node| node.offset <= event_start)
            .checked_sub(1)?;
        let node = &self.root.tree.nodes[node_index];
        let local_start = event_start.checked_sub(node.offset)?;
        let event = node
            .output
            .events
            .events
            .iter()
            .find(|event| event.range.start == local_start)?;
        self.links_contained_by_range(
            &(event.range.start + node.offset..event.range.end + node.offset),
        )
    }

    pub fn links_contained_by_range(&self, event: &Range<usize>) -> Option<Vec<LinkRecord>> {
        let node_index = self
            .root
            .tree
            .nodes
            .partition_point(|node| node.offset <= event.start)
            .checked_sub(1)?;
        let node = &self.root.tree.nodes[node_index];
        let local = event.start.checked_sub(node.offset)?..event.end.checked_sub(node.offset)?;
        if local.end > node.syntax.parsed().source.len() {
            return None;
        }
        if self
            .root
            .first_link_start
            .is_none_or(|first| event.end <= first)
        {
            return Some(Vec::new());
        }
        Some(
            node.output
                .records
                .links
                .iter()
                .filter(|link| local.start <= link.range.start && link.range.end <= local.end)
                .map(|mut link| {
                    link.shift(node.offset as isize);
                    link
                })
                .collect(),
        )
    }

    pub fn links_contained_by_record(&self, event: &crate::EventRecord) -> Vec<LinkRecord> {
        if self
            .root
            .first_link_start
            .is_none_or(|first| event.range.end <= first)
        {
            return Vec::new();
        }
        self.links_contained_by_range(&event.range)
            .unwrap_or_default()
    }
}

pub fn analyze_document(valid: ValidDocument<'_>) -> DocumentOutput {
    let syntax = Arc::new(plumb_syntax::GreenDocument::parse(
        valid.source().to_string(),
    ));
    analyze_semantic_tree(syntax, None, None)
        .expect("a valid document produces a valid semantic tree")
}

pub fn analyze_green_document(
    valid: ValidGreenDocument<'_>,
    syntax: Arc<plumb_syntax::GreenDocument>,
) -> Option<DocumentOutput> {
    std::ptr::eq(valid.syntax(), syntax.as_ref())
        .then(|| analyze_semantic_tree(syntax, None, None))?
}

pub fn analyze_document_incremental(
    valid: ValidDocument<'_>,
    previous: &DocumentOutput,
    change: &DocumentChange,
) -> DocumentOutput {
    let revision = previous.root.tree.syntax.reparse_from_change(
        valid.source().to_string(),
        plumb_syntax::SourceChange {
            old_range: change.old_range.clone(),
            new_range: change.new_range.clone(),
        },
    );
    let tree_change = DocumentChange {
        old_range: revision.old_reparsed_range,
        new_range: revision.reparsed_range,
    };
    let syntax = Arc::new(revision.document);
    analyze_semantic_tree(syntax, Some(previous), Some(&tree_change))
        .expect("a valid document produces a valid semantic tree")
}

pub fn analyze_green_document_incremental(
    valid: ValidGreenDocument<'_>,
    syntax: Arc<plumb_syntax::GreenDocument>,
    previous: &DocumentOutput,
    change: &DocumentChange,
) -> Option<DocumentOutput> {
    std::ptr::eq(valid.syntax(), syntax.as_ref())
        .then(|| analyze_semantic_tree(syntax, Some(previous), Some(change)))?
}

fn analyze_semantic_tree(
    syntax: Arc<plumb_syntax::GreenDocument>,
    previous: Option<&DocumentOutput>,
    change: Option<&DocumentChange>,
) -> Option<DocumentOutput> {
    let valid = syntax.valid_syntax()?;
    let reusable_metadata =
        previous.filter(|previous| can_reuse_metadata(previous, &syntax, change));
    let metadata = reusable_metadata
        .map(|previous| previous.metadata().clone())
        .unwrap_or_else(|| analyze_green_metadata(valid));
    let document_declaration_end = reusable_metadata.map_or_else(
        || document_declaration_end(&syntax),
        |previous| previous.root.document_declaration_end,
    );
    let headings = previous
        .filter(|previous| {
            previous.headings().headings.is_empty()
                && !changed_green_blocks(&syntax, change).any(block_contains_heading)
        })
        .map(|previous| previous.headings().clone())
        .unwrap_or_else(|| analyze_green_headings(valid));
    let reusable = previous.filter(|previous| previous.metadata() == &metadata);
    let reusable_nodes = reusable_node_indices(reusable, &syntax, change);
    let previous_nodes = previous.map(|previous| previous.root.tree.nodes.as_slice());
    let same_node_count = previous_nodes.is_some_and(|nodes| nodes.len() == reusable_nodes.len());
    let mut projections_rebindable = same_node_count
        && previous.is_some_and(|previous| {
            previous.root.citations.diagnostics.is_empty()
                && previous.root.math.diagnostics.is_empty()
                && previous.root.tasks.diagnostics.is_empty()
                && previous.root.events.diagnostics.is_empty()
                && previous.root.tables.diagnostics.is_empty()
                && previous.root.diagnostics.is_empty()
        });
    let mut anchor_ids_rebindable = same_node_count
        && previous.is_some_and(|previous| {
            !previous
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "anchor.duplicate-id")
        });
    let mut lists_rebindable = same_node_count;
    let mut cache_hits = 0;
    let nodes = syntax
        .shards()
        .enumerate()
        .map(|(node_index, view)| {
            let node_syntax = Arc::clone(view.shard());
            let output = reusable_nodes[node_index]
                .map(|previous_index| {
                    Arc::clone(
                        &reusable
                            .expect("reusable node index requires a previous tree")
                            .root
                            .tree
                            .nodes[previous_index]
                            .output,
                    )
                })
                .map(|output| {
                    cache_hits += 1;
                    output
                })
                .unwrap_or_else(|| {
                    let local = node_syntax
                        .parsed()
                        .valid_syntax()
                        .expect("valid green document has valid shards");
                    let local_headings = analyze_headings(local);
                    Arc::new(SemanticNodeOutput {
                        citations: analyze_citations(local),
                        inline_styles: analyze_inline_styles(local),
                        math: analyze_math(local),
                        quotes: analyze_quotes(local),
                        tasks: analyze_tasks(local),
                        events: analyze_events(local, &metadata),
                        lists: analyze_lists(local),
                        tables: analyze_tables(local),
                        records: collect_document_records(
                            local.source(),
                            local.syntax(),
                            &local_headings,
                        ),
                        association_diagnostics: association_arity_diagnostics(local.syntax()),
                    })
                });
            if let Some(previous_node) = previous_nodes.and_then(|nodes| nodes.get(node_index)) {
                let exact_reuse = reusable_nodes[node_index] == Some(node_index);
                projections_rebindable &= exact_reuse
                    || (same_record_counts(&previous_node.output, &output)
                        && node_output_diagnostics_empty(&output));
                anchor_ids_rebindable &= exact_reuse
                    || previous_node
                        .output
                        .records
                        .anchors
                        .iter()
                        .map(|anchor| anchor.id.value)
                        .eq(output.records.anchors.iter().map(|anchor| anchor.id.value));
                lists_rebindable &= exact_reuse
                    || (list_topology_eq(&previous_node.output.lists, &output.lists)
                        && root_list_role(&previous_node.syntax) == root_list_role(&node_syntax));
            } else {
                projections_rebindable = false;
                anchor_ids_rebindable = false;
                lists_rebindable = false;
            }
            SemanticNode {
                syntax: node_syntax,
                offset: view.offset(),
                output,
            }
        })
        .collect::<Vec<_>>();
    let projection_source = previous.filter(|_| projections_rebindable);
    let mut projections = projection_source
        .map(|_| RootProjectionIndex::default())
        .unwrap_or_else(|| RootProjectionIndex::build(&nodes));
    let tree = Arc::new(SemanticTree {
        syntax,
        nodes,
        cache_hits,
    });

    let citations = CitationOutput {
        citations: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.citations.citations),
            std::mem::take(&mut projections.citations),
            |output| &output.citations.citations,
        ),
        diagnostics: std::mem::take(&mut projections.citation_diagnostics),
    };
    let inline_styles = InlineStyleOutput {
        styles: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.inline_styles.styles),
            std::mem::take(&mut projections.inline_styles),
            |output| &output.inline_styles.styles,
        ),
    };
    let math = MathOutput {
        records: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.math.records),
            std::mem::take(&mut projections.math),
            |output| &output.math.records,
        ),
        diagnostics: std::mem::take(&mut projections.math_diagnostics),
    };
    let quotes = QuoteOutput {
        quotes: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.quotes.quotes),
            std::mem::take(&mut projections.quotes),
            |output| &output.quotes.quotes,
        ),
    };
    let tasks = TaskOutput {
        tasks: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.tasks.tasks),
            std::mem::take(&mut projections.tasks),
            |output| &output.tasks.tasks,
        ),
        diagnostics: std::mem::take(&mut projections.task_diagnostics),
    };
    let events = EventOutput {
        events: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.events.events),
            std::mem::take(&mut projections.events),
            |output| &output.events.events,
        ),
        diagnostics: std::mem::take(&mut projections.event_diagnostics),
    };
    let tables = TableOutput {
        tables: projected_records(
            &tree,
            projection_source.map(|previous| &previous.root.tables.tables),
            std::mem::take(&mut projections.tables),
            |output| &output.tables.tables,
        ),
        diagnostics: std::mem::take(&mut projections.table_diagnostics),
    };
    let anchors = projected_records(
        &tree,
        projection_source.map(|previous| &previous.root.anchors),
        std::mem::take(&mut projections.anchors),
        |output| &output.records.anchors,
    );
    let links = projected_records(
        &tree,
        projection_source.map(|previous| &previous.root.links),
        std::mem::take(&mut projections.links),
        |output| &output.records.links,
    );
    let images = projected_records(
        &tree,
        projection_source.map(|previous| &previous.root.images),
        std::mem::take(&mut projections.images),
        |output| &output.records.images,
    );
    let files = projected_records(
        &tree,
        projection_source.map(|previous| &previous.root.files),
        std::mem::take(&mut projections.files),
        |output| &output.records.files,
    );
    let first_link_start = links.first().map(|link| link.range.start);
    let mut record_diagnostics = std::mem::take(&mut projections.record_diagnostics);
    record_diagnostics.retain(|diagnostic| diagnostic.code != "anchor.duplicate-id");
    let duplicate_ids_still_absent = anchor_ids_rebindable;
    if !duplicate_ids_still_absent {
        let absolute_anchors = anchors.iter().collect::<Vec<_>>();
        append_duplicate_anchor_diagnostics(&absolute_anchors, &mut record_diagnostics);
    }
    record_diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.code,
        )
    });
    let lists = previous
        .filter(|_| lists_rebindable)
        .and_then(|previous| rebind_lists(previous.lists(), &tree))
        .unwrap_or_else(|| reduce_lists(&tree));
    let mut diagnostics = std::mem::take(&mut projections.association_diagnostics);
    diagnostics.extend(record_diagnostics.iter().cloned());
    diagnostics.extend(tables.diagnostics.iter().cloned());
    diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.code,
        )
    });

    Some(DocumentOutput {
        root: Arc::new(SemanticRoot {
            tree,
            document_declaration_end,
            headings,
            metadata,
            citations,
            inline_styles,
            lists,
            math,
            quotes,
            tasks,
            events,
            tables,
            anchors,
            links,
            first_link_start,
            images,
            files,
            diagnostics,
        }),
    })
}

fn reusable_node_indices(
    previous: Option<&DocumentOutput>,
    syntax: &plumb_syntax::GreenDocument,
    change: Option<&DocumentChange>,
) -> Vec<Option<usize>> {
    let node_count = syntax.shards().len();
    let (Some(previous), Some(change)) = (previous, change) else {
        return vec![None; node_count];
    };
    let previous_nodes = &previous.root.tree.nodes;
    let previous_prefix = previous_nodes
        .iter()
        .take_while(|node| {
            node.offset + node.syntax.parsed().source.len() <= change.old_range.start
        })
        .count();
    let current_prefix = syntax
        .shards()
        .take_while(|view| view.range().end <= change.new_range.start)
        .count();
    let previous_suffix = previous_nodes.partition_point(|node| node.offset < change.old_range.end);
    let current_suffix = syntax
        .shards()
        .take_while(|view| view.range().start < change.new_range.end)
        .count();
    if previous_prefix != current_prefix
        || previous_nodes.len() - previous_suffix != node_count - current_suffix
    {
        return vec![None; node_count];
    }

    syntax
        .shards()
        .enumerate()
        .map(|(index, view)| {
            let previous_index = if index < current_prefix {
                Some(index)
            } else if index >= current_suffix {
                Some(previous_suffix + index - current_suffix)
            } else {
                None
            }?;
            Arc::ptr_eq(view.shard(), &previous_nodes[previous_index].syntax)
                .then_some(previous_index)
        })
        .collect()
}

fn document_declaration_end(syntax: &plumb_syntax::GreenDocument) -> usize {
    syntax
        .shards()
        .filter(|view| {
            view.shard()
                .parsed()
                .syntax
                .blocks
                .first()
                .is_some_and(crate::is_document_declaration)
        })
        .map(|view| view.range().end)
        .max()
        .unwrap_or(0)
}

fn can_reuse_metadata(
    previous: &DocumentOutput,
    syntax: &plumb_syntax::GreenDocument,
    change: Option<&DocumentChange>,
) -> bool {
    let Some(change) = change else {
        return false;
    };
    if change.old_range.start < previous.root.document_declaration_end {
        return false;
    }
    !changed_green_blocks(syntax, Some(change)).any(crate::is_document_declaration)
}

fn changed_green_blocks<'a>(
    syntax: &'a plumb_syntax::GreenDocument,
    change: Option<&DocumentChange>,
) -> impl Iterator<Item = &'a Block> {
    let range = change
        .map(|change| change.new_range.clone())
        .unwrap_or(0..syntax.source().len());
    syntax
        .shards()
        .filter(move |view| view.range().start < range.end && range.start < view.range().end)
        .filter_map(|view| view.shard().parsed().syntax.blocks.first())
}

fn block_contains_heading(block: &Block) -> bool {
    let mut pending = vec![block];
    while let Some(block) = pending.pop() {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block.mark.as_ref().is_some_and(|mark| {
            let count = mark.marker.bytes().take_while(|byte| *byte == b'#').count();
            count == mark.marker.len() && (1..=6).contains(&count)
        }) {
            return true;
        }
        pending.extend(crate::body_children(block));
    }
    false
}

impl RecordProjectionIndex {
    fn add(&mut self, node_index: usize, records: usize) {
        if records == 0 {
            return;
        }
        self.len += records;
        match self.spans.last_mut() {
            Some(span) if span.end == node_index => span.end += 1,
            _ => self.spans.push(node_index..node_index + 1),
        }
    }
}

fn same_record_counts(previous: &SemanticNodeOutput, current: &SemanticNodeOutput) -> bool {
    previous.citations.citations.len() == current.citations.citations.len()
        && previous.inline_styles.styles.len() == current.inline_styles.styles.len()
        && previous.math.records.len() == current.math.records.len()
        && previous.quotes.quotes.len() == current.quotes.quotes.len()
        && previous.tasks.tasks.len() == current.tasks.tasks.len()
        && previous.events.events.len() == current.events.events.len()
        && previous.tables.tables.len() == current.tables.tables.len()
        && previous.records.anchors.len() == current.records.anchors.len()
        && previous.records.links.len() == current.records.links.len()
        && previous.records.images.len() == current.records.images.len()
        && previous.records.files.len() == current.records.files.len()
}

fn node_output_diagnostics_empty(output: &SemanticNodeOutput) -> bool {
    output.citations.diagnostics.is_empty()
        && output.math.diagnostics.is_empty()
        && output.tasks.diagnostics.is_empty()
        && output.events.diagnostics.is_empty()
        && output.tables.diagnostics.is_empty()
        && output.records.diagnostics.is_empty()
        && output.association_diagnostics.is_empty()
}

impl RootProjectionIndex {
    fn build(nodes: &[SemanticNode]) -> Self {
        let mut output = Self::default();
        for (index, node) in nodes.iter().enumerate() {
            let local = &node.output;
            output.citations.add(index, local.citations.citations.len());
            output
                .inline_styles
                .add(index, local.inline_styles.styles.len());
            output.math.add(index, local.math.records.len());
            output.quotes.add(index, local.quotes.quotes.len());
            output.tasks.add(index, local.tasks.tasks.len());
            output.events.add(index, local.events.events.len());
            output.tables.add(index, local.tables.tables.len());
            output.anchors.add(index, local.records.anchors.len());
            output.links.add(index, local.records.links.len());
            output.images.add(index, local.records.images.len());
            output.files.add(index, local.records.files.len());
            append_projected_diagnostics(
                &mut output.citation_diagnostics,
                &local.citations.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.math_diagnostics,
                &local.math.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.task_diagnostics,
                &local.tasks.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.event_diagnostics,
                &local.events.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.table_diagnostics,
                &local.tables.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.record_diagnostics,
                &local.records.diagnostics,
                node.offset,
            );
            append_projected_diagnostics(
                &mut output.association_diagnostics,
                &local.association_diagnostics,
                node.offset,
            );
        }
        for diagnostics in [
            &mut output.citation_diagnostics,
            &mut output.math_diagnostics,
            &mut output.task_diagnostics,
            &mut output.event_diagnostics,
            &mut output.table_diagnostics,
            &mut output.record_diagnostics,
            &mut output.association_diagnostics,
        ] {
            diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        }
        output
    }
}

fn append_projected_diagnostics(
    output: &mut Vec<Diagnostic>,
    diagnostics: &[Diagnostic],
    offset: usize,
) {
    if diagnostics.is_empty() {
        return;
    }
    let start = output.len();
    output.extend_from_slice(diagnostics);
    shift_diagnostics(&mut output[start..], offset as isize);
}

fn projected_records<T: RelativeSemanticRecord>(
    tree: &Arc<SemanticTree>,
    previous: Option<&SemanticRecords<T>>,
    index: RecordProjectionIndex,
    records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
) -> SemanticRecords<T> {
    if let Some(previous) = previous.and_then(|previous| previous.rebind_tree(Arc::clone(tree))) {
        return previous;
    }
    let segments = index
        .spans
        .iter()
        .flat_map(|span| span.clone())
        .map(|node_index| {
            debug_assert!(records(&tree.nodes[node_index].output)
                .owned_arc()
                .is_some());
            RecordSegment { node_index }
        })
        .collect();
    SemanticRecords::from_segments(Arc::clone(tree), segments, records, index.len)
}

fn reduce_lists(tree: &Arc<SemanticTree>) -> ListOutput {
    let mut groups = Vec::new();
    let mut pending: Option<ReducedListGroup> = None;
    for (node_index, node) in tree.nodes.iter().enumerate() {
        let local_storage = node.output.lists.groups.owned_groups();
        match root_list_role(&node.syntax) {
            RootListRole::Transparent => {
                append_local_list_groups(&mut groups, tree, node_index, node, local_storage, None)
            }
            RootListRole::List(kind) => {
                let root_start = node.offset
                    + node
                        .syntax
                        .parsed()
                        .syntax
                        .blocks
                        .first()
                        .expect("list role has a root block")
                        .range()
                        .start;
                let storage =
                    local_storage.expect("top-level list item produces local list storage");
                let root_index = storage
                    .iter()
                    .position(|group| group.range.start + node.offset == root_start)
                    .expect("top-level list item produces a root group");
                let root = &storage[root_index];
                let segment = ListGroupSegment {
                    nodes: node_index..node_index + 1,
                    group_index: root_index,
                };
                match &mut pending {
                    Some(current) if current.kind == kind => {
                        current.range.end = root.range.end + node.offset;
                        if let Some(last) = current.segments.last_mut().filter(|last| {
                            last.group_index == root_index && last.nodes.end == node_index
                        }) {
                            last.nodes.end += 1;
                        } else {
                            current.segments.push(segment);
                        }
                    }
                    Some(_) => {
                        groups.push(pending.take().expect("pending group exists"));
                        pending = Some(reduced_list_group(
                            tree, node_index, node, storage, root_index,
                        ));
                    }
                    None => {
                        pending = Some(reduced_list_group(
                            tree, node_index, node, storage, root_index,
                        ))
                    }
                }
                append_local_list_groups(
                    &mut groups,
                    tree,
                    node_index,
                    node,
                    Some(storage),
                    Some(root_index),
                );
            }
            RootListRole::Other => {
                if let Some(group) = pending.take() {
                    groups.push(group);
                }
                append_local_list_groups(&mut groups, tree, node_index, node, local_storage, None);
            }
        }
    }
    if let Some(group) = pending {
        groups.push(group);
    }
    groups.sort_by_key(|group| group.range.start);
    ListOutput {
        groups: ListGroups::from_reduced(groups),
    }
}

fn list_topology_eq(previous: &ListOutput, current: &ListOutput) -> bool {
    previous.groups.len() == current.groups.len()
        && previous
            .groups
            .iter()
            .zip(current.groups.iter())
            .all(|(previous, current)| {
                previous.kind == current.kind && previous.items.len() == current.items.len()
            })
}

fn rebind_lists(previous: &ListOutput, tree: &Arc<SemanticTree>) -> Option<ListOutput> {
    let groups = previous.groups.reduced_groups()?;
    let groups = groups
        .iter()
        .map(|group| {
            let first = group.segments.first()?;
            let last = group.segments.last()?;
            let first_node = first.nodes.start;
            let last_node = last.nodes.end.checked_sub(1)?;
            let (first_offset, first_group) =
                tree.list_group_segment(first_node, first.group_index);
            let (last_offset, last_group) = tree.list_group_segment(last_node, last.group_index);
            Some(ReducedListGroup {
                range: first_group.range.start.checked_add_signed(first_offset)?
                    ..last_group.range.end.checked_add_signed(last_offset)?,
                kind: group.kind,
                tree: Arc::clone(tree),
                segments: group.segments.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ListOutput {
        groups: ListGroups::from_reduced(groups),
    })
}

fn append_local_list_groups(
    output: &mut Vec<ReducedListGroup>,
    tree: &Arc<SemanticTree>,
    node_index: usize,
    node: &SemanticNode,
    storage: Option<&[crate::ListGroup]>,
    excluded: Option<usize>,
) {
    let Some(storage) = storage else {
        return;
    };
    output.extend(
        (0..storage.len())
            .filter(|index| Some(*index) != excluded)
            .map(|index| reduced_list_group(tree, node_index, node, storage, index)),
    );
}

fn reduced_list_group(
    tree: &Arc<SemanticTree>,
    node_index: usize,
    node: &SemanticNode,
    storage: &[crate::ListGroup],
    index: usize,
) -> ReducedListGroup {
    let group = &storage[index];
    ReducedListGroup {
        range: group.range.start + node.offset..group.range.end + node.offset,
        kind: group.kind,
        tree: Arc::clone(tree),
        segments: vec![ListGroupSegment {
            nodes: node_index..node_index + 1,
            group_index: index,
        }],
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RootListRole {
    Transparent,
    List(ListKind),
    Other,
}

fn root_list_role(shard: &plumb_syntax::GreenShard) -> RootListRole {
    let Some(block) = shard.parsed().syntax.blocks.first() else {
        return RootListRole::Transparent;
    };
    if crate::is_document_declaration(block) {
        return RootListRole::Transparent;
    }
    let Block::Parsed(block) = block else {
        return RootListRole::Other;
    };
    match block.mark.as_ref().map(|mark| mark.marker.as_str()) {
        Some("-") => RootListRole::List(ListKind::Bullet),
        Some(".") => RootListRole::List(ListKind::Ordered),
        _ => RootListRole::Other,
    }
}

fn collect_document_records(
    source: &str,
    document: &Document,
    headings: &HeadingOutput,
) -> RecordOutput {
    let mut output = SemanticRoot::default();
    output.headings = headings.clone();
    let mut first_ids: HashMap<String, Range<usize>> = HashMap::new();
    collect_blocks(source, &document.blocks, &mut first_ids, &mut output);
    output.diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.code,
        )
    });
    RecordOutput {
        anchors: output.anchors,
        links: output.links,
        images: output.images,
        files: output.files,
        diagnostics: output.diagnostics,
    }
}

fn append_duplicate_anchor_diagnostics(
    anchors: &[AnchorRecord],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut first_ids: HashMap<String, Range<usize>> = HashMap::new();
    for anchor in anchors {
        if let Some(first) = first_ids.get(&anchor.id.value) {
            diagnostics.push(Diagnostic {
                code: "anchor.duplicate-id",
                severity: DiagnosticSeverity::Warning,
                message: format!("duplicate explicit anchor id '{}'", anchor.id.value),
                range: anchor.id.range.clone(),
                related: vec![first.clone()],
            });
        } else {
            first_ids.insert(anchor.id.value.clone(), anchor.id.range.clone());
        }
    }
}

fn shift_source_backed(value: &mut SourceBacked<String>, delta: isize) {
    shift_range(&mut value.range, delta);
    for boundary in &mut value.decoded_boundaries {
        *boundary = boundary.checked_add_signed(delta).unwrap();
    }
}

fn shift_diagnostics(diagnostics: &mut [Diagnostic], delta: isize) {
    for diagnostic in diagnostics {
        shift_range(&mut diagnostic.range, delta);
        for related in &mut diagnostic.related {
            shift_range(related, delta);
        }
    }
}

fn shift_range(range: &mut Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}

fn build_event_link_ranges(
    events: &crate::EventRecords,
    links: &SemanticRecords<LinkRecord>,
) -> Vec<EventLinkRange> {
    let links = links.iter().collect::<Vec<_>>();
    let event_ranges = events.ranges().collect::<Vec<_>>();
    debug_assert!(event_ranges
        .windows(2)
        .all(|events| events[0].start <= events[1].start));
    debug_assert!(links
        .windows(2)
        .all(|links| links[0].range.start <= links[1].range.start));
    event_ranges
        .into_iter()
        .map(|event| {
            let start = links.partition_point(|link| link.range.start < event.start);
            let end = links.partition_point(|link| link.range.start < event.end);
            debug_assert!(links[start..end]
                .iter()
                .all(|link| link.range.end <= event.end));
            EventLinkRange {
                event_start: event.start,
                links: start..end,
            }
        })
        .collect()
}

fn association_arity_diagnostics(document: &Document) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut blocks = document.blocks.iter().collect::<Vec<_>>();
    let mut contents = Vec::new();
    let mut inlines = Vec::new();
    while !blocks.is_empty() || !contents.is_empty() || !inlines.is_empty() {
        if let Some(block) = blocks.pop() {
            match block {
                Block::Parsed(block) => {
                    contents.push(&block.content);
                    blocks.extend(crate::body_children(block));
                }
                Block::Verbatim(_) => {}
            }
            continue;
        }
        if let Some(content) = contents.pop() {
            inlines.extend(content.items.iter().rev());
            continue;
        }
        while let Some(inline) = inlines.pop() {
            match inline {
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    let argument_count = crate::positional_elements(content).len();
                    if mark.as_ref().is_some_and(|mark| mark.marker == "=") && argument_count < 2 {
                        diagnostics.push(Diagnostic {
                            code: "association.invalid-arity",
                            severity: DiagnosticSeverity::Warning,
                            message: "inline '=' association requires a key and value".to_string(),
                            range: range.clone(),
                            related: Vec::new(),
                        });
                    }
                    contents.push(content);
                }
                Inline::Verbatim { .. } => {}
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| diagnostic.range.start);
    diagnostics
}

fn collect_blocks(
    source: &str,
    blocks: &[Block],
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut SemanticRoot,
) {
    for block in blocks {
        match block {
            Block::Parsed(parsed) => {
                if let Some(mark) = &parsed.mark {
                    let kind = if output
                        .headings
                        .heading_at_node_start(parsed.range.start)
                        .is_some()
                    {
                        AnchorKind::Heading
                    } else {
                        AnchorKind::Block
                    };
                    collect_anchor(
                        source,
                        &mark.attrs,
                        kind,
                        parsed.range.clone(),
                        crate::inline_selection_range(&parsed.content),
                        first_ids,
                        output,
                    );
                }
                collect_inlines(source, &parsed.content, first_ids, output);
                for child in crate::body_children(parsed) {
                    collect_blocks(source, std::slice::from_ref(child), first_ids, output);
                }
            }
            Block::Verbatim(block) => {
                if let Some(mark) = &block.mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::VerbatimBlock,
                        block.range.clone(),
                        block.range.clone(),
                        first_ids,
                        output,
                    );
                }
            }
        }
    }
}

fn collect_inlines(
    source: &str,
    content: &InlineContent,
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut SemanticRoot,
) {
    for inline in &content.items {
        match inline {
            Inline::Group {
                range,
                mark,
                content,
            } => {
                let selection_range = crate::positional_elements(content)
                    .first()
                    .map_or_else(|| range.clone(), |element| element.range.clone());
                if let Some(mark) = mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::Inline,
                        range.clone(),
                        selection_range.clone(),
                        first_ids,
                        output,
                    );
                    match mark.marker.as_str() {
                        "->" => collect_link(source, range.clone(), content, output),
                        "img" => collect_image(
                            source,
                            range.clone(),
                            selection_range.clone(),
                            &mark.attrs,
                            output,
                        ),
                        "file" => collect_file(
                            source,
                            range.clone(),
                            selection_range,
                            &mark.attrs,
                            output,
                        ),
                        _ => {}
                    }
                }
                collect_inlines(source, content, first_ids, output);
            }
            Inline::Verbatim {
                range,
                mark,
                text,
                text_range,
                quote_count,
                ..
            } => {
                if let Some(mark) = mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::Inline,
                        range.clone(),
                        range.clone(),
                        first_ids,
                        output,
                    );
                    if mark.marker == "->" {
                        collect_verbatim_link(
                            source,
                            VerbatimLink {
                                range: range.clone(),
                                kind_range: mark.marker_range.clone(),
                                text,
                                text_range: text_range.clone(),
                                quote_count: *quote_count,
                                attrs: &mark.attrs,
                            },
                            output,
                        );
                    }
                }
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

struct VerbatimLink<'a> {
    range: Range<usize>,
    kind_range: Range<usize>,
    text: &'a str,
    text_range: Range<usize>,
    quote_count: usize,
    attrs: &'a Attributes,
}

fn collect_verbatim_link(source: &str, input: VerbatimLink<'_>, output: &mut SemanticRoot) {
    let VerbatimLink {
        range,
        kind_range,
        text,
        text_range,
        quote_count,
        attrs,
    } = input;
    if let Some(conflict) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, range, .. } if key == "to" => Some(range.clone()),
        _ => None,
    }) {
        output.diagnostics.push(Diagnostic {
            code: "link.conflicting-property",
            severity: DiagnosticSeverity::Warning,
            message: "the '->' inline verbatim kind cannot be combined with a 'to' property"
                .to_string(),
            range: conflict,
            related: vec![kind_range],
        });
        return;
    }
    if !valid_derived_link_target(text) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must be a nonempty absolute URI or raw relative path".to_string(),
            range: text_range,
            related: Vec::new(),
        });
        return;
    }
    let envelope = range.start..attrs.range.as_ref().map_or(range.end, |range| range.start);
    let target_range = text_range.clone();
    push_link(
        range,
        text_range.clone(),
        direct_source_backed(source, text.to_string(), text_range),
        LinkSourceProjection {
            spelling: LinkSpelling::Verbatim {
                envelope,
                quote_count,
            },
            target_range,
            target_element_count: 1,
            target_declaration_ranges: Vec::new(),
        },
        classify_raw_target(text),
        output,
    );
}

fn valid_uri_reference(target: &str) -> bool {
    if target.is_empty()
        || target.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '\\'
        })
    {
        return false;
    }
    let bytes = target.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            if cursor + 2 >= bytes.len()
                || !bytes[cursor + 1].is_ascii_hexdigit()
                || !bytes[cursor + 2].is_ascii_hexdigit()
            {
                return false;
            }
            cursor += 3;
        } else {
            cursor += 1;
        }
    }
    let base = Url::parse("https://plumb.invalid/").expect("static base URL is valid");
    Url::parse(target).is_ok() || base.join(target).is_ok()
}

fn valid_derived_link_target(target: &str) -> bool {
    if target.is_empty()
        || target
            .chars()
            .any(|character| character.is_control() || character == '\\')
    {
        return false;
    }
    if has_uri_scheme(target) || target.starts_with("//") {
        return valid_uri_reference(target);
    }
    if target
        .split_once('#')
        .is_some_and(|(_, fragment)| fragment.is_empty() || fragment.contains('#'))
    {
        return false;
    }
    if target.chars().any(char::is_whitespace) {
        let path_end = target.find('#').unwrap_or(target.len());
        if target
            .chars()
            .any(|character| character != ' ' && character.is_whitespace())
            || target[path_end..].contains(' ')
        {
            return false;
        }
    }
    true
}

fn valid_relative_file_path(target: &str) -> bool {
    !target.is_empty()
        && !Path::new(target).is_absolute()
        && !target
            .chars()
            .any(|character| character.is_control() || character == '\\')
}

fn collect_image(
    source: &str,
    range: Range<usize>,
    selection_range: Range<usize>,
    attrs: &Attributes,
    output: &mut SemanticRoot,
) {
    let Some(value) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "src" => Some(value),
        _ => None,
    }) else {
        output.diagnostics.push(Diagnostic {
            code: "image.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "image requires a nonempty 'src' target".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let source_value = attr_source_backed(source, value);
    if source_value.value.is_empty() {
        output.diagnostics.push(Diagnostic {
            code: "image.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "image requires a nonempty 'src' target".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    let target_kind = if has_uri_scheme(&source_value.value) || source_value.value.starts_with("//")
    {
        if !valid_uri_reference(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "image.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "absolute image 'src' must be a valid URI reference".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        ImageTarget::External
    } else {
        if !valid_relative_file_path(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "image.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "relative image 'src' must be a valid raw file path".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        ImageTarget::File {
            path: source_value.value.clone(),
        }
    };
    output.images.push(ImageRecord {
        range,
        selection_range,
        source: source_value,
        target_kind,
    });
}

fn collect_file(
    source: &str,
    range: Range<usize>,
    selection_range: Range<usize>,
    attrs: &Attributes,
    output: &mut SemanticRoot,
) {
    let Some(value) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "src" => Some(value),
        _ => None,
    }) else {
        output.diagnostics.push(Diagnostic {
            code: "file.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "file requires a nonempty 'src' target".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let source_value = attr_source_backed(source, value);
    if source_value.value.is_empty() {
        output.diagnostics.push(Diagnostic {
            code: "file.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "file requires a nonempty 'src' target".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    let target_kind = if has_uri_scheme(&source_value.value) || source_value.value.starts_with("//")
    {
        if !valid_uri_reference(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "file.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "absolute file 'src' must be a valid URI reference".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        FileTarget::External
    } else {
        if !valid_relative_file_path(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "file.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "relative file 'src' must be a valid raw file path".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        FileTarget::File {
            path: source_value.value.clone(),
        }
    };
    output.files.push(FileRecord {
        range,
        selection_range,
        source: source_value,
        target_kind,
    });
}

fn collect_anchor(
    source: &str,
    attrs: &Attributes,
    kind: AnchorKind,
    range: Range<usize>,
    selection_range: Range<usize>,
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut SemanticRoot,
) {
    let Some((value, value_range)) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Id {
            value, value_range, ..
        } => Some((value, value_range)),
        AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
    }) else {
        return;
    };
    let id = direct_source_backed(source, value.clone(), value_range.clone());
    if let Some(first) = first_ids.get(value) {
        output.diagnostics.push(Diagnostic {
            code: "anchor.duplicate-id",
            severity: DiagnosticSeverity::Warning,
            message: format!("duplicate explicit anchor id '{value}'"),
            range: value_range.clone(),
            related: vec![first.clone()],
        });
    } else {
        first_ids.insert(value.clone(), value_range.clone());
    }
    output.anchors.push(AnchorRecord {
        id,
        kind,
        range,
        selection_range,
    });
}

fn collect_link(
    source: &str,
    range: Range<usize>,
    content: &InlineContent,
    output: &mut SemanticRoot,
) {
    let view = crate::owner_semantic_view(content);
    let Some(arguments) = view.split_first() else {
        output.diagnostics.push(Diagnostic {
            code: "link.missing-target",
            severity: DiagnosticSeverity::Warning,
            message: "link requires at least one positional argument".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let derived_label = arguments.rest.is_empty();
    let target_range = if derived_label {
        arguments.first.range.clone()
    } else {
        arguments
            .rest_range()
            .expect("explicit Link has target elements")
    };
    let target_element_count = if derived_label {
        1
    } else {
        arguments.rest.len()
    };
    let target_declaration_ranges = if derived_label {
        Vec::new()
    } else {
        arguments.rest_declaration_ranges()
    };
    let target_content = if derived_label {
        Some(arguments.first.clone())
    } else {
        arguments.rest_content()
    };
    let Some(target) = target_content
        .as_ref()
        .and_then(|content| stringify_target(source, content))
    else {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must stringify to a nonempty value".to_string(),
            range: arguments
                .rest_range()
                .unwrap_or_else(|| arguments.first.range.clone()),
            related: Vec::new(),
        });
        return;
    };
    if derived_label && !valid_derived_link_target(&target.value) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must be a nonempty absolute URI or raw relative path".to_string(),
            range: target.range.clone(),
            related: Vec::new(),
        });
        return;
    }
    let classification = if derived_label {
        classify_raw_target(&target.value)
    } else {
        classify_target(&target.value)
    };
    push_link(
        range,
        crate::element_selection_range(arguments.first),
        target,
        LinkSourceProjection {
            spelling: LinkSpelling::Positional,
            target_range,
            target_element_count,
            target_declaration_ranges,
        },
        classification,
        output,
    );
}

fn stringify_target(source: &str, content: &InlineContent) -> Option<SourceBacked<String>> {
    let mut builder = StringifyBuilder::default();
    stringify_content(source, content, &mut builder);
    builder.finish(source)
}

fn stringify_content(source: &str, content: &InlineContent, output: &mut StringifyBuilder) {
    let view = crate::owner_semantic_view(content);
    if let Some(content) = view.visible_content() {
        for inline in &content.items {
            match inline {
                Inline::Text { text, range } => {
                    output.append_text(source, text, range.clone());
                }
                Inline::Space { range, .. } | Inline::SoftBreak { range } => {
                    output.append_text(source, " ", range.clone());
                }
                Inline::Verbatim {
                    text, text_range, ..
                } => output.append_text(source, text, text_range.clone()),
                Inline::Group { content, .. } => stringify_content(source, content, output),
            }
        }
    }
}

#[derive(Default)]
struct StringifyBuilder {
    value: String,
    decoded_boundaries: Vec<usize>,
    range: Option<Range<usize>>,
}

impl StringifyBuilder {
    fn append_text(&mut self, source: &str, text: &str, source_range: Range<usize>) {
        if text.is_empty() {
            return;
        }
        if self.decoded_boundaries.is_empty() {
            self.decoded_boundaries.push(source_range.start);
            self.range = Some(source_range.clone());
        } else {
            *self.decoded_boundaries.last_mut().unwrap() = source_range.start;
            self.range.as_mut().unwrap().end = source_range.end;
        }
        let source_text = &source[source_range.clone()];
        let escaped_single = text.chars().count() == 1 && source_text.len() != text.len();
        for (offset, character) in text.char_indices() {
            self.value.push(character);
            for byte in 1..=character.len_utf8() {
                let decoded_end = offset + byte == text.len();
                self.decoded_boundaries.push(if decoded_end {
                    source_range.end
                } else if escaped_single {
                    source_range.start
                } else {
                    source_range.start + offset + byte
                });
            }
        }
    }

    fn finish(self, source: &str) -> Option<SourceBacked<String>> {
        let range = self.range?;
        (!self.value.is_empty()).then(|| SourceBacked {
            raw: source[range.clone()].to_string(),
            value: self.value,
            range,
            decoded_boundaries: self.decoded_boundaries,
        })
    }
}

struct LinkSourceProjection {
    spelling: LinkSpelling,
    target_range: Range<usize>,
    target_element_count: usize,
    target_declaration_ranges: Vec<Range<usize>>,
}

fn push_link(
    range: Range<usize>,
    selection_range: Range<usize>,
    target: SourceBacked<String>,
    source: LinkSourceProjection,
    classification: (LinkTarget, Option<Range<usize>>, Option<Range<usize>>),
    output: &mut SemanticRoot,
) {
    let LinkSourceProjection {
        spelling,
        target_range,
        target_element_count,
        target_declaration_ranges,
    } = source;
    let (target_kind, path_decoded, fragment_decoded) = classification;
    let path_range = path_decoded.and_then(|decoded| target.source_range(decoded));
    let fragment_range = fragment_decoded.and_then(|decoded| target.source_range(decoded));
    output.links.push(LinkRecord {
        range,
        selection_range,
        target,
        target_kind,
        spelling,
        target_range,
        target_element_count,
        target_declaration_ranges,
        path_range,
        fragment_range,
    });
}

fn classify_target(target: &str) -> (LinkTarget, Option<Range<usize>>, Option<Range<usize>>) {
    if Url::parse(target).is_ok() || target.starts_with("//") {
        return (LinkTarget::External, None, None);
    }
    let (path, fragment) = match target.split_once('#') {
        Some(parts) => parts,
        None if is_plumb_path(target) => {
            return (
                LinkTarget::Document {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
        None => {
            let path = uri_reference_path(target);
            if path.is_empty() {
                return (LinkTarget::Other, None, None);
            }
            return (
                LinkTarget::File {
                    path: path.to_string(),
                },
                Some(0..path.len()),
                None,
            );
        }
    };
    if fragment.is_empty() {
        return (LinkTarget::Other, None, None);
    }
    if !path.is_empty() && !is_plumb_path(path) {
        let file_path = uri_reference_path(path);
        return (
            LinkTarget::File {
                path: file_path.to_string(),
            },
            Some(0..file_path.len()),
            None,
        );
    }
    let path_value = (!path.is_empty()).then(|| path.to_string());
    let path_range = (!path.is_empty()).then_some(0..path.len());
    let fragment_start = path.len() + 1;
    (
        LinkTarget::Anchor {
            path: path_value,
            fragment: fragment.to_string(),
        },
        path_range,
        Some(fragment_start..target.len()),
    )
}

fn classify_raw_target(target: &str) -> (LinkTarget, Option<Range<usize>>, Option<Range<usize>>) {
    if has_uri_scheme(target) || target.starts_with("//") {
        return (LinkTarget::External, None, None);
    }
    let (path, fragment) = match target.split_once('#') {
        Some(parts) => parts,
        None if is_plumb_path(target) => {
            return (
                LinkTarget::Document {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
        None => {
            return (
                LinkTarget::File {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
    };
    let path_value = (!path.is_empty()).then(|| path.to_string());
    let path_range = (!path.is_empty()).then_some(0..path.len());
    let fragment_start = path.len() + 1;
    (
        LinkTarget::Anchor {
            path: path_value,
            fragment: fragment.to_string(),
        },
        path_range,
        Some(fragment_start..target.len()),
    )
}

pub(crate) fn has_uri_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn uri_reference_path(target: &str) -> &str {
    let end = target.find(['?', '#']).unwrap_or(target.len());
    &target[..end]
}

fn is_plumb_path(value: &str) -> bool {
    value.ends_with(".plumb")
}

fn direct_source_backed(source: &str, value: String, range: Range<usize>) -> SourceBacked<String> {
    let decoded_boundaries = (range.start..=range.end).collect();
    SourceBacked {
        raw: source[range.clone()].to_string(),
        value,
        range,
        decoded_boundaries,
    }
}

pub(crate) fn attr_source_backed(source: &str, value: &AttrValue) -> SourceBacked<String> {
    if !value.quoted || !(value.raw.starts_with('"') && value.raw.ends_with('"')) {
        return direct_source_backed(source, value.decoded.clone(), value.range.clone());
    }
    let mut decoded_boundaries = Vec::with_capacity(value.decoded.len() + 1);
    let mut cursor = value.range.start + 1;
    let end = value.range.end.saturating_sub(1);
    while cursor < end {
        let source_start = cursor;
        if source.as_bytes()[cursor] == b'\\' {
            cursor += 1;
        }
        let character = source[cursor..]
            .chars()
            .next()
            .expect("quoted value cursor is valid");
        for _ in 0..character.len_utf8() {
            decoded_boundaries.push(source_start);
        }
        cursor += character.len_utf8();
    }
    decoded_boundaries.push(end);
    SourceBacked {
        value: value.decoded.clone(),
        raw: value.raw.clone(),
        range: value.range.clone(),
        decoded_boundaries,
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::{parse, parse_incremental};

    use super::*;

    #[test]
    fn green_fresh_analysis_matches_the_complete_profile() {
        let source = "`: first body\n`= date 2026-09-05\n`: second body\n`= timezone +08:00\n\n`# Main\n `@ duplicate\n\n`## Child\n `@ duplicate\n\n`- Task\n `+ task\n `= due 2026-09-06T09:00:00+08:00\n\n`- 10:00 Event\n `+ event\n\nParagraph `->{guide guide.plumb} with `cite{source} and `*{style}.\n\n`table\n `- name age\n  `+ header\n `- Alice 10\n";
        let parsed = parse(source);
        let valid = parsed.valid_syntax().unwrap();
        let headings = crate::analyze_headings(valid);
        let metadata = crate::analyze_metadata(valid);
        let citations = crate::analyze_citations(valid);
        let inline_styles = crate::analyze_inline_styles(valid);
        let lists = crate::analyze_lists(valid);
        let math = crate::analyze_math(valid);
        let quotes = crate::analyze_quotes(valid);
        let tasks = crate::analyze_tasks(valid);
        let events = crate::analyze_events(valid, &metadata);
        let tables = crate::analyze_tables(valid);
        let records = collect_document_records(valid.source(), valid.syntax(), &headings);
        let mut diagnostics = association_arity_diagnostics(valid.syntax());
        diagnostics.extend(records.diagnostics.iter().cloned());
        diagnostics.extend(tables.diagnostics.iter().cloned());
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start,
                diagnostic.range.end,
                diagnostic.code,
            )
        });

        let output = analyze_document(valid);
        assert_eq!(output.headings(), &headings);
        assert_eq!(output.metadata(), &metadata);
        assert_eq!(output.citations(), &citations);
        assert_eq!(output.inline_styles(), &inline_styles);
        assert_eq!(output.lists(), &lists);
        assert_eq!(output.math(), &math);
        assert_eq!(output.quotes(), &quotes);
        assert_eq!(output.tasks(), &tasks);
        assert_eq!(output.events(), &events);
        assert_eq!(output.tables(), &tables);
        assert_eq!(output.anchors(), &records.anchors);
        assert_eq!(output.links(), &records.links);
        assert_eq!(output.images(), &records.images);
        assert_eq!(output.files(), &records.files);
        assert_eq!(output.diagnostics(), diagnostics);
    }

    #[test]
    fn incremental_event_analysis_matches_full_document_semantics() {
        let old = "`= date 2026-09-05\n`= timezone +08:00\n\n`- 09:00 First\n `+ event\n\n`- 10:00 Middle\n `+ event\n\n`- 11:00 Last\n `+ event\n";
        let new = old.replace("Middle", "Changed middle");
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse_incremental(&previous, new.clone());
        let change = DocumentChange {
            old_range: current.old_reparsed_range,
            new_range: current.reparsed_range,
        };
        let incremental = analyze_document_incremental(
            current.document.valid_syntax().unwrap(),
            &previous_output,
            &change,
        );
        let fresh = analyze_document(parse(new).valid_syntax().unwrap());
        assert_eq!(incremental, fresh);
        assert!(incremental
            .events()
            .events
            .shares_segment_topology(&previous_output.events().events));
    }

    #[test]
    fn incremental_event_analysis_invalidates_changed_document_context() {
        let old = "`= date 2026-09-05\n`= timezone +08:00\n\n`- 09:00 Event\n `+ event\n";
        let new = old.replace("2026-09-05", "2026-09-06");
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse_incremental(&previous, new.clone());
        let change = DocumentChange {
            old_range: current.old_reparsed_range,
            new_range: current.reparsed_range,
        };
        let incremental = analyze_document_incremental(
            current.document.valid_syntax().unwrap(),
            &previous_output,
            &change,
        );
        let fresh = analyze_document(parse(new).valid_syntax().unwrap());
        assert_eq!(incremental, fresh);
        assert_eq!(
            incremental.events.events.get(0).unwrap().at.unwrap().value,
            "2026-09-06T09:00:00+08:00"
        );
    }

    #[test]
    fn incremental_document_records_rebase_suffix_and_rebuild_global_diagnostics() {
        let old = "`node First\n `@ same\n\nSee `->{one first.plumb#target}.\n\n`node Middle\n\n `img{old `={src old.png}}\n\n`node Last\n `@ same\n\n `file{manual `={src docs/manual.pdf}}\n";
        let new = old.replace(
            "`node Middle\n\n `img{old `={src old.png}}",
            "`node Changed middle owner\n\n `img{new `={src images/new.png}}",
        );
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse_incremental(&previous, new.clone());
        let change = DocumentChange {
            old_range: current.old_reparsed_range,
            new_range: current.reparsed_range,
        };
        let incremental = analyze_document_incremental(
            current.document.valid_syntax().unwrap(),
            &previous_output,
            &change,
        );
        let fresh = analyze_document(parse(new).valid_syntax().unwrap());
        assert_eq!(incremental, fresh);
        assert_eq!(
            incremental
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "anchor.duplicate-id")
                .count(),
            1
        );
    }

    #[test]
    fn semantic_tree_reuses_relative_nodes_across_a_file_start_shift() {
        let old = "`# Heading\n `@ heading\n\n`- Task `->{guide guide.plumb}\n `+ task\n `@ task\n `img{icon `={src icon.png}}\n\n`- 2026-09-05T09:00:00+08:00 Event\n `+ event\n\n`table\n `- name age\n\n`> Quote `cite{paper} `!{strong} `$\"x\"\n";
        let prefix = "Prelude\n\n";
        let new = format!("{prefix}{old}");
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse(&new);
        let incremental = analyze_document_incremental(
            current.valid_syntax().unwrap(),
            &previous_output,
            &DocumentChange {
                old_range: 0..0,
                new_range: 0..prefix.len(),
            },
        );
        let fresh = analyze_document(current.valid_syntax().unwrap());

        assert_eq!(incremental, fresh);
        assert_eq!(
            incremental.reused_semantic_node_count(),
            previous_output.semantic_node_count()
        );
        assert_eq!(
            incremental.tasks().tasks.get(0).unwrap().range.start,
            previous_output.tasks().tasks.get(0).unwrap().range.start + prefix.len()
        );
    }

    #[test]
    fn semantic_tree_context_change_rebuilds_all_nodes() {
        let old = "`= date 2026-09-05\n`= timezone +08:00\n\n`- 09:00 Event\n `+ event\n";
        let new = old.replace("2026-09-05", "2026-09-06");
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse(&new);
        let start = old.find("2026-09-05").unwrap();
        let incremental = analyze_document_incremental(
            current.valid_syntax().unwrap(),
            &previous_output,
            &DocumentChange {
                old_range: start..start + "2026-09-05".len(),
                new_range: start..start + "2026-09-06".len(),
            },
        );

        assert_eq!(incremental.reused_semantic_node_count(), 0);
        assert_eq!(
            incremental
                .events()
                .events
                .get(0)
                .unwrap()
                .at
                .unwrap()
                .value,
            "2026-09-06T09:00:00+08:00"
        );
    }

    #[test]
    fn semantic_tree_recomputes_metadata_and_empty_outline_fast_paths_when_introduced() {
        let old = "Body\n";
        let addition = "\n`= title Added\n\n`# Added heading\n";
        let new = format!("{old}{addition}");
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse(&new);
        let incremental = analyze_document_incremental(
            current.valid_syntax().unwrap(),
            &previous_output,
            &DocumentChange {
                old_range: old.len()..old.len(),
                new_range: old.len()..new.len(),
            },
        );
        let fresh = analyze_document(current.valid_syntax().unwrap());

        assert_eq!(incremental, fresh);
        assert_eq!(
            incremental.metadata().document_title().as_deref(),
            Some("Added")
        );
        assert_eq!(incremental.headings().headings.len(), 1);
    }

    #[test]
    fn semantic_tree_rebuilds_record_and_list_topology_when_counts_change() {
        let old = "`- One\n\n`- Two\n\nPlain\n";
        let replacement = "`. Two\n\n`->{guide.plumb}\n";
        let start = old.find("`- Two").unwrap();
        let new = format!("{}{replacement}", &old[..start]);
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse(&new);
        let incremental = analyze_document_incremental(
            current.valid_syntax().unwrap(),
            &previous_output,
            &DocumentChange {
                old_range: start..old.len(),
                new_range: start..new.len(),
            },
        );
        let fresh = analyze_document(current.valid_syntax().unwrap());

        assert_eq!(incremental, fresh);
        assert_eq!(incremental.lists().groups.len(), 2);
        assert_eq!(incremental.links().len(), 1);
    }

    #[test]
    fn semantic_tree_rebuilds_diagnostic_projection_when_an_error_appears() {
        let old = "Plain\n";
        let new = "`->{}\n";
        let previous = parse(old);
        let previous_output = analyze_document(previous.valid_syntax().unwrap());
        let current = parse(new);
        let incremental = analyze_document_incremental(
            current.valid_syntax().unwrap(),
            &previous_output,
            &DocumentChange {
                old_range: 0..old.len(),
                new_range: 0..new.len(),
            },
        );
        let fresh = analyze_document(current.valid_syntax().unwrap());

        assert_eq!(incremental, fresh);
        assert!(!incremental.diagnostics().is_empty());
    }

    #[test]
    fn only_shorthand_ids_create_anchors() {
        let parsed = parse("`# Heading\n  `@ intro\n\n`## Pair only\n  `= id|pair\n");
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors.get(0).unwrap().id.value, "intro");
        assert_eq!(output.anchors.get(0).unwrap().kind, AnchorKind::Heading);
    }

    #[test]
    fn verbatim_wrappers_create_syntax_neutral_anchors() {
        let parsed = plumb_syntax::parse("`()\n `@ example\n `text\"\n  raw text\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors.get(0).unwrap().kind, AnchorKind::Block);
    }

    #[test]
    fn recognizes_compact_and_expanded_positional_links() {
        let source = "`->{guide.plumb}\n`->{`!{styled.plumb}}\n`->{`\"Project Guide.plumb#intro\" `@{rich}}\n`->{guide target.plumb}\n`->{{guide page} `\"Project Guide.plumb#intro\"}\n`->{`*{external} https://example.test}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 6);
        assert!(output
            .links
            .iter()
            .all(|link| link.spelling == LinkSpelling::Positional));
        assert_eq!(output.links.get(0).unwrap().target.value, "guide.plumb");
        assert_eq!(output.links.get(1).unwrap().target.value, "styled.plumb");
        assert_eq!(
            &source[output.links.get(1).unwrap().path_range.clone().unwrap()],
            "styled.plumb"
        );
        assert_eq!(
            output.links.get(2).unwrap().target.value,
            "Project Guide.plumb#intro"
        );
        assert_eq!(
            &source[output.links.get(2).unwrap().fragment_range.clone().unwrap()],
            "intro"
        );
        assert_eq!(output.links.get(3).unwrap().target.value, "target.plumb");
        assert_eq!(
            output.links.get(3).unwrap().target_kind,
            LinkTarget::Document {
                path: "target.plumb".to_string()
            }
        );
        assert_eq!(
            &source[output.links.get(4).unwrap().selection_range.clone()],
            "guide page"
        );
        assert_eq!(
            output.links.get(4).unwrap().target.value,
            "Project Guide.plumb#intro"
        );
        assert_eq!(
            output.links.get(4).unwrap().target_kind,
            LinkTarget::Anchor {
                path: Some("Project Guide.plumb".to_string()),
                fragment: "intro".to_string()
            }
        );
        assert_eq!(
            &source[output.links.get(5).unwrap().selection_range.clone()],
            "`*{external}"
        );
        assert_eq!(
            output.links.get(5).unwrap().target_kind,
            LinkTarget::External
        );
    }

    #[test]
    fn indexes_overlapping_event_containment_without_copying_links() {
        let parsed = parse(
            "`->{Before before.plumb}\n\n`- 10:00 {Outer `->{Outer outer.plumb}}\n `+ event\n `- 11:00 {Nested `->{Nested nested.plumb}}\n  `+ event\n\n`->{After after.plumb}\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(parsed.valid_syntax().unwrap());
        assert_eq!(output.links.len(), 4);
        assert_eq!(output.event_link_ranges().len(), 2);

        let outer = output.events.events.get(0).unwrap();
        let nested = output.events.events.get(1).unwrap();
        assert_eq!(
            output
                .links_contained_by_event(outer.range.start)
                .unwrap()
                .iter()
                .map(|link| link.target.value.as_str())
                .collect::<Vec<_>>(),
            ["outer.plumb", "nested.plumb"]
        );
        assert_eq!(
            output
                .links_contained_by_event(nested.range.start)
                .unwrap()
                .iter()
                .map(|link| link.target.value.as_str())
                .collect::<Vec<_>>(),
            ["nested.plumb"]
        );
        assert!(output.links_contained_by_event(usize::MAX).is_none());
        assert_eq!(
            std::mem::size_of::<EventLinkRange>(),
            3 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn positional_link_ranges_map_utf8_and_escaped_delimiters() {
        let source = "`->{目标 目录/项].plumb#章节}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let link = &output.links.get(0).unwrap();
        assert_eq!(link.target.raw, "目录/项].plumb#章节");
        assert_eq!(link.target.value, "目录/项].plumb#章节");
        assert_eq!(&source[link.path_range.clone().unwrap()], "目录/项].plumb");
        assert_eq!(&source[link.fragment_range.clone().unwrap()], "章节");
    }

    #[test]
    fn associations_bind_all_elements_after_the_key_as_value() {
        let source = "`span{value `={key value extra}}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed block");
        };
        let Inline::Group {
            mark: Some(mark), ..
        } = &block.content.items[0]
        else {
            panic!("expected marked inline group");
        };
        assert_eq!(mark.attrs.value("key"), Some("value extra"));
    }

    #[test]
    fn link_kind_is_not_a_standard_link() {
        let parsed = plumb_syntax::parse("`link{generic `={to other.plumb#target}}\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.links.is_empty());
    }

    #[test]
    fn recognizes_inline_verbatim_links_without_normalizing_the_target() {
        let source = "Visit `->\"https://example.test/a%20b\" or `->\"https://[::1]/\".\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 2);
        assert_eq!(
            output.links.get(0).unwrap().target.value,
            "https://example.test/a%20b"
        );
        assert_eq!(
            output.links.get(0).unwrap().target.raw,
            "https://example.test/a%20b"
        );
        assert_eq!(
            output.links.get(0).unwrap().target_kind,
            LinkTarget::External
        );
        assert_eq!(output.links.get(1).unwrap().target.value, "https://[::1]/");
    }

    #[test]
    fn recognizes_relative_verbatim_link_targets() {
        let source = "`->\"other.plumb\"\n`->\"other notes.plumb#section\"\n`->\"../assets/a b.pdf\"\n`->\"../assets/100% done?.pdf\"\n`->\"#local\"\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 5);
        assert!(output
            .links
            .iter()
            .all(|link| matches!(link.spelling, LinkSpelling::Verbatim { .. })));
        assert_eq!(
            output.links.get(0).unwrap().target_kind,
            LinkTarget::Document {
                path: "other.plumb".to_string()
            }
        );
        assert_eq!(
            output.links.get(1).unwrap().target_kind,
            LinkTarget::Anchor {
                path: Some("other notes.plumb".to_string()),
                fragment: "section".to_string()
            }
        );
        assert_eq!(
            output.links.get(2).unwrap().target_kind,
            LinkTarget::File {
                path: "../assets/a b.pdf".to_string()
            }
        );
        assert_eq!(
            &parsed.source[output.links.get(1).unwrap().fragment_range.clone().unwrap()],
            "section"
        );
        assert_eq!(
            output.links.get(3).unwrap().target_kind,
            LinkTarget::File {
                path: "../assets/100% done?.pdf".to_string()
            }
        );
        assert_eq!(
            output.links.get(4).unwrap().target_kind,
            LinkTarget::Anchor {
                path: None,
                fragment: "local".to_string()
            }
        );
    }

    #[test]
    fn recognizes_standard_images_and_diagnoses_invalid_sources() {
        let source = "`img{{Alt `*{text}} `={src `\"static/图 像(100%).png\"} `@{figure} `+{wide} `={loading lazy}}\n`img{`={src https://example.test/a.png}}\n`img{Missing}\n`img{Empty `={src {}}}\n`img{{Invalid URI} `={src `\"https://example.test/bad path.png\"}}\n`img{{Invalid path} `={src bad\\path.png}}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.images.len(), 2);
        assert_eq!(
            output.images.get(0).unwrap().source.value,
            "static/图 像(100%).png"
        );
        assert_eq!(
            output.images.get(0).unwrap().target_kind,
            ImageTarget::File {
                path: "static/图 像(100%).png".to_string()
            }
        );
        assert_eq!(
            output.images.get(1).unwrap().target_kind,
            ImageTarget::External
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "image.missing-source",
                "image.missing-source",
                "image.invalid-source",
                "image.invalid-source"
            ]
        );
    }

    #[test]
    fn recognizes_standard_files_and_diagnoses_invalid_sources() {
        let source = "`file{Demo `={src `\"static/demo video.mp4\"} `@{demo} `+{wide}}\n`file{Remote `={src https://example.test/demo.mp4}}\n`file{Missing}\n`file{Empty `={src {}}}\n`file{{Invalid URI} `={src `\"https://example.test/bad path.mp4\"}}\n`file{{Invalid path} `={src bad\\path.mp4}}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.files.len(), 2);
        assert_eq!(
            output.files.get(0).unwrap().source.value,
            "static/demo video.mp4"
        );
        assert_eq!(
            output.files.get(0).unwrap().target_kind,
            FileTarget::File {
                path: "static/demo video.mp4".to_string()
            }
        );
        assert_eq!(
            output.files.get(1).unwrap().target_kind,
            FileTarget::External
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "file.missing-source",
                "file.missing-source",
                "file.invalid-source",
                "file.invalid-source"
            ]
        );
    }

    #[test]
    fn diagnoses_invalid_derived_link_targets_and_ignores_arrow_facets() {
        let source = "`->\"\"\n`->\"https://example.test/bad path\"\n`->\"https://example.test/%zz\"\n`->\"doc.plumb#one#two\"\n`span{text `+{->}}\n\n`note head\n `+ ->\n\n`()\n `+ ->\n\n `\"\n  raw\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.links.is_empty());
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            [
                "link.invalid-target",
                "link.invalid-target",
                "link.invalid-target",
                "link.invalid-target",
            ]
        );
    }

    #[test]
    fn duplicate_ids_are_semantic_diagnostics() {
        let parsed = parse("`node One\n  `@ same\n\n`other Two\n  `@ same\n");
        assert!(parsed.is_valid());
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.diagnostics[0].code, "anchor.duplicate-id");
    }
}
