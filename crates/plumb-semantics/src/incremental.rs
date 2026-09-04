use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use plumb_syntax::{Diagnostic, GreenDocument, GreenShard};

use crate::{
    analyze_citations, analyze_inline_styles, analyze_lists, analyze_math, analyze_quotes,
    analyze_tasks, CitationOutput, InlineStyleOutput, ListGroup, ListGroups, ListKind, ListOutput,
    MathOutput, QuoteOutput, TaskOutput,
};

#[derive(Debug, Clone)]
pub struct GreenLocalRevision {
    shards: Vec<GreenLocalShard>,
    index: HashMap<usize, usize>,
    cache_hits: usize,
}

#[derive(Debug, Clone)]
struct GreenLocalShard {
    _syntax: Arc<GreenShard>,
    offset: usize,
    output: Arc<GreenLocalOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GreenLocalOutput {
    pub citations: CitationOutput,
    pub inline_styles: InlineStyleOutput,
    pub math: MathOutput,
    pub quotes: QuoteOutput,
    pub tasks: TaskOutput,
}

#[derive(Debug, Clone)]
pub struct GreenListRevision {
    shards: Vec<GreenListShard>,
    index: HashMap<usize, usize>,
    cache_hits: usize,
}

#[derive(Debug, Clone)]
struct GreenListShard {
    syntax: Arc<GreenShard>,
    offset: usize,
    output: Arc<ListOutput>,
}

impl GreenListRevision {
    pub fn analyze(document: &GreenDocument, previous: Option<&Self>) -> Option<Self> {
        if !document.is_valid() {
            return None;
        }
        let mut index = HashMap::with_capacity(document.shards().len());
        let mut cache_hits = 0;
        let shards = document
            .shards()
            .enumerate()
            .map(|(shard_index, view)| {
                let syntax = Arc::clone(view.shard());
                let identity = Arc::as_ptr(&syntax) as usize;
                let output = previous
                    .and_then(|previous| {
                        previous
                            .index
                            .get(&identity)
                            .map(|index| Arc::clone(&previous.shards[*index].output))
                    })
                    .map(|output| {
                        cache_hits += 1;
                        output
                    })
                    .unwrap_or_else(|| {
                        Arc::new(analyze_lists(
                            syntax
                                .parsed()
                                .valid_syntax()
                                .expect("valid green document has valid shards"),
                        ))
                    });
                index.insert(identity, shard_index);
                GreenListShard {
                    syntax,
                    offset: view.offset(),
                    output,
                }
            })
            .collect();
        Some(Self {
            shards,
            index,
            cache_hits,
        })
    }

    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    pub fn materialize(&self) -> ListOutput {
        let mut groups = Vec::new();
        let mut pending: Option<ListGroup> = None;
        for shard in &self.shards {
            let mut local = (*shard.output).clone();
            shift_lists(&mut local, shard.offset as isize);
            let mut local_groups = local.groups.into_owned();
            match root_list_role(&shard.syntax) {
                RootListRole::Transparent => groups.append(&mut local_groups),
                RootListRole::List(kind) => {
                    let root_start = shard.offset
                        + shard
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
}

#[derive(Clone, Copy)]
enum RootListRole {
    Transparent,
    List(ListKind),
    Other,
}

fn root_list_role(shard: &GreenShard) -> RootListRole {
    let blocks = &shard.parsed().syntax.blocks;
    let Some(block) = blocks.first() else {
        return RootListRole::Transparent;
    };
    if crate::is_document_declaration(block) {
        return RootListRole::Transparent;
    }
    let plumb_syntax::Block::Parsed(block) = block else {
        return RootListRole::Other;
    };
    match block.mark.as_ref().map(|mark| mark.marker.as_str()) {
        Some("-") => RootListRole::List(ListKind::Bullet),
        Some(".") => RootListRole::List(ListKind::Ordered),
        _ => RootListRole::Other,
    }
}

impl GreenLocalRevision {
    pub fn analyze(document: &GreenDocument, previous: Option<&Self>) -> Option<Self> {
        if !document.is_valid() {
            return None;
        }
        let mut index = HashMap::with_capacity(document.shards().len());
        let mut cache_hits = 0;
        let shards = document
            .shards()
            .enumerate()
            .map(|(shard_index, view)| {
                let syntax = Arc::clone(view.shard());
                let identity = Arc::as_ptr(&syntax) as usize;
                let output = previous
                    .and_then(|previous| {
                        previous
                            .index
                            .get(&identity)
                            .map(|index| Arc::clone(&previous.shards[*index].output))
                    })
                    .map(|output| {
                        cache_hits += 1;
                        output
                    })
                    .unwrap_or_else(|| {
                        let valid = syntax
                            .parsed()
                            .valid_syntax()
                            .expect("valid green document has valid shards");
                        Arc::new(GreenLocalOutput {
                            citations: analyze_citations(valid),
                            inline_styles: analyze_inline_styles(valid),
                            math: analyze_math(valid),
                            quotes: analyze_quotes(valid),
                            tasks: analyze_tasks(valid),
                        })
                    });
                index.insert(identity, shard_index);
                GreenLocalShard {
                    _syntax: syntax,
                    offset: view.offset(),
                    output,
                }
            })
            .collect();
        Some(Self {
            shards,
            index,
            cache_hits,
        })
    }

    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    pub fn materialize(&self) -> GreenLocalOutput {
        let mut output = GreenLocalOutput::default();
        for shard in &self.shards {
            let delta = shard.offset as isize;
            let mut local = (*shard.output).clone();
            shift_citations(&mut local.citations, delta);
            shift_inline_styles(&mut local.inline_styles, delta);
            shift_math(&mut local.math, delta);
            shift_quotes(&mut local.quotes, delta);
            shift_tasks(&mut local.tasks, delta);
            output
                .citations
                .citations
                .append(&mut local.citations.citations);
            output
                .citations
                .diagnostics
                .append(&mut local.citations.diagnostics);
            output
                .inline_styles
                .styles
                .append(&mut local.inline_styles.styles);
            output.math.records.append(&mut local.math.records);
            output.math.diagnostics.append(&mut local.math.diagnostics);
            output.quotes.quotes.append(&mut local.quotes.quotes);
            output.tasks.tasks.append(&mut local.tasks.tasks);
            output
                .tasks
                .diagnostics
                .append(&mut local.tasks.diagnostics);
        }
        output
    }
}

fn shift_citations(output: &mut CitationOutput, delta: isize) {
    for citation in &mut output.citations {
        shift_range(&mut citation.range, delta);
        shift_range(&mut citation.selection_range, delta);
    }
    shift_diagnostics(&mut output.diagnostics, delta);
}

fn shift_lists(output: &mut ListOutput, delta: isize) {
    let mut groups = std::mem::take(&mut output.groups).into_owned();
    for group in &mut groups {
        shift_range(&mut group.range, delta);
        for item in &mut group.items {
            shift_range(&mut item.range, delta);
            shift_range(&mut item.selection_range, delta);
        }
    }
    output.groups = ListGroups::from_owned(groups);
}

fn shift_inline_styles(output: &mut InlineStyleOutput, delta: isize) {
    for style in &mut output.styles {
        shift_range(&mut style.range, delta);
    }
}

fn shift_math(output: &mut MathOutput, delta: isize) {
    for record in &mut output.records {
        shift_range(&mut record.range, delta);
    }
    shift_diagnostics(&mut output.diagnostics, delta);
}

fn shift_quotes(output: &mut QuoteOutput, delta: isize) {
    for quote in &mut output.quotes {
        shift_range(&mut quote.range, delta);
    }
}

fn shift_tasks(output: &mut TaskOutput, delta: isize) {
    for task in &mut output.tasks {
        shift_range(&mut task.range, delta);
        shift_range(&mut task.marker_range, delta);
        shift_range(&mut task.selection_range, delta);
        task.attribute_insert = task.attribute_insert.checked_add_signed(delta).unwrap();
        shift_range(&mut task.attribute_range, delta);
        for field in [
            &mut task.id,
            &mut task.created,
            &mut task.due,
            &mut task.wait,
            &mut task.done,
            &mut task.canceled,
            &mut task.recur,
            &mut task.prev,
        ] {
            if let Some(field) = field {
                shift_range(&mut field.range, delta);
            }
        }
        for dependency in &mut task.depends {
            shift_range(&mut dependency.range, delta);
        }
    }
    shift_diagnostics(&mut output.diagnostics, delta);
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

#[cfg(test)]
mod tests {
    use plumb_syntax::GreenDocument;

    use super::*;

    #[test]
    fn green_local_revision_reuses_shards_and_matches_full_analyzers() {
        let old = "See `cite{paper} and `!{strong} with `$\"x\".\n\n`> Quote\n\n`- Task\n `+ task\n `@ task\n";
        let green = GreenDocument::parse(old);
        let previous = GreenLocalRevision::analyze(&green, None).unwrap();
        assert_local_parity(&green, &previous);

        let new = old.replace("Task", "Changed task");
        let current_green = green.reparse(new).document;
        let current = GreenLocalRevision::analyze(&current_green, Some(&previous)).unwrap();
        assert_eq!(current.cache_hits() + 1, current.shard_count());
        assert_local_parity(&current_green, &current);
    }

    #[test]
    fn green_list_revision_reduces_cross_shard_runs_like_full_analysis() {
        let old =
            "`- First\n `- Nested\n\n`= title Between\n\n`- Second\n\nParagraph\n\n`. Ordered\n";
        let green = GreenDocument::parse(old);
        let previous = GreenListRevision::analyze(&green, None).unwrap();
        assert_list_parity(&green, &previous);

        let new = old.replace("Second", "Changed second");
        let current_green = green.reparse(new).document;
        let current = GreenListRevision::analyze(&current_green, Some(&previous)).unwrap();
        assert_eq!(current.cache_hits() + 1, current.shards.len());
        assert_list_parity(&current_green, &current);
    }

    fn assert_local_parity(syntax: &GreenDocument, revision: &GreenLocalRevision) {
        let parsed = syntax.materialize();
        let valid = parsed.valid_syntax().unwrap();
        let output = revision.materialize();
        assert_eq!(output.citations, analyze_citations(valid));
        assert_eq!(output.inline_styles, analyze_inline_styles(valid));
        assert_eq!(output.math, analyze_math(valid));
        assert_eq!(output.quotes, analyze_quotes(valid));
        assert_eq!(output.tasks, analyze_tasks(valid));
    }

    fn assert_list_parity(syntax: &GreenDocument, revision: &GreenListRevision) {
        let parsed = syntax.materialize();
        let valid = parsed.valid_syntax().unwrap();
        assert_eq!(revision.materialize(), analyze_lists(valid));
    }
}
