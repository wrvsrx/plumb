use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use plumb_syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent, ValidDocument,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::records::RecordSegment;
use crate::{
    analyze_citations, analyze_events, analyze_headings, analyze_inline_styles, analyze_lists,
    analyze_math, analyze_metadata, analyze_quotes, analyze_tables, analyze_tasks, CitationOutput,
    EventOutput, HeadingOutput, InlineStyleOutput, ListGroup, ListGroups, ListKind, ListOutput,
    MathOutput, MetadataOutput, QuoteOutput, RelativeSemanticRecord, SemanticRecords, TableOutput,
    TaskOutput,
};

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
    pub(crate) event_link_ranges: Vec<EventLinkRange>,
    pub(crate) images: SemanticRecords<ImageRecord>,
    pub(crate) files: SemanticRecords<FileRecord>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct SemanticTree {
    syntax: Arc<plumb_syntax::GreenDocument>,
    nodes: Vec<SemanticNode>,
    index: HashMap<usize, usize>,
    cache_hits: usize,
}

#[derive(Debug, Clone)]
struct SemanticNode {
    syntax: Arc<plumb_syntax::GreenShard>,
    offset: usize,
    output: Arc<SemanticNodeOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticNodeOutput {
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

struct FreshSemanticOutput {
    citations: CitationOutput,
    inline_styles: InlineStyleOutput,
    math: MathOutput,
    quotes: QuoteOutput,
    tasks: TaskOutput,
    events: EventOutput,
    tables: TableOutput,
    records: RecordOutput,
    association_diagnostics: Vec<Diagnostic>,
}

impl Default for SemanticRoot {
    fn default() -> Self {
        Self {
            tree: Arc::new(SemanticTree::empty()),
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
            event_link_ranges: Vec::new(),
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
            index: HashMap::new(),
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
            && self.event_link_ranges() == other.event_link_ranges()
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

    pub fn event_link_ranges(&self) -> &[EventLinkRange] {
        &self.root.event_link_ranges
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
        let index = self
            .event_link_ranges
            .binary_search_by_key(&event_start, |range| range.event_start)
            .ok()?;
        Some(
            self.links
                .iter()
                .skip(self.event_link_ranges[index].links.start)
                .take(self.event_link_ranges[index].links.len())
                .collect(),
        )
    }
}

pub fn analyze_document(valid: ValidDocument<'_>) -> DocumentOutput {
    let syntax = Arc::new(plumb_syntax::GreenDocument::parse(
        valid.source().to_string(),
    ));
    analyze_semantic_tree(valid, syntax, None)
        .expect("a valid document produces a valid semantic tree")
}

pub fn analyze_green_document(
    valid: ValidDocument<'_>,
    syntax: Arc<plumb_syntax::GreenDocument>,
) -> Option<DocumentOutput> {
    analyze_semantic_tree(valid, syntax, None)
}

pub fn analyze_document_incremental(
    valid: ValidDocument<'_>,
    previous: &DocumentOutput,
    change: &DocumentChange,
) -> DocumentOutput {
    let syntax = Arc::new(
        previous
            .root
            .tree
            .syntax
            .reparse_from_change(
                valid.source().to_string(),
                plumb_syntax::SourceChange {
                    old_range: change.old_range.clone(),
                    new_range: change.new_range.clone(),
                },
            )
            .document,
    );
    analyze_semantic_tree(valid, syntax, Some(previous))
        .expect("a valid document produces a valid semantic tree")
}

fn analyze_semantic_tree(
    valid: ValidDocument<'_>,
    syntax: Arc<plumb_syntax::GreenDocument>,
    previous: Option<&DocumentOutput>,
) -> Option<DocumentOutput> {
    if valid.source() != syntax.source() || !syntax.is_valid() {
        return None;
    }
    let metadata = analyze_metadata(valid);
    let headings = analyze_headings(valid);
    let reusable = previous.filter(|previous| previous.metadata() == &metadata);
    let fresh = reusable.is_none().then(|| FreshSemanticOutput {
        citations: analyze_citations(valid),
        inline_styles: analyze_inline_styles(valid),
        math: analyze_math(valid),
        quotes: analyze_quotes(valid),
        tasks: analyze_tasks(valid),
        events: analyze_events(valid, &metadata),
        tables: analyze_tables(valid),
        records: collect_document_records(valid.source(), valid.syntax(), &headings),
        association_diagnostics: association_arity_diagnostics(valid.syntax()),
    });
    let fresh_nodes = fresh.as_ref().map(|fresh| nodes_from_fresh(fresh, &syntax));
    let mut index = HashMap::with_capacity(syntax.shards().len());
    let mut cache_hits = 0;
    let nodes = syntax
        .shards()
        .enumerate()
        .map(|(node_index, view)| {
            let node_syntax = Arc::clone(view.shard());
            let identity = Arc::as_ptr(&node_syntax) as usize;
            let output = reusable
                .and_then(|previous| {
                    previous
                        .root
                        .tree
                        .index
                        .get(&identity)
                        .map(|index| Arc::clone(&previous.root.tree.nodes[*index].output))
                })
                .map(|output| {
                    cache_hits += 1;
                    output
                })
                .unwrap_or_else(|| match &fresh_nodes {
                    Some(nodes) => Arc::clone(&nodes[node_index]),
                    None => {
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
                    }
                });
            index.insert(identity, node_index);
            SemanticNode {
                syntax: node_syntax,
                offset: view.offset(),
                output,
            }
        })
        .collect::<Vec<_>>();
    let tree = Arc::new(SemanticTree {
        syntax,
        nodes,
        index,
        cache_hits,
    });

    let citations = CitationOutput {
        citations: segmented_records(&tree, |output| &output.citations.citations),
        diagnostics: node_diagnostics(&tree, |output| &output.citations.diagnostics),
    };
    let inline_styles = InlineStyleOutput {
        styles: segmented_records(&tree, |output| &output.inline_styles.styles),
    };
    let math = MathOutput {
        records: segmented_records(&tree, |output| &output.math.records),
        diagnostics: node_diagnostics(&tree, |output| &output.math.diagnostics),
    };
    let quotes = QuoteOutput {
        quotes: segmented_records(&tree, |output| &output.quotes.quotes),
    };
    let tasks = TaskOutput {
        tasks: segmented_records(&tree, |output| &output.tasks.tasks),
        diagnostics: node_diagnostics(&tree, |output| &output.tasks.diagnostics),
    };
    let events = EventOutput {
        events: segmented_records(&tree, |output| &output.events.events),
        diagnostics: node_diagnostics(&tree, |output| &output.events.diagnostics),
    };
    let tables = TableOutput {
        tables: segmented_records(&tree, |output| &output.tables.tables),
        diagnostics: node_diagnostics(&tree, |output| &output.tables.diagnostics),
    };
    let anchors = segmented_records(&tree, |output| &output.records.anchors);
    let links = segmented_records(&tree, |output| &output.records.links);
    let images = segmented_records(&tree, |output| &output.records.images);
    let files = segmented_records(&tree, |output| &output.records.files);
    let mut record_diagnostics = node_diagnostics(&tree, |output| &output.records.diagnostics);
    record_diagnostics.retain(|diagnostic| diagnostic.code != "anchor.duplicate-id");
    let absolute_anchors = anchors.iter().collect::<Vec<_>>();
    append_duplicate_anchor_diagnostics(&absolute_anchors, &mut record_diagnostics);
    record_diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.code,
        )
    });
    let lists = reduce_lists(&tree);
    let event_link_ranges = build_event_link_ranges(&events.events, &links);
    let mut diagnostics = node_diagnostics(&tree, |output| &output.association_diagnostics);
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
            event_link_ranges,
            images,
            files,
            diagnostics,
        }),
    })
}

