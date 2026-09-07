use std::ops::Range;
use std::sync::Arc;

use crate::document::SemanticTree;
use plumb_syntax::{Block, InlineContent, ParsedBlock, ValidDocument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRecord {
    pub range: Range<usize>,
    pub term: InlineContent,
    pub term_range: Range<usize>,
    pub inline_body: Option<InlineContent>,
    pub body_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionList {
    pub range: Range<usize>,
    pub definitions: Vec<DefinitionRecord>,
}

/// Definition body semantics, independent of document metadata and inherited context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DefinitionOutput {
    pub groups: DefinitionGroups,
}

#[derive(Clone)]
enum Storage {
    Empty,
    Owned(Arc<[DefinitionList]>),
    Reduced {
        tree: Arc<SemanticTree>,
        groups: Arc<[Vec<(usize, usize)>]>,
    },
}

#[derive(Clone)]
pub struct DefinitionGroups {
    storage: Storage,
}

impl Default for DefinitionGroups {
    fn default() -> Self {
        Self {
            storage: Storage::Empty,
        }
    }
}
impl std::fmt::Debug for DefinitionGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefinitionGroups")
            .field("len", &self.len())
            .finish()
    }
}
impl PartialEq for DefinitionGroups {
    fn eq(&self, other: &Self) -> bool {
        match (&self.storage, &other.storage) {
            (Storage::Empty, Storage::Empty) => true,
            (Storage::Owned(left), Storage::Owned(right)) => left == right,
            _ => self.iter().eq(other.iter()),
        }
    }
}
impl Eq for DefinitionGroups {}

#[cfg(test)]
mod tests {
    use super::*;
    use plumb_syntax::parse;
    #[test]
    fn definitions_use_head_arguments_or_children_for_their_body() {
        let source = "`: term inline body\n`: {term with spaces}\n\n child body\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_definitions(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let group = output.groups.get(0).unwrap();
        let definitions = &group.definitions;
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].term.plain_text(), "term");
        assert_eq!(
            definitions[0]
                .inline_body
                .as_ref()
                .map(InlineContent::plain_text)
                .as_deref(),
            Some("inline body")
        );
        assert_eq!(
            &parsed.source[definitions[0].body_range.clone()],
            "inline body"
        );
        assert_eq!(definitions[1].term.plain_text(), "term with spaces");
        assert!(definitions[1].inline_body.is_none());
        assert_eq!(
            &parsed.source[definitions[1].term_range.clone()],
            "term with spaces"
        );
    }
}

