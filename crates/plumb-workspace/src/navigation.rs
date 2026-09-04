use std::path::Path;

use plumb_semantics::{FileRecord, FileTarget, ImageRecord, ImageTarget};

use crate::{contains_inclusive, resolve_relative, ResolvedTarget, Workspace};

impl Workspace {
    pub fn resolve_image(&self, from: impl AsRef<Path>, image: &ImageRecord) -> ResolvedTarget {
        match &image.target_kind {
            ImageTarget::External => ResolvedTarget::External,
            ImageTarget::File { path } => {
                let target = resolve_relative(from.as_ref(), path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
        }
    }

    pub fn image_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&ImageRecord> {
        self.current_output(path.as_ref())?
            .images()
            .iter()
            .filter(|image| contains_inclusive(&image.range, offset))
            .max_by_key(|image| image.range.start)
    }

    pub fn resolve_file(&self, from: impl AsRef<Path>, file: &FileRecord) -> ResolvedTarget {
        match &file.target_kind {
            FileTarget::External => ResolvedTarget::External,
            FileTarget::File { path } => {
                let target = resolve_relative(from.as_ref(), path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
        }
    }

    pub fn file_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&FileRecord> {
        self.current_output(path.as_ref())?
            .files()
            .iter()
            .filter(|file| contains_inclusive(&file.range, offset))
            .max_by_key(|file| file.range.start)
    }
}
