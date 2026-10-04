//! Current document identities and titles, maintained by source contribution.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use im::{OrdMap, OrdSet};

use crate::{Workspace, WorkspaceQueryError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkCompletionWork {
    pub documents_read: usize,
    pub stored_changes_read: usize,
    pub contributions_changed: usize,
    pub candidates_visited: usize,
}

#[derive(Debug, Clone, Default)]
struct Index {
    initialized: bool,
    identity: Option<Vec<u8>>,
    cursor: i64,
    dirty: OrdSet<PathBuf>,
    titles: OrdMap<PathBuf, String>,
    postings: OrdMap<char, OrdSet<PathBuf>>,
    work: LinkCompletionWork,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LinkCompletionIndex(Arc<Mutex<Index>>);

impl LinkCompletionIndex {
    pub fn fork(&self) -> Self {
        Self(Arc::new(Mutex::new(
            self.0.lock().expect("completion index").clone(),
        )))
    }

    pub fn changed(&mut self, path: PathBuf) {
        if Arc::get_mut(&mut self.0).is_none() {
            *self = self.fork();
        }
        Arc::get_mut(&mut self.0)
            .expect("exclusive completion index")
            .get_mut()
            .expect("completion index")
            .dirty
            .insert(path);
    }

    pub fn paths(
        &self,
        workspace: &Workspace,
        query: &str,
    ) -> Result<Vec<(PathBuf, String)>, WorkspaceQueryError> {
        let mut published = self.0.lock().expect("completion index");
        // Persistent collections make the transaction cheap. Never publish a
        // partial update if a store read fails midway through synchronization.
        let mut index = published.clone();
        index.synchronize(workspace)?;
        let characters: OrdSet<char> = query.chars().flat_map(char::to_lowercase).collect();
        let mut result = Vec::new();
        if characters.is_empty() {
            for (path, title) in &index.titles {
                result.push((path.clone(), title.clone()));
            }
            index.work.candidates_visited = result.len();
        } else if let Some(postings) = characters
            .iter()
            .map(|c| index.postings.get(c))
            .collect::<Option<Vec<_>>>()
        {
            let smallest = postings
                .iter()
                .min_by_key(|paths| paths.len())
                .expect("nonempty query");
            for path in smallest.iter() {
                index.work.candidates_visited += 1;
                if postings.iter().all(|paths| paths.contains(path)) {
                    result.push((path.clone(), index.titles[path].clone()));
                }
            }
        }
        *published = index;
        Ok(result)
    }

    pub fn work(&self) -> LinkCompletionWork {
        self.0.lock().expect("completion index").work
    }
}

fn characters(path: &Path, title: &str) -> OrdSet<char> {
    // Relative paths can introduce ../ independently of the indexed absolute path.
    path.to_string_lossy()
        .chars()
        .chain(title.chars())
        .chain("../".chars())
        .flat_map(char::to_lowercase)
        .collect()
}

impl Index {
    fn replace(&mut self, path: PathBuf, title: Option<String>) {
        if self.titles.get(&path) == title.as_ref() {
            return;
        }
        self.work.contributions_changed += 1;
        if let Some(previous) = self.titles.remove(&path) {
            for character in characters(&path, &previous) {
                let mut paths = self.postings[&character].clone();
                paths.remove(&path);
                if paths.is_empty() {
                    self.postings.remove(&character);
                } else {
                    self.postings.insert(character, paths);
                }
            }
        }
        if let Some(title) = title {
            for character in characters(&path, &title) {
                let mut paths = self.postings.get(&character).cloned().unwrap_or_default();
                paths.insert(path.clone());
                self.postings.insert(character, paths);
            }
            self.titles.insert(path, title);
        }
    }

    fn synchronize(&mut self, workspace: &Workspace) -> Result<(), WorkspaceQueryError> {
        self.work = LinkCompletionWork::default();
        let mut changed = self.dirty.clone();
        if let Some(store) = &workspace.disk_store {
            let requested: Vec<_> = changed
                .iter()
                .filter(|path| !workspace.documents.contains_key(*path))
                .cloned()
                .collect();
            let (identity, cursor, rebuild, rows) =
                store.completion_titles_since(self.identity.as_deref(), self.cursor, &requested)?;
            if rebuild {
                *self = Self::default();
                changed = workspace.documents.keys().cloned().collect();
            }
            self.work.stored_changes_read = rows.len();
            for (path, title) in rows {
                if !workspace.documents.contains_key(&path) {
                    self.work.documents_read += 1;
                    self.replace(path.clone(), title);
                    changed.remove(&path);
                }
            }
            self.identity = Some(identity);
            self.cursor = cursor;
        } else if self.identity.is_some() {
            *self = Self::default();
            changed = workspace.documents.keys().cloned().collect();
        }
        if !self.initialized {
            changed.extend(workspace.documents.keys().cloned());
        }
        for path in changed {
            self.work.documents_read += 1;
            let title = if let Some(entry) = workspace.documents.get(&path) {
                Some(entry.current.as_ref().map_or_else(
                    || {
                        plumb_semantics::green_completion_document_title(entry.parsed.green())
                            .unwrap_or_default()
                    },
                    |current| {
                        current
                            .output
                            .metadata()
                            .document_title()
                            .unwrap_or_default()
                    },
                ))
            } else {
                None
            };
            self.replace(path, title);
        }
        self.dirty.clear();
        self.initialized = true;
        Ok(())
    }
}
