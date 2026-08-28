use plumb_semantics::analyze_document;
use plumb_syntax::parse;

pub fn semantic_store_workload(
    events: usize,
    references: usize,
    title_suffix: &str,
) -> (String, String) {
    let target = "`= title|Target\n\n`- Target\n\n `+ task\n\n `@ target\n".to_string();
    let mut source = String::with_capacity(events * 90 + references * 55);
    source.push_str("`= title|Migrated events\n`= timezone|Z\n\n");
    for index in 0..events {
        let day = index % 28 + 1;
        let hour = index % 24;
        source.push_str(&format!(
            "`- 2026-08-{day:02}T{hour:02}:00|Event {index}{title_suffix}\n\n `+ event\n\n `@ event-{index}\n\n"
        ));
    }
    for _ in 0..references {
        source.push_str("See `->[target|target.plumb#target].\n");
    }

    assert_fixture(&target, &source, events, references);
    (target, source)
}

fn assert_fixture(target: &str, source: &str, events: usize, references: usize) {
    let target = parse(target);
    assert!(
        target.diagnostics.is_empty(),
        "target profiling fixture must strict-parse: {:?}",
        target.diagnostics
    );
    let target_output = analyze_document(target.valid_syntax().unwrap());
    assert_eq!(target_output.tasks.tasks.len(), 1);

    let source = parse(source);
    assert!(
        source.diagnostics.is_empty(),
        "source profiling fixture must strict-parse: {:?}",
        source.diagnostics
    );
    let source_output = analyze_document(source.valid_syntax().unwrap());
    assert_eq!(source_output.events.events.len(), events);
    assert_eq!(source_output.links.len(), references);
}