fn segmented_records<T: RelativeSemanticRecord>(
    tree: &SemanticTree,
    records: fn(&SemanticNodeOutput) -> &SemanticRecords<T>,
) -> SemanticRecords<T> {
    SemanticRecords::from_segments(
        tree.nodes
            .iter()
            .filter_map(|node| {
                Some(RecordSegment {
                    offset: node.offset as isize,
                    records: records(&node.output).owned_arc()?,
                })
            })
            .collect(),
    )
}

fn nodes_from_fresh(
    fresh: &FreshSemanticOutput,
    syntax: &plumb_syntax::GreenDocument,
) -> Vec<Arc<SemanticNodeOutput>> {
    let owners = syntax.shards().map(|view| view.range()).collect::<Vec<_>>();
    let mut citations =
        partition_records(&fresh.citations.citations, &owners, |record| &record.range);
    let mut citation_diagnostics =
        partition_diagnostics(&fresh.citations.diagnostics, &owners, None);
    let mut inline_styles =
        partition_records(&fresh.inline_styles.styles, &owners, |record| &record.range);
    let mut math = partition_records(&fresh.math.records, &owners, |record| &record.range);
    let mut math_diagnostics = partition_diagnostics(&fresh.math.diagnostics, &owners, None);
    let mut quotes = partition_records(&fresh.quotes.quotes, &owners, |record| &record.range);
    let mut tasks = partition_records(&fresh.tasks.tasks, &owners, |record| &record.range);
    let mut task_diagnostics = partition_diagnostics(&fresh.tasks.diagnostics, &owners, None);
    let mut events = partition_records(&fresh.events.events, &owners, |record| &record.range);
    let mut event_diagnostics = partition_diagnostics(&fresh.events.diagnostics, &owners, None);
    let mut tables = partition_records(&fresh.tables.tables, &owners, |record| &record.range);
    let mut table_diagnostics = partition_diagnostics(&fresh.tables.diagnostics, &owners, None);
    let mut anchors = partition_records(&fresh.records.anchors, &owners, |record| &record.range);
    let mut links = partition_records(&fresh.records.links, &owners, |record| &record.range);
    let mut images = partition_records(&fresh.records.images, &owners, |record| &record.range);
    let mut files = partition_records(&fresh.records.files, &owners, |record| &record.range);
    let mut record_diagnostics = partition_diagnostics(
        &fresh.records.diagnostics,
        &owners,
        Some("anchor.duplicate-id"),
    );
    let mut association_diagnostics =
        partition_diagnostics(&fresh.association_diagnostics, &owners, None);

    syntax
        .shards()
        .enumerate()
        .map(|(index, view)| {
            let local = view
                .shard()
                .parsed()
                .valid_syntax()
                .expect("valid green document has valid shards");
            Arc::new(SemanticNodeOutput {
                citations: CitationOutput {
                    citations: std::mem::take(&mut citations[index]),
                    diagnostics: std::mem::take(&mut citation_diagnostics[index]),
                },
                inline_styles: InlineStyleOutput {
                    styles: std::mem::take(&mut inline_styles[index]),
                },
                math: MathOutput {
                    records: std::mem::take(&mut math[index]),
                    diagnostics: std::mem::take(&mut math_diagnostics[index]),
                },
                quotes: QuoteOutput {
                    quotes: std::mem::take(&mut quotes[index]),
                },
                tasks: TaskOutput {
                    tasks: std::mem::take(&mut tasks[index]),
                    diagnostics: std::mem::take(&mut task_diagnostics[index]),
                },
                events: EventOutput {
                    events: std::mem::take(&mut events[index]),
                    diagnostics: std::mem::take(&mut event_diagnostics[index]),
                },
                lists: analyze_lists(local),
                tables: TableOutput {
                    tables: std::mem::take(&mut tables[index]),
                    diagnostics: std::mem::take(&mut table_diagnostics[index]),
                },
                records: RecordOutput {
                    anchors: std::mem::take(&mut anchors[index]),
                    links: std::mem::take(&mut links[index]),
                    images: std::mem::take(&mut images[index]),
                    files: std::mem::take(&mut files[index]),
                    diagnostics: std::mem::take(&mut record_diagnostics[index]),
                },
                association_diagnostics: std::mem::take(&mut association_diagnostics[index]),
            })
        })
        .collect()
}

