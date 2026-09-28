//! Shared revision-bound policy input nodes. Disk generations are conservative
//! whole-document nodes; resident documents retain strict syntax shard identity.
use super::*;
use plumb_semantics::SemanticNodeSnapshot;
use plumb_syntax::GreenShardId;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum NodeIdentity {
    Syntax(GreenShardId),
    Stored([u8; 32]),
}

#[derive(Clone, Debug)]
pub(super) enum PolicyNode {
    Resident(SemanticNodeSnapshot),
    Stored([u8; 32], Arc<Vec<EventRecord>>),
}
impl PolicyNode {
    pub fn id(&self) -> NodeIdentity {
        match self {
            Self::Resident(node) => NodeIdentity::Syntax(node.id()),
            Self::Stored(hash, _) => NodeIdentity::Stored(*hash),
        }
    }
    pub fn offset(&self) -> usize {
        match self {
            Self::Resident(node) => node.offset(),
            Self::Stored(..) => 0,
        }
    }
    pub fn event_count(&self) -> usize {
        match self {
            Self::Resident(node) => node.event_count(),
            Self::Stored(_, events) => events.len(),
        }
    }
    pub fn same_facts(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Resident(a), Self::Resident(b)) => a.same_facts(b),
            (Self::Stored(a, _), Self::Stored(b, _)) => a == b,
            _ => false,
        }
    }
    pub fn events(&self) -> Box<dyn Iterator<Item = EventRecord> + '_> {
        match self {
            Self::Resident(node) => Box::new(node.events()),
            Self::Stored(_, events) => Box::new(events.iter().cloned()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct PolicySources {
    stored: BTreeMap<PathBuf, ([u8; 32], Arc<Vec<EventRecord>>)>,
}

pub(super) struct PolicyInputs {
    pub nodes: Vec<(PathBuf, PolicyNode)>,
    pub issues: Vec<AgendaIssue>,
}

impl Workspace {
    pub(super) fn policy_inputs(
        &self,
        root: &Path,
        excluded: &[PathBuf],
        sources: &mut PolicySources,
    ) -> Result<PolicyInputs, String> {
        let root = normalize(root);
        let in_scope =
            |path: &Path| path.starts_with(&root) && !excluded.iter().any(|r| path.starts_with(r));
        let mut nodes = BTreeMap::<PathBuf, Vec<PolicyNode>>::new();
        let mut issues = Vec::new();
        let mut live_stored = BTreeSet::new();
        if let Some(store) = &self.disk_store {
            for doc in store.documents().map_err(|e| e.to_string())? {
                if !in_scope(&doc.path) || self.documents.contains_key(&doc.path) {
                    continue;
                }
                if !doc.valid {
                    issues.push(issue(
                        "agenda.invalid-document",
                        "document has no valid semantic output",
                        location(&doc.path, 0..0),
                    ));
                    continue;
                }
                live_stored.insert(doc.path.clone());
                let events = if let Some((_, events)) = sources
                    .stored
                    .get(&doc.path)
                    .filter(|(hash, _)| *hash == doc.content_hash)
                {
                    Arc::clone(events)
                } else {
                    let events = Arc::new(
                        store
                            .events_for_path(&doc.path)
                            .map_err(|e| e.to_string())?,
                    );
                    sources
                        .stored
                        .insert(doc.path.clone(), (doc.content_hash, Arc::clone(&events)));
                    events
                };
                nodes.insert(doc.path, vec![PolicyNode::Stored(doc.content_hash, events)]);
            }
        }
        sources.stored.retain(|path, _| live_stored.contains(path));
        for (path, entry) in &self.documents {
            if !in_scope(path) {
                continue;
            }
            let Some(current) = &entry.current else {
                issues.push(issue(
                    "agenda.invalid-document",
                    "document has no valid semantic output",
                    location(path, 0..0),
                ));
                continue;
            };
            nodes.insert(
                path.clone(),
                current
                    .output
                    .semantic_nodes()
                    .filter(|node| node.event_count() > 0)
                    .map(PolicyNode::Resident)
                    .collect(),
            );
        }
        issues.sort_by(|a, b| a.source.cmp(&b.source));
        Ok(PolicyInputs {
            nodes: nodes
                .into_iter()
                .flat_map(|(path, nodes)| nodes.into_iter().map(move |node| (path.clone(), node)))
                .filter(|(_, node)| node.event_count() > 0)
                .collect(),
            issues,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) enum DocumentStamp {
    Resident(Option<Arc<plumb_semantics::DocumentOutput>>),
    Stored([u8; 32], bool),
}
impl PartialEq for DocumentStamp {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Resident(Some(a)), Self::Resident(Some(b))) => Arc::ptr_eq(a, b),
            (Self::Resident(None), Self::Resident(None)) => true,
            (Self::Stored(a, av), Self::Stored(b, bv)) => a == b && av == bv,
            _ => false,
        }
    }
}
impl Workspace {
    pub(super) fn policy_document_stamps(
        &self,
    ) -> Result<BTreeMap<PathBuf, DocumentStamp>, String> {
        let mut result = BTreeMap::new();
        if let Some(store) = &self.disk_store {
            for doc in store.documents().map_err(|e| e.to_string())? {
                result.insert(doc.path, DocumentStamp::Stored(doc.content_hash, doc.valid));
            }
        }
        for (path, entry) in &self.documents {
            result.insert(
                path.clone(),
                DocumentStamp::Resident(
                    entry
                        .current
                        .as_ref()
                        .map(|current| Arc::clone(&current.output)),
                ),
            );
        }
        Ok(result)
    }
}
