use std::collections::HashMap;
use std::path::Path;

use plumb_semantics::{EmbedCompletionContext, EventTitleCompletionContext};

use crate::{
    fuzzy_match, normalize, CompletionCandidate, QueryResult, Workspace, WorkspaceQueryError,
};

const EVENT_TITLE_COMPLETION_LIMIT: usize = 50;

impl Workspace {
    pub fn complete_event_category(
        &self,
        context: &plumb_semantics::EventCategoryCompletionContext,
    ) -> Result<QueryResult<Vec<CompletionCandidate>>, WorkspaceQueryError> {
        let mut values = std::collections::BTreeSet::new();
        for entry in self.documents.values() {
            if let Some(versioned) = entry.current.as_ref().or(entry.last_valid.as_ref()) {
                values.extend(versioned.output.event_category_values());
            }
        }
        if let Some(store) = &self.disk_store {
            values.extend(store.event_category_values(
                &context.query,
                &self.documents.keys().cloned().collect::<Vec<_>>(),
            )?);
        }
        Ok(self.query_result(
            values
                .into_iter()
                .filter(|value| value.starts_with(&context.query) && value != &context.query)
                .map(|value| CompletionCandidate {
                    new_text: plumb_edit::render_authored_text_arguments(&[value.as_str()]),
                    label: value,
                    detail: "event category".to_owned(),
                    replace: context.replace.clone(),
                })
                .collect(),
        ))
    }

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

impl Workspace {
    /// Target identity remains a dependency even while the initial index is pending.
    pub fn link_completion_target(
        &self,
        from: impl AsRef<Path>,
        context: &plumb_semantics::LinkCompletionContext,
    ) -> Option<std::path::PathBuf> {
        link_anchor_target(from.as_ref(), context)
    }

    /// Only this identity can block an anchor completion. Missing targets are ready.
    pub fn pending_link_completion_target(
        &self,
        from: impl AsRef<Path>,
        context: &plumb_semantics::LinkCompletionContext,
    ) -> Option<std::path::PathBuf> {
        let target = link_anchor_target(from.as_ref(), context)?;
        self.document_analysis_pending(&target).then_some(target)
    }

    pub fn link_completion_work(&self) -> crate::LinkCompletionWork {
        self.derived.completion.work()
    }

    pub fn complete_link(
        &self,
        from: impl AsRef<Path>,
        context: &plumb_semantics::LinkCompletionContext,
    ) -> Result<QueryResult<Vec<CompletionCandidate>>, WorkspaceQueryError> {
        use crate::{
            escape_parsed_text, format_inline_verbatim, relative_path, valid_bare_attribute_value,
            valid_verbatim_link_completion_path, verbatim_payload_is_safe,
        };
        use plumb_semantics::LinkCompletionContext;
        let from = normalize(from.as_ref());
        let mut candidates = Vec::new();
        if let Some(target) = link_anchor_target(&from, context) {
            let (replace, query) = match context {
                LinkCompletionContext::Anchor { replace, query, .. }
                | LinkCompletionContext::VerbatimAnchor { replace, query, .. } => (replace, query),
                _ => unreachable!(),
            };
            if self.document_analysis_pending(&target) {
                return Ok(self.query_result_with_pending(candidates, true));
            }
            let anchors = if let Some(entry) = self.documents.get(&target) {
                entry
                    .current
                    .as_ref()
                    .expect("ready target")
                    .output
                    .anchors()
                    .iter()
                    .collect()
            } else if let Some(store) = &self.disk_store {
                store.anchors_for_path(&target)?
            } else {
                Vec::new()
            };
            candidates.extend(
                anchors
                    .into_iter()
                    .filter(|anchor| fuzzy_match(&anchor.id.value, query))
                    .map(|anchor| CompletionCandidate {
                        label: format!("#{}", anchor.id.value),
                        detail: format!("explicit anchor in {}", target.display()),
                        new_text: anchor.id.value,
                        replace: replace.clone(),
                    }),
            );
        } else {
            let query = match context {
                LinkCompletionContext::Path { query, .. }
                | LinkCompletionContext::SingleArgumentPath { query, .. }
                | LinkCompletionContext::VerbatimPath { query, .. } => query,
                _ => unreachable!(),
            };
            for (path, title) in self.derived.completion.paths(self, query)? {
                if path == from {
                    continue;
                }
                let Some(relative) = relative_path(&from, &path) else {
                    continue;
                };
                let title = if title.is_empty() {
                    relative.clone()
                } else {
                    title
                };
                if !fuzzy_match(&relative, query) && !fuzzy_match(&title, query) {
                    continue;
                }
                let (new_text, replace) = match context {
                    LinkCompletionContext::Path {
                        parsed, replace, ..
                    } => {
                        if !*parsed && !valid_bare_attribute_value(&relative) {
                            continue;
                        }
                        (
                            if *parsed {
                                escape_parsed_text(&relative)
                            } else {
                                relative.clone()
                            },
                            replace.clone(),
                        )
                    }
                    LinkCompletionContext::SingleArgumentPath {
                        replace, suffix, ..
                    } => (
                        plumb_edit::render_authored_text_arguments(&[&format!(
                            "{relative}{suffix}"
                        )]),
                        replace.clone(),
                    ),
                    LinkCompletionContext::VerbatimPath {
                        replace,
                        envelope,
                        quote_count,
                        suffix,
                        ..
                    } => {
                        if !valid_verbatim_link_completion_path(&relative) {
                            continue;
                        }
                        let payload = format!("{relative}{suffix}");
                        if verbatim_payload_is_safe(&payload, *quote_count) {
                            (relative.clone(), replace.clone())
                        } else {
                            (format_inline_verbatim(&payload), envelope.clone())
                        }
                    }
                    _ => unreachable!(),
                };
                candidates.push(CompletionCandidate {
                    label: relative,
                    detail: title,
                    new_text,
                    replace,
                });
            }
        }
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        Ok(self.query_result_with_pending(candidates, false))
    }
}

fn link_anchor_target(
    from: &Path,
    context: &plumb_semantics::LinkCompletionContext,
) -> Option<std::path::PathBuf> {
    use plumb_semantics::LinkCompletionContext;
    match context {
        LinkCompletionContext::Anchor { path, .. }
        | LinkCompletionContext::VerbatimAnchor { path, .. } => Some(if path.is_empty() {
            normalize(from)
        } else {
            crate::resolve_relative(&normalize(from), path)
        }),
        _ => None,
    }
}
