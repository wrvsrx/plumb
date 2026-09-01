use std::{ops::Range, path::PathBuf};

use plumb_syntax::{parse, Block, Document, Inline, InlineContent, ParsedBlock, VerbatimBlock};
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
        valid_cases.len() >= 11,
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
    let children = document.blocks.iter().map(project_block).collect();
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
        project_inline_content(&block.content, &mut children);
        children.extend(block.children.iter().map(project_block));
        "marked_block"
    } else {
        project_inline_content(&block.content, &mut children);
        children.extend(block.children.iter().map(project_block));
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
        range: Some(block.opener_range.start..block.opener_range.start + 1),
        children: Vec::new(),
    }];
    if let Some(mark) = &block.mark {
        children.push(ProjectedNode {
            kind: "verbatim_kind",
            range: Some(mark.marker_range.clone()),
            children: Vec::new(),
        });
    }
    ProjectedNode {
        kind: "verbatim_block",
        range: None,
        children,
    }
}

fn project_inline_content(content: &InlineContent, output: &mut Vec<ProjectedNode>) {
    for inline in &content.items {
        match inline {
            Inline::Group {
                range,
                mark,
                content,
            } => {
                let mut children = Vec::new();
                if let Some(mark) = mark {
                    children.push(ProjectedNode {
                        kind: "introducer",
                        range: Some(mark.range.start..mark.marker_range.start),
                        children: Vec::new(),
                    });
                    children.push(ProjectedNode {
                        kind: "inline_kind",
                        range: Some(mark.marker_range.clone()),
                        children: Vec::new(),
                    });
                }
                project_inline_content(content, &mut children);
                output.push(ProjectedNode {
                    kind: if mark.is_some() {
                        "marked_group"
                    } else {
                        "anonymous_group"
                    },
                    range: Some(range.clone()),
                    children,
                });
            }
            Inline::Verbatim { range, mark, .. } => {
                let mut children = Vec::new();
                children.push(ProjectedNode {
                    kind: "introducer",
                    range: Some(range.start..range.start + 1),
                    children: Vec::new(),
                });
                if let Some(mark) = mark {
                    children.push(ProjectedNode {
                        kind: "verbatim_kind",
                        range: Some(mark.marker_range.clone()),
                        children: Vec::new(),
                    });
                }
                output.push(ProjectedNode {
                    kind: "inline_verbatim",
                    range: Some(range.clone()),
                    children,
                });
            }
            Inline::Text { .. } | Inline::Space { .. } => {}
        }
    }
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
        "marked_group" => ("marked_group", true),
        "anonymous_group" => ("anonymous_group", true),
        "inline_verbatim" => ("inline_verbatim", true),
        "introducer" => ("introducer", true),
        "marker" => ("marker", true),
        "inline_kind" => ("inline_kind", true),
        "verbatim_kind" => ("verbatim_kind", true),
        _ => return None,
    })
}
