use std::{ops::Range, path::PathBuf};

use plumb_syntax::{
    parse, AttachedContent, AttachedGroup, Attributes, Block, Document, Inline, InlineContent,
    InlineMember, ParsedBlock, VerbatimBlock,
};
use serde::Deserialize;
use tree_sitter::{Language, Node, Parser};
use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_plumb() -> *const ();
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    source: String,
    valid: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct ProjectedNode {
    kind: &'static str,
    range: Option<Range<usize>>,
    children: Vec<ProjectedNode>,
}

#[test]
fn strict_valid_trees_align_with_tree_sitter() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let corpus = manifest.join("../../../crates/plumb-syntax/tests/fixtures/strict-parser.json");
    let cases: Vec<Case> =
        serde_json::from_slice(&std::fs::read(corpus).expect("read strict parser corpus"))
            .expect("parse strict parser corpus");

    let language_fn = unsafe { LanguageFn::from_raw(tree_sitter_plumb) };
    let language: Language = language_fn.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("load generated plumb grammar");

    let valid_cases = cases
        .into_iter()
        .filter(|case| case.valid)
        .collect::<Vec<_>>();
    assert!(
        valid_cases.len() >= 17,
        "strict alignment corpus unexpectedly shrank"
    );

    for case in &valid_cases {
        let strict = parse(case.source.clone());
        assert!(strict.is_valid(), "{} must be strict-valid", case.name);
        let expected = project_document(&strict.syntax);

        let tree = parser.parse(&case.source, None).expect("tree-sitter parse");
        let root = tree.root_node();
        assert!(
            !root.has_error(),
            "{} produced a tree-sitter error: {}",
            case.name,
            root.to_sexp()
        );
        let mut actual_roots = project_tree_sitter(root);
        assert_eq!(actual_roots.len(), 1, "{} document root count", case.name);
        let actual = actual_roots.pop().expect("tree-sitter document projection");
        assert_eq!(
            actual,
            expected,
            "{} structural CST mismatch\ntree-sitter: {}",
            case.name,
            root.to_sexp()
        );
    }

    println!("aligned {} strict-valid cases", valid_cases.len());
}

fn project_document(document: &Document) -> ProjectedNode {
    let mut children = Vec::new();
    project_attributes(&document.attrs, &mut children);
    children.extend(document.blocks.iter().map(project_block));
    ProjectedNode {
        kind: "document",
        range: None,
        children,
    }
}

fn project_block(block: &Block) -> ProjectedNode {
    match block {
        Block::Parsed(block) => project_parsed_block(block),
        Block::Verbatim(block) => project_verbatim_block(block),
    }
}

fn project_parsed_block(block: &ParsedBlock) -> ProjectedNode {
    let mut children = Vec::new();
    let kind = if let Some(mark) = &block.mark {
        children.push(ProjectedNode {
            kind: "introducer",
            range: Some(mark.range.start..mark.marker_range.start),
            children: Vec::new(),
        });
        children.push(ProjectedNode {
            kind: "marker",
            range: Some(mark.marker_range.clone()),
            children: Vec::new(),
        });
        project_inline_content(&block.head, &mut children);
        project_attributes(&mark.attrs, &mut children);
        children.extend(block.children.iter().map(project_block));
        "marked_block"
    } else {
        project_inline_content(&block.head, &mut children);
        "paragraph"
    };
    ProjectedNode {
        kind,
        range: None,
        children,
    }
}

fn project_verbatim_block(block: &VerbatimBlock) -> ProjectedNode {
    let mut children = vec![ProjectedNode {
        kind: "introducer",
        range: Some(block.opener_range.start..block.kind_range.start),
        children: Vec::new(),
    }];
    if !block.kind_range.is_empty() {
        children.push(ProjectedNode {
            kind: "verbatim_kind",
            range: Some(block.kind_range.clone()),
            children: Vec::new(),
        });
    }
    project_attributes(&block.attrs, &mut children);
    ProjectedNode {
        kind: "verbatim_block",
        range: None,
        children,
    }
}

