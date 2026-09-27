use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CategoryValue {
    declared: bool,
    invalid: bool,
    values: Vec<String>,
}
impl From<Category> for CategoryValue {
    fn from(value: Category) -> Self {
        Self {
            declared: !value.declarations.is_empty(),
            invalid: value.invalid,
            values: value.values,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetValue {
    valid: bool,
    is_task: bool,
    category: CategoryValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Input {
    category: CategoryValue,
    tasks_override: bool,
    references: Vec<(String, TargetValue)>,
}

/// Successful category decisions survive revisions; ranges never enter a key.
/// Malformed inputs are recomputed so their precise issue locations stay current.
#[derive(Clone, Debug, Default)]
pub struct CategoryCheckState {
    entries: HashMap<(PathBuf, usize), (Input, bool)>,
    pub recomputed_events: usize,
}

impl Workspace {
    pub fn check_event_categories_incremental(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        state: &mut CategoryCheckState,
    ) -> Result<CategoryCheckReport, String> {
        self.check_event_categories_incremental_in_scope(root, now, &[], state)
    }

    pub(crate) fn check_event_categories_incremental_in_scope(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        excluded: &[PathBuf],
        state: &mut CategoryCheckState,
    ) -> Result<CategoryCheckReport, String> {
        let selected = self.selected_check_events(root, now, None, excluded)?;
        let mut report = CategoryCheckReport {
            complete: selected.complete,
            checked: selected.events.len(),
            missing: Vec::new(),
            issues: selected.issues,
        };
        let mut next = HashMap::with_capacity(selected.events.len());
        let mut context = AccountingContext::default();
        let mut targets = BTreeMap::<(PathBuf, String), TargetValue>::new();
        let mut recomputed = 0;
        let mut ordinals = BTreeMap::<PathBuf, usize>::new();
        for (path, event) in selected.events {
            let ordinal = ordinals.entry(path.clone()).or_default();
            let current_ordinal = *ordinal;
            *ordinal += 1;
            let category = if event.category.declarations.is_empty() {
                context.document_category(self, &path)?
            } else {
                event.category.clone()
            };
            let use_target_category = category.declarations.is_empty();
            let mut references = Vec::new();
            for (target, spelling, _) in self.accounting_references(&path, &event)? {
                let key = (path.clone(), spelling.clone());
                let mut value = if let Some(value) = targets.get(&key) {
                    value.clone()
                } else {
                    let mut value = TargetValue {
                        valid: false,
                        is_task: false,
                        category: Category::default().into(),
                    };
                    match self
                        .resolve_task_reference_target(&path, &target)
                        .map_err(|e| e.to_string())?
                    {
                        ResolvedTarget::Anchor { path, id, anchor } if anchor.list_item => {
                            value.valid = true;
                            value.is_task = context.is_task(self, &path, Some(&id))?;
                            value.category = if anchor.category.declarations.is_empty() {
                                context.document_category(self, &path)?
                            } else {
                                anchor.category
                            }
                            .into();
                        }
                        ResolvedTarget::Document { path } => {
                            value.valid = true;
                            value.is_task = context.is_task(self, &path, None)?;
                            value.category = context.document_category(self, &path)?.into();
                        }
                        _ => {}
                    }
                    targets.insert(key, value.clone());
                    value
                };
                if !use_target_category {
                    value.category = Category::default().into();
                }
                references.push((spelling, value));
            }
            let input = Input {
                category: category.into(),
                tasks_override: event.tasks_override,
                references,
            };
            let key = (path.clone(), current_ordinal);
            let cached = state.entries.get(&key).filter(|(old, _)| *old == input);
            let missing = if let Some((_, missing)) = cached {
                *missing
            } else {
                recomputed += 1;
                let (shares, issues) =
                    self.event_accounting_with_context(&path, &event, 0.0, &mut context)?;
                let missing = shares.iter().any(|share| share.category.is_none());
                if !issues.is_empty() {
                    report.issues.extend(issues);
                    if missing {
                        report
                            .missing
                            .push(location(&path, event.selection_range.clone()));
                    }
                    continue;
                }
                missing
            };
            if missing {
                report
                    .missing
                    .push(location(&path, event.selection_range.clone()));
            }
            next.insert(key, (input, missing));
        }
        report.complete &= report.issues.is_empty();
        state.entries = next;
        state.recomputed_events = recomputed;
        Ok(report)
    }
}
