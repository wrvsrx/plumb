use std::collections::HashMap;
use std::path::Path;

use plumb_semantics::{EmbedCompletionContext, EventTitleCompletionContext};

use crate::{
    fuzzy_match, normalize, CompletionCandidate, QueryResult, Workspace, WorkspaceQueryError,
};

const EVENT_TITLE_COMPLETION_LIMIT: usize = 50;

impl Workspace {
    pub fn complete_event_title(
        &self,
        context: &EventTitleCompletionContext,
    ) -> Result<QueryResult<Vec<CompletionCandidate>>, WorkspaceQueryError> {
        let excluded = self.documents.keys().cloned().collect::<Vec<_>>();
        let mut counts = HashMap::<String, usize>::new();
        for entry in self.documents.values() {
            let Some(versioned) = entry.current.as_ref().or(entry.last_valid.as_ref()) else {
                continue;
            };
            for event in versioned.output.events().events.views() {
                let title = event.title();
                if title.is_empty() || !title.starts_with(&context.query) || title == context.query
                {
                    continue;
                }
                if let Some(count) = counts.get_mut(title) {
                    *count += 1;
                } else {
                    counts.insert(title.to_owned(), 1);
                }
            }
        }
        if let Some(store) = &self.disk_store {
            for (title, count) in store.event_title_counts(&context.query, &excluded)? {
                *counts.entry(title).or_default() += count;
            }
        }
        let mut titles = counts
            .into_iter()
            .filter(|(title, _)| {
                title.starts_with(&context.query)
                    && (context.query.is_empty() || title != &context.query)
            })
            .collect::<Vec<_>>();
        titles.sort_by(|(left_title, left_count), (right_title, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| left_title.cmp(right_title))
        });
        titles.truncate(EVENT_TITLE_COMPLETION_LIMIT);
        Ok(self.query_result(
            titles
                .into_iter()
                .map(|(title, count)| CompletionCandidate {
                    label: title.clone(),
                    detail: format!("event title, {count} uses"),
                    new_text: title,
                    replace: context.replace.clone(),
                })
                .collect(),
        ))
    }

    pub fn complete_embed_path(
        &self,
        from: impl AsRef<Path>,
        context: &EmbedCompletionContext,
    ) -> Vec<CompletionCandidate> {
        self.complete_resource_path(from.as_ref(), context)
    }

    fn complete_resource_path(
        &self,
        from: &Path,
        context: &EmbedCompletionContext,
    ) -> Vec<CompletionCandidate> {
        let from = normalize(from);
        if Path::new(&context.query).is_absolute() {
            return Vec::new();
        }
        let (directory_prefix, name_query) = context
            .query
            .rsplit_once('/')
            .map_or(("", context.query.as_str()), |(directory, name)| {
                (&context.query[..directory.len() + 1], name)
            });
        let directory = normalize(
            &from
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(directory_prefix),
        );
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut candidates = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if !fuzzy_match(&name, name_query) {
                    return None;
                }
                let path = entry.path();
                let (suffix, detail) = if path.is_dir() {
                    ("/", "resource directory")
                } else if path.is_file() {
                    ("", "embed target")
                } else {
                    return None;
                };
                let path = format!("{directory_prefix}{name}{suffix}");
                if path
                    .chars()
                    .any(|character| character.is_control() || character == '\\')
                {
                    return None;
                }
                let new_text = plumb_edit::render_authored_text_arguments(&[path.as_str()]);
                Some(CompletionCandidate {
                    label: path,
                    detail: detail.to_string(),
                    new_text,
                    replace: context.replace.clone(),
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        candidates
    }
}

#[cfg(test)]
pub(crate) const TEST_EVENT_TITLE_COMPLETION_LIMIT: usize = EVENT_TITLE_COMPLETION_LIMIT;
