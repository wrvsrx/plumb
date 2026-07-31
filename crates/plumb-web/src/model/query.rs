use super::*;

impl WebWorkspace {
    pub fn query_tasks(&self, query: &WebQuery) -> Result<TaskQuerySnapshot, QueryFailure> {
        let now = Local::now().fixed_offset();
        let preset_groups = resolve_presets(&query.presets, TASK_PRESETS)?;
        let mut retained: Option<BTreeSet<(String, usize)>> = None;
        for group in preset_groups {
            let mut matching = BTreeSet::new();
            for (source, expression) in group.expressions {
                let records = self
                    .workspace
                    .search_records_filtered(
                        &self.root,
                        Some(SearchRecordKind::Task),
                        "",
                        usize::MAX,
                        now,
                        Some(expression),
                    )
                    .map_err(|message| QueryFailure { source, message })?;
                matching.extend(
                    records
                        .items
                        .into_iter()
                        .map(|record| (record.relative_path, record.range.start)),
                );
            }
            intersect_keys(&mut retained, matching);
        }
        for (source, expression) in custom_filters(query) {
            let records = self
                .workspace
                .search_records_filtered(
                    &self.root,
                    Some(SearchRecordKind::Task),
                    "",
                    usize::MAX,
                    now,
                    Some(expression),
                )
                .map_err(|message| QueryFailure { source, message })?;
            intersect_keys(
                &mut retained,
                records
                    .items
                    .into_iter()
                    .map(|record| (record.relative_path, record.range.start)),
            );
        }

        let all_tasks = self.tasks().tasks;
        let mut tasks = all_tasks.clone();
        let mut scores = HashMap::new();
        let retained = tasks
            .iter()
            .filter_map(|task| {
                let key = (task.path.clone(), task.location.start);
                if retained.as_ref().is_some_and(|items| !items.contains(&key)) {
                    return None;
                }
                let score = search_score(
                    &query.query,
                    &[
                        &task.title,
                        task.id.as_deref().unwrap_or_default(),
                        &task.path,
                    ],
                );
                if let Some(score) = score {
                    scores.insert(task.key.clone(), score);
                    Some(task.key.clone())
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        tasks.retain(|task| retained.contains(&task.key));
        sort_task_tree(&mut tasks, &query.sort, &scores);
        let limit = query.limit.unwrap_or(usize::MAX);
        let complete = tasks.len() <= limit;
        truncate_complete_task_documents(&mut tasks, limit, |task| &task.path);
        Ok(TaskQuerySnapshot {
            revision: self.revision,
            tasks,
            all_tasks,
            complete,
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
        let mut graph = self.graph_with_excluded(&query.traversal, &excluded, false);
        let mut program_groups = resolve_presets(&query.presets, GRAPH_PRESETS)?
            .into_iter()
            .map(|group| compile_program_group(group.expressions))
            .collect::<Result<Vec<_>, _>>()?;
        program_groups.extend(
            custom_filters(query)
                .map(|expression| compile_program_group(vec![expression]))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let metrics = self.graph_metrics(&graph);
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

    fn graph_metrics(&self, graph: &GraphSnapshot) -> HashMap<String, GraphMetric> {
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
        for task in self.tasks().tasks {
            if let Some(metric) = metrics.get_mut(&task.document_id) {
                metric.task_count += 1;
                if matches!(task.state.as_str(), "ready" | "waiting") {
                    metric.open_task_count += 1;
                }
            }
        }
        metrics
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

fn intersect_keys<T: Ord>(retained: &mut Option<BTreeSet<T>>, values: impl IntoIterator<Item = T>) {
    let values = values.into_iter().collect::<BTreeSet<_>>();
    match retained {
        Some(retained) => retained.retain(|value| values.contains(value)),
        None => *retained = Some(values),
    }
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
        priority: task.priority,
        due: task
            .due
            .as_deref()
            .and_then(|due| chrono::DateTime::parse_from_rfc3339(due).ok()),
        relevance: scores.get(&task.key).copied(),
    });
}
