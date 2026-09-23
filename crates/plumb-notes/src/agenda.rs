use super::{AgendaConfig, AgendaGroup, LoadedWorkspace};

pub(super) fn run(
    loaded: &LoadedWorkspace,
    filter: Option<&str>,
    options: &AgendaConfig,
    accounting: bool,
) -> Result<u8, String> {
    let report = loaded.workspace.agenda_report(
        &loaded.root,
        options.from,
        options.to,
        loaded.now,
        filter,
        accounting,
    )?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!("{} -- {}", report.from.to_rfc3339(), report.to.to_rfc3339());
        println!("filter: {}", filter.unwrap_or("all events"));
        if accounting {
            println!(
                "seconds\t{}",
                match options.group_by {
                    AgendaGroup::Category => "category",
                    AgendaGroup::Item => "item",
                    AgendaGroup::Task => "task",
                }
            );
            match options.group_by {
                AgendaGroup::Category => {
                    for row in &report.categories {
                        println!(
                            "{:.6}\t{}",
                            row.seconds,
                            row.category.as_deref().unwrap_or("(uncategorized)")
                        );
                    }
                }
                AgendaGroup::Item | AgendaGroup::Task => {
                    let rows = if matches!(options.group_by, AgendaGroup::Task) {
                        &report.tasks
                    } else {
                        &report.items
                    };
                    for row in rows {
                        println!(
                            "{:.6}\t{}{}",
                            row.seconds,
                            row.item.path.display(),
                            row.item
                                .id
                                .as_ref()
                                .map(|id| format!("#{id}"))
                                .unwrap_or_default()
                        );
                    }
                }
            }
        }
        println!(
            "accumulated: {:.6}s; covered: {:.6}s; point events: {}",
            report.accumulated_seconds,
            report.covered_seconds,
            report.points.len()
        );
        for (kind, segments) in [("gap", &report.gaps), ("overlap", &report.overlaps)] {
            for segment in segments {
                println!(
                    "{kind}\t{} -- {}",
                    segment.start.to_rfc3339(),
                    segment.end.to_rfc3339()
                );
                for event in &segment.events {
                    println!(
                        "  {}:{}..{}",
                        event.path.display(),
                        event.range.start,
                        event.range.end
                    );
                }
            }
        }
        for issue in &report.issues {
            println!(
                "{}\t{}:{}\t{}",
                issue.code,
                issue.source.path.display(),
                issue.source.range.start,
                issue.message
            );
        }
        if !report.complete {
            println!("incomplete");
        } else if !accounting {
            println!(
                "{}",
                if report.timeline_passed() {
                    "passed"
                } else {
                    "coverage failed"
                }
            );
        }
    }
    Ok(if !report.complete {
        2
    } else if !accounting && !report.timeline_passed() {
        1
    } else {
        0
    })
}

pub(super) fn check_category(
    loaded: &LoadedWorkspace,
    filter: Option<&str>,
    options: &super::CategoryCheckConfig,
) -> Result<u8, String> {
    let report = loaded.workspace.check_event_categories(
        &loaded.root,
        loaded.now,
        filter,
        options.explicit,
    )?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "checked: {}; missing: {}",
            report.checked,
            report.missing.len()
        );
        for source in &report.missing {
            println!(
                "missing-category\t{}:{}..{}",
                source.path.display(),
                source.range.start,
                source.range.end
            );
        }
        for issue in &report.issues {
            println!(
                "{}\t{}:{}\t{}",
                issue.code,
                issue.source.path.display(),
                issue.source.range.start,
                issue.message
            );
        }
        if !report.complete {
            println!("incomplete");
        }
    }
    Ok(if !report.complete {
        2
    } else if !report.missing.is_empty() {
        1
    } else {
        0
    })
}