impl DefinitionGroups {
    #[cfg(test)]
    pub(crate) fn shares_storage(&self, other: &Self) -> bool {
        match (&self.storage, &other.storage) {
            (Storage::Empty, Storage::Empty) => true,
            (Storage::Owned(left), Storage::Owned(right)) => Arc::ptr_eq(left, right),
            (Storage::Reduced { groups: left, .. }, Storage::Reduced { groups: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            _ => false,
        }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Empty => 0,
            Storage::Owned(groups) => groups.len(),
            Storage::Reduced { groups, .. } => groups.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = DefinitionList> + '_ {
        (0..self.len()).map(|index| self.get(index).expect("group index is in bounds"))
    }
    pub fn get(&self, index: usize) -> Option<DefinitionList> {
        match &self.storage {
            Storage::Empty => None,
            Storage::Owned(groups) => groups.get(index).cloned(),
            Storage::Reduced { tree, groups } => {
                let segments = groups.get(index)?;
                let mut result: Option<DefinitionList> = None;
                for &(node, group) in segments {
                    let (offset, local) = tree.definition_group_segment(node, group);
                    let mut projected = local.clone();
                    shift_definition_list(&mut projected, offset);
                    if let Some(result) = &mut result {
                        result.range.end = projected.range.end;
                        result.definitions.extend(projected.definitions);
                    } else {
                        result = Some(projected);
                    }
                }
                result
            }
        }
    }
    fn start(&self, index: usize) -> usize {
        match &self.storage {
            Storage::Empty => unreachable!("empty collection has no start"),
            Storage::Owned(groups) => groups[index].range.start,
            Storage::Reduced { tree, groups } => {
                let (node, group) = groups[index][0];
                let (offset, local) = tree.definition_group_segment(node, group);
                local.range.start.checked_add_signed(offset).unwrap()
            }
        }
    }
    pub(crate) fn owned(&self, index: usize) -> &DefinitionList {
        match &self.storage {
            Storage::Owned(groups) => &groups[index],
            Storage::Reduced { .. } | Storage::Empty => {
                panic!("local definition storage must be nonempty and owned")
            }
        }
    }
    pub(crate) fn rebind(&self, tree: Arc<SemanticTree>) -> Self {
        match &self.storage {
            Storage::Empty => Self::default(),
            Storage::Reduced { groups, .. } => Self {
                storage: Storage::Reduced {
                    tree,
                    groups: Arc::clone(groups),
                },
            },
            Storage::Owned(groups) if groups.is_empty() => Self::default(),
            Storage::Owned(_) => panic!("document definitions must have a tree binding"),
        }
    }
    pub(crate) fn reduce(tree: Arc<SemanticTree>) -> Self {
        let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut pending: Option<usize> = None;
        for (node, role, count) in tree.definition_nodes() {
            match role {
                RootRole::Definition => {
                    if let Some(index) = pending {
                        groups[index].push((node, 0));
                    } else {
                        pending = Some(groups.len());
                        groups.push(vec![(node, 0)]);
                    }
                }
                RootRole::Other => pending = None,
                RootRole::Transparent => {}
            }
            let nested_start = usize::from(role == RootRole::Definition);
            groups.extend((nested_start..count).map(|group| vec![(node, group)]));
        }
        // Each new group is first encountered in source order; extended outer groups
        // keep their original position before nested groups.
        if groups.is_empty() {
            Self::default()
        } else {
            Self {
                storage: Storage::Reduced {
                    tree,
                    groups: groups.into(),
                },
            }
        }
    }
}

impl DefinitionOutput {
    pub fn group_at_node_start(&self, start: usize) -> Option<DefinitionList> {
        let mut lo = 0;
        let mut hi = self.groups.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.groups.start(mid) < start {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lo < self.groups.len() && self.groups.start(lo) == start)
            .then(|| self.groups.get(lo).unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootRole {
    Definition,
    Transparent,
    Other,
}

pub(crate) fn root_role(shard: &plumb_syntax::GreenShard) -> RootRole {
    match shard.parsed().syntax.blocks.first() {
        Some(block) if crate::is_document_declaration(block) => RootRole::Transparent,
        Some(block) if definition_block(block).is_some() => RootRole::Definition,
        None => RootRole::Transparent,
        Some(_) => RootRole::Other,
    }
}

pub fn analyze_definitions(valid: ValidDocument<'_>) -> DefinitionOutput {
    let mut groups = Vec::new();
    collect_definition_lists(
        valid
            .syntax()
            .blocks
            .iter()
            .filter(|block| !crate::is_document_declaration(block)),
        &mut groups,
    );
    groups.sort_by_key(|group| group.range.start);
    if groups.is_empty() {
        return DefinitionOutput::default();
    }
    DefinitionOutput {
        groups: DefinitionGroups {
            storage: Storage::Owned(groups.into()),
        },
    }
}

fn collect_definition_lists<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    output: &mut Vec<DefinitionList>,
) {
    let mut blocks = blocks.into_iter().peekable();
    while let Some(current) = blocks.next() {
        if definition_block(current).is_none() {
            if let Block::Parsed(block) = current {
                collect_definition_lists(crate::body_children(block), output);
            }
            continue;
        }

        let mut definitions = Vec::new();
        let start = current.range().start;
        let mut current = Some(current);
        while let Some(block) = current.and_then(definition_block) {
            let (term, inline_body) = if block.children.is_empty() {
                split_inline_arguments(&block.content)
            } else {
                (block.content.trim_boundary_padding(), None)
            };
            let projected_body_range = inline_body
                .as_ref()
                .map_or_else(|| body_range(block), |body| body.range.clone());
            definitions.push(DefinitionRecord {
                range: block.range.clone(),
                term_range: crate::element_selection_range(&term),
                term,
                inline_body,
                body_range: projected_body_range,
            });
            collect_definition_lists(crate::body_children(block), output);
            current = blocks.next_if(|next| definition_block(next).is_some());
        }
        output.push(DefinitionList {
            range: start
                ..definitions
                    .last()
                    .expect("definition list is nonempty")
                    .range
                    .end,
            definitions,
        });
    }
}

fn definition_block(block: &Block) -> Option<&ParsedBlock> {
    let Block::Parsed(block) = block else {
        return None;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| mark.marker == ":")
        .then_some(block)
}

fn split_inline_arguments(content: &InlineContent) -> (InlineContent, Option<InlineContent>) {
    let view = crate::owner_semantic_view(content);
    let Some(arguments) = view.split_first() else {
        return (content.clone(), None);
    };
    (arguments.first.clone(), arguments.rest_content())
}
fn body_range(block: &ParsedBlock) -> Range<usize> {
    block
        .children
        .first()
        .zip(block.children.last())
        .map_or(block.range.end..block.range.end, |(first, last)| {
            first.range().start..last.range().end
        })
}

fn shift_definition_list(definitions: &mut DefinitionList, delta: isize) {
    shift_range(&mut definitions.range, delta);
    for definition in &mut definitions.definitions {
        shift_range(&mut definition.range, delta);
        crate::text::shift_inline_content(&mut definition.term, delta);
        shift_range(&mut definition.term_range, delta);
        if let Some(body) = &mut definition.inline_body {
            crate::text::shift_inline_content(body, delta);
        }
        shift_range(&mut definition.body_range, delta);
    }
}

fn shift_range(range: &mut Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}