fn partition_records<T: RelativeSemanticRecord>(
    records: &SemanticRecords<T>,
    owners: &[Range<usize>],
    range: fn(&T) -> &Range<usize>,
) -> Vec<SemanticRecords<T>> {
    let mut partitions = (0..owners.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for mut record in records.iter() {
        let record_range = range(&record).clone();
        let index = owners
            .partition_point(|owner| owner.start <= record_range.start)
            .checked_sub(1)
            .expect("a semantic record starts inside a syntax shard");
        assert!(record_range.end <= owners[index].end);
        record.shift(-(owners[index].start as isize));
        partitions[index].push(record);
    }
    partitions
        .into_iter()
        .map(SemanticRecords::from_owned)
        .collect()
}

fn partition_diagnostics(
    diagnostics: &[Diagnostic],
    owners: &[Range<usize>],
    excluded_code: Option<&str>,
) -> Vec<Vec<Diagnostic>> {
    let mut partitions = (0..owners.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| excluded_code != Some(diagnostic.code))
    {
        let index = owners
            .partition_point(|owner| owner.start <= diagnostic.range.start)
            .checked_sub(1)
            .expect("a semantic diagnostic starts inside a syntax shard");
        assert!(diagnostic.range.end <= owners[index].end);
        let mut diagnostic = diagnostic.clone();
        shift_diagnostics(
            std::slice::from_mut(&mut diagnostic),
            -(owners[index].start as isize),
        );
        partitions[index].push(diagnostic);
    }
    partitions
}

fn node_diagnostics(
    tree: &SemanticTree,
    diagnostics: fn(&SemanticNodeOutput) -> &[Diagnostic],
) -> Vec<Diagnostic> {
    let mut output = Vec::new();
    for node in &tree.nodes {
        let mut local = diagnostics(&node.output).to_vec();
        shift_diagnostics(&mut local, node.offset as isize);
        output.append(&mut local);
    }
    output.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    output
}

fn reduce_lists(tree: &SemanticTree) -> ListOutput {
    let mut groups = Vec::new();
    let mut pending: Option<ListGroup> = None;
    for node in &tree.nodes {
        let mut local_groups = node.output.lists.groups.iter().collect::<Vec<_>>();
        for group in &mut local_groups {
            shift_list_group(group, node.offset as isize);
        }
        match root_list_role(&node.syntax) {
            RootListRole::Transparent => groups.append(&mut local_groups),
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
                let root_index = local_groups
                    .iter()
                    .position(|group| group.range.start == root_start)
                    .expect("top-level list item produces a root group");
                let mut root = local_groups.remove(root_index);
                match &mut pending {
                    Some(current) if current.kind == kind => {
                        current.range.end = root.range.end;
                        current.items.append(&mut root.items);
                    }
                    Some(_) => {
                        groups.push(pending.take().expect("pending group exists"));
                        pending = Some(root);
                    }
                    None => pending = Some(root),
                }
                groups.append(&mut local_groups);
            }
            RootListRole::Other => {
                if let Some(group) = pending.take() {
                    groups.push(group);
                }
                groups.append(&mut local_groups);
            }
        }
    }
    if let Some(group) = pending {
        groups.push(group);
    }
    groups.sort_by_key(|group| group.range.start);
    ListOutput {
        groups: ListGroups::from_owned(groups),
    }
}

#[derive(Clone, Copy)]
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

fn shift_list_group(group: &mut ListGroup, delta: isize) {
    shift_range(&mut group.range, delta);
    for item in &mut group.items {
        shift_range(&mut item.range, delta);
        shift_range(&mut item.selection_range, delta);
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
        assert_eq!(output.event_link_ranges.len(), 2);

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
