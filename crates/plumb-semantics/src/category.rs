//! Source-backed accounting category; absence and malformed declarations differ.
use plumb_syntax::{Block, Inline};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub values: Vec<String>,
    pub declarations: Vec<Range<usize>>,
    pub invalid: bool,
}
impl Category {
    pub(crate) fn from_blocks(blocks: &[Block]) -> Self {
        let mut result = Self::default();
        for block in blocks {
            let Block::Parsed(property) = block else {
                continue;
            };
            if !property.mark.as_ref().is_some_and(|m| m.marker == "=") {
                continue;
            }
            let Some((key, _, scalar)) = crate::metadata::direct_property_parts(property) else {
                continue;
            };
            if key != "category" {
                continue;
            }
            result.declarations.push(property.range.clone());
            let mut values = Vec::new();
            if let Some(c) = scalar {
                if plain_scalar(&c.items) && !c.plain_text().trim().is_empty() {
                    values.push(c.plain_text().trim().to_owned());
                } else {
                    result.invalid = true;
                }
            } else {
                for child in &property.children {
                    let Block::Parsed(item) = child else {
                        result.invalid = true;
                        continue;
                    };
                    let text = item.content.plain_text().trim().to_owned();
                    if !item.mark.as_ref().is_some_and(|m| m.marker == "-")
                        || !item.children.is_empty()
                        || !plain_scalar(&item.content.items)
                        || text.is_empty()
                    {
                        result.invalid = true;
                    } else if !values.contains(&text) {
                        values.push(text);
                    }
                }
            }
            result.invalid |= values.is_empty() || result.declarations.len() > 1;
            if result.values.is_empty() {
                result.values = values;
            }
        }
        result
    }
    pub(crate) fn shift(&mut self, delta: isize) {
        for r in &mut self.declarations {
            r.start = r.start.checked_add_signed(delta).unwrap();
            r.end = r.end.checked_add_signed(delta).unwrap();
        }
    }
    pub(crate) fn extend(&mut self, other: Self) {
        self.invalid |=
            other.invalid || (!self.declarations.is_empty() && !other.declarations.is_empty());
        if self.values.is_empty() {
            self.values = other.values;
        }
        self.declarations.extend(other.declarations);
    }
}

fn plain_scalar(inlines: &[Inline]) -> bool {
    let mut pending = inlines.iter().collect::<Vec<_>>();
    while let Some(inline) = pending.pop() {
        match inline {
            Inline::Text { .. }
            | Inline::Space { .. }
            | Inline::SoftBreak { .. }
            | Inline::Verbatim { mark: None, .. } => {}
            Inline::Group {
                mark: None,
                content,
                ..
            } => pending.extend(&content.items),
            _ => return false,
        }
    }
    true
}
