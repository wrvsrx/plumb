//! Read-only migration inventory. Emits tab-separated Rust string literals.
use plumb_syntax::{Block, Inline};
use std::path::Path;
fn visit(path: &Path) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for entry in entries {
            if !entry.is_symlink() {
                visit(&entry);
            }
        }
    } else if path
        .extension()
        .is_some_and(|extension| extension == "plumb")
    {
        let source = std::fs::read_to_string(path).unwrap();
        let newlines: Vec<_> = source.bytes().enumerate().filter_map(|(i, b)| (b == b'\n').then_some(i)).collect();
        let parsed = plumb_syntax::parse(&source);
        if !parsed.is_valid() {
            eprintln!("Invalid syntax: {}", path.display());
            return;
        }
        let mut pending: Vec<_> = parsed.syntax.blocks.iter().collect();
        while let Some(block) = pending.pop() {
            let Block::Parsed(block) = block else {
                continue;
            };
            pending.extend(block.children.iter());
            if !block.children.is_empty() || !block.mark.as_ref().is_some_and(|m| m.marker == "=") {
                continue;
            }
            let elements: Vec<_> = block.content.positional_elements().collect();
            if elements.len() < 3 {
                continue;
            }
            let key = plumb_syntax::InlineContent::from_items(
                plumb_syntax::inline_range(elements[0]).clone(),
                vec![elements[0].clone()],
            )
            .plain_text();
            if key != "event-category" && key != "tags" {
                continue;
            }
            let start = plumb_syntax::inline_range(elements[1]).start;
            let end = plumb_syntax::inline_range(elements.last().unwrap()).end;
            let content = plumb_syntax::InlineContent::from_items(
                start..end,
                block
                    .content
                    .items
                    .iter()
                    .filter(|item| plumb_syntax::inline_range(item).start >= start)
                    .cloned()
                    .collect(),
            );
            let plain = content.plain_text().trim().to_string();
            let simple = elements[1..]
                .iter()
                .all(|e| matches!(e, Inline::Text { .. }));
            let replacement = plumb_edit::render_authored_text_arguments(&[&plain]);
            println!(
                "{:?}\t{}\t{:?}\t{:?}\t{:?}\t{:?}\t{}",
                path.to_string_lossy(),
                newlines.partition_point(|&position| position < start) + 1,
                key,
                &source[start..end],
                replacement,
                plain,
                simple
            );
        }
    }
}
fn main() {
    visit(Path::new(
        &std::env::args().nth(1).expect("notebook directory"),
    ));
}