fn project_attributes(attributes: &Attributes, output: &mut Vec<ProjectedNode>) {
    let Some(attached) = attributes.attached.as_deref() else {
        return;
    };
    output.push(project_attached(attached));
}

fn project_attached(attached: &AttachedGroup) -> ProjectedNode {
    let (kind, children) = match &attached.content {
        AttachedContent::Blocks(blocks) => (
            "attached_block_group",
            blocks.iter().map(project_block).collect(),
        ),
        AttachedContent::Inlines(content) => {
            let mut children = Vec::new();
            project_inline_content(content, &mut children);
            ("attached_inline_group", children)
        }
    };
    ProjectedNode {
        kind,
        range: Some(attached.open_range.start..attached.close_range.end),
        children,
    }
}

fn project_inline_content(content: &InlineContent, output: &mut Vec<ProjectedNode>) {
    for inline in &content.items {
        match inline {
            Inline::Element {
                range,
                kind_range,
                members,
                ..
            } => {
                let mut children = Vec::new();
                if range.start < kind_range.start {
                    children.push(ProjectedNode {
                        kind: "introducer",
                        range: Some(range.start..kind_range.start),
                        children: Vec::new(),
                    });
                }
                children.push(ProjectedNode {
                    kind: "inline_kind",
                    range: Some(kind_range.clone()),
                    children: Vec::new(),
                });
                for member in members {
                    match member {
                        InlineMember::ParsedArgument(argument) => {
                            project_inline_content(&argument.content, &mut children);
                        }
                        InlineMember::VerbatimArgument(_) => {}
                        InlineMember::Child { inline, .. } => {
                            project_inline(inline, &mut children);
                        }
                    }
                }
                output.push(ProjectedNode {
                    kind: "inline_element",
                    range: Some(range.clone()),
                    children,
                });
            }
            Inline::Verbatim {
                range,
                kind_range,
                ..
            } => {
                let mut children = Vec::new();
                if range.start < kind_range.start {
                    children.push(ProjectedNode {
                        kind: "introducer",
                        range: Some(range.start..kind_range.start),
                        children: Vec::new(),
                    });
                }
                if !kind_range.is_empty() {
                    children.push(ProjectedNode {
                        kind: "verbatim_kind",
                        range: Some(kind_range.clone()),
                        children: Vec::new(),
                    });
                }
                output.push(ProjectedNode {
                    kind: "inline_verbatim",
                    range: Some(range.clone()),
                    children,
                });
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

fn project_inline(inline: &Inline, output: &mut Vec<ProjectedNode>) {
    let range = match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range.clone(),
    };
    project_inline_content(
        &InlineContent {
            range,
            items: vec![inline.clone()],
        },
        output,
    );
}

fn project_tree_sitter(node: Node<'_>) -> Vec<ProjectedNode> {
    let mut cursor = node.walk();
    let children = node
        .named_children(&mut cursor)
        .flat_map(project_tree_sitter)
        .collect::<Vec<_>>();
    let Some((kind, compare_range)) = projected_kind(node.kind()) else {
        return children;
    };
    vec![ProjectedNode {
        kind,
        range: compare_range.then(|| node.byte_range()),
        children,
    }]
}

fn projected_kind(kind: &str) -> Option<(&'static str, bool)> {
    Some(match kind {
        "document" => ("document", false),
        "paragraph" => ("paragraph", false),
        "marked_block" => ("marked_block", false),
        "verbatim_block" => ("verbatim_block", false),
        "attached_block_group" => ("attached_block_group", true),
        "attached_inline_group" => ("attached_inline_group", true),
        "inline_element" => ("inline_element", true),
        "inline_verbatim" => ("inline_verbatim", true),
        "introducer" => ("introducer", true),
        "marker" => ("marker", true),
        "inline_kind" => ("inline_kind", true),
        "verbatim_kind" => ("verbatim_kind", true),
        _ => return None,
    })
}
