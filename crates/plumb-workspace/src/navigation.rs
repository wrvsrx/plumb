use std::path::Path;

use plumb_semantics::{EmbedRecord, EmbedTarget};

use crate::{contains_inclusive, resolve_relative, ResolvedTarget, Workspace};

impl Workspace {
    pub fn resolve_embed(&self, from: impl AsRef<Path>, embed: &EmbedRecord) -> ResolvedTarget {
        match &embed.target_kind {
            EmbedTarget::External => ResolvedTarget::External,
            EmbedTarget::File { path } => {
                let target = resolve_relative(from.as_ref(), path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
        }
    }

    pub fn embed_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<EmbedRecord> {
        self.current_output(path.as_ref())?
            .embeds()
            .iter()
            .filter(|embed| contains_inclusive(&embed.range, offset))
            .max_by_key(|embed| embed.range.start)
    }
}
