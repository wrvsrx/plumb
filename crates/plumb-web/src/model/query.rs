use super::*;

impl WebWorkspace {
    pub fn query_tasks(&self, query: &WebQuery) -> Result<TaskQuerySnapshot, QueryFailure> {
        let now = Local::now().fixed_offset();
        let mut filter_groups = resolve_presets(&query.presets, TASK_PRESETS)?
            .into_iter()
            .map(|group| TaskQueryFilterGroup {
                filters: group
                    .expressions
                    .into_iter()
                    .map(|(source, expression)| TaskQueryFilter {
                        source,
                        expression: expression.to_string(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        filter_groups.extend(custom_filters(query).map(|(source, expression)| {
            TaskQueryFilterGroup {
                filters: vec![TaskQueryFilter {
                    source,
                    expression: expression.to_string(),
                }],
            }
        }));
        let mut sort = Vec::new();
        for order in &query.sort {
            let order = match order {
                QuerySort::Source => TaskSortOrder::Source,
                QuerySort::Priority => TaskSortOrder::Priority,
                QuerySort::Due => TaskSortOrder::Due,
                QuerySort::Relevance => TaskSortOrder::Relevance,
            };
            if !sort.contains(&order) {
                sort.push(order);
            }
        }
        let page = self
            .workspace
            .query_task_page(&TaskPageQuery {
                root: self.root.clone(),
                text: query.query.clone(),
                filter_groups,
                sort,
                limit: query.limit.unwrap_or(usize::MAX),
                cursor: query.cursor.clone(),
                workspace_revision: self.revision,
                now,
            })
            .map_err(task_query_failure)?
            .value;
        let mut tasks = page
            .tasks
            .into_iter()
            .filter_map(|task| self.web_task(task))
            .collect::<Vec<_>>();
        assign_task_parents(&mut tasks);
        Ok(TaskQuerySnapshot {
            revision: self.revision,
            tasks,
            all_tasks: Vec::new(),
            complete: page.complete,
            next_cursor: page.next_cursor,
            documents: self.task_documents().map_err(|message| QueryFailure {
                source: "workspace".to_string(),
                message,
            })?,
        })
    }

    fn web_task(&self, item: WorkspaceTask) -> Option<WebTask> {
        let document_id = self.document_id(&item.path)?.to_string();
        let task = item.task;
        let id = task.id.as_ref().map(|field| field.value.clone());
        let key = id.as_ref().map_or_else(
            || format!("{document_id}:{}", task.range.start),
            |id| format!("{document_id}:{id}"),
        );
        let locator = id.as_ref().map_or_else(
            || WebTaskLocator::Offset {
                offset: task.range.start,
            },
            |id| WebTaskLocator::Id { id: id.clone() },
        );
        Some(WebTask {
            key,
            document_id,
            title: task.title.clone(),
            path: display_path(&self.root, &item.path),
            revision: item.revision.to_string(),
            id,
            locator,
            state: item.state.as_str().to_string(),
            created: task.created.as_ref().map(|field| field.value.clone()),
            due: task.due.as_ref().map(|field| field.value.clone()),
            priority: task.priority,
            effective_priority: item.effective_priority,
            wait: task.wait.as_ref().map(|field| field.value.clone()),
            done: task.done.as_ref().map(|field| field.value.clone()),
            canceled: task.canceled.as_ref().map(|field| field.value.clone()),
            recur: task.recur.as_ref().map(|field| field.value.clone()),
            prev: task.prev.as_ref().map(|field| field.value.clone()),
            prev_on: item
                .previous
                .as_ref()
                .map(|target| display_task_ref(&self.root, target)),
            depends: task
                .depends
                .iter()
                .map(|dependency| dependency.source.clone())
                .collect(),
            depends_on: item
                .depends_on
                .iter()
                .map(|target| display_task_ref(&self.root, target))
                .collect(),
            directly_blocking: item
                .directly_blocking
                .iter()
                .map(|target| display_task_ref(&self.root, target))
                .collect(),
            blocked: item.blocked,
            actionable: item.actionable,
            wait_reasons: item
                .wait_reasons
                .into_iter()
                .map(|reason| reason.as_str().to_string())
                .collect(),
            depth: task.depth,
            parent_key: None,
            location: SourceLocation::new(&self.root, &item.path, task.selection_range),
        })
    }

    pub fn query_graph(
        &self,
        query: &WebQuery,
        excluded: Option<&str>,
    ) -> Result<GraphSnapshot, QueryFailure> {
        let excluded = self
            .excluded_documents(excluded)
            .map_err(|message| QueryFailure {
                source: "exclude".to_string(),
                message,
            })?;
        let mut graph = self
            .graph_with_excluded(&query.traversal, &excluded, false)
            .map_err(|message| QueryFailure {
                source: "workspace".to_string(),
                message,
            })?;
        let mut program_groups = resolve_presets(&query.presets, GRAPH_PRESETS)?
            .into_iter()
            .map(|group| compile_program_group(group.expressions))
            .collect::<Result<Vec<_>, _>>()?;
        program_groups.extend(
            custom_filters(query)
                .map(|expression| compile_program_group(vec![expression]))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let metrics = self.graph_metrics(&graph).map_err(|message| QueryFailure {
            source: "workspace".to_string(),
            message,
        })?;
        let mut scores = HashMap::new();
        let mut retained = Vec::new();
        'nodes: for node in std::mem::take(&mut graph.nodes) {
            let Some(score) = search_score(
                &query.query,
                &[&node.title, node.path.as_deref().unwrap_or_default()],
            ) else {
                continue;
            };
            let metric = metrics.get(&node.id).expect("every graph node has metrics");
            for group in &program_groups {
                let mut matches = false;
                for (source, program) in group {
                    match graph_node_matches(program, &node, metric) {
                        Ok(true) => matches = true,
                        Ok(false) => {}
                        Err(message) => {
                            return Err(QueryFailure {
                                source: source.clone(),
                                message,
                            })
                        }
                    }
                }
                if !matches {
                    continue 'nodes;
                }
            }
            scores.insert(node.id.clone(), score);
            retained.push(node);
        }
        graph.nodes = retained;
        let visible = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        graph.edges.retain(|edge| {
            visible.contains(edge.source.as_str()) && visible.contains(edge.target.as_str())
        });
        graph.nodes.sort_by(
            |left, right| match query.sort.first().copied().unwrap_or_default() {
                QuerySort::Relevance if !query.query.is_empty() => scores
                    .get(&right.id)
                    .cmp(&scores.get(&left.id))
                    .then_with(|| graph_source_order(left, right)),
                _ => graph_source_order(left, right),
            },
        );
        let limit = query
            .limit
            .unwrap_or(DEFAULT_GRAPH_LIMIT)
            .min(MAX_GRAPH_LIMIT);
        graph.complete &= graph.nodes.len() <= limit;
        graph.nodes.truncate(limit);
        let visible = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        graph.edges.retain(|edge| {
            visible.contains(edge.source.as_str()) && visible.contains(edge.target.as_str())
        });
        Ok(graph)
    }

    fn graph_metrics(&self, graph: &GraphSnapshot) -> Result<HashMap<String, GraphMetric>, String> {
        let mut metrics = graph
            .nodes
            .iter()
            .map(|node| (node.id.clone(), GraphMetric::default()))
            .collect::<HashMap<_, _>>();
        for edge in &graph.edges {
            if let Some(metric) = metrics.get_mut(&edge.source) {
                metric.outgoing += 1;
                metric.degree += 1;
            }
            if let Some(metric) = metrics.get_mut(&edge.target) {
                metric.incoming += 1;
                metric.degree += 1;
            }
        }
        for summary in self
            .workspace
            .task_document_metrics()
            .map_err(|error| error.to_string())?
            .value
        {
            if let Some(metric) = self
                .document_id(&summary.path)
                .and_then(|document_id| metrics.get_mut(document_id))
            {
                metric.task_count += summary.tasks;
                metric.open_task_count += summary.open_tasks;
            }
        }
        Ok(metrics)
    }
}

fn task_query_failure(error: TaskPageQueryError) -> QueryFailure {
    match error {
        TaskPageQueryError::Filter { source, message } => QueryFailure { source, message },
        TaskPageQueryError::Cursor(message) => QueryFailure {
            source: "cursor".to_string(),
            message,
        },
        TaskPageQueryError::Query(error) => QueryFailure {
            source: "workspace".to_string(),
            message: error.to_string(),
        },
    }
}

fn custom_filters(query: &WebQuery) -> impl Iterator<Item = (String, &str)> {
    let mut filters = query
        .filters
        .iter()
        .map(String::as_str)
        .filter(|expression| !expression.trim().is_empty())
        .enumerate()
        .map(|(index, expression)| (format!("custom:{}", index + 1), expression))
        .collect::<Vec<_>>();
    if !query.filter.trim().is_empty() {
        filters.push(("custom".to_string(), query.filter.as_str()));
    }
    filters.into_iter()
}

#[derive(Debug, Clone, Default)]
struct GraphMetric {
    degree: usize,
    incoming: usize,
    outgoing: usize,
    task_count: usize,
    open_task_count: usize,
}

struct ResolvedPresetGroup<'a> {
    key: String,
    expressions: Vec<(String, &'a str)>,
}

fn resolve_presets<'a>(
    ids: &[String],
    registry: &'a [QueryPreset],
) -> Result<Vec<ResolvedPresetGroup<'a>>, QueryFailure> {
    let mut groups = Vec::<ResolvedPresetGroup<'a>>::new();
    for id in ids {
        let preset = registry
            .iter()
            .find(|preset| preset.id == id)
            .ok_or_else(|| QueryFailure {
                source: format!("preset:{id}"),
                message: format!("unknown query preset '{id}'"),
            })?;
        let key = preset.group.unwrap_or(preset.id).to_string();
        let expression = (format!("preset:{id}"), preset.expression);
        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.expressions.push(expression);
        } else {
            groups.push(ResolvedPresetGroup {
                key,
                expressions: vec![expression],
            });
        }
    }
    Ok(groups)
}

fn compile_program_group(
    expressions: Vec<(String, &str)>,
) -> Result<Vec<(String, Program)>, QueryFailure> {
    expressions
        .into_iter()
        .map(|(source, expression)| {
            Program::compile(expression)
                .map(|program| (source.clone(), program))
                .map_err(|error| QueryFailure {
                    source,
                    message: format!("invalid CEL query: {error}"),
                })
        })
        .collect()
}

fn graph_node_matches(
    program: &Program,
    node: &GraphNode,
    metric: &GraphMetric,
) -> Result<bool, String> {
    let mut context = Context::default();
    context.add_variable_from_value(
        "path",
        node.path
            .clone()
            .map_or(Value::Null, |value| Value::String(value.into())),
    );
    context.add_variable_from_value("title", node.title.clone());
    context.add_variable_from_value("unresolved", node.unresolved);
    context.add_variable_from_value("degree", i64::try_from(metric.degree).unwrap_or(i64::MAX));
    context.add_variable_from_value(
        "incoming",
        i64::try_from(metric.incoming).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "outgoing",
        i64::try_from(metric.outgoing).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "task_count",
        i64::try_from(metric.task_count).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "open_task_count",
        i64::try_from(metric.open_task_count).unwrap_or(i64::MAX),
    );
    match program.execute(&context) {
        Ok(Value::Bool(value)) => Ok(value),
        Ok(value) => Err(format!("CEL query must return bool, got {value:?}")),
        Err(error) => Err(format!(
            "cannot evaluate CEL query for '{}': {error}",
            node.title
        )),
    }
}

fn graph_source_order(left: &GraphNode, right: &GraphNode) -> std::cmp::Ordering {
    left.path
        .as_deref()
        .unwrap_or("\u{10ffff}")
        .cmp(right.path.as_deref().unwrap_or("\u{10ffff}"))
        .then(left.id.cmp(&right.id))
}

pub(super) fn task_source_order(left: &WebTask, right: &WebTask) -> std::cmp::Ordering {
    left.path
        .cmp(&right.path)
        .then(left.location.start.cmp(&right.location.start))
        .then(left.key.cmp(&right.key))
}

pub(super) fn assign_task_parents(tasks: &mut [WebTask]) {
    let mut path = None::<String>;
    let mut ancestors = Vec::<String>::new();
    for task in tasks {
        if path.as_deref() != Some(task.path.as_str()) {
            path = Some(task.path.clone());
            ancestors.clear();
        }
        ancestors.truncate(task.depth);
        ancestors.resize(task.depth, String::new());
        task.parent_key = task
            .depth
            .checked_sub(1)
            .and_then(|depth| ancestors.get(depth).filter(|key| !key.is_empty()).cloned());
        ancestors.push(task.key.clone());
    }
}

pub(super) fn propagate_task_priorities(tasks: &mut [WebTask]) {
    let nodes_by_key = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| (task.key.clone(), index))
        .collect::<HashMap<_, _>>();
    let nodes_by_ref = tasks
        .iter()
        .enumerate()
        .filter_map(|(index, task)| {
            task.id
                .as_ref()
                .map(|id| (format!("{}#{id}", task.path), index))
        })
        .collect::<HashMap<_, _>>();
    let mut edges = Vec::new();
    for (source, task) in tasks.iter().enumerate() {
        if let Some(target) = task
            .parent_key
            .as_ref()
            .and_then(|key| nodes_by_key.get(key))
        {
            edges.push((source, *target));
        }
        edges.extend(
            task.depends_on
                .iter()
                .filter_map(|target| nodes_by_ref.get(target).copied())
                .map(|target| (source, target)),
        );
    }
    let mut priorities = tasks
        .iter()
        .map(|task| task.priority.unwrap_or_default())
        .collect::<Vec<_>>();
    loop {
        let mut changed = false;
        for &(source, target) in &edges {
            if priorities[source] > priorities[target] {
                priorities[target] = priorities[source];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (task, priority) in tasks.iter_mut().zip(priorities) {
        task.effective_priority = priority;
    }
}

pub(super) fn sort_task_tree(
    tasks: &mut Vec<WebTask>,
    sorts: &[QuerySort],
    scores: &HashMap<String, i64>,
) {
    let mut orders = Vec::new();
    for sort in sorts {
        let order = match sort {
            QuerySort::Source => TaskSortOrder::Source,
            QuerySort::Priority => TaskSortOrder::Priority,
            QuerySort::Due => TaskSortOrder::Due,
            QuerySort::Relevance => TaskSortOrder::Relevance,
        };
        if !orders.contains(&order) {
            orders.push(order);
        }
    }
    sort_task_records_by(tasks, &orders, |task| TaskSortFacts {
        document: task.path.clone(),
        source_start: task.location.start,
        depth: task.depth,
        priority: Some(task.effective_priority),
        due: task
            .due
            .as_deref()
            .and_then(|due| chrono::DateTime::parse_from_rfc3339(due).ok()),
        relevance: scores.get(&task.key).copied(),
    });
}
