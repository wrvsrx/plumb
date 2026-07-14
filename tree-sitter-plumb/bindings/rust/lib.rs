//! Plumb language support for tree-sitter.

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_plumb() -> *const ();
}

/// The tree-sitter language function for plumb.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_plumb) };

/// Highlight query for plumb.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// Fold query for plumb.
pub const FOLDS_QUERY: &str = include_str!("../../queries/folds.scm");

/// Language injection query for code payloads.
pub const INJECTIONS_QUERY: &str = include_str!("../../queries/injections.scm");

#[cfg(test)]
mod tests {
    #[test]
    fn loads_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("plumb grammar should load");
        let tree = parser.parse("`# heading\n", None).expect("parse should produce a tree");
        assert!(!tree.root_node().has_error());
    }
}
